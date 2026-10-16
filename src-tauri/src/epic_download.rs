//! Epic Games Store downloads that Orivo performs itself.
//!
//! On a desktop Orivo asks the Epic Games Launcher to install a game and reads
//! the launcher's own records afterwards (`epic_install.rs`). A phone has no
//! launcher, so there Orivo fetches a game's Windows build itself, for Winlator
//! to run (`epic_android.rs`). This module is the platform-independent half:
//! the two file formats Epic's content delivery speaks, and the download that
//! turns them back into a game folder.
//!
//! How a build is delivered:
//!
//! - The launcher service answers, for one owned game, with the URL of a
//!   **manifest** (signed for a few minutes) and the SHA-1 of its bytes.
//! - The manifest lists every file of the build as a run of slices of
//!   **chunks** — 1 MiB windows of data shared across files and across builds —
//!   and names each chunk by group, rolling hash and GUID.
//! - Each chunk is a small file on Epic's CDN, next to the manifest: a header,
//!   then the window, zlib-compressed.
//!
//! Written from the published shape of these formats, not from any existing
//! client: Orivo's licence keeps copyleft code out (CONTRIBUTING.md), and the
//! formats themselves are facts about Epic's service, not anyone's code.

use flate2::read::ZlibDecoder;
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeMap, HashMap},
    fmt, fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

const MANIFEST_MAGIC: u32 = 0x44BE_C00C;
const CHUNK_MAGIC: u32 = 0xB1FE_3AA2;

/// The flag bits a manifest and a chunk header use for how their payload is
/// stored. Encrypted payloads exist in the format and in no shipped build Orivo
/// can find; they are refused rather than written out as noise.
const STORED_COMPRESSED: u8 = 0x01;
const STORED_ENCRYPTED: u8 = 0x02;

/// Bounds on what an answer from the CDN may make Orivo allocate. A real
/// manifest stays far below every one of them: the largest builds on the store
/// list a few hundred thousand chunks and files.
const MAX_MANIFEST_BYTES: usize = 256 * 1024 * 1024;
const MAX_ENTRIES: u32 = 4_000_000;
const MAX_STRING_UNITS: i32 = 64 * 1024;
const MAX_PARTS_PER_FILE: u32 = 1_000_000;
/// A chunk is a 1 MiB window in every build Epic serves; 64 MiB leaves room for
/// any future window size without letting a forged header ask for gigabytes.
const MAX_CHUNK_WINDOW: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum EpicDownloadError {
    /// The manifest or a chunk does not parse as the format it claims to be.
    Format(String),
    /// A digest disagrees with the one the manifest promised.
    Integrity(String),
    /// A manifest lists a path that would land outside the game's folder.
    UnsafePath(String),
    Unsupported(String),
    Network(String),
    Io(String),
    NoSpace { needed: u64, available: u64 },
    Cancelled,
}

impl fmt::Display for EpicDownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(detail) => write!(f, "Epic sent a download Orivo cannot read ({detail})."),
            Self::Integrity(detail) => {
                write!(f, "Part of the download arrived damaged ({detail}). Try again.")
            }
            Self::UnsafePath(path) => {
                write!(f, "Epic's file list names a path Orivo will not write: {path}")
            }
            Self::Unsupported(detail) => write!(f, "{detail}"),
            Self::Network(detail) => write!(f, "The download stopped: {detail}"),
            Self::Io(detail) => write!(f, "The game could not be written to this device: {detail}"),
            Self::NoSpace { needed, available } => write!(
                f,
                "This game needs {} free and this device has {}.",
                human_bytes(*needed),
                human_bytes(*available)
            ),
            Self::Cancelled => write!(f, "The download was stopped."),
        }
    }
}

impl std::error::Error for EpicDownloadError {}

impl From<std::io::Error> for EpicDownloadError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

pub fn human_bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut amount = value as f64;
    let mut unit = 0;
    while amount >= 1000.0 && unit + 1 < UNITS.len() {
        amount /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{amount:.1} {}", UNITS[unit])
    }
}

// ---------------------------------------------------------------------------
// The manifest
// ---------------------------------------------------------------------------

/// A chunk's identity: four little-endian words, written as 32 upper-case hex
/// digits wherever Epic prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Guid(pub [u32; 4]);

impl Guid {
    pub fn hex(&self) -> String {
        format!(
            "{:08X}{:08X}{:08X}{:08X}",
            self.0[0], self.0[1], self.0[2], self.0[3]
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkInfo {
    pub guid: Guid,
    pub rolling_hash: u64,
    pub sha1: [u8; 20],
    pub group: u8,
    /// Uncompressed size of the window.
    pub window_size: u32,
    /// Size of the chunk file on the CDN — what downloading it costs.
    pub file_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPart {
    pub guid: Guid,
    pub offset: u32,
    pub size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFile {
    /// Relative to the game's folder, with `/` separators.
    pub name: String,
    pub sha1: [u8; 20],
    pub parts: Vec<ChunkPart>,
    pub symlink_target: String,
}

impl ManifestFile {
    pub fn size(&self) -> u64 {
        self.parts.iter().map(|part| u64::from(part.size)).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub feature_level: u32,
    pub app_name: String,
    pub build_version: String,
    /// The executable the launcher starts, relative to the game's folder.
    pub launch_exe: String,
    /// Arguments the launcher always passes to it.
    pub launch_command: String,
    pub chunks: Vec<ChunkInfo>,
    pub files: Vec<ManifestFile>,
}

impl Manifest {
    /// Where the chunks of a build live next to its manifest. The folder name
    /// follows the manifest's feature level: each one marks a change to the
    /// chunk header that older clients could not read.
    pub fn chunk_directory(&self) -> &'static str {
        match self.feature_level {
            15.. => "ChunksV4",
            6..=14 => "ChunksV3",
            3..=5 => "ChunksV2",
            _ => "Chunks",
        }
    }

    pub fn chunk_path(&self, chunk: &ChunkInfo) -> String {
        format!(
            "{}/{:02}/{:016X}_{}.chunk",
            self.chunk_directory(),
            chunk.group,
            chunk.rolling_hash,
            chunk.guid.hex()
        )
    }

    pub fn install_size(&self) -> u64 {
        self.files.iter().map(ManifestFile::size).sum()
    }
}

/// A little-endian cursor that never reads past its slice and never allocates
/// more than the bounds above allow.
struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], EpicDownloadError> {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| EpicDownloadError::Format("the file ends early".into()))?;
        let slice = &self.data[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn seek(&mut self, position: usize) -> Result<(), EpicDownloadError> {
        if position > self.data.len() {
            return Err(EpicDownloadError::Format("a section runs past the end".into()));
        }
        self.position = position;
        Ok(())
    }

    fn u8(&mut self) -> Result<u8, EpicDownloadError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, EpicDownloadError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    fn i32(&mut self) -> Result<i32, EpicDownloadError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    fn u64(&mut self) -> Result<u64, EpicDownloadError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    fn i64(&mut self) -> Result<i64, EpicDownloadError> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    fn sha1(&mut self) -> Result<[u8; 20], EpicDownloadError> {
        Ok(self.take(20)?.try_into().expect("twenty bytes"))
    }

    fn guid(&mut self) -> Result<Guid, EpicDownloadError> {
        Ok(Guid([self.u32()?, self.u32()?, self.u32()?, self.u32()?]))
    }

    fn count(&mut self) -> Result<u32, EpicDownloadError> {
        let count = self.u32()?;
        if count > MAX_ENTRIES {
            return Err(EpicDownloadError::Format(format!("{count} entries in one list")));
        }
        Ok(count)
    }

    /// Unreal's `FString`: a signed length that counts the terminating NUL. A
    /// negative length means UTF-16, a positive one a byte string.
    fn fstring(&mut self) -> Result<String, EpicDownloadError> {
        let length = self.i32()?;
        if length == 0 {
            return Ok(String::new());
        }
        if !(-MAX_STRING_UNITS..=MAX_STRING_UNITS).contains(&length) {
            return Err(EpicDownloadError::Format("a name is implausibly long".into()));
        }
        if length < 0 {
            let units = length.unsigned_abs() as usize;
            let bytes = self.take(units * 2)?;
            let mut text: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            while text.last() == Some(&0) {
                text.pop();
            }
            return Ok(String::from_utf16_lossy(&text));
        }
        let mut bytes = self.take(length as usize)?.to_vec();
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Parse a manifest as the CDN serves it: the binary format every current
/// build uses, or the JSON one older builds still answer with.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, EpicDownloadError> {
    if bytes.len() >= 4 && u32::from_le_bytes(bytes[..4].try_into().expect("four")) == MANIFEST_MAGIC
    {
        return parse_binary_manifest(bytes);
    }
    if bytes.first().is_some_and(|byte| *byte == b'{') {
        return parse_json_manifest(bytes);
    }
    Err(EpicDownloadError::Format("not a manifest".into()))
}

fn parse_binary_manifest(bytes: &[u8]) -> Result<Manifest, EpicDownloadError> {
    let mut header = Reader::new(bytes);
    let _magic = header.u32()?;
    let header_size = header.u32()? as usize;
    let size_uncompressed = header.u32()? as usize;
    let size_compressed = header.u32()? as usize;
    let digest = header.sha1()?;
    let stored_as = header.u8()?;
    let version = header.u32()?;

    if size_uncompressed > MAX_MANIFEST_BYTES {
        return Err(EpicDownloadError::Format("the manifest is implausibly large".into()));
    }
    if stored_as & STORED_ENCRYPTED != 0 {
        return Err(EpicDownloadError::Unsupported(
            "This build is encrypted, which Orivo cannot install.".into(),
        ));
    }
    header.seek(header_size)?;
    let body: Vec<u8> = if stored_as & STORED_COMPRESSED != 0 {
        let compressed = header.take(size_compressed)?;
        inflate(compressed, size_uncompressed)?
    } else {
        header.take(size_uncompressed)?.to_vec()
    };
    if body.len() != size_uncompressed {
        return Err(EpicDownloadError::Format("the manifest's size disagrees with its header".into()));
    }
    if digest != [0; 20] && sha1_of(&body) != digest {
        return Err(EpicDownloadError::Integrity("manifest".into()));
    }

    let mut reader = Reader::new(&body);

    // ManifestMeta
    let start = reader.position;
    let meta_size = reader.u32()? as usize;
    let data_version = reader.u8()?;
    let feature_level = reader.u32()?;
    let _is_file_data = reader.u8()?;
    let _app_id = reader.u32()?;
    let app_name = reader.fstring()?;
    let build_version = reader.fstring()?;
    let launch_exe = reader.fstring()?;
    let launch_command = reader.fstring()?;
    let prerequisite_count = reader.count()?;
    for _ in 0..prerequisite_count {
        reader.fstring()?;
    }
    let _prerequisite_name = reader.fstring()?;
    let _prerequisite_path = reader.fstring()?;
    let _prerequisite_args = reader.fstring()?;
    if data_version >= 1 {
        let _build_id = reader.fstring()?;
    }
    reader.seek(start + meta_size)?;

    // ChunkDataList: one array per field, each `count` long.
    let start = reader.position;
    let list_size = reader.u32()? as usize;
    let _list_version = reader.u8()?;
    let count = reader.count()? as usize;
    let guids = (0..count).map(|_| reader.guid()).collect::<Result<Vec<_>, _>>()?;
    let hashes = (0..count).map(|_| reader.u64()).collect::<Result<Vec<_>, _>>()?;
    let digests = (0..count).map(|_| reader.sha1()).collect::<Result<Vec<_>, _>>()?;
    let groups = (0..count).map(|_| reader.u8()).collect::<Result<Vec<_>, _>>()?;
    let windows = (0..count).map(|_| reader.u32()).collect::<Result<Vec<_>, _>>()?;
    let file_sizes = (0..count).map(|_| reader.i64()).collect::<Result<Vec<_>, _>>()?;
    reader.seek(start + list_size)?;
    let chunks = (0..count)
        .map(|index| ChunkInfo {
            guid: guids[index],
            rolling_hash: hashes[index],
            sha1: digests[index],
            group: groups[index],
            window_size: windows[index],
            file_size: file_sizes[index].max(0) as u64,
        })
        .collect::<Vec<_>>();

    // FileManifestList, again one array per field.
    let start = reader.position;
    let list_size = reader.u32()? as usize;
    let _list_version = reader.u8()?;
    let count = reader.count()? as usize;
    let names = (0..count).map(|_| reader.fstring()).collect::<Result<Vec<_>, _>>()?;
    let symlinks = (0..count).map(|_| reader.fstring()).collect::<Result<Vec<_>, _>>()?;
    let file_digests = (0..count).map(|_| reader.sha1()).collect::<Result<Vec<_>, _>>()?;
    let _flags = (0..count).map(|_| reader.u8()).collect::<Result<Vec<_>, _>>()?;
    for _ in 0..count {
        let tags = reader.count()?;
        for _ in 0..tags {
            reader.fstring()?;
        }
    }
    let mut part_lists = Vec::with_capacity(count);
    for _ in 0..count {
        let part_count = reader.u32()?;
        if part_count > MAX_PARTS_PER_FILE {
            return Err(EpicDownloadError::Format("a file is cut into too many parts".into()));
        }
        let mut parts = Vec::with_capacity(part_count as usize);
        for _ in 0..part_count {
            let part_start = reader.position;
            let part_size = reader.u32()? as usize;
            let guid = reader.guid()?;
            let offset = reader.u32()?;
            let size = reader.u32()?;
            parts.push(ChunkPart { guid, offset, size });
            // A part may grow fields in a later version; its own size says
            // where the next one starts.
            reader.seek(part_start + part_size.max(28))?;
        }
        part_lists.push(parts);
    }
    reader.seek(start + list_size)?;
    let files = names
        .into_iter()
        .zip(symlinks)
        .zip(file_digests)
        .zip(part_lists)
        .map(|(((name, symlink_target), sha1), parts)| ManifestFile {
            name,
            sha1,
            parts,
            symlink_target,
        })
        .collect::<Vec<_>>();

    let _ = version;
    Ok(Manifest {
        feature_level,
        app_name,
        build_version,
        launch_exe,
        launch_command,
        chunks,
        files,
    })
}

/// The JSON manifest writes every number as a "blob": its little-endian bytes,
/// each as three decimal digits. `"001000000000"` is the `u32` 1.
fn blob_bytes(blob: &str) -> Result<Vec<u8>, EpicDownloadError> {
    if blob.len() % 3 != 0 || !blob.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(EpicDownloadError::Format("a number is not a blob".into()));
    }
    blob.as_bytes()
        .chunks(3)
        .map(|digits| {
            std::str::from_utf8(digits)
                .ok()
                .and_then(|text| text.parse::<u16>().ok())
                .filter(|value| *value <= 255)
                .map(|value| value as u8)
                .ok_or_else(|| EpicDownloadError::Format("a blob byte is out of range".into()))
        })
        .collect()
}

fn blob_u64(blob: &str) -> Result<u64, EpicDownloadError> {
    let bytes = blob_bytes(blob)?;
    if bytes.len() > 8 {
        return Err(EpicDownloadError::Format("a blob is wider than 64 bits".into()));
    }
    Ok(bytes
        .iter()
        .enumerate()
        .fold(0u64, |value, (index, byte)| value | (u64::from(*byte) << (index * 8))))
}

fn parse_guid_text(text: &str) -> Result<Guid, EpicDownloadError> {
    if text.len() != 32 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(EpicDownloadError::Format(format!("{text} is not a chunk id")));
    }
    let word = |index: usize| u32::from_str_radix(&text[index * 8..index * 8 + 8], 16);
    Ok(Guid([
        word(0).expect("hex"),
        word(1).expect("hex"),
        word(2).expect("hex"),
        word(3).expect("hex"),
    ]))
}

fn hex_digest(text: &str) -> Option<[u8; 20]> {
    if text.len() != 40 {
        return None;
    }
    let mut digest = [0u8; 20];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(digest)
}

#[derive(Deserialize)]
struct JsonManifest {
    #[serde(rename = "ManifestFileVersion", default)]
    manifest_file_version: String,
    #[serde(rename = "AppNameString", default)]
    app_name: String,
    #[serde(rename = "BuildVersionString", default)]
    build_version: String,
    #[serde(rename = "LaunchExeString", default)]
    launch_exe: String,
    #[serde(rename = "LaunchCommand", default)]
    launch_command: String,
    #[serde(rename = "FileManifestList", default)]
    files: Vec<JsonFile>,
    #[serde(rename = "ChunkHashList", default)]
    chunk_hashes: BTreeMap<String, String>,
    #[serde(rename = "ChunkShaList", default)]
    chunk_shas: BTreeMap<String, String>,
    #[serde(rename = "DataGroupList", default)]
    groups: BTreeMap<String, String>,
    #[serde(rename = "ChunkFilesizeList", default)]
    file_sizes: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct JsonFile {
    #[serde(rename = "Filename", default)]
    name: String,
    #[serde(rename = "FileHash", default)]
    hash: String,
    #[serde(rename = "SymlinkTarget", default)]
    symlink_target: String,
    #[serde(rename = "FileChunkParts", default)]
    parts: Vec<JsonPart>,
}

#[derive(Deserialize)]
struct JsonPart {
    #[serde(rename = "Guid")]
    guid: String,
    #[serde(rename = "Offset")]
    offset: String,
    #[serde(rename = "Size")]
    size: String,
}

fn parse_json_manifest(bytes: &[u8]) -> Result<Manifest, EpicDownloadError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(EpicDownloadError::Format("the manifest is implausibly large".into()));
    }
    let manifest: JsonManifest = serde_json::from_slice(bytes)
        .map_err(|error| EpicDownloadError::Format(format!("JSON manifest: {error}")))?;
    let feature_level = blob_u64(&manifest.manifest_file_version).unwrap_or(0) as u32;

    let mut chunks = Vec::with_capacity(manifest.chunk_hashes.len());
    for (guid_text, hash_blob) in &manifest.chunk_hashes {
        let group = manifest
            .groups
            .get(guid_text)
            .ok_or_else(|| {
                EpicDownloadError::Unsupported(
                    "This build predates the chunk layout Orivo can download.".into(),
                )
            })
            .and_then(|blob| blob_u64(blob))? as u8;
        chunks.push(ChunkInfo {
            guid: parse_guid_text(guid_text)?,
            rolling_hash: blob_u64(hash_blob)?,
            sha1: manifest
                .chunk_shas
                .get(guid_text)
                .and_then(|text| hex_digest(text))
                .unwrap_or([0; 20]),
            group,
            window_size: 1024 * 1024,
            file_size: manifest
                .file_sizes
                .get(guid_text)
                .map(|blob| blob_u64(blob))
                .transpose()?
                .unwrap_or(0),
        });
    }

    let mut files = Vec::with_capacity(manifest.files.len());
    for file in manifest.files {
        let digest = blob_bytes(&file.hash)?;
        let parts = file
            .parts
            .iter()
            .map(|part| {
                Ok(ChunkPart {
                    guid: parse_guid_text(&part.guid)?,
                    offset: blob_u64(&part.offset)? as u32,
                    size: blob_u64(&part.size)? as u32,
                })
            })
            .collect::<Result<Vec<_>, EpicDownloadError>>()?;
        files.push(ManifestFile {
            name: file.name,
            sha1: digest.try_into().unwrap_or([0; 20]),
            parts,
            symlink_target: file.symlink_target,
        });
    }

    Ok(Manifest {
        feature_level,
        app_name: manifest.app_name,
        build_version: manifest.build_version,
        launch_exe: manifest.launch_exe,
        launch_command: manifest.launch_command,
        chunks,
        files,
    })
}

// ---------------------------------------------------------------------------
// Chunks
// ---------------------------------------------------------------------------

/// Turn a chunk file back into its window, and check it is the window the
/// manifest asked for.
pub fn decode_chunk(bytes: &[u8], expected: &ChunkInfo) -> Result<Vec<u8>, EpicDownloadError> {
    let mut reader = Reader::new(bytes);
    if reader.u32()? != CHUNK_MAGIC {
        return Err(EpicDownloadError::Format("not a chunk".into()));
    }
    let header_version = reader.u32()?;
    let header_size = reader.u32()? as usize;
    let stored_size = reader.u32()? as usize;
    let guid = reader.guid()?;
    let _rolling_hash = reader.u64()?;
    let stored_as = reader.u8()?;
    let mut header_digest = None;
    let mut window_size = u64::from(expected.window_size);
    if header_version >= 2 {
        header_digest = Some(reader.sha1()?);
        let _hash_type = reader.u8()?;
    }
    if header_version >= 3 {
        window_size = u64::from(reader.u32()?);
    }
    if guid != expected.guid {
        return Err(EpicDownloadError::Integrity(format!(
            "chunk {} answered as {}",
            expected.guid.hex(),
            guid.hex()
        )));
    }
    if window_size > MAX_CHUNK_WINDOW {
        return Err(EpicDownloadError::Format("a chunk claims an implausible size".into()));
    }
    if stored_as & STORED_ENCRYPTED != 0 {
        return Err(EpicDownloadError::Unsupported(
            "This build is encrypted, which Orivo cannot install.".into(),
        ));
    }
    reader.seek(header_size)?;
    let payload = reader.take(stored_size)?;
    let window = if stored_as & STORED_COMPRESSED != 0 {
        inflate(payload, window_size as usize)?
    } else {
        payload.to_vec()
    };
    // The manifest's digest is the one that matters; the header's is the same
    // fact repeated, checked only when the manifest has none.
    let wanted = if expected.sha1 != [0; 20] {
        Some(expected.sha1)
    } else {
        header_digest.filter(|digest| *digest != [0; 20])
    };
    if let Some(wanted) = wanted
        && sha1_of(&window) != wanted
    {
        return Err(EpicDownloadError::Integrity(format!("chunk {}", expected.guid.hex())));
    }
    Ok(window)
}

fn inflate(compressed: &[u8], expected_size: usize) -> Result<Vec<u8>, EpicDownloadError> {
    let mut output = Vec::with_capacity(expected_size.min(MAX_CHUNK_WINDOW as usize));
    ZlibDecoder::new(compressed)
        // One byte past the promise is enough to know it was broken.
        .take(expected_size as u64 + 1)
        .read_to_end(&mut output)
        .map_err(|error| EpicDownloadError::Format(format!("compressed data: {error}")))?;
    if output.len() > expected_size {
        return Err(EpicDownloadError::Format("data inflates past its stated size".into()));
    }
    Ok(output)
}

pub fn sha1_of(bytes: &[u8]) -> [u8; 20] {
    Sha1::digest(bytes).into()
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// The file a manifest entry is written to, inside `root`, or a refusal. A
/// manifest comes from Epic's CDN over TLS, but a path is still never trusted
/// to stay where it was meant to: no absolute path, no `..`, no drive letter.
pub fn install_path(root: &Path, name: &str) -> Result<PathBuf, EpicDownloadError> {
    let relative = name.replace('\\', "/");
    let candidate = Path::new(&relative);
    if relative.is_empty()
        || relative.contains(':')
        || relative.contains('\0')
        || candidate.is_absolute()
    {
        return Err(EpicDownloadError::UnsafePath(name.to_string()));
    }
    let mut path = root.to_path_buf();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            _ => return Err(EpicDownloadError::UnsafePath(name.to_string())),
        }
    }
    if path == root {
        return Err(EpicDownloadError::UnsafePath(name.to_string()));
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// Locating a build
// ---------------------------------------------------------------------------

const LAUNCHER_HOST: &str = "https://launcher-public-service-prod06.ol.epicgames.com";

/// Where one build of one owned game can be fetched from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildLocation {
    pub build_version: String,
    /// Full manifest URLs, each with the short-lived signature it was given.
    pub manifest_urls: Vec<String>,
    /// The folders the chunks live in, one per CDN, most preferred first.
    pub chunk_bases: Vec<String>,
    /// SHA-1 of the manifest file as served, when the service names it.
    pub manifest_sha1: Option<[u8; 20]>,
}

#[derive(Deserialize)]
struct ManifestApiAnswer {
    #[serde(default)]
    elements: Vec<ManifestApiElement>,
}

#[derive(Deserialize)]
struct ManifestApiElement {
    #[serde(rename = "buildVersion", default)]
    build_version: String,
    #[serde(default)]
    hash: String,
    #[serde(default)]
    manifests: Vec<ManifestApiUri>,
}

#[derive(Deserialize)]
struct ManifestApiUri {
    uri: String,
    #[serde(rename = "queryParams", default)]
    query_params: Vec<ManifestApiParam>,
}

#[derive(Deserialize)]
struct ManifestApiParam {
    name: String,
    value: String,
}

pub fn manifest_api_url(platform: &str, namespace: &str, catalog_item_id: &str, app_name: &str) -> String {
    let segment = |value: &str| url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>();
    format!(
        "{LAUNCHER_HOST}/launcher/api/public/assets/v2/platform/{}/namespace/{}/catalogItem/{}/app/{}/label/Live",
        segment(platform),
        segment(namespace),
        segment(catalog_item_id),
        segment(app_name)
    )
}

/// Read the service's answer for one game. Only HTTPS locations are kept: a
/// chunk is checked against its digest, but nothing about a game is fetched
/// in the clear.
pub fn parse_build_location(body: &[u8]) -> Result<BuildLocation, EpicDownloadError> {
    let answer: ManifestApiAnswer = serde_json::from_slice(body)
        .map_err(|_| EpicDownloadError::Format("Epic's answer about this build".into()))?;
    let element = answer.elements.into_iter().next().ok_or_else(|| {
        EpicDownloadError::Unsupported("Epic has no Windows build of this game to download.".into())
    })?;
    let mut manifest_urls = Vec::new();
    let mut chunk_bases = Vec::new();
    for manifest in element.manifests {
        let Ok(parsed) = url::Url::parse(&manifest.uri) else {
            continue;
        };
        if parsed.scheme() != "https" {
            continue;
        }
        let base = manifest
            .uri
            .rsplit_once('/')
            .map(|(base, _)| base.to_string())
            .unwrap_or_default();
        if !base.is_empty() && !chunk_bases.contains(&base) {
            chunk_bases.push(base);
        }
        let mut full = parsed;
        if !manifest.query_params.is_empty() {
            let mut pairs = full.query_pairs_mut();
            for param in &manifest.query_params {
                pairs.append_pair(&param.name, &param.value);
            }
        }
        manifest_urls.push(full.to_string());
    }
    if manifest_urls.is_empty() {
        return Err(EpicDownloadError::Unsupported(
            "Epic did not say where this build can be downloaded from.".into(),
        ));
    }
    Ok(BuildLocation {
        build_version: element.build_version,
        manifest_urls,
        chunk_bases,
        manifest_sha1: hex_digest(&element.hash.to_ascii_lowercase()),
    })
}

// ---------------------------------------------------------------------------
// Installing a build
// ---------------------------------------------------------------------------

/// Downloads in flight at once. Chunks are small and the CDN is fast; eight
/// keeps a phone's Wi-Fi busy without holding more than a few MiB in memory.
const CONCURRENT_CHUNKS: usize = 8;
/// Windows that are still needed later are kept in memory up to this much,
/// and written to the game's folder past it.
const CACHE_LIMIT_BYTES: usize = 128 * 1024 * 1024;
/// The journal is rewritten after this many completed files, so a stopped
/// download resumes close to where it was without rewriting it for every file.
const JOURNAL_EVERY_FILES: usize = 64;
const JOURNAL_FILE: &str = ".orivo-epic.json";
const SPILL_DIRECTORY: &str = ".orivo-chunks";
const PARTIAL_SUFFIX: &str = ".orivo-part";

/// What a running download reports. Every field only ever grows while the
/// download runs, so a reader on another thread never sees it go backwards.
#[derive(Debug, Default)]
pub struct Progress {
    pub downloaded_bytes: AtomicU64,
    pub download_total_bytes: AtomicU64,
    pub written_bytes: AtomicU64,
    pub install_total_bytes: AtomicU64,
}

impl Progress {
    /// Downloading is what takes the time, so it is what the percentage
    /// counts; a resumed download that has nothing left to fetch is writing.
    pub fn percent(&self) -> u8 {
        let total = self.download_total_bytes.load(Ordering::Relaxed);
        let done = self.downloaded_bytes.load(Ordering::Relaxed);
        if total == 0 {
            let total = self.install_total_bytes.load(Ordering::Relaxed);
            let done = self.written_bytes.load(Ordering::Relaxed);
            if total == 0 {
                return 0;
            }
            return ((done.min(total) * 100) / total).min(99) as u8;
        }
        ((done.min(total) * 100) / total).min(99) as u8
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    build_version: String,
    #[serde(default)]
    complete: Vec<String>,
}

fn read_journal(root: &Path, build_version: &str) -> Vec<String> {
    fs::read(root.join(JOURNAL_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Journal>(&bytes).ok())
        .filter(|journal| journal.build_version == build_version)
        .map(|journal| journal.complete)
        .unwrap_or_default()
}

fn write_journal(root: &Path, build_version: &str, complete: &[String]) -> Result<(), EpicDownloadError> {
    let journal = Journal {
        build_version: build_version.to_string(),
        complete: complete.to_vec(),
    };
    let bytes = serde_json::to_vec(&journal).map_err(|error| EpicDownloadError::Io(error.to_string()))?;
    let temporary = root.join(format!("{JOURNAL_FILE}.tmp"));
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, root.join(JOURNAL_FILE))?;
    Ok(())
}

/// Which files still have to be written and which chunks, in which order, they
/// need. A file already complete from an earlier attempt of the same build is
/// left alone, and so are the chunks only it used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub files: Vec<usize>,
    /// Indices into `Manifest::chunks`, in the order the files first use them.
    pub chunk_order: Vec<usize>,
    pub download_bytes: u64,
    pub install_bytes: u64,
}

pub fn plan_install(manifest: &Manifest, root: &Path) -> Result<InstallPlan, EpicDownloadError> {
    let complete: std::collections::HashSet<String> =
        read_journal(root, &manifest.build_version).into_iter().collect();
    let chunk_index: HashMap<Guid, usize> = manifest
        .chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| (chunk.guid, index))
        .collect();
    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut chunk_order = Vec::new();
    let mut install_bytes = 0;
    for (index, file) in manifest.files.iter().enumerate() {
        // A path is checked before anything is planned, so a manifest with one
        // bad name installs nothing rather than half a game.
        let path = install_path(root, &file.name)?;
        // Links do not survive the trip to Android's shared storage, and no
        // Windows build Orivo installs depends on one.
        if !file.symlink_target.is_empty() {
            continue;
        }
        if complete.contains(&file.name)
            && fs::metadata(&path).is_ok_and(|meta| meta.len() == file.size())
        {
            continue;
        }
        files.push(index);
        install_bytes += file.size();
        for part in &file.parts {
            let chunk = *chunk_index.get(&part.guid).ok_or_else(|| {
                EpicDownloadError::Format(format!("{} uses a chunk the list lacks", file.name))
            })?;
            if seen.insert(chunk) {
                chunk_order.push(chunk);
            }
        }
    }
    let download_bytes = chunk_order
        .iter()
        .map(|index| manifest.chunks[*index].file_size)
        .sum();
    Ok(InstallPlan {
        files,
        chunk_order,
        download_bytes,
        install_bytes,
    })
}

/// Fetches one chunk file, raw, from wherever it can be had.
pub trait ChunkFetcher: Send + Sync {
    fn fetch(
        &self,
        path: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, EpicDownloadError>> + Send + '_>>;
}

/// Chunks over HTTPS, from each CDN in turn, with a few retries for the
/// failures a phone on Wi-Fi actually meets.
pub struct HttpChunkFetcher {
    client: reqwest::Client,
    bases: Vec<String>,
}

impl HttpChunkFetcher {
    pub fn new(client: reqwest::Client, bases: Vec<String>) -> Self {
        Self { client, bases }
    }
}

impl ChunkFetcher for HttpChunkFetcher {
    fn fetch(
        &self,
        path: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, EpicDownloadError>> + Send + '_>>
    {
        Box::pin(async move {
            let mut last_error = String::from("no download location");
            for attempt in 0..4u32 {
                if attempt > 0 {
                    tokio::time::sleep(Duration::from_millis(600 * u64::from(attempt))).await;
                }
                for base in &self.bases {
                    let url = format!("{base}/{path}");
                    match self.client.get(&url).send().await {
                        Ok(response) if response.status().is_success() => match response.bytes().await {
                            Ok(bytes) => return Ok(bytes.to_vec()),
                            Err(error) => last_error = error.to_string(),
                        },
                        Ok(response) => last_error = format!("the server answered {}", response.status()),
                        Err(error) => last_error = error.to_string(),
                    }
                }
            }
            Err(EpicDownloadError::Network(last_error))
        })
    }
}

/// The client every CDN request goes through: generous per request, because a
/// manifest can be tens of MiB on a slow phone link, and never redirected off
/// HTTPS.
pub fn download_client() -> Result<reqwest::Client, EpicDownloadError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(300))
        .https_only(true)
        .user_agent("EpicGamesLauncher/16.10.0-orivo Windows/10.0.22631.1.256.64bit")
        .build()
        .map_err(|error| EpicDownloadError::Network(error.to_string()))
}

/// Fetch and check a manifest, trying each signed URL in turn.
pub async fn fetch_manifest(
    client: &reqwest::Client,
    location: &BuildLocation,
) -> Result<Manifest, EpicDownloadError> {
    let mut last_error = EpicDownloadError::Network("no manifest location".into());
    for url in &location.manifest_urls {
        let bytes = match client.get(url).send().await {
            Ok(response) if response.status().is_success() => match response.bytes().await {
                Ok(bytes) => bytes,
                Err(error) => {
                    last_error = EpicDownloadError::Network(error.to_string());
                    continue;
                }
            },
            Ok(response) => {
                last_error = EpicDownloadError::Network(format!("the server answered {}", response.status()));
                continue;
            }
            Err(error) => {
                last_error = EpicDownloadError::Network(error.to_string());
                continue;
            }
        };
        if bytes.len() > MAX_MANIFEST_BYTES {
            last_error = EpicDownloadError::Format("the manifest is implausibly large".into());
            continue;
        }
        if let Some(expected) = location.manifest_sha1
            && sha1_of(&bytes) != expected
        {
            last_error = EpicDownloadError::Integrity("manifest".into());
            continue;
        }
        return parse_manifest(&bytes);
    }
    Err(last_error)
}

/// Bytes free where `path` (or its nearest existing parent) lives.
#[cfg(unix)]
pub fn available_space(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        probe = probe.parent()?.to_path_buf();
    }
    let raw = CString::new(probe.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `raw` is a valid NUL-terminated path and `stats` is a properly
    // sized, writable `statvfs` the call fills in.
    if unsafe { libc::statvfs(raw.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    Some(stats.f_bavail as u64 * stats.f_frsize as u64)
}

#[cfg(not(unix))]
pub fn available_space(_path: &Path) -> Option<u64> {
    None
}

/// Room a download needs beyond the game itself: the in-memory cache spilling
/// to disk, the journal, and not leaving a phone with zero bytes free.
const SPACE_MARGIN_BYTES: u64 = 512 * 1024 * 1024;

/// Write every file of `manifest` that is not already complete under `root`.
///
/// Files are written one after another, each first to a `.orivo-part` beside
/// it and renamed once its digest matches, so a stopped download never leaves
/// a file that looks finished and is not. Chunks are fetched ahead, several at
/// once, in the order the files need them.
pub async fn install_build(
    manifest: &Manifest,
    fetcher: &dyn ChunkFetcher,
    root: &Path,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<(), EpicDownloadError> {
    fs::create_dir_all(root)?;
    let plan = plan_install(manifest, root)?;
    progress
        .download_total_bytes
        .store(plan.download_bytes, Ordering::Relaxed);
    progress
        .install_total_bytes
        .store(plan.install_bytes, Ordering::Relaxed);
    if let Some(available) = available_space(root) {
        let needed = plan.install_bytes + SPACE_MARGIN_BYTES;
        if available < needed {
            return Err(EpicDownloadError::NoSpace {
                needed: plan.install_bytes,
                available,
            });
        }
    }

    // How many more parts still read each chunk, so a window is dropped the
    // moment its last reader is done with it.
    let mut remaining: HashMap<Guid, usize> = HashMap::new();
    for file in &plan.files {
        for part in &manifest.files[*file].parts {
            *remaining.entry(part.guid).or_default() += 1;
        }
    }

    let mut downloads = stream::iter(plan.chunk_order.iter().copied())
        .map(|index| {
            let chunk = manifest.chunks[index].clone();
            let path = manifest.chunk_path(&chunk);
            async move {
                if cancel.load(Ordering::Relaxed) {
                    return Err(EpicDownloadError::Cancelled);
                }
                let raw = fetcher.fetch(path).await?;
                progress
                    .downloaded_bytes
                    .fetch_add(raw.len() as u64, Ordering::Relaxed);
                // Inflating and hashing a window is CPU work; it runs on a
                // blocking thread so eight downloads decode in parallel.
                let window = tauri::async_runtime::spawn_blocking(move || decode_chunk(&raw, &chunk))
                    .await
                    .map_err(|error| EpicDownloadError::Io(error.to_string()))??;
                Ok::<_, EpicDownloadError>((manifest.chunks[index].guid, Arc::new(window)))
            }
        })
        .buffered(CONCURRENT_CHUNKS);

    let spill = root.join(SPILL_DIRECTORY);
    let mut cache: HashMap<Guid, Arc<Vec<u8>>> = HashMap::new();
    let mut cache_bytes = 0usize;
    let mut spilled: std::collections::HashSet<Guid> = std::collections::HashSet::new();
    let mut complete = read_journal(root, &manifest.build_version);
    let mut since_journal = 0usize;

    for file_index in &plan.files {
        let file = &manifest.files[*file_index];
        if cancel.load(Ordering::Relaxed) {
            write_journal(root, &manifest.build_version, &complete)?;
            return Err(EpicDownloadError::Cancelled);
        }
        let target = install_path(root, &file.name)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut partial_name = target.file_name().unwrap_or_default().to_os_string();
        partial_name.push(PARTIAL_SUFFIX);
        let partial = target.with_file_name(partial_name);
        let mut output = std::io::BufWriter::with_capacity(1024 * 1024, fs::File::create(&partial)?);
        let mut hasher = Sha1::new();

        for part in &file.parts {
            let window = if let Some(window) = cache.get(&part.guid) {
                Arc::clone(window)
            } else if spilled.contains(&part.guid) {
                Arc::new(fs::read(spill.join(part.guid.hex()))?)
            } else {
                // Chunks arrive in the order the files first use them, so a
                // window that is neither cached nor spilled is the next one.
                let (guid, window) = downloads.next().await.ok_or_else(|| {
                    EpicDownloadError::Format("the chunk list ended before the files did".into())
                })??;
                if guid != part.guid {
                    return Err(EpicDownloadError::Format("chunks arrived out of order".into()));
                }
                if remaining.get(&guid).copied().unwrap_or(0) > 1 {
                    if cache_bytes + window.len() <= CACHE_LIMIT_BYTES {
                        cache_bytes += window.len();
                        cache.insert(guid, Arc::clone(&window));
                    } else {
                        fs::create_dir_all(&spill)?;
                        fs::write(spill.join(guid.hex()), window.as_slice())?;
                        spilled.insert(guid);
                    }
                }
                window
            };

            let start = part.offset as usize;
            let end = start + part.size as usize;
            let slice = window.get(start..end).ok_or_else(|| {
                EpicDownloadError::Format(format!("{} reads past the end of a chunk", file.name))
            })?;
            output.write_all(slice)?;
            hasher.update(slice);
            progress
                .written_bytes
                .fetch_add(u64::from(part.size), Ordering::Relaxed);

            let left = remaining.entry(part.guid).or_default();
            *left = left.saturating_sub(1);
            if *left == 0 {
                if let Some(window) = cache.remove(&part.guid) {
                    cache_bytes -= window.len();
                }
                if spilled.remove(&part.guid) {
                    let _ = fs::remove_file(spill.join(part.guid.hex()));
                }
            }
        }

        output.flush()?;
        drop(output);
        let digest: [u8; 20] = hasher.finalize().into();
        if file.sha1 != [0; 20] && digest != file.sha1 {
            let _ = fs::remove_file(&partial);
            return Err(EpicDownloadError::Integrity(file.name.clone()));
        }
        fs::rename(&partial, &target)?;
        complete.push(file.name.clone());
        since_journal += 1;
        if since_journal >= JOURNAL_EVERY_FILES {
            write_journal(root, &manifest.build_version, &complete)?;
            since_journal = 0;
        }
    }

    write_journal(root, &manifest.build_version, &complete)?;
    let _ = fs::remove_dir_all(&spill);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::ZlibEncoder};
    use std::sync::Mutex;

    // A writer for the same two formats, so every test reads bytes laid out
    // exactly as the CDN lays them out rather than a struct built by hand.

    fn fstring(out: &mut Vec<u8>, value: &str) {
        if value.is_empty() {
            out.extend_from_slice(&0i32.to_le_bytes());
            return;
        }
        if value.is_ascii() {
            out.extend_from_slice(&((value.len() + 1) as i32).to_le_bytes());
            out.extend_from_slice(value.as_bytes());
            out.push(0);
        } else {
            let units: Vec<u16> = value.encode_utf16().chain([0]).collect();
            out.extend_from_slice(&(-(units.len() as i32)).to_le_bytes());
            for unit in units {
                out.extend_from_slice(&unit.to_le_bytes());
            }
        }
    }

    fn section(out: &mut Vec<u8>, version: u8, body: impl FnOnce(&mut Vec<u8>)) {
        let mut content = vec![version];
        body(&mut content);
        // A trailing field a newer writer added: the size must be what skips it.
        content.extend_from_slice(&[0xEE, 0xEE]);
        out.extend_from_slice(&((content.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(&content);
    }

    fn zlib(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    struct Build {
        manifest: Vec<u8>,
        chunks: HashMap<String, Vec<u8>>,
        files: Vec<(String, Vec<u8>)>,
    }

    /// Cut `files` into chunks of `window` bytes the way Epic does — files
    /// back to back, so one chunk can hold the end of one file and the start
    /// of the next — and lay out a manifest and the chunk files for them.
    fn build(files: &[(&str, &[u8])], window: usize) -> Build {
        let stream: Vec<u8> = files.iter().flat_map(|(_, data)| data.iter().copied()).collect();
        let windows: Vec<Vec<u8>> = stream.chunks(window).map(<[u8]>::to_vec).collect();
        let guids: Vec<Guid> = (0..windows.len())
            .map(|index| Guid([0xA0 + index as u32, 0xB1, 0xC2, 0xD3 + index as u32]))
            .collect();
        let hashes: Vec<u64> = (0..windows.len()).map(|index| 0x1234_5678_9ABC_0000 + index as u64).collect();

        let mut chunk_files = HashMap::new();
        let mut chunk_sizes = Vec::new();
        let probe = Manifest {
            feature_level: 18,
            app_name: String::new(),
            build_version: String::new(),
            launch_exe: String::new(),
            launch_command: String::new(),
            chunks: Vec::new(),
            files: Vec::new(),
        };
        for (index, data) in windows.iter().enumerate() {
            let compressed = zlib(data);
            let mut chunk = Vec::new();
            chunk.extend_from_slice(&CHUNK_MAGIC.to_le_bytes());
            chunk.extend_from_slice(&3u32.to_le_bytes());
            chunk.extend_from_slice(&66u32.to_le_bytes());
            chunk.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            for word in guids[index].0 {
                chunk.extend_from_slice(&word.to_le_bytes());
            }
            chunk.extend_from_slice(&hashes[index].to_le_bytes());
            chunk.push(STORED_COMPRESSED);
            chunk.extend_from_slice(&sha1_of(data));
            chunk.push(0x03);
            chunk.extend_from_slice(&(data.len() as u32).to_le_bytes());
            assert_eq!(chunk.len(), 66);
            chunk.extend_from_slice(&compressed);
            chunk_sizes.push(chunk.len() as i64);
            let info = ChunkInfo {
                guid: guids[index],
                rolling_hash: hashes[index],
                sha1: sha1_of(data),
                group: (index % 100) as u8,
                window_size: data.len() as u32,
                file_size: chunk.len() as u64,
            };
            chunk_files.insert(probe.chunk_path(&info), chunk);
        }

        let mut body = Vec::new();
        section(&mut body, 2, |out| {
            out.extend_from_slice(&18u32.to_le_bytes());
            out.push(0);
            out.extend_from_slice(&0u32.to_le_bytes());
            fstring(out, "Sugar");
            fstring(out, "1.0.0-CL-1");
            fstring(out, "Binaries/Win64/Sugär.exe");
            fstring(out, "-nosplash");
            out.extend_from_slice(&1u32.to_le_bytes());
            fstring(out, "prereq");
            fstring(out, "VC++");
            fstring(out, "Redist/vc.exe");
            fstring(out, "/quiet");
            fstring(out, "build-id");
            fstring(out, "");
            fstring(out, "");
        });
        section(&mut body, 0, |out| {
            out.extend_from_slice(&(windows.len() as u32).to_le_bytes());
            for guid in &guids {
                for word in guid.0 {
                    out.extend_from_slice(&word.to_le_bytes());
                }
            }
            for hash in &hashes {
                out.extend_from_slice(&hash.to_le_bytes());
            }
            for data in &windows {
                out.extend_from_slice(&sha1_of(data));
            }
            for index in 0..windows.len() {
                out.push((index % 100) as u8);
            }
            for data in &windows {
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            }
            for size in &chunk_sizes {
                out.extend_from_slice(&size.to_le_bytes());
            }
        });
        section(&mut body, 2, |out| {
            out.extend_from_slice(&(files.len() as u32).to_le_bytes());
            for (name, _) in files {
                fstring(out, name);
            }
            for _ in files {
                fstring(out, "");
            }
            for (_, data) in files {
                out.extend_from_slice(&sha1_of(data));
            }
            for _ in files {
                out.push(0);
            }
            for _ in files {
                out.extend_from_slice(&1u32.to_le_bytes());
                fstring(out, "");
            }
            let mut cursor = 0usize;
            for (_, data) in files {
                let mut parts = Vec::new();
                let mut left = data.len();
                while left > 0 {
                    let index = cursor / window;
                    let offset = cursor % window;
                    let size = left.min(window - offset);
                    parts.push((guids[index], offset as u32, size as u32));
                    cursor += size;
                    left -= size;
                }
                out.extend_from_slice(&(parts.len() as u32).to_le_bytes());
                for (guid, offset, size) in parts {
                    out.extend_from_slice(&28u32.to_le_bytes());
                    for word in guid.0 {
                        out.extend_from_slice(&word.to_le_bytes());
                    }
                    out.extend_from_slice(&offset.to_le_bytes());
                    out.extend_from_slice(&size.to_le_bytes());
                }
            }
            for _ in files {
                out.extend_from_slice(&0u32.to_le_bytes());
            }
            for _ in files {
                fstring(out, "application/octet-stream");
            }
            for _ in files {
                out.extend_from_slice(&[0u8; 32]);
            }
        });
        section(&mut body, 0, |out| {
            out.extend_from_slice(&0u32.to_le_bytes());
        });

        let compressed = zlib(&body);
        let mut manifest = Vec::new();
        manifest.extend_from_slice(&MANIFEST_MAGIC.to_le_bytes());
        manifest.extend_from_slice(&41u32.to_le_bytes());
        manifest.extend_from_slice(&(body.len() as u32).to_le_bytes());
        manifest.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        manifest.extend_from_slice(&sha1_of(&body));
        manifest.push(STORED_COMPRESSED);
        manifest.extend_from_slice(&18u32.to_le_bytes());
        manifest.extend_from_slice(&compressed);

        Build {
            manifest,
            chunks: chunk_files,
            files: files.iter().map(|(name, data)| (name.to_string(), data.to_vec())).collect(),
        }
    }

    struct MemoryFetcher {
        chunks: HashMap<String, Vec<u8>>,
        requested: Mutex<Vec<String>>,
    }

    impl ChunkFetcher for MemoryFetcher {
        fn fetch(
            &self,
            path: String,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, EpicDownloadError>> + Send + '_>>
        {
            Box::pin(async move {
                self.requested.lock().unwrap().push(path.clone());
                self.chunks
                    .get(&path)
                    .cloned()
                    .ok_or_else(|| EpicDownloadError::Network(format!("404 {path}")))
            })
        }
    }

    fn sample_files() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("Binaries/Win64/Sugär.exe", (0..5000u32).map(|value| (value * 7 % 251) as u8).collect()),
            ("Content/Paks/Sugar-Windows.pak", (0..9000u32).map(|value| (value % 13) as u8).collect()),
            ("Engine/readme.txt", b"three files, cut across shared chunks".to_vec()),
        ]
    }

    fn temporary_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "orivo-epic-{name}-{}-{}",
            std::process::id(),
            sources_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn sources_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[test]
    fn reads_a_binary_manifest_section_by_section() {
        let files = sample_files();
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(name, data)| (*name, data.as_slice())).collect();
        let build = build(&refs, 4096);
        let manifest = parse_manifest(&build.manifest).unwrap();

        assert_eq!(manifest.app_name, "Sugar");
        assert_eq!(manifest.build_version, "1.0.0-CL-1");
        // UTF-16 names survive, and the launch command is kept for the launch.
        assert_eq!(manifest.launch_exe, "Binaries/Win64/Sugär.exe");
        assert_eq!(manifest.launch_command, "-nosplash");
        assert_eq!(manifest.files.len(), 3);
        assert_eq!(manifest.files[0].size(), 5000);
        assert_eq!(manifest.install_size(), 5000 + 9000 + 37);
        assert_eq!(manifest.chunks.len(), (14037usize).div_ceil(4096));
        assert_eq!(manifest.chunk_directory(), "ChunksV4");
        assert_eq!(
            manifest.chunk_path(&manifest.chunks[1]),
            "ChunksV4/01/123456789ABC0001_000000A1000000B1000000C2000000D4.chunk"
        );
    }

    #[test]
    fn refuses_a_manifest_whose_digest_disagrees() {
        let files = sample_files();
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(name, data)| (*name, data.as_slice())).collect();
        let mut bytes = build(&refs, 4096).manifest;
        bytes[20] ^= 0xFF;
        assert!(matches!(parse_manifest(&bytes), Err(EpicDownloadError::Integrity(_))));
        assert!(matches!(parse_manifest(b"not a manifest"), Err(EpicDownloadError::Format(_))));
    }

    #[test]
    fn decodes_a_chunk_and_checks_it_is_the_one_asked_for() {
        let files = sample_files();
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(name, data)| (*name, data.as_slice())).collect();
        let build = build(&refs, 4096);
        let manifest = parse_manifest(&build.manifest).unwrap();
        let chunk = &manifest.chunks[0];
        let raw = &build.chunks[&manifest.chunk_path(chunk)];
        let window = decode_chunk(raw, chunk).unwrap();
        assert_eq!(window.len(), 4096);

        // Asked for one chunk, answered with another.
        assert!(matches!(
            decode_chunk(raw, &manifest.chunks[1]),
            Err(EpicDownloadError::Integrity(_))
        ));
        // A window that inflates to something else than the manifest says.
        let mut wrong = chunk.clone();
        wrong.sha1 = [7; 20];
        assert!(matches!(decode_chunk(raw, &wrong), Err(EpicDownloadError::Integrity(_))));
    }

    #[test]
    fn reads_the_json_manifest_older_builds_still_serve() {
        let json = br#"{
            "ManifestFileVersion": "013000000000",
            "AppNameString": "Old",
            "BuildVersionString": "2.1",
            "LaunchExeString": "Old.exe",
            "LaunchCommand": "",
            "FileManifestList": [{
                "Filename": "Old.exe",
                "FileHash": "001002003004005006007008009010011012013014015016017018019020",
                "FileChunkParts": [{"Guid": "000000A1000000B1000000C2000000D3", "Offset": "000000000000", "Size": "016000000000"}]
            }],
            "ChunkHashList": {"000000A1000000B1000000C2000000D3": "001000000000000000000000"},
            "ChunkShaList": {"000000A1000000B1000000C2000000D3": "0102030405060708090a0b0c0d0e0f1011121314"},
            "DataGroupList": {"000000A1000000B1000000C2000000D3": "042"},
            "ChunkFilesizeList": {"000000A1000000B1000000C2000000D3": "000004000000000000000000"}
        }"#;
        let manifest = parse_manifest(json).unwrap();
        assert_eq!(manifest.feature_level, 13);
        assert_eq!(manifest.files[0].size(), 16);
        assert_eq!(manifest.files[0].sha1[19], 20);
        assert_eq!(manifest.chunks[0].group, 42);
        assert_eq!(manifest.chunks[0].rolling_hash, 1);
        assert_eq!(manifest.chunks[0].file_size, 1024);
        assert_eq!(manifest.chunk_directory(), "ChunksV3");
        assert_eq!(
            manifest.chunk_path(&manifest.chunks[0]),
            "ChunksV3/42/0000000000000001_000000A1000000B1000000C2000000D3.chunk"
        );
    }

    #[test]
    fn never_writes_outside_the_game_folder() {
        let root = Path::new("/storage/emulated/0/Download/Orivo/Sugar");
        assert_eq!(
            install_path(root, "Binaries\\Win64\\Game.exe").unwrap(),
            root.join("Binaries/Win64/Game.exe")
        );
        for bad in ["../escape.exe", "/etc/passwd", "C:/Windows/evil.dll", "a/../../b", "", "."] {
            assert!(install_path(root, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn reads_where_the_service_says_a_build_lives() {
        let body = br#"{"elements":[{"appName":"Sugar","labelName":"Live-Windows","buildVersion":"1.0.0-CL-1",
            "hash":"0102030405060708090A0B0C0D0E0F1011121314",
            "manifests":[
              {"uri":"https://epicgames-download1.akamaized.net/Builds/Org/o-1/abc/default/x.manifest",
               "queryParams":[{"name":"f_token","value":"exp=1~acl=*~hmac=00"}]},
              {"uri":"https://fastly-download.epicgames.com/Builds/Org/o-1/abc/default/x.manifest"},
              {"uri":"http://download.epicgames.com/Builds/Org/o-1/abc/default/x.manifest"}
            ]}]}"#;
        let location = parse_build_location(body).unwrap();
        assert_eq!(location.build_version, "1.0.0-CL-1");
        assert_eq!(location.manifest_urls.len(), 2, "cleartext locations are dropped");
        assert!(location.manifest_urls[0].contains("f_token=exp%3D1%7Eacl%3D*%7Ehmac%3D00"));
        assert_eq!(
            location.chunk_bases,
            vec![
                "https://epicgames-download1.akamaized.net/Builds/Org/o-1/abc/default".to_string(),
                "https://fastly-download.epicgames.com/Builds/Org/o-1/abc/default".to_string(),
            ]
        );
        assert_eq!(location.manifest_sha1.unwrap()[0], 1);
        assert!(parse_build_location(br#"{"elements":[]}"#).is_err());
        assert_eq!(
            manifest_api_url("Windows", "ns", "item", "Sugar App"),
            "https://launcher-public-service-prod06.ol.epicgames.com/launcher/api/public/assets/v2/platform/Windows/namespace/ns/catalogItem/item/app/Sugar+App/label/Live"
        );
    }

    #[test]
    fn installs_every_file_and_resumes_without_fetching_twice() {
        let files = sample_files();
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(name, data)| (*name, data.as_slice())).collect();
        let build = build(&refs, 4096);
        let manifest = parse_manifest(&build.manifest).unwrap();
        let root = temporary_root("install");
        let fetcher = MemoryFetcher {
            chunks: build.chunks.clone(),
            requested: Mutex::new(Vec::new()),
        };
        let progress = Progress::default();
        let cancel = AtomicBool::new(false);

        tauri::async_runtime::block_on(install_build(&manifest, &fetcher, &root, &progress, &cancel)).unwrap();
        for (name, data) in &build.files {
            assert_eq!(&fs::read(root.join(name)).unwrap(), data, "{name}");
        }
        // Each chunk once, even the ones two files share.
        assert_eq!(fetcher.requested.lock().unwrap().len(), manifest.chunks.len());
        assert_eq!(progress.written_bytes.load(Ordering::Relaxed), manifest.install_size());
        assert!(!root.join(SPILL_DIRECTORY).exists());

        // Everything is in the journal: a second run fetches nothing.
        fetcher.requested.lock().unwrap().clear();
        let again = Progress::default();
        tauri::async_runtime::block_on(install_build(&manifest, &fetcher, &root, &again, &cancel)).unwrap();
        assert!(fetcher.requested.lock().unwrap().is_empty());

        // A damaged file is not trusted because the journal names it.
        fs::write(root.join("Engine/readme.txt"), b"short").unwrap();
        let plan = plan_install(&manifest, &root).unwrap();
        assert_eq!(plan.files, vec![2]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stops_when_asked_and_leaves_no_finished_looking_file() {
        let files = sample_files();
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(name, data)| (*name, data.as_slice())).collect();
        let build = build(&refs, 4096);
        let manifest = parse_manifest(&build.manifest).unwrap();
        let root = temporary_root("cancel");
        let fetcher = MemoryFetcher {
            chunks: build.chunks,
            requested: Mutex::new(Vec::new()),
        };
        let cancel = AtomicBool::new(true);
        let result = tauri::async_runtime::block_on(install_build(
            &manifest,
            &fetcher,
            &root,
            &Progress::default(),
            &cancel,
        ));
        assert!(matches!(result, Err(EpicDownloadError::Cancelled)));
        assert!(!root.join("Binaries/Win64/Sugär.exe").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_chunk_fails_the_install() {
        let files = sample_files();
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(name, data)| (*name, data.as_slice())).collect();
        let mut build = build(&refs, 4096);
        let manifest = parse_manifest(&build.manifest).unwrap();
        build.chunks.remove(&manifest.chunk_path(&manifest.chunks[2]));
        let root = temporary_root("missing");
        let fetcher = MemoryFetcher {
            chunks: build.chunks,
            requested: Mutex::new(Vec::new()),
        };
        let result = tauri::async_runtime::block_on(install_build(
            &manifest,
            &fetcher,
            &root,
            &Progress::default(),
            &AtomicBool::new(false),
        ));
        assert!(matches!(result, Err(EpicDownloadError::Network(_))));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn counts_progress_against_the_download() {
        let progress = Progress::default();
        assert_eq!(progress.percent(), 0);
        progress.download_total_bytes.store(200, Ordering::Relaxed);
        progress.downloaded_bytes.store(100, Ordering::Relaxed);
        assert_eq!(progress.percent(), 50);
        progress.downloaded_bytes.store(200, Ordering::Relaxed);
        // 100 is "installed", which only the install itself may say.
        assert_eq!(progress.percent(), 99);
        assert_eq!(human_bytes(1_500_000_000), "1.5 GB");
    }
}
