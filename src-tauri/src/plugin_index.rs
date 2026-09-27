//! Orivo's official plugin registry: a signed index, fetched off the rendering
//! path and cached.
//!
//! The registry compiled into the binary (`resources/plugin-registry.json`) is
//! as trustworthy as the binary, and as fresh as the last release. A remote
//! index is neither, so it is treated the way every other third-party document
//! in this crate is treated: bounded before it is parsed, parsed before it is
//! believed, and believed only for what it was signed for.
//!
//! ```text
//! { "formatVersion": 1,
//!   "document":  "<the index, verbatim, as one JSON string>",
//!   "signature": "<base64 Ed25519 over sha256(document)>" }
//! ```
//!
//! The document is carried as a *string* rather than as a nested object because
//! a signature covers bytes, and two JSON encoders do not agree on which bytes
//! an object is. Signing the string the publisher actually wrote removes the
//! need for a canonicalisation rule that both sides would have to implement
//! identically and neither could test against the other.
//!
//! Four things protect the fetch itself, and none of them is the signature:
//!
//! - **The host allowlist is compiled in.** The index cannot name the host it is
//!   downloaded from, and neither can a package URL inside it — an index that
//!   could widen its own allowlist would be a signed document granting itself
//!   permissions, which is the shape of every plugin rule this crate refuses.
//! - **Every redirect hop is re-checked**, so a moved asset cannot pull bytes
//!   from somewhere else.
//! - **A sequence number only moves forward.** A signature stays valid forever;
//!   replaying yesterday's signed index to hide a security update is the attack
//!   a signature alone does not stop.
//! - **Nothing here runs on a display path.** The catalogue is served from the
//!   cache; refreshing it is a separate, cancellable command.

use crate::plugin_manifest::{MAX_PACKAGE_BYTES, valid_opaque_id};
use ed25519_dalek::{Signature, VerifyingKey};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Where the official index is published. Hosting it — and holding the key that
/// signs it — is a decision outside this module: the constant is the one line
/// that changes when it is made, and until then the fetch simply fails and the
/// compiled-in registry is what the Store shows.
pub const REGISTRY_INDEX_URL: &str =
    "https://raw.githubusercontent.com/justeozan/orivo-plugin-registry/main/index.v1.json";

/// Hosts the registry and its packages may be fetched from. Compiled in, never
/// read from a document: see the module note.
const REGISTRY_HOSTS: &[&str] = &[
    "raw.githubusercontent.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// Orivo's release signing key, in the role of index signer.
///
/// It is deliberately a *separate constant* with the same value. Signing an
/// index and signing a package are different authorities — one says "this is
/// what exists", the other "this is what it contains" — and giving them two
/// names is what makes splitting them later a one-line change rather than an
/// audit.
const INDEX_PUBLIC_KEY_BASE64: &str = "OX9NRNeAEL2tEyS54qUTJ14cFS6smfLu6JoPzbiXG9w=";

pub const INDEX_FORMAT_VERSION: u32 = 1;
pub const INDEX_SCHEMA_VERSION: u32 = 1;
/// A registry is a list of names and digests. Anything larger than this is not
/// a bigger catalogue, it is something else.
const MAX_INDEX_BYTES: u64 = 256 * 1024;
const MAX_INDEX_ENTRIES: usize = 256;
const MAX_TEXT_LENGTH: usize = 256;
const MAX_ETAG_LENGTH: usize = 256;
const MAX_URL_LENGTH: usize = 512;
/// How long a cached index is served without asking the network at all. Short
/// enough that a pulled release is noticed the same day, long enough that
/// opening Settings twice is not two requests.
pub const INDEX_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 6;

const CACHE_DIRECTORY: &str = ".cache";
const CACHE_FILE: &str = "registry-index.json";

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SignedEnvelope {
    format_version: u32,
    document: String,
    signature: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexDocument {
    schema_version: u32,
    /// Monotonic across publications. The cache refuses to move backwards, so a
    /// replayed older index is inert even though its signature still verifies.
    sequence: u64,
    #[serde(default)]
    plugins: Vec<IndexEntryDocument>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexEntryDocument {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    summary: String,
    url: String,
    sha256: String,
    size_bytes: u64,
    /// Declared by the publisher so an index can list a release this host
    /// cannot run without the host having to download it to find out.
    #[serde(default)]
    min_orivo_version: Option<String>,
}

/// One publishable plugin, after the index it came from was verified and every
/// field of it revalidated. Constructing this type is the only way to obtain a
/// URL the downloader will accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub summary: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub min_orivo_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryIndex {
    pub sequence: u64,
    pub entries: Vec<IndexEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexError {
    /// The bytes are not an envelope this host understands.
    Malformed,
    /// The envelope is well formed and was not signed by Orivo.
    Unsigned,
    /// Signed, and describes something this host will not accept — too many
    /// entries, a URL off the allowlist, a digest that is not a digest.
    Rejected,
    /// Signed and well formed, but older than what is already cached.
    Replayed,
}

impl IndexError {
    /// Wording a user can act on. A replayed index and a forged one are the
    /// same event from the user's chair — the registry is not serving what
    /// Orivo can trust — and separating them in the interface would only invite
    /// the wrong conclusion about which one happened.
    pub fn message(self) -> &'static str {
        match self {
            Self::Malformed | Self::Rejected => "Orivo's plugin registry could not be read.",
            Self::Unsigned | Self::Replayed => {
                "Orivo's plugin registry is not signed by Orivo. Nothing was installed."
            }
        }
    }
}

/// Verify an envelope and revalidate everything inside it.
///
/// A signature says who wrote a document, never that the document is
/// well formed — the publisher's own tooling can have a bug, and a signing key
/// that has leaked signs whatever it is asked to. So the checks below run on a
/// *verified* document exactly as they would on an unverified one.
pub fn parse_signed_index(
    bytes: &[u8],
    minimum_sequence: u64,
) -> Result<RegistryIndex, IndexError> {
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err(IndexError::Malformed);
    }
    let envelope =
        serde_json::from_slice::<SignedEnvelope>(bytes).map_err(|_| IndexError::Malformed)?;
    if envelope.format_version != INDEX_FORMAT_VERSION {
        return Err(IndexError::Malformed);
    }
    let signature = decode_base64(&envelope.signature)
        .and_then(|raw| <[u8; 64]>::try_from(raw.as_slice()).ok())
        .ok_or(IndexError::Unsigned)?;
    let key = index_public_key().ok_or(IndexError::Unsigned)?;
    key.verify_strict(
        &Sha256::digest(envelope.document.as_bytes()),
        &Signature::from_bytes(&signature),
    )
    .map_err(|_| IndexError::Unsigned)?;

    parse_signed_index_document(&envelope.document, minimum_sequence)
}

/// Everything [`parse_signed_index`] does once the signature has verified.
/// Split out because it is the half that has to hold on a document Orivo *did*
/// authenticate, and a test proving that cannot hold Orivo's private key.
fn parse_signed_index_document(
    document: &str,
    minimum_sequence: u64,
) -> Result<RegistryIndex, IndexError> {
    let document =
        serde_json::from_str::<IndexDocument>(document).map_err(|_| IndexError::Rejected)?;
    if document.schema_version != INDEX_SCHEMA_VERSION || document.plugins.len() > MAX_INDEX_ENTRIES
    {
        return Err(IndexError::Rejected);
    }
    if document.sequence < minimum_sequence {
        return Err(IndexError::Replayed);
    }

    let mut seen = BTreeSet::new();
    let mut entries = Vec::with_capacity(document.plugins.len());
    for entry in document.plugins {
        let entry = validate_entry(entry).ok_or(IndexError::Rejected)?;
        // A duplicate id makes "which release is this" a question of parse
        // order. A signed document is allowed to be empty, never ambiguous.
        if !seen.insert(entry.id.clone()) {
            return Err(IndexError::Rejected);
        }
        entries.push(entry);
    }
    Ok(RegistryIndex {
        sequence: document.sequence,
        entries,
    })
}

/// The registry compiled into the binary. It needs no signature — it is as
/// trustworthy as the executable that carries it — but it goes through exactly
/// the same grammar, because a typo in a URL is a typo whoever wrote it, and an
/// allowlist that only applies to downloaded documents is an allowlist with a
/// hole shaped like a release.
pub fn parse_unsigned_entries(bytes: &[u8]) -> Vec<IndexEntry> {
    serde_json::from_slice::<Vec<IndexEntryDocument>>(bytes)
        .unwrap_or_default()
        .into_iter()
        .take(MAX_INDEX_ENTRIES)
        .filter_map(validate_entry)
        .collect()
}

fn validate_entry(entry: IndexEntryDocument) -> Option<IndexEntry> {
    if !valid_plugin_id(&entry.id)
        || !valid_text(&entry.name)
        || entry.summary.chars().count() > MAX_TEXT_LENGTH
        || !valid_version(&entry.version)
        || !valid_sha256(&entry.sha256)
        || entry.size_bytes == 0
        || entry.size_bytes > MAX_PACKAGE_BYTES
        || entry.url.len() > MAX_URL_LENGTH
        || entry
            .min_orivo_version
            .as_deref()
            .is_some_and(|version| !valid_version(version))
    {
        return None;
    }
    if !host_allowed(&url_host(&entry.url)?) {
        return None;
    }
    Some(IndexEntry {
        id: entry.id,
        name: entry.name,
        version: entry.version,
        summary: entry.summary,
        url: entry.url,
        sha256: entry.sha256.to_ascii_lowercase(),
        size_bytes: entry.size_bytes,
        min_orivo_version: entry.min_orivo_version,
    })
}

// ---------------------------------------------------------------------------
// URLs
// ---------------------------------------------------------------------------

/// HTTPS only, and only to a host this build was compiled to talk to.
pub fn host_allowed(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    REGISTRY_HOSTS.contains(&host.as_str())
}

/// The host of an HTTPS URL, or nothing. Written by hand rather than through a
/// URL crate because the answer feeds an allowlist: a parser that is generous
/// about what it accepts is the wrong shape for a decision that has to be
/// conservative, and the same function already exists for the Quiky download
/// path — deliberately duplicated rather than shared, so neither allowlist can
/// be widened by a change made for the other.
pub fn url_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|value| !value.is_empty())?;
    // Credentials in a registry URL would be an exfiltration channel and are
    // never part of a legitimate download link.
    if authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// A client that refuses to leave the allowlist, hop by hop.
fn allowlisted_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.stop();
            }
            let allowed = attempt.url().scheme() == "https"
                && attempt.url().host_str().is_some_and(host_allowed);
            if allowed {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|_| "The download client could not start.".to_string())
}

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// What the last fetch produced, kept verbatim.
///
/// The envelope is stored as it arrived and re-verified on every load. The
/// cache file is ordinary user-writable state, so nothing about it is trusted:
/// editing it can make Orivo forget an index, never make it believe one.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedIndex {
    format_version: u32,
    #[serde(default)]
    etag: Option<String>,
    fetched_at_epoch_ms: u64,
    /// Kept beside the envelope so a tampered cache cannot *lower* the
    /// anti-replay floor: the floor is the maximum of this and the sequence the
    /// stored envelope itself carries.
    sequence: u64,
    envelope: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct IndexCache {
    path: PathBuf,
}

/// What a refresh did. The distinction is not cosmetic: `Cached` means no
/// request was made at all, which is the property the display path depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Served from a cache that is still inside its TTL.
    Cached,
    /// The server answered 304 and the cache's clock was reset.
    Unchanged,
    /// A newer index was stored.
    Updated,
}

impl IndexCache {
    pub fn new(plugin_root: &Path) -> Self {
        Self {
            path: plugin_root.join(CACHE_DIRECTORY).join(CACHE_FILE),
        }
    }

    fn read(&self) -> Option<CachedIndex> {
        let metadata = fs::symlink_metadata(&self.path).ok()?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_INDEX_BYTES {
            return None;
        }
        let cached = serde_json::from_slice::<CachedIndex>(&fs::read(&self.path).ok()?).ok()?;
        (cached.format_version == INDEX_FORMAT_VERSION).then_some(cached)
    }

    /// The index the Store may show. Verified again on every read — a cache hit
    /// must not be a shortcut past the signature.
    pub fn load(&self) -> Option<RegistryIndex> {
        let cached = self.read()?;
        let bytes = serde_json::to_vec(&cached.envelope).ok()?;
        parse_signed_index(&bytes, cached.sequence).ok()
    }

    /// The sequence a freshly fetched index has to match or beat.
    fn floor(&self) -> u64 {
        self.read().map_or(0, |cached| cached.sequence)
    }

    fn is_fresh(&self) -> bool {
        self.read().is_some_and(|cached| {
            epoch_ms().saturating_sub(cached.fetched_at_epoch_ms) < INDEX_TTL.as_millis() as u64
        })
    }

    fn etag(&self) -> Option<String> {
        self.read()
            .and_then(|cached| cached.etag)
            .filter(|etag| !etag.is_empty() && etag.len() <= MAX_ETAG_LENGTH)
    }

    fn store(&self, envelope: &[u8], etag: Option<String>, sequence: u64) -> Result<(), String> {
        let envelope = serde_json::from_slice::<serde_json::Value>(envelope)
            .map_err(|_| "The plugin registry could not be cached.".to_string())?;
        let cached = CachedIndex {
            format_version: INDEX_FORMAT_VERSION,
            etag: etag.filter(|etag| !etag.is_empty() && etag.len() <= MAX_ETAG_LENGTH),
            fetched_at_epoch_ms: epoch_ms(),
            sequence,
            envelope,
        };
        self.write(&cached)
    }

    /// Reset the freshness clock without touching the document, which is what a
    /// 304 means and the only reason this is not a whole rewrite.
    fn touch(&self) {
        if let Some(mut cached) = self.read() {
            cached.fetched_at_epoch_ms = epoch_ms();
            let _ = self.write(&cached);
        }
    }

    fn write(&self, cached: &CachedIndex) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|_| "The plugin registry could not be cached.".to_string())?;
        }
        let encoded = serde_json::to_vec(cached)
            .map_err(|_| "The plugin registry could not be cached.".to_string())?;
        let temporary = self.path.with_extension("json.tmp");
        let mut file = fs::File::create(&temporary)
            .map_err(|_| "The plugin registry could not be cached.".to_string())?;
        file.write_all(&encoded)
            .and_then(|()| file.sync_all())
            .map_err(|_| "The plugin registry could not be cached.".to_string())?;
        drop(file);
        fs::rename(&temporary, &self.path)
            .map_err(|_| "The plugin registry could not be cached.".to_string())
    }
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

/// Refresh the cached index. Never called from a command that renders anything:
/// the catalogue reads [`IndexCache::load`], and this is what a background
/// check or an explicit "check for updates" runs.
///
/// `cancelled` is observed between chunks and before each decision, so a user
/// leaving the panel stops the work rather than orphaning it.
pub async fn refresh_index(
    cache: &IndexCache,
    url: &str,
    cancelled: &AtomicBool,
) -> Result<RefreshOutcome, String> {
    if cache.is_fresh() {
        return Ok(RefreshOutcome::Cached);
    }
    if !url_host(url).is_some_and(|host| host_allowed(&host)) {
        return Err("Orivo's plugin registry is not where this build expects it.".into());
    }
    let client = allowlisted_client()?;
    let mut request = client.get(url);
    if let Some(etag) = cache.etag() {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let response = request
        .send()
        .await
        .map_err(|_| "Orivo could not reach its plugin registry.".to_string())?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        cache.touch();
        return Ok(RefreshOutcome::Unchanged);
    }
    if !response.status().is_success() {
        return Err("Orivo's plugin registry did not answer.".into());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_INDEX_BYTES)
    {
        return Err(IndexError::Malformed.message().into());
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancelled.load(Ordering::Acquire) {
            return Err("Cancelled.".into());
        }
        let chunk = chunk.map_err(|_| "Orivo could not reach its plugin registry.".to_string())?;
        if bytes.len() as u64 + chunk.len() as u64 > MAX_INDEX_BYTES {
            return Err(IndexError::Malformed.message().into());
        }
        bytes.extend_from_slice(&chunk);
    }

    let index = parse_signed_index(&bytes, cache.floor()).map_err(|error| error.message())?;
    cache.store(&bytes, etag, index.sequence)?;
    Ok(RefreshOutcome::Updated)
}

/// Download a package named by a verified index entry.
///
/// Length and digest are checked against the entry rather than against
/// anything the response says, so a server that lies about either produces a
/// refusal and not a file.
pub async fn download_entry(
    entry: &IndexEntry,
    cancelled: &AtomicBool,
    on_progress: &mut (dyn FnMut(u64) + Send),
) -> Result<Vec<u8>, String> {
    if !url_host(&entry.url).is_some_and(|host| host_allowed(&host)) {
        return Err("This download is outside Orivo's registry.".into());
    }
    let client = allowlisted_client()?;
    let response = client
        .get(&entry.url)
        .send()
        .await
        .map_err(|_| "The plugin could not be downloaded.".to_string())?;
    if !response.status().is_success() {
        // A 404 here is not a server refusing us: it is a registry entry whose
        // release has not been published yet. Saying so is the difference
        // between a user retrying forever and a user reaching for sideload.
        return Err(if response.status().as_u16() == 404 {
            "This plugin has not been published yet. Install it from a .orivo-plugin file."
                .to_string()
        } else {
            format!(
                "The plugin source rejected the request ({}).",
                response.status().as_u16()
            )
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length != entry.size_bytes)
    {
        return Err("The package size does not match the registry.".into());
    }

    let mut bytes = Vec::with_capacity(entry.size_bytes as usize);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancelled.load(Ordering::Acquire) {
            return Err("Cancelled.".into());
        }
        let chunk = chunk.map_err(|_| "The download was interrupted.".to_string())?;
        if bytes.len() as u64 + chunk.len() as u64 > entry.size_bytes {
            return Err("The package is larger than the registry declares.".into());
        }
        bytes.extend_from_slice(&chunk);
        on_progress(bytes.len() as u64);
    }
    if bytes.len() as u64 != entry.size_bytes {
        return Err("The package size does not match the registry.".into());
    }
    if format!("{:x}", Sha256::digest(&bytes)) != entry.sha256 {
        return Err("The package failed its integrity check.".into());
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------

/// Whether `candidate` is a release to move *to* from `installed`.
///
/// A prerelease sorts below the release it precedes, and an unparseable version
/// on either side is not an upgrade: refusing to compare is the safe answer
/// when the alternative is replacing a working plugin on a guess.
pub fn is_upgrade(candidate: &str, installed: &str) -> bool {
    match (semver_key(candidate), semver_key(installed)) {
        (Some(candidate), Some(installed)) => candidate > installed,
        _ => false,
    }
}

/// `(major, minor, patch, is_release, prerelease)`. `is_release` orders `1.0.0`
/// above `1.0.0-rc.1`, which is the one rule a tuple comparison does not get
/// for free.
fn semver_key(value: &str) -> Option<(u32, u32, u32, bool, String)> {
    let (core, prerelease) = match value.split_once('-') {
        Some((core, prerelease)) => (core, prerelease),
        None => (value, ""),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((
        major,
        minor,
        patch,
        prerelease.is_empty(),
        prerelease.to_owned(),
    ))
}

// ---------------------------------------------------------------------------
// Grammar
// ---------------------------------------------------------------------------

fn valid_plugin_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.split('.').count() >= 3
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn valid_text(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= MAX_TEXT_LENGTH
        && !value.chars().any(char::is_control)
}

fn valid_version(value: &str) -> bool {
    valid_opaque_id(value, 32) && semver_key(value).is_some()
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn index_public_key() -> Option<VerifyingKey> {
    let decoded = decode_base64(INDEX_PUBLIC_KEY_BASE64)?;
    VerifyingKey::from_bytes(&<[u8; 32]>::try_from(decoded.as_slice()).ok()?).ok()
}

/// A tiny standard-alphabet decoder, the twin of the installer's. The only
/// base64 either reads is a fixed-size key or signature, so a dependency would
/// be more surface than the lines it replaces.
fn decode_base64(value: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let mut output = Vec::with_capacity(value.len() / 4 * 3);
    let mut accumulator = 0_u32;
    let mut bits = 0_u32;
    for byte in value.bytes() {
        if byte == b'=' {
            break;
        }
        let index = ALPHABET.iter().position(|candidate| *candidate == byte)? as u32;
        accumulator = (accumulator << 6) | index;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
        }
    }
    Some(output)
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::sync::atomic::AtomicU64;
    use std::time::UNIX_EPOCH as EPOCH;

    /// Tests generate their own key. Orivo's real signing key is not in this
    /// repository and must never be needed to run its suite.
    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7_u8; 32])
    }

    fn encode_base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut output = String::new();
        for chunk in bytes.chunks(3) {
            let mut block = [0_u8; 3];
            block[..chunk.len()].copy_from_slice(chunk);
            let value = u32::from_be_bytes([0, block[0], block[1], block[2]]);
            for slot in 0..4 {
                if slot <= chunk.len() {
                    output.push(ALPHABET[((value >> (18 - slot * 6)) & 0x3f) as usize] as char);
                } else {
                    output.push('=');
                }
            }
        }
        output
    }

    /// The module's key is compiled in, so a test that wants a *verifiable*
    /// envelope has to reach the same verifier the production path does. It
    /// does that by re-implementing the two lines of `parse_signed_index` that
    /// concern the key, against a key it generated itself — which is also the
    /// only honest way to test the signature without holding Orivo's.
    fn verify_with(key: &VerifyingKey, bytes: &[u8]) -> Result<RegistryIndex, IndexError> {
        let envelope =
            serde_json::from_slice::<SignedEnvelope>(bytes).map_err(|_| IndexError::Malformed)?;
        let signature = decode_base64(&envelope.signature)
            .and_then(|raw| <[u8; 64]>::try_from(raw.as_slice()).ok())
            .ok_or(IndexError::Unsigned)?;
        key.verify_strict(
            &Sha256::digest(envelope.document.as_bytes()),
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| IndexError::Unsigned)?;
        parse_signed_index_document(&envelope.document, 0)
    }

    fn document(sequence: u64, plugins: &str) -> String {
        format!(r#"{{"schemaVersion":1,"sequence":{sequence},"plugins":[{plugins}]}}"#)
    }

    fn entry_json(id: &str, version: &str, url: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"Quiky","version":"{version}","summary":"An installer.",
               "url":"{url}","sha256":"{}","sizeBytes":1114}}"#,
            "a".repeat(64)
        )
    }

    fn envelope(document: &str) -> Vec<u8> {
        let signature = signing_key().sign(&Sha256::digest(document.as_bytes()));
        serde_json::to_vec(&SignedEnvelope {
            format_version: INDEX_FORMAT_VERSION,
            document: document.to_string(),
            signature: encode_base64(&signature.to_bytes()),
        })
        .unwrap()
    }

    fn temporary_root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "orivo-plugin-index-{}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(EPOCH).unwrap().as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_signed_index_yields_its_entries() {
        let key = signing_key().verifying_key();
        let bytes = envelope(&document(
            4,
            &entry_json(
                "com.orivo.quiky",
                "0.2.0",
                "https://github.com/justeozan/orivo-plugin-quiky/releases/download/v0.2.0/p.orivo-plugin",
            ),
        ));
        let index = verify_with(&key, &bytes).expect("verifies");
        assert_eq!(index.sequence, 4);
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].id, "com.orivo.quiky");
        assert_eq!(index.entries[0].version, "0.2.0");
    }

    /// The signature is over the document's bytes, so a single byte changed
    /// anywhere inside it — including in a field this host does not read —
    /// invalidates the whole index.
    #[test]
    fn an_index_edited_after_signing_is_refused() {
        let key = signing_key().verifying_key();
        let original = document(
            4,
            &entry_json("com.orivo.quiky", "0.2.0", "https://github.com/a/b"),
        );
        let mut envelope_value =
            serde_json::from_slice::<serde_json::Value>(&envelope(&original)).unwrap();
        envelope_value["document"] = serde_json::Value::String(original.replace("0.2.0", "9.9.9"));
        let tampered = serde_json::to_vec(&envelope_value).unwrap();

        assert_eq!(verify_with(&key, &tampered), Err(IndexError::Unsigned));
    }

    /// Orivo's own key is compiled in and its private half is not in this
    /// repository, so an index signed by anyone else has to be refused by the
    /// production entry point — which is the check this proves.
    #[test]
    fn an_index_signed_by_another_key_never_reaches_the_catalogue() {
        let bytes = envelope(&document(
            1,
            &entry_json("com.orivo.quiky", "0.2.0", "https://github.com/a/b"),
        ));
        assert_eq!(parse_signed_index(&bytes, 0), Err(IndexError::Unsigned));
    }

    /// A signature says who wrote a document, not that the document is sane. A
    /// URL off the allowlist is refused even though the index carrying it
    /// verified — an index that could name its own download host would be a
    /// signed document widening its own permissions.
    #[test]
    fn a_verified_index_is_still_held_to_the_allowlist() {
        let key = signing_key().verifying_key();
        let bytes = envelope(&document(
            1,
            &entry_json("com.orivo.quiky", "0.2.0", "https://cdn.attacker.example/p"),
        ));
        assert_eq!(verify_with(&key, &bytes), Err(IndexError::Rejected));
    }

    #[test]
    fn a_verified_index_is_still_held_to_its_bounds() {
        let key = signing_key().verifying_key();
        let good = "https://github.com/a/b";
        for plugins in [
            // A duplicate id makes "which release" depend on parse order.
            format!(
                "{},{}",
                entry_json("com.orivo.quiky", "0.2.0", good),
                entry_json("com.orivo.quiky", "0.3.0", good)
            ),
            // Not an inverse-DNS identity.
            entry_json("quiky", "0.2.0", good),
            // Not a version this host can order against an installed one.
            entry_json("com.orivo.quiky", "latest", good),
            // A credential smuggled into the authority.
            entry_json("com.orivo.quiky", "0.2.0", "https://user@github.com/a/b"),
            // Plain HTTP, whatever the host.
            entry_json("com.orivo.quiky", "0.2.0", "http://github.com/a/b"),
        ] {
            let bytes = envelope(&document(1, &plugins));
            assert_eq!(
                verify_with(&key, &bytes),
                Err(IndexError::Rejected),
                "accepted {plugins}"
            );
        }
    }

    /// A signature never expires, so replaying an older signed index is how a
    /// registry hides a release. The sequence floor is the only thing that
    /// stops it, and it lives in the cache rather than in the document.
    #[test]
    fn an_older_index_cannot_replace_a_newer_one() {
        let key = signing_key().verifying_key();
        let bytes = envelope(&document(
            3,
            &entry_json("com.orivo.quiky", "0.2.0", "https://github.com/a/b"),
        ));
        let envelope_document = serde_json::from_slice::<SignedEnvelope>(&bytes)
            .unwrap()
            .document;

        assert!(parse_signed_index_document(&envelope_document, 3).is_ok());
        assert_eq!(
            parse_signed_index_document(&envelope_document, 4),
            Err(IndexError::Replayed)
        );
        assert!(verify_with(&key, &bytes).is_ok());
    }

    #[test]
    fn an_index_larger_than_the_host_will_read_is_refused() {
        let oversized = vec![b'{'; MAX_INDEX_BYTES as usize + 1];
        assert_eq!(
            parse_signed_index(&oversized, 0),
            Err(IndexError::Malformed)
        );
    }

    #[test]
    fn only_allowlisted_https_hosts_are_usable() {
        assert!(host_allowed("github.com"));
        assert!(host_allowed("GitHub.com."));
        assert!(!host_allowed("evil.github.com"));
        assert!(!host_allowed("github.com.attacker.example"));
        assert_eq!(
            url_host("https://github.com/a/b").as_deref(),
            Some("github.com")
        );
        assert_eq!(url_host("http://github.com/a").as_deref(), None);
        assert_eq!(url_host("https://a@github.com/b").as_deref(), None);
    }

    #[test]
    fn an_upgrade_is_a_version_that_really_is_newer() {
        assert!(is_upgrade("1.0.1", "1.0.0"));
        assert!(is_upgrade("1.1.0", "1.0.9"));
        assert!(is_upgrade("1.0.0", "1.0.0-rc.1"));
        assert!(!is_upgrade("1.0.0", "1.0.0"));
        assert!(!is_upgrade("0.9.0", "1.0.0"));
        assert!(!is_upgrade("1.0.0-rc.1", "1.0.0"));
        // Neither side is comparable, so neither is an upgrade.
        assert!(!is_upgrade("latest", "1.0.0"));
        assert!(!is_upgrade("1.0.0", "nightly"));
    }

    /// The cache exists so the display path never touches the network, and it
    /// is ordinary user-writable state. Editing it must be able to make Orivo
    /// forget an index, never to make it believe one.
    #[test]
    fn a_tampered_cache_is_forgotten_rather_than_believed() {
        let root = temporary_root();
        let cache = IndexCache::new(&root);
        let bytes = envelope(&document(
            2,
            &entry_json("com.orivo.quiky", "0.2.0", "https://github.com/a/b"),
        ));
        cache.store(&bytes, Some("\"etag-1\"".into()), 2).unwrap();
        assert!(cache.is_fresh());
        assert_eq!(cache.floor(), 2);
        assert_eq!(cache.etag().as_deref(), Some("\"etag-1\""));

        // Signed by a key this build does not hold, so the cached index is
        // unusable — and the Store shows nothing rather than an attacker's list.
        assert_eq!(cache.load(), None);

        fs::write(&cache.path, b"{ not json").unwrap();
        assert_eq!(cache.load(), None);
        assert_eq!(cache.floor(), 0);
        assert!(!cache.is_fresh());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_cache_that_was_never_written_asks_the_network() {
        let root = temporary_root();
        let cache = IndexCache::new(&root);
        assert!(!cache.is_fresh());
        assert_eq!(cache.load(), None);
        assert_eq!(cache.etag(), None);
        fs::remove_dir_all(root).ok();
    }

    /// A refresh that is cancelled before it starts makes no request at all,
    /// which is what makes "checking for updates" abandonable rather than
    /// merely ignorable.
    #[test]
    fn a_refresh_to_a_host_off_the_allowlist_is_never_attempted() {
        let root = temporary_root();
        let cache = IndexCache::new(&root);
        let cancelled = AtomicBool::new(false);
        let error = tauri::async_runtime::block_on(refresh_index(
            &cache,
            "https://cdn.attacker.example/index.json",
            &cancelled,
        ))
        .expect_err("refused");
        assert!(error.contains("not where this build expects it"));
        fs::remove_dir_all(root).ok();
    }
}
