//! Local install state for Epic Games entitlements.
//!
//! The Epic Games Launcher writes one JSON manifest per installed game into a
//! fixed, per-machine directory. Orivo reads those manifests and nothing else:
//! it never asks Epic's servers what is on this disk, and it never writes into
//! the launcher's own data. A manifest tells us three things — that a game is
//! installed, where, and how large the finished install is — and the launcher
//! flags a download still in flight with `bIsIncompleteInstall`.
//!
//! A download in progress does not live beside the finished ones: the launcher
//! writes it into a `Pending` subdirectory and only moves it up once the
//! transfer completes. Reading only the top directory therefore sees every
//! installed game and no running download at all, so both are scanned.
//!
//! Progress is therefore measured, not reported: bytes currently on disk under
//! the install (and staging) directory against the manifest's `InstallSize`.
//! Epic exposes no download-progress API to a third party, so an honest
//! measurement beats a fabricated one.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// The manifest directory can hold entries for every game ever installed. A cap
/// keeps a hostile or corrupted directory from turning a library refresh into an
/// unbounded read.
const MAX_MANIFESTS: usize = 512;
/// One manifest is a small JSON document. Anything larger is not one.
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// Bounds for measuring an in-flight download. A partial install is thousands of
/// files, not millions, and never nests deeply.
const MAX_MEASURED_ENTRIES: usize = 200_000;
const MAX_MEASURED_DEPTH: usize = 12;
/// Where the launcher parks the manifest of a transfer it has not finished.
const PENDING_DIRECTORY: &str = "Pending";

/// What the launcher recorded about one locally installed game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicInstallation {
    pub app_name: String,
    /// The `.item` file this was read from. Removing an install means removing
    /// this too, or the launcher goes on believing the game is there.
    pub manifest_path: PathBuf,
    pub install_location: PathBuf,
    pub staging_location: Option<PathBuf>,
    /// Size of the finished install, in bytes, as the launcher computed it.
    pub install_size: u64,
    /// Set while a download or repair is still running.
    pub incomplete: bool,
}

/// The install state of one Epic game, as the UI needs to render it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpicInstallStatus {
    pub app_name: String,
    pub state: EpicInstallState,
    /// 0–100. Always 100 once installed, and 0 when nothing is on disk yet.
    pub percent: u8,
    pub installed_bytes: u64,
    pub total_bytes: u64,
    /// Shown to the user, so it is the install directory and never a private
    /// launcher path.
    pub install_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EpicInstallState {
    NotInstalled,
    Installing,
    Installed,
}

impl EpicInstallStatus {
    fn not_installed(app_name: &str) -> Self {
        Self {
            app_name: app_name.to_string(),
            state: EpicInstallState::NotInstalled,
            percent: 0,
            installed_bytes: 0,
            total_bytes: 0,
            install_path: None,
        }
    }
}

impl From<EpicInstallStatus> for crate::sources::SourceInstallStatus {
    fn from(status: EpicInstallStatus) -> Self {
        Self {
            installed: status.state == EpicInstallState::Installed,
            installing: status.state == EpicInstallState::Installing,
            percent: status.percent,
            install_path: status.install_path,
        }
    }
}

/// Every game the local Epic Games Launcher believes is installed or is busy
/// downloading, keyed by the `AppName` Orivo already stores as an Epic game's
/// `source_id`.
pub fn installations() -> BTreeMap<String, EpicInstallation> {
    let Some(directory) = manifest_directory() else {
        return BTreeMap::new();
    };

    // Finished installs first, then the pending ones on top: a game being
    // updated has a manifest in both places, and the running transfer is the
    // more useful thing to report.
    let mut installations = read_manifest_directory(&directory);
    for (app_name, pending) in read_manifest_directory(&directory.join(PENDING_DIRECTORY)) {
        installations.insert(app_name, pending);
    }
    installations
}

fn read_manifest_directory(directory: &Path) -> BTreeMap<String, EpicInstallation> {
    let Ok(entries) = fs::read_dir(directory) else {
        return BTreeMap::new();
    };

    let mut installations = BTreeMap::new();
    for entry in entries.flatten().take(MAX_MANIFESTS) {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("item") {
            continue;
        }
        // A single unreadable or malformed manifest is one missing game, never
        // a failed scan.
        let Some(installation) = read_manifest(&path) else {
            continue;
        };
        installations.insert(installation.app_name.clone(), installation);
    }
    installations
}

/// The install state of one entitlement. A game with no manifest is simply not
/// installed, which is the common case for an owned-but-never-downloaded title.
pub fn status(app_name: &str) -> EpicInstallStatus {
    match installations().get(app_name) {
        Some(installation) => status_for(installation),
        None => EpicInstallStatus::not_installed(app_name),
    }
}

pub fn status_for(installation: &EpicInstallation) -> EpicInstallStatus {
    let install_path = installation.install_location.to_str().map(str::to_owned);
    if !installation.incomplete {
        return EpicInstallStatus {
            app_name: installation.app_name.clone(),
            state: EpicInstallState::Installed,
            percent: 100,
            installed_bytes: installation.install_size,
            total_bytes: installation.install_size,
            install_path,
        };
    }

    // A download in flight: measure what has landed. The staging directory
    // usually sits inside the install location, so identical paths are counted
    // once rather than twice.
    let mut installed_bytes = directory_size(&installation.install_location);
    if let Some(staging) = installation.staging_location.as_deref()
        && !staging.starts_with(&installation.install_location)
    {
        installed_bytes = installed_bytes.saturating_add(directory_size(staging));
    }

    EpicInstallStatus {
        app_name: installation.app_name.clone(),
        state: EpicInstallState::Installing,
        percent: percent_of(installed_bytes, installation.install_size),
        installed_bytes,
        total_bytes: installation.install_size,
        install_path,
    }
}

/// Why an uninstall was refused. Each variant is a guard that fired, and the
/// message names it: a silent no-op is what the previous attempt did, and it is
/// worse than an error.
#[derive(Debug, PartialEq, Eq)]
pub enum UninstallError {
    NotInstalled,
    /// The manifest points somewhere Orivo will not delete. See `is_removable`.
    UnsafeLocation,
    Failed(String),
}

impl std::fmt::Display for UninstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled => write!(
                f,
                "The Epic Games Launcher has no record of this game being installed."
            ),
            Self::UnsafeLocation => write!(
                f,
                "This game's recorded install folder is not one Orivo will delete. Uninstall it from the Epic Games Launcher."
            ),
            Self::Failed(reason) => write!(f, "The files could not be removed: {reason}"),
        }
    }
}

/// Remove a game the Epic launcher installed.
///
/// Epic publishes no uninstall URI — the launcher simply ignores one — so the
/// removal has to be done here, the same way every other third-party Epic
/// client does it: delete the directory the launcher recorded, then delete the
/// manifest so the launcher stops believing the game is there.
///
/// This is the only place Orivo deletes a game's files, so the path is never
/// taken from the catalog or from anything a provider sent. It comes from the
/// launcher's own manifest, is matched against the requested game, and has to
/// clear `is_removable` before a single byte is touched.
pub fn uninstall(app_name: &str) -> Result<(), UninstallError> {
    let installation = installations()
        .remove(app_name)
        .ok_or(UninstallError::NotInstalled)?;
    // Paranoia, not politeness: the map is keyed by this, but the delete below
    // is unforgiving enough to be worth confirming twice.
    if installation.app_name != app_name {
        return Err(UninstallError::NotInstalled);
    }
    if !is_removable(&installation.install_location) {
        return Err(UninstallError::UnsafeLocation);
    }

    fs::remove_dir_all(&installation.install_location)
        .map_err(|error| UninstallError::Failed(error.to_string()))?;
    // A manifest left behind would keep the game showing as installed with no
    // files under it, which is a worse state than either end of the operation.
    if installation.manifest_path.is_file() {
        let _ = fs::remove_file(&installation.manifest_path);
    }
    Ok(())
}

/// Whether a recorded install directory may be deleted recursively.
///
/// A game lives in its own folder several levels down. Anything shallow, any
/// symlink, and any of the well-known directories below is refused outright: a
/// corrupted or hand-edited manifest must not be able to turn "uninstall one
/// game" into "erase a home directory".
fn is_removable(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    // `symlink_metadata` does not follow the link, so a manifest pointing at a
    // symlink is refused rather than deleting whatever it aims at.
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return false;
    }
    // `/Users/Shared/Epic Games/AstroDuel2` is four; a game folder is never
    // shallower than three.
    if path.components().count() < 4 {
        return false;
    }
    let protected = [
        std::env::var_os("HOME").map(PathBuf::from),
        Some(PathBuf::from("/")),
        Some(PathBuf::from("/Applications")),
        Some(PathBuf::from("/Users")),
        Some(PathBuf::from("/Users/Shared")),
        Some(PathBuf::from("/System")),
        Some(PathBuf::from("/Library")),
    ];
    !protected
        .into_iter()
        .flatten()
        .any(|guarded| path == guarded)
}

/// Never report a download as finished from a measurement: the launcher's own
/// `bIsIncompleteInstall` flag decides that, so an in-flight install is capped
/// at 99% however the byte counts land.
fn percent_of(installed_bytes: u64, total_bytes: u64) -> u8 {
    if total_bytes == 0 {
        return 0;
    }
    let ratio = (installed_bytes.min(total_bytes) as f64 / total_bytes as f64) * 100.0;
    (ratio.round() as u64).min(99) as u8
}

/// Where the launcher keeps its manifests. macOS stores them per user; Windows
/// stores them once per machine under `ProgramData`.
fn manifest_directory() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library/Application Support/Epic/EpicGamesLauncher/Data/Manifests"),
        )
    }
    #[cfg(target_os = "windows")]
    {
        let program_data = std::env::var_os("ProgramData")?;
        Some(PathBuf::from(program_data).join("Epic/EpicGamesLauncher/Data/Manifests"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default, rename = "AppName")]
    app_name: String,
    #[serde(default, rename = "InstallLocation")]
    install_location: String,
    #[serde(default, rename = "StagingLocation")]
    staging_location: String,
    #[serde(default, rename = "InstallSize")]
    install_size: u64,
    #[serde(default, rename = "bIsIncompleteInstall")]
    incomplete: bool,
}

fn read_manifest(path: &Path) -> Option<EpicInstallation> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let mut installation = parse_manifest(&fs::read_to_string(path).ok()?)?;
    installation.manifest_path = path.to_path_buf();
    Some(installation)
}

fn parse_manifest(contents: &str) -> Option<EpicInstallation> {
    let manifest = serde_json::from_str::<Manifest>(contents).ok()?;
    // The app name is the key Orivo joins on, so a manifest without one — or
    // with one no catalog reference could hold — is unusable rather than
    // half-usable.
    if manifest.install_location.trim().is_empty()
        || !crate::catalog::is_valid_provider_reference(&manifest.app_name)
    {
        return None;
    }
    let install_location = PathBuf::from(manifest.install_location);
    // A manifest can outlive the directory it describes (an external drive that
    // is not mounted, a folder deleted outside the launcher). That is not an
    // installed game — but a download that has not written its first byte yet
    // has no directory either, and reporting it as absent would make Orivo go
    // quiet exactly when the user is watching for progress.
    if !manifest.incomplete && !install_location.is_dir() {
        return None;
    }
    let staging_location = (!manifest.staging_location.trim().is_empty())
        .then(|| PathBuf::from(manifest.staging_location));

    Some(EpicInstallation {
        app_name: manifest.app_name,
        manifest_path: PathBuf::new(),
        install_location,
        staging_location,
        install_size: manifest.install_size,
        incomplete: manifest.incomplete,
    })
}

/// Bytes currently on disk under `root`. Bounded in both breadth and depth, and
/// deliberately blind to symlinks: a link inside a download directory must not
/// be able to send this walk somewhere else on the machine.
fn directory_size(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut visited = 0usize;
    let mut stack = vec![(root.to_path_buf(), 0usize)];

    while let Some((directory, depth)) = stack.pop() {
        if depth > MAX_MEASURED_DEPTH || visited >= MAX_MEASURED_ENTRIES {
            break;
        }
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited >= MAX_MEASURED_ENTRIES {
                break;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push((entry.path(), depth + 1));
            } else if let Ok(metadata) = entry.metadata() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_without_a_real_directory_is_not_an_installed_game() {
        let manifest = r#"{
            "AppName": "Sugar",
            "InstallLocation": "/definitely/not/an/epic/install",
            "InstallSize": 1024,
            "bIsIncompleteInstall": false
        }"#;

        assert!(parse_manifest(manifest).is_none());
    }

    #[test]
    fn a_manifest_reads_the_directory_it_actually_describes() {
        let directory = std::env::temp_dir().join("orivo-epic-manifest-test");
        fs::create_dir_all(&directory).unwrap();
        let manifest = format!(
            r#"{{
                "AppName": "Sugar",
                "InstallLocation": {},
                "StagingLocation": "",
                "InstallSize": 2048,
                "bIsIncompleteInstall": true
            }}"#,
            serde_json::to_string(directory.to_str().unwrap()).unwrap()
        );

        let installation = parse_manifest(&manifest).unwrap();
        assert_eq!(installation.app_name, "Sugar");
        assert_eq!(installation.install_location, directory);
        assert_eq!(installation.install_size, 2048);
        assert!(installation.incomplete);
        assert!(installation.staging_location.is_none());

        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn an_app_name_that_could_not_be_a_catalog_reference_is_refused() {
        let manifest = r#"{
            "AppName": "../../etc/passwd",
            "InstallLocation": "/tmp",
            "InstallSize": 1
        }"#;

        assert!(parse_manifest(manifest).is_none());
    }

    /// The bug this exists to prevent: the launcher parks a running download in
    /// `Pending`, so scanning only the top directory sees every finished game
    /// and no transfer at all — Orivo goes silent exactly when the user is
    /// watching a progress bar.
    #[test]
    fn a_download_in_flight_is_found_in_the_pending_directory_and_wins() {
        let root = std::env::temp_dir().join("orivo-epic-pending-test");
        let pending = root.join(PENDING_DIRECTORY);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&pending).unwrap();
        let install = root.join("game");
        fs::create_dir_all(&install).unwrap();
        let manifest = |incomplete: bool| {
            format!(
                r#"{{"AppName":"Sugar","InstallLocation":{},"InstallSize":1000,"bIsIncompleteInstall":{incomplete}}}"#,
                serde_json::to_string(install.to_str().unwrap()).unwrap()
            )
        };
        fs::write(root.join("done.item"), manifest(false)).unwrap();
        fs::write(pending.join("busy.item"), manifest(true)).unwrap();

        let finished = read_manifest_directory(&root);
        let running = read_manifest_directory(&pending);
        assert!(!finished["Sugar"].incomplete);
        assert!(running["Sugar"].incomplete);

        let _ = fs::remove_dir_all(&root);
    }

    /// A transfer that has not written its first byte has no directory yet.
    /// Treating that as "no such game" would blank the progress row at the one
    /// moment the user is most certainly looking at it.
    #[test]
    fn a_download_that_has_written_nothing_yet_is_still_a_download() {
        let missing = std::env::temp_dir().join("orivo-epic-not-created-yet");
        let _ = fs::remove_dir_all(&missing);
        let manifest = |incomplete: bool| {
            format!(
                r#"{{"AppName":"Sugar","InstallLocation":{},"InstallSize":1000,"bIsIncompleteInstall":{incomplete}}}"#,
                serde_json::to_string(missing.to_str().unwrap()).unwrap()
            )
        };

        let running = parse_manifest(&manifest(true)).expect("a pending transfer survives");
        assert!(running.incomplete);
        assert_eq!(status_for(&running).percent, 0);
        // A *finished* install whose directory is gone is still not installed.
        assert!(parse_manifest(&manifest(false)).is_none());
    }

    /// The guard that matters most in this file. A hand-edited or corrupted
    /// manifest must never be able to turn "uninstall one game" into deleting
    /// a home directory, a volume root, or whatever a symlink happens to aim at.
    #[test]
    fn the_delete_guard_refuses_everything_that_is_not_a_game_folder() {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();

        assert!(!is_removable(Path::new("/")));
        assert!(!is_removable(Path::new("/Users")));
        assert!(!is_removable(Path::new("/Users/Shared")));
        assert!(!is_removable(Path::new("/Applications")));
        assert!(!is_removable(Path::new("/System")));
        assert!(!is_removable(Path::new("/Library")));
        assert!(!is_removable(&home));
        // Relative, traversing, and non-existent paths never reach the disk.
        assert!(!is_removable(Path::new("Epic Games/Game")));
        assert!(!is_removable(Path::new("/Users/Shared/../../etc")));
        assert!(!is_removable(Path::new("/definitely/not/here/at/all")));
        // A file is not a game folder either.
        assert!(!is_removable(Path::new("/etc/hosts")));
    }

    #[test]
    fn the_delete_guard_accepts_a_real_game_folder_and_refuses_a_symlink_to_one() {
        let root = std::env::temp_dir().join("orivo-epic-remove-guard/Epic Games");
        let game = root.join("SomeGame");
        let link = root.join("LinkedGame");
        let _ = fs::remove_dir_all(root.parent().unwrap());
        fs::create_dir_all(&game).unwrap();
        assert!(is_removable(&game));

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&game, &link).unwrap();
            // Following it would delete the target through a name the manifest
            // does not actually own.
            assert!(!is_removable(&link));
        }

        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn uninstalling_a_game_the_launcher_never_recorded_is_an_error_not_a_no_op() {
        // The previous attempt handed Epic a URI it ignores and reported
        // success. Silence is the one outcome this must never have.
        assert_eq!(
            uninstall("definitely-not-an-installed-app-name"),
            Err(UninstallError::NotInstalled)
        );
    }

    #[test]
    fn a_finished_install_reports_one_hundred_percent_without_touching_the_disk() {
        let installation = EpicInstallation {
            app_name: "Sugar".into(),
            manifest_path: PathBuf::new(),
            install_location: PathBuf::from("/definitely/not/an/epic/install"),
            staging_location: None,
            install_size: 4096,
            incomplete: false,
        };

        let status = status_for(&installation);
        assert_eq!(status.state, EpicInstallState::Installed);
        assert_eq!(status.percent, 100);
        assert_eq!(status.installed_bytes, 4096);
    }

    #[test]
    fn an_install_still_running_never_reports_itself_as_finished() {
        assert_eq!(percent_of(0, 100), 0);
        assert_eq!(percent_of(37, 100), 37);
        assert_eq!(percent_of(100, 100), 99);
        assert_eq!(percent_of(500, 100), 99);
        // A manifest that never recorded a size cannot produce a percentage.
        assert_eq!(percent_of(500, 0), 0);
    }

    #[test]
    fn a_game_with_no_manifest_at_all_is_simply_not_installed() {
        let status = EpicInstallStatus::not_installed("Sugar");
        assert_eq!(status.state, EpicInstallState::NotInstalled);
        assert_eq!(status.percent, 0);
        assert!(status.install_path.is_none());
    }
}
