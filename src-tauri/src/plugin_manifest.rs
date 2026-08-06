//! Untrusted plugin manifests are validated here before a component is ever
//! compiled or invoked. This module deliberately has no filesystem, network,
//! process, Tauri, or WebView access: it is a pure policy boundary that can be
//! tested independently of the eventual Wasmtime host.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PLUGIN_SDK_V1: &str = "orivo-plugin@1";
pub const MAX_MANIFEST_NAME_LENGTH: usize = 96;
pub const MAX_MANIFEST_VERSION_LENGTH: usize = 32;
pub const MAX_PLUGIN_ID_LENGTH: usize = 128;
pub const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_PACKAGE_BYTES: u64 = 96 * 1024 * 1024;
const MAX_CAPABILITY_COUNT: usize = 16;
const MAX_DOMAIN_COUNT: usize = 16;
const MAX_ARTIFACT_COUNT: usize = 32;

/// The only kinds of functionality that a v1 plugin may expose. Additions are
/// explicit ABI changes rather than an open string that can silently gain
/// privileges.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PluginExtension {
    Source,
    Runner,
    Metadata,
    Search,
    Automation,
    UiContribution,
    /// Provides a host-executed acquisition catalogue: the plugin supplies the
    /// data, Orivo owns the download, the verification and the extraction.
    Installer,
}

/// A manifest declares what it may ask the user for. A declaration is not a
/// grant; `CapabilityGrant` below remains host-owned and revocable.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    LibraryRead,
    FilesRead,
    NetworkFetch,
    Secrets,
    RunnerPrepare,
    Notifications,
}

/// The component artifact is the only executable payload accepted by the v1
/// package format. Icons, translations and declarative settings schema may be
/// shipped as assets, but native binaries and scripts are not accepted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Component,
    Asset,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDescriptor {
    pub path: String,
    pub kind: ArtifactKind,
    pub sha256: String,
    pub byte_size: u64,
}

/// JSON object stored in `manifest.json` inside a `.orivo-plugin` package.
/// The installer must validate the enclosing package as well as this document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub sdk: String,
    #[serde(default)]
    pub min_orivo_version: Option<String>,
    #[serde(default)]
    pub extensions: Vec<PluginExtension>,
    #[serde(default)]
    pub capabilities: Vec<PluginCapability>,
    #[serde(default)]
    pub network_domains: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<ArtifactDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPluginManifest {
    manifest: PluginManifest,
}

impl ValidatedPluginManifest {
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn has_extension(&self, extension: PluginExtension) -> bool {
        self.manifest.extensions.contains(&extension)
    }

    pub fn declares(&self, capability: PluginCapability) -> bool {
        self.manifest.capabilities.contains(&capability)
    }

    /// Grants are persisted by the host, never by a plugin. This check makes
    /// it impossible to grant a capability that was not shown in the install
    /// consent screen or to widen the plugin's declared network allowlist.
    pub fn validate_grant(&self, grant: &CapabilityGrant) -> Result<(), GrantValidationError> {
        if grant.plugin_id != self.manifest.id {
            return Err(GrantValidationError::PluginMismatch);
        }
        if !self.declares(grant.capability) {
            return Err(GrantValidationError::CapabilityNotDeclared(
                grant.capability,
            ));
        }

        match (grant.capability, &grant.scope) {
            (PluginCapability::LibraryRead, CapabilityScope::LibraryGames(ids))
                if ids.iter().all(|id| valid_opaque_id(id, 256)) =>
            {
                Ok(())
            }
            (PluginCapability::FilesRead, CapabilityScope::DirectoryGrants(ids))
                if ids.iter().all(|id| valid_opaque_id(id, 256)) =>
            {
                Ok(())
            }
            (PluginCapability::NetworkFetch, CapabilityScope::Domains(domains))
                if !domains.is_empty()
                    && domains.iter().all(|domain| {
                        normalize_domain(domain).as_deref().is_some_and(|domain| {
                            self.manifest.network_domains.contains(&domain.to_string())
                        })
                    }) =>
            {
                Ok(())
            }
            (PluginCapability::Secrets, CapabilityScope::SecretNames(names))
                if names.iter().all(|name| valid_opaque_id(name, 128)) =>
            {
                Ok(())
            }
            (PluginCapability::RunnerPrepare, CapabilityScope::RunnerProfiles(ids))
                if !ids.is_empty() && ids.iter().all(|id| valid_opaque_id(id, 128)) =>
            {
                Ok(())
            }
            (PluginCapability::Notifications, CapabilityScope::Notifications) => Ok(()),
            _ => Err(GrantValidationError::InvalidScope(grant.capability)),
        }
    }
}

impl PluginManifest {
    pub fn validate(&self) -> Result<ValidatedPluginManifest, ManifestValidationErrors> {
        let mut errors = Vec::new();

        if !valid_plugin_id(&self.id) {
            errors.push("plugin id must be a lowercase reverse-DNS identifier".into());
        }
        if self.name.trim().is_empty() || self.name.chars().count() > MAX_MANIFEST_NAME_LENGTH {
            errors.push("plugin name must be between 1 and 96 characters".into());
        }
        if !valid_semver(&self.version) || self.version.len() > MAX_MANIFEST_VERSION_LENGTH {
            errors.push("plugin version must be a semantic version such as 1.0.0".into());
        }
        if self.sdk != PLUGIN_SDK_V1 {
            errors.push(format!("plugin sdk must be {PLUGIN_SDK_V1}"));
        }
        if self
            .min_orivo_version
            .as_deref()
            .is_some_and(|version| !valid_semver(version))
        {
            errors.push("minimum Orivo version must be a semantic version".into());
        }
        if self.extensions.is_empty() {
            errors.push("plugin must declare at least one extension".into());
        }
        if !unique(&self.extensions) {
            errors.push("plugin extensions must not contain duplicates".into());
        }
        if self.capabilities.len() > MAX_CAPABILITY_COUNT || !unique(&self.capabilities) {
            errors.push("plugin capabilities must be unique and bounded".into());
        }
        if self.network_domains.len() > MAX_DOMAIN_COUNT {
            errors.push("plugin network domains exceed the v1 limit".into());
        }
        let mut normalized_domains = BTreeSet::new();
        for domain in &self.network_domains {
            match normalize_domain(domain) {
                Some(normalized) if normalized_domains.insert(normalized.clone()) => {}
                _ => errors.push("plugin network domains must be unique HTTPS host names".into()),
            }
        }
        if self.capabilities.contains(&PluginCapability::NetworkFetch)
            != !self.network_domains.is_empty()
        {
            errors.push("network domains require, and require only, network_fetch".into());
        }
        if self.extensions.contains(&PluginExtension::Runner)
            && !self.capabilities.contains(&PluginCapability::RunnerPrepare)
        {
            errors.push("runner plugins must declare runner_prepare".into());
        }
        // The host, not the plugin, performs the fetch. Requiring the
        // declaration keeps the download allowlist visible in the manifest the
        // user consents to instead of hiding it inside catalogue data.
        if self.extensions.contains(&PluginExtension::Installer)
            && !self.capabilities.contains(&PluginCapability::NetworkFetch)
        {
            errors.push("installer plugins must declare network_fetch".into());
        }
        if self.artifacts.len() > MAX_ARTIFACT_COUNT || self.artifacts.is_empty() {
            errors.push("plugin must contain a bounded non-empty artifact list".into());
        }
        if self
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == ArtifactKind::Component)
            .count()
            != 1
        {
            errors.push("plugin must declare exactly one component artifact".into());
        }
        let mut artifact_paths = BTreeSet::new();
        let mut package_bytes = 0_u64;
        for artifact in &self.artifacts {
            package_bytes = package_bytes.saturating_add(artifact.byte_size);
            if !valid_package_path(&artifact.path)
                || !artifact_paths.insert(artifact.path.clone())
                || !valid_sha256(&artifact.sha256)
                || artifact.byte_size > MAX_ARTIFACT_BYTES
            {
                errors.push("plugin artifact path, hash or size is invalid".into());
            }
            if artifact.kind == ArtifactKind::Component && artifact.path != "component.wasm" {
                errors.push("the component artifact must be component.wasm".into());
            }
            if artifact.kind == ArtifactKind::Asset && !valid_asset_path(&artifact.path) {
                errors.push("plugin assets must be non-executable declarative files".into());
            }
        }
        if package_bytes > MAX_PACKAGE_BYTES {
            errors.push("plugin package exceeds the v1 size limit".into());
        }

        if errors.is_empty() {
            let mut manifest = self.clone();
            // A validated manifest has one canonical domain representation,
            // so case differences cannot accidentally widen or invalidate a
            // later host grant comparison.
            manifest.network_domains = normalized_domains.into_iter().collect();
            Ok(ValidatedPluginManifest { manifest })
        } else {
            Err(ManifestValidationErrors(errors))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestValidationErrors(pub Vec<String>);

impl std::fmt::Display for ManifestValidationErrors {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid plugin manifest: {}", self.0.join("; "))
    }
}

impl std::error::Error for ManifestValidationErrors {}

/// Compatibility remains a host decision. A component can be a valid package
/// yet be unavailable on an older Orivo host without being installed or run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostCompatibility {
    pub orivo_version: &'static str,
    pub sdk: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatibleVersionInfo {
    Compatible,
    UnsupportedSdk,
    RequiresNewerOrivo,
}

impl HostCompatibility {
    pub const fn v1(orivo_version: &'static str) -> Self {
        Self {
            orivo_version,
            sdk: PLUGIN_SDK_V1,
        }
    }

    pub fn compatibility_for(&self, manifest: &ValidatedPluginManifest) -> CompatibleVersionInfo {
        if manifest.manifest.sdk != self.sdk {
            return CompatibleVersionInfo::UnsupportedSdk;
        }
        if manifest
            .manifest
            .min_orivo_version
            .as_deref()
            .is_some_and(|minimum| {
                version_at_least(self.orivo_version, minimum).is_none_or(|ok| !ok)
            })
        {
            return CompatibleVersionInfo::RequiresNewerOrivo;
        }
        CompatibleVersionInfo::Compatible
    }
}

/// A persisted grant uses opaque IDs generated by the host. In particular,
/// directory grants do not expose raw filesystem paths to the plugin manifest
/// or to the WebView.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityGrant {
    pub plugin_id: String,
    pub capability: PluginCapability,
    pub scope: CapabilityScope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "values", rename_all = "snake_case")]
pub enum CapabilityScope {
    LibraryGames(BTreeSet<String>),
    DirectoryGrants(BTreeSet<String>),
    Domains(BTreeSet<String>),
    SecretNames(BTreeSet<String>),
    RunnerProfiles(BTreeSet<String>),
    Notifications,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantValidationError {
    PluginMismatch,
    CapabilityNotDeclared(PluginCapability),
    InvalidScope(PluginCapability),
}

impl std::fmt::Display for GrantValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PluginMismatch => write!(formatter, "grant belongs to a different plugin"),
            Self::CapabilityNotDeclared(capability) => {
                write!(
                    formatter,
                    "plugin did not declare capability {capability:?}"
                )
            }
            Self::InvalidScope(capability) => write!(formatter, "invalid scope for {capability:?}"),
        }
    }
}

impl std::error::Error for GrantValidationError {}

/// The archive reader supplies this neutral representation. Keeping ZIP/TAR
/// parsing outside the policy module lets the installer reject unsafe packages
/// before extraction and makes the package rules easy to unit test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInspection {
    pub entries: Vec<PackageEntry>,
    pub signature: PackageSignatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageEntry {
    pub path: String,
    pub byte_size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageSignatureStatus {
    Trusted,
    Development,
    Missing,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPluginPackage {
    pub manifest: ValidatedPluginManifest,
    pub signature: PackageSignatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageValidationErrors(pub Vec<String>);

impl std::fmt::Display for PackageValidationErrors {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid plugin package: {}", self.0.join("; "))
    }
}

impl std::error::Error for PackageValidationErrors {}

pub fn validate_plugin_package(
    manifest: PluginManifest,
    inspection: &PackageInspection,
) -> Result<ValidatedPluginPackage, PackageValidationErrors> {
    let manifest = manifest
        .validate()
        .map_err(|errors| PackageValidationErrors(errors.0))?;
    let mut errors = Vec::new();
    let mut paths = BTreeSet::new();
    let allowed_paths = manifest
        .manifest
        .artifacts
        .iter()
        .map(|artifact| artifact.path.as_str())
        .chain(["manifest.json", "signature.ed25519"])
        .collect::<BTreeSet<_>>();
    let mut total_bytes = 0_u64;

    for entry in &inspection.entries {
        total_bytes = total_bytes.saturating_add(entry.byte_size);
        if !valid_package_path(&entry.path) || !paths.insert(entry.path.clone()) {
            errors.push("package contains an invalid or duplicate path".into());
        }
        if blocked_payload_path(&entry.path) {
            errors.push("package contains a forbidden native binary or script".into());
        }
        if !allowed_paths.contains(entry.path.as_str()) {
            errors.push("package contains an undeclared payload".into());
        }
    }
    if total_bytes > MAX_PACKAGE_BYTES {
        errors.push("package exceeds the v1 size limit".into());
    }
    if !paths.contains("manifest.json") || !paths.contains("component.wasm") {
        errors.push("package must contain manifest.json and component.wasm".into());
    }
    for artifact in &manifest.manifest.artifacts {
        if !paths.contains(&artifact.path) {
            errors.push("package is missing a manifest artifact".into());
        }
    }
    match inspection.signature {
        PackageSignatureStatus::Trusted | PackageSignatureStatus::Development => {}
        PackageSignatureStatus::Missing => errors.push("package signature is missing".into()),
        PackageSignatureStatus::Invalid => errors.push("package signature is invalid".into()),
    }

    if errors.is_empty() {
        Ok(ValidatedPluginPackage {
            manifest,
            signature: inspection.signature,
        })
    } else {
        Err(PackageValidationErrors(errors))
    }
}

fn unique<T: Ord + Clone>(values: &[T]) -> bool {
    values.iter().cloned().collect::<BTreeSet<_>>().len() == values.len()
}

fn valid_plugin_id(value: &str) -> bool {
    value.len() <= MAX_PLUGIN_ID_LENGTH
        && value.split('.').count() >= 3
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment.len() <= 63
                && segment
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_lowercase())
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn valid_opaque_id(value: &str, max_length: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_length
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

fn valid_semver(value: &str) -> bool {
    let mut parts = value.splitn(2, '-');
    let core = parts.next().unwrap_or_default();
    let prerelease = parts.next();
    let core_valid = core.split('.').count() == 3
        && core
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    core_valid
        && prerelease.is_none_or(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        })
}

fn version_at_least(actual: &str, minimum: &str) -> Option<bool> {
    let parse = |value: &str| {
        let core = value.split('-').next()?;
        let mut parts = core.split('.').map(str::parse::<u32>);
        Some((
            parts.next()?.ok()?,
            parts.next()?.ok()?,
            parts.next()?.ok()?,
            parts.next().is_none(),
        ))
    };
    let (actual_major, actual_minor, actual_patch, exact_actual) = parse(actual)?;
    let (minimum_major, minimum_minor, minimum_patch, exact_minimum) = parse(minimum)?;
    (exact_actual && exact_minimum).then_some(
        (actual_major, actual_minor, actual_patch) >= (minimum_major, minimum_minor, minimum_patch),
    )
}

fn normalize_domain(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    let candidate = value.strip_prefix("*.").unwrap_or(&value);
    (!candidate.is_empty()
        && candidate.len() <= 253
        && candidate.split('.').count() >= 2
        && candidate.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric())
                && label
                    .bytes()
                    .last()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric())
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        }))
    .then_some(value)
}

fn valid_package_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 256
        && !path.starts_with('/')
        && !path.contains('\\')
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment.bytes().all(|byte| byte.is_ascii_graphic())
        })
}

fn valid_asset_path(path: &str) -> bool {
    path.starts_with("assets/")
        && path.rsplit('.').next().is_some_and(|extension| {
            matches!(
                extension,
                "json" | "svg" | "png" | "webp" | "jpg" | "jpeg" | "ftl"
            )
        })
}

fn blocked_payload_path(path: &str) -> bool {
    path.rsplit('.').next().is_some_and(|extension| {
        matches!(
            extension,
            "dylib" | "so" | "dll" | "exe" | "app" | "sh" | "js" | "py" | "rb"
        )
    })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner_manifest() -> PluginManifest {
        PluginManifest {
            id: "com.orivo.ryujinx".into(),
            name: "Ryujinx Runner".into(),
            version: "1.0.0".into(),
            sdk: PLUGIN_SDK_V1.into(),
            min_orivo_version: Some("0.3.0".into()),
            extensions: vec![PluginExtension::Runner],
            capabilities: vec![PluginCapability::FilesRead, PluginCapability::RunnerPrepare],
            network_domains: Vec::new(),
            artifacts: vec![ArtifactDescriptor {
                path: "component.wasm".into(),
                kind: ArtifactKind::Component,
                sha256: "a".repeat(64),
                byte_size: 1_024,
            }],
        }
    }

    #[test]
    fn validates_a_minimal_runner_manifest() {
        let manifest = runner_manifest().validate().unwrap();

        assert!(manifest.has_extension(PluginExtension::Runner));
        assert!(manifest.declares(PluginCapability::RunnerPrepare));
        assert_eq!(
            HostCompatibility::v1("0.3.0").compatibility_for(&manifest),
            CompatibleVersionInfo::Compatible
        );
    }

    #[test]
    fn rejects_an_unsafe_manifest_before_install() {
        let mut manifest = runner_manifest();
        manifest.id = "Com.Orivo.Runner".into();
        manifest.capabilities.push(PluginCapability::RunnerPrepare);
        manifest.artifacts.push(ArtifactDescriptor {
            path: "assets/launch.sh".into(),
            kind: ArtifactKind::Asset,
            sha256: "b".repeat(64),
            byte_size: 4,
        });

        let errors = manifest.validate().unwrap_err();
        assert!(errors.0.iter().any(|error| error.contains("reverse-DNS")));
        assert!(errors.0.iter().any(|error| error.contains("unique")));
        assert!(
            errors
                .0
                .iter()
                .any(|error| error.contains("non-executable"))
        );
    }

    #[test]
    fn grants_are_narrow_and_must_be_declared() {
        let manifest = runner_manifest().validate().unwrap();
        let grant = CapabilityGrant {
            plugin_id: manifest.id().into(),
            capability: PluginCapability::RunnerPrepare,
            scope: CapabilityScope::RunnerProfiles(BTreeSet::from(["profile-1".into()])),
        };
        assert!(manifest.validate_grant(&grant).is_ok());

        let overly_broad = CapabilityGrant {
            plugin_id: manifest.id().into(),
            capability: PluginCapability::NetworkFetch,
            scope: CapabilityScope::Domains(BTreeSet::from(["api.example.com".into()])),
        };
        assert!(matches!(
            manifest.validate_grant(&overly_broad),
            Err(GrantValidationError::CapabilityNotDeclared(
                PluginCapability::NetworkFetch
            ))
        ));
    }

    #[test]
    fn package_rejects_native_payloads_and_path_traversal() {
        let inspection = PackageInspection {
            entries: vec![
                PackageEntry {
                    path: "manifest.json".into(),
                    byte_size: 100,
                },
                PackageEntry {
                    path: "component.wasm".into(),
                    byte_size: 1_024,
                },
                PackageEntry {
                    path: "../escape.dylib".into(),
                    byte_size: 4,
                },
            ],
            signature: PackageSignatureStatus::Trusted,
        };

        let errors = validate_plugin_package(runner_manifest(), &inspection).unwrap_err();
        assert!(
            errors
                .0
                .iter()
                .any(|error| error.contains("invalid or duplicate"))
        );
        assert!(
            errors
                .0
                .iter()
                .any(|error| error.contains("forbidden native"))
        );
    }

    #[test]
    fn package_signature_status_is_an_explicit_install_boundary() {
        let entries = vec![
            PackageEntry {
                path: "manifest.json".into(),
                byte_size: 100,
            },
            PackageEntry {
                path: "component.wasm".into(),
                byte_size: 1_024,
            },
        ];
        let development = PackageInspection {
            entries: entries.clone(),
            signature: PackageSignatureStatus::Development,
        };
        assert!(validate_plugin_package(runner_manifest(), &development).is_ok());

        for signature in [
            PackageSignatureStatus::Missing,
            PackageSignatureStatus::Invalid,
        ] {
            let inspection = PackageInspection {
                entries: entries.clone(),
                signature,
            };
            assert!(validate_plugin_package(runner_manifest(), &inspection).is_err());
        }
    }

    #[test]
    fn network_grants_cannot_escape_the_manifest_allowlist() {
        let mut source = runner_manifest();
        source.extensions = vec![PluginExtension::Metadata];
        source.capabilities = vec![PluginCapability::NetworkFetch];
        source.network_domains = vec!["api.igdb.com".into()];
        let source = source.validate().unwrap();

        let allowed = CapabilityGrant {
            plugin_id: source.id().into(),
            capability: PluginCapability::NetworkFetch,
            scope: CapabilityScope::Domains(BTreeSet::from(["api.igdb.com".into()])),
        };
        assert!(source.validate_grant(&allowed).is_ok());

        let denied = CapabilityGrant {
            plugin_id: source.id().into(),
            capability: PluginCapability::NetworkFetch,
            scope: CapabilityScope::Domains(BTreeSet::from(["evil.example.com".into()])),
        };
        assert!(matches!(
            source.validate_grant(&denied),
            Err(GrantValidationError::InvalidScope(
                PluginCapability::NetworkFetch
            ))
        ));
    }

    #[test]
    fn network_domains_are_canonicalised_before_grants_are_checked() {
        let mut source = runner_manifest();
        source.extensions = vec![PluginExtension::Metadata];
        source.capabilities = vec![PluginCapability::NetworkFetch];
        source.network_domains = vec!["API.IGDB.COM".into()];
        let source = source.validate().unwrap();

        let grant = CapabilityGrant {
            plugin_id: source.id().into(),
            capability: PluginCapability::NetworkFetch,
            scope: CapabilityScope::Domains(BTreeSet::from(["api.igdb.com".into()])),
        };
        assert!(source.validate_grant(&grant).is_ok());
    }
}
