use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

/// Schema v6 moves Wine graphics selection to the private game inventory.
/// Existing direct, Steam, generic-runner and Wine records remain
/// data-preserving; an existing Wine game keeps its old profile backend and
/// shared prefix rather than being silently moved to a new compatibility
/// layer.
pub const CURRENT_SCHEMA_VERSION: u32 = 6;
const SCHEMA_VERSION_V1: u32 = 1;
const SCHEMA_VERSION_V2: u32 = 2;
const SCHEMA_VERSION_V3: u32 = 3;
const SCHEMA_VERSION_V4: u32 = 4;
const SCHEMA_VERSION_V5: u32 = 5;

/// The stable identity for Orivo's first official Wine runner. It is an
/// opaque runner identifier, never a Wine executable path or command.
pub const WINE_STAGING_RUNNER_ID: &str = "com.orivo.wine-staging";

fn default_wine_profile_enabled() -> bool {
    true
}

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
    /// A game launched through an installed runner profile, such as an
    /// emulator. These are stable opaque identifiers, never a command line,
    /// executable path, or ROM path supplied by the WebView.
    Runner {
        runner_id: String,
        game_ref: String,
        profile_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Catalog {
    pub schema_version: u32,
    pub games: Vec<Game>,
    /// Host-private Wine profile configuration. These paths must never be
    /// projected into a WebView view model; they are only resolved by the
    /// native runner host after it has accepted a typed launch intent.
    #[serde(default)]
    pub wine_profiles: Vec<WineProfile>,
    /// Host-private mapping from opaque Wine game references to the selected
    /// executable. A library `Game` deliberately holds only `game_ref`.
    #[serde(default)]
    pub wine_inventory: Vec<WineGameInventoryEntry>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedCatalog {
    pub catalog: Catalog,
    pub migrated_from: Option<u32>,
}

/// A Wine prefix owned by Orivo. The host creates and validates it before the
/// profile is written; catalog validation is intentionally structural so a
/// temporarily unavailable external volume cannot make the whole library
/// unreadable on startup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WineProfile {
    /// Opaque, stable Orivo profile identifier.
    pub id: String,
    /// User-facing label. This is distinct from any filesystem component.
    pub display_name: String,
    /// Validated Wine-Staging executable selected through the native picker.
    pub wine_binary: PathBuf,
    /// A dedicated prefix created by Orivo. Prefixes may not be shared across
    /// profiles, which avoids ever mutating another application's prefix.
    pub prefix: PathBuf,
    /// Directories explicitly granted to this profile for Windows game scans
    /// and launches. No implicit disk-wide fallback is permitted.
    #[serde(default)]
    pub game_directories: Vec<PathBuf>,
    /// The v5 profile-wide graphics setting. It remains persisted so existing
    /// profiles can be migrated without changing their launch behaviour. New
    /// games use the closed per-game policy on `WineGameInventoryEntry`.
    /// Neither representation can carry raw Wine flags, environment
    /// variables, or arbitrary command arguments.
    #[serde(default)]
    pub graphics: WineGraphicsOptions,
    /// Last host probe of the selected Wine engine's DXMT presentation ABI.
    /// `None` represents a profile created before the probe existed or an
    /// engine that has not been revalidated yet. This is a capability hint,
    /// never a user-controlled graphics setting.
    #[serde(default)]
    pub dxmt_engine_supported: Option<bool>,
    /// Last host-applied high-density display policy for this private macOS
    /// Wine prefix. `None` means that an older profile has not been brought
    /// forward yet; the native host resolves it from the active display and
    /// writes only Wine's fixed `RetinaMode` registry value.
    #[serde(default)]
    pub macos_retina_mode_enabled: Option<bool>,
    /// Disabled profiles and their games remain persisted and visible, but
    /// cannot be launched until the user enables them again.
    #[serde(default = "default_wine_profile_enabled")]
    pub enabled: bool,
    /// Unix milliseconds of the last completed import, if one has completed.
    #[serde(default)]
    pub last_imported_at: Option<u64>,
}

/// Graphics settings which the Wine host can translate into fixed, tokenised
/// Wine arguments. New options require a schema and host implementation
/// change; no free-form setting is persisted here.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WineGraphicsOptions {
    /// The graphics translation path is a closed host-owned enum. It never
    /// contains an environment variable, a DLL path, a command flag, or any
    /// value supplied verbatim by a plugin/WebView.
    #[serde(default)]
    pub backend: WineGraphicsBackend,
    /// When present, the host may invoke Wine's fixed virtual-desktop mode.
    /// The desktop name and argument shape remain host-owned.
    #[serde(default)]
    pub virtual_desktop: Option<WineVirtualDesktop>,
}

/// Graphics translation implementations supported by the built-in Wine host.
/// `DxvkMacos` is enabled only after the host has validated and copied the
/// fixed, allowlisted runtime into an Orivo-owned prefix.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum WineGraphicsBackend {
    #[default]
    WineD3d,
    DxvkMacos,
    /// A host-managed Direct3D 10/11 Metal backend. It may be selected only
    /// after the native host has verified that the chosen Wine engine exports
    /// the required macOS driver API; no plugin/WebView value can enable it.
    Dxmt,
    /// Automatically resolve the best host-supported backend for a newly
    /// imported game. This value is valid only for game inventory policies,
    /// never as the legacy profile-wide setting.
    Auto,
}

/// Bounded virtual desktop dimensions. The host chooses the fixed Wine mode;
/// this structure cannot represent arbitrary flags or a shell fragment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WineVirtualDesktop {
    pub width: u16,
    pub height: u16,
}

/// The private executable inventory behind a Wine runner game. `game_ref`
/// is the sole value copied into `LaunchTarget::Runner`; `executable_path`
/// never crosses the WebView boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WineGameInventoryEntry {
    pub profile_id: String,
    pub game_ref: String,
    pub title: String,
    pub executable_path: PathBuf,
    /// A stable scanner fingerprint, e.g. a namespaced content hash. It is
    /// used by the host to reconcile incremental scans without exposing paths.
    pub fingerprint: String,
    #[serde(default)]
    pub imported_at: Option<u64>,
    /// Private, closed compatibility state for this exact game. No prefix
    /// pathname is persisted: the native host derives it from the opaque
    /// profile id and game reference at launch time.
    #[serde(default)]
    pub compatibility: WineGameCompatibility,
    /// When a user deliberately associates an existing Direct local `.exe`
    /// with Wine, retain only its catalog id. The original Direct record stays
    /// intact and is shown again if this Wine profile is removed; neither a
    /// path nor direct launch arguments are copied into the runner card.
    #[serde(default)]
    pub origin_direct_game_id: Option<String>,
}

/// The graphics policy and prefix layout are per game because different D3D
/// runtimes place DLLs and registry state in a Wine prefix. Sharing that
/// mutable state across games would make an Auto fallback contaminate another
/// game in the same profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WineGameCompatibility {
    /// The host resolves this closed selection into fixed environment values
    /// and tokenised Wine arguments. It never contains an arbitrary variable,
    /// DLL path, or user-supplied argument.
    #[serde(default = "default_wine_game_graphics_options")]
    pub graphics: WineGraphicsOptions,
    /// v5 games retain their shared profile prefix exactly as before. New
    /// imports use a derived Orivo-owned prefix per game.
    #[serde(default)]
    pub prefix_layout: WinePrefixLayout,
    /// Backends rejected by an explicit path-free retry action. The host
    /// computes candidates; this records only closed enum variants so a UI
    /// cannot force a command or runtime path.
    #[serde(default)]
    pub rejected_backends: Vec<WineGraphicsBackend>,
    /// The closed backend used by the most recent prepared launch. It is
    /// host-written before spawning so an explicit retry can advance only to
    /// a known safe fallback without the WebView choosing an implementation.
    #[serde(default)]
    pub last_backend: Option<WineGraphicsBackend>,
}

impl Default for WineGameCompatibility {
    fn default() -> Self {
        Self::automatic()
    }
}

impl WineGameCompatibility {
    pub fn automatic() -> Self {
        Self {
            graphics: default_wine_game_graphics_options(),
            prefix_layout: WinePrefixLayout::Isolated,
            rejected_backends: Vec::new(),
            last_backend: None,
        }
    }

    fn legacy_profile(graphics: WineGraphicsOptions) -> Self {
        let backend = graphics.backend;
        Self {
            graphics,
            prefix_layout: WinePrefixLayout::LegacySharedProfile,
            rejected_backends: Vec::new(),
            last_backend: Some(backend),
        }
    }

    pub fn validate(&self) -> Result<(), CatalogError> {
        self.graphics.validate()?;
        let mut rejected = BTreeSet::new();
        for backend in &self.rejected_backends {
            if matches!(backend, WineGraphicsBackend::Auto) || !rejected.insert(backend) {
                return Err(CatalogError::Invalid(
                    "Wine game compatibility has invalid rejected backends".into(),
                ));
            }
        }
        if matches!(self.last_backend, Some(WineGraphicsBackend::Auto)) {
            return Err(CatalogError::Invalid(
                "Wine game compatibility cannot record automatic as a backend".into(),
            ));
        }
        if self.graphics.backend == WineGraphicsBackend::Auto
            && self.prefix_layout != WinePrefixLayout::Isolated
        {
            return Err(CatalogError::Invalid(
                "Automatic Wine graphics require an isolated game prefix".into(),
            ));
        }
        if self.prefix_layout == WinePrefixLayout::LegacySharedProfile
            && !self.rejected_backends.is_empty()
        {
            return Err(CatalogError::Invalid(
                "Legacy Wine games cannot carry automatic fallback state".into(),
            ));
        }
        Ok(())
    }
}

fn default_wine_game_graphics_options() -> WineGraphicsOptions {
    WineGraphicsOptions {
        backend: WineGraphicsBackend::Auto,
        virtual_desktop: None,
    }
}

/// Prefix layout is deliberately a closed enum rather than a pathname. The
/// host owns the only path derivation for both variants.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WinePrefixLayout {
    /// New games receive a clean, derived Orivo prefix per game/backend.
    #[default]
    Isolated,
    /// Preserves v5 profile state until the user explicitly chooses to move a
    /// game. This prevents silently losing saves, registry state, or runtime
    /// DLLs from an existing profile.
    LegacySharedProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Game {
    pub id: String,
    pub title: String,
    /// Present only for direct local launches. Steam and runner-backed records
    /// deliberately do not pretend that a provider URI or opaque game
    /// reference is a file.
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
            wine_profiles: Vec::new(),
            wine_inventory: Vec::new(),
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
            SCHEMA_VERSION_V5 => {
                migrate_v5_to_v6(&mut catalog);
                Some(SCHEMA_VERSION_V5)
            }
            SCHEMA_VERSION_V4 => {
                migrate_v4_to_v5(&mut catalog);
                migrate_v5_to_v6(&mut catalog);
                Some(SCHEMA_VERSION_V4)
            }
            SCHEMA_VERSION_V3 => {
                migrate_v3_to_v4(&mut catalog);
                migrate_v4_to_v5(&mut catalog);
                migrate_v5_to_v6(&mut catalog);
                Some(SCHEMA_VERSION_V3)
            }
            SCHEMA_VERSION_V2 => {
                migrate_v2_to_v3(&mut catalog);
                migrate_v3_to_v4(&mut catalog);
                migrate_v4_to_v5(&mut catalog);
                migrate_v5_to_v6(&mut catalog);
                Some(SCHEMA_VERSION_V2)
            }
            SCHEMA_VERSION_V1 => {
                migrate_v1_to_v2(&mut catalog);
                migrate_v2_to_v3(&mut catalog);
                migrate_v3_to_v4(&mut catalog);
                migrate_v4_to_v5(&mut catalog);
                migrate_v5_to_v6(&mut catalog);
                Some(SCHEMA_VERSION_V1)
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
        if let Some((runner_id, profile_id, game_ref)) = runner_target_key(&game) {
            if runner_id == WINE_STAGING_RUNNER_ID {
                self.validate_wine_runner_reference(profile_id, game_ref)?;
            }
            if self.games.iter().any(|existing| {
                runner_target_key(existing)
                    .is_some_and(|existing_key| existing_key == (runner_id, profile_id, game_ref))
            }) {
                return Err(CatalogError::Invalid(
                    "duplicate runner game reference for profile".into(),
                ));
            }
        }
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

    /// Insert a runner record or refresh an existing record with the same
    /// `(runner_id, profile_id, game_ref)` identity. This is deliberately not
    /// keyed by a title or a path: titles can change and paths stay private in
    /// the Wine inventory.
    ///
    /// Returns `true` when a card is first imported and `false` when a scan
    /// refreshes an existing card. Playback state and any opaque metadata that
    /// the scanner did not replace are retained across refreshes.
    pub fn upsert_runner(&mut self, mut game: Game) -> Result<bool, CatalogError> {
        game.validate()?;
        let (runner_id, profile_id, game_ref) = runner_target_key(&game).ok_or_else(|| {
            CatalogError::Invalid("upsert_runner requires a runner launch target".into())
        })?;

        if runner_id == WINE_STAGING_RUNNER_ID {
            self.validate_wine_runner_reference(profile_id, game_ref)?;
        }

        if let Some(index) = self.games.iter().position(|existing| {
            runner_target_key(existing)
                .is_some_and(|existing_key| existing_key == (runner_id, profile_id, game_ref))
        }) {
            let existing = &self.games[index];
            // The tuple is the stable provider identity. Keep the existing
            // Orivo card id even if a newer scanner implementation derives a
            // different display id for the same external game.
            game.id = existing.id.clone();
            preserve_runner_game_state(&mut game, existing);
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

    /// Return a Wine profile by its opaque host identifier. Callers must not
    /// project this value or any of its paths into a WebView response.
    pub fn wine_profile(&self, profile_id: &str) -> Option<&WineProfile> {
        self.wine_profiles
            .iter()
            .find(|profile| profile.id == profile_id)
    }

    /// Return the private inventory entry for a typed Wine runner reference.
    pub fn wine_inventory_entry(
        &self,
        profile_id: &str,
        game_ref: &str,
    ) -> Option<&WineGameInventoryEntry> {
        self.wine_inventory
            .iter()
            .find(|entry| entry.profile_id == profile_id && entry.game_ref == game_ref)
    }

    /// Insert or replace a Wine profile after structural validation. Updating
    /// a profile cannot silently make existing inventory entries escape its
    /// granted directories: the full candidate catalog is validated first.
    pub fn upsert_wine_profile(&mut self, mut profile: WineProfile) -> Result<bool, CatalogError> {
        profile.validate()?;
        if let Some(index) = self
            .wine_profiles
            .iter()
            .position(|existing| existing.id == profile.id)
        {
            if profile.last_imported_at.is_none() {
                profile.last_imported_at = self.wine_profiles[index].last_imported_at;
            }
            let mut candidate = self.clone();
            candidate.wine_profiles[index] = profile;
            candidate.validate()?;
            *self = candidate;
            return Ok(false);
        }

        let mut candidate = self.clone();
        candidate.wine_profiles.push(profile);
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    /// Insert or refresh a private Wine inventory entry. The entry is scoped
    /// to an existing profile and its executable must remain inside one of the
    /// profile's recorded game directories.
    pub fn upsert_wine_inventory(
        &mut self,
        mut entry: WineGameInventoryEntry,
    ) -> Result<bool, CatalogError> {
        entry.validate()?;
        let profile = self.wine_profile(&entry.profile_id).ok_or_else(|| {
            CatalogError::Invalid("Wine inventory entry references an unknown profile".into())
        })?;
        validate_inventory_scope(&entry, profile)?;

        if let Some(index) = self.wine_inventory.iter().position(|existing| {
            existing.profile_id == entry.profile_id && existing.game_ref == entry.game_ref
        }) {
            if entry.imported_at.is_none() {
                entry.imported_at = self.wine_inventory[index].imported_at;
            }
            if entry.origin_direct_game_id.is_none() {
                entry.origin_direct_game_id =
                    self.wine_inventory[index].origin_direct_game_id.clone();
            }
            // A rescan is discovery, not a compatibility reset. Preserve a
            // deliberate host-selected fallback and the prefix isolation mode
            // across idempotent imports of the same opaque game reference.
            entry.compatibility = self.wine_inventory[index].compatibility.clone();
            self.wine_inventory[index] = entry;
            return Ok(false);
        }

        self.wine_inventory.push(entry);
        Ok(true)
    }

    /// Associate a pre-existing local Direct Windows executable with a Wine
    /// inventory entry. The original direct card remains untouched so this is
    /// reversible: removing the Wine profile reveals it again. The caller
    /// must have already canonicalised and revalidated the executable through
    /// the Wine host; this catalog method only ensures the transition remains
    /// structurally atomic.
    pub fn associate_direct_game_with_wine_profile(
        &mut self,
        direct_game_id: &str,
        inventory: WineGameInventoryEntry,
        runner_game: Game,
    ) -> Result<bool, CatalogError> {
        validate_direct_game_id(direct_game_id)?;
        let direct_game = self
            .games
            .iter()
            .find(|game| game.id == direct_game_id)
            .ok_or_else(|| CatalogError::Invalid("direct game is no longer available".into()))?;
        validate_associable_direct_game(direct_game)?;

        if inventory.origin_direct_game_id.as_deref() != Some(direct_game_id) {
            return Err(CatalogError::Invalid(
                "Wine inventory entry must retain its direct game origin".into(),
            ));
        }
        let (runner_id, profile_id, game_ref) =
            runner_target_key(&runner_game).ok_or_else(|| {
                CatalogError::Invalid("Wine association requires a runner game".into())
            })?;
        if runner_id != WINE_STAGING_RUNNER_ID
            || profile_id != inventory.profile_id
            || game_ref != inventory.game_ref
        {
            return Err(CatalogError::Invalid(
                "Wine association runner target does not match its inventory".into(),
            ));
        }

        if self.wine_inventory.iter().any(|entry| {
            entry.origin_direct_game_id.as_deref() == Some(direct_game_id)
                && (entry.profile_id != inventory.profile_id
                    || entry.game_ref != inventory.game_ref)
        }) {
            return Err(CatalogError::Invalid(
                "direct game is already associated with another Wine profile".into(),
            ));
        }

        let mut candidate = self.clone();
        candidate.upsert_wine_inventory(inventory)?;
        let inserted = candidate.upsert_runner(runner_game)?;
        candidate.validate()?;
        *self = candidate;
        Ok(inserted)
    }

    /// Remove an Orivo-owned Wine profile and its private inventory. The
    /// linked library cards are removed atomically from the in-memory catalog,
    /// while Direct, Steam, and other runner records are never touched.
    pub fn remove_wine_profile(&mut self, profile_id: &str) -> Result<bool, CatalogError> {
        validate_opaque_runner_token("profile id", profile_id, MAX_PROFILE_ID_LENGTH)?;
        if self.wine_profile(profile_id).is_none() {
            return Ok(false);
        }

        let mut candidate = self.clone();
        candidate
            .wine_profiles
            .retain(|profile| profile.id != profile_id);
        candidate
            .wine_inventory
            .retain(|entry| entry.profile_id != profile_id);
        candidate.games.retain(|game| {
            !matches!(
                &game.launch_target,
                LaunchTarget::Runner {
                    runner_id,
                    profile_id: target_profile_id,
                    ..
                } if runner_id == WINE_STAGING_RUNNER_ID && target_profile_id == profile_id
            )
        });
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(CatalogError::UnsupportedSchema {
                found: self.schema_version,
                current: CURRENT_SCHEMA_VERSION,
            });
        }
        let mut wine_profiles = BTreeMap::new();
        let mut wine_prefixes = BTreeSet::new();
        for profile in &self.wine_profiles {
            profile.validate()?;
            if wine_profiles.insert(profile.id.as_str(), profile).is_some() {
                return Err(CatalogError::Invalid("duplicate Wine profile id".into()));
            }
            if !wine_prefixes.insert(profile.prefix.clone()) {
                return Err(CatalogError::Invalid(
                    "Wine prefixes cannot be shared across profiles".into(),
                ));
            }
        }

        let mut wine_inventory = BTreeSet::new();
        let mut direct_game_origins = BTreeSet::new();
        for entry in &self.wine_inventory {
            entry.validate()?;
            let profile = wine_profiles
                .get(entry.profile_id.as_str())
                .ok_or_else(|| {
                    CatalogError::Invalid(
                        "Wine inventory entry references an unknown profile".into(),
                    )
                })?;
            validate_inventory_scope(entry, profile)?;
            if !wine_inventory.insert((entry.profile_id.as_str(), entry.game_ref.as_str())) {
                return Err(CatalogError::Invalid(
                    "duplicate Wine inventory game reference for profile".into(),
                ));
            }
            if let Some(direct_game_id) = entry.origin_direct_game_id.as_deref() {
                validate_direct_game_id(direct_game_id)?;
                if !direct_game_origins.insert(direct_game_id) {
                    return Err(CatalogError::Invalid(
                        "direct game is associated with more than one Wine profile".into(),
                    ));
                }
            }
        }

        let mut ids = BTreeSet::new();
        let mut source_ids = BTreeSet::new();
        let mut runner_targets = BTreeSet::new();
        let mut direct_games = BTreeMap::new();
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
            if let Some((runner_id, profile_id, game_ref)) = runner_target_key(game) {
                if !runner_targets.insert((runner_id, profile_id, game_ref)) {
                    return Err(CatalogError::Invalid(
                        "duplicate runner game reference for profile".into(),
                    ));
                }
                if runner_id == WINE_STAGING_RUNNER_ID {
                    let profile = wine_profiles.get(profile_id).ok_or_else(|| {
                        CatalogError::Invalid("Wine game references an unknown profile".into())
                    })?;
                    if !wine_inventory.contains(&(profile_id, game_ref)) {
                        return Err(CatalogError::Invalid(
                            "Wine game is missing its private inventory entry".into(),
                        ));
                    }
                    // Re-check the profile reference here so a future change
                    // to inventory validation cannot weaken runner targets.
                    profile.validate()?;
                }
            }
            if matches!(&game.launch_target, LaunchTarget::Direct) {
                direct_games.insert(game.id.as_str(), game);
            }
        }
        for entry in &self.wine_inventory {
            let Some(direct_game_id) = entry.origin_direct_game_id.as_deref() else {
                continue;
            };
            let direct_game = direct_games.get(direct_game_id).ok_or_else(|| {
                CatalogError::Invalid("Wine association references a missing Direct game".into())
            })?;
            validate_associable_direct_game(direct_game)?;
        }
        Ok(())
    }

    fn validate_wine_runner_reference(
        &self,
        profile_id: &str,
        game_ref: &str,
    ) -> Result<(), CatalogError> {
        let profile = self.wine_profile(profile_id).ok_or_else(|| {
            CatalogError::Invalid("Wine game references an unknown profile".into())
        })?;
        if self.wine_inventory_entry(profile_id, game_ref).is_none() {
            return Err(CatalogError::Invalid(
                "Wine game is missing its private inventory entry".into(),
            ));
        }
        profile.validate()
    }
}

fn has_steam_store_copy(extra: &BTreeMap<String, serde_json::Value>) -> bool {
    extra.contains_key(STEAM_STORE_METADATA_MARKER)
        || extra.contains_key(LEGACY_STEAM_STORE_METADATA_MARKER)
}

const MAX_WINE_PROFILE_NAME_LENGTH: usize = 120;
const MAX_WINE_GAME_TITLE_LENGTH: usize = 512;
const MAX_WINE_FINGERPRINT_LENGTH: usize = 256;
const MIN_WINE_VIRTUAL_DESKTOP_DIMENSION: u16 = 320;
const MAX_WINE_VIRTUAL_DESKTOP_DIMENSION: u16 = 8192;

impl WineProfile {
    /// Validate only the stable on-disk shape of a profile. The native host
    /// separately checks filesystem existence, code identity, permissions,
    /// and canonical scope containment immediately before import or launch.
    pub fn validate(&self) -> Result<(), CatalogError> {
        validate_opaque_runner_token("profile id", &self.id, MAX_PROFILE_ID_LENGTH)?;
        validate_display_text(
            "Wine profile name",
            &self.display_name,
            MAX_WINE_PROFILE_NAME_LENGTH,
        )?;
        validate_private_absolute_path("Wine binary", &self.wine_binary)?;
        validate_private_absolute_path("Wine prefix", &self.prefix)?;
        if self.game_directories.is_empty() {
            return Err(CatalogError::Invalid(
                "Wine profile needs at least one granted game directory".into(),
            ));
        }
        let mut game_directories = BTreeSet::new();
        for directory in &self.game_directories {
            validate_private_absolute_path("Wine game directory", directory)?;
            if !game_directories.insert(directory) {
                return Err(CatalogError::Invalid(
                    "Wine profile has duplicate granted game directories".into(),
                ));
            }
        }
        self.graphics.validate()?;
        if self.graphics.backend == WineGraphicsBackend::Auto {
            return Err(CatalogError::Invalid(
                "Wine profile graphics cannot use an automatic game backend".into(),
            ));
        }
        Ok(())
    }
}

impl WineGraphicsOptions {
    pub fn validate(&self) -> Result<(), CatalogError> {
        if let Some(virtual_desktop) = &self.virtual_desktop {
            virtual_desktop.validate()?;
        }
        Ok(())
    }
}

impl WineVirtualDesktop {
    pub fn validate(&self) -> Result<(), CatalogError> {
        let bounds = MIN_WINE_VIRTUAL_DESKTOP_DIMENSION..=MAX_WINE_VIRTUAL_DESKTOP_DIMENSION;
        if !bounds.contains(&self.width) || !bounds.contains(&self.height) {
            return Err(CatalogError::Invalid(format!(
                "Wine virtual desktop dimensions must be between {MIN_WINE_VIRTUAL_DESKTOP_DIMENSION} and {MAX_WINE_VIRTUAL_DESKTOP_DIMENSION}"
            )));
        }
        Ok(())
    }
}

impl WineGameInventoryEntry {
    /// Paths in this type are private host data. This verifies their durable
    /// shape; the host must canonicalize and recheck them against a live grant
    /// before it reads the executable or starts Wine.
    pub fn validate(&self) -> Result<(), CatalogError> {
        validate_opaque_runner_token("profile id", &self.profile_id, MAX_PROFILE_ID_LENGTH)?;
        validate_opaque_runner_token("game reference", &self.game_ref, MAX_GAME_REF_LENGTH)?;
        validate_display_text("Wine game title", &self.title, MAX_WINE_GAME_TITLE_LENGTH)?;
        validate_private_absolute_path("Wine game executable", &self.executable_path)?;
        if !self
            .executable_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            return Err(CatalogError::Invalid(
                "Wine inventory executable must be a Windows .exe file".into(),
            ));
        }
        validate_opaque_runner_token(
            "Wine game fingerprint",
            &self.fingerprint,
            MAX_WINE_FINGERPRINT_LENGTH,
        )?;
        self.compatibility.validate()?;
        if let Some(direct_game_id) = self.origin_direct_game_id.as_deref() {
            validate_direct_game_id(direct_game_id)?;
        }
        Ok(())
    }
}

/// A legacy direct game id may be a canonical path from an older catalog. It
/// is used solely as an exact catalog lookup key, never passed to a process or
/// interpreted as a new filesystem path from the WebView.
fn validate_direct_game_id(value: &str) -> Result<(), CatalogError> {
    if value.is_empty() || value.len() > 8_192 || value.chars().any(char::is_control) {
        return Err(CatalogError::Invalid(
            "direct game origin must be a bounded catalog identifier".into(),
        ));
    }
    Ok(())
}

fn validate_associable_direct_game(game: &Game) -> Result<(), CatalogError> {
    if game.source != GameSource::Local || !matches!(&game.launch_target, LaunchTarget::Direct) {
        return Err(CatalogError::Invalid(
            "Wine association requires a local Direct game".into(),
        ));
    }
    let executable = game.executable_path.as_deref().ok_or_else(|| {
        CatalogError::Invalid("Wine association requires a local Windows executable".into())
    })?;
    if !executable
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err(CatalogError::Invalid(
            "Wine association requires a Windows .exe file".into(),
        ));
    }
    Ok(())
}

fn validate_display_text(field: &str, value: &str, max_length: usize) -> Result<(), CatalogError> {
    if value.trim().is_empty() || value.len() > max_length || value.chars().any(char::is_control) {
        return Err(CatalogError::Invalid(format!(
            "{field} must be non-empty display text"
        )));
    }
    Ok(())
}

/// This is not a filesystem authorization check. It rejects relative,
/// traversal-shaped, or root-only persisted paths before a host operation can
/// accidentally interpret them. The launch/import host still canonicalizes
/// live paths and applies security-scoped grants at the point of use.
fn validate_private_absolute_path(field: &str, path: &Path) -> Result<(), CatalogError> {
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || path == Path::new("/")
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(CatalogError::Invalid(format!(
            "{field} must be an absolute host-owned path"
        )));
    }
    Ok(())
}

fn validate_inventory_scope(
    entry: &WineGameInventoryEntry,
    profile: &WineProfile,
) -> Result<(), CatalogError> {
    if profile.game_directories.iter().any(|directory| {
        entry.executable_path.as_path() != directory.as_path()
            && entry.executable_path.starts_with(directory)
    }) {
        Ok(())
    } else {
        Err(CatalogError::Invalid(
            "Wine game executable is outside the profile's granted directories".into(),
        ))
    }
}

fn runner_target_key(game: &Game) -> Option<(&str, &str, &str)> {
    match &game.launch_target {
        LaunchTarget::Runner {
            runner_id,
            profile_id,
            game_ref,
        } => Some((runner_id, profile_id, game_ref)),
        LaunchTarget::Direct | LaunchTarget::Steam { .. } => None,
    }
}

fn preserve_runner_game_state(incoming: &mut Game, existing: &Game) {
    if incoming.description.is_none() {
        incoming.description = existing.description.clone();
    }
    if incoming.metadata.is_none() {
        incoming.metadata = existing.metadata.clone();
    }
    if incoming.artwork_path.is_none() {
        incoming.artwork_path = existing.artwork_path.clone();
    }
    if incoming.artwork_source_path.is_none() {
        incoming.artwork_source_path = existing.artwork_source_path.clone();
    }
    if incoming.cover_path.is_none() {
        incoming.cover_path = existing.cover_path.clone();
    }
    if incoming.cover_source_path.is_none() {
        incoming.cover_source_path = existing.cover_source_path.clone();
    }
    if incoming.logo_path.is_none() {
        incoming.logo_path = existing.logo_path.clone();
    }
    if incoming.hero_video_path.is_none() {
        incoming.hero_video_path = existing.hero_video_path.clone();
    }
    if incoming.last_played_at.is_none() {
        incoming.last_played_at = existing.last_played_at.clone();
    }
    if incoming.play_time_seconds == 0 {
        incoming.play_time_seconds = existing.play_time_seconds;
    }
    for (key, value) in &existing.extra {
        incoming
            .extra
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
}

fn migrate_v1_to_v2(catalog: &mut Catalog) {
    // v1 records always represented direct executable launches. The v2
    // fields deserialize with defaults, so upgrading is deterministic and
    // does not invent provider-owned data.
    catalog.schema_version = SCHEMA_VERSION_V2;
}

fn migrate_v2_to_v3(catalog: &mut Catalog) {
    // v2 records already use a structured launch target. `Runner` is an
    // additive variant, so no existing game needs a rewritten target.
    catalog.schema_version = SCHEMA_VERSION_V3;
}

fn migrate_v3_to_v4(catalog: &mut Catalog) {
    // `wine_profiles` and `wine_inventory` use serde defaults. Their absence
    // in a v3 file therefore represents an empty private Wine store rather
    // than a lossy conversion of any existing direct, Steam, or runner game.
    catalog.schema_version = SCHEMA_VERSION_V4;
}

fn migrate_v4_to_v5(catalog: &mut Catalog) {
    // `WineGraphicsOptions.backend` and the optional Direct-origin marker on
    // private inventory entries both deserialize to safe defaults. A v4 Wine
    // profile therefore keeps its current WineD3D behavior until a user
    // explicitly installs and enables the experimental DXVK-macOS backend.
    catalog.schema_version = SCHEMA_VERSION_V5;
}

fn migrate_v5_to_v6(catalog: &mut Catalog) {
    // A profile-wide graphics option and prefix were the v5 launch contract.
    // Copy that exact closed setting to every existing Wine game before new
    // imports begin using isolated `Auto`. Direct/Steam records and typed
    // runner references are intentionally left byte-for-byte untouched.
    let legacy_graphics = catalog
        .wine_profiles
        .iter()
        .map(|profile| (profile.id.clone(), profile.graphics.clone()))
        .collect::<BTreeMap<_, _>>();
    for entry in &mut catalog.wine_inventory {
        let graphics = legacy_graphics
            .get(&entry.profile_id)
            .cloned()
            .unwrap_or_default();
        entry.compatibility = WineGameCompatibility::legacy_profile(graphics);
    }
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
            (
                GameSource::Local,
                _,
                LaunchTarget::Runner {
                    runner_id,
                    game_ref,
                    profile_id,
                },
            ) => {
                validate_runner_target(runner_id, game_ref, profile_id)?;
                // Runner launch configuration belongs to the host-owned
                // profile. Keeping all executable-style fields empty makes it
                // impossible for a catalog record to smuggle a command into a
                // future runner implementation.
                if self.executable_path.is_some()
                    || self.installation_path.is_some()
                    || self.working_directory.is_some()
                    || !self.arguments.is_empty()
                {
                    return Err(CatalogError::Invalid(format!(
                        "runner game {} cannot contain executable launch fields",
                        self.id
                    )));
                }
            }
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

const MAX_RUNNER_ID_LENGTH: usize = 128;
const MAX_PROFILE_ID_LENGTH: usize = 128;
const MAX_GAME_REF_LENGTH: usize = 512;

fn validate_runner_target(
    runner_id: &str,
    game_ref: &str,
    profile_id: &str,
) -> Result<(), CatalogError> {
    validate_opaque_runner_token("runner id", runner_id, MAX_RUNNER_ID_LENGTH)?;
    validate_opaque_runner_token("profile id", profile_id, MAX_PROFILE_ID_LENGTH)?;
    validate_opaque_runner_token("game reference", game_ref, MAX_GAME_REF_LENGTH)?;
    Ok(())
}

/// Runner fields are references in Orivo's domain, not filesystem locations
/// or command fragments. The deliberately small grammar leaves room for
/// namespaced IDs such as `com.orivo.ryujinx` and `rom:sha256:…`, while
/// excluding whitespace, path separators, shell metacharacters, and control
/// characters before a runner host ever sees the record.
fn validate_opaque_runner_token(
    field: &str,
    value: &str,
    max_length: usize,
) -> Result<(), CatalogError> {
    let mut bytes = value.bytes();
    let starts_with_alphanumeric = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric());
    let is_safe = starts_with_alphanumeric
        && value.len() <= max_length
        && bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'));
    if is_safe {
        Ok(())
    } else {
        Err(CatalogError::Invalid(format!(
            "runner {field} must be a non-empty opaque identifier"
        )))
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
            ..Catalog::default()
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
        assert!(loaded.catalog.wine_profiles.is_empty());
        assert!(loaded.catalog.wine_inventory.is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), v1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn upgrades_a_v2_steam_game_without_rewriting_its_source_file() {
        let path = temporary_catalog_path("v2-load");
        let v2 = r#"{
  "schema_version": 2,
  "games": [
    {
      "id": "steam:480",
      "title": "Spacewar",
      "source": "steam",
      "source_id": "480",
      "launch_target": { "kind": "steam", "app_id": 480 }
    }
  ]
}"#;
        fs::write(&path, v2).unwrap();

        let loaded = Catalog::load_with_migration(&path).unwrap();

        assert_eq!(loaded.migrated_from, Some(SCHEMA_VERSION_V2));
        assert_eq!(loaded.catalog.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(
            loaded.catalog.games[0].launch_target,
            LaunchTarget::Steam { app_id: 480 }
        );
        assert!(loaded.catalog.wine_profiles.is_empty());
        assert!(loaded.catalog.wine_inventory.is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), v2);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn upgrades_a_v3_direct_and_steam_catalog_without_losing_games() {
        let path = temporary_catalog_path("v3-load");
        let v3 = r#"{
  "schema_version": 3,
  "games": [
    {
      "id": "local-example",
      "title": "Example",
      "executable_path": "/Games/Example.app/Contents/MacOS/Example"
    },
    {
      "id": "steam:480",
      "title": "Spacewar",
      "source": "steam",
      "source_id": "480",
      "launch_target": { "kind": "steam", "app_id": 480 }
    }
  ]
}"#;
        fs::write(&path, v3).unwrap();

        let loaded = Catalog::load_with_migration(&path).unwrap();

        assert_eq!(loaded.migrated_from, Some(SCHEMA_VERSION_V3));
        assert_eq!(loaded.catalog.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.catalog.games.len(), 2);
        assert_eq!(loaded.catalog.games[0].launch_target, LaunchTarget::Direct);
        assert_eq!(
            loaded.catalog.games[1].launch_target,
            LaunchTarget::Steam { app_id: 480 }
        );
        assert!(loaded.catalog.wine_profiles.is_empty());
        assert!(loaded.catalog.wine_inventory.is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), v3);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn upgrades_a_v4_catalog_without_changing_existing_games_or_wine_entries() {
        let path = temporary_catalog_path("v4-load");
        let v4 = r#"{
  "schema_version": 4,
  "games": [
    {
      "id": "local-blue-prince",
      "title": "Blue Prince",
      "executable_path": "/Games/Direct/BLUE PRINCE.exe"
    },
    {
      "id": "steam:480",
      "title": "Spacewar",
      "source": "steam",
      "source_id": "480",
      "launch_target": { "kind": "steam", "app_id": 480 }
    },
    {
      "id": "runner:wine-example",
      "title": "Windows Example",
      "source": "local",
      "launch_target": {
        "kind": "runner",
        "runner_id": "com.orivo.wine-staging",
        "profile_id": "wine-profile-1",
        "game_ref": "wine-game-1"
      }
    }
  ],
  "wine_profiles": [
    {
      "id": "wine-profile-1",
      "display_name": "Windows games",
      "wine_binary": "/Applications/Wine Staging.app/Contents/Resources/wine/bin/wine",
      "prefix": "/Users/orivo/Library/Application Support/Orivo/wine-prefixes/wine-profile-1",
      "game_directories": ["/Games/Windows"],
      "graphics": { "virtual_desktop": { "width": 1280, "height": 720 } },
      "enabled": true
    }
  ],
  "wine_inventory": [
    {
      "profile_id": "wine-profile-1",
      "game_ref": "wine-game-1",
      "title": "Windows Example",
      "executable_path": "/Games/Windows/Example/Game.exe",
      "fingerprint": "sha256:abc123"
    }
  ]
}"#;
        fs::write(&path, v4).unwrap();

        let loaded = Catalog::load_with_migration(&path).unwrap();

        assert_eq!(loaded.migrated_from, Some(SCHEMA_VERSION_V4));
        assert_eq!(loaded.catalog.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.catalog.games.len(), 3);
        assert!(matches!(
            &loaded.catalog.games[0].launch_target,
            LaunchTarget::Direct
        ));
        assert!(matches!(
            &loaded.catalog.games[1].launch_target,
            LaunchTarget::Steam { app_id: 480 }
        ));
        assert!(matches!(
            &loaded.catalog.games[2].launch_target,
            LaunchTarget::Runner { .. }
        ));
        assert_eq!(loaded.catalog.wine_profiles.len(), 1);
        assert_eq!(
            loaded.catalog.wine_profiles[0].graphics.backend,
            WineGraphicsBackend::WineD3d
        );
        assert!(
            loaded.catalog.wine_profiles[0]
                .graphics
                .virtual_desktop
                .is_some()
        );
        assert_eq!(loaded.catalog.wine_inventory.len(), 1);
        assert_eq!(fs::read_to_string(&path).unwrap(), v4);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn wine_profile_defaults_enabled_when_read_from_persistence() {
        let profile: WineProfile = serde_json::from_value(serde_json::json!({
            "id": "wine-profile-1",
            "display_name": "Windows classics",
            "wine_binary": "/Applications/Wine Staging.app/Contents/Resources/wine/bin/wine",
            "prefix": "/Users/orivo/Library/Application Support/Orivo/wine-prefixes/wine-profile-1",
            "game_directories": ["/Games/Windows"]
        }))
        .unwrap();

        assert!(profile.enabled);
        assert_eq!(profile.graphics, WineGraphicsOptions::default());
        assert_eq!(profile.last_imported_at, None);
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
            ..Catalog::default()
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
    fn validates_a_typed_runner_launch_target() {
        let game = runner_game();

        assert!(game.validate().is_ok());
        assert_eq!(
            serde_json::to_value(&game).unwrap()["launch_target"],
            serde_json::json!({
                "kind": "runner",
                "runner_id": "com.orivo.ryujinx",
                "game_ref": "rom:sha256:abc123",
                "profile_id": "profile-7f3b",
            })
        );
    }

    #[test]
    fn rejects_a_runner_target_with_a_path_like_game_reference() {
        let mut game = runner_game();
        game.launch_target = LaunchTarget::Runner {
            runner_id: "com.orivo.ryujinx".into(),
            game_ref: "/Users/example/Library/Game.nsp".into(),
            profile_id: "profile-7f3b".into(),
        };

        assert!(
            matches!(game.validate(), Err(CatalogError::Invalid(message)) if message.contains("game reference"))
        );
    }

    #[test]
    fn rejects_executable_style_fields_on_a_runner_target() {
        let mut game = runner_game();
        game.executable_path = Some(PathBuf::from("/Applications/Ryujinx.app"));
        game.arguments = vec!["--unsafe-argument".into()];

        assert!(
            matches!(game.validate(), Err(CatalogError::Invalid(message)) if message.contains("cannot contain executable launch fields"))
        );
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

    #[test]
    fn persists_a_wine_profile_inventory_and_runner_card() {
        let path = temporary_catalog_path("wine-persist");
        let mut catalog = catalog_with_wine_profile();
        catalog
            .upsert_wine_inventory(wine_inventory_entry("wine-game-abc123"))
            .unwrap();
        catalog
            .upsert_runner(wine_runner_game("wine-game-abc123", "Windows Example"))
            .unwrap();
        catalog.save_atomically(&path).unwrap();

        let reloaded = Catalog::load(&path).unwrap();
        let expected_inventory = wine_inventory_entry("wine-game-abc123");

        assert_eq!(reloaded.wine_profiles.len(), 1);
        assert_eq!(reloaded.wine_inventory.len(), 1);
        assert_eq!(reloaded.games.len(), 1);
        assert_eq!(
            reloaded.wine_inventory_entry("wine-profile-1", "wine-game-abc123"),
            Some(&expected_inventory)
        );
        assert!(matches!(
            &reloaded.games[0].launch_target,
            LaunchTarget::Runner { runner_id, profile_id, game_ref }
                if runner_id == WINE_STAGING_RUNNER_ID
                    && profile_id == "wine-profile-1"
                    && game_ref == "wine-game-abc123"
        ));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn refuses_wine_inventory_outside_the_profile_grant() {
        let mut catalog = catalog_with_wine_profile();
        let mut entry = wine_inventory_entry("wine-game-outside");
        entry.executable_path = PathBuf::from("/Elsewhere/Windows/Example.exe");

        assert!(matches!(
            catalog.upsert_wine_inventory(entry),
            Err(CatalogError::Invalid(message)) if message.contains("outside the profile's granted directories")
        ));
        assert!(catalog.wine_inventory.is_empty());
    }

    #[test]
    fn rejects_a_wine_profile_with_a_relative_binary_path() {
        let mut profile = wine_profile();
        profile.wine_binary = PathBuf::from("wine");

        assert!(matches!(
            profile.validate(),
            Err(CatalogError::Invalid(message)) if message.contains("Wine binary")
        ));
    }

    #[test]
    fn rejects_unbounded_wine_virtual_desktop_dimensions() {
        let mut profile = wine_profile();
        profile.graphics.virtual_desktop = Some(WineVirtualDesktop {
            width: 200,
            height: 9_000,
        });

        assert!(matches!(
            profile.validate(),
            Err(CatalogError::Invalid(message)) if message.contains("virtual desktop dimensions")
        ));
    }

    #[test]
    fn upserts_runner_games_by_runner_profile_and_game_reference() {
        let mut catalog = catalog_with_wine_profile();
        catalog
            .upsert_wine_inventory(wine_inventory_entry("wine-game-abc123"))
            .unwrap();

        let mut first = wine_runner_game("wine-game-abc123", "Original title");
        first.last_played_at = Some("2026-07-21T09:00:00Z".into());
        first.play_time_seconds = 42;
        first.extra.insert(
            "orivo_user_note".into(),
            serde_json::Value::String("keep me".into()),
        );
        assert!(catalog.upsert_runner(first).unwrap());

        let mut refreshed = wine_runner_game("wine-game-abc123", "New scanner title");
        refreshed.id = "runner:changed-id".into();
        assert!(!catalog.upsert_runner(refreshed).unwrap());

        assert_eq!(catalog.games.len(), 1);
        assert_eq!(catalog.games[0].id, "runner:wine-game-abc123");
        assert_eq!(catalog.games[0].title, "New scanner title");
        assert_eq!(
            catalog.games[0].last_played_at.as_deref(),
            Some("2026-07-21T09:00:00Z")
        );
        assert_eq!(catalog.games[0].play_time_seconds, 42);
        assert_eq!(
            catalog.games[0]
                .extra
                .get("orivo_user_note")
                .and_then(serde_json::Value::as_str),
            Some("keep me")
        );
        catalog.validate().unwrap();
    }

    #[test]
    fn removes_only_the_selected_wine_profile_and_its_cards() {
        let mut catalog = catalog_with_wine_profile();
        catalog
            .upsert_wine_inventory(wine_inventory_entry("wine-game-abc123"))
            .unwrap();
        catalog
            .upsert_runner(wine_runner_game("wine-game-abc123", "Windows Example"))
            .unwrap();
        catalog.upsert_steam(steam_game("Spacewar")).unwrap();

        assert!(catalog.remove_wine_profile("wine-profile-1").unwrap());

        assert!(catalog.wine_profiles.is_empty());
        assert!(catalog.wine_inventory.is_empty());
        assert_eq!(catalog.games.len(), 1);
        assert_eq!(catalog.games[0].source, GameSource::Steam);
        assert!(catalog.validate().is_ok());
    }

    #[test]
    fn associates_a_direct_windows_game_reversibly_without_copying_launch_fields() {
        let path = temporary_catalog_path("direct-wine-association");
        let mut catalog = catalog_with_wine_profile();
        let direct = direct_windows_game();
        let direct_id = direct.id.clone();
        catalog.add(direct.clone()).unwrap();
        let mut inventory = wine_inventory_entry("wine-game-blue-prince");
        inventory.title = direct.title.clone();
        inventory.executable_path = direct.executable_path.clone().unwrap();
        inventory.origin_direct_game_id = Some(direct_id.clone());
        let runner = wine_runner_game("wine-game-blue-prince", &direct.title);

        assert!(
            catalog
                .associate_direct_game_with_wine_profile(
                    &direct_id,
                    inventory.clone(),
                    runner.clone()
                )
                .unwrap()
        );
        assert!(
            !catalog
                .associate_direct_game_with_wine_profile(&direct_id, inventory.clone(), runner)
                .unwrap()
        );
        assert_eq!(catalog.games.len(), 2);
        assert_eq!(
            catalog
                .wine_inventory_entry("wine-profile-1", "wine-game-blue-prince")
                .and_then(|entry| entry.origin_direct_game_id.as_deref()),
            Some(direct_id.as_str())
        );
        let runner_card = catalog
            .games
            .iter()
            .find(|game| matches!(&game.launch_target, LaunchTarget::Runner { .. }))
            .unwrap();
        assert_eq!(runner_card.title, "Blue Prince");
        assert!(runner_card.executable_path.is_none());
        assert!(runner_card.working_directory.is_none());
        assert!(runner_card.arguments.is_empty());
        assert_eq!(
            catalog
                .games
                .iter()
                .find(|game| game.id == direct_id)
                .unwrap()
                .arguments,
            vec!["--legacy-direct-option"]
        );
        catalog.save_atomically(&path).unwrap();
        let reloaded = Catalog::load(&path).unwrap();
        assert_eq!(reloaded.games.len(), 2);

        let mut reloaded = reloaded;
        assert!(reloaded.remove_wine_profile("wine-profile-1").unwrap());
        assert_eq!(reloaded.games.len(), 1);
        assert_eq!(reloaded.games[0], direct);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn refuses_an_association_outside_the_wine_profile_scope_atomically() {
        let mut catalog = catalog_with_wine_profile();
        let mut direct = direct_windows_game();
        direct.executable_path = Some(PathBuf::from("/Elsewhere/Blue Prince/BLUE PRINCE.exe"));
        let direct_id = direct.id.clone();
        catalog.add(direct.clone()).unwrap();
        let mut inventory = wine_inventory_entry("wine-game-outside-direct");
        inventory.executable_path = direct.executable_path.clone().unwrap();
        inventory.origin_direct_game_id = Some(direct_id.clone());
        let before = catalog.clone();

        assert!(matches!(
            catalog.associate_direct_game_with_wine_profile(
                &direct_id,
                inventory,
                wine_runner_game("wine-game-outside-direct", "Blue Prince"),
            ),
            Err(CatalogError::Invalid(message)) if message.contains("outside the profile's granted directories")
        ));
        assert_eq!(catalog, before);
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

    fn direct_windows_game() -> Game {
        Game {
            id: "local-blue-prince".into(),
            title: "Blue Prince".into(),
            executable_path: Some(PathBuf::from("/Games/Windows/Blue Prince/BLUE PRINCE.exe")),
            source: GameSource::Local,
            source_id: None,
            launch_target: LaunchTarget::Direct,
            installation_path: None,
            working_directory: Some(PathBuf::from("/Games/Windows/Blue Prince")),
            arguments: vec!["--legacy-direct-option".into()],
            description: Some("A Windows game.".into()),
            metadata: Some("Local import".into()),
            artwork_path: Some(PathBuf::from("/cache/blue-prince.jpg")),
            artwork_source_path: None,
            cover_path: Some(PathBuf::from("/cache/blue-prince-cover.jpg")),
            cover_source_path: None,
            logo_path: None,
            hero_video_path: None,
            last_played_at: Some("2026-08-01T00:00:00Z".into()),
            play_time_seconds: 42,
            extra: BTreeMap::new(),
        }
    }

    fn runner_game() -> Game {
        Game {
            id: "runner:com.orivo.ryujinx:abc123".into(),
            title: "Example Switch Game".into(),
            executable_path: None,
            source: GameSource::Local,
            source_id: None,
            launch_target: LaunchTarget::Runner {
                runner_id: "com.orivo.ryujinx".into(),
                game_ref: "rom:sha256:abc123".into(),
                profile_id: "profile-7f3b".into(),
            },
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

    fn wine_profile() -> WineProfile {
        WineProfile {
            id: "wine-profile-1".into(),
            display_name: "Windows classics".into(),
            wine_binary: PathBuf::from(
                "/Applications/Wine Staging.app/Contents/Resources/wine/bin/wine",
            ),
            prefix: PathBuf::from(
                "/Users/orivo/Library/Application Support/Orivo/wine-prefixes/wine-profile-1",
            ),
            game_directories: vec![PathBuf::from("/Games/Windows")],
            graphics: WineGraphicsOptions::default(),
            dxmt_engine_supported: None,
            macos_retina_mode_enabled: None,
            enabled: true,
            last_imported_at: Some(1_721_553_600_000),
        }
    }

    fn catalog_with_wine_profile() -> Catalog {
        let mut catalog = Catalog::default();
        assert!(catalog.upsert_wine_profile(wine_profile()).unwrap());
        catalog
    }

    fn wine_inventory_entry(game_ref: &str) -> WineGameInventoryEntry {
        WineGameInventoryEntry {
            profile_id: "wine-profile-1".into(),
            game_ref: game_ref.into(),
            title: "Windows Example".into(),
            executable_path: PathBuf::from("/Games/Windows/Example/Game.EXE"),
            fingerprint: "sha256:abc123".into(),
            imported_at: Some(1_721_553_600_000),
            compatibility: WineGameCompatibility::automatic(),
            origin_direct_game_id: None,
        }
    }

    fn wine_runner_game(game_ref: &str, title: &str) -> Game {
        Game {
            id: format!("runner:{game_ref}"),
            title: title.into(),
            executable_path: None,
            source: GameSource::Local,
            source_id: None,
            launch_target: LaunchTarget::Runner {
                runner_id: WINE_STAGING_RUNNER_ID.into(),
                game_ref: game_ref.into(),
                profile_id: "wine-profile-1".into(),
            },
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
