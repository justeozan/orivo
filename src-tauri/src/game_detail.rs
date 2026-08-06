//! Composable Game Detail projection and durable user state.
//!
//! The Tauri root intentionally owns command registration. This module keeps
//! the IPC-shaped functions public while accepting an explicit service, so the
//! private application state and filesystem roots never have to be exposed to
//! the WebView.

use crate::catalog::{
    Catalog, Game, GameSource as CatalogGameSource, LaunchTarget, SOURCE_COVER_URL_KEY,
    SOURCE_GENRE_KEY, SOURCE_HERO_URL_KEY, SOURCE_LANDSCAPE_URL_KEY, STEAM_STORE_GENRE_KEY,
    STEAM_STORE_PLATFORMS_KEY, WINE_STAGING_RUNNER_ID,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

const GAME_STATE_SCHEMA_VERSION: u32 = 1;
const MAX_OPAQUE_ID_BYTES: usize = 512;
/// How many `game-state.corrupt-<n>.json` slots may accumulate next to the
/// live document before quarantine gives up and lets the unusable file be
/// replaced. Repeated corruption is already preserved by the earlier slots.
const QUARANTINE_SLOT_LIMIT: u32 = 64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum GameMediaKind {
    Wallpaper,
    Video,
    Icon,
    Cover,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GameMediaOrigin {
    Bundled,
    Provider,
    Imported,
    Downloaded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GameMediaView {
    pub id: String,
    pub kind: GameMediaKind,
    pub title: String,
    pub preview_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster_url: Option<String>,
    pub origin: GameMediaOrigin,
    pub selected: bool,
    pub available_offline: bool,
}

/// Host-owned media metadata. It may contain a provider URL, but never a host
/// filesystem path. `local_file` is a single opaque filename resolved against
/// the service-owned media directory only after validation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GameMediaAsset {
    pub id: String,
    pub kind: GameMediaKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster_url: Option<String>,
    pub origin: GameMediaOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub byte_size: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl GameMediaAsset {
    pub fn view(&self, selected: bool) -> GameMediaView {
        let preview_url = self
            .local_file
            .as_deref()
            .map(|file| format!("game-media:{file}"))
            .or_else(|| self.source_url.clone())
            .unwrap_or_default();
        GameMediaView {
            id: self.id.clone(),
            kind: self.kind,
            title: self.title.clone(),
            preview_url,
            poster_url: self.poster_url.clone(),
            origin: self.origin,
            selected,
            available_offline: self.local_file.is_some() || self.origin == GameMediaOrigin::Bundled,
        }
    }

    pub fn validate(&self) -> Result<(), GameDetailError> {
        validate_opaque_id("media id", &self.id)?;
        validate_display_text("media title", &self.title, 256)?;
        if let Some(file) = self.local_file.as_deref()
            && !valid_opaque_file_name(file)
        {
            return Err(GameDetailError::Invalid(
                "media file reference is not opaque".into(),
            ));
        }
        if self.source_url.is_none() && self.local_file.is_none() {
            return Err(GameDetailError::Invalid(
                "media has no available source".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GameSourceView {
    Steam,
    Wine,
    Local,
    Showcase,
    Store,
    Epic,
    Gog,
    Ubisoft,
    Xbox,
    MicrosoftStore,
    InstantGaming,
}

impl GameSourceView {
    /// The one place the catalog's connected-store sources become view sources.
    /// The Library projection and this detail projection both go through it, so
    /// the two can never disagree about which badge a game wears.
    pub fn from_catalog_source(source: CatalogGameSource) -> Option<Self> {
        match source {
            CatalogGameSource::Epic => Some(Self::Epic),
            CatalogGameSource::Gog => Some(Self::Gog),
            CatalogGameSource::Ubisoft => Some(Self::Ubisoft),
            CatalogGameSource::Xbox => Some(Self::Xbox),
            CatalogGameSource::MicrosoftStore => Some(Self::MicrosoftStore),
            CatalogGameSource::InstantGaming => Some(Self::InstantGaming),
            CatalogGameSource::Local | CatalogGameSource::Steam => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PlatformView {
    Windows,
    Macos,
    Linux,
    Ios,
    Android,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PrimaryAction {
    Play,
    InstallSteam,
    ConfigureWine,
    ViewOffer,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StoreProviderView {
    Steam,
    Ubisoft,
    Microsoft,
    Apple,
    GooglePlay,
    InstantGaming,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoreOfferView {
    pub id: String,
    pub game_id: String,
    pub provider: StoreProviderView,
    pub provider_label: String,
    pub price_minor: Option<u64>,
    pub currency: Option<String>,
    pub region: String,
    pub verified_at: Option<String>,
    pub availability: OfferAvailability,
    pub stale: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OfferAvailability {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AchievementItemView {
    pub id: String,
    pub title: String,
    pub icon_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AchievementsView {
    pub unlocked: u32,
    pub total: u32,
    pub items: Vec<AchievementItemView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GameSummaryView {
    pub id: String,
    pub title: String,
    pub source: GameSourceView,
    pub short_description: String,
    pub cover_url: String,
    pub hero_url: String,
    pub landscape_url: String,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub supported_platforms: Vec<PlatformView>,
    pub owned: bool,
    pub launchable: bool,
    pub wishlisted: bool,
    pub play_time_seconds: u64,
    pub last_played_at: Option<String>,
    pub recommendation_reasons: Vec<String>,
    pub offers: Vec<StoreOfferView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GameDetailView {
    #[serde(flatten)]
    pub summary: GameSummaryView,
    pub about: String,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub release_date: Option<String>,
    pub features: Vec<String>,
    pub achievements: Option<AchievementsView>,
    pub media: Vec<GameMediaView>,
    pub related_games: Vec<GameSummaryView>,
    pub primary_action: PrimaryAction,
}

/// Normalised provider/library record accepted by the detail service. Store
/// adapters can construct this directly; catalog projection uses the same
/// shape, so title equality is never an identity or merge key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameDetailRecord {
    pub summary: GameSummaryView,
    pub about: String,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub release_date: Option<String>,
    pub features: Vec<String>,
    pub achievements: Option<AchievementsView>,
    pub media: Vec<GameMediaAsset>,
    pub related_games: Vec<GameSummaryView>,
    pub primary_action: PrimaryAction,
}

impl GameDetailRecord {
    pub fn validate(&self) -> Result<(), GameDetailError> {
        validate_opaque_id("game id", &self.summary.id)?;
        validate_display_text("game title", &self.summary.title, 512)?;
        let mut media_ids = BTreeSet::new();
        for media in &self.media {
            media.validate()?;
            if !media_ids.insert(media.id.as_str()) {
                return Err(GameDetailError::Invalid("duplicate media id".into()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersistedGameState {
    #[serde(default)]
    pub wishlisted: bool,
    #[serde(default)]
    pub selected_media: BTreeMap<GameMediaKind, String>,
    #[serde(default)]
    pub media: BTreeMap<String, GameMediaAsset>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GameStateDocument {
    pub schema_version: u32,
    #[serde(default)]
    pub games: BTreeMap<String, PersistedGameState>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for GameStateDocument {
    fn default() -> Self {
        Self {
            schema_version: GAME_STATE_SCHEMA_VERSION,
            games: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Debug)]
pub struct GameStateStore {
    path: PathBuf,
    document: Mutex<GameStateDocument>,
}

impl GameStateStore {
    /// Wishlist and media selections are recoverable user state, not critical
    /// data. An unreadable, corrupt, or unknown-version document is therefore
    /// moved aside instead of failing the load: refusing to start would leave
    /// the user with no way back into the application at all.
    pub fn load(path: PathBuf) -> Result<Self, GameDetailError> {
        let document = match read_state_file(&path) {
            LoadedStateFile::Valid { document, .. } => document,
            LoadedStateFile::Missing | LoadedStateFile::Quarantined => GameStateDocument::default(),
        };
        Ok(Self {
            path,
            document: Mutex::new(document),
        })
    }

    pub fn in_memory_for_tests() -> Self {
        Self {
            path: PathBuf::new(),
            document: Mutex::new(GameStateDocument::default()),
        }
    }

    pub fn snapshot(&self) -> Result<GameStateDocument, GameDetailError> {
        Ok(self.lock()?.clone())
    }

    pub fn wishlisted(&self, game_id: &str) -> Result<bool, GameDetailError> {
        Ok(self
            .lock()?
            .games
            .get(game_id)
            .is_some_and(|state| state.wishlisted))
    }

    pub fn set_wishlist(&self, game_id: &str, wishlisted: bool) -> Result<(), GameDetailError> {
        validate_opaque_id("game id", game_id)?;
        self.mutate(|document| {
            document
                .games
                .entry(game_id.to_owned())
                .or_default()
                .wishlisted = wishlisted;
            Ok(())
        })
    }

    pub fn media_for_game(&self, game_id: &str) -> Result<Vec<GameMediaAsset>, GameDetailError> {
        Ok(self
            .lock()?
            .games
            .get(game_id)
            .map(|state| state.media.values().cloned().collect())
            .unwrap_or_default())
    }

    pub fn selected_media(
        &self,
        game_id: &str,
    ) -> Result<BTreeMap<GameMediaKind, String>, GameDetailError> {
        Ok(self
            .lock()?
            .games
            .get(game_id)
            .map(|state| state.selected_media.clone())
            .unwrap_or_default())
    }

    pub fn register_media(
        &self,
        game_id: &str,
        media: GameMediaAsset,
    ) -> Result<(), GameDetailError> {
        validate_opaque_id("game id", game_id)?;
        media.validate()?;
        self.mutate(|document| {
            document
                .games
                .entry(game_id.to_owned())
                .or_default()
                .media
                .insert(media.id.clone(), media);
            Ok(())
        })
    }

    /// Register and select in a single validated mutation. Applying new media
    /// must be all-or-nothing: a rejected document leaves both the media table
    /// and the previous selection exactly as they were, on disk and in memory.
    pub fn register_and_select_media(
        &self,
        game_id: &str,
        media: GameMediaAsset,
    ) -> Result<(), GameDetailError> {
        validate_opaque_id("game id", game_id)?;
        media.validate()?;
        self.mutate(|document| {
            let state = document.games.entry(game_id.to_owned()).or_default();
            state.selected_media.insert(media.kind, media.id.clone());
            state.media.insert(media.id.clone(), media);
            Ok(())
        })
    }

    pub fn select_media(&self, game_id: &str, media_id: &str) -> Result<(), GameDetailError> {
        validate_opaque_id("game id", game_id)?;
        validate_opaque_id("media id", media_id)?;
        self.mutate(|document| {
            let state = document
                .games
                .get_mut(game_id)
                .ok_or(GameDetailError::NotFound)?;
            let media = state.media.get(media_id).ok_or(GameDetailError::NotFound)?;
            state.selected_media.insert(media.kind, media_id.to_owned());
            Ok(())
        })
    }

    pub fn protected_local_files(&self) -> Result<BTreeSet<String>, GameDetailError> {
        let document = self.lock()?;
        let mut protected = BTreeSet::new();
        for state in document.games.values() {
            for (kind, media_id) in &state.selected_media {
                if let Some(media) = state.media.get(media_id)
                    && media.kind == *kind
                    && let Some(file) = media.local_file.as_ref()
                {
                    protected.insert(file.clone());
                }
            }
            // Imported files are user data, not a derived cache, even before
            // they become the active selection.
            for media in state.media.values() {
                if media.origin == GameMediaOrigin::Imported
                    && let Some(file) = media.local_file.as_ref()
                {
                    protected.insert(file.clone());
                }
            }
        }
        Ok(protected)
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut GameStateDocument) -> Result<T, GameDetailError>,
    ) -> Result<T, GameDetailError> {
        let mut guard = self.lock()?;
        let mut candidate = guard.clone();
        let output = mutation(&mut candidate)?;
        validate_state_document(&candidate)?;
        if !self.path.as_os_str().is_empty() {
            save_state_atomically(&self.path, &candidate)?;
        }
        *guard = candidate;
        Ok(output)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, GameStateDocument>, GameDetailError> {
        self.document
            .lock()
            .map_err(|_| GameDetailError::Unavailable("game state lock is poisoned".into()))
    }
}

#[derive(Debug)]
pub struct GameDetailService {
    records: RwLock<BTreeMap<String, GameDetailRecord>>,
    state: Arc<GameStateStore>,
    /// The app's own media cache. Without it the projection can still resolve
    /// bundled artwork, but an imported file has no addressable form — which is
    /// exactly the divergence `media_source_url` exists to prevent, so the root
    /// hands the real directory over at startup.
    media_cache_dir: Option<PathBuf>,
}

impl GameDetailService {
    pub fn new(state: Arc<GameStateStore>) -> Self {
        Self {
            records: RwLock::new(BTreeMap::new()),
            state,
            media_cache_dir: None,
        }
    }

    /// Point the projection at the directory imported artwork is cached in, so
    /// it resolves media exactly the way the Library does.
    #[must_use]
    pub fn with_media_cache_dir(mut self, media_cache_dir: Option<PathBuf>) -> Self {
        self.media_cache_dir = media_cache_dir;
        self
    }

    pub fn media_cache_dir(&self) -> Option<&Path> {
        self.media_cache_dir.as_deref()
    }

    pub fn from_catalog(
        catalog: &Catalog,
        state: Arc<GameStateStore>,
        media_cache_dir: Option<PathBuf>,
    ) -> Result<Self, GameDetailError> {
        let service = Self::new(state).with_media_cache_dir(media_cache_dir);
        service.replace_catalog(catalog)?;
        Ok(service)
    }

    pub fn replace_catalog(&self, catalog: &Catalog) -> Result<(), GameDetailError> {
        let mut projected = BTreeMap::new();
        for game in &catalog.games {
            let record = project_catalog_game(game, catalog, self.media_cache_dir.as_deref());
            record.validate()?;
            projected.insert(game.id.clone(), record);
        }
        *self.records.write().map_err(|_| {
            GameDetailError::Unavailable("detail catalog lock is poisoned".into())
        })? = projected;
        Ok(())
    }

    pub fn upsert_record(&self, record: GameDetailRecord) -> Result<(), GameDetailError> {
        record.validate()?;
        self.records
            .write()
            .map_err(|_| GameDetailError::Unavailable("detail catalog lock is poisoned".into()))?
            .insert(record.summary.id.clone(), record);
        Ok(())
    }

    pub fn contains(&self, game_id: &str) -> Result<bool, GameDetailError> {
        Ok(self
            .records
            .read()
            .map_err(|_| GameDetailError::Unavailable("detail catalog lock is poisoned".into()))?
            .contains_key(game_id))
    }

    pub fn state(&self) -> &Arc<GameStateStore> {
        &self.state
    }

    pub fn media_asset(
        &self,
        game_id: &str,
        media_id: &str,
    ) -> Result<Option<GameMediaAsset>, GameDetailError> {
        let record_asset = self
            .records
            .read()
            .map_err(|_| GameDetailError::Unavailable("detail catalog lock is poisoned".into()))?
            .get(game_id)
            .and_then(|record| record.media.iter().find(|media| media.id == media_id))
            .cloned();
        let persisted = self
            .state
            .media_for_game(game_id)?
            .into_iter()
            .find(|media| media.id == media_id);
        Ok(persisted.or(record_asset))
    }

    /// The media list a game currently offers: catalog/provider entries merged
    /// with durable user state, with the active selection flagged. Media
    /// mutations return this instead of a whole detail projection.
    pub fn media_views(&self, game_id: &str) -> Result<Vec<GameMediaView>, GameDetailError> {
        let record_media = self
            .records
            .read()
            .map_err(|_| GameDetailError::Unavailable("detail catalog lock is poisoned".into()))?
            .get(game_id)
            .map(|record| record.media.clone())
            .unwrap_or_default();
        self.merged_media(game_id, record_media)
    }

    fn merged_media(
        &self,
        game_id: &str,
        record_media: Vec<GameMediaAsset>,
    ) -> Result<Vec<GameMediaView>, GameDetailError> {
        let selected = self.state.selected_media(game_id)?;
        let mut media = record_media
            .into_iter()
            .map(|asset| (asset.id.clone(), asset))
            .collect::<BTreeMap<_, _>>();
        for persisted in self.state.media_for_game(game_id)? {
            media.insert(persisted.id.clone(), persisted);
        }
        Ok(media
            .into_values()
            .map(|asset| {
                let is_selected = selected.get(&asset.kind) == Some(&asset.id);
                asset.view(is_selected)
            })
            .collect())
    }

    fn detail(&self, game_id: &str) -> Result<Option<GameDetailView>, GameDetailError> {
        let Some(mut record) = self
            .records
            .read()
            .map_err(|_| GameDetailError::Unavailable("detail catalog lock is poisoned".into()))?
            .get(game_id)
            .cloned()
        else {
            return Ok(None);
        };
        record.summary.wishlisted = self.state.wishlisted(game_id)?;
        let media = self.merged_media(game_id, record.media)?;
        Ok(Some(GameDetailView {
            summary: record.summary,
            about: record.about,
            developer: record.developer,
            publisher: record.publisher,
            release_date: record.release_date,
            features: record.features,
            achievements: record.achievements,
            media,
            related_games: record.related_games,
            primary_action: record.primary_action,
        }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WishlistMutationView {
    pub game_id: String,
    pub wishlisted: bool,
}

/// IPC-shaped, path-free service entry point. The root can wrap this with
/// `#[tauri::command]` while retaining its private `AppState`.
pub fn get_game_detail(
    service: &GameDetailService,
    game_id: String,
) -> Result<Option<GameDetailView>, GameDetailError> {
    validate_opaque_id("game id", &game_id)?;
    service.detail(&game_id)
}

/// Wishlist is deliberately stored outside `catalog.json` and does not alter
/// ownership or Library membership.
pub fn set_game_wishlist(
    service: &GameDetailService,
    game_id: String,
    wishlisted: bool,
) -> Result<WishlistMutationView, GameDetailError> {
    validate_opaque_id("game id", &game_id)?;
    if !service.contains(&game_id)? {
        return Err(GameDetailError::NotFound);
    }
    service.state.set_wishlist(&game_id, wishlisted)?;
    Ok(WishlistMutationView {
        game_id,
        wishlisted,
    })
}

fn project_catalog_game(
    game: &Game,
    catalog: &Catalog,
    cache_dir: Option<&Path>,
) -> GameDetailRecord {
    let source = match &game.launch_target {
        LaunchTarget::Runner { runner_id, .. } if runner_id == WINE_STAGING_RUNNER_ID => {
            GameSourceView::Wine
        }
        _ if game.id.starts_with("showcase-") => GameSourceView::Showcase,
        _ if game.source == CatalogGameSource::Steam => GameSourceView::Steam,
        _ => GameSourceView::from_catalog_source(game.source).unwrap_or(GameSourceView::Local),
    };
    let launchable = match &game.launch_target {
        LaunchTarget::Steam { .. } => game.installation_path.is_some(),
        LaunchTarget::Direct => !game.id.starts_with("showcase-"),
        LaunchTarget::Runner {
            runner_id,
            profile_id,
            game_ref,
        } if runner_id == WINE_STAGING_RUNNER_ID => {
            cfg!(target_os = "macos")
                && catalog
                    .wine_profile(profile_id)
                    .is_some_and(|profile| profile.enabled)
                && catalog.wine_inventory_entry(profile_id, game_ref).is_some()
        }
        LaunchTarget::Runner { .. } => false,
        // Only a store whose client is on this machine gets a live Play
        // button; the rest stay records of what the account owns.
        LaunchTarget::Provider { provider, .. } => crate::launcher::provider_launchable(provider),
    };
    let primary_action = if launchable {
        PrimaryAction::Play
    } else {
        match (&game.launch_target, source) {
            (LaunchTarget::Steam { .. }, _) => PrimaryAction::InstallSteam,
            (LaunchTarget::Runner { runner_id, .. }, GameSourceView::Wine)
                if runner_id == WINE_STAGING_RUNNER_ID =>
            {
                PrimaryAction::ConfigureWine
            }
            _ => PrimaryAction::Unavailable,
        }
    };
    let description = game
        .description
        .clone()
        .unwrap_or_else(|| "Ready for your next session.".into());
    let genres = game
        .extra
        .get(STEAM_STORE_GENRE_KEY)
        .or_else(|| game.extra.get(SOURCE_GENRE_KEY))
        .and_then(serde_json::Value::as_str)
        .filter(|genre| !genre.trim().is_empty())
        .map(|genre| vec![genre.to_owned()])
        .unwrap_or_default();
    let supported_platforms = game
        .extra
        .get(STEAM_STORE_PLATFORMS_KEY)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter_map(|platform| match platform {
            "windows" => Some(PlatformView::Windows),
            "macos" => Some(PlatformView::Macos),
            "linux" => Some(PlatformView::Linux),
            _ => None,
        })
        .collect::<Vec<_>>();
    let media = catalog_media(game, cache_dir);
    let hero_url = media
        .iter()
        .find(|media| media.kind == GameMediaKind::Wallpaper)
        .and_then(|media| media.source_url.clone())
        .unwrap_or_default();
    let cover_url = media
        .iter()
        .find(|media| media.kind == GameMediaKind::Cover)
        .and_then(|media| media.source_url.clone())
        .unwrap_or_else(|| hero_url.clone());
    let landscape_url = steam_asset_url(game, "library_hero.jpg")
        .or_else(|| source_asset_url(game, SOURCE_LANDSCAPE_URL_KEY))
        .or_else(|| media_source_url(game.artwork_path.as_deref(), cache_dir))
        .unwrap_or_else(|| hero_url.clone());

    GameDetailRecord {
        summary: GameSummaryView {
            id: game.id.clone(),
            title: game.title.clone(),
            source,
            short_description: description.clone(),
            cover_url,
            hero_url,
            landscape_url,
            genres,
            tags: Vec::new(),
            supported_platforms,
            owned: !matches!(source, GameSourceView::Store | GameSourceView::Showcase),
            launchable,
            wishlisted: false,
            play_time_seconds: game.play_time_seconds,
            last_played_at: game.last_played_at.clone(),
            recommendation_reasons: Vec::new(),
            offers: Vec::new(),
        },
        about: description,
        developer: game
            .extra
            .get("developer")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        publisher: game
            .extra
            .get("publisher")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        release_date: game
            .extra
            .get("release_date")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        features: Vec::new(),
        achievements: None,
        media,
        related_games: Vec::new(),
        primary_action,
    }
}

fn catalog_media(game: &Game, cache_dir: Option<&Path>) -> Vec<GameMediaAsset> {
    let mut media = Vec::new();
    let wallpaper = steam_asset_url(game, "capsule_616x353.jpg")
        .or_else(|| source_asset_url(game, SOURCE_HERO_URL_KEY))
        .or_else(|| media_source_url(game.artwork_path.as_deref(), cache_dir));
    if let Some(url) = wallpaper {
        media.push(media_asset(
            &game.id,
            GameMediaKind::Wallpaper,
            "Wallpaper",
            url,
        ));
    }
    let cover = steam_asset_url(game, "library_600x900.jpg")
        .or_else(|| source_asset_url(game, SOURCE_COVER_URL_KEY))
        .or_else(|| media_source_url(game.cover_path.as_deref(), cache_dir));
    if let Some(url) = cover {
        media.push(media_asset(&game.id, GameMediaKind::Cover, "Cover", url));
    }
    if let Some(url) = media_source_url(game.logo_path.as_deref(), cache_dir) {
        media.push(media_asset(&game.id, GameMediaKind::Icon, "Icon", url));
    }
    if let Some(url) = media_source_url(game.hero_video_path.as_deref(), cache_dir) {
        media.push(media_asset(&game.id, GameMediaKind::Video, "Video", url));
    }
    media
}

fn media_asset(game_id: &str, kind: GameMediaKind, title: &str, url: String) -> GameMediaAsset {
    let mut digest = Sha256::new();
    digest.update(b"orivo-detail-media-v1\0");
    digest.update(game_id.as_bytes());
    digest.update([kind as u8]);
    digest.update(url.as_bytes());
    let origin = if url.starts_with("/media/") {
        GameMediaOrigin::Bundled
    } else {
        GameMediaOrigin::Provider
    };
    GameMediaAsset {
        id: format!("media:{:x}", digest.finalize()),
        kind,
        title: title.into(),
        source_url: Some(url),
        poster_url: None,
        origin,
        local_file: None,
        mime_type: None,
        byte_size: 0,
        extra: BTreeMap::new(),
    }
}

fn steam_asset_url(game: &Game, asset: &str) -> Option<String> {
    match &game.launch_target {
        LaunchTarget::Steam { app_id } if *app_id > 0 => Some(format!(
            "https://cdn.cloudflare.steamstatic.com/steam/apps/{app_id}/{asset}"
        )),
        _ => None,
    }
}

/// Artwork a connected store published for one of its own games. The value was
/// already held to that provider's host allowlist when the sync wrote it, so
/// this only has to refuse a record that predates the check or was edited on
/// disk: nothing but an absolute HTTPS URL is ever handed to the WebView.
fn source_asset_url(game: &Game, key: &str) -> Option<String> {
    let url = game.extra.get(key)?.as_str()?;
    (url.starts_with("https://") && !url.chars().any(char::is_control)).then(|| url.to_owned())
}

/// The one rule that turns a stored artwork path into something a WebView may
/// load. The Library projection in the crate root and this detail projection
/// both call it, so the two views can never disagree about what an imported
/// asset resolves to — a second copy of this rule is what left cached artwork
/// visible in the Library and blank on the detail page.
///
/// Exactly two shapes are addressable and nothing else:
///
/// * a bundled `/media/…` public asset, returned verbatim, and
/// * a regular file inside the app's own media cache, returned as the opaque
///   `cache:<file name>` token the WebView maps through the scoped asset
///   protocol.
///
/// A `cache:` token names one bare file and can never describe a path: the name
/// is held to the same opaque rule the `game-media:` scheme enforces. Traversal,
/// Windows separators and control characters are refused rather than
/// reinterpreted, and a path outside both roots resolves to nothing.
pub(crate) fn media_source_url(path: Option<&Path>, cache_dir: Option<&Path>) -> Option<String> {
    let path = path?;
    let value = path.to_str()?;
    if value.chars().any(char::is_control) {
        return None;
    }
    if value.starts_with("/media/") {
        // A bundled asset is handed to the WebView as written, so the literal
        // string must not be able to name anything but a file under `/media/`.
        return (!value.contains("..") && !value.contains('\\')).then(|| value.to_owned());
    }
    let cache_dir = cache_dir?;
    if !path.starts_with(cache_dir)
        || path.components().any(|part| part == Component::ParentDir)
        || !path.is_file()
    {
        return None;
    }
    let file_name = path.file_name()?.to_str()?;
    valid_opaque_file_name(file_name).then(|| format!("cache:{file_name}"))
}

fn validate_state_document(document: &GameStateDocument) -> Result<(), GameDetailError> {
    if document.schema_version != GAME_STATE_SCHEMA_VERSION {
        return Err(GameDetailError::Invalid(
            "unsupported game-state schema".into(),
        ));
    }
    for (game_id, state) in &document.games {
        validate_opaque_id("game id", game_id)?;
        for (media_id, media) in &state.media {
            if media_id != &media.id {
                return Err(GameDetailError::Invalid(
                    "media map key does not match its id".into(),
                ));
            }
            media.validate()?;
        }
        for (kind, media_id) in &state.selected_media {
            if state
                .media
                .get(media_id)
                .is_none_or(|media| media.kind != *kind)
            {
                return Err(GameDetailError::Invalid(
                    "selected media reference is missing or has the wrong kind".into(),
                ));
            }
        }
    }
    Ok(())
}

fn save_state_atomically(path: &Path, document: &GameStateDocument) -> Result<(), GameDetailError> {
    let temporary = path.with_extension("json.tmp");
    write_state_file(&temporary, &serialize_state(document)?)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn serialize_state(document: &GameStateDocument) -> Result<Vec<u8>, GameDetailError> {
    let mut bytes = serde_json::to_vec_pretty(document)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn write_state_file(path: &Path, bytes: &[u8]) -> Result<(), GameDetailError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

enum LoadedStateFile {
    Missing,
    Valid {
        document: GameStateDocument,
        bytes: Vec<u8>,
    },
    /// The stored content could not be used and was moved aside (or, if even
    /// that failed, left in place to be replaced by the next write).
    Quarantined,
}

/// Read `game-state.json` without ever returning an error: the caller can
/// always continue from a default document. `StoreCache::read` and
/// `PreferencesService::load` degrade the same way.
fn read_state_file(path: &Path) -> LoadedStateFile {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return LoadedStateFile::Missing;
        }
        Err(error) => {
            quarantine_state_file(path, &format!("could not be read ({error})"));
            return LoadedStateFile::Quarantined;
        }
    };
    let document = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|json| serde_json::from_str::<GameStateDocument>(json).ok())
        .filter(|document| validate_state_document(document).is_ok());
    match document {
        Some(document) => LoadedStateFile::Valid { document, bytes },
        None => {
            quarantine_state_file(path, "is corrupt or uses an unsupported schema");
            LoadedStateFile::Quarantined
        }
    }
}

/// Move an unusable state document aside as `<stem>.corrupt-<n>.json` so the
/// user keeps a copy of whatever was there. Only the file name is logged: the
/// containing directory is host-private.
fn quarantine_state_file(path: &Path, reason: &str) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("game-state.json");
    match rename_to_free_quarantine_slot(path) {
        Ok(moved) => eprintln!(
            "orivo: {name} {reason}; it was kept as {moved} and game state was reset to defaults."
        ),
        Err(error) => eprintln!(
            "orivo: {name} {reason} and could not be kept aside ({error}); game state was reset to defaults."
        ),
    }
}

fn rename_to_free_quarantine_slot(path: &Path) -> Result<String, std::io::Error> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("game-state");
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("json");
    for slot in 1..=QUARANTINE_SLOT_LIMIT {
        let name = format!("{stem}.corrupt-{slot}.{extension}");
        let candidate = path.with_file_name(&name);
        if candidate.exists() {
            continue;
        }
        fs::rename(path, &candidate)?;
        return Ok(name);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "every quarantine slot is taken",
    ))
}

/// Apply a catalog migration's `old id -> new id` map to durable game state.
///
/// The rewrite is a single simultaneous permutation, never a sequence of
/// renames, so a new id that happens to equal another record's old id cannot
/// be rewritten twice. Ids absent from the map (Steam, runner, and any record
/// the migration did not touch) are left exactly as they are.
///
/// Precedence when an entry already exists under both the old and the new id:
/// the migrated entry — the one attached to the catalog record being renamed —
/// wins. The pre-existing entry is merged underneath it, so nothing is
/// dropped: wishlist flags are OR-ed, media registrations are unioned, and its
/// selections survive for every kind the winner does not itself select.
fn rekey_game_states(
    games: &BTreeMap<String, PersistedGameState>,
    rewritten_ids: &BTreeMap<String, String>,
) -> BTreeMap<String, PersistedGameState> {
    let mut rekeyed: BTreeMap<String, PersistedGameState> = BTreeMap::new();
    // Two passes so precedence never depends on map iteration order: renamed
    // entries claim their target first and therefore win every conflict.
    for (old_id, state) in games {
        if let Some(new_id) = rewritten_ids.get(old_id) {
            merge_state_entry(&mut rekeyed, new_id, state.clone());
        }
    }
    for (game_id, state) in games {
        if rewritten_ids.contains_key(game_id) {
            continue;
        }
        merge_state_entry(&mut rekeyed, game_id, state.clone());
    }
    rekeyed
}

fn merge_state_entry(
    target: &mut BTreeMap<String, PersistedGameState>,
    game_id: &str,
    incoming: PersistedGameState,
) {
    match target.get_mut(game_id) {
        Some(winner) => merge_persisted_state(winner, incoming),
        None => {
            target.insert(game_id.to_owned(), incoming);
        }
    }
}

fn merge_persisted_state(winner: &mut PersistedGameState, loser: PersistedGameState) {
    // Wishlisting is an explicit intent, so a merge may add it but never
    // revoke it.
    winner.wishlisted |= loser.wishlisted;
    // Imported media is user data and is also what `protected_local_files`
    // keeps out of the media quota's reach, so every registration is kept.
    for (media_id, media) in loser.media {
        winner.media.entry(media_id).or_insert(media);
    }
    for (kind, media_id) in loser.selected_media {
        if winner.selected_media.contains_key(&kind) {
            continue;
        }
        if winner
            .media
            .get(&media_id)
            .is_some_and(|media| media.kind == kind)
        {
            winner.selected_media.insert(kind, media_id);
        }
    }
    for (key, value) in loser.extra {
        winner.extra.entry(key).or_insert(value);
    }
}

/// A prepared `game-state.json` rewrite that is fully written to a sibling
/// temporary file but not yet published.
///
/// `game-state.json` is keyed by game id, so a catalog migration that rewrites
/// ids has to move it in the same step. Staging first keeps the two files from
/// diverging: everything that can fail (reading, re-keying, validating,
/// writing, fsync) happens before either file is published, publishing is a
/// single rename, and `restore` puts the previous bytes back if a later step
/// of the same migration fails.
#[derive(Debug)]
pub struct StagedGameState {
    path: PathBuf,
    temporary: PathBuf,
    /// The bytes to put back if the migration fails after publishing. `None`
    /// means there was no usable document to begin with.
    original: Option<Vec<u8>>,
    pending: bool,
}

impl StagedGameState {
    /// A missing document is normal — the store creates it on first use — and
    /// an unusable one is quarantined and treated as empty, so neither can turn
    /// a catalog upgrade into a startup failure. Nothing is written when the
    /// map leaves the document unchanged, which keeps a re-run of the same
    /// migration a true no-op.
    pub fn stage(
        path: &Path,
        rewritten_ids: &BTreeMap<String, String>,
    ) -> Result<Self, GameDetailError> {
        let (document, original) = match read_state_file(path) {
            LoadedStateFile::Valid { document, bytes } => (document, Some(bytes)),
            LoadedStateFile::Missing | LoadedStateFile::Quarantined => {
                (GameStateDocument::default(), None)
            }
        };
        let mut staged = document.clone();
        staged.games = rekey_game_states(&document.games, rewritten_ids);
        validate_state_document(&staged)?;
        let pending = staged.games != document.games;
        let temporary = path.with_extension("json.migrating");
        if pending {
            write_state_file(&temporary, &serialize_state(&staged)?)?;
        }
        Ok(Self {
            path: path.to_path_buf(),
            temporary,
            original,
            pending,
        })
    }

    pub fn commit(&self) -> Result<(), GameDetailError> {
        if !self.pending {
            return Ok(());
        }
        fs::rename(&self.temporary, &self.path)?;
        Ok(())
    }

    /// Put the pre-migration document back after a later step failed, so the
    /// catalog and the game state can never end up one migrated and one not.
    pub fn restore(&self) -> Result<(), GameDetailError> {
        if !self.pending {
            return Ok(());
        }
        match self.original.as_deref() {
            Some(bytes) => {
                write_state_file(&self.temporary, bytes)?;
                fs::rename(&self.temporary, &self.path)?;
            }
            None => match fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(GameDetailError::Io(error)),
            },
        }
        Ok(())
    }
}

impl Drop for StagedGameState {
    fn drop(&mut self) {
        // A staged file that was never published must not be left behind.
        let _ = fs::remove_file(&self.temporary);
    }
}

pub fn validate_opaque_id(field: &str, value: &str) -> Result<(), GameDetailError> {
    let valid = !value.is_empty()
        && value.len() <= MAX_OPAQUE_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'));
    if valid {
        Ok(())
    } else {
        Err(GameDetailError::Invalid(format!(
            "{field} must be an opaque identifier"
        )))
    }
}

pub fn valid_opaque_file_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && !value.contains("..")
}

fn validate_display_text(
    field: &str,
    value: &str,
    max_length: usize,
) -> Result<(), GameDetailError> {
    if value.trim().is_empty() || value.len() > max_length || value.chars().any(char::is_control) {
        Err(GameDetailError::Invalid(format!(
            "{field} must be bounded display text"
        )))
    } else {
        Ok(())
    }
}

#[derive(Debug)]
pub enum GameDetailError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(String),
    NotFound,
    Unavailable(String),
}

impl std::fmt::Display for GameDetailError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(_) => formatter.write_str("game state could not be persisted"),
            Self::Json(_) => formatter.write_str("game state has an invalid format"),
            Self::Invalid(message) => write!(formatter, "invalid game detail request: {message}"),
            Self::NotFound => formatter.write_str("game detail was not found"),
            Self::Unavailable(message) => {
                write!(formatter, "game detail is unavailable: {message}")
            }
        }
    }
}

impl std::error::Error for GameDetailError {}

impl From<std::io::Error> for GameDetailError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for GameDetailError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_state_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "orivo-game-state-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn record(id: &str, owned: bool, action: PrimaryAction) -> GameDetailRecord {
        GameDetailRecord {
            summary: GameSummaryView {
                id: id.into(),
                title: "Example".into(),
                source: if owned {
                    GameSourceView::Local
                } else {
                    GameSourceView::Store
                },
                short_description: "Short description".into(),
                cover_url: "/media/example-cover.jpg".into(),
                hero_url: "/media/example-hero.jpg".into(),
                landscape_url: "/media/example-landscape.jpg".into(),
                genres: vec!["Adventure".into()],
                tags: Vec::new(),
                supported_platforms: vec![PlatformView::Macos],
                owned,
                launchable: action == PrimaryAction::Play,
                wishlisted: false,
                play_time_seconds: 0,
                last_played_at: None,
                recommendation_reasons: Vec::new(),
                offers: Vec::new(),
            },
            about: "About".into(),
            developer: None,
            publisher: None,
            release_date: None,
            features: Vec::new(),
            achievements: None,
            media: vec![GameMediaAsset {
                id: "media:wallpaper".into(),
                kind: GameMediaKind::Wallpaper,
                title: "Wallpaper".into(),
                source_url: Some("/media/example-hero.jpg".into()),
                poster_url: None,
                origin: GameMediaOrigin::Bundled,
                local_file: None,
                mime_type: Some("image/jpeg".into()),
                byte_size: 10,
                extra: BTreeMap::new(),
            }],
            related_games: Vec::new(),
            primary_action: action,
        }
    }

    #[test]
    fn projects_owned_and_store_records_without_title_identity_merges() {
        let state = Arc::new(GameStateStore::in_memory_for_tests());
        let service = GameDetailService::new(state);
        service
            .upsert_record(record("local:aaa", true, PrimaryAction::Play))
            .unwrap();
        service
            .upsert_record(record("steam:480", false, PrimaryAction::ViewOffer))
            .unwrap();

        let owned = get_game_detail(&service, "local:aaa".into())
            .unwrap()
            .unwrap();
        let store = get_game_detail(&service, "steam:480".into())
            .unwrap()
            .unwrap();

        assert!(owned.summary.owned);
        assert_eq!(owned.primary_action, PrimaryAction::Play);
        assert!(!store.summary.owned);
        assert_eq!(store.primary_action, PrimaryAction::ViewOffer);
    }

    #[test]
    fn wishlist_persists_separately_and_preserves_unknown_state() {
        let path = temporary_state_path("wishlist");
        fs::write(
            &path,
            r#"{"schema_version":1,"future":{"keep":true},"games":{}}"#,
        )
        .unwrap();
        let state = Arc::new(GameStateStore::load(path.clone()).unwrap());
        let service = GameDetailService::new(state);
        service
            .upsert_record(record("local:aaa", true, PrimaryAction::Play))
            .unwrap();

        set_game_wishlist(&service, "local:aaa".into(), true).unwrap();
        let reloaded = GameStateStore::load(path.clone()).unwrap();

        assert!(reloaded.wishlisted("local:aaa").unwrap());
        assert_eq!(
            reloaded.snapshot().unwrap().extra.get("future"),
            Some(&serde_json::json!({ "keep": true }))
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_selection_keeps_the_previous_media() {
        let state = GameStateStore::in_memory_for_tests();
        let first = record("local:aaa", true, PrimaryAction::Play).media[0].clone();
        state.register_media("local:aaa", first).unwrap();
        state.select_media("local:aaa", "media:wallpaper").unwrap();

        assert!(matches!(
            state.select_media("local:aaa", "media:missing"),
            Err(GameDetailError::NotFound)
        ));
        assert_eq!(
            state
                .selected_media("local:aaa")
                .unwrap()
                .get(&GameMediaKind::Wallpaper)
                .map(String::as_str),
            Some("media:wallpaper")
        );
    }

    #[test]
    fn registering_and_selecting_media_is_one_atomic_mutation() {
        let state = Arc::new(GameStateStore::in_memory_for_tests());
        let service = GameDetailService::new(Arc::clone(&state));
        service
            .upsert_record(record("local:aaa", true, PrimaryAction::Play))
            .unwrap();
        let mut applied = record("local:aaa", true, PrimaryAction::Play).media[0].clone();
        applied.id = "media:applied".into();
        applied.local_file = Some("abc123.png".into());
        state
            .register_and_select_media("local:aaa", applied)
            .unwrap();

        let views = service.media_views("local:aaa").unwrap();
        assert!(
            views
                .iter()
                .any(|view| view.id == "media:applied" && view.selected)
        );
        assert!(
            state
                .protected_local_files()
                .unwrap()
                .contains("abc123.png")
        );

        // A rejected asset changes neither the media table nor the selection.
        let mut hostile = record("local:aaa", true, PrimaryAction::Play).media[0].clone();
        hostile.id = "media:hostile".into();
        hostile.local_file = Some("../../escape.png".into());
        assert!(
            state
                .register_and_select_media("local:aaa", hostile)
                .is_err()
        );
        assert_eq!(
            state
                .selected_media("local:aaa")
                .unwrap()
                .get(&GameMediaKind::Wallpaper)
                .map(String::as_str),
            Some("media:applied")
        );
        assert!(
            !state
                .media_for_game("local:aaa")
                .unwrap()
                .iter()
                .any(|media| media.id == "media:hostile")
        );
    }

    #[test]
    fn dto_serialization_never_contains_host_paths() {
        let state = Arc::new(GameStateStore::in_memory_for_tests());
        let service = GameDetailService::new(state);
        service
            .upsert_record(record("local:aaa", true, PrimaryAction::Play))
            .unwrap();
        let dto = get_game_detail(&service, "local:aaa".into())
            .unwrap()
            .unwrap();
        let json = serde_json::to_string(&dto).unwrap();

        assert!(!json.contains("/Users/"));
        assert!(!json.contains("executable"));
    }

    fn quarantine_slot(path: &Path, slot: u32) -> PathBuf {
        path.with_file_name(format!(
            "{}.corrupt-{slot}.json",
            path.file_stem().unwrap().to_str().unwrap()
        ))
    }

    #[test]
    fn corrupt_game_state_is_quarantined_instead_of_failing_startup() {
        let path = temporary_state_path("corrupt");
        fs::write(&path, b"{ not json at all").unwrap();

        let store = GameStateStore::load(path.clone()).unwrap();

        assert!(store.snapshot().unwrap().games.is_empty());
        let quarantined = quarantine_slot(&path, 1);
        assert_eq!(
            fs::read_to_string(&quarantined).unwrap(),
            "{ not json at all"
        );
        assert!(!path.exists());

        // The store is usable afterwards instead of blocking startup.
        store.set_wishlist("local:aaa", true).unwrap();
        assert!(
            GameStateStore::load(path.clone())
                .unwrap()
                .wishlisted("local:aaa")
                .unwrap()
        );
        fs::remove_file(path).unwrap();
        fs::remove_file(quarantined).unwrap();
    }

    #[test]
    fn unknown_schema_and_invalid_game_state_are_quarantined_into_separate_slots() {
        let path = temporary_state_path("unsupported");
        fs::write(&path, r#"{"schema_version":99,"games":{}}"#).unwrap();
        assert!(
            GameStateStore::load(path.clone())
                .unwrap()
                .snapshot()
                .unwrap()
                .games
                .is_empty()
        );

        // A selection that points at nothing is corrupt too, and must not
        // overwrite the first quarantined copy.
        fs::write(
            &path,
            r#"{"schema_version":1,"games":{"local:aaa":{"selectedMedia":{"cover":"media:missing"}}}}"#,
        )
        .unwrap();
        assert!(
            GameStateStore::load(path.clone())
                .unwrap()
                .snapshot()
                .unwrap()
                .games
                .is_empty()
        );

        let first = quarantine_slot(&path, 1);
        let second = quarantine_slot(&path, 2);
        assert!(fs::read_to_string(&first).unwrap().contains("99"));
        assert!(
            fs::read_to_string(&second)
                .unwrap()
                .contains("media:missing")
        );
        assert!(!path.exists());
        fs::remove_file(first).unwrap();
        fs::remove_file(second).unwrap();
    }

    #[test]
    fn rejects_path_and_url_shaped_webview_ids() {
        for id in [
            "../../secret",
            "https://example.com/a",
            "/Users/private/game",
        ] {
            assert!(validate_opaque_id("game id", id).is_err());
        }
    }
}
