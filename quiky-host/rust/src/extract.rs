//! Extraction and setup helpers, platform-specific.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractKind {
    Zip,
    SevenZ,
    Unknown,
}

pub fn detect_kind(path: &str) -> ExtractKind {
    let lower = path.to_lowercase();
    if lower.ends_with(".zip") {
        ExtractKind::Zip
    } else if lower.ends_with(".7z") || lower.ends_with(".exe") {
        ExtractKind::SevenZ
    } else {
        ExtractKind::Unknown
    }
}

/// Find `setup*.exe` (case-insensitive) in a directory.
pub fn find_setup_exe(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut found: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| {
                        let lower = n.to_lowercase();
                        lower.starts_with("setup") && lower.ends_with(".exe")
                    })
                    .unwrap_or(false)
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Locate a 7-Zip executable.
pub fn find_7z() -> Option<PathBuf> {
    for name in ["7z", "7zz", "7z.exe"] {
        if let Some(path) = which(name) {
            return Some(path);
        }
    }
    None
}

/// Locate a host tool even when Orivo was started from Finder.
///
/// A macOS app bundle inherits a very small `PATH` (`/usr/bin:/bin:...`), so
/// Homebrew's `innoextract` or `aria2c` is invisible unless the common prefix
/// directories are searched explicitly. An explicit `QUIKY_<NAME>` environment
/// variable wins over both.
pub(crate) fn which(name: &str) -> Option<PathBuf> {
    let override_key = format!("QUIKY_{}", name.to_ascii_uppercase().replace(['-', '.'], "_"));
    if let Some(path) = std::env::var_os(override_key).map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    let mut directories = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    for directory in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        let directory = PathBuf::from(directory);
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    directories
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The Inno Setup extractor used to unpack a FitGirl repack's own tools.
pub fn find_innoextract() -> Option<PathBuf> {
    which("innoextract")
}

/// Extract an archive with 7-Zip, falling back to `unzip` for zip files.
pub fn extract_archive(archive: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    if let Some(seven) = find_7z() {
        let status = Command::new(seven)
            .arg("x")
            .arg(format!("-o{}", dest.display()))
            .arg("-y")
            .arg(archive)
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() {
            return Ok(());
        }
        return Err(format!("7z exited with {status}"));
    }
    if detect_kind(&archive.to_string_lossy()) == ExtractKind::Zip {
        let status = Command::new("unzip")
            .arg("-o")
            .arg(archive)
            .arg("-d")
            .arg(dest)
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() {
            return Ok(());
        }
        return Err(format!("unzip exited with {status}"));
    }
    Err("no extraction tool found (install 7-Zip)".into())
}

/// Timeout wrapper runner used by the Wine/Windows setup steps.
fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<bool, String> {
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("setup timed out".into());
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Inno Setup silent flags, also accepted by many FitGirl setups.
pub fn silent_args(install_dir: &Path) -> Vec<String> {
    vec![
        "/VERYSILENT".into(),
        "/SUPPRESSMSGBOXES".into(),
        "/NORESTART".into(),
        "/NOCANCEL".into(),
        format!("/DIR={}", install_dir.display()),
    ]
}

#[cfg(target_os = "windows")]
pub fn run_setup(setup: &Path, install_dir: &Path) -> Result<bool, String> {
    let mut command = Command::new(setup);
    command.args(silent_args(install_dir));
    run_with_timeout(command, Duration::from_secs(1800))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn wine_binary() -> PathBuf {
    if let Ok(explicit) = std::env::var("QUIKY_WINE") {
        return PathBuf::from(explicit);
    }
    for candidate in [
        "/Applications/Wine Staging.app/Contents/Resources/wine/bin/wine",
        "/Applications/Wine Stable.app/Contents/Resources/wine/bin/wine",
        "/usr/local/bin/wine",
        "/opt/homebrew/bin/wine",
    ] {
        if Path::new(candidate).is_file() {
            return PathBuf::from(candidate);
        }
    }
    PathBuf::from("wine")
}

/// Silent install under Wine. This is the macOS/Linux fallback when Win32 UI
/// automation is unavailable.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn run_setup(setup: &Path, install_dir: &Path) -> Result<bool, String> {
    let mut command = Command::new(wine_binary());
    command.arg(setup).args(silent_args(install_dir));
    run_with_timeout(command, Duration::from_secs(1800))
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn run_setup(_setup: &Path, _install_dir: &Path) -> Result<bool, String> {
    Err("unsupported platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_zip() {
        assert_eq!(detect_kind("a.zip"), ExtractKind::Zip);
    }

    #[test]
    fn detect_7z() {
        assert_eq!(detect_kind("a.7z"), ExtractKind::SevenZ);
        assert_eq!(detect_kind("setup.exe"), ExtractKind::SevenZ);
    }

    #[test]
    fn find_setup_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Setup.EXE"), b"x").unwrap();
        std::fs::write(dir.path().join("readme.txt"), b"x").unwrap();
        let found = find_setup_exe(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "Setup.EXE");
    }

    #[test]
    fn silent_args_include_dir() {
        let args = silent_args(Path::new("/tmp/game"));
        assert!(args.contains(&"/VERYSILENT".to_string()));
        assert!(args.contains(&"/DIR=/tmp/game".to_string()));
    }
}
