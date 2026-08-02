mod catalog;
mod game_detail;
mod game_media;
mod launcher;
mod plugin_manifest;
mod plugin_registry;
mod plugin_runtime;
mod preferences;
mod steam;
mod steam_account;
mod store;
mod wallpaper_credentials;
mod wallpaper_search;
mod wine_runner;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use catalog::{
    Catalog, CatalogError, Game, GameSource, LaunchTarget, WINE_STAGING_RUNNER_ID,
    WineGameCompatibility, WineGameInventoryEntry, WineGraphicsBackend, WineGraphicsOptions,
    WineProfile,
};
use futures_util::StreamExt;
use game_detail::{
    GameDetailError, GameDetailService, GameDetailView, WishlistMutationView, media_source_url,
};
use plugin_manifest::HostCompatibility;
use plugin_registry::{PLUGINS_DIRECTORY, PluginRegistry, RunnerPluginView};
use plugin_runtime::PluginRuntime;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{
    AppHandle, Emitter, Manager, State, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
    webview::{NewWindowResponse, PageLoadEvent},
};

const CATALOG_FILE: &str = "catalog.json";
/// Enough distinct backups of one source schema to survive repeated recovery
/// attempts without letting a loop fill the data directory.
const MAX_CATALOG_BACKUPS_PER_SCHEMA: u32 = 32;
const MEDIA_DIRECTORY: &str = "media";
/// Wishlist and media selections live outside `catalog.json`: they are user
/// state, not owned-library facts, and must survive a catalog migration.
const GAME_STATE_FILE: &str = "game-state.json";
/// An open-ended range request never streams a whole video into memory. The
/// WebView simply asks for the next window when it needs more.
const MAX_MEDIA_RANGE_CHUNK_BYTES: u64 = 8 * 1_024 * 1_024;
const MEDIA_MAGIC_PROBE_BYTES: usize = 16;
const WINE_PREFIXES_DIRECTORY: &str = "wine-prefixes";
/// The single Orivo-managed Wine profile that every local Windows `.exe` game
/// is associated with automatically. It is provisioned without the setup
/// wizard so users never have to add a game via Wine by hand. Its id is a
/// fixed opaque token so the profile — and its host-owned prefix — is found
/// and reused across restarts instead of being recreated.
const AUTO_WINE_PROFILE_ID: &str = "orivo-auto-wine";
const AUTO_WINE_PROFILE_NAME: &str = "Jeux Windows (.exe)";
const MAX_WINE_SETUP_SESSIONS: usize = 12;
const MAX_WINE_SCAN_JOBS: usize = 12;
const MAX_WINE_IMPORT_SELECTION: usize = 2_000;
const MAX_STEAM_IMPORT_SELECTION: usize = 2_000;
const MAX_STEAM_PREVIEW_MEDIA: usize = 16;
const STEAM_PREVIEW_SNAPSHOT_TTL: Duration = Duration::from_secs(30);
// Store copy improves a library entry, but must never make a large first sync
// wait for every public app-details request. Missing entries are retried on a
// later sync because only successfully enriched records receive the marker.
const STEAM_STORE_METADATA_SYNC_BUDGET: Duration = Duration::from_secs(8);
const MAX_MEDIA_FILE_BYTES: u64 = 20 * 1_024 * 1_024;
const MAX_MEDIA_CACHE_BYTES_PER_OPERATION: u64 = 128 * 1_024 * 1_024;
const STEAM_AUTH_WINDOW_LABEL: &str = "steam-auth";
/// Every application event is addressed to this one window. A broadcast would
/// also reach the capability-free Steam sign-in WebView, which has no business
/// seeing an account identifier or a launch status.
const MAIN_WINDOW_LABEL: &str = "main";
const STEAM_EXPLORE_URL: &str = "https://store.steampowered.com/explore/";
const STEAM_ACCOUNT_CONNECTED_EVENT: &str = "steam-account-authenticated";
const STEAM_ACCOUNT_LOGIN_CANCELLED_EVENT: &str = "steam-account-login-cancelled";
const STEAM_ACCOUNT_LOGIN_FAILED_EVENT: &str = "steam-account-login-failed";
const STEAM_ACCOUNT_LOGIN_PENDING_EVENT: &str = "steam-account-login-pending";
const WINE_LAUNCH_STATUS_EVENT: &str = "wine-launch-status";
const WINE_EARLY_EXIT_WINDOW: Duration = Duration::from_secs(8);
static MEDIA_CACHE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
/// The detail projection is derived state built once during setup. Keeping the
/// handle here lets every catalog write refresh it through one choke point,
/// including the background workers that only own catalog handles.
static DETAIL_PROJECTION: OnceLock<Arc<GameDetailService>> = OnceLock::new();

// This runs only in the dedicated, capability-free Steam authentication
// window. It returns two short strings rather than the page HTML, and the
// callback keeps both inside Rust so the main WebView never receives a token.
const STEAM_LOGIN_EXTRACTION_SCRIPT: &str = r#"
(() => {
  const context = window.g_rgAppContextData || {};
  let steamId = typeof context.steamid === 'string' ? context.steamid : '';
  let accessToken = typeof context.webapi_token === 'string' ? context.webapi_token : '';
  if (!steamId || !accessToken) {
    const html = document.documentElement ? document.documentElement.innerHTML : '';
    const read = (name) => {
      const expression = new RegExp('(?:&quot;|\\")' + name + '(?:&quot;|\\")\\s*:\\s*(?:&quot;|\\")([^&\\"<]+)');
      const match = expression.exec(html);
      return match ? match[1] : '';
    };
    steamId = steamId || read('steamid');
    accessToken = accessToken || read('webapi_token');
  }
  return JSON.stringify({ steamId, accessToken });
})()
"#;

/// The backend deliberately owns every executable path and launch argument.
/// The WebView only ever sees presentation data and can ask to launch a stable
/// game id, never an arbitrary system command.
struct AppState {
    catalog_path: PathBuf,
    /// A host-owned root for every Wine prefix. A profile stores only a child
    /// generated by Orivo, never a prefix supplied by a WebView or plugin.
    wine_prefix_root: PathBuf,
    /// The directory is only read when the user opens an extension surface.
    /// Startup never scans or compiles third-party plugin contents.
    plugin_root: PathBuf,
    /// One bounded discovery worker is enough. Repeated UI clicks must not
    /// compile the same third-party components concurrently.
    plugin_discovery_in_flight: Arc<AtomicBool>,
    catalog: Arc<RwLock<Catalog>>,
    /// Serializes catalog mutations without forcing readers (launch and rail
    /// rendering) to wait for an atomic disk write.
    catalog_mutation: Arc<Mutex<()>>,
    /// A short-lived Rust-only discovery snapshot avoids immediately parsing
    /// every manifest a second time just to hydrate the first preview images.
    steam_preview: Mutex<Option<SteamPreviewSnapshot>>,
    /// The active dedicated Steam sign-in window. It only tracks whether the
    /// one-time token extraction was consumed; no credential is held here.
    steam_auth_settled: Mutex<Option<Arc<AtomicBool>>>,
    /// Ephemeral setup grants hold native-picker selections until a profile is
    /// explicitly created. The WebView sees only opaque ids and labels.
    wine_setups: Arc<Mutex<BTreeMap<String, WineSetupSession>>>,
    /// Long-running scans are owned by Rust and polled through safe, bounded
    /// view models. Cancelling a job never touches the persistent library.
    wine_scan_jobs: Mutex<BTreeMap<String, Arc<WineScanJob>>>,
    wine_operation_sequence: AtomicU64,
}

#[derive(Debug, Clone)]
struct SteamPreviewSnapshot {
    captured_at: Instant,
    games: BTreeMap<u32, steam::SteamGame>,
}

/// Native-picker values that are intentionally short lived. Neither field is
/// ever serialised for the WebView; a setup id is the only browser-visible
/// handle until the user confirms profile creation.
#[derive(Debug, Clone)]
struct WineSetupSession {
    /// Only a binary that has passed the explicit, user-confirmed probe is
    /// eligible for profile creation.
    wine_binary: Option<PathBuf>,
    /// Discovery can identify a conventional candidate without running it.
    /// It stays separate until the user asks the host to validate it.
    detected_wine_binary: Option<PathBuf>,
    game_directories: BTreeMap<String, PathBuf>,
    detection: Arc<WineDetectionJob>,
}

impl WineSetupSession {
    fn new() -> Self {
        Self {
            wine_binary: None,
            detected_wine_binary: None,
            game_directories: BTreeMap::new(),
            detection: Arc::new(WineDetectionJob {
                cancelled: Arc::new(AtomicBool::new(false)),
                state: Mutex::new(WineDetectionState {
                    phase: WineDetectionPhase::Detecting,
                    message: "Looking for a Wine-Staging installation…".into(),
                }),
            }),
        }
    }
}

#[derive(Debug)]
struct WineDetectionJob {
    cancelled: Arc<AtomicBool>,
    state: Mutex<WineDetectionState>,
}

#[derive(Debug, Clone)]
struct WineDetectionState {
    phase: WineDetectionPhase,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WineDetectionPhase {
    Detecting,
    Ready,
    Unavailable,
    Cancelled,
    Failed,
}

#[derive(Debug)]
struct WineScanJob {
    profile_id: String,
    cancelled: Arc<AtomicBool>,
    state: Mutex<WineScanJobState>,
}

#[derive(Debug, Clone)]
struct WineScanJobState {
    phase: WineScanPhase,
    scanned_files: usize,
    candidates: Vec<wine_runner::ScannedWineGame>,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WineScanPhase {
    Scanning,
    Ready,
    Cancelled,
    Failed,
}

struct PluginDiscoveryLease(Arc<AtomicBool>);

impl Drop for PluginDiscoveryLease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameView {
    id: String,
    title: String,
    description: String,
    metadata: String,
    genre: String,
    source: String,
    hero_url: Option<String>,
    cover_url: Option<String>,
    landscape_url: Option<String>,
    last_played_at: String,
    play_time_seconds: u64,
    launchable: bool,
    host_platform: String,
    supported_platforms: Vec<String>,
    compatible_with_host: Option<bool>,
    /// A local Windows executable is not dispatched through macOS's generic
    /// direct launcher. The UI can instead offer a profile-scoped Wine
    /// association using only this existing catalog id.
    wine_attachable: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryState {
    games: Vec<GameView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportResponse {
    games: Vec<GameView>,
    imported_id: Option<String>,
}

/// Presentation-only data for Steam's local discovery. Paths stay in Rust:
/// users see a source and an installed game, never a private filesystem path.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SteamImportPreview {
    status: &'static str,
    libraries: usize,
    games: Vec<SteamPreviewGame>,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SteamPreviewGame {
    app_id: String,
    title: String,
    location_label: &'static str,
    last_updated: String,
    selected: bool,
    already_imported: bool,
    cover_url: Option<String>,
    hero_url: Option<String>,
}

/// Cache paths produced for a small, bounded progressive preview window.
/// Only catalog references are persisted on a confirmed import; orphaned
/// cache files remain opaque and harmless to the WebView.
#[derive(Debug, Clone, Default)]
struct SteamPreviewMedia {
    cover_path: Option<PathBuf>,
    hero_path: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SteamPreviewMediaView {
    app_id: String,
    cover_url: Option<String>,
    hero_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SteamImportResponse {
    imported_ids: Vec<String>,
    updated_ids: Vec<String>,
    skipped_app_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SteamAccountSyncResponse {
    total_games: usize,
    imported_games: usize,
    updated_games: usize,
    installed_games: usize,
}

#[derive(Debug, Serialize)]
struct LaunchResult {
    status: String,
}

/// A Wine process may fail only after it has been created (for example while
/// the runtime loads D3D). The host watches a short startup window and sends
/// a path-free lifecycle update so the UI can offer Retry without waiting for
/// the game process or blocking navigation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WineLaunchStatusEvent {
    game_id: String,
    phase: &'static str,
    message: String,
}

/// Public state for the built-in runner. This deliberately has no plugin
/// package path, Wine path, prefix path, or directory grant in it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WineRunnerStatusView {
    state: &'static str,
    available: bool,
    message: String,
    version: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WineSetupView {
    setup_id: String,
    wine_label: Option<String>,
    detected_wine_label: Option<String>,
    detection_state: &'static str,
    detection_message: String,
    directories: Vec<WineDirectoryView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WineDirectoryView {
    id: String,
    label: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WineProfileView {
    id: String,
    display_name: String,
    enabled: bool,
    wine_label: String,
    directories: Vec<WineDirectoryView>,
    last_import_at: Option<u64>,
    last_import_summary: String,
    graphics_backend: &'static str,
    graphics_summary: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WineSettingsView {
    runner: WineRunnerStatusView,
    profiles: Vec<WineProfileView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WineProfileCreatedView {
    profile_id: String,
    profile: WineProfileView,
}

/// The association creates a separate, opaque Runner card. Returning only
/// that id lets the WebView retain its selection without ever deriving or
/// receiving the executable path behind it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WineAssociationView {
    game_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WineScanJobView {
    job_id: String,
    profile_id: String,
    state: &'static str,
    scanned_files: usize,
    found_games: usize,
    message: String,
    complete: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WineScanPageView {
    games: Vec<WineScanGameView>,
    next_cursor: Option<String>,
    complete: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WineScanGameView {
    game_ref: String,
    title: String,
    directory_label: String,
    already_imported: bool,
    launchable: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WineImportResponse {
    imported_ids: Vec<String>,
    updated_ids: Vec<String>,
    skipped_refs: Vec<String>,
}

impl AppState {
    fn load(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let catalog_path = resolved_catalog_path(app)?;
        let app_data = app.path().app_data_dir()?;
        let plugin_root = app_data.join(PLUGINS_DIRECTORY);
        let wine_prefix_root = app_data.join(WINE_PREFIXES_DIRECTORY);
        // The game-state document is re-keyed with the catalog it belongs to,
        // before `GameStateStore` reads it during setup.
        let mut catalog = load_or_migrate_catalog(&catalog_path, &app_data.join(GAME_STATE_FILE))?;

        // Imported artwork is copied once into the app cache. That keeps the
        // browser's asset protocol tightly scoped and means a moved source
        // folder cannot break the selected game's visual state.
        if hydrate_catalog_media(app, &mut catalog)? {
            catalog.save_atomically(&catalog_path)?;
        }

        // Bring every local Windows .exe under the managed default Wine profile
        // so it launches through Wine-Staging without a manual setup step. This
        // is a no-op off macOS and on machines without a detected Wine-Staging
        // installation, so the first paint is never blocked waiting for Wine.
        if auto_apply_wine_to_direct_games(&mut catalog, &wine_prefix_root) {
            catalog.save_atomically(&catalog_path)?;
        }

        Ok(Self {
            catalog_path,
            wine_prefix_root,
            plugin_root,
            plugin_discovery_in_flight: Arc::new(AtomicBool::new(false)),
            catalog: Arc::new(RwLock::new(catalog)),
            catalog_mutation: Arc::new(Mutex::new(())),
            steam_preview: Mutex::new(None),
            steam_auth_settled: Mutex::new(None),
            wine_setups: Arc::new(Mutex::new(BTreeMap::new())),
            wine_scan_jobs: Mutex::new(BTreeMap::new()),
            wine_operation_sequence: AtomicU64::new(0),
        })
    }
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let state = AppState::load(app.handle())?;
            let app_data = app.path().app_data_dir()?;

            // The detail projection is derived from the catalog that was just
            // loaded. Reading `catalog.json` a second time could observe a
            // different file and make the Library and the detail page disagree.
            let game_state = Arc::new(game_detail::GameStateStore::load(
                app_data.join(GAME_STATE_FILE),
            )?);
            // The same media cache the Library resolves against. Both sides
            // call one resolver, so handing it the same directory is what makes
            // imported artwork resolve to the same URL on both pages.
            let detail = Arc::new(
                GameDetailService::new(game_state)
                    .with_media_cache_dir(media_cache_dir(app.handle()).ok()),
            );
            {
                let catalog = state
                    .catalog
                    .read()
                    .map_err(|_| "the game catalog is temporarily unavailable")?;
                project_presentation_catalog(&detail, &catalog)?;
            }

            // Media resolution must go through the same service instance the
            // detail commands read, otherwise an imported asset would not be
            // visible to the page that asked for it.
            let media = game_media::GameMediaService::new(
                Arc::clone(&detail),
                app_data.join(game_media::MEDIA_DIRECTORY),
            )?;

            // Two live services would let one instance answer the WebView while
            // catalog writes refresh the other, so a second setup is a hard
            // startup failure rather than a silently divergent projection.
            DETAIL_PROJECTION
                .set(Arc::clone(&detail))
                .map_err(|_| "the game detail projection is already initialised")?;
            app.manage(state);
            app.manage(detail);
            app.manage(media);
            // The same credential store backs both the Settings commands and
            // the wallpaper search, so a saved key is used without a restart.
            let wallpaper_credentials = Arc::new(wallpaper_credentials::WallpaperCredentialsService::load(
                app_data.join(wallpaper_credentials::CREDENTIALS_FILE),
            ));
            app.manage(Arc::clone(&wallpaper_credentials));
            app.manage(wallpaper_search::WallpaperSearchService::new(wallpaper_credentials));
            Ok(())
        })
        // Media files stay behind a host-owned scheme instead of a broad
        // filesystem grant: the WebView can only name an opaque file inside
        // the app's own media directory.
        .register_uri_scheme_protocol(game_media::GAME_MEDIA_URI_SCHEME, |context, request| {
            game_media_scheme_response(context.app_handle(), &request)
        })
        .invoke_handler(tauri::generate_handler![
            get_library,
            import_game,
            fetch_game_artwork,
            remove_game,
            set_home_image,
            get_steam_import_preview,
            get_steam_preview_media,
            import_steam_games,
            get_steam_account_status,
            begin_steam_web_login,
            complete_steam_web_login,
            cancel_steam_web_login,
            connect_steam_with_api_key,
            sync_steam_account_library,
            disconnect_steam_account,
            get_runner_plugins,
            get_wine_runner_status,
            begin_wine_profile_setup,
            get_wine_profile_setup,
            cancel_wine_detection,
            confirm_detected_wine_staging,
            select_wine_staging,
            choose_wine_game_directory,
            remove_wine_setup_directory,
            create_wine_profile,
            start_wine_profile_scan,
            get_wine_scan_status,
            get_wine_scan_page,
            cancel_wine_scan,
            import_wine_games,
            associate_direct_game_with_wine_profile,
            install_dxvk_macos_for_profile,
            use_wine_3d_for_profile,
            retry_wine_game_in_compatibility,
            get_wine_runner_settings,
            set_wine_profile_enabled,
            delete_wine_profile,
            launch_game,
            install_steam_game,
            get_game_detail,
            set_game_wishlist,
            store::get_store_home,
            store::browse_store_games,
            store::refresh_store_sources,
            store::open_store_offer,
            game_media::select_game_media,
            game_media::export_game_media,
            game_media::import_game_media,
            game_media::cancel_game_media_download,
            wallpaper_search::search_wallpapers,
            wallpaper_search::import_wallpaper_candidate,
            wallpaper_credentials::get_wallpaper_credentials,
            wallpaper_credentials::update_wallpaper_credentials,
            preferences::get_preferences,
            preferences::update_preferences,
            preferences::get_data_usage,
            preferences::clear_derived_cache
        ])
        .run(tauri::generate_context!())
        .expect("error while running Orivo");
}

/// `GameDetailError` carries IO and parse detail that must never reach a
/// WebView. Every detail command answers with one short, actionable sentence.
fn game_detail_message(error: GameDetailError) -> String {
    match error {
        GameDetailError::NotFound => "This game is no longer in your library.".into(),
        GameDetailError::Invalid(_) => "Orivo could not read this game request.".into(),
        GameDetailError::Io(_) | GameDetailError::Json(_) => {
            "Orivo could not save this game's state. Try again.".into()
        }
        GameDetailError::Unavailable(_) => "Game details are temporarily unavailable.".into(),
    }
}

#[tauri::command]
fn get_game_detail(
    game_id: String,
    detail: State<'_, Arc<GameDetailService>>,
) -> Result<Option<GameDetailView>, String> {
    game_detail::get_game_detail(detail.inner(), game_id).map_err(game_detail_message)
}

#[tauri::command]
fn set_game_wishlist(
    game_id: String,
    wishlisted: bool,
    detail: State<'_, Arc<GameDetailService>>,
) -> Result<WishlistMutationView, String> {
    game_detail::set_game_wishlist(detail.inner(), game_id, wishlisted).map_err(game_detail_message)
}

/// The single catalog write path. Persisting and republishing the derived
/// detail projection together is what keeps the detail page and the Store from
/// serving a snapshot the Library has already replaced.
fn persist_catalog(catalog: &Catalog, path: &Path) -> Result<(), CatalogError> {
    catalog.save_atomically(path)?;
    refresh_detail_projection(catalog);
    Ok(())
}

/// A stale projection must never fail a mutation that is already durable on
/// disk: the next successful write republishes the same data.
fn refresh_detail_projection(catalog: &Catalog) {
    if let Some(detail) = DETAIL_PROJECTION.get()
        && project_presentation_catalog(detail, catalog).is_err()
    {
        eprintln!("orivo: the game detail projection could not be refreshed");
    }
}

/// The detail page must resolve exactly what the Library renders. The Library
/// paints `presentation_catalog`, so projecting the raw stored catalog would
/// leave every showcase card — the whole library on a fresh profile — pointing
/// at a game the detail service has never heard of.
fn project_presentation_catalog(
    detail: &GameDetailService,
    stored_catalog: &Catalog,
) -> Result<(), GameDetailError> {
    // The detail projection is a reachability superset: it always includes the
    // showcase games so that, when the debug toggle surfaces them, their detail
    // pages resolve. The Library alone decides whether they are shown.
    let presentation = presentation_catalog(stored_catalog, true);
    match detail.replace_catalog(&presentation) {
        Ok(()) => Ok(()),
        // One record the detail service refuses must not freeze the projection
        // at its startup contents: drop that record and keep the rest current.
        Err(_) => detail.replace_catalog(&projectable_catalog(detail, &presentation)),
    }
}

fn projectable_catalog(detail: &GameDetailService, presentation: &Catalog) -> Catalog {
    let mut template = presentation.clone();
    template.games.clear();
    let mut projectable = template.clone();
    for game in &presentation.games {
        let mut candidate = template.clone();
        candidate.games = vec![game.clone()];
        if GameDetailService::from_catalog(
            &candidate,
            Arc::clone(detail.state()),
            detail.media_cache_dir().map(Path::to_path_buf),
        )
        .is_ok()
        {
            projectable.games.push(game.clone());
        } else {
            // `{:?}` so a hostile identifier cannot forge log lines.
            eprintln!(
                "orivo: game {:?} was skipped by the detail projection",
                game.id
            );
        }
    }
    projectable
}

/// Resolve the one opaque file name a `game-media:` request may address.
/// Everything else — nested paths, absolute paths, traversal, percent escapes —
/// fails the opaque-name check and is answered with 404.
fn game_media_requested_file(uri: &str) -> Option<&str> {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    // `game-media:<name>` carries no authority, so it arrives whole; the
    // `//authority/<name>` forms come from the platforms that rewrite a custom
    // scheme onto an http origin.
    let (_, rest) = path.split_once(':')?;
    let rest = match rest.strip_prefix("//") {
        Some(authority_and_path) => authority_and_path.split_once('/')?.1,
        None => rest,
    };
    // Exactly one opaque segment. A request that tries to describe a path at
    // all is refused rather than reinterpreted.
    if rest.contains('/') {
        return None;
    }
    game_detail::valid_opaque_file_name(rest).then_some(rest)
}

/// Parse a single byte range. Multi-range requests and unusable specs return
/// `None`; the caller decides between a full body and 416.
fn parse_media_byte_range(header_value: &str, length: u64) -> Option<(u64, u64)> {
    let spec = header_value.trim().strip_prefix("bytes=")?.trim();
    if spec.contains(',') || length == 0 {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", "") => return None,
        // A suffix range asks for the final N bytes.
        ("", suffix) => {
            let suffix = suffix.parse::<u64>().ok()?.min(length);
            if suffix == 0 {
                return None;
            }
            (length - suffix, length - 1)
        }
        (start, "") => (start.parse::<u64>().ok()?, length - 1),
        (start, end) => (start.parse::<u64>().ok()?, end.parse::<u64>().ok()?),
    };
    if start > end || start >= length {
        return None;
    }
    let end = end.min(length - 1);
    // Serving fewer bytes than requested is allowed and keeps a 250 MB video
    // from being buffered in one response.
    let end = end.min(start.saturating_add(MAX_MEDIA_RANGE_CHUNK_BYTES - 1));
    Some((start, end))
}

fn read_media_slice(path: &Path, start: u64, length: u64) -> Option<Vec<u8>> {
    let mut file = fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut body = vec![0_u8; usize::try_from(length).ok()?];
    file.read_exact(&mut body).ok()?;
    Some(body)
}

/// The format is sniffed from the file itself. A stored file name is never
/// allowed to declare what the WebView should treat the bytes as, nor how much
/// of them may be answered at once.
fn media_format(path: &Path) -> Option<game_media::MediaFormat> {
    let mut head = [0_u8; MEDIA_MAGIC_PROBE_BYTES];
    let read = fs::File::open(path)
        .and_then(|mut file| file.read(&mut head))
        .unwrap_or(0);
    game_media::MediaFormat::from_magic(&head[..read])
}

fn media_content_type(format: Option<game_media::MediaFormat>) -> &'static str {
    format
        .map(game_media::MediaFormat::mime)
        .unwrap_or("application/octet-stream")
}

/// How much of a file a request without a `Range` header may be answered with.
///
/// Images are served whole: they are already bounded by the import cap, and an
/// unrequested 206 is what a WebView renders as a broken `<img>`. Only video —
/// and anything whose format could not be established — keeps the bounded
/// window, because reading a 250 MB file into one `Vec` on the protocol thread
/// is never worth it and a `<video>` asks for the rest with a `Range`.
fn unranged_media_chunk(format: Option<game_media::MediaFormat>, length: u64) -> u64 {
    let limit = match format {
        Some(game_media::MediaFormat::Mp4) | None => MAX_MEDIA_RANGE_CHUNK_BYTES,
        Some(_) => game_media::MAX_IMAGE_BYTES.max(MAX_MEDIA_RANGE_CHUNK_BYTES),
    };
    limit.min(length)
}

fn media_scheme_error(status: tauri::http::StatusCode) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(status)
        .header(tauri::http::header::CONTENT_TYPE, "text/plain")
        .body(Vec::new())
        .unwrap_or_default()
}

fn game_media_scheme_response(
    app: &AppHandle,
    request: &tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let Ok(app_data) = app.path().app_data_dir() else {
        return media_scheme_error(tauri::http::StatusCode::NOT_FOUND);
    };
    game_media_response(&app_data.join(game_media::MEDIA_DIRECTORY), request)
}

fn game_media_response(
    media_dir: &Path,
    request: &tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let uri = request.uri().to_string();
    let Some(file_name) = game_media_requested_file(&uri) else {
        return media_scheme_error(tauri::http::StatusCode::NOT_FOUND);
    };
    let path = media_dir.join(file_name);
    // A symlink planted under an opaque-looking name must never be followed:
    // the scheme only ever serves a regular file this app wrote itself.
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return media_scheme_error(tauri::http::StatusCode::NOT_FOUND);
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return media_scheme_error(tauri::http::StatusCode::NOT_FOUND);
    }

    let length = metadata.len();
    let format = media_format(&path);
    let content_type = media_content_type(format);
    let requested_range = request
        .headers()
        .get(tauri::http::header::RANGE)
        .and_then(|value| value.to_str().ok());

    let Some(range) = requested_range else {
        // An image is answered whole with a 200; video keeps the bounded window
        // and lets the element ask for the rest.
        let chunk = unranged_media_chunk(format, length);
        let Some(body) = read_media_slice(&path, 0, chunk) else {
            return media_scheme_error(tauri::http::StatusCode::NOT_FOUND);
        };
        if chunk == length {
            return tauri::http::Response::builder()
                .status(tauri::http::StatusCode::OK)
                .header(tauri::http::header::CONTENT_TYPE, content_type)
                .header(tauri::http::header::ACCEPT_RANGES, "bytes")
                .header(tauri::http::header::CONTENT_LENGTH, length)
                .body(body)
                .unwrap_or_else(|_| media_scheme_error(tauri::http::StatusCode::NOT_FOUND));
        }
        return tauri::http::Response::builder()
            .status(tauri::http::StatusCode::PARTIAL_CONTENT)
            .header(tauri::http::header::CONTENT_TYPE, content_type)
            .header(tauri::http::header::ACCEPT_RANGES, "bytes")
            .header(
                tauri::http::header::CONTENT_RANGE,
                format!("bytes 0-{}/{length}", chunk - 1),
            )
            .header(tauri::http::header::CONTENT_LENGTH, body.len())
            .body(body)
            .unwrap_or_else(|_| media_scheme_error(tauri::http::StatusCode::NOT_FOUND));
    };

    // `<video>` seeks with ranges. An unsatisfiable range gets 416 with the
    // real size so the element can retry instead of failing playback.
    let Some((start, end)) = parse_media_byte_range(range, length) else {
        return tauri::http::Response::builder()
            .status(tauri::http::StatusCode::RANGE_NOT_SATISFIABLE)
            .header(
                tauri::http::header::CONTENT_RANGE,
                format!("bytes */{length}"),
            )
            .header(tauri::http::header::ACCEPT_RANGES, "bytes")
            .body(Vec::new())
            .unwrap_or_else(|_| media_scheme_error(tauri::http::StatusCode::NOT_FOUND));
    };
    let Some(body) = read_media_slice(&path, start, end - start + 1) else {
        return media_scheme_error(tauri::http::StatusCode::NOT_FOUND);
    };
    tauri::http::Response::builder()
        .status(tauri::http::StatusCode::PARTIAL_CONTENT)
        .header(tauri::http::header::CONTENT_TYPE, content_type)
        .header(tauri::http::header::ACCEPT_RANGES, "bytes")
        .header(
            tauri::http::header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{length}"),
        )
        .header(tauri::http::header::CONTENT_LENGTH, body.len())
        .body(body)
        .unwrap_or_else(|_| media_scheme_error(tauri::http::StatusCode::NOT_FOUND))
}

/// Plugins are discovered only after a user explicitly opens the emulator
/// flow. Filesystem parsing and component hashing stay off the WebView/UI
/// executor, and malformed components degrade into a per-plugin state.
#[tauri::command]
async fn get_runner_plugins(state: State<'_, AppState>) -> Result<Vec<RunnerPluginView>, String> {
    let lease_flag = Arc::clone(&state.plugin_discovery_in_flight);
    if lease_flag
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("Plugin discovery is already in progress.".into());
    }
    let root = state.plugin_root.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _lease = PluginDiscoveryLease(lease_flag);
        let runtime = PluginRuntime::new().map_err(|error| error.to_string())?;
        Ok::<_, String>(
            PluginRegistry::new(root, HostCompatibility::v1(env!("CARGO_PKG_VERSION")))
                .runner_plugins(&runtime),
        )
    })
    .await
    .map_err(|error| format!("Plugin discovery did not finish: {error}"))?
}

fn wine_runner_status_view() -> WineRunnerStatusView {
    if cfg!(target_os = "macos") {
        WineRunnerStatusView {
            state: "ready",
            available: true,
            message: "Built-in Wine-Staging runner. Select a Wine-Staging installation to create a profile."
                .into(),
            version: "built-in",
        }
    } else {
        WineRunnerStatusView {
            state: "unavailable",
            available: false,
            message: "Wine-Staging profiles are available on macOS only.".into(),
            version: "built-in",
        }
    }
}

fn require_wine_runner_platform() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err("Wine-Staging profiles are available on macOS only.".into())
    }
}

/// Whether Orivo is running on an Apple Silicon (M-series) Mac. On such
/// machines the D3D10/11 → Metal path (DXVK-macOS) is the default graphics
/// backend so the user never enables it by hand.
///
/// This reads the `hw.optional.arm64` capability rather than the compile-time
/// architecture, so it stays correct even for an x86_64 Orivo build translated
/// by Rosetta on an M-series Mac. The result never crosses the WebView
/// boundary; it only selects a value from the closed host-owned graphics enum.
#[cfg(target_os = "macos")]
fn macos_is_apple_silicon() -> bool {
    use std::sync::OnceLock;
    static APPLE_SILICON: OnceLock<bool> = OnceLock::new();
    *APPLE_SILICON.get_or_init(|| {
        let mut value: i64 = 0;
        let mut size = std::mem::size_of::<i64>();
        // SAFETY: `sysctlbyname` only writes up to `size` bytes into `value`
        // and updates `size`; the name is a fixed NUL-terminated string.
        let result = unsafe {
            libc::sysctlbyname(
                b"hw.optional.arm64\0".as_ptr() as *const libc::c_char,
                &mut value as *mut i64 as *mut libc::c_void,
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        result == 0 && value == 1
    })
}

#[cfg(not(target_os = "macos"))]
fn macos_is_apple_silicon() -> bool {
    false
}

fn next_wine_opaque_id(state: &AppState, kind: &str) -> String {
    let sequence = state
        .wine_operation_sequence
        .fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    digest.update(nanos.to_le_bytes());
    digest.update(sequence.to_le_bytes());
    digest.update(std::process::id().to_le_bytes());
    let hash = format!("{:x}", digest.finalize());
    format!("{kind}-{}", &hash[..24])
}

fn valid_wine_opaque_id(value: &str) -> bool {
    value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

fn safe_wine_label(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
        .map(|name| name.chars().take(96).collect())
        .unwrap_or_else(|| fallback.into())
}

fn wine_directory_view_id(profile_or_setup: &str, directory: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(profile_or_setup.as_bytes());
    digest.update(directory.as_os_str().as_encoded_bytes());
    let hash = format!("{:x}", digest.finalize());
    format!("grant-{}", &hash[..24])
}

fn wine_detection_phase_name(phase: WineDetectionPhase) -> &'static str {
    match phase {
        WineDetectionPhase::Detecting => "detecting",
        WineDetectionPhase::Ready => "ready",
        WineDetectionPhase::Unavailable => "unavailable",
        WineDetectionPhase::Cancelled => "cancelled",
        WineDetectionPhase::Failed => "error",
    }
}

fn wine_setup_view(setup_id: String, setup: &WineSetupSession) -> WineSetupView {
    let (detection_state, detection_message) = setup
        .detection
        .state
        .lock()
        .map(|detection| {
            (
                wine_detection_phase_name(detection.phase),
                detection.message.clone(),
            )
        })
        .unwrap_or((
            "error",
            "Wine-Staging detection is temporarily unavailable.".into(),
        ));
    WineSetupView {
        setup_id: setup_id.clone(),
        wine_label: setup
            .wine_binary
            .as_deref()
            .map(|path| safe_wine_label(path, "Wine-Staging")),
        detected_wine_label: setup
            .wine_binary
            .is_none()
            .then_some(())
            .and_then(|_| setup.detected_wine_binary.as_deref())
            .map(|path| safe_wine_label(path, "Wine-Staging")),
        detection_state,
        detection_message,
        directories: setup
            .game_directories
            .iter()
            .map(|(id, path)| WineDirectoryView {
                id: id.clone(),
                label: safe_wine_label(path, "Authorized game folder"),
            })
            .collect(),
    }
}

fn wine_profile_view(catalog: &Catalog, profile: &WineProfile) -> WineProfileView {
    let imported_games = catalog
        .wine_inventory
        .iter()
        .filter(|entry| entry.profile_id == profile.id)
        .count();
    let last_import_summary = if imported_games == 1 {
        "1 imported game".into()
    } else {
        format!("{imported_games} imported games")
    };
    WineProfileView {
        id: profile.id.clone(),
        display_name: profile.display_name.clone(),
        enabled: profile.enabled,
        wine_label: safe_wine_label(&profile.wine_binary, "Wine-Staging"),
        directories: profile
            .game_directories
            .iter()
            .map(|directory| WineDirectoryView {
                id: wine_directory_view_id(&profile.id, directory),
                label: safe_wine_label(directory, "Authorized game folder"),
            })
            .collect(),
        last_import_at: profile.last_imported_at,
        last_import_summary,
        graphics_backend: match &profile.graphics.backend {
            WineGraphicsBackend::WineD3d => "wine_d3d",
            WineGraphicsBackend::DxvkMacos => "dxvk_macos",
            WineGraphicsBackend::Dxmt => "dxmt",
            WineGraphicsBackend::Auto => "auto",
        },
        graphics_summary: match &profile.graphics.backend {
            WineGraphicsBackend::WineD3d => "Wine 3D · mode de compatibilité",
            WineGraphicsBackend::DxvkMacos => {
                "DXVK-macOS · DirectX 10/11 via MoltenVK vers Metal (par défaut sur Apple Silicon)"
            }
            WineGraphicsBackend::Dxmt => {
                "DXMT · DirectX 10/11 via Metal, en attente d’un moteur Wine compatible"
            }
            WineGraphicsBackend::Auto => "Compatibilité automatique par jeu",
        },
    }
}

fn wine_settings_view(catalog: &Catalog) -> WineSettingsView {
    let mut profiles = catalog
        .wine_profiles
        .iter()
        .map(|profile| wine_profile_view(catalog, profile))
        .collect::<Vec<_>>();
    profiles.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    WineSettingsView {
        runner: wine_runner_status_view(),
        profiles,
    }
}

fn wine_graphics_backend_label(backend: WineGraphicsBackend) -> &'static str {
    match backend {
        WineGraphicsBackend::WineD3d => "Wine 3D",
        WineGraphicsBackend::DxvkMacos => "DXVK-macOS",
        WineGraphicsBackend::Dxmt => "DXMT",
        WineGraphicsBackend::Auto => "automatic compatibility",
    }
}

#[tauri::command]
fn get_wine_runner_status() -> WineRunnerStatusView {
    wine_runner_status_view()
}

#[tauri::command]
async fn begin_wine_profile_setup(state: State<'_, AppState>) -> Result<WineSetupView, String> {
    require_wine_runner_platform()?;
    let setup_id = next_wine_opaque_id(&state, "wine-setup");
    let session = WineSetupSession::new();
    let detection = Arc::clone(&session.detection);
    {
        let mut setups = state
            .wine_setups
            .lock()
            .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?;
        while setups.len() >= MAX_WINE_SETUP_SESSIONS {
            let Some(oldest) = setups.keys().next().cloned() else {
                break;
            };
            if let Some(evicted) = setups.remove(&oldest) {
                evicted.detection.cancelled.store(true, Ordering::Release);
            }
        }
        setups.insert(setup_id.clone(), session);
    }

    // Discovery happens in a cancellable worker after the user opens this
    // optional panel. It only identifies a conventional candidate; it never
    // executes Wine until the user explicitly confirms it.
    let worker_setups = Arc::clone(&state.wine_setups);
    let worker_setup_id = setup_id.clone();
    let worker_detection = Arc::clone(&detection);
    tauri::async_runtime::spawn_blocking(move || {
        let result = wine_runner::detect_wine_staging(&worker_detection.cancelled);
        let (mut phase, mut message, detected) =
            if worker_detection.cancelled.load(Ordering::Acquire) {
                (
                    WineDetectionPhase::Cancelled,
                    "Wine-Staging detection was cancelled.".to_string(),
                    None,
                )
            } else {
                match result {
                Ok(Some(binary)) => (
                    WineDetectionPhase::Ready,
                    "A Wine-Staging candidate was found. Confirm it before Orivo uses it.".into(),
                    Some(binary),
                ),
                Ok(None) => (
                    WineDetectionPhase::Unavailable,
                    "Wine-Staging was not found automatically. Select its Wine binary to continue."
                        .into(),
                    None,
                ),
                Err(wine_runner::WineRunnerError::Cancelled) => (
                    WineDetectionPhase::Cancelled,
                    "Wine-Staging detection was cancelled.".into(),
                    None,
                ),
                Err(error) => (WineDetectionPhase::Failed, error.to_string(), None),
            }
            };
        if let Some(binary) = detected
            && !worker_detection.cancelled.load(Ordering::Acquire)
            && let Ok(mut setups) = worker_setups.lock()
            && let Some(setup) = setups.get_mut(&worker_setup_id)
            && Arc::ptr_eq(&setup.detection, &worker_detection)
        {
            setup.detected_wine_binary = Some(binary);
        }
        if worker_detection.cancelled.load(Ordering::Acquire) {
            phase = WineDetectionPhase::Cancelled;
            message = "Wine-Staging detection was cancelled.".into();
        }
        // Publish Ready only after the setup registry owns the candidate, so
        // a poll can never observe a completed detection with no confirmation
        // target to act on.
        if let Ok(mut status) = worker_detection.state.lock() {
            status.phase = phase;
            status.message = message;
        }
    });

    let setups = state
        .wine_setups
        .lock()
        .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?;
    let setup = setups
        .get(&setup_id)
        .ok_or_else(|| "This Wine setup is no longer available. Start again.".to_string())?;
    Ok(wine_setup_view(setup_id, setup))
}

#[tauri::command]
fn get_wine_profile_setup(
    setup_id: String,
    state: State<'_, AppState>,
) -> Result<WineSetupView, String> {
    require_wine_runner_platform()?;
    if !valid_wine_opaque_id(&setup_id) {
        return Err("This Wine setup is no longer available. Start again.".into());
    }
    let setups = state
        .wine_setups
        .lock()
        .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?;
    let setup = setups
        .get(&setup_id)
        .ok_or_else(|| "This Wine setup is no longer available. Start again.".to_string())?;
    Ok(wine_setup_view(setup_id, setup))
}

#[tauri::command]
fn cancel_wine_detection(
    setup_id: String,
    state: State<'_, AppState>,
) -> Result<WineSetupView, String> {
    require_wine_runner_platform()?;
    if !valid_wine_opaque_id(&setup_id) {
        return Err("This Wine setup is no longer available. Start again.".into());
    }
    let setups = state
        .wine_setups
        .lock()
        .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?;
    let setup = setups
        .get(&setup_id)
        .ok_or_else(|| "This Wine setup is no longer available. Start again.".to_string())?;
    setup.detection.cancelled.store(true, Ordering::Release);
    if let Ok(mut detection) = setup.detection.state.lock() {
        detection.phase = WineDetectionPhase::Cancelled;
        detection.message =
            "Wine-Staging detection was cancelled. You can select Wine manually.".into();
    }
    Ok(wine_setup_view(setup_id, setup))
}

#[tauri::command]
async fn confirm_detected_wine_staging(
    setup_id: String,
    state: State<'_, AppState>,
) -> Result<WineSetupView, String> {
    require_wine_runner_platform()?;
    if !valid_wine_opaque_id(&setup_id) {
        return Err("This Wine setup is no longer available. Start again.".into());
    }
    let candidate = {
        let setups = state
            .wine_setups
            .lock()
            .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?;
        let setup = setups
            .get(&setup_id)
            .ok_or_else(|| "This Wine setup is no longer available. Start again.".to_string())?;
        if setup.wine_binary.is_some() {
            return Ok(wine_setup_view(setup_id, setup));
        }
        let candidate = setup.detected_wine_binary.clone().ok_or_else(|| {
            "No Wine-Staging installation is ready to confirm. Select Wine manually instead."
                .to_string()
        })?;
        setup.detection.cancelled.store(true, Ordering::Release);
        candidate
    };
    let wine_binary = tauri::async_runtime::spawn_blocking(move || {
        wine_runner::probe_wine_staging(&candidate, &AtomicBool::new(false))
    })
    .await
    .map_err(|_| "Wine-Staging validation did not finish. Try again.".to_string())?
    .map_err(|error| error.to_string())?;

    let mut setups = state
        .wine_setups
        .lock()
        .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?;
    let setup = setups
        .get_mut(&setup_id)
        .ok_or_else(|| "This Wine setup is no longer available. Start again.".to_string())?;
    if setup.wine_binary.is_none() {
        setup.wine_binary = Some(wine_binary);
        setup.detected_wine_binary = None;
    }
    if let Ok(mut detection) = setup.detection.state.lock() {
        detection.phase = WineDetectionPhase::Ready;
        detection.message = "Wine-Staging was validated for this profile.".into();
    }
    Ok(wine_setup_view(setup_id, setup))
}

#[tauri::command]
fn choose_wine_game_directory(
    setup_id: String,
    state: State<'_, AppState>,
) -> Result<WineSetupView, String> {
    require_wine_runner_platform()?;
    if !valid_wine_opaque_id(&setup_id) {
        return Err("This Wine setup is no longer available. Start again.".into());
    }
    let Some(selected) = rfd::FileDialog::new()
        .set_title("Choose a Windows games folder for this Wine profile")
        .pick_folder()
    else {
        let setups = state
            .wine_setups
            .lock()
            .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?;
        let setup = setups
            .get(&setup_id)
            .ok_or_else(|| "This Wine setup is no longer available. Start again.".to_string())?;
        return Ok(wine_setup_view(setup_id, setup));
    };

    let directory = fs::canonicalize(selected).map_err(|_| {
        "Orivo could not access that folder. Choose a readable games folder.".to_string()
    })?;
    if !directory.is_dir() {
        return Err("Choose a folder containing your Windows games.".into());
    }

    let mut setups = state
        .wine_setups
        .lock()
        .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?;
    let setup = setups
        .get_mut(&setup_id)
        .ok_or_else(|| "This Wine setup is no longer available. Start again.".to_string())?;
    if !setup
        .game_directories
        .values()
        .any(|existing| existing == &directory)
    {
        let directory_id = next_wine_opaque_id(&state, "wine-directory");
        setup.game_directories.insert(directory_id, directory);
    }
    Ok(wine_setup_view(setup_id, setup))
}

#[tauri::command]
fn remove_wine_setup_directory(
    setup_id: String,
    directory_id: String,
    state: State<'_, AppState>,
) -> Result<WineSetupView, String> {
    require_wine_runner_platform()?;
    if !valid_wine_opaque_id(&setup_id) || !valid_wine_opaque_id(&directory_id) {
        return Err("This Wine setup is no longer available. Start again.".into());
    }
    let mut setups = state
        .wine_setups
        .lock()
        .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?;
    let setup = setups
        .get_mut(&setup_id)
        .ok_or_else(|| "This Wine setup is no longer available. Start again.".to_string())?;
    setup.game_directories.remove(&directory_id);
    Ok(wine_setup_view(setup_id, setup))
}

#[tauri::command]
fn get_wine_runner_settings(state: State<'_, AppState>) -> Result<WineSettingsView, String> {
    let catalog = state
        .catalog
        .read()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
    Ok(wine_settings_view(&catalog))
}

#[tauri::command]
fn set_wine_profile_enabled(
    profile_id: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<WineSettingsView, String> {
    require_wine_runner_platform()?;
    if !valid_wine_opaque_id(&profile_id) {
        return Err("This Wine profile is no longer available.".into());
    }
    let _mutation = state
        .catalog_mutation
        .lock()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
    let mut next = state
        .catalog
        .read()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?
        .clone();
    let profile = next
        .wine_profiles
        .iter_mut()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| "This Wine profile is no longer available.".to_string())?;
    profile.enabled = enabled;
    persist_catalog(&next, &state.catalog_path)
        .map_err(|_| "Orivo could not save this Wine profile change. Try again.".to_string())?;
    let response = wine_settings_view(&next);
    let mut catalog = state
        .catalog
        .write()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
    *catalog = next;
    drop(catalog);
    if !enabled {
        if let Ok(jobs) = state.wine_scan_jobs.lock() {
            for job in jobs.values() {
                if job.profile_id == profile_id {
                    job.cancelled.store(true, Ordering::Release);
                    if let Ok(mut scan) = job.state.lock() {
                        scan.phase = WineScanPhase::Cancelled;
                        scan.message =
                            "Wine import was cancelled because this profile was disabled.".into();
                    }
                }
            }
        }
    }
    Ok(response)
}

#[tauri::command]
fn delete_wine_profile(
    profile_id: String,
    state: State<'_, AppState>,
) -> Result<WineSettingsView, String> {
    require_wine_runner_platform()?;
    if !valid_wine_opaque_id(&profile_id) {
        return Err("This Wine profile is no longer available.".into());
    }
    let _mutation = state
        .catalog_mutation
        .lock()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
    let mut next = state
        .catalog
        .read()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?
        .clone();
    if !next
        .remove_wine_profile(&profile_id)
        .map_err(|_| "This Wine profile could not be removed.".to_string())?
    {
        return Err("This Wine profile is no longer available.".into());
    }
    persist_catalog(&next, &state.catalog_path)
        .map_err(|_| "Orivo could not save this Wine profile change. Try again.".to_string())?;
    let response = wine_settings_view(&next);
    let mut catalog = state
        .catalog
        .write()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
    *catalog = next;
    drop(catalog);

    if let Ok(mut jobs) = state.wine_scan_jobs.lock() {
        for job in jobs.values() {
            if job.profile_id == profile_id {
                job.cancelled.store(true, Ordering::Release);
            }
        }
        jobs.retain(|_, job| job.profile_id != profile_id);
    }
    Ok(response)
}

#[tauri::command]
async fn select_wine_staging(
    setup_id: String,
    state: State<'_, AppState>,
) -> Result<WineSetupView, String> {
    require_wine_runner_platform()?;
    if !valid_wine_opaque_id(&setup_id) {
        return Err("This Wine setup is no longer available. Start again.".into());
    }
    // A manual picker supersedes discovery. This prevents a late background
    // candidate from overwriting the binary the user explicitly chose.
    {
        let setups = state
            .wine_setups
            .lock()
            .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?;
        let setup = setups
            .get(&setup_id)
            .ok_or_else(|| "This Wine setup is no longer available. Start again.".to_string())?;
        setup.detection.cancelled.store(true, Ordering::Release);
        if let Ok(mut detection) = setup.detection.state.lock() {
            detection.phase = WineDetectionPhase::Cancelled;
            detection.message = "Automatic Wine-Staging detection was cancelled.".into();
        }
    }
    let Some(selected) = rfd::FileDialog::new()
        .set_title("Select the Wine-Staging wine binary")
        .pick_file()
    else {
        let setups = state
            .wine_setups
            .lock()
            .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?;
        let setup = setups
            .get(&setup_id)
            .ok_or_else(|| "This Wine setup is no longer available. Start again.".to_string())?;
        return Ok(wine_setup_view(setup_id, setup));
    };

    // Version probing can start a native binary, so it stays off the UI
    // executor and only receives a fixed `--version` argument.
    let wine_binary = tauri::async_runtime::spawn_blocking(move || {
        wine_runner::probe_wine_staging(&selected, &AtomicBool::new(false))
    })
    .await
    .map_err(|_| "Wine-Staging validation did not finish. Try again.".to_string())?
    .map_err(|error| error.to_string())?;

    let mut setups = state
        .wine_setups
        .lock()
        .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?;
    let setup = setups
        .get_mut(&setup_id)
        .ok_or_else(|| "This Wine setup is no longer available. Start again.".to_string())?;
    setup.wine_binary = Some(wine_binary);
    setup.detected_wine_binary = None;
    if let Ok(mut detection) = setup.detection.state.lock() {
        detection.phase = WineDetectionPhase::Ready;
        detection.message = "Wine-Staging was validated for this profile.".into();
    }
    Ok(wine_setup_view(setup_id, setup))
}

#[tauri::command]
async fn create_wine_profile(
    setup_id: String,
    display_name: String,
    state: State<'_, AppState>,
) -> Result<WineProfileCreatedView, String> {
    require_wine_runner_platform()?;
    if !valid_wine_opaque_id(&setup_id) {
        return Err("This Wine setup is no longer available. Start again.".into());
    }
    let setup = state
        .wine_setups
        .lock()
        .map_err(|_| "Wine setup is temporarily unavailable.".to_string())?
        .get(&setup_id)
        .cloned()
        .ok_or_else(|| "This Wine setup is no longer available. Start again.".to_string())?;
    let wine_binary = setup.wine_binary.ok_or_else(|| {
        "Select a valid Wine-Staging installation before creating a profile.".to_string()
    })?;
    let game_directories = setup.game_directories.into_values().collect::<Vec<_>>();
    if game_directories.is_empty() {
        return Err("Choose at least one Windows games folder for this profile.".into());
    }

    let profile_id = next_wine_opaque_id(&state, "wine-profile");
    let prefix = state.wine_prefix_root.join(&profile_id);
    let trimmed_name = display_name.trim().to_string();
    let profile = WineProfile {
        id: profile_id.clone(),
        display_name: trimmed_name,
        wine_binary,
        prefix: prefix.clone(),
        game_directories,
        graphics: WineGraphicsOptions::default(),
        dxmt_engine_supported: None,
        macos_retina_mode_enabled: None,
        enabled: true,
        last_imported_at: None,
    };

    // Reprobe the selected binary and create only the generated, dedicated
    // prefix directory through the runner host. Wine itself initialises it on
    // first launch through the controlled WINEPREFIX environment; no external
    // prefix is touched or adopted.
    profile
        .validate()
        .map_err(|_| "Give this Wine profile a valid name and games folder.".to_string())?;
    let (validated_binary, managed_prefix, dxmt_engine_supported) =
        tauri::async_runtime::spawn_blocking({
            let wine_binary = profile.wine_binary.clone();
            let profile_id = profile_id.clone();
            let prefix_root = state.wine_prefix_root.clone();
            move || -> Result<(PathBuf, PathBuf, bool), wine_runner::WineRunnerError> {
                let wine = wine_runner::probe_wine_staging(&wine_binary, &AtomicBool::new(false))?;
                let dxmt_engine_supported = matches!(
                    wine_runner::probe_dxmt_wine_engine_for_validated_binary(
                        &wine,
                        &AtomicBool::new(false),
                    )?,
                    wine_runner::DxmtWineEngineSupport::Supported
                );
                let prefix = wine_runner::create_managed_prefix(&prefix_root, &profile_id)?;
                Ok((wine, prefix, dxmt_engine_supported))
            }
        })
        .await
        .map_err(|_| "Wine profile validation did not finish. Try again.".to_string())?
        .map_err(|error| error.to_string())?;
    let mut profile = profile;
    profile.wine_binary = validated_binary;
    profile.prefix = managed_prefix;
    profile.dxmt_engine_supported = Some(dxmt_engine_supported);
    profile
        .validate()
        .map_err(|_| "Give this Wine profile a valid name and games folder.".to_string())?;

    let _mutation = state
        .catalog_mutation
        .lock()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
    let mut next = state
        .catalog
        .read()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?
        .clone();
    next.upsert_wine_profile(profile.clone())
        .map_err(|_| "This Wine profile could not be created.".to_string())?;
    persist_catalog(&next, &state.catalog_path)
        .map_err(|_| "Orivo could not save this Wine profile. Try again.".to_string())?;
    let profile_view = wine_profile_view(&next, &profile);
    {
        let mut catalog = state
            .catalog
            .write()
            .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
        *catalog = next;
    }
    if let Ok(mut setups) = state.wine_setups.lock() {
        setups.remove(&setup_id);
    }
    Ok(WineProfileCreatedView {
        profile_id,
        profile: profile_view,
    })
}

fn wine_scan_phase_name(phase: WineScanPhase) -> &'static str {
    match phase {
        WineScanPhase::Scanning => "scanning",
        WineScanPhase::Ready => "ready",
        WineScanPhase::Cancelled => "cancelled",
        WineScanPhase::Failed => "error",
    }
}

fn wine_scan_job_view(job_id: &str, job: &WineScanJob) -> Result<WineScanJobView, String> {
    let scan = job
        .state
        .lock()
        .map_err(|_| "Wine import status is temporarily unavailable.".to_string())?;
    Ok(WineScanJobView {
        job_id: job_id.into(),
        profile_id: job.profile_id.clone(),
        state: wine_scan_phase_name(scan.phase),
        scanned_files: scan.scanned_files,
        found_games: scan.candidates.len(),
        message: scan.message.clone(),
        complete: scan.phase != WineScanPhase::Scanning,
    })
}

fn wine_scan_cursor(job_id: &str, offset: usize) -> String {
    format!("wine-page:{job_id}:{offset}")
}

fn wine_scan_cursor_offset(job_id: &str, cursor: Option<String>) -> Result<usize, String> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let Some(offset) = cursor
        .strip_prefix(&format!("wine-page:{job_id}:"))
        .and_then(|value| value.parse::<usize>().ok())
    else {
        return Err("This Wine import page is no longer available. Refresh the scan.".into());
    };
    Ok(offset)
}

#[tauri::command]
fn start_wine_profile_scan(
    profile_id: String,
    state: State<'_, AppState>,
) -> Result<WineScanJobView, String> {
    require_wine_runner_platform()?;
    if !valid_wine_opaque_id(&profile_id) {
        return Err("This Wine profile is no longer available.".into());
    }
    let profile = state
        .catalog
        .read()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?
        .wine_profile(&profile_id)
        .cloned()
        .ok_or_else(|| "This Wine profile is no longer available.".to_string())?;
    if !profile.enabled {
        return Err("This Wine profile is disabled. Enable it before importing games.".into());
    }

    let job_id = next_wine_opaque_id(&state, "wine-scan");
    let job = Arc::new(WineScanJob {
        profile_id: profile_id.clone(),
        cancelled: Arc::new(AtomicBool::new(false)),
        state: Mutex::new(WineScanJobState {
            phase: WineScanPhase::Scanning,
            scanned_files: 0,
            candidates: Vec::new(),
            message: "Scanning the folders allowed for this Wine profile…".into(),
        }),
    });
    {
        let mut jobs = state
            .wine_scan_jobs
            .lock()
            .map_err(|_| "Wine import is temporarily unavailable.".to_string())?;
        while jobs.len() >= MAX_WINE_SCAN_JOBS {
            let Some(oldest) = jobs.keys().next().cloned() else {
                break;
            };
            if let Some(evicted) = jobs.remove(&oldest) {
                evicted.cancelled.store(true, Ordering::Release);
                if let Ok(mut scan) = evicted.state.lock() {
                    scan.phase = WineScanPhase::Cancelled;
                    scan.message = "Wine import was cancelled because its preview expired.".into();
                }
            }
        }
        jobs.insert(job_id.clone(), Arc::clone(&job));
    }

    let worker_job = Arc::clone(&job);
    tauri::async_runtime::spawn_blocking(move || {
        let cancellation = Arc::clone(&worker_job.cancelled);
        let result = wine_runner::scan_wine_games(
            &profile,
            &cancellation,
            wine_runner::ScanLimits::default(),
            |scanned_files| {
                if let Ok(mut scan) = worker_job.state.lock() {
                    if scan.phase == WineScanPhase::Scanning {
                        scan.scanned_files = scanned_files;
                    }
                }
            },
        );
        let Ok(mut scan) = worker_job.state.lock() else {
            return;
        };
        if worker_job.cancelled.load(Ordering::Acquire) {
            scan.phase = WineScanPhase::Cancelled;
            scan.message = "Wine import was cancelled.".into();
            return;
        }
        match result {
            Ok(result) => {
                scan.phase = WineScanPhase::Ready;
                scan.scanned_files = result.scanned_files;
                scan.candidates = result.games;
                scan.message = if scan.candidates.is_empty() {
                    "No Windows executables were found in the allowed folders.".into()
                } else {
                    "Wine game scan is ready to import.".into()
                };
            }
            Err(wine_runner::WineRunnerError::Cancelled) => {
                scan.phase = WineScanPhase::Cancelled;
                scan.message = "Wine import was cancelled.".into();
            }
            Err(error) => {
                scan.phase = WineScanPhase::Failed;
                scan.message = error.to_string();
            }
        }
    });
    wine_scan_job_view(&job_id, &job)
}

#[tauri::command]
fn get_wine_scan_status(
    job_id: String,
    state: State<'_, AppState>,
) -> Result<WineScanJobView, String> {
    if !valid_wine_opaque_id(&job_id) {
        return Err("This Wine import is no longer available. Start it again.".into());
    }
    let jobs = state
        .wine_scan_jobs
        .lock()
        .map_err(|_| "Wine import status is temporarily unavailable.".to_string())?;
    let job = jobs
        .get(&job_id)
        .ok_or_else(|| "This Wine import is no longer available. Start it again.".to_string())?;
    wine_scan_job_view(&job_id, job)
}

#[tauri::command]
fn get_wine_scan_page(
    job_id: String,
    cursor: Option<String>,
    limit: u32,
    state: State<'_, AppState>,
) -> Result<WineScanPageView, String> {
    if !valid_wine_opaque_id(&job_id) {
        return Err("This Wine import is no longer available. Start it again.".into());
    }
    let offset = wine_scan_cursor_offset(&job_id, cursor)?;
    let (profile_id, phase, candidates) = {
        let jobs = state
            .wine_scan_jobs
            .lock()
            .map_err(|_| "Wine import status is temporarily unavailable.".to_string())?;
        let job = jobs.get(&job_id).ok_or_else(|| {
            "This Wine import is no longer available. Start it again.".to_string()
        })?;
        let scan = job
            .state
            .lock()
            .map_err(|_| "Wine import status is temporarily unavailable.".to_string())?;
        (job.profile_id.clone(), scan.phase, scan.candidates.clone())
    };
    if phase == WineScanPhase::Scanning {
        return Ok(WineScanPageView {
            games: Vec::new(),
            next_cursor: None,
            complete: false,
        });
    }
    if phase != WineScanPhase::Ready {
        return Err("This Wine import did not complete. Retry the scan.".into());
    }
    let requested_limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let (page, next) = wine_runner::page_wine_inventory(&candidates, offset, requested_limit)
        .map_err(|_| {
            "This Wine import page is no longer available. Refresh the scan.".to_string()
        })?;
    let catalog = state
        .catalog
        .read()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
    let profile_enabled = catalog
        .wine_profile(&profile_id)
        .is_some_and(|profile| profile.enabled);
    Ok(WineScanPageView {
        games: page
            .into_iter()
            .map(|game| WineScanGameView {
                already_imported: catalog
                    .wine_inventory_entry(&profile_id, &game.game_ref)
                    .is_some(),
                game_ref: game.game_ref,
                title: game.title,
                directory_label: game.directory_label,
                launchable: profile_enabled,
            })
            .collect(),
        next_cursor: next.map(|next| wine_scan_cursor(&job_id, next)),
        complete: next.is_none(),
    })
}

#[tauri::command]
fn cancel_wine_scan(job_id: String, state: State<'_, AppState>) -> Result<WineScanJobView, String> {
    if !valid_wine_opaque_id(&job_id) {
        return Err("This Wine import is no longer available. Start it again.".into());
    }
    let jobs = state
        .wine_scan_jobs
        .lock()
        .map_err(|_| "Wine import is temporarily unavailable.".to_string())?;
    let job = jobs
        .get(&job_id)
        .ok_or_else(|| "This Wine import is no longer available. Start it again.".to_string())?;
    job.cancelled.store(true, Ordering::Release);
    if let Ok(mut scan) = job.state.lock() {
        scan.phase = WineScanPhase::Cancelled;
        scan.message = "Wine import was cancelled.".into();
    }
    wine_scan_job_view(&job_id, job)
}

fn wine_game_id(profile_id: &str, game_ref: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(profile_id.as_bytes());
    digest.update(game_ref.as_bytes());
    format!(
        "runner:{WINE_STAGING_RUNNER_ID}:{profile_id}:{:x}",
        digest.finalize()
    )
}

fn wine_catalog_game(profile_id: &str, candidate: &wine_runner::ScannedWineGame) -> Game {
    Game {
        id: wine_game_id(profile_id, &candidate.game_ref),
        title: candidate.title.clone(),
        executable_path: None,
        source: GameSource::Local,
        source_id: None,
        launch_target: LaunchTarget::Runner {
            runner_id: WINE_STAGING_RUNNER_ID.into(),
            game_ref: candidate.game_ref.clone(),
            profile_id: profile_id.into(),
        },
        installation_path: None,
        working_directory: None,
        arguments: Vec::new(),
        description: Some("Windows game imported through your Wine-Staging profile.".into()),
        metadata: Some("Wine-Staging".into()),
        artwork_path: None,
        artwork_source_path: None,
        cover_path: None,
        cover_source_path: None,
        home_image_path: None,
        landscape_image_path: None,
        logo_path: None,
        hero_video_path: None,
        last_played_at: None,
        play_time_seconds: 0,
        extra: BTreeMap::new(),
    }
}

/// Produce a path-free runner card from an existing Direct game. The original
/// Direct record remains persisted as the reversible source of truth; this
/// card carries only its presentation and playback state plus the opaque Wine
/// runner target.
fn wine_catalog_game_from_direct(
    profile_id: &str,
    candidate: &wine_runner::ScannedWineGame,
    direct_game: &Game,
) -> Game {
    let mut runner_game = wine_catalog_game(profile_id, candidate);
    runner_game.title = direct_game.title.clone();
    runner_game.description = direct_game.description.clone();
    runner_game.metadata = direct_game.metadata.clone();
    runner_game.artwork_path = direct_game.artwork_path.clone();
    runner_game.artwork_source_path = direct_game.artwork_source_path.clone();
    runner_game.cover_path = direct_game.cover_path.clone();
    runner_game.cover_source_path = direct_game.cover_source_path.clone();
    runner_game.logo_path = direct_game.logo_path.clone();
    runner_game.hero_video_path = direct_game.hero_video_path.clone();
    runner_game.last_played_at = direct_game.last_played_at.clone();
    runner_game.play_time_seconds = direct_game.play_time_seconds;
    runner_game.extra = direct_game.extra.clone();
    runner_game
}

fn valid_catalog_game_lookup_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 8_192 && !value.chars().any(char::is_control)
}

fn is_local_direct_windows_game(game: &Game) -> bool {
    game.source == GameSource::Local
        && matches!(&game.launch_target, LaunchTarget::Direct)
        && game
            .executable_path
            .as_deref()
            .and_then(|path| path.extension())
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

/// Associate a local Windows executable already known to the catalog with a
/// profile. IPC carries only an existing catalog id and a profile id; the
/// host resolves, canonicalises, hashes and scopes the executable itself.
#[tauri::command]
async fn associate_direct_game_with_wine_profile(
    game_id: String,
    profile_id: String,
    state: State<'_, AppState>,
) -> Result<WineAssociationView, String> {
    require_wine_runner_platform()?;
    if !valid_catalog_game_lookup_id(&game_id) || !valid_wine_opaque_id(&profile_id) {
        return Err("This Windows game or Wine profile is no longer available.".into());
    }
    let (direct_game, profile, executable) = {
        let catalog = state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable.".to_string())?;
        let direct_game = catalog
            .games
            .iter()
            .find(|game| game.id == game_id)
            .cloned()
            .filter(is_local_direct_windows_game)
            .ok_or_else(|| {
                "Choose a local Windows .exe that is already in your library.".to_string()
            })?;
        let profile = catalog
            .wine_profile(&profile_id)
            .cloned()
            .filter(|profile| profile.enabled)
            .ok_or_else(|| "This Wine profile is disabled or no longer available.".to_string())?;
        let executable = direct_game.executable_path.clone().ok_or_else(|| {
            "This local Windows game needs to be imported again before Wine can use it.".to_string()
        })?;
        (direct_game, profile, executable)
    };

    let candidate = tauri::async_runtime::spawn_blocking({
        let profile = profile.clone();
        move || wine_runner::validate_wine_game_for_profile(&profile, &executable, &AtomicBool::new(false))
    })
    .await
    .map_err(|_| "Wine could not validate this Windows game. Try again.".to_string())?
    .map_err(|error| match error {
        wine_runner::WineRunnerError::GameOutsideScope => {
            "This Windows game is outside the folders allowed for that Wine profile. Choose a profile that authorizes its games folder.".to_string()
        }
        wine_runner::WineRunnerError::GameMissing => {
            "This Windows game is no longer available. Import it again and retry.".to_string()
        }
        error => error.to_string(),
    })?;

    let _mutation = state
        .catalog_mutation
        .lock()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
    let mut next = state
        .catalog
        .read()
        .map_err(|_| "The game catalog is temporarily unavailable.".to_string())?
        .clone();
    if next.wine_profile(&profile_id) != Some(&profile) {
        return Err(
            "This Wine profile changed while the game was being validated. Try again.".into(),
        );
    }
    if next.games.iter().find(|game| game.id == game_id) != Some(&direct_game) {
        return Err("This local game changed while Wine was being prepared. Try again.".into());
    }
    let imported_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default();
    let inventory = WineGameInventoryEntry {
        profile_id: profile_id.clone(),
        game_ref: candidate.game_ref.clone(),
        title: candidate.title.clone(),
        executable_path: candidate.executable_path.clone(),
        fingerprint: candidate.fingerprint.clone(),
        imported_at: Some(imported_at),
        compatibility: WineGameCompatibility::automatic(),
        origin_direct_game_id: Some(game_id.clone()),
    };
    let runner_game = wine_catalog_game_from_direct(&profile_id, &candidate, &direct_game);
    let runner_game_id = runner_game.id.clone();
    next.associate_direct_game_with_wine_profile(&game_id, inventory, runner_game)
        .map_err(|_| {
            "This Windows game could not be associated with the Wine profile safely.".to_string()
        })?;
    if let Some(profile) = next
        .wine_profiles
        .iter_mut()
        .find(|profile| profile.id == profile_id)
    {
        profile.last_imported_at = Some(imported_at);
    }
    persist_catalog(&next, &state.catalog_path).map_err(|_| {
        "Orivo could not save this Wine association. Your library was left unchanged.".to_string()
    })?;
    let mut catalog = state
        .catalog
        .write()
        .map_err(|_| "The game catalog is temporarily unavailable.".to_string())?;
    *catalog = next;
    Ok(WineAssociationView {
        game_id: runner_game_id,
    })
}

/// Best-effort conversion of every local Direct Windows `.exe` in the catalog
/// into a card backed by the Orivo-managed default Wine profile, so Windows
/// games launch through Wine-Staging without the user creating a profile or
/// associating a game by hand.
///
/// Every host invariant of the manual association path is preserved: each
/// executable is canonicalised, scope-checked against a grant derived only
/// from the folder Orivo already holds for that game, content-fingerprinted,
/// and reduced to opaque ids before it becomes a runner card. The original
/// Direct record is retained so the association is fully reversible — deleting
/// the managed profile restores it. Orivo never bundles Wine, so when no
/// Wine-Staging installation is detected the games stay Direct and nothing is
/// changed.
///
/// Returns `true` when the catalog was modified. The caller owns locking and
/// persistence.
fn auto_apply_wine_to_direct_games(catalog: &mut Catalog, wine_prefix_root: &Path) -> bool {
    if !cfg!(target_os = "macos") {
        // The built-in Wine-Staging runner is macOS-only.
        return false;
    }

    let pending = catalog
        .games
        .iter()
        .filter(|game| is_local_direct_windows_game(game))
        .filter(|game| {
            !catalog
                .wine_inventory
                .iter()
                .any(|entry| entry.origin_direct_game_id.as_deref() == Some(game.id.as_str()))
        })
        .filter_map(|game| {
            game.executable_path
                .clone()
                .map(|executable| (game.id.clone(), executable))
        })
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return false;
    }

    let cancelled = AtomicBool::new(false);
    let apple_silicon = macos_is_apple_silicon();

    // Reuse the persisted managed default profile, or provision a new one. A
    // new profile needs a real, probed Wine-Staging engine and its own
    // host-owned prefix; without a detected engine the games stay Direct.
    let mut profile = match catalog.wine_profile(AUTO_WINE_PROFILE_ID).cloned() {
        Some(profile) if profile.enabled => profile,
        // Respect an explicit user disable of the managed default profile.
        Some(_) => return false,
        None => {
            let wine_binary = match wine_runner::detect_wine_staging(&cancelled) {
                Ok(Some(binary)) => match wine_runner::probe_wine_staging(&binary, &cancelled) {
                    Ok(validated) => validated,
                    Err(_) => return false,
                },
                Ok(None) | Err(_) => return false,
            };
            let prefix = match wine_runner::ensure_managed_profile_prefix(
                wine_prefix_root,
                AUTO_WINE_PROFILE_ID,
            ) {
                Ok(prefix) => prefix,
                Err(_) => return false,
            };
            WineProfile {
                id: AUTO_WINE_PROFILE_ID.to_string(),
                display_name: AUTO_WINE_PROFILE_NAME.to_string(),
                wine_binary,
                prefix,
                game_directories: Vec::new(),
                graphics: WineGraphicsOptions {
                    // On Apple Silicon, DXVK-macOS is the default so the user
                    // never enables Metal translation by hand. New game cards
                    // are Auto+Isolated and still install the pinned, verified
                    // DXVK runtime lazily on first launch; this profile-wide
                    // value drives the settings display and any legacy
                    // shared-prefix game (of which the managed default has
                    // none).
                    backend: if apple_silicon {
                        WineGraphicsBackend::DxvkMacos
                    } else {
                        WineGraphicsBackend::WineD3d
                    },
                    virtual_desktop: None,
                },
                dxmt_engine_supported: None,
                macos_retina_mode_enabled: None,
                enabled: true,
                last_imported_at: None,
            }
        }
    };

    let imported_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default();

    let mut changed = false;
    for (direct_game_id, executable) in pending {
        // Canonicalise the stored path and derive a grant no broader than the
        // executable's own directory — the folder the user already pointed
        // Orivo at when importing this game. This never widens scope to a
        // parent tree and never runs a new disk scan.
        let Ok(canonical) = fs::canonicalize(&executable) else {
            continue;
        };
        let Some(parent) = canonical.parent().map(Path::to_path_buf) else {
            continue;
        };

        // Validate the game against a trial profile carrying the derived grant.
        // This hashes and scope-checks the executable exactly as the manual
        // association command does.
        let mut trial = profile.clone();
        if !trial.game_directories.contains(&parent) {
            trial.game_directories.push(parent);
        }
        if trial.validate().is_err() {
            continue;
        }
        let candidate =
            match wine_runner::validate_wine_game_for_profile(&trial, &canonical, &cancelled) {
                Ok(candidate) => candidate,
                Err(_) => continue,
            };
        let Some(direct_game) = catalog
            .games
            .iter()
            .find(|game| game.id == direct_game_id)
            .cloned()
            .filter(is_local_direct_windows_game)
        else {
            continue;
        };

        let inventory = WineGameInventoryEntry {
            profile_id: AUTO_WINE_PROFILE_ID.to_string(),
            game_ref: candidate.game_ref.clone(),
            title: candidate.title.clone(),
            executable_path: candidate.executable_path.clone(),
            fingerprint: candidate.fingerprint.clone(),
            imported_at: Some(imported_at),
            compatibility: WineGameCompatibility::automatic(),
            origin_direct_game_id: Some(direct_game_id.clone()),
        };
        let runner_game =
            wine_catalog_game_from_direct(AUTO_WINE_PROFILE_ID, &candidate, &direct_game);

        // Apply the association on a candidate clone so a rejected game leaves
        // the catalog (and the accumulated grant) untouched. The managed
        // profile carrying the derived grant must be present before the
        // association's catalog-level scope check runs.
        let mut next = catalog.clone();
        if next
            .wine_profiles
            .iter_mut()
            .find(|existing| existing.id == AUTO_WINE_PROFILE_ID)
            .map(|existing| *existing = trial.clone())
            .is_none()
        {
            next.wine_profiles.push(trial.clone());
        }
        if next
            .associate_direct_game_with_wine_profile(&direct_game_id, inventory, runner_game)
            .is_err()
        {
            continue;
        }
        if let Some(persisted) = next
            .wine_profiles
            .iter_mut()
            .find(|persisted| persisted.id == AUTO_WINE_PROFILE_ID)
        {
            persisted.last_imported_at = Some(imported_at);
        }
        if next.validate().is_err() {
            continue;
        }
        *catalog = next;
        profile = trial;
        changed = true;
    }

    changed
}

const MAX_DXVK_DOWNLOAD_BYTES: usize = 128 * 1024 * 1024;

/// The explicit profile action is allowed to change its legacy graphics
/// setting. The automatic path is deliberately narrower: it prepares only
/// the requested new game's private compatibility seed and must never alter
/// an existing shared-prefix game or override a user's Wine 3D choice.
#[derive(Debug)]
enum DxvkMacosInstallTarget {
    ProfileSettings,
    AutomaticGame { game_ref: String },
}

/// Download the one pinned DXVK-macOS archive from a host-owned URL, cap it
/// before parsing, then validate its release hash and file allowlist. No URL,
/// archive pathname, command, environment variable, or DLL choice crosses
/// the WebView boundary.
async fn download_verified_dxvk_macos_package() -> Result<wine_runner::DxvkMacosPackage, String> {
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|_| wine_runner::WineRunnerError::DxvkDownload.to_string())?
        .get(wine_runner::DXVK_MACOS_DOWNLOAD_URL)
        .send()
        .await
        .map_err(|_| wine_runner::WineRunnerError::DxvkDownload.to_string())?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > MAX_DXVK_DOWNLOAD_BYTES as u64)
    {
        return Err(wine_runner::WineRunnerError::DxvkDownload.to_string());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| wine_runner::WineRunnerError::DxvkDownload.to_string())?;
        if bytes.len().saturating_add(chunk.len()) > MAX_DXVK_DOWNLOAD_BYTES {
            return Err(wine_runner::WineRunnerError::DxvkDownload.to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    tauri::async_runtime::spawn_blocking(move || wine_runner::load_dxvk_macos_package_bytes(&bytes))
        .await
        .map_err(|_| "DXVK-macOS package validation did not finish. Try again.".to_string())?
        .map_err(|error| error.to_string())
}

/// Download and install the one allowlisted DXVK-macOS archive into a profile
/// private prefix. Callers hold only a validated opaque profile id; the fixed
/// URL, archive parser, Wine process and DLL selection remain host-owned.
async fn install_verified_dxvk_macos_for_profile(
    profile_id: &str,
    target: DxvkMacosInstallTarget,
    state: &AppState,
) -> Result<WineSettingsView, String> {
    if !valid_wine_opaque_id(profile_id) {
        return Err("This Wine profile is no longer available.".into());
    }
    if let DxvkMacosInstallTarget::AutomaticGame { game_ref } = &target
        && !valid_wine_opaque_id(game_ref)
    {
        return Err("This Windows game needs to be reimported before it can launch.".into());
    }
    let profile_id = profile_id.to_string();
    let profile_is_available = state
        .catalog
        .read()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?
        .wine_profile(&profile_id)
        .is_some_and(|profile| profile.enabled);
    if !profile_is_available {
        return Err("Enable this Wine profile before installing DXVK-macOS.".into());
    }

    // Downloading and parsing happen without the catalog/prefix mutation
    // lease, so settings and navigation stay responsive while the fixed
    // package is fetched. Prefix writes begin only after validation succeeds.
    let package = download_verified_dxvk_macos_package().await?;

    let prefix_root = state.wine_prefix_root.clone();
    let catalog_path = state.catalog_path.clone();
    let catalog_state = Arc::clone(&state.catalog);
    let mutation = Arc::clone(&state.catalog_mutation);
    tauri::async_runtime::spawn_blocking(move || {
        let _mutation = mutation
            .lock()
            .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
        let profile = {
            let catalog = catalog_state
                .read()
                .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
            let profile = catalog
                .wine_profile(&profile_id)
                .cloned()
                .filter(|profile| profile.enabled)
                .ok_or_else(|| "Enable this Wine profile before installing DXVK-macOS.".to_string())?;
            if let DxvkMacosInstallTarget::AutomaticGame { game_ref } = &target {
                let inventory = catalog.wine_inventory_entry(&profile_id, game_ref).ok_or_else(|| {
                    "This Windows game needs to be reimported before it can launch.".to_string()
                })?;
                if !automatic_dxvk_is_eligible(inventory) {
                    return Err("This Windows game keeps its selected Wine compatibility mode.".into());
                }
            }
            profile
        };

        wine_runner::install_dxvk_macos(&profile, &prefix_root, &package)
            .map_err(|error| error.to_string())?;

        let mut next = catalog_state
            .read()
            .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?
            .clone();
        let current_profile = next
            .wine_profiles
            .iter_mut()
            .find(|candidate| candidate.id == profile_id)
            .ok_or_else(|| "This Wine profile is no longer available.".to_string())?;
        if *current_profile != profile {
            return Err("This Wine profile changed while DXVK-macOS was installing. Try again.".into());
        }
        match target {
            // This is the user-selected profile setting. It retains the
            // legacy shared-prefix migration behaviour and refreshes Auto
            // choices that have not explicitly rejected DXVK.
            DxvkMacosInstallTarget::ProfileSettings => {
                current_profile.graphics.backend = WineGraphicsBackend::DxvkMacos;
                for entry in next
                    .wine_inventory
                    .iter_mut()
                    .filter(|entry| entry.profile_id == profile_id)
                    .filter(|entry| {
                        entry.compatibility.prefix_layout
                            == catalog::WinePrefixLayout::LegacySharedProfile
                    })
                {
                    entry.compatibility.graphics.backend = WineGraphicsBackend::DxvkMacos;
                    entry.compatibility.last_backend = Some(WineGraphicsBackend::DxvkMacos);
                }
                for entry in next
                    .wine_inventory
                    .iter_mut()
                    .filter(|entry| entry.profile_id == profile_id)
                    .filter(|entry| automatic_dxvk_is_eligible(entry))
                {
                    entry.compatibility.last_backend = None;
                }
            }
            // The first automatic launch is never a profile-wide graphics
            // choice. Persist only this game's fresh Auto decision so legacy
            // games and any explicit Wine 3D setting stay exactly as they
            // were.
            DxvkMacosInstallTarget::AutomaticGame { game_ref } => {
                let entry = next
                    .wine_inventory
                    .iter_mut()
                    .find(|entry| entry.profile_id == profile_id && entry.game_ref == game_ref)
                    .ok_or_else(|| {
                        "This Windows game needs to be reimported before it can launch."
                            .to_string()
                    })?;
                if !automatic_dxvk_is_eligible(entry) {
                    return Err("This Windows game keeps its selected Wine compatibility mode.".into());
                }
                entry.compatibility.last_backend = None;
            }
        }
        persist_catalog(&next, &catalog_path).map_err(|_| {
            "DXVK-macOS was installed, but Orivo could not save this profile setting. Reinstall it and try again."
                .to_string()
        })?;
        let response = wine_settings_view(&next);
        let mut catalog = catalog_state
            .write()
            .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
        *catalog = next;
        Ok::<WineSettingsView, String>(response)
    })
    .await
    .map_err(|_| "DXVK-macOS installation did not finish. Try again.".to_string())?
}

/// Install the fixed DXVK-macOS runtime on explicit user request. The same
/// host-only helper is also used before an automatic Wine game launch so that
/// a new profile does not silently fall back to Wine 3D for D3D11 titles.
#[tauri::command]
async fn install_dxvk_macos_for_profile(
    profile_id: String,
    state: State<'_, AppState>,
) -> Result<WineSettingsView, String> {
    require_wine_runner_platform()?;
    install_verified_dxvk_macos_for_profile(
        &profile_id,
        DxvkMacosInstallTarget::ProfileSettings,
        &state,
    )
    .await
}

fn automatic_dxvk_is_eligible(inventory: &WineGameInventoryEntry) -> bool {
    inventory.compatibility.graphics.backend == WineGraphicsBackend::Auto
        && inventory.compatibility.prefix_layout == catalog::WinePrefixLayout::Isolated
        && !inventory
            .compatibility
            .rejected_backends
            .contains(&WineGraphicsBackend::DxvkMacos)
}

/// Prepare DXVK on demand for a new Auto game before it reaches Wine 3D. The
/// preflight reads only the selected profile's host-owned prefix; a missing or
/// corrupted seed triggers the one pinned, verified download rather than an
/// unhelpful D3D11 device failure inside the game.
async fn ensure_dxvk_macos_for_automatic_game(
    app: &AppHandle,
    game_id: &str,
    profile: &WineProfile,
    inventory: &WineGameInventoryEntry,
    state: &AppState,
) -> Result<bool, String> {
    if !automatic_dxvk_is_eligible(inventory) {
        return Ok(false);
    }

    let profile_for_check = profile.clone();
    let prefix_root = state.wine_prefix_root.clone();
    let available = tauri::async_runtime::spawn_blocking(move || {
        wine_runner::profile_has_dxvk_macos_runtime(&profile_for_check, &prefix_root)
    })
    .await
    .map_err(|_| "Wine compatibility preflight did not finish. Try again.".to_string())?;
    if available {
        return Ok(false);
    }

    let _ = app.emit_to(
        MAIN_WINDOW_LABEL,
        WINE_LAUNCH_STATUS_EVENT,
        WineLaunchStatusEvent {
            game_id: game_id.to_string(),
            phase: "preparing",
            message: "Téléchargement et vérification de DXVK-macOS pour ce jeu…".into(),
        },
    );
    install_verified_dxvk_macos_for_profile(
        &profile.id,
        DxvkMacosInstallTarget::AutomaticGame {
            game_ref: inventory.game_ref.clone(),
        },
        state,
    )
    .await?;
    Ok(true)
}

/// Return a profile to Wine's built-in 3D path. The optional DXVK files stay
/// inside the profile's private prefix, but no longer receive an override at
/// launch. This gives users a safe, reversible fallback when an experimental
/// D3D10/11 translation causes a regression.
#[tauri::command]
fn use_wine_3d_for_profile(
    profile_id: String,
    state: State<'_, AppState>,
) -> Result<WineSettingsView, String> {
    require_wine_runner_platform()?;
    if !valid_wine_opaque_id(&profile_id) {
        return Err("This Wine profile is no longer available.".into());
    }
    let _mutation = state
        .catalog_mutation
        .lock()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
    let mut next = state
        .catalog
        .read()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?
        .clone();
    let profile = next
        .wine_profiles
        .iter_mut()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| "This Wine profile is no longer available.".to_string())?;
    profile.graphics.backend = WineGraphicsBackend::WineD3d;
    for entry in next
        .wine_inventory
        .iter_mut()
        .filter(|entry| entry.profile_id == profile_id)
        .filter(|entry| {
            entry.compatibility.prefix_layout == catalog::WinePrefixLayout::LegacySharedProfile
        })
    {
        entry.compatibility.graphics.backend = WineGraphicsBackend::WineD3d;
        entry.compatibility.last_backend = Some(WineGraphicsBackend::WineD3d);
    }
    persist_catalog(&next, &state.catalog_path)
        .map_err(|_| "Orivo could not save this graphics setting. Try again.".to_string())?;
    let response = wine_settings_view(&next);
    let mut catalog = state
        .catalog
        .write()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
    *catalog = next;
    Ok(response)
}

/// Advance one Wine game's closed Auto policy after the user explicitly asks
/// for a retry. This never observes a short-lived process and relaunches on
/// its own: early exit is ambiguous, so the next backend is selected only by
/// this path-free user action.
#[tauri::command]
async fn retry_wine_game_in_compatibility(
    game_id: String,
    state: State<'_, AppState>,
) -> Result<LaunchResult, String> {
    require_wine_runner_platform()?;
    if !valid_catalog_game_lookup_id(&game_id) {
        return Err("This Wine game is no longer available.".into());
    }
    let catalog_path = state.catalog_path.clone();
    let catalog_state = Arc::clone(&state.catalog);
    let mutation = Arc::clone(&state.catalog_mutation);
    tauri::async_runtime::spawn_blocking(move || {
        let _mutation = mutation
            .lock()
            .map_err(|_| "Wine compatibility is temporarily unavailable.".to_string())?;
        let mut next = catalog_state
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable.".to_string())?
            .clone();
        let (profile_id, game_ref) = next
            .games
            .iter()
            .find(|game| game.id == game_id)
            .and_then(|game| match &game.launch_target {
                LaunchTarget::Runner {
                    runner_id,
                    profile_id,
                    game_ref,
                } if runner_id == WINE_STAGING_RUNNER_ID => {
                    Some((profile_id.clone(), game_ref.clone()))
                }
                LaunchTarget::Direct | LaunchTarget::Steam { .. } | LaunchTarget::Runner { .. } => {
                    None
                }
            })
            .ok_or_else(|| "This Wine game is no longer available.".to_string())?;
        let inventory = next
            .wine_inventory
            .iter_mut()
            .find(|entry| entry.profile_id == profile_id && entry.game_ref == game_ref)
            .ok_or_else(|| "This Windows game needs to be reimported before it can launch.".to_string())?;
        if inventory.compatibility.graphics.backend != WineGraphicsBackend::Auto
            || inventory.compatibility.prefix_layout
                != catalog::WinePrefixLayout::Isolated
        {
            return Err(
                "This imported game keeps its existing Wine compatibility setup. Launch it again or import it as a new isolated game."
                    .into(),
            );
        }
        let current = inventory.compatibility.last_backend.ok_or_else(|| {
            "Orivo has not selected a compatibility mode for this game yet. Launch it once first."
                .to_string()
        })?;
        if current == WineGraphicsBackend::WineD3d {
            return Err(
                "Wine 3D is already the final compatibility fallback for this game. Launch it again after updating its Wine engine or compatibility runtime."
                    .into(),
            );
        }
        if current == WineGraphicsBackend::Auto
            || inventory.compatibility.rejected_backends.contains(&current)
        {
            return Err("This Wine compatibility retry is no longer available. Launch the game again.".into());
        }
        inventory.compatibility.rejected_backends.push(current);
        inventory.compatibility.last_backend = None;
        persist_catalog(&next, &catalog_path).map_err(|_| {
            "Orivo could not save this compatibility retry. The game was not launched.".to_string()
        })?;
        let mut catalog = catalog_state
            .write()
            .map_err(|_| "The game catalog is temporarily unavailable.".to_string())?;
        *catalog = next;
        Ok(LaunchResult {
            status: "Retrying this game in the next compatible Wine mode".into(),
        })
    })
    .await
    .map_err(|_| "Wine compatibility retry did not finish. Try again.".to_string())?
}

#[tauri::command]
async fn import_wine_games(
    profile_id: String,
    job_id: String,
    game_refs: Vec<String>,
    state: State<'_, AppState>,
) -> Result<WineImportResponse, String> {
    require_wine_runner_platform()?;
    if !valid_wine_opaque_id(&profile_id)
        || !valid_wine_opaque_id(&job_id)
        || game_refs.is_empty()
        || game_refs.len() > MAX_WINE_IMPORT_SELECTION
        || game_refs
            .iter()
            .any(|game_ref| !valid_wine_opaque_id(game_ref))
    {
        return Err("Choose valid Windows games from this Wine import preview.".into());
    }
    let requested = game_refs.into_iter().collect::<BTreeSet<_>>();
    let (candidates, job) = {
        let jobs = state
            .wine_scan_jobs
            .lock()
            .map_err(|_| "Wine import is temporarily unavailable.".to_string())?;
        let job = jobs.get(&job_id).ok_or_else(|| {
            "This Wine import is no longer available. Start it again.".to_string()
        })?;
        if job.profile_id != profile_id {
            return Err("This Wine import does not belong to that profile.".into());
        }
        let scan = job
            .state
            .lock()
            .map_err(|_| "Wine import is temporarily unavailable.".to_string())?;
        if scan.phase != WineScanPhase::Ready {
            return Err("Wait for the Wine scan to finish before importing games.".into());
        }
        (scan.candidates.clone(), Arc::clone(job))
    };
    let candidates_by_ref = candidates
        .into_iter()
        .map(|candidate| (candidate.game_ref.clone(), candidate))
        .collect::<BTreeMap<_, _>>();
    let mut selected = Vec::new();
    let mut skipped_refs = Vec::new();
    for game_ref in requested {
        match candidates_by_ref.get(&game_ref) {
            Some(candidate) => selected.push(candidate.clone()),
            None => skipped_refs.push(game_ref),
        }
    }

    let (profile, direct_games) = {
        let catalog = state
            .catalog
            .read()
            .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
        let profile = catalog
            .wine_profile(&profile_id)
            .cloned()
            .filter(|profile| profile.enabled)
            .ok_or_else(|| "This Wine profile is disabled or no longer available.".to_string())?;
        let direct_games = catalog
            .games
            .iter()
            .filter(|game| is_local_direct_windows_game(game))
            .filter_map(|game| {
                game.executable_path
                    .as_ref()
                    .map(|path| (game.id.clone(), path.clone()))
            })
            .collect::<Vec<_>>();
        (profile, direct_games)
    };
    // A preview is not an authorization grant. Hashing and canonical scope
    // checks happen in a worker before anything is persisted, so a moved or
    // swapped executable cannot become a durable but dead library card.
    let revalidation_profile = profile.clone();
    let revalidation_cancelled = Arc::clone(&job.cancelled);
    let selected = tauri::async_runtime::spawn_blocking(move || {
        // A Direct import is a host-owned prior grant, not a new path coming
        // from the WebView. Resolve it in the same worker as the selected
        // scan candidates so matching it never blocks navigation or expands
        // the selected profile's scopes.
        let mut canonical_direct_games = BTreeMap::new();
        for (game_id, executable) in direct_games {
            if let Ok(executable) = fs::canonicalize(executable) {
                canonical_direct_games.entry(executable).or_insert(game_id);
            }
        }
        selected
            .into_iter()
            .map(|candidate| {
                let candidate = wine_runner::revalidate_wine_import_candidate(
                    &revalidation_profile,
                    &candidate,
                    &revalidation_cancelled,
                )?;
                let origin_direct_game_id = canonical_direct_games
                    .get(&candidate.executable_path)
                    .cloned();
                Ok((candidate, origin_direct_game_id))
            })
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    .map_err(|_| "Wine import validation did not finish. Try the scan again.".to_string())?
    .map_err(|error| match error {
        wine_runner::WineRunnerError::GameOutsideScope => {
            "A selected Windows game is outside this profile's allowed folders. Scan again."
                .to_string()
        }
        wine_runner::WineRunnerError::GameMissing => {
            "A selected Windows game is no longer available. Scan again.".to_string()
        }
        wine_runner::WineRunnerError::GameNotLaunchable => {
            "A selected Windows game changed after the preview. Scan again.".to_string()
        }
        error => error.to_string(),
    })?;

    let _mutation = state
        .catalog_mutation
        .lock()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
    let mut next = state
        .catalog
        .read()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?
        .clone();
    if next.wine_profile(&profile_id) != Some(&profile) {
        return Err(
            "This Wine profile changed while games were being validated. Scan again.".into(),
        );
    }
    let scan_is_ready = job
        .state
        .lock()
        .map_err(|_| "Wine import is temporarily unavailable.".to_string())?
        .phase
        == WineScanPhase::Ready;
    if !scan_is_ready {
        return Err("This Wine import was cancelled. Scan again before importing games.".into());
    }
    let imported_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default();
    let mut imported_ids = Vec::new();
    let mut updated_ids = Vec::new();
    for (candidate, suggested_direct_game_id) in selected {
        let direct_game = suggested_direct_game_id.as_deref().and_then(|game_id| {
            next.games
                .iter()
                .find(|game| game.id == game_id)
                .filter(|game| is_local_direct_windows_game(game))
                .cloned()
        });
        let can_associate_direct = direct_game.as_ref().is_some_and(|direct_game| {
            !next.wine_inventory.iter().any(|entry| {
                entry.origin_direct_game_id.as_deref() == Some(direct_game.id.as_str())
                    && (entry.profile_id != profile_id || entry.game_ref != candidate.game_ref)
            })
        });
        let inventory = WineGameInventoryEntry {
            profile_id: profile_id.clone(),
            game_ref: candidate.game_ref.clone(),
            title: candidate.title.clone(),
            executable_path: candidate.executable_path.clone(),
            fingerprint: candidate.fingerprint.clone(),
            imported_at: Some(imported_at),
            compatibility: WineGameCompatibility::automatic(),
            origin_direct_game_id: can_associate_direct
                .then(|| direct_game.as_ref().map(|game| game.id.clone()))
                .flatten(),
        };
        let game = direct_game
            .as_ref()
            .filter(|_| can_associate_direct)
            .map(|game| wine_catalog_game_from_direct(&profile_id, &candidate, game))
            .unwrap_or_else(|| wine_catalog_game(&profile_id, &candidate));
        let game_id = game.id.clone();
        let inserted = if let Some(direct_game) = direct_game.filter(|_| can_associate_direct) {
            next.associate_direct_game_with_wine_profile(&direct_game.id, inventory, game)
                .map_err(|_| "A selected Windows game could not be imported safely.".to_string())?
        } else {
            next.upsert_wine_inventory(inventory).map_err(|_| {
                "A selected Windows game is no longer inside this profile's allowed folders."
                    .to_string()
            })?;
            next.upsert_runner(game)
                .map_err(|_| "A selected Windows game could not be imported safely.".to_string())?
        };
        if inserted {
            imported_ids.push(game_id);
        } else {
            updated_ids.push(game_id);
        }
    }
    if !imported_ids.is_empty() || !updated_ids.is_empty() {
        if let Some(profile) = next
            .wine_profiles
            .iter_mut()
            .find(|profile| profile.id == profile_id)
        {
            profile.last_imported_at = Some(imported_at);
        }
    }
    persist_catalog(&next, &state.catalog_path).map_err(|_| {
        "Orivo could not save these Wine games. Your library was left unchanged.".to_string()
    })?;
    let mut catalog = state
        .catalog
        .write()
        .map_err(|_| "Wine profiles are temporarily unavailable.".to_string())?;
    *catalog = next;
    Ok(WineImportResponse {
        imported_ids,
        updated_ids,
        skipped_refs,
    })
}

#[tauri::command]
fn get_library(app: AppHandle, state: State<'_, AppState>) -> Result<LibraryState, String> {
    let catalog = state
        .catalog
        .read()
        .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
    Ok(library_state(&app, &catalog))
}

#[tauri::command]
fn import_game(app: AppHandle, state: State<'_, AppState>) -> Result<ImportResponse, String> {
    let Some(executable) = rfd::FileDialog::new()
        .set_title("Import a local game executable")
        .pick_file()
    else {
        let catalog = state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        return Ok(ImportResponse {
            games: library_state(&app, &catalog).games,
            imported_id: None,
        });
    };

    let mut game = Game::from_executable(executable).map_err(|error| error.to_string())?;
    // Media is optional; a damaged image must not prevent a valid executable
    // from entering the library.
    let _ = cache_game_media(&app, &mut game);
    let direct_id = game.id.clone();

    let (response, imported_id) = {
        let _mutation = state
            .catalog_mutation
            .lock()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        let mut next_catalog = state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?
            .clone();
        next_catalog.add(game).map_err(|error| error.to_string())?;
        // A newly imported Windows .exe becomes a Wine-Staging card in the same
        // transaction, so the returned library already shows it as launchable
        // without the user opening any Wine setup.
        auto_apply_wine_to_direct_games(&mut next_catalog, &state.wine_prefix_root);
        // When the .exe was auto-associated, its Direct card is hidden behind a
        // managed Wine runner card with a different id. Surface that runner id so
        // the UI selects the card it can actually see and launch.
        let imported_id = next_catalog
            .wine_inventory
            .iter()
            .find(|entry| entry.origin_direct_game_id.as_deref() == Some(direct_id.as_str()))
            .map(|entry| wine_game_id(&entry.profile_id, &entry.game_ref))
            .unwrap_or_else(|| direct_id.clone());
        persist_catalog(&next_catalog, &state.catalog_path).map_err(|error| error.to_string())?;
        {
            let mut catalog = state
                .catalog
                .write()
                .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
            *catalog = next_catalog;
        }
        let catalog = state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        (library_state(&app, &catalog), imported_id)
    };
    debug_assert!(response.games.iter().any(|game| game.id == imported_id));
    Ok(ImportResponse {
        games: response.games,
        imported_id: Some(imported_id),
    })
}

/// Download best-effort cover/hero art for a game via the keyless Steam Store
/// and persist it onto the catalog record, so both the library card and the
/// detail hero paint it. Used right after a manual import and from the game
/// detail page's "Search cover & images" action.
#[tauri::command]
async fn fetch_game_artwork(
    app: AppHandle,
    state: State<'_, AppState>,
    wallpaper: State<'_, wallpaper_search::WallpaperSearchService>,
    game_id: String,
    force: bool,
) -> Result<(), String> {
    let (title, has_art) = {
        let catalog = state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        let game = catalog
            .games
            .iter()
            .find(|game| game.id == game_id)
            .ok_or_else(|| "This game is no longer in your library.".to_string())?;
        (
            game.title.clone(),
            game.artwork_path.is_some() || game.cover_path.is_some(),
        )
    };
    // The post-import pass (`force = false`) only fills a gap: a game that
    // already found local art keeps it. The detail page's explicit "Search
    // cover & images" passes `force = true` to re-run and replace.
    if has_art && !force {
        return Ok(());
    }
    let url = wallpaper
        .top_artwork_url(&title)
        .await
        .ok_or_else(|| format!("No cover art was found for \u{201c}{title}\u{201d}."))?;
    // Only Steam's own CDNs are trusted for a background download.
    let trusted = ["steamstatic.com", "steampowered.com", "akamai"];
    if !trusted.iter().any(|host| url.contains(host)) {
        return Err("The located artwork came from an untrusted source.".to_string());
    }
    let bytes = download_artwork_bytes(&url).await?;
    let cache_dir = media_cache_dir(&app).map_err(|error| error.to_string())?;
    fs::create_dir_all(&cache_dir).map_err(|error| error.to_string())?;
    let extension = if url
        .split('?')
        .next()
        .unwrap_or(&url)
        .to_ascii_lowercase()
        .ends_with(".png")
    {
        "png"
    } else {
        "jpg"
    };
    let stem = cache_stem(&game_id);
    // A fresh filename each time so the WebView never serves a stale cached
    // image (the asset URL is keyed by path), and it repaints without a restart.
    remove_cached_artwork(&cache_dir, &format!("{stem}-fetched-"));
    let path = cache_dir.join(format!("{stem}-fetched-{}.{extension}", cache_nonce()));
    fs::write(&path, &bytes).map_err(|error| error.to_string())?;

    let _mutation = state
        .catalog_mutation
        .lock()
        .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
    let mut next_catalog = state
        .catalog
        .read()
        .map_err(|_| "The game catalog is temporarily unavailable".to_string())?
        .clone();
    let Some(game) = next_catalog.games.iter_mut().find(|game| game.id == game_id) else {
        return Err("This game is no longer in your library.".to_string());
    };
    game.artwork_path = Some(path.clone());
    game.artwork_source_path = Some(path.clone());
    game.cover_path = Some(path.clone());
    game.cover_source_path = Some(path);
    persist_catalog(&next_catalog, &state.catalog_path).map_err(|error| error.to_string())?;
    {
        let mut catalog = state
            .catalog
            .write()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        *catalog = next_catalog;
    }
    Ok(())
}

async fn download_artwork_bytes(url: &str) -> Result<Vec<u8>, String> {
    const MAX_ARTWORK_BYTES: usize = 16 * 1024 * 1024;
    let client = reqwest::Client::builder()
        .build()
        .map_err(|_| "The artwork download could not be started.".to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "The artwork could not be downloaded.".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "The artwork server returned status {}.",
            response.status()
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "The artwork download was interrupted.".to_string())?;
    if bytes.len() > MAX_ARTWORK_BYTES {
        return Err("The artwork file was too large to store.".to_string());
    }
    Ok(bytes.to_vec())
}

/// A filesystem-safe stem derived from an opaque game id.
fn cache_stem(game_id: &str) -> String {
    game_id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

/// A monotonic-ish suffix so each cached artwork file has a unique path, which
/// forces the WebView to reload it instead of serving a stale cached copy.
fn cache_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0)
}

/// Best-effort removal of a game's prior cached artwork sharing `prefix`, so the
/// cache does not accumulate a copy per change.
fn remove_cached_artwork(cache_dir: &Path, prefix: &str) {
    if let Ok(entries) = fs::read_dir(cache_dir) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(prefix))
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Remove a game from the library. The game's own files are never deleted; this
/// only drops the catalog record so its card and detail page disappear.
#[tauri::command]
fn remove_game(
    app: AppHandle,
    state: State<'_, AppState>,
    game_id: String,
) -> Result<LibraryState, String> {
    {
        let _mutation = state
            .catalog_mutation
            .lock()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        let mut next_catalog = state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?
            .clone();
        if next_catalog
            .remove(&game_id)
            .map_err(|error| error.to_string())?
        {
            persist_catalog(&next_catalog, &state.catalog_path).map_err(|error| error.to_string())?;
            let mut catalog = state
                .catalog
                .write()
                .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
            *catalog = next_catalog;
        }
    }
    let catalog = state
        .catalog
        .read()
        .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
    Ok(library_state(&app, &catalog))
}

/// Promote a chosen media asset to the game's home (Library) background art, so
/// the wallpaper picked on the detail page is the image the home screen paints.
/// The asset is copied into the artwork cache; the game's own files are never
/// touched.
#[tauri::command]
fn set_home_image(
    app: AppHandle,
    state: State<'_, AppState>,
    detail: State<'_, Arc<GameDetailService>>,
    media: State<'_, game_media::GameMediaService>,
    game_id: String,
    media_id: String,
    role: String,
) -> Result<(), String> {
    let asset = detail
        .media_asset(&game_id, &media_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "That image is no longer available.".to_string())?;
    let file = asset
        .local_file
        .ok_or_else(|| "That image has not been downloaded yet.".to_string())?;
    let source = media.media_root().join(&file);
    if !source.is_file() {
        return Err("That image could not be found on disk.".to_string());
    }
    // The chosen image fills one role: the home background, the portrait card
    // cover, or the wide landscape card — each stored independently.
    let prefix = match role.as_str() {
        "cover" => "usercover",
        "landscape" => "landscape",
        _ => "home",
    };
    // Copy the chosen asset into the artwork cache the WebView resolves against.
    let cache_dir = media_cache_dir(&app).map_err(|error| error.to_string())?;
    fs::create_dir_all(&cache_dir).map_err(|error| error.to_string())?;
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("jpg");
    let stem = cache_stem(&game_id);
    // A fresh filename each time so the card repaints live instead of showing
    // the WebView's cached copy of a stable path (no restart needed).
    remove_cached_artwork(&cache_dir, &format!("{stem}-{prefix}-"));
    let dest = cache_dir.join(format!("{stem}-{prefix}-{}.{extension}", cache_nonce()));
    fs::copy(&source, &dest).map_err(|error| error.to_string())?;

    let _mutation = state
        .catalog_mutation
        .lock()
        .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
    let mut next_catalog = state
        .catalog
        .read()
        .map_err(|_| "The game catalog is temporarily unavailable".to_string())?
        .clone();
    {
        let Some(game) = next_catalog.games.iter_mut().find(|game| game.id == game_id) else {
            return Err("This game is no longer in your library.".to_string());
        };
        match role.as_str() {
            "cover" => {
                game.cover_path = Some(dest.clone());
                game.cover_source_path = Some(dest);
            }
            "landscape" => {
                game.landscape_image_path = Some(dest);
            }
            _ => {
                game.home_image_path = Some(dest);
            }
        }
    }
    persist_catalog(&next_catalog, &state.catalog_path).map_err(|error| error.to_string())?;
    {
        let mut catalog = state
            .catalog
            .write()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        *catalog = next_catalog;
    }
    Ok(())
}

/// Scan Steam outside the Tauri command/UI executor. Discovery does no writes,
/// which keeps opening the panel inexpensive and preserves the current library
/// if Steam is mid-update or one manifest happens to be malformed.
#[tauri::command]
async fn get_steam_import_preview(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SteamImportPreview, String> {
    let imported = {
        let catalog = state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        imported_steam_games(&catalog)
    };
    let discovery = scan_steam_in_worker().await?;
    remember_steam_preview(&state, &discovery);
    Ok(steam_preview(&app, discovery, &imported))
}

/// Hydrate a small visible slice of artwork only after the import panel has
/// rendered. IDs are resolved through a short-lived Rust-only discovery
/// snapshot (or a fresh scan when that snapshot expires), so the WebView still
/// cannot turn this into arbitrary filesystem access.
#[tauri::command]
async fn get_steam_preview_media(
    app_ids: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<SteamPreviewMediaView>, String> {
    let requested = requested_steam_preview_app_ids(app_ids)?;
    let games = match steam_preview_snapshot_games(&state, &requested)? {
        Some(games) => games,
        None => scan_steam_in_worker()
            .await?
            .games
            .into_iter()
            .filter(|game| requested.contains(&game.app_id))
            .collect(),
    };
    let media = cache_steam_preview_media_in_worker(app.clone(), games).await;
    Ok(steam_preview_media_views(&app, media))
}

/// Import receives only opaque app ids. It deliberately scans Steam again and
/// rejects ids not present in that fresh, fully-installed local discovery.
/// Thus the WebView cannot nominate a path, title, artwork, command, or URL.
#[tauri::command]
async fn import_steam_games(
    app_ids: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SteamImportResponse, String> {
    let requested = requested_steam_app_ids(app_ids)?;
    let discovery = scan_steam_in_worker().await?;
    let discovered = discovery
        .games
        .into_iter()
        .map(|game| (game.app_id, game))
        .collect::<BTreeMap<_, _>>();

    let mut skipped_app_ids = Vec::new();
    let selected = requested
        .into_iter()
        .filter_map(|app_id| match discovered.get(&app_id) {
            Some(game) => Some(game.clone()),
            None => {
                skipped_app_ids.push(app_id.to_string());
                None
            }
        })
        .collect::<Vec<_>>();

    // Copies are intentionally performed before catalog mutation begins.
    // A large Steam library may take time to cache, but rail navigation and
    // reads remain available throughout that work.
    // Store metadata is public and independent from the local cache copy, so
    // resolve both together. A bounded request fan-out keeps an import of a
    // larger selection responsive without needing an Orivo backend.
    let selected_app_ids = selected
        .iter()
        .map(|steam_game| steam_game.app_id)
        .collect::<Vec<_>>();
    let metadata_request = fetch_steam_store_metadata_with_budget(selected_app_ids);
    let staged_request = cache_steam_games_in_worker(app.clone(), selected);
    let (store_metadata, staged) =
        futures_util::future::join(metadata_request, staged_request).await;
    let mut staged = staged?;
    for (app_id, game) in &mut staged {
        apply_steam_store_metadata(game, store_metadata.get(app_id));
    }
    let mut imported_ids = Vec::new();
    let mut updated_ids = Vec::new();
    {
        let _mutation = state
            .catalog_mutation
            .lock()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        let mut next_catalog = state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?
            .clone();
        for (app_id, game) in staged {
            if next_catalog
                .upsert_steam(game)
                .map_err(|error| error.to_string())?
            {
                imported_ids.push(app_id.to_string());
            } else {
                updated_ids.push(app_id.to_string());
            }
        }
        if !imported_ids.is_empty() || !updated_ids.is_empty() {
            persist_catalog(&next_catalog, &state.catalog_path)
                .map_err(|error| error.to_string())?;
            let mut catalog = state
                .catalog
                .write()
                .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
            *catalog = next_catalog;
        }
    }

    Ok(SteamImportResponse {
        imported_ids,
        updated_ids,
        skipped_app_ids,
    })
}

/// Return only the connection metadata safe to show in the main WebView. The
/// Steam credential itself remains in the system keychain and is never part of
/// an IPC response.
#[tauri::command]
async fn get_steam_account_status() -> Result<steam_account::SteamAccountStatus, String> {
    tauri::async_runtime::spawn_blocking(steam_account::account_status)
        .await
        .map_err(|error| format!("Steam account status did not finish: {error}"))?
        .map_err(|error| error.to_string())
}

/// Open the same local-first Steam sign-in pattern used by desktop launchers:
/// a dedicated Steam-only WebView, then a direct local sync. There is no
/// Orivo server, callback URL, password collection, or token IPC channel.
#[tauri::command]
fn begin_steam_web_login(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(STEAM_AUTH_WINDOW_LABEL) {
        window
            .show()
            .and_then(|_| window.set_focus())
            .map_err(|error| format!("Steam sign-in window could not be focused: {error}"))?;
        return Ok(());
    }

    let initial_url =
        Url::parse(STEAM_EXPLORE_URL).map_err(|_| "Steam sign-in URL is invalid".to_string())?;
    let settled = Arc::new(AtomicBool::new(false));
    let settled_for_page_load = Arc::clone(&settled);
    let app_for_page_load = app.clone();

    // Steam Guard can embed HTTPS challenges outside Steam's own domains. The
    // sign-in WebView has no IPC capabilities and extraction stays pinned to
    // the exact Steam Store host below, so permitting HTTPS navigation keeps
    // the auth flow functional without granting the page any Orivo access.

    let window = WebviewWindowBuilder::new(
        &app,
        STEAM_AUTH_WINDOW_LABEL,
        WebviewUrl::External(initial_url),
    )
    .title("Connect Steam to Orivo")
    .inner_size(640.0, 760.0)
    .min_inner_size(460.0, 580.0)
    .center()
    // The credential lives in Keychain instead. A non-persistent WebView
    // avoids leaving a browser session behind in Orivo's app data.
    .incognito(true)
    .on_navigation(is_allowed_steam_auth_navigation)
    .on_new_window(|_, _| NewWindowResponse::Deny)
    .on_page_load(move |window, payload| {
        if payload.event() != PageLoadEvent::Finished
            || !is_steam_store_page(payload.url())
            || settled_for_page_load.load(Ordering::Acquire)
        {
            return;
        }
        let _ = attempt_steam_web_login(
            &app_for_page_load,
            &window,
            Arc::clone(&settled_for_page_load),
            false,
        );
    })
    .build()
    .map_err(|error| format!("Steam sign-in window could not open: {error}"))?;

    *state
        .steam_auth_settled
        .lock()
        .map_err(|_| "Steam sign-in state is temporarily unavailable".to_string())? =
        Some(Arc::clone(&settled));

    let settled_for_close = Arc::clone(&settled);
    let app_for_close = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed)
            && settled_for_close
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            let _ =
                app_for_close.emit_to(MAIN_WINDOW_LABEL, STEAM_ACCOUNT_LOGIN_CANCELLED_EVENT, ());
        }
    });

    Ok(())
}

/// Steam occasionally finishes authentication without another full page load.
/// The explicit confirmation lets the user re-check the current Store page
/// instead of waiting indefinitely when that happens.
#[tauri::command]
fn complete_steam_web_login(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let settled = state
        .steam_auth_settled
        .lock()
        .map_err(|_| "Steam sign-in state is temporarily unavailable".to_string())?
        .clone()
        .ok_or_else(|| "Open the Steam sign-in window first.".to_string())?;
    let window = app
        .get_webview_window(STEAM_AUTH_WINDOW_LABEL)
        .ok_or_else(|| "The Steam sign-in window is no longer open.".to_string())?;
    let url = window.url().map_err(|_| {
        "Steam sign-in could not be checked. Return to the Steam Store window.".to_string()
    })?;
    if !is_steam_store_page(&url) {
        return Err("Finish sign-in on the Steam Store page, then try again.".into());
    }
    attempt_steam_web_login(&app, &window, settled, true)
}

#[tauri::command]
fn cancel_steam_web_login(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(STEAM_AUTH_WINDOW_LABEL) {
        window
            .close()
            .map_err(|error| format!("Steam sign-in window could not close: {error}"))?;
    }
    Ok(())
}

fn attempt_steam_web_login(
    app: &AppHandle,
    window: &WebviewWindow,
    settled: Arc<AtomicBool>,
    notify_if_pending: bool,
) -> Result<(), String> {
    let app_for_callback = app.clone();
    let login_window = window.clone();
    window
        .eval_with_callback(STEAM_LOGIN_EXTRACTION_SCRIPT, move |raw_result| {
            let Some((steam_id, access_token)) = steam_account::web_login_from_eval(&raw_result)
            else {
                if notify_if_pending {
                    let _ = app_for_callback.emit_to(
                        MAIN_WINDOW_LABEL,
                        STEAM_ACCOUNT_LOGIN_PENDING_EVENT,
                        (),
                    );
                }
                return;
            };
            if settled
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }

            match steam_account::save_web_login(steam_id.clone(), access_token) {
                Ok(()) => {
                    let _ = app_for_callback.emit_to(
                        MAIN_WINDOW_LABEL,
                        STEAM_ACCOUNT_CONNECTED_EVENT,
                        steam_account::SteamAccountConnectedEvent { steam_id },
                    );
                    let _ = login_window.close();
                }
                Err(error) => {
                    // Saving failed, so keep the sign-in window available and
                    // allow a later page load to retry without exposing a
                    // secret or an OS error string to the WebView.
                    settled.store(false, Ordering::Release);
                    let _ = app_for_callback.emit_to(
                        MAIN_WINDOW_LABEL,
                        STEAM_ACCOUNT_LOGIN_FAILED_EVENT,
                        error.to_string(),
                    );
                }
            }
        })
        .map_err(|_| "Steam sign-in could not be checked. Try again in the Steam window.".into())
}

#[tauri::command]
async fn connect_steam_with_api_key(
    steam_id: String,
    api_key: String,
) -> Result<steam_account::SteamAccountStatus, String> {
    steam_account::connect_api_key(steam_id, api_key)
        .await
        .map_err(|error| error.to_string())
}

/// Pull all owned games directly from Steam, then join their AppIDs with the
/// local manifest scanner. The remote inventory remains useful without Steam
/// installed; only the joined records are marked as ready to launch.
#[tauri::command]
async fn sync_steam_account_library(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SteamAccountSyncResponse, String> {
    let owned_games = steam_account::fetch_owned_games()
        .await
        .map_err(|error| error.to_string())?;
    let metadata_app_ids = steam_games_needing_store_metadata(&state, &owned_games)?;
    // Public Store details and local manifest discovery do not depend on one
    // another. Running them in parallel avoids making the account modal wait
    // for the full sum of both operations.
    let metadata_request = fetch_steam_store_metadata_with_budget(metadata_app_ids);
    let discovery_request = scan_steam_in_worker();
    let (store_metadata, discovery) =
        futures_util::future::join(metadata_request, discovery_request).await;
    let discovery = discovery?;
    let prepared =
        prepare_steam_account_games_in_worker(app.clone(), owned_games, discovery, store_metadata)
            .await?;

    let mut imported_games = 0;
    let mut updated_games = 0;
    {
        let _mutation = state
            .catalog_mutation
            .lock()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        let mut next_catalog = state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?
            .clone();
        for game in prepared.games {
            if next_catalog
                .upsert_steam(game)
                .map_err(|error| error.to_string())?
            {
                imported_games += 1;
            } else {
                updated_games += 1;
            }
        }
        if imported_games > 0 || updated_games > 0 {
            persist_catalog(&next_catalog, &state.catalog_path)
                .map_err(|error| error.to_string())?;
            let mut catalog = state
                .catalog
                .write()
                .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
            *catalog = next_catalog;
        }
    }

    Ok(SteamAccountSyncResponse {
        total_games: prepared.total_games,
        imported_games,
        updated_games,
        installed_games: prepared.installed_games,
    })
}

async fn fetch_steam_store_metadata_with_budget(
    app_ids: Vec<u32>,
) -> BTreeMap<u32, steam_account::SteamStoreGameMetadata> {
    match tokio::time::timeout(
        STEAM_STORE_METADATA_SYNC_BUDGET,
        steam_account::fetch_store_metadata(app_ids),
    )
    .await
    {
        Ok(metadata) => metadata,
        Err(_) => {
            eprintln!(
                "Steam Store metadata exceeded the {} second sync budget; continuing without it",
                STEAM_STORE_METADATA_SYNC_BUDGET.as_secs()
            );
            BTreeMap::new()
        }
    }
}

#[tauri::command]
async fn disconnect_steam_account() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(steam_account::disconnect)
        .await
        .map_err(|error| format!("Steam disconnect did not finish: {error}"))?
        .map_err(|error| error.to_string())
}

fn monitor_wine_startup(
    app: AppHandle,
    game_id: String,
    graphics_label: &'static str,
    mut child: std::process::Child,
) {
    thread::spawn(move || {
        let deadline = Instant::now() + WINE_EARLY_EXIT_WINDOW;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    let _ = app.emit_to(
            MAIN_WINDOW_LABEL,
                        WINE_LAUNCH_STATUS_EVENT,
                        WineLaunchStatusEvent {
                            game_id,
                            phase: "failed",
                            message: "Wine stopped before this game finished starting. Retry in compatibility mode from Orivo."
                                .into(),
                        },
                    );
                    return;
                }
                Err(_) => {
                    let _ = app.emit_to(
            MAIN_WINDOW_LABEL,
                        WINE_LAUNCH_STATUS_EVENT,
                        WineLaunchStatusEvent {
                            game_id,
                            phase: "failed",
                            message: "Wine could not confirm this game started. Retry in compatibility mode from Orivo."
                                .into(),
                        },
                    );
                    return;
                }
                Ok(None) if Instant::now() >= deadline => {
                    let _ = app.emit_to(
                        MAIN_WINDOW_LABEL,
                        WINE_LAUNCH_STATUS_EVENT,
                        WineLaunchStatusEvent {
                            game_id,
                            phase: "started",
                            message: format!(
                                "Wine-Staging is running this game with {graphics_label}."
                            ),
                        },
                    );
                    // Reap the direct Wine process once it exits, but do not
                    // turn a normal end-of-session into a launch failure.
                    let _ = child.wait();
                    return;
                }
                Ok(None) => thread::sleep(Duration::from_millis(75)),
            }
        }
    });
}

#[tauri::command]
async fn launch_game(
    app: AppHandle,
    game_id: String,
    state: State<'_, AppState>,
) -> Result<LaunchResult, String> {
    let (game, mut wine_launch) = {
        let catalog = state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        let game = catalog
            .games
            .iter()
            .find(|game| game.id == game_id)
            .cloned()
            .ok_or_else(|| {
                "This is a visual showcase. Import a local game to launch it.".to_string()
            })?;
        let wine_launch = match &game.launch_target {
            LaunchTarget::Runner {
                runner_id,
                profile_id,
                game_ref,
            } if runner_id == WINE_STAGING_RUNNER_ID => {
                let profile = catalog.wine_profile(profile_id).cloned().ok_or_else(|| {
                    "This Wine profile is no longer available. Review its setup and try again."
                        .to_string()
                })?;
                let inventory = catalog
                    .wine_inventory_entry(profile_id, game_ref)
                    .cloned()
                    .ok_or_else(|| {
                        "This Windows game needs to be reimported before it can launch.".to_string()
                    })?;
                Some((profile, inventory, game_ref.clone()))
            }
            LaunchTarget::Direct | LaunchTarget::Steam { .. } | LaunchTarget::Runner { .. } => None,
        };
        (game, wine_launch)
    };
    let title = game.title.clone();

    // A Windows executable must never fall through to the generic direct
    // launcher on macOS. Orivo applies Wine-Staging to every local .exe
    // automatically (at import, at startup, and whenever the library reloads),
    // so a game only stays a Direct record here when no Wine-Staging engine was
    // detected. This host-side check also keeps a forged/stale WebView request
    // from invoking a Windows binary as a native program.
    if cfg!(target_os = "macos") && is_local_direct_windows_game(&game) {
        return Err(
            "This is a Windows game. Install Wine-Staging so Orivo can run it automatically."
                .into(),
        );
    }

    if wine_launch.is_some() {
        require_wine_runner_platform()?;
        let prepared_dxvk = match wine_launch.as_ref() {
            Some((profile, inventory, _)) => {
                ensure_dxvk_macos_for_automatic_game(&app, &game.id, profile, inventory, &state)
                    .await?
            }
            None => false,
        };

        // Preparing DXVK persists a new profile state and resets the closed
        // Auto choice for eligible games. Reload the host-owned records
        // before taking the launch lease, rather than launching with stale
        // clones that could race a disable/delete from Settings.
        if prepared_dxvk {
            let (profile_id, game_ref) = wine_launch
                .as_ref()
                .map(|(profile, _, game_ref)| (profile.id.clone(), game_ref.clone()))
                .ok_or_else(|| {
                    "This Wine profile is no longer available. Review its setup and try again."
                        .to_string()
                })?;
            let catalog = state
                .catalog
                .read()
                .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
            let profile = catalog.wine_profile(&profile_id).cloned().ok_or_else(|| {
                "This Wine profile is no longer available. Review its setup and try again."
                    .to_string()
            })?;
            let inventory = catalog
                .wine_inventory_entry(&profile_id, &game_ref)
                .cloned()
                .ok_or_else(|| {
                    "This Windows game needs to be reimported before it can launch.".to_string()
                })?;
            wine_launch = Some((profile, inventory, game_ref));
        }
    }

    if let Some((profile, inventory, game_ref)) = wine_launch {
        let prefix_root = state.wine_prefix_root.clone();
        let catalog_path = state.catalog_path.clone();
        let current_catalog = Arc::clone(&state.catalog);
        let mutation = Arc::clone(&state.catalog_mutation);
        let app_for_monitor = app.clone();
        let game_id_for_monitor = game.id.clone();
        let (selected_backend, macos_retina_mode_enabled) =
            tauri::async_runtime::spawn_blocking(move || {
                // Prefix mutation (DXVK initialization/copy) and a launch cannot
                // overlap. This lease is acquired in the worker, so the WebView
                // remains responsive while a conflicting operation finishes.
                let _mutation = mutation
                    .lock()
                    .map_err(|_| wine_runner::WineRunnerError::InvalidProfile)?;
                // Wine-Staging is a trusted native reference adapter, not a
                // Wasm component. It constructs the same typed launch-intent
                // shape that WIT runner `prepare-launch` describes, directly
                // from catalog-owned opaque IDs. No WIT mode string, path, or
                // argument crosses this boundary.
                let intent = wine_runner::WineLaunchIntent::new(&profile.id, &game_ref)?;
                let prepared =
                    wine_runner::prepare_wine_launch(&profile, &inventory, &intent, &prefix_root)?;
                let selected_backend = prepared.graphics_backend();
                let macos_retina_mode_enabled = prepared.macos_retina_mode_enabled();
                // A concurrent disable/delete needs the mutation lease, so it
                // cannot revoke this profile between the final authorization
                // check and process start. Persist the closed selected backend
                // before spawning so an explicit retry can advance predictably
                // after an app restart; no WebView field selects that backend.
                let catalog = current_catalog
                    .read()
                    .map_err(|_| wine_runner::WineRunnerError::InvalidProfile)?;
                let profile_is_current = catalog.wine_profile(&profile.id) == Some(&profile);
                let inventory_is_current = catalog
                    .wine_inventory_entry(&profile.id, &inventory.game_ref)
                    == Some(&inventory);
                if !profile_is_current || !inventory_is_current {
                    return Err(wine_runner::WineRunnerError::InvalidProfile);
                }
                drop(catalog);
                let mut next = current_catalog
                    .read()
                    .map_err(|_| wine_runner::WineRunnerError::InvalidProfile)?
                    .clone();
                let current_profile = next
                    .wine_profiles
                    .iter_mut()
                    .find(|candidate| candidate.id == profile.id)
                    .ok_or(wine_runner::WineRunnerError::InvalidProfile)?;
                if *current_profile != profile {
                    return Err(wine_runner::WineRunnerError::InvalidProfile);
                }
                current_profile.macos_retina_mode_enabled = macos_retina_mode_enabled;
                let current_inventory = next
                    .wine_inventory
                    .iter_mut()
                    .find(|entry| {
                        entry.profile_id == inventory.profile_id
                            && entry.game_ref == inventory.game_ref
                    })
                    .ok_or(wine_runner::WineRunnerError::InvalidProfile)?;
                if *current_inventory != inventory {
                    return Err(wine_runner::WineRunnerError::InvalidProfile);
                }
                current_inventory.compatibility.last_backend = Some(selected_backend);
                persist_catalog(&next, &catalog_path)
                    .map_err(|_| wine_runner::WineRunnerError::InvalidProfile)?;
                let mut catalog = current_catalog
                    .write()
                    .map_err(|_| wine_runner::WineRunnerError::InvalidProfile)?;
                *catalog = next;
                drop(catalog);
                let child = prepared.spawn()?;
                monitor_wine_startup(
                    app_for_monitor,
                    game_id_for_monitor,
                    wine_graphics_backend_label(selected_backend),
                    child,
                );
                Ok((selected_backend, macos_retina_mode_enabled))
            })
            .await
            .map_err(|_| "Wine launch did not finish. Try again.".to_string())?
            .map_err(|error| error.to_string())?;
        let retina_status =
            matches!(macos_retina_mode_enabled, Some(true)).then_some(" · affichage Retina natif");
        return Ok(LaunchResult {
            status: format!(
                "Launching {title} with Wine-Staging · {}{}",
                wine_graphics_backend_label(selected_backend),
                retina_status.unwrap_or_default(),
            ),
        });
    }

    // `/usr/bin/open` can briefly wait while macOS checks whether the Steam
    // bundle accepted the URI. Keep that confirmation off the command/UI
    // executor, while direct launches still return once their process starts.
    tauri::async_runtime::spawn_blocking(move || launcher::launch(&game))
        .await
        .map_err(|error| format!("Launch request did not finish: {error}"))?
        .map_err(|error| error.to_string())?;
    Ok(LaunchResult {
        status: format!("Launching {title}"),
    })
}

/// Ask the locally installed Steam client to begin an install for one owned
/// catalog entry. The WebView still supplies only a stable id; Rust validates
/// the source and constructs the fixed Steam URI itself.
#[tauri::command]
async fn install_steam_game(
    game_id: String,
    state: State<'_, AppState>,
) -> Result<LaunchResult, String> {
    let game = {
        state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?
            .games
            .iter()
            .find(|game| game.id == game_id)
            .cloned()
            .ok_or_else(|| {
                "This game is no longer in your library. Refresh Steam and try again.".to_string()
            })?
    };
    let title = game.title.clone();
    let app_id = match game.launch_target {
        LaunchTarget::Steam { app_id } => app_id,
        LaunchTarget::Direct | LaunchTarget::Runner { .. } => {
            return Err("Only Steam library games can be installed from Orivo.".into());
        }
    };

    tauri::async_runtime::spawn_blocking(move || launcher::install_steam(app_id))
        .await
        .map_err(|error| format!("Steam install request did not finish: {error}"))?
        .map_err(|error| error.to_string())?;
    Ok(LaunchResult {
        status: format!("Opening Steam to install {title}"),
    })
}

async fn scan_steam_in_worker() -> Result<steam::SteamDiscovery, String> {
    tauri::async_runtime::spawn_blocking(steam::discover_default)
        .await
        .map_err(|error| format!("Steam scan did not finish: {error}"))
}

#[derive(Debug)]
struct PreparedSteamAccountGames {
    games: Vec<Game>,
    total_games: usize,
    installed_games: usize,
}

async fn prepare_steam_account_games_in_worker(
    app: AppHandle,
    owned_games: Vec<steam_account::OwnedSteamGame>,
    discovery: steam::SteamDiscovery,
    store_metadata: BTreeMap<u32, steam_account::SteamStoreGameMetadata>,
) -> Result<PreparedSteamAccountGames, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let total_games = owned_games.len();
        let mut installed_by_app_id = discovery
            .games
            .into_iter()
            .map(|game| (game.app_id, game))
            .collect::<BTreeMap<_, _>>();
        let mut budget = MediaCacheBudget::new();
        let mut installed_games = 0;
        let mut games = Vec::with_capacity(total_games);

        for owned_game in owned_games {
            let installed_game = installed_by_app_id.remove(&owned_game.app_id);
            if installed_game.is_some() {
                installed_games += 1;
            }
            let metadata = store_metadata.get(&owned_game.app_id);
            let mut game = owned_steam_game_to_catalog_game(owned_game, installed_game);
            apply_steam_store_metadata(&mut game, metadata);
            // Local artwork remains an optional visual enhancement. Never let
            // a cache copy delay or prevent an account-library sync.
            if game.installation_path.is_some() {
                let _ = cache_game_media_with_budget(&app, &mut game, &mut budget);
            }
            games.push(game);
        }

        Ok(PreparedSteamAccountGames {
            games,
            total_games,
            installed_games,
        })
    })
    .await
    .map_err(|error| format!("Steam library preparation did not finish: {error}"))?
}

fn owned_steam_game_to_catalog_game(
    owned_game: steam_account::OwnedSteamGame,
    installed_game: Option<steam::SteamGame>,
) -> Game {
    let app_id = owned_game.app_id;
    let (installation_path, artwork_source_path, cover_source_path) = match installed_game {
        Some(installed_game) => (
            Some(installed_game.installation_path),
            installed_game.hero_path,
            installed_game.cover_path,
        ),
        None => (None, None, None),
    };
    let installed = installation_path.is_some();

    Game {
        id: format!("steam:{app_id}"),
        title: owned_game.title,
        executable_path: None,
        source: GameSource::Steam,
        source_id: Some(app_id.to_string()),
        launch_target: LaunchTarget::Steam { app_id },
        installation_path,
        working_directory: None,
        arguments: Vec::new(),
        description: Some(
            if installed {
                "Installed through Steam. Ready for your next session."
            } else {
                "Owned on Steam. Install it in Steam to play."
            }
            .into(),
        ),
        metadata: Some(
            if installed {
                "Installed"
            } else {
                "Not installed"
            }
            .into(),
        ),
        artwork_path: None,
        artwork_source_path,
        cover_path: None,
        cover_source_path,
        home_image_path: None,
        landscape_image_path: None,
        logo_path: None,
        hero_video_path: None,
        last_played_at: None,
        play_time_seconds: owned_game.play_time_seconds,
        extra: BTreeMap::new(),
    }
}

fn steam_games_needing_store_metadata(
    state: &AppState,
    owned_games: &[steam_account::OwnedSteamGame],
) -> Result<Vec<u32>, String> {
    let catalog = state
        .catalog
        .read()
        .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
    let cached = catalog
        .games
        .iter()
        .filter_map(|game| match (&game.source, &game.launch_target) {
            (GameSource::Steam, LaunchTarget::Steam { app_id })
                if game
                    .extra
                    .contains_key(catalog::STEAM_STORE_METADATA_MARKER) =>
            {
                Some(*app_id)
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    Ok(owned_games
        .iter()
        .map(|game| game.app_id)
        .filter(|app_id| !cached.contains(app_id))
        .collect())
}

fn apply_steam_store_metadata(
    game: &mut Game,
    metadata: Option<&steam_account::SteamStoreGameMetadata>,
) {
    let Some(metadata) = metadata else {
        return;
    };

    if let Some(description) = metadata.short_description.as_ref() {
        game.description = Some(description.clone());
    }
    if let Some(genre) = metadata.genre.as_ref() {
        game.extra.insert(
            catalog::STEAM_STORE_GENRE_KEY.into(),
            serde_json::Value::String(genre.clone()),
        );
    }
    if let Some(platforms) = metadata.platforms {
        let supported = [
            ("windows", platforms.windows),
            ("macos", platforms.macos),
            ("linux", platforms.linux),
        ]
        .into_iter()
        .filter_map(|(platform, supported)| {
            supported.then_some(serde_json::Value::String(platform.into()))
        })
        .collect();
        game.extra.insert(
            catalog::STEAM_STORE_PLATFORMS_KEY.into(),
            serde_json::Value::Array(supported),
        );
    }
    game.extra.insert(
        catalog::STEAM_STORE_METADATA_MARKER.into(),
        serde_json::Value::Bool(true),
    );
}

fn is_allowed_steam_auth_navigation(url: &Url) -> bool {
    // This is deliberately broader than the extraction check: Steam Guard
    // and CAPTCHA providers may use an HTTPS challenge iframe or redirect.
    // The WebView is capability-free and a credential is accepted only after
    // a completed `store.steampowered.com` page load.
    url.scheme() == "https"
}

fn is_steam_store_page(url: &Url) -> bool {
    url.scheme() == "https" && url.host_str() == Some("store.steampowered.com")
}

fn remember_steam_preview(state: &AppState, discovery: &steam::SteamDiscovery) {
    let snapshot = SteamPreviewSnapshot {
        captured_at: Instant::now(),
        games: discovery
            .games
            .iter()
            .map(|game| (game.app_id, game.clone()))
            .collect(),
    };
    if let Ok(mut stored) = state.steam_preview.lock() {
        *stored = Some(snapshot);
    }
}

fn steam_preview_snapshot_games(
    state: &AppState,
    requested: &BTreeSet<u32>,
) -> Result<Option<Vec<steam::SteamGame>>, String> {
    let mut stored = state
        .steam_preview
        .lock()
        .map_err(|_| "Steam preview is temporarily unavailable".to_string())?;
    let Some(snapshot) = stored.as_ref() else {
        return Ok(None);
    };
    if snapshot.captured_at.elapsed() > STEAM_PREVIEW_SNAPSHOT_TTL {
        *stored = None;
        return Ok(None);
    }
    Ok(Some(
        requested
            .iter()
            .filter_map(|app_id| snapshot.games.get(app_id).cloned())
            .collect(),
    ))
}

async fn cache_steam_games_in_worker(
    app: AppHandle,
    games: Vec<steam::SteamGame>,
) -> Result<Vec<(u32, Game)>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut budget = MediaCacheBudget::new();
        games
            .into_iter()
            .map(|steam_game| {
                let app_id = steam_game.app_id;
                let mut game = steam_game_to_catalog_game(steam_game);
                // Artwork is an optional enhancement. A permission error or
                // damaged image must not make the source record unimportable.
                let _ = cache_game_media_with_budget(&app, &mut game, &mut budget);
                (app_id, game)
            })
            .collect()
    })
    .await
    .map_err(|error| format!("Steam import preparation did not finish: {error}"))
}

async fn cache_steam_preview_media_in_worker(
    app: AppHandle,
    games: Vec<steam::SteamGame>,
) -> BTreeMap<u32, SteamPreviewMedia> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut budget = MediaCacheBudget::new();
        games
            .into_iter()
            .filter_map(|steam_game| {
                let app_id = steam_game.app_id;
                let mut game = steam_game_to_catalog_game(steam_game);
                cache_game_media_with_budget(&app, &mut game, &mut budget).ok()?;
                Some((
                    app_id,
                    SteamPreviewMedia {
                        cover_path: game.cover_path,
                        hero_path: game.artwork_path,
                    },
                ))
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

fn requested_steam_app_ids(app_ids: Vec<String>) -> Result<BTreeSet<u32>, String> {
    if app_ids.len() > MAX_STEAM_IMPORT_SELECTION {
        return Err(format!(
            "Choose at most {MAX_STEAM_IMPORT_SELECTION} Steam games at a time"
        ));
    }

    let mut result = BTreeSet::new();
    for app_id in app_ids {
        let parsed = app_id
            .parse::<u32>()
            .ok()
            .filter(|app_id| *app_id > 0)
            .ok_or_else(|| "Steam import received an invalid game id".to_string())?;
        result.insert(parsed);
    }
    if result.is_empty() {
        return Err("Choose at least one Steam game to import".into());
    }
    Ok(result)
}

fn requested_steam_preview_app_ids(app_ids: Vec<String>) -> Result<BTreeSet<u32>, String> {
    if app_ids.len() > MAX_STEAM_PREVIEW_MEDIA {
        return Err(format!(
            "Preview at most {MAX_STEAM_PREVIEW_MEDIA} Steam games at a time"
        ));
    }
    requested_steam_app_ids(app_ids)
}

fn imported_steam_games(catalog: &Catalog) -> BTreeMap<u32, Game> {
    catalog
        .games
        .iter()
        .filter_map(|game| match (&game.source, &game.launch_target) {
            (GameSource::Steam, LaunchTarget::Steam { app_id }) => Some((*app_id, game.clone())),
            _ => None,
        })
        .collect()
}

fn steam_preview(
    app: &AppHandle,
    discovery: steam::SteamDiscovery,
    imported: &BTreeMap<u32, Game>,
) -> SteamImportPreview {
    if discovery.steam_root.is_none() {
        return SteamImportPreview {
            status: "unavailable",
            libraries: 0,
            games: Vec::new(),
            message: "Steam was not found locally. Install Steam or open it once, then scan again."
                .into(),
        };
    }

    let cache_dir = media_cache_dir(app).ok();
    let games = discovery
        .games
        .into_iter()
        .map(|steam_game| {
            let app_id = steam_game.app_id;
            steam_preview_game(steam_game, imported.get(&app_id), cache_dir.as_deref())
        })
        .collect();

    let message = if discovery.issues.is_empty() {
        String::new()
    } else {
        format!(
            "{} Steam item{} could not be read and were skipped.",
            discovery.issues.len(),
            if discovery.issues.len() == 1 { "" } else { "s" }
        )
    };
    SteamImportPreview {
        status: "available",
        libraries: discovery.libraries.len(),
        games,
        message,
    }
}

fn steam_preview_game(
    steam_game: steam::SteamGame,
    imported_game: Option<&Game>,
    cache_dir: Option<&Path>,
) -> SteamPreviewGame {
    // Preview assets are returned only if they are already inside Orivo's
    // scoped cache. Source asset paths remain Rust-only, including for games
    // that have not been imported yet.
    let cover_url =
        imported_game.and_then(|game| media_source_url(game.cover_path.as_deref(), cache_dir));
    let hero_url =
        imported_game.and_then(|game| media_source_url(game.artwork_path.as_deref(), cache_dir));
    SteamPreviewGame {
        app_id: steam_game.app_id.to_string(),
        title: steam_game.title,
        location_label: "Installed locally",
        // Steam's manifest timestamp is an update timestamp, not a play
        // session. Do not misrepresent it as user activity.
        last_updated: String::new(),
        selected: imported_game.is_none(),
        already_imported: imported_game.is_some(),
        cover_url,
        hero_url,
    }
}

fn steam_preview_media_views(
    app: &AppHandle,
    media: BTreeMap<u32, SteamPreviewMedia>,
) -> Vec<SteamPreviewMediaView> {
    let cache_dir = media_cache_dir(app).ok();
    media
        .into_iter()
        .filter_map(|(app_id, media)| {
            let cover_url = media_source_url(media.cover_path.as_deref(), cache_dir.as_deref());
            let hero_url = media_source_url(media.hero_path.as_deref(), cache_dir.as_deref());
            (cover_url.is_some() || hero_url.is_some()).then_some(SteamPreviewMediaView {
                app_id: app_id.to_string(),
                cover_url,
                hero_url,
            })
        })
        .collect()
}

fn steam_game_to_catalog_game(steam_game: steam::SteamGame) -> Game {
    let app_id = steam_game.app_id;
    Game {
        id: format!("steam:{app_id}"),
        title: steam_game.title,
        executable_path: None,
        source: GameSource::Steam,
        source_id: Some(app_id.to_string()),
        launch_target: LaunchTarget::Steam { app_id },
        installation_path: Some(steam_game.installation_path),
        working_directory: None,
        arguments: Vec::new(),
        description: Some("Installed through Steam. Ready for your next session.".into()),
        metadata: Some("Steam · Installed".into()),
        artwork_path: None,
        artwork_source_path: steam_game.hero_path,
        cover_path: None,
        cover_source_path: steam_game.cover_path,
        home_image_path: None,
        landscape_image_path: None,
        logo_path: None,
        hero_video_path: None,
        last_played_at: None,
        play_time_seconds: 0,
        extra: BTreeMap::new(),
    }
}

fn resolved_catalog_path(app: &AppHandle) -> Result<PathBuf, tauri::Error> {
    if let Some(path) = std::env::var_os("ORIVO_CATALOG_PATH") {
        return Ok(PathBuf::from(path));
    }
    Ok(app.path().app_data_dir()?.join(CATALOG_FILE))
}

/// A migration that rewrites game ids also has to move the wishlist, media
/// selections and imported files that are keyed by them, so the catalog and the
/// game-state document are published as one unit rather than one at a time.
fn load_or_migrate_catalog(
    path: &Path,
    game_state_path: &Path,
) -> Result<Catalog, Box<dyn std::error::Error>> {
    if path.is_file() {
        let loaded = Catalog::load_with_migration(path)?;
        if let Some(migrated_from) = loaded.migrated_from {
            backup_catalog(path, migrated_from)?;
            loaded.commit_migration(path, game_state_path)?;
        }
        return Ok(loaded.catalog);
    }

    // Preserve installations of the Slint prototype without replacing the
    // original file. A copied, validated catalog is safer than silently
    // overwriting a malformed legacy library on the first Tauri launch.
    let legacy_path = catalog::default_path();
    if legacy_path != path && legacy_path.is_file() {
        let catalog = Catalog::load(&legacy_path)?;
        catalog.save_atomically(path)?;
        return Ok(catalog);
    }

    Ok(Catalog::default())
}

/// The pre-migration catalog is the only copy of the user's library in the
/// older format, so the backup is both versioned and published atomically: a
/// later migration cannot overwrite an earlier one, and a crash halfway through
/// leaves a `.part` file rather than a truncated backup.
fn backup_catalog(path: &Path, migrated_from: u32) -> Result<PathBuf, std::io::Error> {
    let bytes = fs::read(path)?;
    let backup = unused_backup_path(path, migrated_from)?;
    let staging = backup.with_extension("bak.part");
    let outcome = (|| -> Result<(), std::io::Error> {
        // `remove_file` unlinks a symlink instead of following it, and
        // `create_new` refuses anything that reappears underneath us.
        let _ = fs::remove_file(&staging);
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staging)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&staging, &backup)
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&staging);
    }
    outcome.map(|()| backup)
}

fn unused_backup_path(path: &Path, migrated_from: u32) -> Result<PathBuf, std::io::Error> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(CATALOG_FILE);
    for attempt in 0..MAX_CATALOG_BACKUPS_PER_SCHEMA {
        let suffix = if attempt == 0 {
            String::new()
        } else {
            format!("-{attempt}")
        };
        let candidate = path.with_file_name(format!("{file_name}.v{migrated_from}{suffix}.bak"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many catalog backups already exist for this schema version",
    ))
}

fn media_cache_dir(app: &AppHandle) -> Result<PathBuf, tauri::Error> {
    Ok(app.path().app_cache_dir()?.join(MEDIA_DIRECTORY))
}

fn hydrate_catalog_media(
    app: &AppHandle,
    catalog: &mut Catalog,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut changed = false;
    let mut budget = MediaCacheBudget::new();
    for game in &mut catalog.games {
        // A large Steam library can contain hundreds of artworks. They are
        // cached on explicit import/refresh in a worker, never all copied on
        // application startup.
        if game.source == GameSource::Steam {
            continue;
        }
        changed |= cache_game_media_with_budget(app, game, &mut budget)?;
    }
    Ok(changed)
}

/// Copy only explicitly discovered artwork into `$APPCACHE/media`. The app
/// config grants the WebView access to this one directory and nowhere else.
fn cache_game_media(app: &AppHandle, game: &mut Game) -> Result<bool, std::io::Error> {
    let mut budget = MediaCacheBudget::new();
    cache_game_media_with_budget(app, game, &mut budget)
}

fn cache_game_media_with_budget(
    app: &AppHandle,
    game: &mut Game,
    budget: &mut MediaCacheBudget,
) -> Result<bool, std::io::Error> {
    let cache_dir = media_cache_dir(app).map_err(std::io::Error::other)?;
    fs::create_dir_all(&cache_dir)?;
    let artwork_changed = cache_media_role(
        &cache_dir,
        &game.id,
        "hero",
        &mut game.artwork_path,
        &mut game.artwork_source_path,
        budget,
    )?;
    let cover_changed = cache_media_role(
        &cache_dir,
        &game.id,
        "cover",
        &mut game.cover_path,
        &mut game.cover_source_path,
        budget,
    )?;
    Ok(artwork_changed || cover_changed)
}

/// Retain a private source path separately from the asset-protocol path that
/// the WebView may display. This lets a refresh repair a pruned cache without
/// ever passing the original Steam location across IPC.
fn cache_media_role(
    cache_dir: &Path,
    game_id: &str,
    role: &str,
    cached_path: &mut Option<PathBuf>,
    source_path: &mut Option<PathBuf>,
    budget: &mut MediaCacheBudget,
) -> Result<bool, std::io::Error> {
    let Some(source) = source_path.clone().or_else(|| cached_path.clone()) else {
        return Ok(false);
    };

    if source.starts_with(cache_dir) {
        if source.is_file() {
            *cached_path = Some(source);
        }
        return Ok(false);
    }

    match cache_media_file(cache_dir, game_id, role, &source, budget)? {
        Some(cached) => {
            let changed = cached_path.as_ref() != Some(&cached);
            *source_path = Some(source);
            *cached_path = Some(cached);
            Ok(changed)
        }
        None => {
            // Do not retain a raw source path in the presentation slot. The
            // source is still kept privately for a later refresh if it exists
            // again, while the UI receives its stable visual fallback.
            if cached_path
                .as_ref()
                .is_some_and(|path| !path.starts_with(cache_dir))
            {
                *cached_path = None;
            }
            Ok(false)
        }
    }
}

fn cache_media_file(
    cache_dir: &Path,
    game_id: &str,
    role: &str,
    source: &Path,
    budget: &mut MediaCacheBudget,
) -> Result<Option<PathBuf>, std::io::Error> {
    if source.starts_with(cache_dir) && source.is_file() {
        return Ok(Some(source.to_path_buf()));
    }
    if !source.is_file() {
        // Missing artwork should not make an otherwise valid local game
        // unlaunchable. The frontend will use its stable dark fallback.
        return Ok(None);
    }

    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| matches!(*extension, "png" | "jpg" | "jpeg" | "webp" | "bmp"))
        .unwrap_or("jpg");
    let target = cache_dir.join(format!("{:016x}-{role}.{extension}", stable_hash(game_id)));
    let source_metadata = source.metadata()?;

    if target.is_file()
        && let Ok(target_metadata) = target.metadata()
    {
        let source_changed = source_metadata.len() != target_metadata.len()
            || matches!(
                (source_metadata.modified(), target_metadata.modified()),
                (Ok(source_modified), Ok(target_modified)) if source_modified > target_modified
            );
        if !source_changed {
            return Ok(Some(target));
        }
    }

    let source_size = source_metadata.len();
    if source_size > MAX_MEDIA_FILE_BYTES || !budget.can_copy(source_size) {
        return Ok(None);
    }

    // A preview hydration and an import may reach the same stable cache name
    // concurrently. Copy to a unique sibling first, then publish atomically
    // so the WebView can never observe a half-written asset.
    let sequence = MEDIA_CACHE_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = cache_dir.join(format!(
        ".{:016x}-{role}-{}-{sequence}.tmp",
        stable_hash(game_id),
        std::process::id(),
    ));
    let copy_limit = MAX_MEDIA_FILE_BYTES.min(budget.remaining_bytes);
    let copy_result = (|| -> Result<u64, std::io::Error> {
        let source_file = fs::File::open(source)?;
        let mut reader = source_file.take(copy_limit.saturating_add(1));
        let mut temporary_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let copied = std::io::copy(&mut reader, &mut temporary_file)?;
        if copied > copy_limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "media file exceeds the cache budget",
            ));
        }
        Ok(copied)
    })();
    let copied = match copy_result {
        Ok(copied) => copied,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            if error.kind() == std::io::ErrorKind::InvalidData {
                return Ok(None);
            }
            return Err(error);
        }
    };
    budget.remaining_bytes -= copied;

    match fs::rename(&temporary, &target) {
        Ok(()) => Ok(Some(target)),
        Err(_error) if target.is_file() => {
            let _ = fs::remove_file(&temporary);
            Ok(Some(target))
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

#[derive(Debug)]
struct MediaCacheBudget {
    remaining_bytes: u64,
}

impl MediaCacheBudget {
    fn new() -> Self {
        Self {
            remaining_bytes: MAX_MEDIA_CACHE_BYTES_PER_OPERATION,
        }
    }

    fn can_copy(&self, bytes: u64) -> bool {
        bytes <= self.remaining_bytes
    }
}

fn stable_hash(value: &str) -> u64 {
    value
        .as_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

fn library_state(app: &AppHandle, stored_catalog: &Catalog) -> LibraryState {
    let cache_dir = media_cache_dir(app).ok();
    let include_showcase = preferences::PreferencesService::from_app(app)
        .and_then(|service| service.load())
        .map(|preferences| preferences.show_showcase_games)
        .unwrap_or(false);
    let presentation = presentation_catalog(stored_catalog, include_showcase);
    LibraryState {
        games: presentation
            .games
            .iter()
            .map(|game| game_view(game, &presentation, cache_dir.as_deref()))
            .collect(),
    }
}

fn game_view(game: &Game, catalog: &Catalog, cache_dir: Option<&Path>) -> GameView {
    // Wallpapers the user deliberately chose on the detail page. Each role is
    // independent: the background never overrides a card cover and vice versa.
    let home_image = media_source_url(game.home_image_path.as_deref(), cache_dir);
    let landscape_image = media_source_url(game.landscape_image_path.as_deref(), cache_dir);
    let local_hero = media_source_url(game.artwork_path.as_deref(), cache_dir);
    let local_cover = media_source_url(game.cover_path.as_deref(), cache_dir);
    // Steam exposes distinct artwork roles for its library. Its landscape
    // capsule is the publisher's official branded wallpaper, while the wide
    // library hero belongs to the selected horizontal card. Do not reuse a
    // low-resolution `header.jpg` for all three positions: it both looks soft
    // as a wallpaper and makes the selected card collapse into the background.
    let steam_wallpaper = steam_store_asset_url(game, "capsule_616x353.jpg");
    let steam_cover = steam_store_asset_url(game, "library_600x900.jpg");
    let steam_landscape = steam_store_asset_url(game, "library_hero.jpg");
    let host_platform = current_host_platform();
    let supported_platforms = steam_supported_platforms(game);
    let compatible_with_host = steam_compatibility(game, host_platform, &supported_platforms);
    GameView {
        id: game.id.clone(),
        title: game.title.clone(),
        description: game
            .description
            .clone()
            .unwrap_or_else(|| "Ready for your next session.".into()),
        metadata: game
            .metadata
            .clone()
            .unwrap_or_else(|| "Ready to play".into()),
        genre: genre_for_game(game),
        source: match &game.launch_target {
            LaunchTarget::Runner { runner_id, .. } if runner_id == WINE_STAGING_RUNNER_ID => "wine",
            LaunchTarget::Direct | LaunchTarget::Steam { .. } | LaunchTarget::Runner { .. } => {
                match &game.source {
                    GameSource::Steam => "steam",
                    GameSource::Local => "local",
                }
            }
        }
        .into(),
        hero_url: home_image
            .or_else(|| steam_wallpaper.clone())
            .or_else(|| local_hero.clone()),
        cover_url: local_cover
            .clone()
            .or_else(|| steam_cover.clone())
            .or_else(|| local_hero.clone())
            .or_else(|| steam_wallpaper.clone()),
        landscape_url: landscape_image
            .or_else(|| steam_landscape)
            .or_else(|| local_hero.clone())
            .or_else(|| steam_wallpaper.clone())
            .or_else(|| local_cover.clone())
            .or(steam_cover),
        last_played_at: game.last_played_at.clone().unwrap_or_default(),
        play_time_seconds: game.play_time_seconds,
        launchable: match &game.launch_target {
            // A Steam URI can still be launched after an uninstall, but that
            // would hand users a dead-end client error. The source refresh is
            // the cheap, authoritative state boundary; no filesystem stat per
            // rendered card is needed here.
            LaunchTarget::Steam { .. } => game.installation_path.is_some(),
            LaunchTarget::Direct => !game.id.starts_with("showcase-"),
            LaunchTarget::Runner {
                runner_id,
                profile_id,
                game_ref,
            } if runner_id == WINE_STAGING_RUNNER_ID => {
                catalog
                    .wine_profile(profile_id)
                    .is_some_and(|profile| profile.enabled)
                    && catalog.wine_inventory_entry(profile_id, game_ref).is_some()
                    && cfg!(target_os = "macos")
            }
            // Third-party runner execution is still deliberately unavailable
            // until its WIT host can resolve a typed intent and grants.
            LaunchTarget::Runner { .. } => false,
        },
        host_platform: host_platform.into(),
        supported_platforms,
        compatible_with_host,
        wine_attachable: cfg!(target_os = "macos") && is_local_direct_windows_game(game),
    }
}

/// This is the machine actually running Orivo, not a Steam account preference.
/// Steam's Store flags only declare native support; they do not promise that a
/// title will work through compatibility layers such as Proton.
fn current_host_platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        "linux" => "linux",
        _ => "other",
    }
}

fn steam_supported_platforms(game: &Game) -> Vec<String> {
    if game.source != GameSource::Steam {
        return Vec::new();
    }
    game.extra
        .get(catalog::STEAM_STORE_PLATFORMS_KEY)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|platform| matches!(*platform, "windows" | "macos" | "linux"))
        .map(str::to_owned)
        .collect()
}

fn steam_compatibility(game: &Game, host_platform: &str, supported: &[String]) -> Option<bool> {
    (game.source == GameSource::Steam
        && matches!(host_platform, "windows" | "macos" | "linux")
        && !supported.is_empty())
    .then(|| supported.iter().any(|platform| platform == host_platform))
}

/// All remote artwork URLs are derived from a validated numeric Steam AppID,
/// never taken from a Steam response. This narrow host is explicitly allowed
/// in the app CSP and lets a newly synced, non-installed library paint without
/// downloading thousands of images into the local cache first.
fn steam_store_asset_url(game: &Game, asset: &str) -> Option<String> {
    match &game.launch_target {
        LaunchTarget::Steam { app_id } if *app_id > 0 => Some(format!(
            "https://cdn.cloudflare.steamstatic.com/steam/apps/{app_id}/{asset}"
        )),
        LaunchTarget::Steam { .. } | LaunchTarget::Direct | LaunchTarget::Runner { .. } => None,
    }
}

fn genre_for_game(game: &Game) -> String {
    if let Some(genre) = game
        .extra
        .get(catalog::STEAM_STORE_GENRE_KEY)
        .and_then(serde_json::Value::as_str)
        .filter(|genre| !genre.trim().is_empty())
    {
        return genre.to_string();
    }

    match game_key(&game.title).as_str() {
        "eldenring" | "baldursgate3" | "thewitcher3" => "RPG",
        "cyberpunk2077" => "Action RPG",
        "hadesii" => "Roguelike",
        "reddeadredemption2" | "horizonforbiddenwest" => "Adventure",
        "godofwar" | "astroduel2" => "Action",
        "unrailed" => "Co-op",
        _ => "Library",
    }
    .into()
}

/// The fixed rail is a visual fixture, not persisted user data. Local records
/// of matching games retain their launch configuration while borrowing the
/// bundled editorial artwork. Steam records replace a matching fixture whole:
/// their Store artwork, source badge and no-session state must stay authentic.
fn presentation_catalog(stored_catalog: &Catalog, include_showcase: bool) -> Catalog {
    // The bundled demo (showcase) games are a debug fixture, off by default so a
    // real library shows only the user's own games.
    let mut presentation = showcase_catalog();
    if !include_showcase {
        presentation.games.clear();
    }
    // Profiles and the private executable inventory do not change rail
    // presentation, but they let runner cards derive a safe launchable state
    // without projecting a path across IPC.
    presentation.wine_profiles = stored_catalog.wine_profiles.clone();
    presentation.wine_inventory = stored_catalog.wine_inventory.clone();

    // An explicit Direct → Wine association keeps the original local record
    // for a reversible fallback, but the library should surface one card.
    // If the profile is deleted, its inventory and runner card disappear and
    // this filtering naturally stops on the next reload.
    let associated_direct_ids = stored_catalog
        .wine_inventory
        .iter()
        .filter_map(|entry| {
            let has_runner_card = stored_catalog.games.iter().any(|game| {
                matches!(
                    &game.launch_target,
                    LaunchTarget::Runner {
                        runner_id,
                        profile_id,
                        game_ref,
                    } if runner_id == WINE_STAGING_RUNNER_ID
                        && profile_id == &entry.profile_id
                        && game_ref == &entry.game_ref
                )
            });
            has_runner_card
                .then_some(entry.origin_direct_game_id.as_deref())
                .flatten()
        })
        .collect::<BTreeSet<_>>();

    for local_game in &stored_catalog.games {
        if associated_direct_ids.contains(local_game.id.as_str())
            && is_local_direct_windows_game(local_game)
        {
            continue;
        }
        if let Some(showcase_game) = presentation
            .games
            .iter_mut()
            .find(|showcase_game| game_key(&showcase_game.title) == game_key(&local_game.title))
        {
            if local_game.source == GameSource::Steam {
                *showcase_game = local_game.clone();
                continue;
            }
            let mut merged = local_game.clone();
            merged.description = local_game
                .description
                .clone()
                .or_else(|| showcase_game.description.clone());
            merged.metadata = local_game
                .metadata
                .clone()
                .or_else(|| showcase_game.metadata.clone());
            merged.artwork_path = showcase_game.artwork_path.clone();
            merged.cover_path = showcase_game.cover_path.clone();
            if merged.last_played_at.is_none() {
                merged.last_played_at = showcase_game.last_played_at.clone();
            }
            if merged.play_time_seconds == 0 {
                merged.play_time_seconds = showcase_game.play_time_seconds;
            }
            *showcase_game = merged;
        } else {
            presentation.games.push(local_game.clone());
        }
    }

    // A local record with no discovered or fetched artwork borrows the bundled
    // plate whose title matches its own (case-insensitive, loose), and
    // otherwise wears the neutral placeholder. This is presentation-only: the
    // stored record keeps its empty slots, so the post-import artwork search
    // still sees the gap and can fill it with real art.
    for game in &mut presentation.games {
        if game.source != GameSource::Local
            || (game.cover_path.is_some() && game.artwork_path.is_some())
        {
            continue;
        }
        let matched = bundled_artwork_for_title(&game.title);
        let cover = matched.and_then(|entry| entry.cover.clone());
        let hero = matched.and_then(|entry| entry.hero.clone());
        if game.cover_path.is_none() {
            game.cover_path = Some(PathBuf::from(
                cover
                    .clone()
                    .or_else(|| hero.clone())
                    .unwrap_or_else(|| NEUTRAL_ARTWORK_PLACEHOLDER.to_owned()),
            ));
        }
        if game.artwork_path.is_none() {
            game.artwork_path = Some(PathBuf::from(
                hero.or(cover)
                    .unwrap_or_else(|| NEUTRAL_ARTWORK_PLACEHOLDER.to_owned()),
            ));
        }
    }

    presentation
}

fn showcase_catalog() -> Catalog {
    Catalog {
        schema_version: catalog::CURRENT_SCHEMA_VERSION,
        games: vec![
            showcase_game(
                "elden-ring",
                "Elden Ring",
                "A vast world full of mystery and peril. What will you discover?",
                "Achievements 67/82",
                "2 days ago",
                128,
                "elden-ring.jpg",
                Some("elden-ring-wallpaper.png"),
            ),
            showcase_game(
                "cyberpunk-2077",
                "Cyberpunk 2077",
                "Night City is yours for the taking. Choose your legend.",
                "Achievements 41/57",
                "5 days ago",
                85,
                "cyberpunk-2077.jpg",
                Some("cyberpunk-2077.webp"),
            ),
            showcase_game(
                "baldurs-gate-3",
                "Baldur's Gate 3",
                "Gather your party and return to the Forgotten Realms.",
                "Achievements 39/54",
                "1 week ago",
                97,
                "baldurs-gate-3.jpg",
                Some("baldurs-gate-3.jpg"),
            ),
            showcase_game(
                "hades-2",
                "Hades II",
                "Defy the Titan of Time beneath a moonlit underworld.",
                "Achievements 26/50",
                "1 week ago",
                51,
                "hades-2.jpg",
                Some("hades-2.jpg"),
            ),
            showcase_game(
                "red-dead-redemption-2",
                "Red Dead Redemption 2",
                "Outlaws for life in a fading American frontier.",
                "Achievements 35/51",
                "2 weeks ago",
                110,
                "red-dead-redemption-2.jpg",
                Some("red-dead-redemption-2.jpg"),
            ),
            showcase_game(
                "the-witcher-3",
                "The Witcher 3",
                "Track monsters and chase the Wild Hunt across the Continent.",
                "Achievements 56/78",
                "3 weeks ago",
                200,
                "the-witcher-3-wild-hunt.jpg",
                Some("the-witcher-3-wild-hunt.jpg"),
            ),
            showcase_game(
                "horizon-forbidden-west",
                "Horizon Forbidden West",
                "Explore a vibrant frontier ruled by colossal machines.",
                "Achievements 33/59",
                "1 month ago",
                68,
                "horizon-forbidden-west.jpg",
                Some("horizon-forbidden-west.jpg"),
            ),
            showcase_game(
                "god-of-war",
                "God of War",
                "A deeply personal journey through the Norse realms.",
                "Achievements 28/37",
                "1 month ago",
                120,
                "god-of-war.jpg",
                Some("god-of-war.jpg"),
            ),
            showcase_game(
                "unrailed",
                "Unrailed!",
                "Build a railway together before the runaway train reaches the end.",
                "Local co-op • Railway 18",
                "2 months ago",
                24,
                "unrailed.jpg",
                None,
            ),
            showcase_game(
                "astro-duel-2",
                "Astro Duel 2",
                "A high-speed space battle where pilots jump between ship and body.",
                "Versus • Campaign",
                "2 months ago",
                12,
                "astro-duel-2.jpg",
                None,
            ),
        ],
        wine_profiles: Vec::new(),
        wine_inventory: Vec::new(),
        extra: BTreeMap::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn showcase_game(
    id: &str,
    title: &str,
    description: &str,
    metadata: &str,
    last_played_at: &str,
    hours: u64,
    cover_file: &str,
    hero_file: Option<&str>,
) -> Game {
    // The selected card intentionally echoes the cinematic Elden Ring scene,
    // matching the reference selector rather than showing a logo-heavy box art.
    let cover_path = if id == "elden-ring" {
        PathBuf::from("/media/igdb/heroes/elden-ring-wallpaper.png")
    } else {
        PathBuf::from(format!("/media/igdb/covers/{cover_file}"))
    };
    let artwork_path = hero_file
        .map(|file| PathBuf::from(format!("/media/igdb/heroes/{file}")))
        .or_else(|| Some(cover_path.clone()));

    Game {
        id: format!("showcase-{id}"),
        title: title.into(),
        executable_path: Some(PathBuf::from(format!("showcase://{id}"))),
        source: GameSource::Local,
        source_id: None,
        launch_target: LaunchTarget::Direct,
        installation_path: None,
        working_directory: None,
        arguments: Vec::new(),
        description: Some(description.into()),
        metadata: Some(metadata.into()),
        artwork_path,
        artwork_source_path: None,
        cover_path: Some(cover_path),
        cover_source_path: None,
        home_image_path: None,
        landscape_image_path: None,
        logo_path: None,
        hero_video_path: None,
        last_played_at: Some(last_played_at.into()),
        play_time_seconds: hours * 3_600,
        extra: BTreeMap::new(),
    }
}

fn game_key(title: &str) -> String {
    title
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The bundled IGDB artwork manifest shipped with the app. It names the titles
/// whose covers and heroes exist as `/media/igdb` public assets.
const BUNDLED_ARTWORK_MANIFEST: &str = include_str!("../../assets/igdb/sources.json");

/// A neutral bundled brand image for games whose artwork was not found. An
/// unknown game must never wear another game's art (this used to fall through
/// to the Elden Ring plates).
const NEUTRAL_ARTWORK_PLACEHOLDER: &str = "/media/orivo-ring-icon.png";

/// One bundled title from the manifest, keyed by its normalized title.
struct BundledArtwork {
    key: String,
    cover: Option<String>,
    hero: Option<String>,
}

fn bundled_artwork_index() -> &'static [BundledArtwork] {
    static INDEX: OnceLock<Vec<BundledArtwork>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let Ok(manifest) = serde_json::from_str::<serde_json::Value>(BUNDLED_ARTWORK_MANIFEST)
        else {
            return Vec::new();
        };
        let file_of = |asset: &serde_json::Value, slot: &str| {
            asset
                .get(slot)
                .and_then(|slot| slot.get("file"))
                .and_then(serde_json::Value::as_str)
                .filter(|file| !file.contains("..") && !file.contains('\\'))
                .map(|file| format!("/media/igdb/{file}"))
        };
        manifest
            .get("assets")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|asset| {
                let title = asset.get("title").and_then(serde_json::Value::as_str)?;
                let key = game_key(title);
                (!key.is_empty()).then(|| BundledArtwork {
                    key,
                    cover: file_of(asset, "cover"),
                    hero: file_of(asset, "hero"),
                })
            })
            .collect()
    })
}

/// Case-insensitive, punctuation-free title lookup with a loose containment
/// fallback, so an executable-style name ("EldenRing", "witcher3") still finds
/// its bundled artwork. A containment match requires a stem of at least five
/// characters so a short junk title cannot alias onto a manifest entry.
fn bundled_artwork_for_title(title: &str) -> Option<&'static BundledArtwork> {
    const MIN_LOOSE_KEY: usize = 5;
    let key = game_key(title);
    if key.is_empty() {
        return None;
    }
    let index = bundled_artwork_index();
    if let Some(exact) = index.iter().find(|entry| entry.key == key) {
        return Some(exact);
    }
    index.iter().find(|entry| {
        let (short, long) = if entry.key.len() <= key.len() {
            (entry.key.as_str(), key.as_str())
        } else {
            (key.as_str(), entry.key.as_str())
        };
        short.len() >= MIN_LOOSE_KEY && long.contains(short)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn showcase_is_stable_and_never_exposes_a_launch_target() {
        let showcase = showcase_catalog();
        assert_eq!(showcase.games.len(), 10);
        assert!(
            showcase
                .games
                .iter()
                .all(|game| game.id.starts_with("showcase-"))
        );
        assert!(showcase.games.iter().all(|game| {
            game.executable_path
                .as_ref()
                .is_some_and(|path| path.to_string_lossy().starts_with("showcase://"))
        }));
    }

    #[test]
    fn game_media_scheme_accepts_only_an_opaque_file_inside_its_own_directory() {
        // The three URL shapes the platforms produce for one minted preview.
        assert_eq!(
            game_media_requested_file("game-media:cover-abc123.jpg"),
            Some("cover-abc123.jpg")
        );
        assert_eq!(
            game_media_requested_file("game-media://localhost/cover-abc123.jpg"),
            Some("cover-abc123.jpg")
        );
        assert_eq!(
            game_media_requested_file("http://game-media.localhost/cover-abc123.jpg?v=2"),
            Some("cover-abc123.jpg")
        );

        for hostile in [
            "game-media:../../catalog.json",
            "game-media://localhost/..%2f..%2fcatalog.json",
            "game-media:/etc/passwd",
            "game-media:.hidden",
            "game-media:",
            "game-media://localhost/",
        ] {
            assert_eq!(game_media_requested_file(hostile), None, "{hostile}");
        }
    }

    #[test]
    fn media_range_requests_stay_inside_the_file_and_below_one_chunk() {
        assert_eq!(parse_media_byte_range("bytes=0-99", 1_000), Some((0, 99)));
        assert_eq!(
            parse_media_byte_range("bytes=990-", 1_000),
            Some((990, 999))
        );
        assert_eq!(parse_media_byte_range("bytes=-10", 1_000), Some((990, 999)));
        // A clamped end is a legal short answer and bounds the response size.
        assert_eq!(
            parse_media_byte_range("bytes=0-", 64 * 1_024 * 1_024),
            Some((0, MAX_MEDIA_RANGE_CHUNK_BYTES - 1))
        );
        for unusable in [
            "bytes=1000-1200",
            "bytes=500-100",
            "bytes=0-10,20-30",
            "items=0-10",
            "bytes=-",
            "bytes=abc-def",
        ] {
            assert_eq!(parse_media_byte_range(unusable, 1_000), None, "{unusable}");
        }
    }

    #[test]
    fn cached_media_name_is_stable_without_leaking_the_source_path() {
        let first = stable_hash("/Games/Example.app/Contents/MacOS/Example");
        let second = stable_hash("/Games/Example.app/Contents/MacOS/Example");
        assert_eq!(first, second);
        assert_ne!(
            first,
            stable_hash("/Games/Another.app/Contents/MacOS/Another")
        );
    }

    #[test]
    fn cache_media_respects_its_budget_before_publishing_an_asset() {
        let root = std::env::temp_dir().join(format!(
            "orivo-media-budget-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cache_dir = root.join("cache");
        let source = root.join("source.jpg");
        fs::create_dir_all(&cache_dir).unwrap();
        fs::write(&source, b"image").unwrap();

        let mut too_small = MediaCacheBudget { remaining_bytes: 4 };
        assert_eq!(
            cache_media_file(&cache_dir, "steam:480", "cover", &source, &mut too_small).unwrap(),
            None
        );
        assert!(fs::read_dir(&cache_dir).unwrap().next().is_none());

        let mut enough = MediaCacheBudget { remaining_bytes: 5 };
        let cached = cache_media_file(&cache_dir, "steam:480", "cover", &source, &mut enough)
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(&cached).unwrap(), b"image");

        fs::write(&source, b"updated").unwrap();
        let mut refresh_budget = MediaCacheBudget { remaining_bytes: 7 };
        let refreshed = cache_media_file(
            &cache_dir,
            "steam:480",
            "cover",
            &source,
            &mut refresh_budget,
        )
        .unwrap()
        .unwrap();
        assert_eq!(refreshed, cached);
        assert_eq!(fs::read(&refreshed).unwrap(), b"updated");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn steam_source_uses_a_typed_target_without_a_fake_executable() {
        let game = steam_game_fixture();
        let imported = steam_game_to_catalog_game(game);

        assert_eq!(imported.id, "steam:480");
        assert_eq!(imported.source, GameSource::Steam);
        assert_eq!(imported.source_id.as_deref(), Some("480"));
        assert_eq!(imported.launch_target, LaunchTarget::Steam { app_id: 480 });
        assert!(imported.executable_path.is_none());
        assert!(imported.validate().is_ok());
    }

    #[test]
    fn owned_steam_games_stay_visible_without_an_installation() {
        let game = owned_steam_game_to_catalog_game(owned_game_fixture(), None);
        let view = game_view(&game, &Catalog::default(), None);

        assert_eq!(game.id, "steam:480");
        assert!(game.installation_path.is_none());
        assert_eq!(game.metadata.as_deref(), Some("Not installed"));
        assert_eq!(game.play_time_seconds, 4_200);
        assert!(!view.launchable);
        assert_eq!(
            view.hero_url.as_deref(),
            Some("https://cdn.cloudflare.steamstatic.com/steam/apps/480/capsule_616x353.jpg")
        );
        assert_eq!(
            view.cover_url.as_deref(),
            Some("https://cdn.cloudflare.steamstatic.com/steam/apps/480/library_600x900.jpg")
        );
        assert_eq!(
            view.landscape_url.as_deref(),
            Some("https://cdn.cloudflare.steamstatic.com/steam/apps/480/library_hero.jpg")
        );
        assert_eq!(view.source, "steam");
    }

    #[test]
    fn owned_steam_games_merge_the_local_install_state_by_app_id() {
        let game =
            owned_steam_game_to_catalog_game(owned_game_fixture(), Some(steam_game_fixture()));
        let view = game_view(&game, &Catalog::default(), None);

        assert!(game.installation_path.is_some());
        assert_eq!(game.metadata.as_deref(), Some("Installed"));
        assert!(view.launchable);
        assert_eq!(game.launch_target, LaunchTarget::Steam { app_id: 480 });
    }

    #[test]
    fn store_metadata_replaces_the_generic_owned_game_copy_and_genre() {
        let mut game = owned_steam_game_to_catalog_game(owned_game_fixture(), None);
        let metadata = steam_account::SteamStoreGameMetadata {
            app_id: 480,
            short_description: Some("A real short description from Steam.".into()),
            genre: Some("Racing".into()),
            platforms: Some(steam_account::SteamStorePlatforms {
                windows: true,
                macos: true,
                linux: true,
            }),
        };

        apply_steam_store_metadata(&mut game, Some(&metadata));
        let view = game_view(&game, &Catalog::default(), None);

        assert_eq!(
            game.description.as_deref(),
            Some("A real short description from Steam.")
        );
        assert_eq!(view.genre, "Racing");
        assert_eq!(view.metadata, "Not installed");
        assert_eq!(view.supported_platforms, ["windows", "macos", "linux"]);
        assert_eq!(
            view.compatible_with_host,
            matches!(current_host_platform(), "windows" | "macos" | "linux").then_some(true)
        );
        assert_eq!(
            steam_compatibility(&game, "macos", &["windows".into()]),
            Some(false)
        );
        assert!(
            game.extra
                .contains_key(catalog::STEAM_STORE_METADATA_MARKER)
        );
    }

    #[test]
    fn matching_steam_games_keep_their_own_store_presentation() {
        let mut catalog = Catalog::default();
        let mut steam_game = owned_steam_game_to_catalog_game(owned_game_fixture(), None);
        steam_game.title = "Elden Ring".into();
        catalog.games.push(steam_game);

        let presentation = presentation_catalog(&catalog, true);
        let game = presentation
            .games
            .iter()
            .find(|game| game.title == "Elden Ring")
            .unwrap();
        let view = game_view(game, &presentation, None);

        assert_eq!(game.source, GameSource::Steam);
        assert!(game.artwork_path.is_none());
        assert!(game.last_played_at.is_none());
        assert_eq!(
            view.hero_url.as_deref(),
            Some("https://cdn.cloudflare.steamstatic.com/steam/apps/480/capsule_616x353.jpg")
        );
        assert_eq!(view.last_played_at, "");
    }

    #[test]
    fn steam_auth_navigation_allows_secure_challenges_but_not_local_schemes() {
        assert!(is_allowed_steam_auth_navigation(
            &Url::parse("https://store.steampowered.com/explore/").unwrap()
        ));
        assert!(is_steam_store_page(
            &Url::parse("https://store.steampowered.com/explore/").unwrap()
        ));
        assert!(is_allowed_steam_auth_navigation(
            &Url::parse("https://captcha.example.test/challenge").unwrap()
        ));
        assert!(!is_steam_store_page(
            &Url::parse("https://captcha.example.test/challenge").unwrap()
        ));
        assert!(!is_allowed_steam_auth_navigation(
            &Url::parse("http://store.steampowered.com/explore/").unwrap()
        ));
        assert!(!is_allowed_steam_auth_navigation(
            &Url::parse("file:///Users/example/private.html").unwrap()
        ));
    }

    #[test]
    fn steam_preview_does_not_serialize_source_paths() {
        let source = steam_game_fixture();
        let imported = steam_game_to_catalog_game(source.clone());
        let preview = steam_preview_game(source, Some(&imported), None);
        let json = serde_json::to_string(&preview).unwrap();

        assert!(!json.contains("/Users/example"));
        assert!(!json.contains("Steam/steamapps"));
        assert_eq!(preview.cover_url, None);
        assert_eq!(preview.hero_url, None);
        assert!(preview.already_imported);
    }

    #[test]
    fn steam_import_ids_are_numeric_nonzero_and_deduplicated() {
        let ids = requested_steam_app_ids(vec!["480".into(), "480".into(), "570".into()]).unwrap();
        assert_eq!(ids.into_iter().collect::<Vec<_>>(), vec![480, 570]);
        assert!(requested_steam_app_ids(vec!["0".into()]).is_err());
        assert!(requested_steam_app_ids(vec!["not-an-app-id".into()]).is_err());
        assert!(requested_steam_app_ids(Vec::new()).is_err());
    }

    #[test]
    fn catalog_migration_writes_a_versioned_backup_before_persisting_current_schema() {
        let root = temporary_directory("catalog-migration");
        let catalog_path = root.join("catalog.json");
        fs::write(
            &catalog_path,
            r#"{"schema_version":1,"games":[{"id":"local","title":"Local","executable_path":"/Games/Local"}]}"#,
        )
        .unwrap();

        let catalog = load_or_migrate_catalog(&catalog_path, &root.join(GAME_STATE_FILE)).unwrap();
        let backup = root.join("catalog.json.v1.bak");

        assert_eq!(catalog.schema_version, catalog::CURRENT_SCHEMA_VERSION);
        let backed_up = fs::read_to_string(&backup).unwrap();
        assert!(backed_up.contains("\"schema_version\":1"));
        assert!(backed_up.contains("\"id\":\"local\""));
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&catalog_path).unwrap()).unwrap();
        assert_eq!(
            persisted["schema_version"],
            serde_json::Value::from(catalog::CURRENT_SCHEMA_VERSION)
        );
        // Nothing half-written survives a successful backup.
        assert!(!root.join("catalog.json.v1.bak.part").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_migration_that_rewrites_ids_moves_the_game_state_with_it() {
        let root = temporary_directory("catalog-migration-state");
        let catalog_path = root.join("catalog.json");
        let state_path = root.join(GAME_STATE_FILE);
        fs::write(
            &catalog_path,
            r#"{"schema_version":1,"games":[{"id":"local","title":"Local","executable_path":"/Games/Local"}]}"#,
        )
        .unwrap();
        fs::write(
            &state_path,
            r#"{"schema_version":1,"games":{"local":{"wishlisted":true}}}"#,
        )
        .unwrap();

        let catalog = load_or_migrate_catalog(&catalog_path, &state_path).unwrap();
        let migrated_id = catalog.games[0].id.clone();
        assert_ne!(migrated_id, "local");

        // Wishlist and media selections are keyed by game id, so an id rewrite
        // that left them behind would silently drop them.
        let state: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&state_path).unwrap()).unwrap();
        assert!(state["games"].get("local").is_none());
        assert_eq!(
            state["games"][migrated_id.as_str()]["wishlisted"],
            serde_json::Value::Bool(true)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_later_migration_never_clobbers_an_earlier_backup() {
        let root = temporary_directory("catalog-backup-versions");
        let catalog_path = root.join("catalog.json");
        fs::write(
            &catalog_path,
            r#"{"schema_version":1,"games":[{"id":"first","title":"First","executable_path":"/Games/First"}]}"#,
        )
        .unwrap();
        load_or_migrate_catalog(&catalog_path, &root.join(GAME_STATE_FILE)).unwrap();

        // A restored, still-legacy file is migrated again later. The only copy
        // of the first pre-migration library must survive that.
        fs::write(
            &catalog_path,
            r#"{"schema_version":1,"games":[{"id":"second","title":"Second","executable_path":"/Games/Second"}]}"#,
        )
        .unwrap();
        load_or_migrate_catalog(&catalog_path, &root.join(GAME_STATE_FILE)).unwrap();

        assert!(
            fs::read_to_string(root.join("catalog.json.v1.bak"))
                .unwrap()
                .contains("\"id\":\"first\"")
        );
        assert!(
            fs::read_to_string(root.join("catalog.json.v1-1.bak"))
                .unwrap()
                .contains("\"id\":\"second\"")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_fresh_profile_resolves_every_library_card_on_the_detail_page() {
        // A fresh profile has no `catalog.json`, so every card the Library
        // paints comes from the showcase rail. Projecting the stored catalog
        // instead of the presentation one left all of them dead on click.
        let stored = Catalog::default();
        let detail = detail_projection();
        project_presentation_catalog(&detail, &stored).unwrap();

        let library = presentation_catalog(&stored, true);
        assert!(!library.games.is_empty());
        for game in &library.games {
            let view = game_detail::get_game_detail(&detail, game.id.clone())
                .unwrap()
                .unwrap_or_else(|| panic!("{} has no detail page", game.id));
            assert_eq!(view.summary.id, game.id);
            assert!(!view.summary.cover_url.is_empty(), "{}", game.id);
            assert!(
                game_detail::set_game_wishlist(&detail, game.id.clone(), true).is_ok(),
                "{}",
                game.id
            );
        }
    }

    #[test]
    fn the_detail_projection_uses_the_same_identities_as_the_library() {
        // A Steam record replaces the fixture it matches by title, so the
        // fixture id stops existing the moment the library is rendered.
        let mut stored = Catalog::default();
        let mut steam_game = owned_steam_game_to_catalog_game(owned_game_fixture(), None);
        steam_game.title = "Elden Ring".into();
        stored.games.push(steam_game);

        let detail = detail_projection();
        project_presentation_catalog(&detail, &stored).unwrap();

        assert!(detail.contains("steam:480").unwrap());
        assert!(!detail.contains("showcase-elden-ring").unwrap());
        assert!(detail.contains("showcase-cyberpunk-2077").unwrap());
    }

    #[test]
    fn one_unprojectable_record_does_not_freeze_the_whole_projection() {
        let detail = detail_projection();
        let mut stored = Catalog::default();
        let mut broken = owned_steam_game_to_catalog_game(owned_game_fixture(), None);
        broken.id = "not an opaque id".into();
        broken.title = "Broken".into();
        stored.games.push(broken);
        let mut healthy = owned_steam_game_to_catalog_game(owned_game_fixture(), None);
        healthy.id = "steam:570".into();
        healthy.title = "Healthy".into();
        stored.games.push(healthy);

        project_presentation_catalog(&detail, &stored).unwrap();
        assert!(!detail.contains("not an opaque id").unwrap());
        assert!(detail.contains("steam:570").unwrap());
        assert!(detail.contains("showcase-elden-ring").unwrap());

        // The projection keeps following later writes rather than freezing on
        // the record it had to drop.
        let mut added = owned_steam_game_to_catalog_game(owned_game_fixture(), None);
        added.id = "steam:571".into();
        added.title = "Added Later".into();
        stored.games.push(added);
        project_presentation_catalog(&detail, &stored).unwrap();
        assert!(detail.contains("steam:571").unwrap());
        assert!(detail.contains("steam:570").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn the_media_scheme_never_follows_a_symlink_planted_in_its_directory() {
        let root = temporary_directory("media-symlink");
        let media = root.join(game_media::MEDIA_DIRECTORY);
        fs::create_dir_all(&media).unwrap();
        let outside = root.join("catalog.json");
        fs::write(&outside, b"\x89PNG\r\n\x1a\nprivate").unwrap();
        std::os::unix::fs::symlink(&outside, media.join("cover-planted.png")).unwrap();
        fs::write(media.join("cover-real.png"), b"\x89PNG\r\n\x1a\nreal").unwrap();

        let planted = game_media_response(&media, &media_request("cover-planted.png", None));
        assert_eq!(planted.status(), tauri::http::StatusCode::NOT_FOUND);
        assert!(planted.body().is_empty());

        // The file the app wrote itself is still served.
        let real = game_media_response(&media, &media_request("cover-real.png", None));
        assert_eq!(real.status(), tauri::http::StatusCode::OK);
        assert_eq!(real.body(), b"\x89PNG\r\n\x1a\nreal");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_media_request_without_a_range_header_is_clamped_to_one_chunk() {
        let root = temporary_directory("media-chunk");
        let media = root.join(game_media::MEDIA_DIRECTORY);
        fs::create_dir_all(&media).unwrap();
        let large = media_bytes(
            MP4_MAGIC,
            usize::try_from(MAX_MEDIA_RANGE_CHUNK_BYTES).unwrap() + 4_096,
        );
        fs::write(media.join("trailer-large.mp4"), &large).unwrap();
        fs::write(media.join("cover-small.png"), b"\x89PNG\r\n\x1a\nsmall").unwrap();

        // No `Range` header: a 250 MB video must not be buffered whole on the
        // protocol thread just because the WebView did not ask for a window.
        // The format is sniffed from the bytes, so the file name cannot opt out.
        let response = game_media_response(&media, &media_request("trailer-large.mp4", None));
        assert_eq!(response.status(), tauri::http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.body().len(),
            usize::try_from(MAX_MEDIA_RANGE_CHUNK_BYTES).unwrap()
        );
        assert_eq!(
            response
                .headers()
                .get(tauri::http::header::CONTENT_RANGE)
                .unwrap(),
            format!(
                "bytes 0-{}/{}",
                MAX_MEDIA_RANGE_CHUNK_BYTES - 1,
                large.len()
            )
            .as_str()
        );

        // Anything that already fits in one chunk is still answered whole.
        let small = game_media_response(&media, &media_request("cover-small.png", None));
        assert_eq!(small.status(), tauri::http::StatusCode::OK);
        assert_eq!(small.body(), b"\x89PNG\r\n\x1a\nsmall");
        fs::remove_dir_all(root).unwrap();
    }

    /// A 12 MB cover is over the video chunk bound but well under the image
    /// import cap. Answering it with a 206 nobody asked for is what a WebView
    /// renders as a broken `<img>`, so the whole file must come back as a 200.
    #[test]
    fn an_image_larger_than_one_chunk_is_served_whole_without_a_range_header() {
        let root = temporary_directory("media-whole-image");
        let media = root.join(game_media::MEDIA_DIRECTORY);
        fs::create_dir_all(&media).unwrap();
        let image = media_bytes(PNG_MAGIC, 12 * 1_024 * 1_024);
        assert!(u64::try_from(image.len()).unwrap() > MAX_MEDIA_RANGE_CHUNK_BYTES);
        assert!(u64::try_from(image.len()).unwrap() <= game_media::MAX_IMAGE_BYTES);
        fs::write(media.join("hero-large.png"), &image).unwrap();

        let response = game_media_response(&media, &media_request("hero-large.png", None));
        assert_eq!(response.status(), tauri::http::StatusCode::OK);
        assert_eq!(response.body().as_slice(), image.as_slice());
        assert!(
            response
                .headers()
                .get(tauri::http::header::CONTENT_RANGE)
                .is_none()
        );
        assert_eq!(
            response
                .headers()
                .get(tauri::http::header::CONTENT_TYPE)
                .unwrap(),
            "image/png"
        );

        // A ranged request is still answered as a bounded 206.
        let ranged =
            game_media_response(&media, &media_request("hero-large.png", Some("bytes=0-")));
        assert_eq!(ranged.status(), tauri::http::StatusCode::PARTIAL_CONTENT);
        fs::remove_dir_all(root).unwrap();
    }

    /// A freshly imported local game must never wear another game's art: a
    /// title matching a bundled plate borrows that plate, and an unknown title
    /// gets the neutral placeholder rather than the Elden Ring fallback.
    #[test]
    fn local_games_without_artwork_get_bundled_or_neutral_presentation_art() {
        let mut stored = Catalog::default();
        let mut matched = imported_local_game_fixture();
        matched.id = "local-elden".into();
        matched.title = "EldenRing".into();
        let mut unknown = imported_local_game_fixture();
        unknown.id = "local-hozy".into();
        unknown.title = "Hozy Playtest".into();
        stored.games.push(matched);
        stored.games.push(unknown);

        let presentation = presentation_catalog(&stored, false);
        let matched = presentation
            .games
            .iter()
            .find(|game| game.id == "local-elden")
            .unwrap();
        assert_eq!(
            matched.cover_path.as_deref(),
            Some(Path::new("/media/igdb/covers/elden-ring.jpg"))
        );
        assert_eq!(
            matched.artwork_path.as_deref(),
            Some(Path::new("/media/igdb/heroes/elden-ring-wallpaper.png"))
        );
        let unknown = presentation
            .games
            .iter()
            .find(|game| game.id == "local-hozy")
            .unwrap();
        assert_eq!(
            unknown.cover_path.as_deref(),
            Some(Path::new(NEUTRAL_ARTWORK_PLACEHOLDER))
        );
        assert_eq!(
            unknown.artwork_path.as_deref(),
            Some(Path::new(NEUTRAL_ARTWORK_PLACEHOLDER))
        );
        // Presentation enrichment must stay out of the stored catalog so the
        // post-import artwork search still sees the gap.
        assert!(stored.games.iter().all(|game| game.cover_path.is_none()));
    }

    /// The Library and the detail page resolve artwork through one function.
    /// Asserting both here is what stops the two projections from drifting back
    /// apart: imported art used to be a `cache:` token in the Library and an
    /// empty string on the detail page.
    #[test]
    fn imported_cache_artwork_resolves_identically_on_the_library_and_the_detail_page() {
        let root = temporary_directory("media-agreement");
        let cache_dir = root.join(MEDIA_DIRECTORY);
        fs::create_dir_all(&cache_dir).unwrap();
        let artwork = cache_dir.join("0123456789abcdef-hero.jpg");
        let cover = cache_dir.join("0123456789abcdef-cover.jpg");
        fs::write(&artwork, b"\xff\xd8\xffhero").unwrap();
        fs::write(&cover, b"\xff\xd8\xffcover").unwrap();

        let mut game = imported_local_game_fixture();
        game.artwork_path = Some(artwork);
        game.cover_path = Some(cover);
        let mut stored = Catalog::default();
        stored.games.push(game.clone());

        let presentation = presentation_catalog(&stored, true);
        let library = game_view(&game, &presentation, Some(cache_dir.as_path()));

        let detail = Arc::new(
            GameDetailService::new(Arc::new(game_detail::GameStateStore::in_memory_for_tests()))
                .with_media_cache_dir(Some(cache_dir.clone())),
        );
        project_presentation_catalog(&detail, &stored).unwrap();
        let projected = game_detail::get_game_detail(&detail, game.id.clone())
            .unwrap()
            .unwrap();

        assert_eq!(
            library.cover_url.as_deref(),
            Some("cache:0123456789abcdef-cover.jpg")
        );
        assert_eq!(
            library.hero_url.as_deref(),
            Some("cache:0123456789abcdef-hero.jpg")
        );
        assert_eq!(projected.summary.cover_url, library.cover_url.unwrap());
        assert_eq!(projected.summary.hero_url, library.hero_url.unwrap());
        assert_eq!(
            projected.summary.landscape_url,
            library.landscape_url.unwrap()
        );
        assert_eq!(projected.media.len(), 2);
    }

    #[test]
    fn the_shared_media_resolver_refuses_traversal_and_anything_outside_its_roots() {
        let root = temporary_directory("media-resolver");
        let cache_dir = root.join(MEDIA_DIRECTORY);
        fs::create_dir_all(&cache_dir).unwrap();
        let outside = root.join("catalog.json");
        fs::write(&outside, b"private").unwrap();
        let cached = cache_dir.join("0123456789abcdef-cover.jpg");
        fs::write(&cached, b"\xff\xd8\xffcover").unwrap();
        let resolve =
            |path: PathBuf| media_source_url(Some(path.as_path()), Some(cache_dir.as_path()));

        // The two accepted shapes.
        assert_eq!(
            resolve(PathBuf::from("/media/igdb/covers/elden-ring.jpg")).as_deref(),
            Some("/media/igdb/covers/elden-ring.jpg")
        );
        assert_eq!(
            resolve(cached.clone()).as_deref(),
            Some("cache:0123456789abcdef-cover.jpg")
        );

        // Traversal out of either root, a Windows separator, a control
        // character, a file that only looks like it is in the cache, and a
        // directory are all refused rather than reinterpreted.
        assert_eq!(resolve(PathBuf::from("/media/../../etc/passwd")), None);
        assert_eq!(resolve(PathBuf::from("/media/..\\secret.png")), None);
        assert_eq!(resolve(PathBuf::from("/media/cover\n.png")), None);
        assert_eq!(resolve(cache_dir.join("..").join("catalog.json")), None);
        assert_eq!(resolve(outside), None);
        assert_eq!(resolve(cache_dir.clone()), None);
        assert_eq!(resolve(cache_dir.join("missing.jpg")), None);
        // A cache token may only ever name a bare opaque file.
        assert_eq!(resolve(cache_dir.join(".hidden.jpg")), None);
        // Without a cache directory only bundled media stays addressable.
        assert_eq!(media_source_url(Some(cached.as_path()), None), None);
        fs::remove_dir_all(root).unwrap();
    }

    const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
    /// An MP4 is recognised by `ftyp` at offset four, never by its extension.
    const MP4_MAGIC: &[u8] = b"\0\0\0\x18ftypisom";

    fn media_bytes(magic: &[u8], length: usize) -> Vec<u8> {
        let mut bytes = magic.to_vec();
        bytes.resize(length, 0x42);
        bytes
    }

    /// A local record whose artwork was copied into Orivo's own media cache.
    /// The title deliberately matches no showcase fixture, so the presentation
    /// catalog keeps the imported paths instead of borrowing bundled artwork.
    fn imported_local_game_fixture() -> Game {
        Game {
            id: "local-imported-artwork".into(),
            title: "Imported Artwork Fixture".into(),
            executable_path: Some(PathBuf::from("/Applications/Fixture.app")),
            source: GameSource::Local,
            source_id: None,
            launch_target: LaunchTarget::Direct,
            installation_path: None,
            working_directory: None,
            arguments: Vec::new(),
            description: Some("An imported local game.".into()),
            metadata: Some("Ready to play".into()),
            artwork_path: None,
            artwork_source_path: None,
            cover_path: None,
            cover_source_path: None,
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        }
    }

    fn temporary_directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "orivo-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn detail_projection() -> Arc<GameDetailService> {
        Arc::new(GameDetailService::new(Arc::new(
            game_detail::GameStateStore::in_memory_for_tests(),
        )))
    }

    /// The http-origin form the platforms rewrite `game-media:` into.
    fn media_request(file_name: &str, range: Option<&str>) -> tauri::http::Request<Vec<u8>> {
        let mut builder =
            tauri::http::Request::builder().uri(format!("http://game-media.localhost/{file_name}"));
        if let Some(range) = range {
            builder = builder.header(tauri::http::header::RANGE, range);
        }
        builder.body(Vec::new()).unwrap()
    }

    fn steam_game_fixture() -> steam::SteamGame {
        steam::SteamGame {
            app_id: 480,
            title: "Spacewar".into(),
            installation_path: PathBuf::from("/Users/example/Steam/steamapps/common/Spacewar"),
            manifest_path: PathBuf::from("/Users/example/Steam/steamapps/appmanifest_480.acf"),
            last_updated: Some(1_710_000_000),
            cover_path: Some(PathBuf::from(
                "/Users/example/Steam/appcache/librarycache/480_library_600x900.jpg",
            )),
            hero_path: Some(PathBuf::from(
                "/Users/example/Steam/appcache/librarycache/480_library_hero.jpg",
            )),
        }
    }

    fn owned_game_fixture() -> steam_account::OwnedSteamGame {
        steam_account::OwnedSteamGame {
            app_id: 480,
            title: "Spacewar".into(),
            play_time_seconds: 4_200,
        }
    }
}
