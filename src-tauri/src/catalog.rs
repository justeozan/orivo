use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

pub const CURRENT_SCHEMA_VERSION: u32 = 2;
/// Keys reserved for Steam Store metadata cached on a game record. Keeping
/// these opaque values in the catalog means a temporary Store outage cannot
/// replace a real description or genre with a generic fallback on refresh.
/// v2 adds Store platform support. v1 records remain readable but are
/// refreshed once so their compatibility information can be completed.
pub const STEAM_STORE_METADATA_MARKER: &str = "orivo_steam_store_metadata_v2";
pub const LEGACY_STEAM_STORE_METADATA_MARKER: &str = "orivo_steam_store_metadata_v1";
pub const STEAM_STORE_GENRE_KEY: &str = "orivo_steam_genre";
pub const STEAM_STORE_PLATFORMS_KEY: &str = "orivo_steam_platforms";

/// The provider that owns the external identity of a library entry.  Catalog
/// records created before sources existed deserialize as `Local`, preserving
/// the v1 file format without a migration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum GameSource {
    #[default]
    Local,
    Steam,
}

/// A launch target is deliberately structured rather than represented as a
/// command string.  The WebView can request only a game id; the backend chooses
/// the fixed launch strategy for that record.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LaunchTarget {
    #[default]
    Direct,
    Steam {
        app_id: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Catalog {
    pub schema_version: u32,
    pub games: Vec<Game>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedCatalog {
    pub catalog: Catalog,
    pub migrated_from: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Game {
    pub id: String,
    pub title: String,
    /// Present only for direct local launches. Steam and future source-backed
    /// records deliberately do not pretend that a provider URI is a file.
    #[serde(default)]
    pub executable_path: Option<PathBuf>,
    #[serde(default)]
    pub source: GameSource,
    #[serde(default)]
    pub source_id: Option<String>,
    #[serde(default)]
    pub launch_target: LaunchTarget,
    #[serde(default)]
    pub installation_path: Option<PathBuf>,
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub metadata: Option<String>,
    #[serde(default)]
    pub artwork_path: Option<PathBuf>,
    /// Backend-only origin used to rebuild a scoped cache entry. This path is
    /// never returned to the WebView.
    #[serde(default)]
    pub artwork_source_path: Option<PathBuf>,
    #[serde(default)]
    pub cover_path: Option<PathBuf>,
    /// Backend-only origin used to rebuild a scoped cache entry. This path is
    /// never returned to the WebView.
    #[serde(default)]
    pub cover_source_path: Option<PathBuf>,
    #[serde(default)]
    pub logo_path: Option<PathBuf>,
    #[serde(default)]
    pub hero_video_path: Option<PathBuf>,
    #[serde(default)]
    pub last_played_at: Option<String>,
    #[serde(default)]
    pub play_time_seconds: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug)]
pub enum CatalogError {
    Io(io::Error),
    Json(serde_json::Error),
    UnsupportedSchema { found: u32, current: u32 },
    Invalid(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "catalog I/O error: {error}"),
            Self::Json(error) => write!(f, "catalog format error: {error}"),
            Self::UnsupportedSchema { found, current } => {
                write!(
                    f,
                    "catalog schema {found} is newer than supported schema {current}"
                )
            }
            Self::Invalid(message) => write!(f, "invalid catalog: {message}"),
        }
    }
}

impl std::error::Error for CatalogError {}

impl From<io::Error> for CatalogError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for CatalogError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            games: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Self, CatalogError> {
        Ok(Self::load_with_migration(path)?.catalog)
    }

    /// Read the oldest catalog format supported by this build and migrate it
    /// in memory. The caller decides when to create a backup and persist the
    /// migrated record, so a failed write cannot damage the source file.
    pub fn load_with_migration(path: &Path) -> Result<LoadedCatalog, CatalogError> {
        let contents = fs::read_to_string(path)?;
        let mut catalog: Self = serde_json::from_str(&contents)?;
        let migrated_from = match catalog.schema_version {
            CURRENT_SCHEMA_VERSION => None,
            1 => {
                migrate_v1_to_v2(&mut catalog);
                Some(1)
            }
            found => {
                return Err(CatalogError::UnsupportedSchema {
                    found,
                    current: CURRENT_SCHEMA_VERSION,
                });
            }
        };
        catalog.validate()?;
        Ok(LoadedCatalog {
            catalog,
            migrated_from,
        })
    }

    pub fn save_atomically(&self, path: &Path) -> Result<(), CatalogError> {
        self.validate()?;
        let json = serde_json::to_string_pretty(self)? + "\n";
        let temporary_path = path.with_extension("json.tmp");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&temporary_path, json)?;
        fs::rename(temporary_path, path)?;
        Ok(())
    }

    pub fn add(&mut self, game: Game) -> Result<(), CatalogError> {
        game.validate()?;
        if self.games.iter().any(|existing| existing.id == game.id) {
            return Err(CatalogError::Invalid(format!(
                "duplicate game id: {}",
                game.id
            )));
        }
        if let Some(source_id) = game.source_id.as_deref()
            && self.games.iter().any(|existing| {
                existing.source == game.source && existing.source_id.as_deref() == Some(source_id)
            })
        {
            return Err(CatalogError::Invalid(format!(
                "duplicate source id {} for {:?}",
                source_id, game.source
            )));
        }
        self.games.push(game);
        Ok(())
    }

    /// Insert a Steam record or replace its provider-owned fields on refresh.
    /// Keeping this as a catalog operation makes repeated imports idempotent
    /// and prevents a second library scan from creating duplicate rail cards.
    /// Returns `true` for a new record and `false` for a refresh.
    pub fn upsert_steam(&mut self, mut game: Game) -> Result<bool, CatalogError> {
        game.validate()?;
        if game.source != GameSource::Steam {
            return Err(CatalogError::Invalid(
                "upsert_steam requires a Steam source record".into(),
            ));
        }
        let source_id = game.source_id.clone().ok_or_else(|| {
            CatalogError::Invalid("steam game requires a stable source id".into())
        })?;
        let incoming_has_store_metadata = game.extra.contains_key(STEAM_STORE_METADATA_MARKER);

        if let Some(index) = self.games.iter().position(|existing| {
            existing.source == GameSource::Steam
                && existing.source_id.as_deref() == Some(source_id.as_str())
        }) {
            let existing = &self.games[index];
            // Store metadata is best-effort. Once a complete Store response
            // has been persisted, retain its human-readable description if a
            // later sync cannot reach that public endpoint.
            if has_steam_store_copy(&existing.extra) && !incoming_has_store_metadata {
                game.description = existing.description.clone();
            }
            // The current v1 catalog does not expose editable Steam-specific
            // preferences yet. Preserve opaque fields so future user-owned
            // metadata remains intact across refreshes.
            for (key, value) in &existing.extra {
                game.extra
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
            if incoming_has_store_metadata {
                game.extra.remove(LEGACY_STEAM_STORE_METADATA_MARKER);
            }
            if game.last_played_at.is_none() {
                game.last_played_at = existing.last_played_at.clone();
            }
            if game.play_time_seconds == 0 {
                game.play_time_seconds = existing.play_time_seconds;
            }
            // Steam's artwork cache may be pruned or temporarily unavailable
            // during a refresh. Keep the prior scoped cache references rather
            // than making an already-present game visually regress.
            if game.artwork_path.is_none() {
                game.artwork_path = existing.artwork_path.clone();
            }
            if game.cover_path.is_none() {
                game.cover_path = existing.cover_path.clone();
            }
            if game.artwork_source_path.is_none() {
                game.artwork_source_path = existing.artwork_source_path.clone();
            }
            if game.cover_source_path.is_none() {
                game.cover_source_path = existing.cover_source_path.clone();
            }
            self.games[index] = game;
            return Ok(false);
        }

        if self.games.iter().any(|existing| existing.id == game.id) {
            return Err(CatalogError::Invalid(format!(
                "game id {} already belongs to another source",
                game.id
            )));
        }
        self.games.push(game);
        Ok(true)
    }

    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(CatalogError::UnsupportedSchema {
                found: self.schema_version,
                current: CURRENT_SCHEMA_VERSION,
            });
        }
        let mut ids = BTreeSet::new();
        let mut source_ids = BTreeSet::new();
        for game in &self.games {
            game.validate()?;
            if !ids.insert(game.id.clone()) {
                return Err(CatalogError::Invalid(format!(
                    "duplicate game id: {}",
                    game.id
                )));
            }
            if let Some(source_id) = game.source_id.as_ref()
                && !source_ids.insert((game.source.clone(), source_id.clone()))
            {
                return Err(CatalogError::Invalid(format!(
                    "duplicate source id {} for {:?}",
                    source_id, game.source
                )));
            }
        }
        Ok(())
    }
}

fn has_steam_store_copy(extra: &BTreeMap<String, serde_json::Value>) -> bool {
    extra.contains_key(STEAM_STORE_METADATA_MARKER)
        || extra.contains_key(LEGACY_STEAM_STORE_METADATA_MARKER)
}

fn migrate_v1_to_v2(catalog: &mut Catalog) {
    // v1 records always represented direct executable launches. The v2
    // fields deserialize with defaults, so upgrading is deterministic and
    // does not invent provider-owned data.
    catalog.schema_version = CURRENT_SCHEMA_VERSION;
}

pub fn default_path() -> PathBuf {
    if let Some(path) = std::env::var_os("ORIVO_CATALOG_PATH") {
        return PathBuf::from(path);
    }

    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Library/Application Support/Orivo/catalog.json");
    }

    #[cfg(target_os = "windows")]
    if let Some(app_data) = std::env::var_os("APPDATA") {
        return PathBuf::from(app_data).join("Orivo/catalog.json");
    }

    PathBuf::from("orivo-catalog.json")
}

impl Game {
    pub fn from_executable(path: impl Into<PathBuf>) -> Result<Self, CatalogError> {
        let selected_path = path.into();
        let executable_path = resolve_executable(&selected_path)?;
        let title_path = if selected_path
            .extension()
            .is_some_and(|extension| extension == "app")
        {
            selected_path.clone()
        } else {
            executable_path.clone()
        };
        let title = executable_path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| {
                title_path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .ok_or_else(|| CatalogError::Invalid("executable has no usable filename".into()))?;
        let title = if selected_path
            .extension()
            .is_some_and(|extension| extension == "app")
        {
            bundle_display_name(&selected_path).unwrap_or(title)
        } else {
            title
        };
        let artwork_path = discover_artwork(&selected_path, &executable_path);
        let id = executable_path.to_string_lossy().to_string();

        Ok(Self {
            id,
            title,
            working_directory: executable_path.parent().map(Path::to_path_buf),
            executable_path: Some(executable_path),
            source: GameSource::Local,
            source_id: None,
            launch_target: LaunchTarget::Direct,
            installation_path: None,
            arguments: Vec::new(),
            description: None,
            metadata: None,
            artwork_source_path: artwork_path.clone(),
            artwork_path,
            cover_source_path: None,
            cover_path: None,
            logo_path: None,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        })
    }

    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.id.trim().is_empty() {
            return Err(CatalogError::Invalid("game id cannot be empty".into()));
        }
        if self.title.trim().is_empty() {
            return Err(CatalogError::Invalid(format!(
                "game {} has no title",
                self.id
            )));
        }
        match (&self.source, &self.source_id, &self.launch_target) {
            (GameSource::Local, _, LaunchTarget::Direct)
                if self
                    .executable_path
                    .as_ref()
                    .is_none_or(|path| path.as_os_str().is_empty()) =>
            {
                return Err(CatalogError::Invalid(format!(
                    "game {} has no executable",
                    self.id
                )));
            }
            (GameSource::Local, _, LaunchTarget::Direct) => {}
            (GameSource::Steam, Some(source_id), LaunchTarget::Steam { app_id })
                if *app_id > 0 && source_id == &app_id.to_string() => {}
            _ => {
                return Err(CatalogError::Invalid(format!(
                    "game {} has an invalid source or launch target",
                    self.id
                )));
            }
        }
        Ok(())
    }
}

fn resolve_executable(path: &Path) -> Result<PathBuf, CatalogError> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }

    if !path.exists() && path.extension().is_none_or(|extension| extension != "app") {
        return Ok(path.to_path_buf());
    }

    if path.extension().is_some_and(|extension| extension == "app") && path.is_dir() {
        let info_path = path.join("Contents/Info.plist");
        let executable_name = plist::Value::from_file(&info_path)
            .ok()
            .and_then(|value| value.into_dictionary())
            .and_then(|dictionary| dictionary.get("CFBundleExecutable").cloned())
            .and_then(|value| value.into_string());
        if let Some(executable_name) = executable_name {
            let executable = path.join("Contents/MacOS").join(executable_name);
            if executable.is_file() {
                return Ok(executable);
            }
        }
    }

    Err(CatalogError::Invalid(format!(
        "could not resolve an executable from {}",
        path.display()
    )))
}

fn bundle_display_name(path: &Path) -> Option<String> {
    let info_path = path.join("Contents/Info.plist");
    plist::Value::from_file(info_path)
        .ok()
        .and_then(|value| value.into_dictionary())
        .and_then(|dictionary| {
            dictionary
                .get("CFBundleDisplayName")
                .or_else(|| dictionary.get("CFBundleName"))
                .cloned()
        })
        .and_then(|value| value.into_string())
}

fn discover_artwork(selected_path: &Path, executable_path: &Path) -> Option<PathBuf> {
    let mut directories = Vec::new();
    if selected_path
        .extension()
        .is_some_and(|extension| extension == "app")
    {
        directories.push(selected_path.join("Contents/Resources"));
    }
    if let Some(parent) = executable_path.parent() {
        directories.push(parent.to_path_buf());
    }

    directories.into_iter().find_map(|directory| {
        let mut candidates = std::fs::read_dir(directory)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path.extension().is_some_and(|extension| {
                        matches!(
                            extension.to_str(),
                            Some("png" | "jpg" | "jpeg" | "bmp" | "webp")
                        )
                    })
            })
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.into_iter().next()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_manual_import_from_an_executable() {
        let game = Game::from_executable("/Games/Nightfall/Nightfall.app/Contents/MacOS/Nightfall")
            .unwrap();

        assert_eq!(game.title, "Nightfall");
        assert_eq!(
            game.working_directory,
            Some(PathBuf::from(
                "/Games/Nightfall/Nightfall.app/Contents/MacOS"
            ))
        );
        assert!(game.arguments.is_empty());
    }

    #[test]
    fn rejects_a_future_schema_without_mutating_data() {
        let catalog = Catalog {
            schema_version: CURRENT_SCHEMA_VERSION + 1,
            games: Vec::new(),
            extra: BTreeMap::new(),
        };

        assert!(matches!(
            catalog.validate(),
            Err(CatalogError::UnsupportedSchema { .. })
        ));
    }

    #[test]
    fn upgrades_a_v1_direct_game_in_memory_without_rewriting_its_source_file() {
        let path = temporary_catalog_path("v1-load");
        let v1 = r#"{
  "schema_version": 1,
  "games": [
    {
      "id": "local-example",
      "title": "Example",
      "executable_path": "/Games/Example.app/Contents/MacOS/Example"
    }
  ]
}"#;
        fs::write(&path, v1).unwrap();

        let loaded = Catalog::load_with_migration(&path).unwrap();

        assert_eq!(loaded.migrated_from, Some(1));
        assert_eq!(loaded.catalog.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.catalog.games[0].source, GameSource::Local);
        assert_eq!(loaded.catalog.games[0].launch_target, LaunchTarget::Direct);
        assert_eq!(fs::read_to_string(&path).unwrap(), v1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_duplicate_game_ids() {
        let game = Game::from_executable("/Games/Nightfall").unwrap();
        let mut catalog = Catalog::default();
        catalog.add(game.clone()).unwrap();

        assert!(
            matches!(catalog.add(game), Err(CatalogError::Invalid(message)) if message.contains("duplicate game id"))
        );
    }

    #[test]
    fn rejects_duplicate_provider_records_in_a_persisted_catalog() {
        let path = temporary_catalog_path("duplicate-steam-id");
        let first = steam_game("Spacewar");
        let mut duplicate = steam_game("Spacewar duplicate");
        duplicate.id = "steam:480-copy".into();
        let catalog = Catalog {
            schema_version: CURRENT_SCHEMA_VERSION,
            games: vec![first, duplicate],
            extra: BTreeMap::new(),
        };
        fs::write(&path, serde_json::to_string(&catalog).unwrap()).unwrap();

        assert!(
            matches!(Catalog::load(&path), Err(CatalogError::Invalid(message)) if message.contains("duplicate source id"))
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn validates_a_typed_steam_launch_target() {
        let game = Game {
            id: "steam:480".into(),
            title: "Spacewar".into(),
            executable_path: None,
            source: GameSource::Steam,
            source_id: Some("480".into()),
            launch_target: LaunchTarget::Steam { app_id: 480 },
            installation_path: Some(PathBuf::from("/Games/Spacewar")),
            working_directory: None,
            arguments: Vec::new(),
            description: None,
            metadata: None,
            artwork_path: None,
            artwork_source_path: None,
            cover_path: None,
            cover_source_path: None,
            logo_path: None,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        };

        assert!(game.validate().is_ok());
    }

    #[test]
    fn rejects_a_steam_game_without_a_matching_app_id() {
        let game = Game {
            id: "steam:480".into(),
            title: "Spacewar".into(),
            executable_path: None,
            source: GameSource::Steam,
            source_id: Some("481".into()),
            launch_target: LaunchTarget::Steam { app_id: 480 },
            installation_path: None,
            working_directory: None,
            arguments: Vec::new(),
            description: None,
            metadata: None,
            artwork_path: None,
            artwork_source_path: None,
            cover_path: None,
            cover_source_path: None,
            logo_path: None,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        };

        assert!(matches!(game.validate(), Err(CatalogError::Invalid(_))));
    }

    #[test]
    fn rejects_a_local_game_that_claims_a_steam_target() {
        let game = Game {
            id: "invalid".into(),
            title: "Invalid".into(),
            executable_path: None,
            source: GameSource::Local,
            source_id: None,
            launch_target: LaunchTarget::Steam { app_id: 480 },
            installation_path: None,
            working_directory: None,
            arguments: Vec::new(),
            description: None,
            metadata: None,
            artwork_path: None,
            artwork_source_path: None,
            cover_path: None,
            cover_source_path: None,
            logo_path: None,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        };

        assert!(matches!(game.validate(), Err(CatalogError::Invalid(_))));
    }

    #[test]
    fn refreshes_steam_games_by_external_id_without_duplication() {
        let mut catalog = Catalog::default();
        let first = steam_game("Spacewar");
        assert!(catalog.upsert_steam(first).unwrap());

        let refreshed = steam_game("Spacewar (updated)");
        assert!(!catalog.upsert_steam(refreshed).unwrap());

        assert_eq!(catalog.games.len(), 1);
        assert_eq!(catalog.games[0].title, "Spacewar (updated)");
    }

    #[test]
    fn refresh_keeps_a_cached_artwork_when_the_source_is_temporarily_missing() {
        let mut catalog = Catalog::default();
        let mut first = steam_game("Spacewar");
        first.cover_path = Some(PathBuf::from("/cache/steam-cover.jpg"));
        first.cover_source_path = Some(PathBuf::from("/steam/cache/480_cover.jpg"));
        catalog.upsert_steam(first).unwrap();

        let mut refreshed = steam_game("Spacewar");
        refreshed.cover_source_path = Some(PathBuf::from("/steam/cache/480_new_cover.jpg"));
        catalog.upsert_steam(refreshed).unwrap();

        assert_eq!(
            catalog.games[0].cover_path,
            Some(PathBuf::from("/cache/steam-cover.jpg"))
        );
        assert_eq!(
            catalog.games[0].cover_source_path,
            Some(PathBuf::from("/steam/cache/480_new_cover.jpg"))
        );
    }

    #[test]
    fn refresh_keeps_cached_store_copy_when_the_public_lookup_is_unavailable() {
        let mut catalog = Catalog::default();
        let mut first = steam_game("Spacewar");
        first.description = Some("A real Steam short description.".into());
        first.extra.insert(
            STEAM_STORE_METADATA_MARKER.into(),
            serde_json::Value::Bool(true),
        );
        first.extra.insert(
            STEAM_STORE_GENRE_KEY.into(),
            serde_json::Value::String("Action".into()),
        );
        catalog.upsert_steam(first).unwrap();

        let mut refreshed = steam_game("Spacewar");
        refreshed.description = Some("Owned on Steam. Install it in Steam to play.".into());
        catalog.upsert_steam(refreshed).unwrap();

        assert_eq!(
            catalog.games[0].description.as_deref(),
            Some("A real Steam short description.")
        );
        assert_eq!(
            catalog.games[0]
                .extra
                .get(STEAM_STORE_GENRE_KEY)
                .and_then(serde_json::Value::as_str),
            Some("Action")
        );
    }

    fn steam_game(title: &str) -> Game {
        Game {
            id: "steam:480".into(),
            title: title.into(),
            executable_path: None,
            source: GameSource::Steam,
            source_id: Some("480".into()),
            launch_target: LaunchTarget::Steam { app_id: 480 },
            installation_path: None,
            working_directory: None,
            arguments: Vec::new(),
            description: None,
            metadata: None,
            artwork_path: None,
            artwork_source_path: None,
            cover_path: None,
            cover_source_path: None,
            logo_path: None,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        }
    }

    fn temporary_catalog_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "orivo-catalog-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn resolves_a_macos_app_bundle_to_its_declared_executable() {
        let root = std::env::temp_dir().join(format!("orivo-app-test-{}", std::process::id()));
        let bundle = root.join("Unrailed!.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleDisplayName</key><string>Unrailed!</string>
<key>CFBundleExecutable</key><string>UnrailedGame</string>
</dict></plist>"#,
        )
        .unwrap();
        std::fs::write(macos.join("UnrailedGame"), "#!/bin/sh\n").unwrap();

        let game = Game::from_executable(&bundle).unwrap();

        assert_eq!(game.title, "Unrailed!");
        assert_eq!(game.executable_path, Some(macos.join("UnrailedGame")));
        std::fs::remove_dir_all(root).unwrap();
    }
}
