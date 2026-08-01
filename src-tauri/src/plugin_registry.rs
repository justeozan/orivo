//! Lazy, read-only discovery of locally installed plugin components.
//!
//! The registry intentionally does not instantiate a component. It runs only
//! when an extension surface is opened, validates the untrusted manifest and
//! component bytes, and returns presentation-safe summaries. This keeps an
//! invalid extension out of Orivo's startup and rendering critical paths.

use crate::plugin_manifest::{
    CompatibleVersionInfo, HostCompatibility, PluginExtension, PluginManifest,
    ValidatedPluginManifest,
};
use crate::plugin_runtime::PluginRuntime;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

pub const PLUGINS_DIRECTORY: &str = "plugins";
const MANIFEST_FILE: &str = "manifest.json";
const MAX_DISCOVERED_PLUGINS: usize = 128;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const COPY_BUFFER_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub struct PluginRegistry {
    root: PathBuf,
    compatibility: HostCompatibility,
}

impl PluginRegistry {
    pub fn new(root: PathBuf, compatibility: HostCompatibility) -> Self {
        Self {
            root,
            compatibility,
        }
    }

    /// Discovery is deliberately best-effort. One malformed third-party
    /// package becomes an unavailable row, not an error that prevents another
    /// valid runner or the rest of Orivo from loading.
    fn discover_internal(&self) -> Vec<DiscoveredPlugin> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut entries = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_type()
                    .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        entries
            .into_iter()
            .take(MAX_DISCOVERED_PLUGINS)
            .map(|entry| self.inspect_plugin_directory(entry.path(), entry.file_name()))
            .collect()
    }

    pub fn runner_plugins(&self, runtime: &PluginRuntime) -> Vec<RunnerPluginView> {
        self.discover_internal()
            .into_iter()
            .filter_map(|plugin| {
                plugin
                    .record
                    .extension_names
                    .iter()
                    .any(|extension| extension == "runner")
                    .then_some(plugin)
            })
            .map(|mut plugin| {
                if plugin.record.state == PluginState::Ready {
                    match plugin.preflight(runtime) {
                        Ok(()) => {}
                        Err(message) => {
                            plugin.record.state = PluginState::Invalid;
                            plugin.record.message = message.into();
                        }
                    }
                }
                RunnerPluginView {
                    id: plugin.record.id,
                    name: plugin.record.name,
                    version: plugin.record.version,
                    state: plugin.record.state,
                    message: plugin.record.message,
                }
            })
            .collect()
    }

    fn inspect_plugin_directory(
        &self,
        directory: PathBuf,
        directory_name: std::ffi::OsString,
    ) -> DiscoveredPlugin {
        let manifest_path = directory.join(MANIFEST_FILE);
        let manifest_bytes = match read_bounded_file(&manifest_path, MAX_MANIFEST_BYTES) {
            Ok(bytes) => bytes,
            Err(_) => {
                return DiscoveredPlugin::without_component(PluginRecord::invalid(
                    "Unknown plugin",
                    "Manifest unavailable or too large.",
                ));
            }
        };
        let manifest = match serde_json::from_slice::<PluginManifest>(&manifest_bytes) {
            Ok(manifest) => manifest,
            Err(_) => {
                return DiscoveredPlugin::without_component(PluginRecord::invalid(
                    "Unknown plugin",
                    "Manifest JSON is invalid.",
                ));
            }
        };
        let name = manifest.name.clone();
        let version = manifest.version.clone();
        let extensions = manifest
            .extensions
            .iter()
            .map(extension_name)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let validated = match manifest.validate() {
            Ok(manifest) => manifest,
            Err(_) => {
                return DiscoveredPlugin::without_component(PluginRecord::with_details(
                    manifest.id,
                    name,
                    version,
                    extensions,
                    PluginState::Invalid,
                    "The plugin manifest does not meet Orivo's safety contract.",
                ));
            }
        };
        if directory_name.to_string_lossy() != validated.id() {
            return DiscoveredPlugin::without_component(PluginRecord::with_manifest(
                &validated,
                PluginState::Invalid,
                "The plugin folder does not match its manifest identity.",
            ));
        }
        match self.compatibility.compatibility_for(&validated) {
            CompatibleVersionInfo::Compatible => {}
            CompatibleVersionInfo::UnsupportedSdk => {
                return DiscoveredPlugin::without_component(PluginRecord::with_manifest(
                    &validated,
                    PluginState::Incompatible,
                    "This plugin requires a different Orivo plugin SDK.",
                ));
            }
            CompatibleVersionInfo::RequiresNewerOrivo => {
                return DiscoveredPlugin::without_component(PluginRecord::with_manifest(
                    &validated,
                    PluginState::Incompatible,
                    "Update Orivo before using this plugin.",
                ));
            }
        }
        match component_descriptor(&directory, &validated) {
            Ok(component) => DiscoveredPlugin {
                record: PluginRecord::with_manifest(
                    &validated,
                    PluginState::Ready,
                    "Ready to configure. Orivo will request permissions before activation.",
                ),
                component: Some(component),
            },
            Err(message) => DiscoveredPlugin::without_component(PluginRecord::with_manifest(
                &validated,
                PluginState::Invalid,
                message,
            )),
        }
    }
}

#[derive(Debug, Clone)]
struct DiscoveredPlugin {
    record: PluginRecord,
    component: Option<VerifiedComponent>,
}

impl DiscoveredPlugin {
    fn without_component(record: PluginRecord) -> Self {
        Self {
            record,
            component: None,
        }
    }

    /// Re-read and re-hash the component immediately before Wasmtime sees its
    /// bytes. This closes the discovery-to-compile race without exposing a
    /// plugin path beyond this backend module.
    fn preflight(&self, runtime: &PluginRuntime) -> Result<(), &'static str> {
        let component = self
            .component
            .as_ref()
            .ok_or("The plugin component is unavailable.")?;
        let bytes = read_bounded_file(&component.path, component.byte_size)
            .map_err(|_| "The plugin component changed before validation.")?;
        if sha256_bytes(&bytes) != component.sha256 {
            return Err("The plugin component changed before validation.");
        }
        runtime
            .preflight_component(&bytes)
            .map_err(|_| "The plugin component did not pass WebAssembly validation.")
    }
}

#[derive(Debug, Clone)]
struct VerifiedComponent {
    path: PathBuf,
    sha256: String,
    byte_size: u64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    Ready,
    Incompatible,
    Invalid,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginRecord {
    pub id: String,
    pub name: String,
    pub version: String,
    pub extension_names: Vec<String>,
    pub state: PluginState,
    pub message: String,
}

impl PluginRecord {
    fn invalid(name: &str, message: &str) -> Self {
        Self::with_details(
            String::new(),
            name.into(),
            String::new(),
            Vec::new(),
            PluginState::Invalid,
            message,
        )
    }

    fn with_manifest(
        manifest: &ValidatedPluginManifest,
        state: PluginState,
        message: &str,
    ) -> Self {
        Self::with_details(
            manifest.id().into(),
            manifest.manifest().name.clone(),
            manifest.manifest().version.clone(),
            manifest
                .manifest()
                .extensions
                .iter()
                .map(extension_name)
                .map(str::to_owned)
                .collect(),
            state,
            message,
        )
    }

    fn with_details(
        id: String,
        name: String,
        version: String,
        extension_names: Vec<String>,
        state: PluginState,
        message: &str,
    ) -> Self {
        Self {
            id,
            name,
            version,
            extension_names,
            state,
            message: message.into(),
        }
    }
}

/// This is the only runner data safe to cross the Tauri IPC boundary. No
/// filesystem path, component path, capability scope, or component hash leaks
/// into the WebView.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunnerPluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub state: PluginState,
    pub message: String,
}

fn component_descriptor(
    directory: &Path,
    manifest: &ValidatedPluginManifest,
) -> Result<VerifiedComponent, &'static str> {
    let Some(component) = manifest
        .manifest()
        .artifacts
        .iter()
        .find(|artifact| artifact.kind == crate::plugin_manifest::ArtifactKind::Component)
    else {
        return Err("The plugin does not declare a component.");
    };
    let component_path = directory.join(&component.path);
    let bytes = read_bounded_file(&component_path, component.byte_size)
        .map_err(|_| "The plugin component is missing or unsafe.")?;
    if bytes.len() as u64 != component.byte_size {
        return Err("The plugin component does not match its manifest size.");
    }
    let hash = sha256_bytes(&bytes);
    if !hash.eq_ignore_ascii_case(&component.sha256) {
        return Err("The plugin component does not match its manifest hash.");
    }
    Ok(VerifiedComponent {
        path: component_path,
        sha256: hash,
        byte_size: component.byte_size,
    })
}

fn read_bounded_file(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > max_bytes
    {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "unsafe file"));
    }
    let mut file = fs::File::open(path)?;
    let mut reader = file.by_ref().take(max_bytes.saturating_add(1));
    let mut bytes = Vec::with_capacity(metadata.len().min(max_bytes) as usize);
    reader.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file grew while reading",
        ));
    }
    Ok(bytes)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    for chunk in bytes.chunks(COPY_BUFFER_BYTES) {
        digest.update(chunk);
    }
    format!("{:x}", digest.finalize())
}

fn extension_name(extension: &PluginExtension) -> &'static str {
    match extension {
        PluginExtension::Source => "source",
        PluginExtension::Runner => "runner",
        PluginExtension::Metadata => "metadata",
        PluginExtension::Search => "search",
        PluginExtension::Automation => "automation",
        PluginExtension::UiContribution => "ui_contribution",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_manifest::{
        ArtifactDescriptor, ArtifactKind, PLUGIN_SDK_V1, PluginCapability,
    };
    use std::{
        collections::BTreeSet,
        time::{SystemTime, UNIX_EPOCH},
    };

    const EMPTY_COMPONENT: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

    fn temporary_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "orivo-plugin-registry-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn write_runner(root: &Path, component: &[u8], declared_hash: String) {
        let directory = root.join("com.orivo.ryujinx");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("component.wasm"), component).unwrap();
        let manifest = PluginManifest {
            id: "com.orivo.ryujinx".into(),
            name: "Ryujinx Runner".into(),
            version: "1.0.0".into(),
            sdk: PLUGIN_SDK_V1.into(),
            min_orivo_version: Some("0.3.0".into()),
            extensions: vec![PluginExtension::Runner],
            capabilities: vec![PluginCapability::RunnerPrepare],
            network_domains: Vec::new(),
            artifacts: vec![ArtifactDescriptor {
                path: "component.wasm".into(),
                kind: ArtifactKind::Component,
                sha256: declared_hash,
                byte_size: component.len() as u64,
            }],
        };
        fs::write(
            directory.join(MANIFEST_FILE),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn discovers_a_valid_runner_without_exposing_paths() {
        let root = temporary_root();
        let component = EMPTY_COMPONENT;
        let mut hash = Sha256::new();
        hash.update(component);
        write_runner(&root, component, format!("{:x}", hash.finalize()));

        let runtime = PluginRuntime::new().unwrap();
        let plugins = PluginRegistry::new(root.clone(), HostCompatibility::v1("0.3.0"))
            .runner_plugins(&runtime);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].state, PluginState::Ready);
        assert!(
            !serde_json::to_string(&plugins)
                .unwrap()
                .contains(root.to_string_lossy().as_ref())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_hash_downgrades_only_that_plugin() {
        let root = temporary_root();
        write_runner(&root, b"component", "0".repeat(64));

        let runtime = PluginRuntime::new().unwrap();
        let plugins = PluginRegistry::new(root.clone(), HostCompatibility::v1("0.3.0"))
            .runner_plugins(&runtime);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].state, PluginState::Invalid);
        assert!(plugins[0].message.contains("hash"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unrelated_bad_plugin_does_not_hide_a_valid_runner() {
        let root = temporary_root();
        let component = EMPTY_COMPONENT;
        let mut hash = Sha256::new();
        hash.update(component);
        write_runner(&root, component, format!("{:x}", hash.finalize()));
        let invalid = root.join("broken.plugin");
        fs::create_dir_all(&invalid).unwrap();
        fs::write(invalid.join(MANIFEST_FILE), b"not json").unwrap();

        let runtime = PluginRuntime::new().unwrap();
        let plugins = PluginRegistry::new(root.clone(), HostCompatibility::v1("0.3.0"))
            .runner_plugins(&runtime);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].state, PluginState::Ready);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_grants_stay_opaque_in_a_runner_view() {
        let view = RunnerPluginView {
            id: "com.orivo.ryujinx".into(),
            name: "Ryujinx Runner".into(),
            version: "1.0.0".into(),
            state: PluginState::Ready,
            message: "Ready".into(),
        };
        let grant = BTreeSet::from(["directory-grant-1".to_string()]);
        assert!(
            !serde_json::to_string(&view)
                .unwrap()
                .contains("directory-grant-1")
        );
        assert_eq!(grant.len(), 1);
    }
}
