//! Epic games Orivo installs on a phone itself, for Winlator to run.
//!
//! A phone has no Epic Games Launcher, so the desktop hand-off cannot happen
//! there. Instead Orivo downloads the game's Windows build (`epic_download.rs`)
//! into shared storage, where Winlator Cmod can read it, writes the `.desktop`
//! shortcut Cmod starts a game from, and keeps one small record per installed
//! game. The Epic card stays one card: on a phone its Install button downloads,
//! its progress is this module's, and its Play button hands the shortcut to
//! Winlator.
//!
//! Where things go:
//!
//! - the game: `Download/Orivo/Epic/<Folder>` — Winlator Cmod maps a
//!   container's `D:` drive to the shared Download folder by default, so the
//!   game is `D:\Orivo\Epic\<Folder>` inside Windows;
//! - the shortcut: `Download/Orivo/Shortcuts/<Folder>.desktop`, a file only
//!   Orivo writes, read by Cmod when Orivo starts it;
//! - the record: `epic-installs.json` in Orivo's own data folder.
//!
//! No sign-in material ever goes into the shortcut. Epic can hand a launcher a
//! one-time login code for the game's command line, but a file in shared
//! storage is readable by every app with storage access, so Orivo starts games
//! the way they run offline.

use crate::epic_download::{self, EpicDownloadError, Progress};
use crate::epic_install::{EpicInstallState, EpicInstallStatus};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

/// The shared Download folder of the device's primary user, which is what
/// Winlator Cmod maps to `D:` (`Container.DEFAULT_DRIVES`).
pub const SHARED_DOWNLOADS: &str = "/storage/emulated/0/Download";
const INSTALL_DIRECTORY: &str = "Orivo/Epic";
const SHORTCUT_DIRECTORY: &str = "Orivo/Shortcuts";
const WINDOWS_INSTALL_ROOT: &str = "D:\\Orivo\\Epic";
const RECORDS_FILE: &str = "epic-installs.json";
/// Sent to the main window whenever a download starts, ends or fails, so the
/// player hears about a failure instead of watching a bar disappear.
pub const EPIC_INSTALL_EVENT: &str = "epic-install-status";
const MAIN_WINDOW_LABEL: &str = "main";
const MAX_FOLDER_CHARS: usize = 48;
const MAX_SHORTCUT_NAMES: u32 = 9;
/// What Epic's launcher puts on every game's command line. A game built on Epic
/// Online Services looks for it and, when it is missing, quits to relaunch
/// itself through a launcher this phone does not have (Astro Duel 2 does).
const EPIC_PORTAL_ARGUMENT: &str = "-EpicPortal";
/// The graphics driver a shortcut asks for on a phone with an Adreno GPU: the
/// Turnip build Winlator Cmod 13 bundles. Cmod's default, Zink over Qualcomm's
/// own Vulkan driver, leaves the screen black; with Turnip, Cave Story+ and
/// Astro Duel 2 both draw at 60 fps on an Adreno 730. A shortcut's value
/// replaces the container's whole string rather than merging into it, so every
/// key is spelled out — Cmod cannot start without `version`.
const TURNIP_DRIVER_CONFIG: &str =
    "version=turnip25.1.0;blacklistedExtensions=;maxDeviceMemory=0;adrenotoolsTurnip=1;frameSync=Normal";

/// What Orivo remembers about one game it installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRecord {
    pub app_name: String,
    pub title: String,
    pub build_version: String,
    /// The game's folder under `Download/Orivo/Epic`. A name Orivo derived, so
    /// it never carries a separator.
    pub folder: String,
    /// The executable, relative to the folder, with `/` separators.
    pub launch_exe: String,
    /// The arguments Epic's launcher always passes.
    #[serde(default)]
    pub launch_command: String,
    pub install_bytes: u64,
    pub installed_at_ms: u64,
}

struct Job {
    progress: Arc<Progress>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct Service {
    data_dir: Option<PathBuf>,
    downloads_root: Option<PathBuf>,
    jobs: HashMap<String, Job>,
}

fn service() -> &'static Mutex<Service> {
    static SERVICE: OnceLock<Mutex<Service>> = OnceLock::new();
    SERVICE.get_or_init(|| Mutex::new(Service::default()))
}

/// Where the records live, and — for tests — where "shared storage" is.
pub fn configure(data_dir: PathBuf, downloads_root: Option<PathBuf>) {
    if let Ok(mut service) = service().lock() {
        service.data_dir = Some(data_dir);
        service.downloads_root = downloads_root;
    }
}

fn downloads_root() -> PathBuf {
    service()
        .lock()
        .ok()
        .and_then(|service| service.downloads_root.clone())
        .unwrap_or_else(|| PathBuf::from(SHARED_DOWNLOADS))
}

fn records_path() -> Option<PathBuf> {
    service()
        .lock()
        .ok()
        .and_then(|service| service.data_dir.as_ref().map(|dir| dir.join(RECORDS_FILE)))
}

pub fn records() -> Vec<InstallRecord> {
    records_path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Vec<InstallRecord>>(&bytes).ok())
        .unwrap_or_default()
}

fn save_records(records: &[InstallRecord]) -> Result<(), String> {
    let path = records_path().ok_or("Orivo's data folder is not ready yet.")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(records).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

pub fn record(app_name: &str) -> Option<InstallRecord> {
    records().into_iter().find(|record| record.app_name == app_name)
}

pub fn install_root(folder: &str) -> PathBuf {
    downloads_root().join(INSTALL_DIRECTORY).join(folder)
}

pub fn shortcut_path(folder: &str) -> PathBuf {
    downloads_root()
        .join(SHORTCUT_DIRECTORY)
        .join(format!("{folder}.desktop"))
}

/// Where a game's shortcut may live: its own name first, then numbered ones.
/// A file in shared storage stays the app's that created it, so once Orivo is
/// reinstalled — or anything else rewrote the file — Orivo may still read the
/// old shortcut but no longer replace it, and writes the next name instead.
fn shortcut_candidates(folder: &str) -> impl Iterator<Item = PathBuf> + '_ {
    std::iter::once(shortcut_path(folder))
        .chain((2..=MAX_SHORTCUT_NAMES).map(move |number| shortcut_path(&format!("{folder}-{number}"))))
}

/// Writes `text` to the first of `candidates` this app may write, and says which.
fn write_shortcut(candidates: impl IntoIterator<Item = PathBuf>, text: &str) -> Result<PathBuf, String> {
    let failed = |error: std::io::Error| format!("The shortcut could not be written: {error}");
    let mut refused = None;
    for path in candidates {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(failed)?;
        }
        match fs::write(&path, text) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => refused = Some(error),
            Err(error) => return Err(failed(error)),
        }
    }
    Err(refused.map(failed).unwrap_or_else(|| "The shortcut could not be written.".into()))
}

/// A folder name Windows, Android's shared storage and a `.desktop` line can
/// all carry unchanged: letters and digits of the title, nothing else. A title
/// with none of those falls back to Epic's own app name.
pub fn folder_name(title: &str, app_name: &str) -> String {
    let from = |value: &str| {
        value
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(|word| {
                let mut characters = word.chars();
                characters
                    .next()
                    .map(|first| first.to_ascii_uppercase().to_string() + characters.as_str())
                    .unwrap_or_default()
            })
            .collect::<String>()
            .chars()
            .take(MAX_FOLDER_CHARS)
            .collect::<String>()
    };
    let folder = from(title);
    if !folder.is_empty() {
        return folder;
    }
    let folder = from(app_name);
    if folder.is_empty() { "EpicGame".into() } else { folder }
}

/// `D:\Orivo\Epic\<Folder>\<exe>`, as Windows inside the container sees it.
pub fn windows_executable(folder: &str, launch_exe: &str) -> String {
    let relative = launch_exe.replace('/', "\\");
    format!("{WINDOWS_INSTALL_ROOT}\\{folder}\\{}", relative.trim_start_matches('\\'))
}

/// The `Exec=` value Cmod reads. Cmod takes everything after the last `wine `
/// and unescapes it twice the way desktop entries are written, so a backslash
/// is written as four and a space as an escaped one.
pub fn exec_line(windows_path: &str) -> String {
    let escaped = windows_path.replace('\\', "\\\\\\\\").replace(' ', "\\\\ ");
    format!("env WINEPREFIX=\"/home/xuser/.wine\" wine {escaped}")
}

/// The whole shortcut. `container_id` in `[Extra Data]` is what Cmod falls back
/// to when the intent does not name one; the arguments Epic's launcher would
/// pass ride in `execArgs`, which Cmod appends to the command line; `turnip`
/// swaps the container's graphics driver for Turnip, for this game only.
pub fn shortcut_text(record: &InstallRecord, container_id: u32, turnip: bool) -> String {
    let windows_path = windows_executable(&record.folder, &record.launch_exe);
    let exe_name = record
        .launch_exe
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut text = format!(
        "[Desktop Entry]\nName={}\nExec={}\nType=Application\nStartupNotify=true\nPath={}\nStartupWMClass={}\n\n[Extra Data]\ncontainer_id={}\n",
        record.title.replace(['\n', '\r'], " "),
        exec_line(&windows_path),
        windows_path
            .rsplit_once('\\')
            .map(|(directory, _)| directory)
            .unwrap_or(WINDOWS_INSTALL_ROOT),
        exe_name,
        container_id
    );
    if turnip {
        text.push_str(&format!("graphicsDriverConfig={TURNIP_DRIVER_CONFIG}\n"));
    }
    let mut arguments = record.launch_command.replace(['\n', '\r'], " ").trim().to_string();
    if !arguments
        .split_whitespace()
        .any(|argument| argument.eq_ignore_ascii_case(EPIC_PORTAL_ARGUMENT))
    {
        arguments = format!("{arguments} {EPIC_PORTAL_ARGUMENT}").trim_start().to_string();
    }
    text.push_str(&format!("execArgs={arguments}\n"));
    text
}

/// Whether this phone draws with an Adreno GPU, as the system properties its
/// Vulkan and EGL loaders read in every app process say.
fn adreno_gpu() -> bool {
    #[cfg(target_os = "android")]
    {
        ["ro.hardware.vulkan", "ro.hardware.egl"]
            .iter()
            .any(|name| system_property(name).eq_ignore_ascii_case("adreno"))
    }
    #[cfg(not(target_os = "android"))]
    {
        false
    }
}

#[cfg(target_os = "android")]
fn system_property(name: &str) -> String {
    // PROP_VALUE_MAX: the longest value a system property can hold, NUL included.
    const PROP_VALUE_MAX: usize = 92;
    let Ok(name) = std::ffi::CString::new(name) else {
        return String::new();
    };
    let mut value = [0 as libc::c_char; PROP_VALUE_MAX];
    // SAFETY: `name` is NUL-terminated and `value` holds PROP_VALUE_MAX bytes,
    // the most bionic writes.
    let length = unsafe { libc::__system_property_get(name.as_ptr(), value.as_mut_ptr()) };
    let length = usize::try_from(length).unwrap_or(0).min(PROP_VALUE_MAX);
    let bytes: Vec<u8> = value[..length].iter().map(|&byte| byte as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Every game this phone has, installed or downloading, keyed by Epic app name.
pub fn statuses() -> BTreeMap<String, EpicInstallStatus> {
    let mut statuses = BTreeMap::new();
    for record in records() {
        let path = install_root(&record.folder).to_string_lossy().into_owned();
        statuses.insert(
            record.app_name.clone(),
            EpicInstallStatus {
                app_name: record.app_name.clone(),
                state: EpicInstallState::Installed,
                percent: 100,
                installed_bytes: record.install_bytes,
                total_bytes: record.install_bytes,
                install_path: Some(path),
            },
        );
    }
    if let Ok(service) = service().lock() {
        for (app_name, job) in &service.jobs {
            let progress = &job.progress;
            statuses.insert(
                app_name.clone(),
                EpicInstallStatus {
                    app_name: app_name.clone(),
                    state: EpicInstallState::Installing,
                    percent: progress.percent(),
                    installed_bytes: progress.written_bytes.load(Ordering::Relaxed),
                    total_bytes: progress.install_total_bytes.load(Ordering::Relaxed),
                    install_path: None,
                },
            );
        }
    }
    statuses
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallEvent {
    app_name: String,
    title: String,
    /// `installing`, `installed`, `failed` or `cancelled`.
    state: &'static str,
    message: String,
}

fn publish(app: &tauri::AppHandle, app_name: &str, title: &str, state: &'static str, message: String) {
    use tauri::Emitter;
    let _ = app.emit_to(
        MAIN_WINDOW_LABEL,
        EPIC_INSTALL_EVENT,
        InstallEvent {
            app_name: app_name.to_string(),
            title: title.to_string(),
            state,
            message,
        },
    );
}

/// An Epic launch reference is `namespace:catalogItem:appName`.
pub fn parse_app_ref(app_ref: &str) -> Option<(String, String, String)> {
    let mut parts = app_ref.splitn(3, ':');
    let namespace = parts.next()?.to_string();
    let catalog_item = parts.next()?.to_string();
    let app_name = parts.next()?.to_string();
    (!namespace.is_empty() && !catalog_item.is_empty() && !app_name.is_empty())
        .then_some((namespace, catalog_item, app_name))
}

/// Start downloading one owned game in the background. Returns at once; the
/// card follows the download through `statuses`, and the player hears how it
/// ended through `EPIC_INSTALL_EVENT`.
pub fn start_install(app: tauri::AppHandle, title: String, app_ref: &str) -> Result<String, String> {
    let (namespace, catalog_item, app_name) =
        parse_app_ref(app_ref).ok_or("This Epic game has no download reference.")?;
    if record(&app_name).is_some() {
        return Ok(format!("{title} is already installed"));
    }
    let progress = Arc::new(Progress::default());
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut service = service().lock().map_err(|_| "Orivo is busy. Try again.")?;
        if service.jobs.contains_key(&app_name) {
            return Ok(format!("{title} is already downloading"));
        }
        service.jobs.insert(
            app_name.clone(),
            Job {
                progress: Arc::clone(&progress),
                cancel: Arc::clone(&cancel),
            },
        );
    }

    let folder = unique_folder(&folder_name(&title, &app_name), &app_name);
    publish(&app, &app_name, &title, "installing", format!("Downloading {title}"));
    let task_title = title.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = download(&namespace, &catalog_item, &app_name, &task_title, &folder, &progress, &cancel).await;
        if let Ok(mut service) = service().lock() {
            service.jobs.remove(&app_name);
        }
        match outcome {
            Ok(()) => publish(
                &app,
                &app_name,
                &task_title,
                "installed",
                format!("{task_title} is installed. Play starts it with Winlator."),
            ),
            Err(EpicDownloadError::Cancelled) => publish(
                &app,
                &app_name,
                &task_title,
                "cancelled",
                format!("The download of {task_title} was stopped. Install again to pick it up where it left off."),
            ),
            Err(error) => {
                eprintln!("Epic download of {app_name} failed: {error}");
                publish(&app, &app_name, &task_title, "failed", error.to_string());
            }
        }
    });
    Ok(format!("Downloading {title} to this phone"))
}

/// A folder another installed game already uses gets the app name appended, so
/// two games called alike never write into one another.
fn unique_folder(folder: &str, app_name: &str) -> String {
    let taken = records()
        .into_iter()
        .any(|record| record.folder.eq_ignore_ascii_case(folder) && record.app_name != app_name);
    if taken {
        format!("{folder}{}", folder_name(app_name, "Epic"))
            .chars()
            .take(MAX_FOLDER_CHARS * 2)
            .collect()
    } else {
        folder.to_string()
    }
}

async fn download(
    namespace: &str,
    catalog_item: &str,
    app_name: &str,
    title: &str,
    folder: &str,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<(), EpicDownloadError> {
    let location = crate::source_epic::build_location(namespace, catalog_item, app_name)
        .await
        .map_err(|error| EpicDownloadError::Network(error.to_string()))?;
    let client = epic_download::download_client()?;
    let manifest = epic_download::fetch_manifest(&client, &location).await?;
    let root = install_root(folder);
    let fetcher = epic_download::HttpChunkFetcher::new(client, location.chunk_bases.clone());
    epic_download::install_build(&manifest, &fetcher, &root, progress, cancel).await?;

    let launch_exe = if manifest.launch_exe.trim().is_empty() {
        largest_executable(&root).ok_or_else(|| {
            EpicDownloadError::Unsupported(format!(
                "{title} downloaded, but Epic names no program to start and Orivo found none."
            ))
        })?
    } else {
        manifest.launch_exe.replace('\\', "/")
    };
    let record = InstallRecord {
        app_name: app_name.to_string(),
        title: title.to_string(),
        build_version: manifest.build_version.clone(),
        folder: folder.to_string(),
        launch_exe,
        launch_command: manifest.launch_command.clone(),
        install_bytes: manifest.install_size(),
        installed_at_ms: crate::sources::unix_millis(),
    };
    let mut records = records();
    records.retain(|existing| existing.app_name != app_name);
    records.push(record);
    save_records(&records).map_err(EpicDownloadError::Io)
}

/// The biggest `.exe` in the game's top two levels: what a build that names no
/// launch program almost always means.
fn largest_executable(root: &Path) -> Option<String> {
    let mut best: Option<(u64, String)> = None;
    let mut visit = |directory: &Path, prefix: &str| {
        let Ok(entries) = fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut subdirectories = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                subdirectories.push((entry.path(), format!("{prefix}{name}/")));
            } else if name.to_ascii_lowercase().ends_with(".exe")
                && !name.to_ascii_lowercase().contains("crash")
                && best.as_ref().is_none_or(|(size, _)| meta.len() > *size)
            {
                best = Some((meta.len(), format!("{prefix}{name}")));
            }
        }
        subdirectories
    };
    for (directory, prefix) in visit(root, "") {
        visit(&directory, &prefix);
    }
    best.map(|(_, path)| path)
}

pub fn cancel(app_name: &str) -> bool {
    service()
        .lock()
        .ok()
        .and_then(|service| {
            service
                .jobs
                .get(app_name)
                .map(|job| job.cancel.store(true, Ordering::Relaxed))
        })
        .is_some()
}

/// Remove an installed game: its folder, its shortcut and its record. The
/// folder is removed only when it is where Orivo itself put it.
pub fn uninstall(app_name: &str) -> Result<(), String> {
    cancel(app_name);
    let mut records = records();
    let Some(position) = records.iter().position(|record| record.app_name == app_name) else {
        return Err("This game is not installed on this phone.".into());
    };
    let record = records.remove(position);
    let root = install_root(&record.folder);
    let expected_parent = downloads_root().join(INSTALL_DIRECTORY);
    if record.folder.is_empty()
        || record.folder.contains(['/', '\\'])
        || root.parent() != Some(expected_parent.as_path())
    {
        return Err("Orivo will not remove a folder it did not create.".into());
    }
    if root.exists() {
        fs::remove_dir_all(&root).map_err(|error| format!("The game could not be removed: {error}"))?;
    }
    for shortcut in shortcut_candidates(&record.folder) {
        let _ = fs::remove_file(shortcut);
    }
    save_records(&records)
}

/// Write (or refresh) the game's shortcut and hand it to Winlator Cmod.
pub fn launch(app_name: &str, container_id: Option<u32>) -> Result<String, String> {
    let record = record(app_name).ok_or("This game is not installed on this phone yet.")?;
    let executable = install_root(&record.folder).join(&record.launch_exe);
    if !executable.is_file() {
        return Err(format!(
            "{} is missing from this phone. Uninstall it and install it again.",
            record.launch_exe
        ));
    }
    let container = container_id.unwrap_or(1);
    let shortcut = write_shortcut(shortcut_candidates(&record.folder), &shortcut_text(&record, container, adreno_gpu()))?;
    crate::winlator_runner::launch_orivo_shortcut(&record.title, &shortcut, Some(container))
        .map_err(|error| error.to_string())?;
    Ok(format!("Launching {} with Winlator", record.title))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_a_folder_windows_and_a_shortcut_can_carry() {
        assert_eq!(folder_name("Nine Sols", "Salt"), "NineSols");
        assert_eq!(folder_name("Hades II: Early Access", "x"), "HadesIIEarlyAccess");
        assert_eq!(folder_name("東方", "Fable5"), "Fable5");
        assert_eq!(folder_name("—", ""), "EpicGame");
    }

    #[test]
    fn writes_the_path_the_way_cmod_unescapes_it() {
        let path = windows_executable("NineSols", "Binaries/Win64/Nine Sols.exe");
        assert_eq!(path, "D:\\Orivo\\Epic\\NineSols\\Binaries\\Win64\\Nine Sols.exe");
        let exec = exec_line(&path);
        assert_eq!(
            exec,
            "env WINEPREFIX=\"/home/xuser/.wine\" wine D:\\\\\\\\Orivo\\\\\\\\Epic\\\\\\\\NineSols\\\\\\\\Binaries\\\\\\\\Win64\\\\\\\\Nine\\\\ Sols.exe"
        );
        // Cmod's own unescape, step for step: two passes that drop a backslash
        // before anything that is not one, then one that halves the pairs.
        let after_wine = &exec[exec.rfind("wine ").unwrap() + 5..];
        let pass = |text: &str| {
            let mut out = String::new();
            let characters: Vec<char> = text.chars().collect();
            let mut index = 0;
            while index < characters.len() {
                if characters[index] == '\\'
                    && characters.get(index + 1).is_some_and(|next| *next != '\\')
                {
                    index += 1;
                    continue;
                }
                out.push(characters[index]);
                index += 1;
            }
            out
        };
        let unescaped = pass(&pass(after_wine)).replace("\\\\", "\\");
        assert_eq!(unescaped, path);
    }

    #[test]
    fn writes_a_shortcut_cmod_reads_a_container_and_arguments_from() {
        let record = InstallRecord {
            app_name: "Salt".into(),
            title: "Nine Sols".into(),
            build_version: "1.0".into(),
            folder: "NineSols".into(),
            launch_exe: "NineSols.exe".into(),
            launch_command: "-nolauncher".into(),
            install_bytes: 1,
            installed_at_ms: 1,
        };
        let text = shortcut_text(&record, 2, false);
        assert!(text.starts_with("[Desktop Entry]\nName=Nine Sols\n"));
        assert!(text.contains("\nPath=D:\\Orivo\\Epic\\NineSols\n"));
        assert!(text.contains("\nStartupWMClass=ninesols.exe\n"));
        assert!(text.contains("\n[Extra Data]\ncontainer_id=2\nexecArgs=-nolauncher -EpicPortal\n"));
        assert!(!text.contains("graphicsDriverConfig="));
    }

    #[test]
    #[cfg(unix)]
    fn writes_the_next_shortcut_name_when_the_old_file_is_someone_elses() {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!("orivo-shortcut-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let taken = directory.join("Game.desktop");
        fs::write(&taken, "old").unwrap();
        fs::set_permissions(&taken, fs::Permissions::from_mode(0o444)).unwrap();
        let next = directory.join("Game-2.desktop");
        let written = write_shortcut([taken.clone(), next.clone()], "new").unwrap();
        assert_eq!(written, next);
        assert_eq!(fs::read_to_string(&next).unwrap(), "new");
        assert_eq!(fs::read_to_string(&taken).unwrap(), "old");
        fs::set_permissions(&taken, fs::Permissions::from_mode(0o644)).unwrap();
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn asks_for_turnip_on_an_adreno_phone_and_says_epic_once() {
        let record = InstallRecord {
            app_name: "Duel".into(),
            title: "Astro Duel 2".into(),
            build_version: "1.0".into(),
            folder: "AstroDuel2".into(),
            launch_exe: "AstroDuel2_EOS.exe".into(),
            launch_command: "-epicportal".into(),
            install_bytes: 1,
            installed_at_ms: 1,
        };
        let text = shortcut_text(&record, 1, true);
        assert!(text.contains(
            "\ncontainer_id=1\ngraphicsDriverConfig=version=turnip25.1.0;blacklistedExtensions=;maxDeviceMemory=0;adrenotoolsTurnip=1;frameSync=Normal\n"
        ));
        assert!(text.ends_with("\nexecArgs=-epicportal\n"));
        let bare = InstallRecord { launch_command: String::new(), ..record };
        assert!(shortcut_text(&bare, 1, true).ends_with("\nexecArgs=-EpicPortal\n"));
    }

    #[test]
    fn reads_an_epic_launch_reference() {
        assert_eq!(
            parse_app_ref("ns:item:Salt"),
            Some(("ns".into(), "item".into(), "Salt".into()))
        );
        assert_eq!(parse_app_ref("ns:item"), None);
        assert_eq!(parse_app_ref("::"), None);
    }

    #[test]
    fn records_installs_and_removes_only_its_own_folders() {
        let base = std::env::temp_dir().join(format!("orivo-epic-android-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        configure(base.join("data"), Some(base.join("Download")));
        let record = InstallRecord {
            app_name: "Salt".into(),
            title: "Nine Sols".into(),
            build_version: "1.0".into(),
            folder: "NineSols".into(),
            launch_exe: "NineSols.exe".into(),
            launch_command: String::new(),
            install_bytes: 42,
            installed_at_ms: 1,
        };
        fs::create_dir_all(install_root("NineSols")).unwrap();
        fs::write(install_root("NineSols").join("NineSols.exe"), b"MZ").unwrap();
        save_records(std::slice::from_ref(&record)).unwrap();

        let statuses = statuses();
        assert_eq!(statuses["Salt"].state, EpicInstallState::Installed);
        assert_eq!(statuses["Salt"].total_bytes, 42);
        assert_eq!(unique_folder("NineSols", "Other"), "NineSolsOther");
        assert_eq!(unique_folder("NineSols", "Salt"), "NineSols");

        // A record that names a folder elsewhere is never followed.
        let mut forged = record.clone();
        forged.app_name = "Forged".into();
        forged.folder = "../..".into();
        save_records(&[record.clone(), forged]).unwrap();
        assert!(uninstall("Forged").is_err());

        uninstall("Salt").unwrap();
        assert!(!install_root("NineSols").exists());
        assert!(record_for_test("Salt").is_none());
        let _ = fs::remove_dir_all(&base);
    }

    fn record_for_test(app_name: &str) -> Option<InstallRecord> {
        record(app_name)
    }
}
