//! Host-executed game acquisition, gated on an installer plugin.
//!
//! Orivo never depends on this feature being present. When no plugin declares
//! the `installer` extension the service reports itself unavailable and the
//! Store renders exactly as it does today. When a plugin is installed, the
//! plugin supplies *data only* — a bounded catalogue of titles, download URLs,
//! digests and sizes — and the host performs every privileged step itself:
//! the HTTPS fetch (restricted to the manifest's declared allowlist), the
//! SHA-256 verification, and the silent extraction of the Windows installer
//! through Wine into a plain macOS directory.
//!
//! The WebView only ever sends an opaque catalogue slug. It cannot supply a
//! URL, a path, an installer flag or a command line.

use crate::plugin_manifest::HostCompatibility;
use crate::plugin_registry::{InstallerPlugin, PluginRegistry, PluginState};
use crate::plugin_runtime::PluginRuntime;
use crate::wine_runner;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, State};

/// Every acquisition update is addressed to the one application window, never
/// broadcast to the capability-free authentication WebView.
const MAIN_WINDOW_LABEL: &str = "main";
pub const QUIKY_PROGRESS_EVENT: &str = "quiky-install-status";

const CATALOG_ASSET: &str = "assets/catalog.json";
const MAX_CATALOG_BYTES: u64 = 512 * 1024;
const MAX_CATALOG_TITLES: usize = 256;
const MAX_DOWNLOAD_BYTES: u64 = 24 * 1024 * 1024 * 1024;
const DOWNLOADS_DIRECTORY: &str = "quiky-downloads";
const INSTALLER_PROFILE_ID: &str = "quiky-installer";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(220);
const PREFIX_BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(180);
const INSTALLER_TIMEOUT: Duration = Duration::from_secs(1800);
const MAX_SIZE_SCAN_ENTRIES: usize = 200_000;

// ---------------------------------------------------------------------------
// Catalogue supplied by the plugin (untrusted data, validated on every read)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuikyCatalog {
    #[allow(dead_code)]
    version: u32,
    titles: Vec<QuikyCatalogTitle>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuikyCatalogTitle {
    slug: String,
    title: String,
    #[serde(default)]
    match_titles: Vec<String>,
    url: String,
    sha256: String,
    download_bytes: u64,
    #[serde(default)]
    installed_bytes: u64,
    installer_kind: String,
    folder_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallerKind {
    Nsis,
    Inno,
}

impl InstallerKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "nsis" => Some(Self::Nsis),
            "inno" => Some(Self::Inno),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedTitle {
    slug: String,
    title: String,
    match_titles: Vec<String>,
    url: String,
    sha256: String,
    download_bytes: u64,
    installed_bytes: u64,
    kind: InstallerKind,
    folder_name: String,
}

// ---------------------------------------------------------------------------
// IPC views
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuikyTitleView {
    pub slug: String,
    pub title: String,
    pub match_titles: Vec<String>,
    pub download_bytes: u64,
    pub installed: bool,
    pub install_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuikyStatusView {
    pub available: bool,
    pub plugin_name: String,
    pub version: String,
    pub message: String,
    pub titles: Vec<QuikyTitleView>,
}

impl QuikyStatusView {
    fn unavailable(message: &str) -> Self {
        Self {
            available: false,
            plugin_name: String::new(),
            version: String::new(),
            message: message.into(),
            titles: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuikyProgressView {
    pub slug: String,
    pub phase: &'static str,
    pub percent: u8,
    pub message: String,
    pub install_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct InstallJob {
    cancelled: Arc<AtomicBool>,
    progress: Mutex<QuikyProgressView>,
}

#[derive(Debug)]
pub struct QuikyService {
    plugin_root: PathBuf,
    wine_prefix_root: PathBuf,
    downloads_root: PathBuf,
    games_root: PathBuf,
    host_version: &'static str,
    jobs: Mutex<BTreeMap<String, Arc<InstallJob>>>,
}

impl QuikyService {
    pub fn new(
        plugin_root: PathBuf,
        wine_prefix_root: PathBuf,
        cache_root: PathBuf,
        games_root: PathBuf,
        host_version: &'static str,
    ) -> Self {
        Self {
            plugin_root,
            wine_prefix_root,
            downloads_root: cache_root.join(DOWNLOADS_DIRECTORY),
            games_root,
            host_version,
            jobs: Mutex::new(BTreeMap::new()),
        }
    }

    fn plugin(&self) -> Option<InstallerPlugin> {
        let runtime = PluginRuntime::new().ok()?;
        PluginRegistry::new(
            self.plugin_root.clone(),
            HostCompatibility::v1(self.host_version),
        )
        .installer_plugin(&runtime)
    }

    fn destination(&self, folder_name: &str) -> PathBuf {
        self.games_root.join(folder_name)
    }

    fn job(&self, slug: &str) -> Option<Arc<InstallJob>> {
        self.jobs.lock().ok()?.get(slug).cloned()
    }
}

// ---------------------------------------------------------------------------
// Catalogue loading and validation
// ---------------------------------------------------------------------------

/// A slug is an opaque identifier chosen by the plugin. It is used as a map
/// key and as a download subdirectory, so it may never contain a separator or
/// a relative segment.
fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-'))
}

/// The install folder is created under the user's games directory. It must be
/// a single, non-relative, non-hidden path segment.
fn valid_folder_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && !value.starts_with('.')
        && !value.contains('/')
        && !value.contains('\\')
        && value != "."
        && value != ".."
        && value
            .chars()
            .all(|character| character.is_ascii_graphic() || character == ' ')
        && !value.contains(':')
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A URL is only usable when it is HTTPS and its host is inside the plugin
/// manifest's declared allowlist. The allowlist was shown to the user at
/// install time; catalogue data cannot widen it.
fn host_allowed(host: &str, allowlist: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    allowlist.iter().any(|domain| {
        let domain = domain.trim().to_ascii_lowercase();
        match domain.strip_prefix("*.") {
            Some(suffix) => host == suffix || host.ends_with(&format!(".{suffix}")),
            None => host == domain,
        }
    })
}

fn url_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|value| !value.is_empty())?;
    // Credentials in a catalogue URL would be an exfiltration channel and are
    // never part of a legitimate download link.
    if authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn read_bounded(path: &Path, max_bytes: u64) -> Option<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > max_bytes {
        return None;
    }
    fs::read(path).ok()
}

fn load_catalog(plugin: &InstallerPlugin) -> Vec<ResolvedTitle> {
    let Some(bytes) = read_bounded(&plugin.directory.join(CATALOG_ASSET), MAX_CATALOG_BYTES) else {
        return Vec::new();
    };
    let Ok(catalog) = serde_json::from_slice::<QuikyCatalog>(&bytes) else {
        return Vec::new();
    };
    let mut seen = Vec::new();
    catalog
        .titles
        .into_iter()
        .take(MAX_CATALOG_TITLES)
        .filter_map(|entry| resolve_title(entry, &plugin.network_domains))
        .filter(|title| {
            let fresh = !seen.contains(&title.slug);
            if fresh {
                seen.push(title.slug.clone());
            }
            fresh
        })
        .collect()
}

fn resolve_title(entry: QuikyCatalogTitle, allowlist: &[String]) -> Option<ResolvedTitle> {
    if !valid_slug(&entry.slug)
        || !valid_folder_name(&entry.folder_name)
        || !valid_sha256(&entry.sha256)
        || entry.title.trim().is_empty()
        || entry.title.chars().count() > 128
        || entry.download_bytes == 0
        || entry.download_bytes > MAX_DOWNLOAD_BYTES
    {
        return None;
    }
    let kind = InstallerKind::parse(&entry.installer_kind)?;
    let host = url_host(&entry.url)?;
    if !host_allowed(&host, allowlist) {
        return None;
    }
    Some(ResolvedTitle {
        slug: entry.slug,
        title: entry.title,
        match_titles: entry
            .match_titles
            .into_iter()
            .filter(|value| !value.trim().is_empty() && value.chars().count() <= 128)
            .take(16)
            .collect(),
        url: entry.url,
        sha256: entry.sha256.to_ascii_lowercase(),
        download_bytes: entry.download_bytes,
        installed_bytes: entry.installed_bytes,
        kind,
        folder_name: entry.folder_name,
    })
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_quiky_status(
    service: State<'_, Arc<QuikyService>>,
) -> Result<QuikyStatusView, String> {
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || {
        let Some(plugin) = service.plugin() else {
            return QuikyStatusView::unavailable("No installer plugin is installed.");
        };
        if plugin.state != PluginState::Ready {
            return QuikyStatusView::unavailable(&plugin.message);
        }
        let titles = load_catalog(&plugin)
            .into_iter()
            .map(|title| {
                let destination = service.destination(&title.folder_name);
                let installed = directory_has_content(&destination);
                QuikyTitleView {
                    slug: title.slug,
                    title: title.title,
                    match_titles: title.match_titles,
                    download_bytes: title.download_bytes,
                    installed,
                    install_path: installed.then(|| destination.to_string_lossy().into_owned()),
                }
            })
            .collect::<Vec<_>>();
        QuikyStatusView {
            available: true,
            plugin_name: plugin.name,
            version: plugin.version,
            message: if titles.is_empty() {
                "This plugin does not list any installable title yet.".into()
            } else {
                String::new()
            },
            titles,
        }
    })
    .await
    .map_err(|_| "The installer plugin could not be read.".to_string())
}

#[tauri::command]
pub fn get_quiky_progress(
    slug: String,
    service: State<'_, Arc<QuikyService>>,
) -> Result<Option<QuikyProgressView>, String> {
    Ok(service
        .job(&slug)
        .and_then(|job| job.progress.lock().ok().map(|progress| progress.clone())))
}

#[tauri::command]
pub fn cancel_quiky_install(
    slug: String,
    service: State<'_, Arc<QuikyService>>,
) -> Result<(), String> {
    let Some(job) = service.job(&slug) else {
        return Ok(());
    };
    job.cancelled.store(true, Ordering::Release);
    Ok(())
}

#[tauri::command]
pub async fn start_quiky_install(
    app: AppHandle,
    slug: String,
    game_id: Option<String>,
    service: State<'_, Arc<QuikyService>>,
) -> Result<(), String> {
    let service = Arc::clone(&service);
    let plugin = {
        let service = Arc::clone(&service);
        tauri::async_runtime::spawn_blocking(move || service.plugin())
            .await
            .map_err(|_| "The installer plugin could not be read.".to_string())?
    }
    .ok_or_else(|| "No installer plugin is installed.".to_string())?;
    if plugin.state != PluginState::Ready {
        return Err(plugin.message);
    }
    let title = load_catalog(&plugin)
        .into_iter()
        .find(|candidate| candidate.slug == slug)
        .ok_or_else(|| "This title is not part of the installer catalogue.".to_string())?;

    // Refuse an install that cannot fit before spending the bandwidth on it.
    let required = title
        .download_bytes
        .saturating_add(title.installed_bytes)
        .saturating_add(512 * 1024 * 1024);
    let available = free_disk_bytes(&service.games_root);
    if available > 0 && available < required {
        return Err(format!(
            "Il manque de l'espace disque : {} nécessaires, {} disponibles.",
            human_bytes(required),
            human_bytes(available)
        ));
    }

    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let mut jobs = service
            .jobs
            .lock()
            .map_err(|_| "The installer is busy.".to_string())?;
        if let Some(existing) = jobs.get(&slug)
            && existing.progress.lock().is_ok_and(|progress| {
                matches!(progress.phase, "queued" | "downloading" | "extracting")
            })
        {
            return Err("This game is already being installed.".into());
        }
        jobs.insert(
            slug.clone(),
            Arc::new(InstallJob {
                cancelled: Arc::clone(&cancelled),
                progress: Mutex::new(QuikyProgressView {
                    slug: slug.clone(),
                    phase: "queued",
                    percent: 0,
                    message: "Preparing…".into(),
                    install_path: None,
                }),
            }),
        );
    }

    let job_app = app.clone();
    let job_service = Arc::clone(&service);
    tauri::async_runtime::spawn(async move {
        let outcome = run_install(
            &job_app,
            &job_service,
            &title,
            game_id.as_deref(),
            &cancelled,
        )
        .await;
        match outcome {
            Ok(path) => publish(
                &job_app,
                &job_service,
                &title.slug,
                "installed",
                100,
                "Installed.".into(),
                Some(path.to_string_lossy().into_owned()),
            ),
            Err(error) if cancelled.load(Ordering::Acquire) => publish(
                &job_app,
                &job_service,
                &title.slug,
                "cancelled",
                0,
                error,
                None,
            ),
            Err(error) => publish(
                &job_app,
                &job_service,
                &title.slug,
                "failed",
                0,
                error,
                None,
            ),
        }
    });
    Ok(())
}

fn publish(
    app: &AppHandle,
    service: &QuikyService,
    slug: &str,
    phase: &'static str,
    percent: u8,
    message: String,
    install_path: Option<String>,
) {
    append_log(service, &format!("[{phase}] {slug} {percent}% {message}"));
    let view = QuikyProgressView {
        slug: slug.to_string(),
        phase,
        percent: percent.min(100),
        message,
        install_path,
    };
    if let Some(job) = service.job(slug)
        && let Ok(mut progress) = job.progress.lock()
    {
        *progress = view.clone();
    }
    let _ = app.emit_to(MAIN_WINDOW_LABEL, QUIKY_PROGRESS_EVENT, view);
}

/// The installation log. Orivo keeps one rolling file so a failed install can
/// be diagnosed after the fact without re-running it, mirroring the
/// `install_log.txt` the original PowerShell tool wrote.
const LOG_FILE: &str = "quiky-install.log";
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

fn append_log(service: &QuikyService, line: &str) {
    let path = service.downloads_root.join(LOG_FILE);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // A log that grows without bound is a log nobody reads. Rotating on size
    // keeps the most recent run whole rather than truncating mid-install.
    if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > MAX_LOG_BYTES) {
        let _ = fs::rename(&path, path.with_extension("log.1"));
    }
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{seconds} {line}");
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuikyDiagnosticsView {
    pub plugin_available: bool,
    pub plugin_message: String,
    pub title_count: usize,
    /// Whether a Wine installation Orivo recognises is present. The path
    /// itself stays in the backend.
    pub wine_ready: bool,
    pub wine_message: String,
    pub games_folder: String,
    pub downloads_bytes: u64,
    pub installed_bytes: u64,
    pub free_disk_bytes: u64,
    pub log_lines: Vec<String>,
}

/// The `config` and `validate` commands of the original tool, merged into one
/// read-only snapshot: what is installed, what is missing, and what the last
/// run did.
#[tauri::command]
pub async fn get_quiky_diagnostics(
    service: State<'_, Arc<QuikyService>>,
) -> Result<QuikyDiagnosticsView, String> {
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || {
        let plugin = service.plugin();
        let titles = plugin
            .as_ref()
            .filter(|plugin| plugin.state == PluginState::Ready)
            .map(load_catalog)
            .unwrap_or_default();
        let wine = wine_runner::detect_wine_staging(&AtomicBool::new(false))
            .ok()
            .flatten();
        QuikyDiagnosticsView {
            plugin_available: plugin
                .as_ref()
                .is_some_and(|plugin| plugin.state == PluginState::Ready),
            plugin_message: plugin
                .as_ref()
                .map(|plugin| plugin.message.clone())
                .unwrap_or_else(|| "Aucun plugin installeur n'est installé.".into()),
            title_count: titles.len(),
            wine_ready: wine.is_some(),
            wine_message: if wine.is_some() {
                "Wine-Staging est détecté.".into()
            } else {
                "Installe Wine-Staging pour extraire les installeurs Windows.".into()
            },
            games_folder: service.games_root.to_string_lossy().into_owned(),
            downloads_bytes: directory_bytes(&service.downloads_root),
            installed_bytes: directory_bytes(&service.games_root),
            free_disk_bytes: free_disk_bytes(&service.games_root),
            log_lines: tail_log(&service, 60),
        }
    })
    .await
    .map_err(|_| "Le diagnostic n'a pas abouti.".to_string())
}

fn tail_log(service: &QuikyService, lines: usize) -> Vec<String> {
    let Ok(contents) = fs::read_to_string(service.downloads_root.join(LOG_FILE)) else {
        return Vec::new();
    };
    let collected = contents.lines().collect::<Vec<_>>();
    collected
        .iter()
        .skip(collected.len().saturating_sub(lines))
        .map(|line| (*line).to_string())
        .collect()
}

/// Free space on the volume that holds the games folder. A repack that cannot
/// fit should be refused before the download, not after it.
fn free_disk_bytes(path: &Path) -> u64 {
    let mut probe = path.to_path_buf();
    // The folder may not exist yet on a first run; walk up to a real ancestor.
    while !probe.exists() {
        match probe.parent() {
            Some(parent) => probe = parent.to_path_buf(),
            None => return 0,
        }
    }
    free_space_of_existing(&probe)
}

/// The volume query, per family. Both answer in bytes available to this user,
/// which is the number the pre-flight check needs: a quota can leave a volume
/// with plenty of raw free space and still refuse the write.
#[cfg(unix)]
fn free_space_of_existing(probe: &Path) -> u64 {
    let Ok(encoded) = std::ffi::CString::new(probe.as_os_str().as_encoded_bytes()) else {
        return 0;
    };
    // SAFETY: `statfs` only reads through the NUL-terminated path above and
    // fills the caller-owned struct, which is zeroed before the call.
    unsafe {
        let mut stats: libc::statfs = std::mem::zeroed();
        if libc::statfs(encoded.as_ptr(), &mut stats) != 0 {
            return 0;
        }
        (stats.f_bavail as u64).saturating_mul(stats.f_bsize as u64)
    }
}

#[cfg(windows)]
fn free_space_of_existing(probe: &Path) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    // A directory path is enough: the call reports on the volume holding it.
    let wide: Vec<u16> = probe.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available: u64 = 0;
    // SAFETY: the path is NUL-terminated above and only read; the one output
    // pointer is a live local, and the two totals are opted out of with null.
    let queried = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if queried == 0 { 0 } else { available }
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

async fn run_install(
    app: &AppHandle,
    service: &Arc<QuikyService>,
    title: &ResolvedTitle,
    store_game_id: Option<&str>,
    cancelled: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    let package = download_package(app, service, title, cancelled).await?;
    let service = Arc::clone(service);
    let title = title.clone();
    let cancelled = Arc::clone(cancelled);
    let app = app.clone();
    let store_game_id = store_game_id.map(str::to_owned);
    tauri::async_runtime::spawn_blocking(move || {
        extract_package(
            &app,
            &service,
            &title,
            &package,
            store_game_id.as_deref(),
            &cancelled,
        )
    })
    .await
    .map_err(|_| "The installation did not finish.".to_string())?
}

async fn download_package(
    app: &AppHandle,
    service: &Arc<QuikyService>,
    title: &ResolvedTitle,
    cancelled: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    let directory = service.downloads_root.join(&title.slug);
    fs::create_dir_all(&directory)
        .map_err(|_| "The download folder is unavailable.".to_string())?;
    let package = directory.join("setup.exe");

    // A previously completed, digest-matching download is reused rather than
    // re-fetched. This makes a cancelled or failed install cheap to retry.
    if fs::metadata(&package).is_ok_and(|meta| meta.len() == title.download_bytes)
        && file_digest(&package).is_some_and(|digest| digest == title.sha256)
    {
        publish(
            app,
            service,
            &title.slug,
            "downloading",
            100,
            "Already downloaded.".into(),
            None,
        );
        return Ok(package);
    }

    publish(
        app,
        service,
        &title.slug,
        "downloading",
        0,
        "Starting the download…".into(),
        None,
    );

    let allowlist = title_allowlist(service, title);
    let temporary = directory.join("setup.exe.part");
    let total = title.download_bytes;
    fetch_verified_package(
        &title.url,
        &title.sha256,
        total,
        &allowlist,
        &temporary,
        cancelled,
        &mut |written| {
            publish(
                app,
                service,
                &title.slug,
                "downloading",
                percentage(written, total),
                format!(
                    "Downloading {} of {}",
                    human_bytes(written),
                    human_bytes(total)
                ),
                None,
            );
        },
    )
    .await?;
    fs::rename(&temporary, &package)
        .map_err(|_| "The verified package could not be stored.".to_string())?;
    publish(
        app,
        service,
        &title.slug,
        "downloading",
        100,
        "Download verified.".into(),
        None,
    );
    Ok(package)
}

/// The whole network surface, in one place: the allowlist is re-checked on
/// every redirect hop, the stream is bounded, and the file is only accepted
/// when both its length and its SHA-256 match the catalogue entry.
async fn fetch_verified_package(
    url: &str,
    expected_sha256: &str,
    expected_bytes: u64,
    allowlist: &[String],
    temporary: &Path,
    cancelled: &AtomicBool,
    on_progress: &mut (dyn FnMut(u64) + Send),
) -> Result<(), String> {
    let redirect_allowlist = allowlist.to_vec();
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        // Every redirect hop is re-checked against the manifest allowlist, so
        // a moved asset cannot silently pull bytes from another host.
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 6 {
                return attempt.stop();
            }
            let allowed = attempt.url().scheme() == "https"
                && attempt
                    .url()
                    .host_str()
                    .is_some_and(|host| host_allowed(host, &redirect_allowlist));
            if allowed {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|_| "The download client could not start.".to_string())?;

    if !url_host(url).is_some_and(|host| host_allowed(&host, allowlist)) {
        return Err("This download is outside the plugin's declared domains.".into());
    }
    if expected_bytes == 0 || expected_bytes > MAX_DOWNLOAD_BYTES {
        return Err("This package is larger than Orivo allows.".into());
    }

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "The download could not start.".to_string())?;
    if !response.status().is_success() {
        return Err("The download source rejected the request.".into());
    }
    if response
        .content_length()
        .is_some_and(|length| length != expected_bytes)
    {
        return Err("The package size does not match its catalogue entry.".into());
    }

    if let Some(parent) = temporary.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| "The download folder is unavailable.".to_string())?;
    }
    let mut file =
        fs::File::create(temporary).map_err(|_| "The download file is unavailable.".to_string())?;
    let mut digest = Sha256::new();
    let mut written = 0_u64;
    let mut last_tick = Instant::now();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancelled.load(Ordering::Acquire) {
            let _ = fs::remove_file(temporary);
            return Err("Installation cancelled.".into());
        }
        let chunk = chunk.map_err(|_| "The download was interrupted.".to_string())?;
        written = written.saturating_add(chunk.len() as u64);
        if written > expected_bytes {
            let _ = fs::remove_file(temporary);
            return Err("The package is larger than its catalogue entry.".into());
        }
        digest.update(&chunk);
        file.write_all(&chunk)
            .map_err(|_| "The download could not be written to disk.".to_string())?;
        if last_tick.elapsed() >= PROGRESS_INTERVAL {
            last_tick = Instant::now();
            on_progress(written);
        }
    }
    file.flush()
        .and_then(|()| file.sync_all())
        .map_err(|_| "The download could not be completed.".to_string())?;
    drop(file);

    if written != expected_bytes {
        let _ = fs::remove_file(temporary);
        return Err("The package size does not match its catalogue entry.".into());
    }
    if format!("{:x}", digest.finalize()) != expected_sha256 {
        let _ = fs::remove_file(temporary);
        return Err("The package failed its integrity check.".into());
    }
    Ok(())
}

fn title_allowlist(service: &QuikyService, title: &ResolvedTitle) -> Vec<String> {
    service
        .plugin()
        .map(|plugin| plugin.network_domains)
        .unwrap_or_else(|| url_host(&title.url).into_iter().collect())
}

// ---------------------------------------------------------------------------
// Extraction through Wine, with no visible window
// ---------------------------------------------------------------------------

fn extract_package(
    app: &AppHandle,
    service: &Arc<QuikyService>,
    title: &ResolvedTitle,
    package: &Path,
    store_game_id: Option<&str>,
    cancelled: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    publish(
        app,
        service,
        &title.slug,
        "extracting",
        0,
        "Preparing the Windows runtime…".into(),
        None,
    );

    let wine = wine_runner::detect_wine_staging(&AtomicBool::new(false))
        .ok()
        .flatten()
        .ok_or_else(|| {
            "Wine is required to unpack this installer. Install Wine-Staging and try again."
                .to_string()
        })?;
    // A prefix that already records this game makes the installer run its
    // "remove the previous version" path, which fails and raises a modal the
    // user has to click. Every install therefore starts from a virgin prefix,
    // one per title, so the installer only ever sees a clean machine.
    let profile_id = format!("{INSTALLER_PROFILE_ID}-{}", title.slug);
    let stale = service.wine_prefix_root.join(&profile_id);
    if stale.exists() {
        stop_wineserver(&wine, &stale);
        let _ = fs::remove_dir_all(&stale);
    }
    let prefix = wine_runner::ensure_managed_profile_prefix(&service.wine_prefix_root, &profile_id)
        .map_err(|_| "The Windows runtime could not be prepared.".to_string())?;
    bootstrap_prefix(&wine, &prefix, cancelled)?;

    let destination = service.destination(&title.folder_name);
    // Leftover files from a failed attempt would let the installer skip work
    // and report success over a half-written game.
    let _ = fs::remove_dir_all(&destination);
    fs::create_dir_all(&destination)
        .map_err(|_| "The games folder could not be created.".to_string())?;
    let windows_destination = windows_path(&destination)
        .ok_or_else(|| "The games folder is outside the Windows view.".to_string())?;

    // Headless first: with no graphics driver the installer cannot draw
    // anything at all. The Mac driver is only used as a fallback for
    // installers that refuse to run without a display, and even then the
    // silent switches below mean no window is created.
    let mut last_error = String::new();
    for attempt in 0..2 {
        if cancelled.load(Ordering::Acquire) {
            return Err("Installation cancelled.".into());
        }
        // A wineserver left over from an earlier attempt owns the prefix and
        // makes the next `wine` invocation block instead of running.
        stop_wineserver(&wine, &prefix);
        if attempt == 0 {
            set_graphics_driver(&wine, &prefix, false);
        }
        match run_installer(
            app,
            service,
            title,
            &wine,
            &prefix,
            package,
            &windows_destination,
            &destination,
            cancelled,
        ) {
            Ok(()) => {
                if directory_has_content(&destination) {
                    stop_wineserver(&wine, &prefix);
                    // An installed game that is not in the library would leave
                    // the user with files and no way to launch them.
                    if let Some(executable) = primary_executable(&destination) {
                        let _ = crate::register_installed_game(app, executable, store_game_id);
                    }
                    return Ok(destination);
                }
                last_error = "The installer produced no files.".into();
            }
            Err(error) => {
                if cancelled.load(Ordering::Acquire) {
                    return Err("Installation cancelled.".into());
                }
                last_error = error;
            }
        }
    }
    stop_wineserver(&wine, &prefix);
    let _ = fs::remove_dir(&destination);
    Err(last_error)
}

/// `wineboot -u` creates the private prefix. Mono and Gecko are disabled so
/// Wine never raises its own download dialog on a user's screen.
fn bootstrap_prefix(wine: &Path, prefix: &Path, cancelled: &AtomicBool) -> Result<(), String> {
    if prefix.join("user.reg").is_file() && prefix.join("drive_c").is_dir() {
        return Ok(());
    }
    let mut child = wine_command(wine, prefix, true)
        .arg("wineboot")
        .arg("-u")
        .spawn()
        .map_err(|_| "The Windows runtime could not start.".to_string())?;
    wait_with_deadline(&mut child, PREFIX_BOOTSTRAP_TIMEOUT, cancelled)
        .map_err(|_| "The Windows runtime did not finish preparing.".to_string())?;
    Ok(())
}

/// Wine keeps one server per prefix. Leaving it alive between attempts is
/// what turns a retry into a hang, so it is stopped before every run.
fn stop_wineserver(wine: &Path, prefix: &Path) {
    let Some(server) = wine.parent().map(|dir| dir.join("wineserver")) else {
        return;
    };
    if !server.is_file() {
        return;
    }
    let mut command = Command::new(server);
    command
        .arg("-k")
        .arg("-w")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("WINEPREFIX", prefix);
    if let Ok(mut child) = command.spawn() {
        let _ = wait_with_deadline(&mut child, Duration::from_secs(15), &AtomicBool::new(false));
    }
}

fn set_graphics_driver(wine: &Path, prefix: &Path, headless: bool) {
    let mut command = wine_command(wine, prefix, true);
    command
        .arg("reg")
        .arg("add")
        .arg(r"HKEY_CURRENT_USER\Software\Wine\Drivers")
        .arg("/v")
        .arg("Graphics")
        .arg("/t")
        .arg("REG_SZ")
        .arg("/d")
        .arg(if headless { "" } else { "mac" })
        .arg("/f");
    if let Ok(mut child) = command.spawn() {
        let _ = wait_with_deadline(&mut child, Duration::from_secs(20), &AtomicBool::new(false));
    }
}

#[allow(clippy::too_many_arguments)]
fn run_installer(
    app: &AppHandle,
    service: &Arc<QuikyService>,
    title: &ResolvedTitle,
    wine: &Path,
    prefix: &Path,
    package: &Path,
    windows_destination: &str,
    destination: &Path,
    cancelled: &Arc<AtomicBool>,
) -> Result<(), String> {
    let mut command = wine_command(wine, prefix, false);
    command.arg(package);
    match title.kind {
        // Inno Setup: /VERYSILENT suppresses the progress window entirely.
        InstallerKind::Inno => {
            command
                .arg("/VERYSILENT")
                .arg("/SUPPRESSMSGBOXES")
                .arg("/NORESTART")
                .arg("/SP-")
                .arg(format!("/DIR={windows_destination}"));
        }
        // NSIS: /S is the silent switch and /D must come last, unquoted.
        InstallerKind::Nsis => {
            command.arg("/S").arg(format!("/D={windows_destination}"));
        }
    }
    if let Some(parent) = package.parent() {
        command.current_dir(parent);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "The installer could not be started.".to_string())?;

    let expected = if title.installed_bytes > 0 {
        title.installed_bytes
    } else {
        title.download_bytes.saturating_mul(3)
    };
    let deadline = Instant::now() + INSTALLER_TIMEOUT;
    loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Installation cancelled.".into());
        }
        match child.try_wait() {
            // A silent installer that refuses the current graphics driver
            // exits non-zero without writing anything. Reporting that as a
            // failure is what lets the caller retry on the other driver.
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err("The installer could not unpack this package.".into()),
            Err(_) => return Err("The installer stopped unexpectedly.".into()),
            Ok(None) => {}
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("The installer took too long and was stopped.".into());
        }
        let written = directory_bytes(destination);
        publish(
            app,
            service,
            &title.slug,
            "extracting",
            percentage(written, expected).min(97),
            format!("Extracting {}", human_bytes(written)),
            None,
        );
        thread::sleep(PROGRESS_INTERVAL);
    }
}

/// Wine reads a broad family of `WINE*` switches, and macOS honours `DYLD_*`
/// injection. Both families are scrubbed so the parent environment can never
/// redirect the loader or the prefix of an installer Orivo starts.
fn wine_command(wine: &Path, prefix: &Path, quiet: bool) -> Command {
    let mut command = Command::new(wine);
    for key in [
        "WINEPREFIX",
        "WINEARCH",
        "WINEDLLOVERRIDES",
        "WINEDEBUG",
        "WINESERVER",
        "WINELOADER",
        "WINELOADERNOEXEC",
        "DYLD_INSERT_LIBRARIES",
        "DYLD_LIBRARY_PATH",
        "DYLD_FRAMEWORK_PATH",
    ] {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        let Some(name) = key.to_str() else { continue };
        let upper = name.to_ascii_uppercase();
        if upper.starts_with("WINE") || upper.starts_with("DYLD_") {
            command.env_remove(key);
        }
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("WINEPREFIX", prefix)
        .env("WINEDEBUG", if quiet { "-all" } else { "fixme-all" })
        // Wine's own Mono and Gecko prompts are the only dialogs a silent
        // installer can still raise. Disabling both keeps the run invisible.
        .env("WINEDLLOVERRIDES", "mscoree,mshtml=");
    command
}

fn wait_with_deadline(
    child: &mut std::process::Child,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<(), ()> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(());
        }
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Err(_) => return Err(()),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(());
            }
            Ok(None) => thread::sleep(Duration::from_millis(120)),
        }
    }
}

/// `Z:` is Wine's standard mapping of the host root, so a POSIX path becomes a
/// Windows path without creating a new drive mapping for the installer.
fn windows_path(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    if !text.starts_with('/') || text.contains('\\') {
        return None;
    }
    Some(format!("Z:{}", text.replace('/', "\\")))
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn percentage(done: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((done.min(total) as f64 / total as f64) * 100.0).round() as u8
}

fn human_bytes(value: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Hash an already-downloaded package in bounded chunks so a resumed install
/// never has to hold a multi-gigabyte file in memory to prove it is intact.
fn file_digest(path: &Path) -> Option<String> {
    use std::io::Read;

    let mut file = fs::File::open(path).ok()?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Some(format!("{:x}", digest.finalize()))
}

/// The game is the biggest executable the installer left behind. Uninstallers
/// and helper tools ship alongside it and must never become the library card.
fn primary_executable(directory: &Path) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    let mut stack = vec![directory.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !name.ends_with(".exe") || name.starts_with("unins") {
                continue;
            }
            let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            if best.as_ref().is_none_or(|(largest, _)| size > *largest) {
                best = Some((size, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

fn directory_has_content(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
}

fn directory_bytes(path: &Path) -> u64 {
    let mut total = 0_u64;
    let mut visited = 0_usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_SIZE_SCAN_ENTRIES {
                return total;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(entry.path());
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

    /// The plugin lives in its own project and is installed as a package, so
    /// the host suite looks for a checked-out build and skips when there is
    /// none rather than pinning the host to the plugin's layout.
    fn shipped_plugin_root() -> Option<PathBuf> {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
        [
            workspace.join("plugin-quiky/dist/stage"),
            workspace.join("plugins"),
        ]
        .into_iter()
        .find(|candidate| candidate.join("com.orivo.quiky/manifest.json").is_file())
    }

    #[test]
    fn only_allowlisted_https_hosts_are_usable() {
        let allowlist = vec![
            "cdn.openttd.org".to_string(),
            "*.githubusercontent.com".to_string(),
        ];
        assert!(host_allowed("cdn.openttd.org", &allowlist));
        assert!(host_allowed("objects.githubusercontent.com", &allowlist));
        assert!(host_allowed("githubusercontent.com", &allowlist));
        assert!(!host_allowed("evil.cdn.openttd.org", &allowlist));
        assert!(!host_allowed("attacker.example", &allowlist));
    }

    #[test]
    fn plain_http_and_credentialed_urls_have_no_host() {
        assert_eq!(
            url_host("https://cdn.openttd.org/a.exe").as_deref(),
            Some("cdn.openttd.org")
        );
        assert_eq!(url_host("http://cdn.openttd.org/a.exe"), None);
        assert_eq!(url_host("https://user:pass@cdn.openttd.org/a.exe"), None);
        assert_eq!(url_host("https:///a.exe"), None);
    }

    #[test]
    fn slugs_and_folder_names_cannot_escape_their_directory() {
        assert!(valid_slug("openttd"));
        assert!(!valid_slug("../etc"));
        assert!(!valid_slug("Open TTD"));
        assert!(valid_folder_name("OpenTTD"));
        assert!(!valid_folder_name("../Games"));
        assert!(!valid_folder_name(".hidden"));
        assert!(!valid_folder_name("C:games"));
    }

    #[test]
    fn a_catalogue_entry_outside_the_allowlist_is_dropped() {
        let entry = QuikyCatalogTitle {
            slug: "openttd".into(),
            title: "OpenTTD".into(),
            match_titles: vec!["openttd".into()],
            url: "https://elsewhere.example/openttd.exe".into(),
            sha256: "f".repeat(64),
            download_bytes: 10,
            installed_bytes: 20,
            installer_kind: "nsis".into(),
            folder_name: "OpenTTD".into(),
        };
        assert!(resolve_title(entry, &["cdn.openttd.org".to_string()]).is_none());
    }

    #[test]
    fn a_posix_path_maps_onto_the_wine_root_drive() {
        assert_eq!(
            windows_path(Path::new("/Users/tester/Games/OpenTTD")).as_deref(),
            Some(r"Z:\Users\tester\Games\OpenTTD")
        );
        assert_eq!(windows_path(Path::new("relative/path")), None);
    }

    /// End-to-end over the real plugin package that ships in this repository:
    /// manifest validation, `installer` extension discovery and catalogue
    /// resolution all have to agree before a title can ever be offered.
    #[test]
    fn the_shipped_plugin_package_is_discovered_and_its_catalogue_resolves() {
        let Some(plugins_root) = shipped_plugin_root() else {
            // The plugin is a separate project; a checkout without its built
            // output should not fail the host's own suite.
            return;
        };
        let runtime = PluginRuntime::new().expect("runtime");
        let plugin = PluginRegistry::new(plugins_root, HostCompatibility::v1("0.3.0"))
            .installer_plugin(&runtime)
            .expect("the installer plugin is discovered");

        assert_eq!(plugin.id, "com.orivo.quiky");
        assert_eq!(plugin.state, PluginState::Ready, "{}", plugin.message);
        assert!(
            plugin
                .network_domains
                .contains(&"cdn.openttd.org".to_string())
        );

        let titles = load_catalog(&plugin);
        assert!(
            !titles.is_empty(),
            "the catalogue resolves at least one title"
        );
        let openttd = titles
            .iter()
            .find(|title| title.slug == "openttd")
            .expect("openttd is offered");
        assert_eq!(openttd.kind, InstallerKind::Nsis);
        assert_eq!(openttd.folder_name, "OpenTTD");
        assert!(openttd.url.starts_with("https://cdn.openttd.org/"));
    }

    /// Exercises the real network path end to end against the catalogue entry
    /// that ships with the plugin: redirect allowlisting, streaming, progress,
    /// the exact byte count and the SHA-256 gate.
    #[test]
    fn a_catalogued_package_downloads_and_verifies() {
        let Some(plugins_root) = shipped_plugin_root() else {
            return;
        };
        let runtime = PluginRuntime::new().expect("runtime");
        let plugin = PluginRegistry::new(plugins_root, HostCompatibility::v1("0.3.0"))
            .installer_plugin(&runtime)
            .expect("plugin");
        let title = load_catalog(&plugin)
            .into_iter()
            .find(|title| title.slug == "openttd")
            .expect("openttd");

        let temporary = std::env::temp_dir()
            .join(format!("orivo-quiky-download-{}", std::process::id()))
            .join("setup.exe.part");
        let cancelled = AtomicBool::new(false);
        let mut ticks = 0_usize;
        let outcome = tauri::async_runtime::block_on(fetch_verified_package(
            &title.url,
            &title.sha256,
            title.download_bytes,
            &plugin.network_domains,
            &temporary,
            &cancelled,
            &mut |written| {
                assert!(written <= title.download_bytes);
                ticks += 1;
            },
        ));
        assert_eq!(outcome, Ok(()), "the catalogued package downloads");
        assert!(ticks > 0, "progress is reported while the package streams");
        assert_eq!(
            fs::metadata(&temporary).expect("package").len(),
            title.download_bytes
        );
        let _ = fs::remove_dir_all(temporary.parent().expect("directory"));
    }

    /// A tampered digest must stop the package before it can ever be run.
    #[test]
    fn a_package_that_fails_its_digest_is_discarded() {
        let temporary = std::env::temp_dir()
            .join(format!("orivo-quiky-digest-{}", std::process::id()))
            .join("setup.exe.part");
        let cancelled = AtomicBool::new(false);
        let outcome = tauri::async_runtime::block_on(fetch_verified_package(
            "https://cdn.openttd.org/openttd-releases/15.3/openttd-15.3-windows-win64.exe",
            &"0".repeat(64),
            8_916_160,
            &["cdn.openttd.org".to_string()],
            &temporary,
            &cancelled,
            &mut |_| {},
        ));
        assert_eq!(
            outcome,
            Err("The package failed its integrity check.".into())
        );
        assert!(!temporary.exists(), "a rejected package is removed");
        let _ = fs::remove_dir_all(temporary.parent().expect("directory"));
    }

    #[test]
    fn a_url_outside_the_allowlist_is_never_fetched() {
        let cancelled = AtomicBool::new(false);
        let outcome = tauri::async_runtime::block_on(fetch_verified_package(
            "https://attacker.example/setup.exe",
            &"0".repeat(64),
            10,
            &["cdn.openttd.org".to_string()],
            Path::new("/dev/null"),
            &cancelled,
            &mut |_| {},
        ));
        assert_eq!(
            outcome,
            Err("This download is outside the plugin's declared domains.".into())
        );
    }

    #[test]
    fn free_space_is_measured_on_the_volume_that_will_hold_the_game() {
        // A real directory reports a real figure, and a folder that does not
        // exist yet resolves through its first existing ancestor rather than
        // reporting zero and blocking every install.
        assert!(free_disk_bytes(Path::new("/")) > 0);
        let unborn = std::env::temp_dir().join("orivo-not-created-yet/deeper/still");
        assert!(free_disk_bytes(&unborn) > 0);
    }

    #[test]
    fn the_log_keeps_the_most_recent_lines_for_a_post_mortem() {
        let root = std::env::temp_dir().join(format!("orivo-quiky-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let service = QuikyService::new(
            root.join("plugins"),
            root.join("prefixes"),
            root.clone(),
            root.join("games"),
            "0.3.0",
        );
        assert!(tail_log(&service, 10).is_empty(), "no log yet");
        for index in 0..5 {
            append_log(&service, &format!("line-{index}"));
        }
        let tail = tail_log(&service, 3);
        assert_eq!(tail.len(), 3);
        assert!(tail[2].ends_with("line-4"), "{tail:?}");
        assert!(tail[0].ends_with("line-2"), "{tail:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn percentages_stay_inside_the_bar() {
        assert_eq!(percentage(0, 100), 0);
        assert_eq!(percentage(50, 100), 50);
        assert_eq!(percentage(500, 100), 100);
        assert_eq!(percentage(5, 0), 0);
    }
}
