mod catalog;
mod launcher;
mod steam;
mod steam_account;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use catalog::{Catalog, Game, GameSource, LaunchTarget};
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, State, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
    webview::{NewWindowResponse, PageLoadEvent},
};

const CATALOG_FILE: &str = "catalog.json";
const MEDIA_DIRECTORY: &str = "media";
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
const STEAM_EXPLORE_URL: &str = "https://store.steampowered.com/explore/";
const STEAM_ACCOUNT_CONNECTED_EVENT: &str = "steam-account-authenticated";
const STEAM_ACCOUNT_LOGIN_CANCELLED_EVENT: &str = "steam-account-login-cancelled";
const STEAM_ACCOUNT_LOGIN_FAILED_EVENT: &str = "steam-account-login-failed";
const STEAM_ACCOUNT_LOGIN_PENDING_EVENT: &str = "steam-account-login-pending";
static MEDIA_CACHE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    catalog: RwLock<Catalog>,
    /// Serializes catalog mutations without forcing readers (launch and rail
    /// rendering) to wait for an atomic disk write.
    catalog_mutation: Mutex<()>,
    /// A short-lived Rust-only discovery snapshot avoids immediately parsing
    /// every manifest a second time just to hydrate the first preview images.
    steam_preview: Mutex<Option<SteamPreviewSnapshot>>,
    /// The active dedicated Steam sign-in window. It only tracks whether the
    /// one-time token extraction was consumed; no credential is held here.
    steam_auth_settled: Mutex<Option<Arc<AtomicBool>>>,
}

#[derive(Debug, Clone)]
struct SteamPreviewSnapshot {
    captured_at: Instant,
    games: BTreeMap<u32, steam::SteamGame>,
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

impl AppState {
    fn load(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let catalog_path = resolved_catalog_path(app)?;
        let mut catalog = load_or_migrate_catalog(&catalog_path)?;

        // Imported artwork is copied once into the app cache. That keeps the
        // browser's asset protocol tightly scoped and means a moved source
        // folder cannot break the selected game's visual state.
        if hydrate_catalog_media(app, &mut catalog)? {
            catalog.save_atomically(&catalog_path)?;
        }

        Ok(Self {
            catalog_path,
            catalog: RwLock::new(catalog),
            catalog_mutation: Mutex::new(()),
            steam_preview: Mutex::new(None),
            steam_auth_settled: Mutex::new(None),
        })
    }
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let state = AppState::load(app.handle())?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_library,
            import_game,
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
            launch_game,
            install_steam_game
        ])
        .run(tauri::generate_context!())
        .expect("error while running Orivo");
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
    let imported_id = game.id.clone();

    let response = {
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
        next_catalog
            .save_atomically(&state.catalog_path)
            .map_err(|error| error.to_string())?;
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
        library_state(&app, &catalog)
    };
    debug_assert!(response.games.iter().any(|game| game.id == imported_id));
    Ok(ImportResponse {
        games: response.games,
        imported_id: Some(imported_id),
    })
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
            next_catalog
                .save_atomically(&state.catalog_path)
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
            let _ = app_for_close.emit(STEAM_ACCOUNT_LOGIN_CANCELLED_EVENT, ());
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
                    let _ = app_for_callback.emit(STEAM_ACCOUNT_LOGIN_PENDING_EVENT, ());
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
                    let _ = app_for_callback.emit(
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
                    let _ =
                        app_for_callback.emit(STEAM_ACCOUNT_LOGIN_FAILED_EVENT, error.to_string());
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
            next_catalog
                .save_atomically(&state.catalog_path)
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

#[tauri::command]
async fn launch_game(game_id: String, state: State<'_, AppState>) -> Result<LaunchResult, String> {
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
                "This is a visual showcase. Import a local game to launch it.".to_string()
            })?
    };
    let title = game.title.clone();

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
        LaunchTarget::Direct => {
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
        imported_game.and_then(|game| media_source(game.cover_path.as_deref(), cache_dir));
    let hero_url =
        imported_game.and_then(|game| media_source(game.artwork_path.as_deref(), cache_dir));
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
            let cover_url = media_source(media.cover_path.as_deref(), cache_dir.as_deref());
            let hero_url = media_source(media.hero_path.as_deref(), cache_dir.as_deref());
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

fn load_or_migrate_catalog(path: &Path) -> Result<Catalog, Box<dyn std::error::Error>> {
    if path.is_file() {
        let loaded = Catalog::load_with_migration(path)?;
        if loaded.migrated_from.is_some() {
            backup_catalog(path)?;
            loaded.catalog.save_atomically(path)?;
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

fn backup_catalog(path: &Path) -> Result<(), std::io::Error> {
    let backup = path.with_extension("json.bak");
    fs::copy(path, backup)?;
    Ok(())
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
    let presentation = presentation_catalog(stored_catalog);
    LibraryState {
        games: presentation
            .games
            .iter()
            .map(|game| game_view(game, cache_dir.as_deref()))
            .collect(),
    }
}

fn game_view(game: &Game, cache_dir: Option<&Path>) -> GameView {
    let local_hero = media_source(game.artwork_path.as_deref(), cache_dir);
    let local_cover = media_source(game.cover_path.as_deref(), cache_dir);
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
        source: match game.source {
            GameSource::Steam => "steam",
            GameSource::Local => "local",
        }
        .into(),
        hero_url: steam_wallpaper.clone().or_else(|| local_hero.clone()),
        cover_url: local_cover
            .clone()
            .or_else(|| steam_cover.clone())
            .or_else(|| local_hero.clone())
            .or_else(|| steam_wallpaper.clone()),
        landscape_url: steam_landscape
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
        },
        host_platform: host_platform.into(),
        supported_platforms,
        compatible_with_host,
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
        _ => None,
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

fn media_source(path: Option<&Path>, cache_dir: Option<&Path>) -> Option<String> {
    let path = path?;
    let as_string = path.to_string_lossy();
    if as_string.starts_with("/media/") {
        return Some(as_string.into_owned());
    }
    let cache_dir = cache_dir?;
    if path.starts_with(cache_dir) && path.is_file() {
        return path
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .map(|file_name| format!("cache:{file_name}"));
    }
    None
}

/// The fixed rail is a visual fixture, not persisted user data. Local records
/// of matching games retain their launch configuration while borrowing the
/// bundled editorial artwork. Steam records replace a matching fixture whole:
/// their Store artwork, source badge and no-session state must stay authentic.
fn presentation_catalog(stored_catalog: &Catalog) -> Catalog {
    let mut presentation = showcase_catalog();

    for local_game in &stored_catalog.games {
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
        let view = game_view(&game, None);

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
        let view = game_view(&game, None);

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
        let view = game_view(&game, None);

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

        let presentation = presentation_catalog(&catalog);
        let game = presentation
            .games
            .iter()
            .find(|game| game.title == "Elden Ring")
            .unwrap();
        let view = game_view(game, None);

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
    fn catalog_migration_writes_a_backup_before_persisting_v2() {
        let root = std::env::temp_dir().join(format!(
            "orivo-catalog-migration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let catalog_path = root.join("catalog.json");
        fs::write(
            &catalog_path,
            r#"{"schema_version":1,"games":[{"id":"local","title":"Local","executable_path":"/Games/Local"}]}"#,
        )
        .unwrap();

        let catalog = load_or_migrate_catalog(&catalog_path).unwrap();
        let backup = catalog_path.with_extension("json.bak");

        assert_eq!(catalog.schema_version, catalog::CURRENT_SCHEMA_VERSION);
        assert!(
            fs::read_to_string(&backup)
                .unwrap()
                .contains("\"schema_version\":1")
        );
        assert!(
            fs::read_to_string(&catalog_path)
                .unwrap()
                .contains("\"schema_version\": 2")
        );
        fs::remove_dir_all(root).unwrap();
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
