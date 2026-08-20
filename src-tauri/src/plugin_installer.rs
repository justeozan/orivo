//! Installing and removing plugin packages.
//!
//! A plugin arrives as a signed `.orivo-plugin` archive. The host reads it
//! wholly in memory, re-derives every digest, checks the package against the
//! v1 policy in [`crate::plugin_manifest`], and only then materialises it on
//! disk — into a staging directory that is renamed into place, so a failed or
//! cancelled install can never leave a half-written plugin for the registry to
//! discover.
//!
//! Two channels, two signature policies. A package pulled from the embedded
//! registry must carry a signature made by Orivo's own key. A package the user
//! picks by hand may be unsigned; it installs as `Development` and every
//! surface that shows it says so.

use crate::plugin_manifest::{
    HostCompatibility, PackageEntry, PackageInspection, PackageSignatureStatus, PluginManifest,
    ValidatedPluginPackage, validate_plugin_package,
};
use crate::plugin_registry::{PLUGINS_DIRECTORY, PluginRegistry, PluginState};
use crate::plugin_runtime::PluginRuntime;
use ed25519_dalek::{Signature, VerifyingKey};
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};

const MAIN_WINDOW_LABEL: &str = "main";
pub const PLUGIN_INSTALL_EVENT: &str = "plugin-install-status";

const REGISTRY_JSON: &str = include_str!("../resources/plugin-registry.json");
const MANIFEST_FILE: &str = "manifest.json";
const COMPONENT_FILE: &str = "component.wasm";
const SIGNATURE_FILE: &str = "signature.ed25519";
const STAGING_DIRECTORY: &str = ".staging";
/// Mirrors `MAX_PACKAGE_BYTES` in the manifest policy, with headroom for the
/// signature and the archive's own framing.
const MAX_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PACKAGE_ENTRIES: usize = 64;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Orivo's release signing key. A registry package that is not signed by this
/// key is refused: the registry is a distribution channel, not a trust
/// boundary the user is asked to evaluate per download.
///
/// The matching private key lives only in the plugin project's gitignored
/// `keys/` directory. Rotating it is a host release, which is the point: a
/// compromised signing key cannot be replaced by anything a package says.
const RELEASE_PUBLIC_KEY_BASE64: &str = "OX9NRNeAEL2tEyS54qUTJ14cFS6smfLu6JoPzbiXG9w=";

// ---------------------------------------------------------------------------
// Embedded registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryEntry {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    summary: String,
    url: String,
    sha256: String,
    size_bytes: u64,
}

fn registry_entries() -> Vec<RegistryEntry> {
    serde_json::from_str::<Vec<RegistryEntry>>(REGISTRY_JSON)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| {
            entry.url.starts_with("https://")
                && entry.sha256.len() == 64
                && entry.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                && entry.size_bytes > 0
                && entry.size_bytes <= MAX_PACKAGE_BYTES
        })
        .collect()
}

// ---------------------------------------------------------------------------
// IPC views
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub extensions: Vec<String>,
    pub state: PluginState,
    pub message: String,
    pub trusted: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AvailablePluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub summary: String,
    pub size_bytes: u64,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginCatalogView {
    pub installed: Vec<InstalledPluginView>,
    pub available: Vec<AvailablePluginView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginInstallProgress {
    plugin_id: String,
    phase: &'static str,
    percent: u8,
    message: String,
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct PluginInstallerService {
    plugin_root: PathBuf,
    host_version: &'static str,
}

impl PluginInstallerService {
    pub fn new(plugin_root: PathBuf, host_version: &'static str) -> Self {
        Self {
            plugin_root,
            host_version,
        }
    }

    fn installed(&self) -> Vec<InstalledPluginView> {
        let Ok(runtime) = PluginRuntime::new() else {
            return Vec::new();
        };
        PluginRegistry::new(
            self.plugin_root.clone(),
            HostCompatibility::v1(self.host_version),
        )
        .installed_plugins(&runtime)
        .into_iter()
        .map(|record| InstalledPluginView {
            trusted: trust_marker_path(&self.plugin_root, &record.id).is_file(),
            id: record.id,
            name: record.name,
            version: record.version,
            extensions: record.extension_names,
            state: record.state,
            message: record.message,
        })
        .collect()
    }
}

/// A one-byte marker the installer writes beside a plugin it accepted with a
/// release signature. It is host-owned state about *how* a plugin arrived, so
/// it deliberately lives outside the plugin's own directory, where a package
/// could otherwise declare itself trusted.
fn trust_marker_path(plugin_root: &Path, plugin_id: &str) -> PathBuf {
    plugin_root
        .join(STAGING_DIRECTORY)
        .join("trusted")
        .join(plugin_id)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_plugin_catalog(
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<PluginCatalogView, String> {
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || {
        let installed = service.installed();
        let available = registry_entries()
            .into_iter()
            .map(|entry| AvailablePluginView {
                installed: installed.iter().any(|plugin| plugin.id == entry.id),
                id: entry.id,
                name: entry.name,
                version: entry.version,
                summary: entry.summary,
                size_bytes: entry.size_bytes,
            })
            .collect();
        PluginCatalogView {
            installed,
            available,
        }
    })
    .await
    .map_err(|_| "The plugin catalogue could not be read.".to_string())
}

#[tauri::command]
pub async fn install_plugin_from_registry(
    app: AppHandle,
    plugin_id: String,
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<(), String> {
    let service = Arc::clone(&service);
    let entry = registry_entries()
        .into_iter()
        .find(|entry| entry.id == plugin_id)
        .ok_or_else(|| "This plugin is not in Orivo's registry.".to_string())?;

    publish(&app, &entry.id, "downloading", 0, "Downloading…");
    let bytes = download_package(&app, &entry).await?;

    publish(&app, &entry.id, "verifying", 100, "Verifying…");
    let service_for_install = Arc::clone(&service);
    let id_for_install = entry.id.clone();
    let installed = tauri::async_runtime::spawn_blocking(move || {
        // The registry is a distribution channel, not a trust decision the
        // user is asked to make per download: a release signature is required.
        install_package(&service_for_install, &bytes, SignaturePolicy::ReleaseOnly)
    })
    .await
    .map_err(|_| "The installation did not finish.".to_string())?;

    match installed {
        Ok(id) if id == id_for_install => {
            publish(&app, &id, "installed", 100, "Installed.");
            Ok(())
        }
        Ok(_) => {
            let message = "The package does not contain the plugin the registry names.";
            publish(&app, &id_for_install, "failed", 0, message);
            Err(message.into())
        }
        Err(error) => {
            publish(&app, &id_for_install, "failed", 0, &error);
            Err(error)
        }
    }
}

/// Synchronous on purpose: macOS requires the native picker on the main
/// thread, which is where Tauri runs a non-async command. The work that
/// follows is bounded by `MAX_PACKAGE_BYTES` and reads an already-local file,
/// so it does not need to leave that thread — the same shape `import_game`
/// uses for picking an executable.
#[tauri::command]
pub fn install_plugin_from_file(
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<Option<String>, String> {
    let Some(selected) = rfd::FileDialog::new()
        .set_title("Choose an Orivo plugin package")
        .add_filter("Orivo plugin", &["orivo-plugin"])
        .pick_file()
    else {
        return Ok(None);
    };
    let bytes = read_bounded_file(&selected, MAX_PACKAGE_BYTES)
        .map_err(|_| "This package could not be read.".to_string())?;
    // A package the user picked by hand may be unsigned. It installs as a
    // development build and every surface that lists it says so.
    install_package(&service, &bytes, SignaturePolicy::AllowUnsigned).map(Some)
}

#[tauri::command]
pub async fn uninstall_plugin(
    plugin_id: String,
    service: State<'_, Arc<PluginInstallerService>>,
) -> Result<(), String> {
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || {
        if !valid_plugin_directory_name(&plugin_id) {
            return Err("That is not a plugin Orivo installed.".to_string());
        }
        let directory = service.plugin_root.join(&plugin_id);
        // `symlink_metadata` never follows: a symlink planted in the plugin
        // root must not turn a removal into a delete somewhere else.
        let metadata = fs::symlink_metadata(&directory)
            .map_err(|_| "That plugin is not installed.".to_string())?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err("That is not a plugin Orivo installed.".into());
        }
        fs::remove_dir_all(&directory)
            .map_err(|_| "The plugin could not be removed.".to_string())?;
        let _ = fs::remove_file(trust_marker_path(&service.plugin_root, &plugin_id));
        Ok(())
    })
    .await
    .map_err(|_| "The removal did not finish.".to_string())?
}

fn publish(app: &AppHandle, plugin_id: &str, phase: &'static str, percent: u8, message: &str) {
    let _ = app.emit_to(
        MAIN_WINDOW_LABEL,
        PLUGIN_INSTALL_EVENT,
        PluginInstallProgress {
            plugin_id: plugin_id.to_string(),
            phase,
            percent: percent.min(100),
            message: message.to_string(),
        },
    );
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

async fn download_package(app: &AppHandle, entry: &RegistryEntry) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|_| "The download client could not start.".to_string())?;
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
        let chunk = chunk.map_err(|_| "The download was interrupted.".to_string())?;
        if bytes.len() as u64 + chunk.len() as u64 > entry.size_bytes {
            return Err("The package is larger than the registry declares.".into());
        }
        bytes.extend_from_slice(&chunk);
        publish(
            app,
            &entry.id,
            "downloading",
            ((bytes.len() as f64 / entry.size_bytes as f64) * 100.0) as u8,
            "Downloading…",
        );
    }
    if bytes.len() as u64 != entry.size_bytes {
        return Err("The package size does not match the registry.".into());
    }
    if hex_digest(&bytes) != entry.sha256.to_ascii_lowercase() {
        return Err("The package failed its integrity check.".into());
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Package reading, validation and installation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignaturePolicy {
    /// Only a package signed by Orivo's release key is accepted.
    ReleaseOnly,
    /// An unsigned package is accepted and marked as a development build. A
    /// present-but-wrong signature is still a hard failure.
    AllowUnsigned,
}

/// Every entry of the archive, already bounded and read into memory.
type PackageFiles = BTreeMap<String, Vec<u8>>;

fn install_package(
    service: &PluginInstallerService,
    bytes: &[u8],
    policy: SignaturePolicy,
) -> Result<String, String> {
    let files = read_package(bytes)?;
    let manifest_bytes = files
        .get(MANIFEST_FILE)
        .ok_or_else(|| "The package has no manifest.".to_string())?;
    let manifest = serde_json::from_slice::<PluginManifest>(manifest_bytes)
        .map_err(|_| "The package manifest is not valid JSON.".to_string())?;

    let signature = signature_status(manifest_bytes, files.get(SIGNATURE_FILE));
    let signature = match (policy, signature) {
        (SignaturePolicy::ReleaseOnly, PackageSignatureStatus::Trusted) => {
            PackageSignatureStatus::Trusted
        }
        (SignaturePolicy::ReleaseOnly, _) => {
            return Err("This package is not signed by Orivo.".into());
        }
        (SignaturePolicy::AllowUnsigned, PackageSignatureStatus::Invalid) => {
            return Err("The package signature is invalid.".into());
        }
        (SignaturePolicy::AllowUnsigned, PackageSignatureStatus::Missing) => {
            PackageSignatureStatus::Development
        }
        (SignaturePolicy::AllowUnsigned, status) => status,
    };

    let inspection = PackageInspection {
        entries: files
            .iter()
            .map(|(path, contents)| PackageEntry {
                path: path.clone(),
                byte_size: contents.len() as u64,
            })
            .collect(),
        signature,
    };
    let validated: ValidatedPluginPackage = validate_plugin_package(manifest, &inspection)
        .map_err(|errors| {
            format!("This package does not meet Orivo's plugin contract: {errors}")
        })?;

    // The policy check above proves the manifest is well formed; this proves
    // the bytes in the archive are the ones it describes.
    for artifact in &validated.manifest.manifest().artifacts {
        let contents = files
            .get(&artifact.path)
            .ok_or_else(|| "The package is missing a declared artifact.".to_string())?;
        if contents.len() as u64 != artifact.byte_size
            || !hex_digest(contents).eq_ignore_ascii_case(&artifact.sha256)
        {
            return Err("A package artifact does not match its manifest entry.".into());
        }
    }

    let runtime =
        PluginRuntime::new().map_err(|_| "The plugin runtime is unavailable.".to_string())?;
    let component = files
        .get(COMPONENT_FILE)
        .ok_or_else(|| "The package has no component.".to_string())?;
    runtime
        .preflight_component(component)
        .map_err(|_| "The plugin component did not pass WebAssembly validation.".to_string())?;

    let plugin_id = validated.manifest.id().to_string();
    if !valid_plugin_directory_name(&plugin_id) {
        return Err("The plugin identity is not usable as a directory.".into());
    }
    materialise(service, &plugin_id, &files, signature)?;
    Ok(plugin_id)
}

/// Unpack into a staging directory and rename it into place. A reader that
/// walks the plugin root during an install sees either the previous plugin or
/// the new one, never a directory being filled in.
fn materialise(
    service: &PluginInstallerService,
    plugin_id: &str,
    files: &PackageFiles,
    signature: PackageSignatureStatus,
) -> Result<(), String> {
    let staging_root = service.plugin_root.join(STAGING_DIRECTORY);
    let staging = staging_root.join(plugin_id);
    fs::create_dir_all(&staging_root)
        .map_err(|_| "The plugin folder is unavailable.".to_string())?;
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|_| "The plugin folder is unavailable.".to_string())?;

    for (path, contents) in files {
        if path == SIGNATURE_FILE {
            continue;
        }
        let target = staging.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|_| "The plugin folder is unavailable.".to_string())?;
        }
        fs::write(&target, contents)
            .map_err(|_| "The plugin could not be written to disk.".to_string())?;
    }

    let destination = service.plugin_root.join(plugin_id);
    let _ = fs::remove_dir_all(&destination);
    fs::rename(&staging, &destination)
        .map_err(|_| "The plugin could not be installed.".to_string())?;

    let marker = trust_marker_path(&service.plugin_root, plugin_id);
    if let Some(parent) = marker.parent() {
        let _ = fs::create_dir_all(parent);
    }
    match signature {
        PackageSignatureStatus::Trusted => {
            let _ = fs::write(&marker, b"1");
        }
        _ => {
            let _ = fs::remove_file(&marker);
        }
    }
    Ok(())
}

/// Read the gzipped tar wholly in memory, bounded on entry count, per-entry
/// size and total size. Nothing is written to disk until the whole archive has
/// been read and accepted.
fn read_package(bytes: &[u8]) -> Result<PackageFiles, String> {
    if bytes.len() as u64 > MAX_PACKAGE_BYTES {
        return Err("This package is larger than Orivo allows.".into());
    }
    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
    let entries = archive
        .entries()
        .map_err(|_| "The package is not a readable archive.".to_string())?;
    let mut files = PackageFiles::new();
    let mut total = 0_u64;
    for entry in entries {
        let mut entry = entry.map_err(|_| "The package archive is damaged.".to_string())?;
        if !entry.header().entry_type().is_file() {
            // Directories carry no payload, and a link could point anywhere on
            // the host. Only regular files are ever taken from a package.
            continue;
        }
        let path = entry
            .path()
            .map_err(|_| "The package contains an unreadable path.".to_string())?
            .to_string_lossy()
            .into_owned();
        if !safe_entry_path(&path) {
            return Err("The package contains an unsafe path.".into());
        }
        if files.len() >= MAX_PACKAGE_ENTRIES {
            return Err("The package contains too many files.".into());
        }
        let declared = entry.header().size().unwrap_or(0);
        total = total.saturating_add(declared);
        if declared > MAX_PACKAGE_BYTES || total > MAX_PACKAGE_BYTES {
            return Err("This package is larger than Orivo allows.".into());
        }
        let mut contents = Vec::with_capacity(declared.min(1024 * 1024) as usize);
        entry
            .take(MAX_PACKAGE_BYTES)
            .read_to_end(&mut contents)
            .map_err(|_| "The package archive is damaged.".to_string())?;
        if files.insert(path, contents).is_some() {
            return Err("The package declares the same file twice.".into());
        }
    }
    if files.is_empty() {
        return Err("The package is empty.".into());
    }
    Ok(files)
}

/// A package path is relative, has no traversal segment, and stays inside the
/// shallow shape the manifest policy allows.
fn safe_entry_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 256
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn valid_plugin_directory_name(value: &str) -> bool {
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

/// The signature covers the SHA-256 digest of `manifest.json`, which in turn
/// pins every artifact by digest. Signing the manifest therefore signs the
/// whole package without the signer having to hash the archive framing.
fn signature_status(manifest_bytes: &[u8], signature: Option<&Vec<u8>>) -> PackageSignatureStatus {
    let Some(signature) = signature else {
        return PackageSignatureStatus::Missing;
    };
    let Ok(signature) = <[u8; 64]>::try_from(signature.as_slice()) else {
        return PackageSignatureStatus::Invalid;
    };
    let Some(key) = release_public_key() else {
        // No release key is compiled in, so provenance cannot be established
        // either way. This is not `Invalid` — that verdict is reserved for a
        // signature that demonstrably fails a key we hold, and reporting it
        // here would block the plugin author's own signed builds. Integrity is
        // unaffected: every artifact is checked against the manifest digests
        // regardless, and only the release channel requires `Trusted`.
        return PackageSignatureStatus::Development;
    };
    let digest = Sha256::digest(manifest_bytes);
    match key.verify_strict(&digest, &Signature::from_bytes(&signature)) {
        Ok(()) => PackageSignatureStatus::Trusted,
        Err(_) => PackageSignatureStatus::Invalid,
    }
}

fn release_public_key() -> Option<VerifyingKey> {
    let decoded = decode_base64(RELEASE_PUBLIC_KEY_BASE64)?;
    VerifyingKey::from_bytes(&<[u8; 32]>::try_from(decoded.as_slice()).ok()?).ok()
}

/// A tiny standard-alphabet decoder. The only base64 this crate reads is a
/// 32-byte key compiled into the binary, so a dependency would be more
/// surface than the four lines it replaces.
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

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_bounded_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, std::io::Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsafe file",
        ));
    }
    fs::read(path)
}

pub fn service_for(app: &AppHandle) -> Option<Arc<PluginInstallerService>> {
    app.try_state::<Arc<PluginInstallerService>>()
        .map(|state| Arc::clone(&state))
}

pub fn plugin_root_for(app_data: &Path) -> PathBuf {
    app_data.join(PLUGINS_DIRECTORY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::time::{SystemTime, UNIX_EPOCH};

    const EMPTY_COMPONENT: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

    fn temporary_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "orivo-plugin-installer-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn manifest_json(id: &str, catalog: &[u8]) -> Vec<u8> {
        format!(
            r#"{{
              "id": "{id}",
              "name": "Quiky",
              "version": "0.1.0",
              "sdk": "orivo-plugin@1",
              "minOrivoVersion": "0.3.0",
              "extensions": ["installer"],
              "capabilities": ["network_fetch"],
              "networkDomains": ["cdn.openttd.org"],
              "artifacts": [
                {{"path":"component.wasm","kind":"component","sha256":"{}","byteSize":{}}},
                {{"path":"assets/catalog.json","kind":"asset","sha256":"{}","byteSize":{}}}
              ]
            }}"#,
            hex_digest(EMPTY_COMPONENT),
            EMPTY_COMPONENT.len(),
            hex_digest(catalog),
            catalog.len(),
        )
        .into_bytes()
    }

    fn package(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for (path, contents) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, contents.as_slice())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn valid_package(id: &str) -> Vec<u8> {
        let catalog = br#"{"version":1,"titles":[]}"#.to_vec();
        package(&[
            ("manifest.json", manifest_json(id, &catalog)),
            ("component.wasm", EMPTY_COMPONENT.to_vec()),
            ("assets/catalog.json", catalog),
        ])
    }

    fn service(root: &Path) -> PluginInstallerService {
        PluginInstallerService::new(root.to_path_buf(), "0.3.0")
    }

    #[test]
    fn an_unsigned_package_installs_only_through_the_sideload_channel() {
        let root = temporary_root("sideload");
        fs::create_dir_all(&root).unwrap();
        let service = service(&root);
        let bytes = valid_package("com.orivo.quiky");

        assert_eq!(
            install_package(&service, &bytes, SignaturePolicy::ReleaseOnly),
            Err("This package is not signed by Orivo.".into())
        );
        assert!(!root.join("com.orivo.quiky").exists());

        let installed =
            install_package(&service, &bytes, SignaturePolicy::AllowUnsigned).expect("installs");
        assert_eq!(installed, "com.orivo.quiky");
        assert!(root.join("com.orivo.quiky/manifest.json").is_file());
        assert!(root.join("com.orivo.quiky/component.wasm").is_file());
        assert!(root.join("com.orivo.quiky/assets/catalog.json").is_file());
        // A sideloaded package is never marked trusted.
        assert!(!trust_marker_path(&root, "com.orivo.quiky").exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_package_whose_artifact_bytes_changed_is_refused() {
        let root = temporary_root("tampered");
        fs::create_dir_all(&root).unwrap();
        let service = service(&root);
        let catalog = br#"{"version":1,"titles":[]}"#.to_vec();
        let bytes = package(&[
            ("manifest.json", manifest_json("com.orivo.quiky", &catalog)),
            ("component.wasm", EMPTY_COMPONENT.to_vec()),
            // The manifest still pins the original digest and length.
            (
                "assets/catalog.json",
                br#"{"version":1,"titles":[ ]}"#.to_vec(),
            ),
        ]);

        assert_eq!(
            install_package(&service, &bytes, SignaturePolicy::AllowUnsigned),
            Err("A package artifact does not match its manifest entry.".into())
        );
        assert!(!root.join("com.orivo.quiky").exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_package_carrying_an_undeclared_payload_is_refused() {
        let root = temporary_root("payload");
        fs::create_dir_all(&root).unwrap();
        let service = service(&root);
        let catalog = br#"{"version":1,"titles":[]}"#.to_vec();
        let bytes = package(&[
            ("manifest.json", manifest_json("com.orivo.quiky", &catalog)),
            ("component.wasm", EMPTY_COMPONENT.to_vec()),
            ("assets/catalog.json", catalog),
            ("aria2c.exe", b"MZ".to_vec()),
        ]);

        assert!(
            install_package(&service, &bytes, SignaturePolicy::AllowUnsigned)
                .is_err_and(|error| error.contains("plugin contract"))
        );
        assert!(!root.join("com.orivo.quiky").exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_traversal_path_never_leaves_the_plugin_root() {
        assert!(safe_entry_path("assets/catalog.json"));
        assert!(!safe_entry_path("../escape.json"));
        assert!(!safe_entry_path("/etc/passwd"));
        assert!(!safe_entry_path("assets/../../escape.json"));
        assert!(!safe_entry_path("assets\\catalog.json"));
    }

    #[test]
    fn signature_verdicts_separate_absence_from_malformation() {
        let manifest = b"{}";
        assert_eq!(
            signature_status(manifest, None),
            PackageSignatureStatus::Missing
        );
        // A signature of the wrong length can never be evaluated by anyone.
        assert_eq!(
            signature_status(manifest, Some(&vec![0_u8; 8])),
            PackageSignatureStatus::Invalid
        );
        // Well formed, but it does not verify against the compiled release
        // key, and that is exactly what `Invalid` is reserved for.
        assert_eq!(
            signature_status(manifest, Some(&vec![0_u8; 64])),
            PackageSignatureStatus::Invalid
        );
    }

    /// The end-to-end check against the real artefact the plugin project
    /// builds: its signature verifies against the compiled release key, so it
    /// is accepted by the strict registry channel and marked trusted on disk.
    #[test]
    fn the_plugin_projects_own_package_is_trusted_by_the_release_key() {
        let artefact = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace")
            .join("plugin-quiky/dist/com.orivo.quiky-0.1.0.orivo-plugin");
        let Ok(bytes) = fs::read(&artefact) else {
            // The plugin is a separate project; skip when it is not built.
            return;
        };
        let root = temporary_root("real-package");
        fs::create_dir_all(&root).unwrap();
        let service = service(&root);

        let id = install_package(&service, &bytes, SignaturePolicy::ReleaseOnly)
            .expect("the released package installs through the strict channel");
        assert_eq!(id, "com.orivo.quiky");
        assert!(root.join("com.orivo.quiky/manifest.json").is_file());
        assert!(root.join("com.orivo.quiky/assets/catalog.json").is_file());
        // The signature file itself is never written into the plugin tree.
        assert!(!root.join("com.orivo.quiky/signature.ed25519").exists());
        assert!(
            trust_marker_path(&root, "com.orivo.quiky").is_file(),
            "marked trusted"
        );

        // A manifest edited after signing breaks the signature, and the strict
        // channel is the one that must notice.
        let files = read_package(&bytes).unwrap();
        let mut tampered = files.clone();
        let mut manifest = tampered.get("manifest.json").unwrap().clone();
        manifest.push(b' ');
        tampered.insert("manifest.json".into(), manifest);
        let repacked = package(
            &tampered
                .iter()
                .map(|(path, contents)| (path.as_str(), contents.clone()))
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            install_package(&service, &repacked, SignaturePolicy::ReleaseOnly),
            Err("This package is not signed by Orivo.".into())
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn uninstalling_removes_only_a_real_plugin_directory() {
        assert!(valid_plugin_directory_name("com.orivo.quiky"));
        assert!(!valid_plugin_directory_name("../../etc"));
        assert!(!valid_plugin_directory_name("quiky"));
        assert!(!valid_plugin_directory_name("com.Orivo.quiky"));
    }

    #[test]
    fn base64_decodes_a_key_and_rejects_rubbish() {
        assert_eq!(decode_base64("QUJD").as_deref(), Some(&b"ABC"[..]));
        assert_eq!(decode_base64(""), None);
        assert_eq!(decode_base64("!!!!"), None);
    }
}
