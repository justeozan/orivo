//! Validates a manifest the way Orivo itself will: by calling
//! [`orivo_lib::plugin_manifest`] and nothing else. Every rule an author sees
//! here — the reverse-DNS id, the one component artifact, the closed set of
//! extensions and capabilities — lives in that module; this file only reads
//! its output and attaches a location an author can act on.
//!
//! Deliberately out of scope: signature verification and the real archive
//! format (`.orivo-plugin`, tar+gzip). Those are the installer's job
//! (`plugin_installer.rs`, owned elsewhere this wave). This module works
//! against an unpacked directory — `manifest.json`, `component.wasm`,
//! `assets/`, optionally `signature.ed25519` — which is exactly what an
//! author has before they package anything.

use orivo_lib::plugin_manifest::{
    PackageEntry, PackageInspection, PackageSignatureStatus, PluginManifest,
    ValidatedPluginManifest, validate_plugin_package,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// One problem, with enough context that an author does not have to guess
/// which field or file it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedError {
    /// A manifest field (`manifest.json#capabilities`), a package-relative
    /// path, or `manifest.json` itself for a parse failure.
    pub location: String,
    pub message: String,
}

impl std::fmt::Display for LocatedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

/// A manifest that parsed and validated, plus anything worth telling the
/// author even though it did not fail validation.
#[derive(Debug, Clone)]
pub struct ManifestReport {
    pub manifest: ValidatedPluginManifest,
    pub notes: Vec<String>,
}

/// A full package directory that also passed [`validate_plugin_package`].
#[derive(Debug, Clone)]
pub struct PackageReport {
    pub manifest: ValidatedPluginManifest,
    /// Every artifact whose declared `sha256` did not match the file Orivo
    /// would actually read. `validate_plugin_package` does not check content
    /// against the hash — the installer re-hashes on its own path — so this
    /// is arithmetic the SDK does directly with the same bytes, not a policy
    /// rule copied from anywhere.
    pub hash_mismatches: Vec<String>,
}

/// Parses and validates `manifest.json`'s contents. Reused by both entry
/// points below so a bare manifest and a full package directory report
/// exactly the same errors for the same manifest content.
pub fn validate_manifest_json(text: &str) -> Result<ManifestReport, Vec<LocatedError>> {
    let manifest: PluginManifest = serde_json::from_str(text).map_err(|error| {
        vec![LocatedError {
            location: format!("manifest.json:{}:{}", error.line(), error.column()),
            message: error.to_string(),
        }]
    })?;

    let validated = manifest.validate().map_err(|errors| {
        errors
            .0
            .iter()
            .map(|message| LocatedError {
                location: format!("manifest.json#{}", field_for_message(message)),
                message: message.clone(),
            })
            .collect::<Vec<_>>()
    })?;

    Ok(ManifestReport {
        manifest: validated,
        notes: Vec::new(),
    })
}

/// Validates `dir/manifest.json` on its own. This is the check an author runs
/// while iterating on the manifest, before `component.wasm` or a signature
/// exist at all.
pub fn validate_manifest_file(path: &Path) -> Result<ManifestReport, Vec<LocatedError>> {
    let text = fs::read_to_string(path).map_err(|error| {
        vec![LocatedError {
            location: path.display().to_string(),
            message: error.to_string(),
        }]
    })?;
    validate_manifest_json(&text)
}

/// Validates a whole package directory: the manifest, plus every rule
/// `validate_plugin_package` applies to the archive's contents — undeclared
/// payloads, blocked extensions, path traversal, the one-component rule.
///
/// A missing `signature.ed25519` is reported as a note, not an error: the
/// signature is meaningless before Orivo's release key or its development
/// channel assigns one, and an author validating a manifest mid-iteration has
/// neither yet. Once the file exists it is treated as a development-channel
/// signature — the only channel this tool can speak for, since the release
/// key lives with the registry, not with a plugin author.
pub fn validate_package_dir(dir: &Path) -> Result<PackageReport, Vec<LocatedError>> {
    let manifest_path = dir.join("manifest.json");
    let report = validate_manifest_file(&manifest_path)?;

    let mut entries = Vec::new();
    let mut file_bytes: Vec<(String, PathBuf)> = Vec::new();
    collect_entries(dir, dir, &mut entries, &mut file_bytes).map_err(|error| {
        vec![LocatedError {
            location: dir.display().to_string(),
            message: error.to_string(),
        }]
    })?;

    let signature = if dir.join("signature.ed25519").exists() {
        PackageSignatureStatus::Development
    } else {
        PackageSignatureStatus::Missing
    };

    let inspection = PackageInspection { entries, signature };
    let manifest = report.manifest.manifest().clone();
    let validated = validate_plugin_package(manifest, &inspection).map_err(|errors| {
        errors
            .0
            .iter()
            .map(|message| LocatedError {
                location: package_location(message),
                message: message.clone(),
            })
            .collect::<Vec<_>>()
    })?;

    let mut hash_mismatches = Vec::new();
    for artifact in &validated.manifest.manifest().artifacts {
        let Some((_, full_path)) = file_bytes
            .iter()
            .find(|(relative, _)| relative == &artifact.path)
        else {
            continue;
        };
        if let Ok(bytes) = fs::read(full_path) {
            let mut digest = Sha256::new();
            digest.update(&bytes);
            let actual = format!("{:x}", digest.finalize());
            if actual != artifact.sha256 {
                hash_mismatches.push(format!(
                    "{}: declared sha256 {} does not match the file's actual sha256 {actual}",
                    artifact.path, artifact.sha256
                ));
            }
        }
    }

    Ok(PackageReport {
        manifest: validated.manifest,
        hash_mismatches,
    })
}

fn collect_entries(
    root: &Path,
    dir: &Path,
    entries: &mut Vec<PackageEntry>,
    file_bytes: &mut Vec<(String, PathBuf)>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_entries(root, &path, entries, file_bytes)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let byte_size = entry.metadata()?.len();
        entries.push(PackageEntry {
            path: relative.clone(),
            byte_size,
        });
        file_bytes.push((relative, path));
    }
    Ok(())
}

/// `plugin_manifest::PluginManifest::validate` returns plain sentences rather
/// than a field path, so this table gives an author something to jump to. It
/// only labels messages that already exist in that module; it does not decide
/// what is valid, so a wrong label here can make an error less precise but
/// never wrong about whether the manifest passes.
fn field_for_message(message: &str) -> &'static str {
    const TABLE: &[(&str, &str)] = &[
        ("reverse-DNS", "id"),
        ("plugin name must be", "name"),
        ("plugin version must be", "version"),
        ("plugin sdk must be", "sdk"),
        ("minimum Orivo version", "minOrivoVersion"),
        ("at least one extension", "extensions"),
        ("extensions must not contain duplicates", "extensions"),
        ("capabilities must be unique", "capabilities"),
        ("runner_prepare", "capabilities"),
        ("installer plugins must declare", "capabilities"),
        ("network domains exceed", "networkDomains"),
        ("network domains must be unique", "networkDomains"),
        ("require, and require only, network_fetch", "networkDomains"),
        ("bounded non-empty artifact list", "artifacts"),
        ("exactly one component artifact", "artifacts"),
        ("artifact path, hash or size", "artifacts"),
        ("must be component.wasm", "artifacts"),
        ("non-executable declarative files", "artifacts"),
        ("package exceeds the v1 size limit", "artifacts"),
    ];
    TABLE
        .iter()
        .find(|(needle, _)| message.contains(needle))
        .map(|(_, field)| *field)
        .unwrap_or("manifest")
}

/// Same idea as [`field_for_message`], for the package-level errors
/// `validate_plugin_package` adds on top of manifest validation.
fn package_location(message: &str) -> String {
    if message.contains("signature") {
        "signature.ed25519".into()
    } else if message.contains("manifest artifact") {
        "manifest.json#artifacts".into()
    } else if message.contains("path") || message.contains("payload") || message.contains("size")
    {
        "package".into()
    } else {
        "manifest.json".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn minimal_manifest_json(id: &str) -> String {
        format!(
            r#"{{
                "id": "{id}",
                "name": "Test Runner",
                "version": "1.0.0",
                "sdk": "orivo-plugin@1",
                "extensions": ["runner"],
                "capabilities": ["runner_prepare"],
                "artifacts": [
                    {{"path": "component.wasm", "kind": "component", "sha256": "{hash}", "byteSize": 4}}
                ]
            }}"#,
            id = id,
            hash = "a".repeat(64)
        )
    }

    #[test]
    fn accepts_the_same_manifest_the_host_would() {
        let report = validate_manifest_json(&minimal_manifest_json("com.example.runner")).unwrap();
        assert_eq!(report.manifest.id(), "com.example.runner");
    }

    #[test]
    fn rejects_a_manifest_the_host_would_reject_and_names_the_field() {
        let errors =
            validate_manifest_json(&minimal_manifest_json("Not.A.Valid.Id")).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.location.ends_with("#id") && error.message.contains("reverse-DNS"))
        );
    }

    #[test]
    fn reports_a_json_syntax_error_with_a_line_and_column() {
        let errors = validate_manifest_json("{ not json").unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].location.starts_with("manifest.json:1:"));
    }

    /// This is the failing-then-passing test the fix comes with: before
    /// `hash_mismatches` existed, a corrupted `component.wasm` passed
    /// `validate_package_dir` silently because `validate_plugin_package` only
    /// checks declared metadata, never file content. Asserting the mismatch is
    /// reported is what keeps that regression from coming back unnoticed.
    #[test]
    fn flags_a_component_whose_bytes_do_not_match_its_declared_hash() {
        let dir = tempdir("hash-mismatch");
        let mut digest = Sha256::new();
        digest.update(b"the real component bytes");
        let real_hash = format!("{:x}", digest.finalize());

        let manifest = format!(
            r#"{{
                "id": "com.example.runner",
                "name": "Test Runner",
                "version": "1.0.0",
                "sdk": "orivo-plugin@1",
                "extensions": ["runner"],
                "capabilities": ["runner_prepare"],
                "artifacts": [
                    {{"path": "component.wasm", "kind": "component", "sha256": "{real_hash}", "byteSize": 25}}
                ]
            }}"#
        );
        fs::write(dir.join("manifest.json"), manifest).unwrap();
        // Bytes on disk do not match the hash the manifest declares.
        fs::write(dir.join("component.wasm"), b"different bytes on disk!!").unwrap();
        fs::write(dir.join("signature.ed25519"), b"dev").unwrap();

        let report = validate_package_dir(&dir).unwrap();
        assert_eq!(report.hash_mismatches.len(), 1);
        assert!(report.hash_mismatches[0].contains("component.wasm"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_signature_is_a_note_not_a_manifest_error_but_the_package_error_still_fires() {
        let dir = tempdir("no-signature");
        fs::write(dir.join("manifest.json"), minimal_manifest_json("com.example.runner")).unwrap();
        fs::write(dir.join("component.wasm"), b"wasm").unwrap();

        let errors = validate_package_dir(&dir).unwrap_err();
        assert!(errors.iter().any(|error| error.location == "signature.ed25519"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_an_undeclared_payload_reusing_the_same_package_rule_as_the_host() {
        let dir = tempdir("undeclared-payload");
        fs::write(dir.join("manifest.json"), minimal_manifest_json("com.example.runner")).unwrap();
        fs::write(dir.join("component.wasm"), b"wasm").unwrap();
        fs::write(dir.join("signature.ed25519"), b"dev").unwrap();
        let mut launcher = fs::File::create(dir.join("launch.sh")).unwrap();
        launcher.write_all(b"#!/bin/sh\n").unwrap();

        let errors = validate_package_dir(&dir).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("forbidden native"))
        );

        let _ = fs::remove_dir_all(&dir);
    }

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "orivo-plugin-sdk-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
