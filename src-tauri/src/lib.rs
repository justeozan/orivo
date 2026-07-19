mod catalog;
mod launcher;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

use catalog::{Catalog, Game};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

const CATALOG_FILE: &str = "catalog.json";
const MEDIA_DIRECTORY: &str = "media";

/// The backend deliberately owns every executable path and launch argument.
/// The WebView only ever sees presentation data and can ask to launch a stable
/// game id, never an arbitrary system command.
struct AppState {
    catalog_path: PathBuf,
    catalog: RwLock<Catalog>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameView {
    id: String,
    title: String,
    description: String,
    metadata: String,
    genre: String,
    hero_url: Option<String>,
    cover_url: Option<String>,
    last_played_at: String,
    play_time_seconds: u64,
    launchable: bool,
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
        })
    }
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let state = AppState::load(&app.handle())?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_library,
            import_game,
            launch_game
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
    cache_game_media(&app, &mut game).map_err(|error| error.to_string())?;
    let imported_id = game.id.clone();

    let mut catalog = state
        .catalog
        .write()
        .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
    catalog.add(game).map_err(|error| error.to_string())?;
    catalog
        .save_atomically(&state.catalog_path)
        .map_err(|error| error.to_string())?;

    let response = library_state(&app, &catalog);
    debug_assert!(response.games.iter().any(|game| game.id == imported_id));
    Ok(ImportResponse {
        games: response.games,
        imported_id: Some(imported_id),
    })
}

#[tauri::command]
fn launch_game(game_id: String, state: State<'_, AppState>) -> Result<LaunchResult, String> {
    let game = state
        .catalog
        .read()
        .map_err(|_| "The game catalog is temporarily unavailable".to_string())?
        .games
        .iter()
        .find(|game| game.id == game_id)
        .cloned()
        .ok_or_else(|| {
            "This is a visual showcase. Import a local game to launch it.".to_string()
        })?;

    launcher::launch(&game).map_err(|error| error.to_string())?;
    Ok(LaunchResult {
        status: format!("Launching {}", game.title),
    })
}

fn resolved_catalog_path(app: &AppHandle) -> Result<PathBuf, tauri::Error> {
    if let Some(path) = std::env::var_os("ORIVO_CATALOG_PATH") {
        return Ok(PathBuf::from(path));
    }
    Ok(app.path().app_data_dir()?.join(CATALOG_FILE))
}

fn load_or_migrate_catalog(path: &Path) -> Result<Catalog, Box<dyn std::error::Error>> {
    if path.is_file() {
        return Ok(Catalog::load(path)?);
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

fn media_cache_dir(app: &AppHandle) -> Result<PathBuf, tauri::Error> {
    Ok(app.path().app_cache_dir()?.join(MEDIA_DIRECTORY))
}

fn hydrate_catalog_media(
    app: &AppHandle,
    catalog: &mut Catalog,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut changed = false;
    for game in &mut catalog.games {
        changed |= cache_game_media(app, game)?;
    }
    Ok(changed)
}

/// Copy only explicitly discovered artwork into `$APPCACHE/media`. The app
/// config grants the WebView access to this one directory and nowhere else.
fn cache_game_media(app: &AppHandle, game: &mut Game) -> Result<bool, std::io::Error> {
    let cache_dir = media_cache_dir(app).map_err(std::io::Error::other)?;
    fs::create_dir_all(&cache_dir)?;
    let mut changed = false;

    if let Some(source) = game.artwork_path.clone()
        && let Some(cached) = cache_media_file(&cache_dir, &game.id, "hero", &source)?
    {
        changed |= cached != source;
        game.artwork_path = Some(cached);
    }

    if let Some(source) = game.cover_path.clone()
        && let Some(cached) = cache_media_file(&cache_dir, &game.id, "cover", &source)?
    {
        changed |= cached != source;
        game.cover_path = Some(cached);
    }

    Ok(changed)
}

fn cache_media_file(
    cache_dir: &Path,
    game_id: &str,
    role: &str,
    source: &Path,
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

    if !target.is_file() {
        fs::copy(source, &target)?;
    }
    Ok(Some(target))
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
        genre: genre_for_game(game).into(),
        hero_url: media_source(game.artwork_path.as_deref(), cache_dir),
        cover_url: media_source(game.cover_path.as_deref(), cache_dir)
            .or_else(|| media_source(game.artwork_path.as_deref(), cache_dir)),
        last_played_at: game.last_played_at.clone().unwrap_or_default(),
        play_time_seconds: game.play_time_seconds,
        launchable: !game.id.starts_with("showcase-"),
    }
}

fn genre_for_game(game: &Game) -> &'static str {
    match game_key(&game.title).as_str() {
        "eldenring" | "baldursgate3" | "thewitcher3" => "RPG",
        "cyberpunk2077" => "Action RPG",
        "hadesii" => "Roguelike",
        "reddeadredemption2" | "horizonforbiddenwest" => "Adventure",
        "godofwar" | "astroduel2" => "Action",
        "unrailed" => "Co-op",
        _ => "Library",
    }
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
/// bundled editorial artwork, which keeps the selector faithful to the design.
fn presentation_catalog(stored_catalog: &Catalog) -> Catalog {
    let mut presentation = showcase_catalog();

    for local_game in &stored_catalog.games {
        if let Some(showcase_game) = presentation
            .games
            .iter_mut()
            .find(|showcase_game| game_key(&showcase_game.title) == game_key(&local_game.title))
        {
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
        executable_path: PathBuf::from(format!("showcase://{id}")),
        working_directory: None,
        arguments: Vec::new(),
        description: Some(description.into()),
        metadata: Some(metadata.into()),
        artwork_path,
        cover_path: Some(cover_path),
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
                .to_string_lossy()
                .starts_with("showcase://")
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
}
