use crate::game_detail::{GameDetailError, StagedGameState};
use crate::plugin_manifest::{CapabilityGrant, CapabilityScope, PluginCapability};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

/// Schema v8 adds the three host-private tables a third-party runner needs to
/// become usable: a validated `RunnerProfile`, the private inventory that maps
/// an opaque game reference to a file inside a granted folder, and the grant
/// ledger the plugin host resolves before every invocation. All three are
/// additive, so a v7 document is read as one with all three empty.
pub const CURRENT_SCHEMA_VERSION: u32 = 8;
const SCHEMA_VERSION_V1: u32 = 1;
const SCHEMA_VERSION_V2: u32 = 2;
const SCHEMA_VERSION_V3: u32 = 3;
const SCHEMA_VERSION_V4: u32 = 4;
const SCHEMA_VERSION_V5: u32 = 5;
const SCHEMA_VERSION_V6: u32 = 6;
const SCHEMA_VERSION_V7: u32 = 7;

/// The stable identity for Orivo's first official Wine runner. It is an
/// opaque runner identifier, never a Wine executable path or command.
pub const WINE_STAGING_RUNNER_ID: &str = "com.orivo.wine-staging";

/// The stable identity for Orivo's Winlator runner on Android. Like every
/// runner id it is opaque: never an Android package name, an activity class,
/// or an intent.
pub const WINLATOR_RUNNER_ID: &str = "com.orivo.winlator";

/// The stable identities of Orivo's console-emulator runners on Android.
///
/// One id per *emulator*, not per console and not one for all of them. Per
/// console would claim Orivo knows which app the user wants to play an NES game
/// in; one shared id would make the identity say nothing about which package
/// receives the intent — and that package, with its exported activity and the
/// extras it reads, is the whole of the contract.
pub const RETROARCH_RUNNER_ID: &str = "com.orivo.retroarch";
pub const PPSSPP_RUNNER_ID: &str = "com.orivo.ppsspp";

/// Is this one of the console-emulator runners Orivo ships natively?
pub fn is_console_runner_id(runner_id: &str) -> bool {
    runner_id == RETROARCH_RUNNER_ID || runner_id == PPSSPP_RUNNER_ID
}

fn default_wine_profile_enabled() -> bool {
    true
}

fn default_winlator_profile_enabled() -> bool {
    true
}

fn default_runner_profile_enabled() -> bool {
    true
}

fn default_console_profile_enabled() -> bool {
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

/// Keys reserved for artwork a connected store account published for one of
/// its own games. Only URLs whose host passed the connector's allowlist are
/// ever written here, so the WebView still cannot be pointed at an arbitrary
/// origin by a provider response.
pub const SOURCE_COVER_URL_KEY: &str = "orivo_source_cover_url";
pub const SOURCE_HERO_URL_KEY: &str = "orivo_source_hero_url";
pub const SOURCE_LANDSCAPE_URL_KEY: &str = "orivo_source_landscape_url";
pub const SOURCE_GENRE_KEY: &str = "orivo_source_genre";
/// The studio that made the game, as the store named it.
///
/// It used to travel in `metadata`, which is a mixed field — a connected store
/// fills it with the developer, Steam with install state, Wine with the runner
/// name, the bundled demo with an achievement count. The hero now prints the
/// studio on its own, and a slot that prints "Achievements 67/82" where a
/// company belongs is why this has a key of its own.
pub const SOURCE_DEVELOPER_KEY: &str = "orivo_source_developer";
/// A provider's transparent wordmark, kept apart from the artwork roles: it is
/// drawn over the scene, never used as one.
pub const SOURCE_LOGO_URL_KEY: &str = "orivo_source_logo_url";
/// Whether the store publishes a build of this game that runs natively on
/// macOS. Written only by a connector that can actually tell — Epic lists its
/// entitlements per platform — so an absent key means "unknown", not "no".
pub const SOURCE_NATIVE_MAC_KEY: &str = "orivo_source_native_mac";
/// Every platform the store says this game ships a build for, as the same
/// `windows` / `macos` / `linux` tokens Steam's own answer uses.
///
/// `SOURCE_NATIVE_MAC_KEY` answers one question about one platform, which is
/// all Epic's per-platform entitlement lists can support. A store that
/// publishes the whole matrix — GOG returns `content_system_compatibility` on
/// every product it already fetches — writes it here instead, so a
/// cross-platform title is filed under each platform it actually runs on
/// rather than only under macOS. An absent key means "unknown", never "none".
pub const SOURCE_PLATFORMS_KEY: &str = "orivo_source_platforms";
/// Whether the store's own client reports this game as installed on this
/// machine. A boolean, deliberately not a path: a connected-source record may
/// never carry a filesystem location, so "is it installed" is recorded without
/// ever writing where. The detail page asks the launcher directly when it needs
/// the location.
pub const SOURCE_INSTALLED_KEY: &str = "orivo_source_installed";
/// The percentage of a download the store's own client is still running, and
/// the flag that says one is running at all. Both are re-read from the client
/// on every refresh, so a finished install drops them.
pub const SOURCE_INSTALLING_KEY: &str = "orivo_source_installing";
pub const SOURCE_INSTALL_PERCENT_KEY: &str = "orivo_source_install_percent";

/// The `extra` keys a connected store owns outright.
///
/// Everything else in `extra` — Steam store metadata, a wallpaper chosen in the
/// Store — belongs to Orivo and survives a re-sync. These do not: the provider's
/// latest answer is the whole truth about them, so a value it has stopped
/// publishing has to disappear. Merging them forwards is how a genre the Epic
/// connector wrongly filled with a studio name outlived the fix.
///
/// The three artwork URLs are deliberately absent. Orivo fills them in itself
/// when a store publishes none — that is what `fill_missing_source_artwork`
/// does for Xbox and Microsoft Store — so treating the provider as their sole
/// author let one slow sync, which returns nothing rather than something new,
/// erase covers Orivo had resolved on an earlier pass.
pub const SOURCE_OWNED_EXTRA_KEYS: [&str; 8] = [
    SOURCE_GENRE_KEY,
    SOURCE_DEVELOPER_KEY,
    SOURCE_LOGO_URL_KEY,
    SOURCE_NATIVE_MAC_KEY,
    SOURCE_PLATFORMS_KEY,
    SOURCE_INSTALLED_KEY,
    SOURCE_INSTALLING_KEY,
    SOURCE_INSTALL_PERCENT_KEY,
];

/// The provider that owns the external identity of a library entry.  Catalog
/// records created before sources existed deserialize as `Local`, preserving
/// the v1 file format without a migration.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum GameSource {
    #[default]
    Local,
    Steam,
    Epic,
    Gog,
    Ubisoft,
    Xbox,
    MicrosoftStore,
    InstantGaming,
}

impl GameSource {
    /// The opaque provider token that a connected-account record carries in its
    /// launch target and view model. `Local` and `Steam` are deliberately not
    /// part of this namespace: they have their own dedicated import paths and
    /// their own launch strategies.
    pub fn provider_token(self) -> Option<&'static str> {
        match self {
            Self::Local | Self::Steam => None,
            Self::Epic => Some("epic"),
            Self::Gog => Some("gog"),
            Self::Ubisoft => Some("ubisoft"),
            Self::Xbox => Some("xbox"),
            Self::MicrosoftStore => Some("microsoft-store"),
            Self::InstantGaming => Some("instant-gaming"),
        }
    }

    pub fn from_provider_token(token: &str) -> Option<Self> {
        [
            Self::Epic,
            Self::Gog,
            Self::Ubisoft,
            Self::Xbox,
            Self::MicrosoftStore,
            Self::InstantGaming,
        ]
        .into_iter()
        .find(|source| source.provider_token() == Some(token))
    }
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
    /// A game owned through a connected store account and started by that
    /// store's own client. Both fields are opaque tokens: the host turns them
    /// into one fixed, percent-encoded provider URI, never into a command.
    Provider {
        /// Must equal the record's `GameSource::provider_token()`.
        provider: String,
        /// The provider-owned launch reference (an Epic
        /// `namespace:catalogItem:appName`, a GOG product id, a Ubisoft
        /// launch id, a Microsoft package family name, …).
        app_ref: String,
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
    /// Host-private Winlator references. Unlike Wine these carry no prefix and
    /// no engine binary: Winlator owns both, inside its own app storage.
    #[serde(default)]
    pub winlator_profiles: Vec<WinlatorProfile>,
    /// Host-private mapping from opaque Winlator game references to the
    /// exported shortcut file Orivo hands back to Winlator at launch.
    #[serde(default)]
    pub winlator_inventory: Vec<WinlatorShortcutInventoryEntry>,
    /// Host-private references to the console emulators already installed on
    /// this Android device. Like Winlator these own no engine and no data
    /// directory of Orivo's: a profile is an emulator, a console, and the ROM
    /// folders the user granted.
    #[serde(default)]
    pub console_profiles: Vec<ConsoleEmulatorProfile>,
    /// Host-private mapping from opaque console game references to the ROM file
    /// Orivo hands the emulator at launch.
    #[serde(default)]
    pub console_inventory: Vec<ConsoleRomInventoryEntry>,
    /// Host-private profiles for runners provided by third-party plugin
    /// components. Unlike the two native adapters above, nothing here is
    /// launchable until the owning plugin has accepted the profile.
    #[serde(default)]
    pub runner_profiles: Vec<RunnerProfile>,
    /// Host-private mapping from opaque third-party game references to the
    /// game file the host resolved inside a granted folder.
    #[serde(default)]
    pub runner_inventory: Vec<RunnerGameInventoryEntry>,
    /// The grant ledger. It is persisted beside the profiles it scopes because
    /// granting a folder and recording the permission to read it have to land
    /// or fail together.
    #[serde(default)]
    pub plugin_grants: Vec<PluginGrantRecord>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedCatalog {
    pub catalog: Catalog,
    pub migrated_from: Option<u32>,
    /// Every `old id -> new id` rewrite the migration performed. Durable user
    /// state that is keyed by game id — `game-state.json` — has to be re-keyed
    /// with exactly this map before the migrated catalog is published.
    pub rewritten_game_ids: BTreeMap<String, String>,
}

impl LoadedCatalog {
    /// Publish a migrated catalog and the dependent `game-state.json` re-key as
    /// one unit.
    ///
    /// Wishlist flags, media selections and imported media are keyed by game
    /// id, so a migration that rewrites ids must move both files or neither:
    /// orphaned state would silently lose the user's selections and would keep
    /// its imported files pinned in `protected_local_files` forever, where they
    /// consume the media quota and can never be pruned.
    ///
    /// Everything that can fail is done before either file is published. The
    /// state rewrite is staged and fsynced first, then published with a single
    /// rename, and if the catalog write still fails the previous state document
    /// is put back, so the pair can never end up one migrated and one not.
    /// Publish without anything to fall back on. Production always takes a
    /// backup first; this is the door the tests use when what they are asserting
    /// on is the raw failure rather than the recovery.
    #[cfg(test)]
    pub fn commit_migration(
        &self,
        catalog_path: &Path,
        game_state_path: &Path,
    ) -> Result<(), CatalogError> {
        self.publish(catalog_path, game_state_path, None)
    }

    /// The same publication, with the pre-migration catalog to fall back on.
    ///
    /// A migration has three ways to go wrong, and the backup answers all
    /// three: the migrated document can fail validation, the dependent
    /// `game-state.json` rewrite can fail, and the file that comes back from
    /// disk can disagree with the one that was written. Whichever it is, the
    /// catalog the user had is put back and the error says whether that
    /// succeeded — a library must never end up readable only by the build that
    /// migrated it.
    pub fn commit_migration_with_backup(
        &self,
        catalog_path: &Path,
        game_state_path: &Path,
        backup_path: &Path,
    ) -> Result<(), CatalogError> {
        self.publish(catalog_path, game_state_path, Some(backup_path))
    }

    fn publish(
        &self,
        catalog_path: &Path,
        game_state_path: &Path,
        backup_path: Option<&Path>,
    ) -> Result<(), CatalogError> {
        let failure = match self.publish_once(catalog_path, game_state_path) {
            Ok(()) => return Ok(()),
            Err(failure) => failure,
        };
        let Some(backup_path) = backup_path else {
            return Err(failure);
        };
        match restore_catalog_backup(backup_path, catalog_path) {
            Ok(()) => Err(CatalogError::Invalid(format!(
                "the catalog migration failed ({failure}) and the previous catalog was restored"
            ))),
            Err(restore_error) => Err(CatalogError::Invalid(format!(
                "the catalog migration failed ({failure}) and the previous catalog could not be restored ({restore_error})"
            ))),
        }
    }

    fn publish_once(
        &self,
        catalog_path: &Path,
        game_state_path: &Path,
    ) -> Result<(), CatalogError> {
        self.catalog.validate()?;
        let staged = StagedGameState::stage(game_state_path, &self.rewritten_game_ids)
            .map_err(state_error)?;
        staged.commit().map_err(state_error)?;
        match self
            .catalog
            .save_atomically(catalog_path)
            .and_then(|()| self.verify_published(catalog_path))
        {
            Ok(()) => Ok(()),
            // The game-state rewrite is keyed to the ids this migration
            // invented, so it goes back with the catalog or the pair ends up one
            // migrated and one not.
            Err(error) => match staged.restore() {
                Ok(()) => Err(error),
                Err(restore_error) => Err(CatalogError::Invalid(format!(
                    "{error} and the previous game state could not be restored ({restore_error})"
                ))),
            },
        }
    }

    /// Read back what was just published. The rename is atomic, but the bytes
    /// under it are not guaranteed to be the host's: a file-syncing client, a
    /// restored snapshot, or a write that never reached the device would all
    /// leave a catalog that no longer says what the migration decided, and
    /// noticing that while the backup still exists is the whole point of taking
    /// one.
    fn verify_published(&self, catalog_path: &Path) -> Result<(), CatalogError> {
        let published = Self::verify_load(catalog_path)?;
        if published != self.catalog {
            return Err(CatalogError::Invalid(
                "the published catalog does not match the migrated one".into(),
            ));
        }
        Ok(())
    }

    fn verify_load(catalog_path: &Path) -> Result<Catalog, CatalogError> {
        let loaded = Catalog::load_with_migration(catalog_path)?;
        if loaded.migrated_from.is_some() {
            return Err(CatalogError::Invalid(
                "the published catalog still reports an older schema".into(),
            ));
        }
        Ok(loaded.catalog)
    }
}

/// Put a pre-migration catalog back, atomically. The staged copy is written and
/// fsynced before the rename, so a crash in the middle leaves the migrated file
/// rather than a half-restored one.
pub fn restore_catalog_backup(backup_path: &Path, catalog_path: &Path) -> Result<(), CatalogError> {
    let bytes = fs::read(backup_path)?;
    if bytes.is_empty() {
        return Err(CatalogError::Invalid("the catalog backup is empty".into()));
    }
    let staging = catalog_path.with_extension("json.restoring");
    let outcome = (|| -> Result<(), io::Error> {
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
        fs::rename(&staging, catalog_path)
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&staging);
    }
    outcome.map_err(CatalogError::Io)
}

fn state_error(error: GameDetailError) -> CatalogError {
    CatalogError::Invalid(format!("game state could not be migrated: {error}"))
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

/// Which Winlator build a profile points at. This is closed because each entry
/// is a different Android package whose *exported* launch surface differs, as
/// read from that project's own `AndroidManifest.xml`. A distribution can never
/// be a package name supplied by the WebView.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum WinlatorDistribution {
    /// The `com.winlator.cmod` lineage (Winlator Cmod and the forks that took
    /// its code), the only published Winlator family whose display activity is
    /// exported and therefore reachable from another app.
    #[default]
    Cmod,
    /// brunodev85's official `com.winlator` build. A profile may name it so
    /// Orivo can say *why* it cannot start a game there instead of failing
    /// silently: its display activity is not exported.
    Official,
}

/// A Winlator installation Orivo may hand a game to.
///
/// This is deliberately a reference and not a prefix. A Winlator Wine prefix
/// lives inside a *container*, in Winlator's private app storage, which Orivo
/// can neither create, read, nor validate. What Orivo does own is the granted
/// directory holding the shortcuts Winlator exported for a frontend, and the
/// decision to send an intent at all.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WinlatorProfile {
    /// Opaque, stable Orivo profile identifier.
    pub id: String,
    /// User-facing label, distinct from any filesystem component.
    pub display_name: String,
    #[serde(default)]
    pub distribution: WinlatorDistribution,
    /// The Winlator container to activate for shortcuts that do not name one.
    /// Winlator numbers its own containers and Orivo cannot enumerate them, so
    /// this is an unverifiable hint bounded to a small integer.
    #[serde(default)]
    pub container_id: Option<u32>,
    /// Directories explicitly granted to this profile, where Winlator wrote
    /// its exported frontend shortcuts. No implicit device-wide fallback.
    #[serde(default)]
    pub shortcut_directories: Vec<PathBuf>,
    /// Android storage access grants covering those same directories, when the
    /// folder is one Orivo can only read through a `ContentResolver`. A grant is
    /// held *in addition to* the directory it stands for, never instead of it:
    /// the directory is the scope every check already uses and the path Winlator
    /// itself opens, and the tree URI is only how Orivo reads it.
    #[serde(default)]
    pub shortcut_trees: Vec<String>,
    /// Disabled profiles and their games stay persisted and visible, but
    /// cannot be launched until the user enables them again.
    #[serde(default = "default_winlator_profile_enabled")]
    pub enabled: bool,
    /// Unix milliseconds of the last completed import, if one has completed.
    #[serde(default)]
    pub last_imported_at: Option<u64>,
}

/// The private shortcut inventory behind a Winlator runner game. `game_ref` is
/// the sole value copied into `LaunchTarget::Runner`; `shortcut_path` never
/// crosses the WebView boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WinlatorShortcutInventoryEntry {
    pub profile_id: String,
    pub game_ref: String,
    pub title: String,
    /// The `.desktop` file Winlator exported into a granted directory. Orivo
    /// reads and hashes this file; it never reads the container behind it.
    pub shortcut_path: PathBuf,
    /// A namespaced content hash of the shortcut file, so a rewritten shortcut
    /// is refused until a deliberate reimport has updated this inventory.
    pub fingerprint: String,
    /// The container id Winlator wrote into the exported shortcut, when it
    /// wrote one. Absent means Winlator resolves the container itself.
    #[serde(default)]
    pub container_id: Option<u32>,
    #[serde(default)]
    pub imported_at: Option<u64>,
}

/// Which installed Android emulator a profile hands a game to.
///
/// Closed, because each entry is a different package whose *exported* launch
/// surface was read from that project's own manifest and then from the APK that
/// was installed to verify it. An emulator can never be a package name the
/// WebView supplied. What each one actually reads is in
/// `docs/console-emulators.md`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleEmulator {
    /// RetroArch: one app, many consoles, the console chosen by libretro core.
    #[default]
    RetroArch,
    /// PPSSPP: the PSP, and the one emulator here that opens a content URI.
    Ppsspp,
}

impl ConsoleEmulator {
    /// The name the app calls itself, for a sentence the user reads.
    pub fn label(self) -> &'static str {
        match self {
            Self::RetroArch => "RetroArch",
            Self::Ppsspp => "PPSSPP",
        }
    }

    /// A stable token for an identifier the host composes. Separate from the
    /// serde name so renaming one cannot silently move every managed profile.
    pub fn slug(self) -> &'static str {
        match self {
            Self::RetroArch => "retroarch",
            Self::Ppsspp => "ppsspp",
        }
    }

    /// The consoles this emulator runs, in the order a menu lists them.
    pub fn systems(self) -> &'static [ConsoleSystem] {
        match self {
            Self::RetroArch => &[
                ConsoleSystem::Nes,
                ConsoleSystem::Snes,
                ConsoleSystem::GameBoy,
                ConsoleSystem::GameBoyAdvance,
                ConsoleSystem::MegaDrive,
            ],
            Self::Ppsspp => &[ConsoleSystem::PlayStationPortable],
        }
    }

    /// Which of those consoles a file's name belongs to, if any.
    ///
    /// This is an answer and not a guess: no extension in
    /// [`ConsoleSystem::rom_extensions`] is claimed by two of these consoles,
    /// which `no_console_claims_another_console_s_extension` holds to.
    pub fn system_for(self, path: &Path) -> Option<ConsoleSystem> {
        self.systems()
            .iter()
            .copied()
            .find(|system| system.recognises_rom(path))
    }

    /// Does this emulator run that console at all?
    pub fn runs(self, system: ConsoleSystem) -> bool {
        self.systems().contains(&system)
    }

    /// The emulator a WebView token names, or nothing.
    ///
    /// The WebView may name an emulator — it is choosing a menu row, not a
    /// package — and this is the only door that token comes through.
    pub fn from_slug(slug: &str) -> Option<Self> {
        [Self::RetroArch, Self::Ppsspp]
            .into_iter()
            .find(|emulator| emulator.slug() == slug)
    }
}

/// The console a profile's games are for.
///
/// This is not cosmetic: it decides which file extensions are a ROM at all, and
/// for RetroArch which core the intent names. Both come out of closed tables
/// rather than from anything a user or a file can write.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleSystem {
    #[default]
    Nes,
    Snes,
    GameBoy,
    GameBoyAdvance,
    MegaDrive,
    PlayStationPortable,
}

impl ConsoleSystem {
    /// The file extensions a ROM for this console has, lowercase and without a
    /// dot. Deliberately narrow: an archive is not on the list, because Orivo
    /// would then be offering a file whose contents it never looked at, and a
    /// generic extension such as `bin` is not either, because every console
    /// claims it and the folder is shared storage.
    pub fn rom_extensions(self) -> &'static [&'static str] {
        match self {
            Self::Nes => &["nes", "fds", "unf", "unif"],
            Self::Snes => &["smc", "sfc", "swc", "fig"],
            Self::GameBoy => &["gb", "gbc"],
            Self::GameBoyAdvance => &["gba"],
            // `md` is deliberately absent: it is a Mega Drive dump to one person
            // and a README to everyone else, and this list decides what Orivo
            // offers out of a folder on shared storage.
            Self::MegaDrive => &["smd", "gen", "sms", "gg"],
            Self::PlayStationPortable => &["iso", "cso", "chd", "pbp", "elf"],
        }
    }

    /// Is this file name one this console's ROMs use?
    pub fn recognises_rom(self, path: &Path) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                self.rom_extensions()
                    .iter()
                    .any(|known| extension.eq_ignore_ascii_case(known))
            })
    }

    /// A stable token for an identifier the host composes.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Nes => "nes",
            Self::Snes => "snes",
            Self::GameBoy => "gb",
            Self::GameBoyAdvance => "gba",
            Self::MegaDrive => "megadrive",
            Self::PlayStationPortable => "psp",
        }
    }

    /// The name a menu uses for this console.
    pub fn label(self) -> &'static str {
        match self {
            Self::Nes => "NES",
            Self::Snes => "SNES",
            Self::GameBoy => "Game Boy",
            Self::GameBoyAdvance => "Game Boy Advance",
            Self::MegaDrive => "Mega Drive",
            Self::PlayStationPortable => "PSP",
        }
    }
}

/// One console emulator already installed on this device, and the ROM folders
/// the user pointed Orivo at.
///
/// Like a Winlator profile this is a *reference* and not an installation: the
/// emulator owns its cores, its BIOS files and its save states, in storage Orivo
/// can neither read nor validate. What Orivo owns is the granted folder and the
/// decision to send an intent at all.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsoleEmulatorProfile {
    /// Opaque, stable Orivo profile identifier.
    pub id: String,
    /// User-facing label, distinct from any filesystem component.
    pub display_name: String,
    #[serde(default)]
    pub emulator: ConsoleEmulator,
    #[serde(default)]
    pub system: ConsoleSystem,
    /// Directories explicitly granted to this profile, where the ROMs are. No
    /// implicit device-wide fallback, and never a shared drop folder: the runner
    /// refuses those on every use of a grant, not only when one was picked.
    #[serde(default)]
    pub rom_directories: Vec<PathBuf>,
    /// Android storage access grants covering those same directories. Held *in
    /// addition to* the directory, never instead of it — the directory is the
    /// scope every check uses and, for an emulator that takes a path, the thing
    /// the emulator itself opens.
    #[serde(default)]
    pub rom_trees: Vec<String>,
    #[serde(default = "default_console_profile_enabled")]
    pub enabled: bool,
    /// Unix milliseconds of the last completed import, if one has completed.
    #[serde(default)]
    pub last_imported_at: Option<u64>,
}

/// The private ROM inventory behind a console runner game. `game_ref` is the
/// sole value copied into `LaunchTarget::Runner`; `rom_path` never crosses the
/// WebView boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsoleRomInventoryEntry {
    pub profile_id: String,
    pub game_ref: String,
    pub title: String,
    /// The ROM inside a granted directory. Orivo reads enough of it to know it
    /// is still the same file; it never parses it.
    pub rom_path: PathBuf,
    /// A namespaced content digest of the ROM, so a file swapped after the user
    /// confirmed it is refused until a deliberate reimport. What the digest
    /// covers — and what it cannot, for an image too large to hash whole — is the
    /// runner's decision, recorded in `console_runner.rs`.
    pub fingerprint: String,
    #[serde(default)]
    pub imported_at: Option<u64>,
}

/// One folder the user handed to a third-party runner profile through a native
/// picker, and the opaque id the plugin knows it by.
///
/// The plugin never receives `path`. It asks `host-files` for `id`, and the host
/// is the only side that can turn that into a directory. The id is stored rather
/// than derived because the component chooses it: the v1 `runner` world gives a
/// plugin no way to *declare* the directory grants it will ask for, so the host
/// records the slot the folder was granted under and hands back exactly that.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerGrantedDirectory {
    pub id: String,
    pub path: PathBuf,
    /// The folder's own identity when it was granted, so a launch can tell the
    /// folder the user pointed at from another one that has since taken its
    /// name. A canonical path answers "is this still the same place?" only
    /// while nobody renames a directory and builds a new one where it was.
    ///
    /// `None` on a platform that does not publish one; the canonical path is
    /// then the whole of the check, and it still catches a parent replaced by
    /// a link.
    #[serde(default)]
    pub device: Option<u64>,
    #[serde(default)]
    pub inode: Option<u64>,
}

/// Which package a permission was given to.
///
/// A grant belongs to code, not to an identifier. The two halves are used
/// differently on purpose: a release-signed package can be updated and the
/// signature is what carries the consent forward, while a package that arrived
/// unsigned has no such chain, so its own bytes are the only identity it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPackageIdentity {
    pub fingerprint: String,
    pub trusted: bool,
}

/// Whether the plugin that owns a profile has accepted it.
///
/// `Unvalidated` is not a soft `Valid`: a profile only becomes launchable once
/// the component itself answered `validate-profile` with `valid`. A profile the
/// plugin refused is kept, with its reason, so the user can see what to change
/// rather than losing the folders they picked.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunnerProfileStatus {
    #[default]
    Unvalidated,
    Valid,
    Rejected,
}

/// The launch shape a validated profile authorises. It is an enum and not a
/// template because the host builds the process: a plugin that could name the
/// mode as free text would be naming an argument list.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunnerLaunchMode {
    /// The emulation application is started with the resolved game file as its
    /// single argument, in the application's own directory.
    #[default]
    Default,
}

/// The settings half of a profile — everything a `validate-profile` round trip
/// is about. It stays a closed record: a JSON blob here would be the one place
/// a plugin's answer could grow into launch configuration.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunnerProfileSettings {
    #[serde(default)]
    pub launch_mode: RunnerLaunchMode,
}

/// A third-party runner the user configured: which installed plugin prepares
/// its launches, which emulation application the host will start, which folders
/// it may look in, and how far its last import got.
///
/// Like every runner record in this file it is host-private. The emulation
/// application and the granted folders are launch data; only ids, labels and
/// status ever reach a view model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerProfile {
    /// Opaque, stable Orivo profile identifier. It is also what the host passes
    /// to `validate-profile`, `discover-page` and `prepare-launch`.
    pub id: String,
    /// The installed runner plugin that owns this profile. It is the same value
    /// a `LaunchTarget::Runner` carries as its `runner_id`.
    pub plugin_id: String,
    /// User-facing label, and the second half of the WIT `runner-profile`
    /// record the plugin validates.
    pub display_name: String,
    /// The emulation application the user chose through a native picker. The
    /// host canonicalises and rechecks it immediately before a launch; storing
    /// it is not an authorisation to run it.
    pub application: PathBuf,
    #[serde(default)]
    pub game_directories: Vec<RunnerGrantedDirectory>,
    #[serde(default)]
    pub settings: RunnerProfileSettings,
    #[serde(default)]
    pub status: RunnerProfileStatus,
    /// The sentence the plugin gave for refusing this profile, already
    /// bounded and stripped by the host before it was written here.
    #[serde(default)]
    pub status_message: Option<String>,
    /// Disabled profiles and their games stay persisted and visible, but
    /// cannot be launched until the user enables them again.
    #[serde(default = "default_runner_profile_enabled")]
    pub enabled: bool,
    /// Where the last `discover-page` stopped. This is what lets an import
    /// resume after a cancellation or a restart instead of walking a whole
    /// library again; it is the plugin's own opaque cursor, revalidated by the
    /// host before it was stored.
    #[serde(default)]
    pub import_cursor: Option<String>,
    /// Set once the plugin reported a page as the last one. A completed import
    /// starts again from the beginning rather than from a spent cursor.
    #[serde(default)]
    pub import_complete: bool,
    /// Unix milliseconds of the last completed import page, if one has landed.
    #[serde(default)]
    pub last_imported_at: Option<u64>,
    /// The component this profile's verdict was earned against. A package that
    /// changed under the same identifier has not been asked about this profile,
    /// so the status is not about it.
    #[serde(default)]
    pub package_fingerprint: Option<String>,
}

/// The private inventory behind a third-party runner game. `game_ref` is the
/// only value copied into `LaunchTarget::Runner`; `game_path` never crosses the
/// WebView boundary and is never taken from a plugin.
///
/// The host resolved that path itself, from the candidate's external id, inside
/// one named granted folder — which is why the grant it was resolved under is
/// recorded beside it. A launch re-checks both.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerGameInventoryEntry {
    pub profile_id: String,
    pub game_ref: String,
    pub title: String,
    /// The provider half of the plugin's stable external reference. Together
    /// with `external_id` it is what makes a repeated import idempotent.
    pub provider_id: String,
    pub external_id: String,
    /// The game file the host resolved, canonical at import time.
    pub game_path: PathBuf,
    /// Which of the profile's granted folders `game_path` was resolved in.
    pub directory_grant_id: String,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub imported_at: Option<u64>,
}

/// One row of the grant ledger: what a plugin was allowed, for which scope,
/// when, and when it stopped being allowed.
///
/// Revoking writes `revoked_at` instead of deleting the row, so "this plugin
/// could read that folder between these two dates" stays answerable. Nothing
/// else in the catalog depends on a grant: revoking one takes a permission
/// away and leaves the profile and the imported games exactly where they were.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginGrantRecord {
    pub plugin_id: String,
    pub capability: PluginCapability,
    pub scope: CapabilityScope,
    /// Unix milliseconds. A grant with no time is not a grant the host can
    /// audit, so zero is refused.
    pub granted_at: u64,
    #[serde(default)]
    pub revoked_at: Option<u64>,
    /// The component this permission was given to, and whether that package
    /// arrived release-signed. A row that carries neither is a permission over
    /// nothing identifiable, so resolution refuses it rather than guessing.
    #[serde(default)]
    pub package_fingerprint: Option<String>,
    #[serde(default)]
    pub package_trusted: Option<bool>,
}

impl PluginGrantRecord {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }

    /// Whether this row still speaks for the package installed now.
    ///
    /// A release-signed package may be updated under the same signature and
    /// keep what it was allowed; that is what the channel is for. A package
    /// that arrived unsigned has no signer to vouch for a new build, so only
    /// the exact component it was allowed to counts — which is what stops an
    /// uninstall and a hand-loaded replacement from inheriting the folders the
    /// user allowed something else.
    pub fn applies_to(&self, package: &PluginPackageIdentity) -> bool {
        match (self.package_trusted, self.package_fingerprint.as_deref()) {
            (Some(true), Some(_)) => package.trusted,
            (Some(false), Some(fingerprint)) => {
                !package.trusted && fingerprint == package.fingerprint
            }
            _ => false,
        }
    }

    fn identity(&self) -> Option<PluginPackageIdentity> {
        Some(PluginPackageIdentity {
            fingerprint: self.package_fingerprint.clone()?,
            trusted: self.package_trusted?,
        })
    }

    /// The manifest-checkable form of this row. `validate_grant` is what
    /// refuses a persisted grant whose plugin stopped declaring the capability
    /// between one launch and the next, so the conversion is deliberately not
    /// a `From` that could be used without that check.
    pub fn to_capability_grant(&self) -> CapabilityGrant {
        CapabilityGrant {
            plugin_id: self.plugin_id.clone(),
            capability: self.capability,
            scope: self.scope.clone(),
        }
    }
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
    /// A wallpaper the user explicitly chose on the game detail page as the
    /// home (Library) background. It outranks discovered and Steam artwork so a
    /// deliberate choice is never overridden by a store capsule.
    #[serde(default)]
    pub home_image_path: Option<PathBuf>,
    /// A landscape image the user chose for the wide (landscape) card, kept
    /// separate from the background so each role can be set independently.
    #[serde(default)]
    pub landscape_image_path: Option<PathBuf>,
    #[serde(default)]
    pub logo_path: Option<PathBuf>,
    /// Hidden from the library without being forgotten. The record keeps its
    /// artwork, its play time and its launch configuration; it simply stops
    /// being projected. Removing a game is the destructive door, this is not.
    #[serde(default)]
    pub hidden: bool,
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
            winlator_profiles: Vec::new(),
            winlator_inventory: Vec::new(),
            console_profiles: Vec::new(),
            console_inventory: Vec::new(),
            runner_profiles: Vec::new(),
            runner_inventory: Vec::new(),
            plugin_grants: Vec::new(),
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
        let mut rewritten_game_ids = BTreeMap::new();
        let migrated_from = match catalog.schema_version {
            CURRENT_SCHEMA_VERSION => None,
            SCHEMA_VERSION_V7 => {
                migrate_v7_to_v8(&mut catalog);
                Some(SCHEMA_VERSION_V7)
            }
            SCHEMA_VERSION_V6 => {
                rewritten_game_ids = migrate_v6_to_v7(&mut catalog)?;
                migrate_v7_to_v8(&mut catalog);
                Some(SCHEMA_VERSION_V6)
            }
            SCHEMA_VERSION_V5 => {
                migrate_v5_to_v6(&mut catalog);
                rewritten_game_ids = migrate_v6_to_v7(&mut catalog)?;
                migrate_v7_to_v8(&mut catalog);
                Some(SCHEMA_VERSION_V5)
            }
            SCHEMA_VERSION_V4 => {
                migrate_v4_to_v5(&mut catalog);
                migrate_v5_to_v6(&mut catalog);
                rewritten_game_ids = migrate_v6_to_v7(&mut catalog)?;
                migrate_v7_to_v8(&mut catalog);
                Some(SCHEMA_VERSION_V4)
            }
            SCHEMA_VERSION_V3 => {
                migrate_v3_to_v4(&mut catalog);
                migrate_v4_to_v5(&mut catalog);
                migrate_v5_to_v6(&mut catalog);
                rewritten_game_ids = migrate_v6_to_v7(&mut catalog)?;
                migrate_v7_to_v8(&mut catalog);
                Some(SCHEMA_VERSION_V3)
            }
            SCHEMA_VERSION_V2 => {
                migrate_v2_to_v3(&mut catalog);
                migrate_v3_to_v4(&mut catalog);
                migrate_v4_to_v5(&mut catalog);
                migrate_v5_to_v6(&mut catalog);
                rewritten_game_ids = migrate_v6_to_v7(&mut catalog)?;
                migrate_v7_to_v8(&mut catalog);
                Some(SCHEMA_VERSION_V2)
            }
            SCHEMA_VERSION_V1 => {
                migrate_v1_to_v2(&mut catalog);
                migrate_v2_to_v3(&mut catalog);
                migrate_v3_to_v4(&mut catalog);
                migrate_v4_to_v5(&mut catalog);
                migrate_v5_to_v6(&mut catalog);
                rewritten_game_ids = migrate_v6_to_v7(&mut catalog)?;
                migrate_v7_to_v8(&mut catalog);
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
            rewritten_game_ids,
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
            if runner_id == WINLATOR_RUNNER_ID {
                self.validate_winlator_runner_reference(profile_id, game_ref)?;
            }
            if is_console_runner_id(runner_id) {
                self.validate_console_runner_reference(profile_id, game_ref, runner_id)?;
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
            // Deliberate home-background / landscape choices survive a re-sync.
            if game.home_image_path.is_none() {
                game.home_image_path = existing.home_image_path.clone();
            }
            if game.landscape_image_path.is_none() {
                game.landscape_image_path = existing.landscape_image_path.clone();
            }
            // A wordmark the user reset by hand survives a resync, exactly as
            // their chosen cover and landscape do.
            if game.logo_path.is_none() {
                game.logo_path = existing.logo_path.clone();
            }
            game.hidden = existing.hidden;
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

    /// Insert a connected-store record or refresh the provider-owned fields of
    /// the one that already carries the same `(source, source_id)` identity.
    ///
    /// This is the Epic/GOG/Ubisoft/Xbox/Microsoft Store/Instant Gaming
    /// equivalent of `upsert_steam`: re-syncing an account must never duplicate
    /// a rail card, and it must never discard state the user owns — a chosen
    /// wallpaper, a chosen landscape image, or play time Orivo recorded itself
    /// while the provider still reports zero.
    ///
    /// Returns `true` for a newly imported game and `false` for a refresh.
    pub fn upsert_source(&mut self, mut game: Game) -> Result<bool, CatalogError> {
        game.validate()?;
        let source = game.source;
        if source.provider_token().is_none() {
            return Err(CatalogError::Invalid(
                "upsert_source requires a connected-store source record".into(),
            ));
        }
        let source_id = game.source_id.clone().ok_or_else(|| {
            CatalogError::Invalid("connected-source game requires a stable source id".into())
        })?;

        if let Some(index) = self.games.iter().position(|existing| {
            existing.source == source && existing.source_id.as_deref() == Some(source_id.as_str())
        }) {
            let existing = &self.games[index];
            // The provider identity is the stable key, so keep the Orivo card
            // id even if a later connector build derives a different one.
            game.id = existing.id.clone();
            if game.description.is_none() {
                game.description = existing.description.clone();
            }
            for (key, value) in &existing.extra {
                if SOURCE_OWNED_EXTRA_KEYS.contains(&key.as_str()) {
                    continue;
                }
                game.extra
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
            if game.last_played_at.is_none() {
                game.last_played_at = existing.last_played_at.clone();
            }
            if game.play_time_seconds == 0 {
                game.play_time_seconds = existing.play_time_seconds;
            }
            // Deliberate home-background / landscape choices survive a re-sync,
            // exactly as they do for Steam.
            if game.home_image_path.is_none() {
                game.home_image_path = existing.home_image_path.clone();
            }
            if game.landscape_image_path.is_none() {
                game.landscape_image_path = existing.landscape_image_path.clone();
            }
            // A wordmark the user reset by hand survives a resync, exactly as
            // their chosen cover and landscape do.
            if game.logo_path.is_none() {
                game.logo_path = existing.logo_path.clone();
            }
            game.hidden = existing.hidden;
            if game.artwork_path.is_none() {
                game.artwork_path = existing.artwork_path.clone();
            }
            if game.cover_path.is_none() {
                game.cover_path = existing.cover_path.clone();
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

    /// Drop every game imported from one connected store. Disconnecting an
    /// account is a separate, explicit choice from forgetting its library, so
    /// only the command that asks for it calls this.
    pub fn remove_source_games(&mut self, source: GameSource) -> usize {
        let before = self.games.len();
        self.games.retain(|game| game.source != source);
        before - self.games.len()
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
        if runner_id == WINLATOR_RUNNER_ID {
            self.validate_winlator_runner_reference(profile_id, game_ref)?;
        }
        if is_console_runner_id(runner_id) {
            self.validate_console_runner_reference(profile_id, game_ref, runner_id)?;
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

    /// Return a Winlator profile by its opaque host identifier. Callers must
    /// not project this value or any of its paths into a WebView response.
    pub fn winlator_profile(&self, profile_id: &str) -> Option<&WinlatorProfile> {
        self.winlator_profiles
            .iter()
            .find(|profile| profile.id == profile_id)
    }

    /// Return the private inventory entry for a typed Winlator runner
    /// reference.
    pub fn winlator_inventory_entry(
        &self,
        profile_id: &str,
        game_ref: &str,
    ) -> Option<&WinlatorShortcutInventoryEntry> {
        self.winlator_inventory
            .iter()
            .find(|entry| entry.profile_id == profile_id && entry.game_ref == game_ref)
    }

    /// Insert or replace a Winlator profile after structural validation.
    /// Narrowing a profile's grant cannot leave an inventory entry outside it:
    /// the full candidate catalog is validated before it is adopted.
    pub fn upsert_winlator_profile(
        &mut self,
        mut profile: WinlatorProfile,
    ) -> Result<bool, CatalogError> {
        profile.validate()?;
        if let Some(index) = self
            .winlator_profiles
            .iter()
            .position(|existing| existing.id == profile.id)
        {
            if profile.last_imported_at.is_none() {
                profile.last_imported_at = self.winlator_profiles[index].last_imported_at;
            }
            let mut candidate = self.clone();
            candidate.winlator_profiles[index] = profile;
            candidate.validate()?;
            *self = candidate;
            return Ok(false);
        }

        let mut candidate = self.clone();
        candidate.winlator_profiles.push(profile);
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    /// Insert or refresh a private Winlator inventory entry. The entry is
    /// scoped to an existing profile and its shortcut must remain inside one of
    /// that profile's granted directories.
    pub fn upsert_winlator_inventory(
        &mut self,
        mut entry: WinlatorShortcutInventoryEntry,
    ) -> Result<bool, CatalogError> {
        entry.validate()?;
        let profile = self.winlator_profile(&entry.profile_id).ok_or_else(|| {
            CatalogError::Invalid("Winlator inventory entry references an unknown profile".into())
        })?;
        validate_winlator_inventory_scope(&entry, profile)?;

        if let Some(index) = self.winlator_inventory.iter().position(|existing| {
            existing.profile_id == entry.profile_id && existing.game_ref == entry.game_ref
        }) {
            if entry.imported_at.is_none() {
                entry.imported_at = self.winlator_inventory[index].imported_at;
            }
            self.winlator_inventory[index] = entry;
            return Ok(false);
        }

        self.winlator_inventory.push(entry);
        Ok(true)
    }

    /// Return a console emulator profile by its opaque host identifier. Callers
    /// must not project it, or any of its paths, into a WebView response.
    pub fn console_profile(&self, profile_id: &str) -> Option<&ConsoleEmulatorProfile> {
        self.console_profiles
            .iter()
            .find(|profile| profile.id == profile_id)
    }

    /// Return the private inventory entry for a typed console runner reference.
    pub fn console_inventory_entry(
        &self,
        profile_id: &str,
        game_ref: &str,
    ) -> Option<&ConsoleRomInventoryEntry> {
        self.console_inventory
            .iter()
            .find(|entry| entry.profile_id == profile_id && entry.game_ref == game_ref)
    }

    /// Insert or replace a console emulator profile after structural validation.
    /// Narrowing a profile's grant cannot leave an inventory entry outside it:
    /// the full candidate catalog is validated before it is adopted.
    pub fn upsert_console_profile(
        &mut self,
        mut profile: ConsoleEmulatorProfile,
    ) -> Result<bool, CatalogError> {
        profile.validate()?;
        if let Some(index) = self
            .console_profiles
            .iter()
            .position(|existing| existing.id == profile.id)
        {
            if profile.last_imported_at.is_none() {
                profile.last_imported_at = self.console_profiles[index].last_imported_at;
            }
            let mut candidate = self.clone();
            candidate.console_profiles[index] = profile;
            candidate.validate()?;
            *self = candidate;
            return Ok(false);
        }

        let mut candidate = self.clone();
        candidate.console_profiles.push(profile);
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    /// Insert or refresh a private console ROM inventory entry. The entry is
    /// scoped to an existing profile and its ROM must stay inside one of that
    /// profile's granted directories.
    pub fn upsert_console_inventory(
        &mut self,
        mut entry: ConsoleRomInventoryEntry,
    ) -> Result<bool, CatalogError> {
        entry.validate()?;
        let profile = self.console_profile(&entry.profile_id).ok_or_else(|| {
            CatalogError::Invalid("console inventory entry references an unknown profile".into())
        })?;
        validate_console_inventory_scope(&entry, profile)?;

        if let Some(index) = self.console_inventory.iter().position(|existing| {
            existing.profile_id == entry.profile_id && existing.game_ref == entry.game_ref
        }) {
            if entry.imported_at.is_none() {
                entry.imported_at = self.console_inventory[index].imported_at;
            }
            self.console_inventory[index] = entry;
            return Ok(false);
        }

        self.console_inventory.push(entry);
        Ok(true)
    }

    /// Return a third-party runner profile by its opaque host identifier.
    /// Callers must not project this value or any of its paths into a WebView
    /// response.
    pub fn runner_profile(&self, profile_id: &str) -> Option<&RunnerProfile> {
        self.runner_profiles
            .iter()
            .find(|profile| profile.id == profile_id)
    }

    pub fn runner_profiles_for_plugin(&self, plugin_id: &str) -> Vec<&RunnerProfile> {
        self.runner_profiles
            .iter()
            .filter(|profile| profile.plugin_id == plugin_id)
            .collect()
    }

    /// Return the private inventory entry for a typed third-party runner
    /// reference.
    pub fn runner_inventory_entry(
        &self,
        profile_id: &str,
        game_ref: &str,
    ) -> Option<&RunnerGameInventoryEntry> {
        self.runner_inventory
            .iter()
            .find(|entry| entry.profile_id == profile_id && entry.game_ref == game_ref)
    }

    /// The entry a candidate's external reference already maps to, if any.
    /// This is the lookup that makes a repeated import a refresh rather than a
    /// second card: the plugin's title may change, its reference may not.
    pub fn runner_inventory_by_external_ref(
        &self,
        profile_id: &str,
        provider_id: &str,
        external_id: &str,
    ) -> Option<&RunnerGameInventoryEntry> {
        self.runner_inventory.iter().find(|entry| {
            entry.profile_id == profile_id
                && entry.provider_id == provider_id
                && entry.external_id == external_id
        })
    }

    /// Insert or replace a third-party runner profile after structural
    /// validation. Narrowing a profile's granted folders cannot strand an
    /// inventory entry outside them: the whole candidate catalog is validated
    /// before it is adopted.
    pub fn upsert_runner_profile(
        &mut self,
        mut profile: RunnerProfile,
    ) -> Result<bool, CatalogError> {
        profile.validate()?;
        if let Some(index) = self
            .runner_profiles
            .iter()
            .position(|existing| existing.id == profile.id)
        {
            let existing = &self.runner_profiles[index];
            if existing.plugin_id != profile.plugin_id {
                return Err(CatalogError::Invalid(
                    "a runner profile cannot change owner plugin".into(),
                ));
            }
            if profile.last_imported_at.is_none() {
                profile.last_imported_at = existing.last_imported_at;
            }
            let mut candidate = self.clone();
            candidate.runner_profiles[index] = profile;
            candidate.validate()?;
            *self = candidate;
            return Ok(false);
        }

        let mut candidate = self.clone();
        candidate.runner_profiles.push(profile);
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    /// Insert or refresh a private third-party inventory entry. The entry is
    /// scoped to an existing profile and its file must remain inside the
    /// granted folder it names.
    pub fn upsert_runner_inventory(
        &mut self,
        mut entry: RunnerGameInventoryEntry,
    ) -> Result<bool, CatalogError> {
        entry.validate()?;
        let profile = self.runner_profile(&entry.profile_id).ok_or_else(|| {
            CatalogError::Invalid("runner inventory entry references an unknown profile".into())
        })?;
        validate_runner_inventory_scope(&entry, profile)?;
        // An external reference belongs to exactly one game reference. Letting
        // a second one claim it is how a re-import would quietly split a game
        // in two, so it is refused here rather than at whole-catalog validation.
        if let Some(existing) = self.runner_inventory_by_external_ref(
            &entry.profile_id,
            &entry.provider_id,
            &entry.external_id,
        ) && existing.game_ref != entry.game_ref
        {
            return Err(CatalogError::Invalid(
                "runner external reference already belongs to another game reference".into(),
            ));
        }

        if let Some(index) = self.runner_inventory.iter().position(|existing| {
            existing.profile_id == entry.profile_id && existing.game_ref == entry.game_ref
        }) {
            if entry.imported_at.is_none() {
                entry.imported_at = self.runner_inventory[index].imported_at;
            }
            self.runner_inventory[index] = entry;
            return Ok(false);
        }

        self.runner_inventory.push(entry);
        Ok(true)
    }

    /// Remove a third-party runner profile, its private inventory and the
    /// cards that depend on it, atomically.
    ///
    /// Grants are deliberately left behind as revoked rows rather than
    /// deleted: the ledger is what makes "this plugin could read that folder"
    /// answerable afterwards, and a deleted row answers nothing.
    pub fn remove_runner_profile(
        &mut self,
        profile_id: &str,
        revoked_at: u64,
    ) -> Result<bool, CatalogError> {
        validate_opaque_runner_token("profile id", profile_id, MAX_PROFILE_ID_LENGTH)?;
        let Some(profile) = self.runner_profile(profile_id).cloned() else {
            return Ok(false);
        };

        let mut candidate = self.clone();
        candidate
            .runner_profiles
            .retain(|entry| entry.id != profile_id);
        candidate
            .runner_inventory
            .retain(|entry| entry.profile_id != profile_id);
        candidate.games.retain(|game| {
            !matches!(
                &game.launch_target,
                LaunchTarget::Runner {
                    runner_id,
                    profile_id: target_profile_id,
                    ..
                } if runner_id == &profile.plugin_id && target_profile_id == profile_id
            )
        });
        // What this profile authorised goes with it, and nothing else does:
        // every value here is keyed to this profile, so another profile of the
        // same plugin keeps exactly what it was allowed.
        for directory in &profile.game_directories {
            candidate.revoke_plugin_scope_value(
                &profile.plugin_id,
                PluginCapability::FilesRead,
                &directory_grant_key(profile_id, &directory.id),
                revoked_at,
            )?;
        }
        candidate.revoke_plugin_scope_value(
            &profile.plugin_id,
            PluginCapability::RunnerPrepare,
            profile_id,
            revoked_at,
        )?;
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    /// The grant row currently in force for one plugin capability, if any.
    pub fn active_plugin_grant(
        &self,
        plugin_id: &str,
        capability: PluginCapability,
    ) -> Option<&PluginGrantRecord> {
        self.plugin_grants.iter().find(|grant| {
            grant.plugin_id == plugin_id && grant.capability == capability && grant.is_active()
        })
    }

    /// Put one capability in force with a new scope, retiring whatever was in
    /// force before it. Granting is therefore always a complete statement of
    /// what the plugin may reach, and the row it replaced keeps its dates.
    pub fn grant_plugin_capability(
        &mut self,
        record: PluginGrantRecord,
    ) -> Result<(), CatalogError> {
        record.validate()?;
        let mut candidate = self.clone();
        for grant in &mut candidate.plugin_grants {
            if grant.plugin_id == record.plugin_id
                && grant.capability == record.capability
                && grant.is_active()
            {
                grant.revoked_at = Some(record.granted_at.max(grant.granted_at));
            }
        }
        candidate.plugin_grants.push(record);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    /// Take one capability out of force. Nothing else moves: the profile, its
    /// granted folders and every game already imported stay exactly as they
    /// are, and only the plugin's permission to use them stops.
    pub fn revoke_plugin_capability(
        &mut self,
        plugin_id: &str,
        capability: PluginCapability,
        revoked_at: u64,
    ) -> Result<bool, CatalogError> {
        validate_opaque_runner_token("plugin id", plugin_id, MAX_RUNNER_ID_LENGTH)?;
        let mut candidate = self.clone();
        let mut revoked = false;
        for grant in &mut candidate.plugin_grants {
            if grant.plugin_id == plugin_id && grant.capability == capability && grant.is_active() {
                grant.revoked_at = Some(revoked_at.max(grant.granted_at));
                revoked = true;
            }
        }
        if !revoked {
            return Ok(false);
        }
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    /// Add one value to what a capability allows, and record which package it
    /// was allowed to.
    ///
    /// Granting is additive and explicit: only the folder or profile the user
    /// just consented to moves. Nothing restates a scope from what the catalog
    /// happens to hold, because a restatement is how a permission the user took
    /// away comes back the next time they rename something. A value allowed to
    /// a package this is no longer starts the scope over rather than joining it.
    pub fn allow_plugin_scope_value(
        &mut self,
        plugin_id: &str,
        capability: PluginCapability,
        value: &str,
        package: &PluginPackageIdentity,
        granted_at: u64,
    ) -> Result<(), CatalogError> {
        let active = self.active_plugin_grant(plugin_id, capability);
        let carried = active
            .filter(|grant| grant.applies_to(package))
            .map(|grant| (grant.scope.clone(), grant.granted_at));
        let mut values = match carried.as_ref().map(|(scope, _)| scope) {
            Some(CapabilityScope::DirectoryGrants(ids) | CapabilityScope::RunnerProfiles(ids)) => {
                ids.clone()
            }
            _ => BTreeSet::new(),
        };
        values.insert(value.to_owned());
        let scope = scope_for(capability, values)?;
        self.grant_plugin_capability(PluginGrantRecord {
            plugin_id: plugin_id.to_owned(),
            capability,
            scope,
            granted_at: carried.map_or(granted_at, |(_, at)| granted_at.max(at)),
            revoked_at: None,
            package_fingerprint: Some(package.fingerprint.clone()),
            package_trusted: Some(package.trusted),
        })
    }

    /// Take one value out of what a capability allows. The row that was in
    /// force is retired and a narrower one takes its place, so the ledger reads
    /// as the history it is; a scope emptied this way is revoked outright.
    pub fn revoke_plugin_scope_value(
        &mut self,
        plugin_id: &str,
        capability: PluginCapability,
        value: &str,
        revoked_at: u64,
    ) -> Result<bool, CatalogError> {
        let Some(active) = self.active_plugin_grant(plugin_id, capability) else {
            return Ok(false);
        };
        let (mut values, identity) = match (&active.scope, active.identity()) {
            (
                CapabilityScope::DirectoryGrants(ids) | CapabilityScope::RunnerProfiles(ids),
                identity,
            ) => (ids.clone(), identity),
            _ => return Ok(false),
        };
        if !values.remove(value) {
            return Ok(false);
        }
        if values.is_empty() {
            return self.revoke_plugin_capability(plugin_id, capability, revoked_at);
        }
        let Some(identity) = identity else {
            // A row with no package behind it resolves to nothing anyway, so
            // there is no narrower version of it worth writing.
            return self.revoke_plugin_capability(plugin_id, capability, revoked_at);
        };
        let scope = scope_for(capability, values)?;
        self.grant_plugin_capability(PluginGrantRecord {
            plugin_id: plugin_id.to_owned(),
            capability,
            scope,
            granted_at: revoked_at,
            revoked_at: None,
            package_fingerprint: Some(identity.fingerprint),
            package_trusted: Some(identity.trusted),
        })?;
        Ok(true)
    }

    /// Withdraw one granted folder without touching anything it produced. The
    /// folder stays recorded on the profile — the games inside it are still the
    /// user's — and only the permission to reach it is taken away.
    pub fn revoke_runner_directory(
        &mut self,
        profile_id: &str,
        directory_id: &str,
        revoked_at: u64,
    ) -> Result<bool, CatalogError> {
        let Some(profile) = self.runner_profile(profile_id).cloned() else {
            return Ok(false);
        };
        if profile.granted_directory(directory_id).is_none() {
            return Ok(false);
        }
        self.revoke_plugin_scope_value(
            &profile.plugin_id,
            PluginCapability::FilesRead,
            &directory_grant_key(profile_id, directory_id),
            revoked_at,
        )
    }

    /// Everything one plugin was allowed, taken back at once. Used when a
    /// package leaves: its profiles and the games they imported stay, and only
    /// the permissions go.
    #[allow(dead_code)]
    pub fn revoke_plugin_grants(
        &mut self,
        plugin_id: &str,
        revoked_at: u64,
    ) -> Result<bool, CatalogError> {
        let capabilities = self
            .plugin_grants
            .iter()
            .filter(|grant| grant.plugin_id == plugin_id && grant.is_active())
            .map(|grant| grant.capability)
            .collect::<BTreeSet<_>>();
        let mut revoked = false;
        for capability in capabilities {
            revoked |= self.revoke_plugin_capability(plugin_id, capability, revoked_at)?;
        }
        Ok(revoked)
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

    /// Remove a game from the library by its opaque id. Returns whether a game
    /// was actually removed. The game's own files on disk are never touched;
    /// this only drops the catalog record.
    pub fn remove(&mut self, game_id: &str) -> Result<bool, CatalogError> {
        let before = self.games.len();
        self.games.retain(|game| game.id != game_id);
        Ok(self.games.len() != before)
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

        let mut winlator_profiles = BTreeMap::new();
        for profile in &self.winlator_profiles {
            profile.validate()?;
            if winlator_profiles
                .insert(profile.id.as_str(), profile)
                .is_some()
            {
                return Err(CatalogError::Invalid(
                    "duplicate Winlator profile id".into(),
                ));
            }
        }

        let mut winlator_inventory = BTreeSet::new();
        for entry in &self.winlator_inventory {
            entry.validate()?;
            let profile = winlator_profiles
                .get(entry.profile_id.as_str())
                .ok_or_else(|| {
                    CatalogError::Invalid(
                        "Winlator inventory entry references an unknown profile".into(),
                    )
                })?;
            validate_winlator_inventory_scope(entry, profile)?;
            if !winlator_inventory.insert((entry.profile_id.as_str(), entry.game_ref.as_str())) {
                return Err(CatalogError::Invalid(
                    "duplicate Winlator inventory game reference for profile".into(),
                ));
            }
        }

        let mut console_profiles = BTreeMap::new();
        for profile in &self.console_profiles {
            profile.validate()?;
            if console_profiles
                .insert(profile.id.as_str(), profile)
                .is_some()
            {
                return Err(CatalogError::Invalid(
                    "duplicate console emulator profile id".into(),
                ));
            }
        }

        let mut console_inventory = BTreeSet::new();
        for entry in &self.console_inventory {
            entry.validate()?;
            let profile = console_profiles
                .get(entry.profile_id.as_str())
                .ok_or_else(|| {
                    CatalogError::Invalid(
                        "console inventory entry references an unknown profile".into(),
                    )
                })?;
            validate_console_inventory_scope(entry, profile)?;
            if !console_inventory.insert((entry.profile_id.as_str(), entry.game_ref.as_str())) {
                return Err(CatalogError::Invalid(
                    "duplicate console inventory game reference for profile".into(),
                ));
            }
        }

        let mut runner_profiles = BTreeMap::new();
        for profile in &self.runner_profiles {
            profile.validate()?;
            if runner_profiles
                .insert(profile.id.as_str(), profile)
                .is_some()
            {
                return Err(CatalogError::Invalid("duplicate runner profile id".into()));
            }
        }

        let mut runner_inventory = BTreeSet::new();
        let mut runner_external_refs = BTreeSet::new();
        for entry in &self.runner_inventory {
            entry.validate()?;
            let profile = runner_profiles
                .get(entry.profile_id.as_str())
                .ok_or_else(|| {
                    CatalogError::Invalid(
                        "runner inventory entry references an unknown profile".into(),
                    )
                })?;
            validate_runner_inventory_scope(entry, profile)?;
            if !runner_inventory.insert((entry.profile_id.as_str(), entry.game_ref.as_str())) {
                return Err(CatalogError::Invalid(
                    "duplicate runner inventory game reference for profile".into(),
                ));
            }
            // The external reference is what makes a re-import idempotent, so
            // two entries claiming the same one would make it ambiguous.
            if !runner_external_refs.insert((
                entry.profile_id.as_str(),
                entry.provider_id.as_str(),
                entry.external_id.as_str(),
            )) {
                return Err(CatalogError::Invalid(
                    "duplicate runner external reference for profile".into(),
                ));
            }
        }

        // Grant rows are checked for shape only. A grant deliberately outlives
        // the profile or the plugin it names — that is what makes "this plugin
        // could read that folder until this date" answerable after an
        // uninstall — so a dangling scope id is resolved away at invocation
        // time rather than making the whole library unreadable here.
        let mut active_grants = BTreeSet::new();
        for grant in &self.plugin_grants {
            grant.validate()?;
            if grant.is_active()
                && !active_grants.insert((grant.plugin_id.as_str(), grant.capability))
            {
                return Err(CatalogError::Invalid(
                    "a plugin capability cannot be granted twice at once".into(),
                ));
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
                if runner_id == WINLATOR_RUNNER_ID {
                    let profile = winlator_profiles.get(profile_id).ok_or_else(|| {
                        CatalogError::Invalid("Winlator game references an unknown profile".into())
                    })?;
                    if !winlator_inventory.contains(&(profile_id, game_ref)) {
                        return Err(CatalogError::Invalid(
                            "Winlator game is missing its private inventory entry".into(),
                        ));
                    }
                    profile.validate()?;
                }
                if is_console_runner_id(runner_id) {
                    let profile = console_profiles.get(profile_id).ok_or_else(|| {
                        CatalogError::Invalid("console game references an unknown profile".into())
                    })?;
                    // A card that named the other emulator's runner would be a
                    // PSP image handed to RetroArch, or the reverse.
                    if profile.runner_id() != runner_id {
                        return Err(CatalogError::Invalid(
                            "console game names a profile that belongs to another emulator".into(),
                        ));
                    }
                    if !console_inventory.contains(&(profile_id, game_ref)) {
                        return Err(CatalogError::Invalid(
                            "console game is missing its private inventory entry".into(),
                        ));
                    }
                    profile.validate()?;
                }
                // A third-party runner card whose profile is gone is kept, not
                // refused: uninstalling a plugin or deleting its profile must
                // not cost the user the games it imported, and an orphan simply
                // reports why it cannot start. What is refused is a card
                // pointing at a profile that exists and disagrees with it.
                if runner_id != WINE_STAGING_RUNNER_ID
                    && runner_id != WINLATOR_RUNNER_ID
                    && !is_console_runner_id(runner_id)
                    && let Some(profile) = runner_profiles.get(profile_id)
                {
                    if profile.plugin_id != runner_id {
                        return Err(CatalogError::Invalid(
                            "runner game names a profile that belongs to another plugin".into(),
                        ));
                    }
                    if !runner_inventory.contains(&(profile_id, game_ref)) {
                        return Err(CatalogError::Invalid(
                            "runner game is missing its private inventory entry".into(),
                        ));
                    }
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

    fn validate_console_runner_reference(
        &self,
        profile_id: &str,
        game_ref: &str,
        runner_id: &str,
    ) -> Result<(), CatalogError> {
        let profile = self.console_profile(profile_id).ok_or_else(|| {
            CatalogError::Invalid("console game references an unknown profile".into())
        })?;
        // A profile belongs to one emulator, and so does a runner id. A card that
        // named the other one would be a PSP ISO handed to RetroArch.
        if profile.runner_id() != runner_id {
            return Err(CatalogError::Invalid(
                "console game names a profile that belongs to another emulator".into(),
            ));
        }
        if self.console_inventory_entry(profile_id, game_ref).is_none() {
            return Err(CatalogError::Invalid(
                "console game is missing its private inventory entry".into(),
            ));
        }
        profile.validate()
    }

    fn validate_winlator_runner_reference(
        &self,
        profile_id: &str,
        game_ref: &str,
    ) -> Result<(), CatalogError> {
        let profile = self.winlator_profile(profile_id).ok_or_else(|| {
            CatalogError::Invalid("Winlator game references an unknown profile".into())
        })?;
        if self
            .winlator_inventory_entry(profile_id, game_ref)
            .is_none()
        {
            return Err(CatalogError::Invalid(
                "Winlator game is missing its private inventory entry".into(),
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

const MAX_WINLATOR_PROFILE_NAME_LENGTH: usize = 120;
/// A profile points at the folder Winlator exports into. A handful of grants is
/// already more than Winlator's own exporter can produce.
const MAX_WINLATOR_SHORTCUT_TREES: usize = 8;
const MAX_WINLATOR_SHORTCUT_TREE_LENGTH: usize = 2_048;
const MAX_WINLATOR_GAME_TITLE_LENGTH: usize = 512;
const MAX_WINLATOR_FINGERPRINT_LENGTH: usize = 256;
/// Winlator numbers containers from 1 upwards as the user creates them. The
/// ceiling is not Winlator's — it is Orivo refusing to persist an integer wide
/// enough to be something other than a container number.
const MAX_WINLATOR_CONTAINER_ID: u32 = 9_999;

impl WinlatorProfile {
    /// Validate only the stable on-disk shape of a profile. There is no engine
    /// binary and no prefix to check here, and deliberately no attempt to
    /// verify the container: it lives in Winlator's private storage. The host
    /// rechecks the granted directories and the shortcut file immediately
    /// before an import or a launch.
    pub fn validate(&self) -> Result<(), CatalogError> {
        validate_opaque_runner_token("profile id", &self.id, MAX_PROFILE_ID_LENGTH)?;
        validate_display_text(
            "Winlator profile name",
            &self.display_name,
            MAX_WINLATOR_PROFILE_NAME_LENGTH,
        )?;
        validate_winlator_container_id(self.container_id)?;
        if self.shortcut_directories.is_empty() {
            return Err(CatalogError::Invalid(
                "Winlator profile needs at least one granted shortcut directory".into(),
            ));
        }
        let mut shortcut_directories = BTreeSet::new();
        for directory in &self.shortcut_directories {
            validate_private_absolute_path("Winlator shortcut directory", directory)?;
            if !shortcut_directories.insert(directory) {
                return Err(CatalogError::Invalid(
                    "Winlator profile has duplicate granted shortcut directories".into(),
                ));
            }
        }
        // Only the durable shape of a storage access grant is checked here. What
        // a tree URI may actually name — which provider, which volume, and the
        // folder it resolves to — is the runner's to decide, against the device.
        if self.shortcut_trees.len() > MAX_WINLATOR_SHORTCUT_TREES {
            return Err(CatalogError::Invalid(
                "Winlator profile has too many granted shortcut folders".into(),
            ));
        }
        let mut shortcut_trees = BTreeSet::new();
        for tree in &self.shortcut_trees {
            if tree.len() > MAX_WINLATOR_SHORTCUT_TREE_LENGTH
                || !tree.starts_with("content://")
                || tree.chars().any(|character| {
                    character.is_control() || character.is_whitespace() || !character.is_ascii()
                })
            {
                return Err(CatalogError::Invalid(
                    "Winlator granted shortcut folder must be a content URI".into(),
                ));
            }
            if !shortcut_trees.insert(tree) {
                return Err(CatalogError::Invalid(
                    "Winlator profile has duplicate granted shortcut folders".into(),
                ));
            }
        }
        Ok(())
    }
}

impl WinlatorShortcutInventoryEntry {
    /// The path here is private host data. This verifies its durable shape;
    /// the host canonicalises it and rechecks it against a live grant before it
    /// reads the shortcut or sends an intent.
    pub fn validate(&self) -> Result<(), CatalogError> {
        validate_opaque_runner_token("profile id", &self.profile_id, MAX_PROFILE_ID_LENGTH)?;
        validate_opaque_runner_token("game reference", &self.game_ref, MAX_GAME_REF_LENGTH)?;
        validate_display_text(
            "Winlator game title",
            &self.title,
            MAX_WINLATOR_GAME_TITLE_LENGTH,
        )?;
        validate_private_absolute_path("Winlator shortcut", &self.shortcut_path)?;
        if !self
            .shortcut_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("desktop"))
        {
            return Err(CatalogError::Invalid(
                "Winlator inventory shortcut must be a .desktop file".into(),
            ));
        }
        validate_opaque_runner_token(
            "Winlator game fingerprint",
            &self.fingerprint,
            MAX_WINLATOR_FINGERPRINT_LENGTH,
        )?;
        validate_winlator_container_id(self.container_id)
    }
}

fn validate_winlator_container_id(container_id: Option<u32>) -> Result<(), CatalogError> {
    match container_id {
        Some(id) if id == 0 || id > MAX_WINLATOR_CONTAINER_ID => Err(CatalogError::Invalid(
            format!("Winlator container id must be between 1 and {MAX_WINLATOR_CONTAINER_ID}"),
        )),
        Some(_) | None => Ok(()),
    }
}

impl ConsoleEmulatorProfile {
    /// Validate only the stable on-disk shape. There is no engine binary, no
    /// core and no BIOS to check here: all three live in the emulator's own
    /// storage. The runner rechecks the granted folders and the ROM immediately
    /// before an import or a launch.
    pub fn validate(&self) -> Result<(), CatalogError> {
        validate_opaque_runner_token("profile id", &self.id, MAX_PROFILE_ID_LENGTH)?;
        validate_display_text(
            "console emulator profile name",
            &self.display_name,
            MAX_CONSOLE_PROFILE_NAME_LENGTH,
        )?;
        // An emulator and a console that disagree would make the runner choose
        // one of them, and either choice would be a guess about what the user
        // meant. PPSSPP is a PSP emulator and nothing else; RetroArch covers the
        // cartridge-era consoles through its cores and Orivo names no libretro
        // PSP core.
        if !self.emulator.runs(self.system) {
            return Err(CatalogError::Invalid(
                "console emulator profile names a console that emulator does not run".into(),
            ));
        }
        if self.rom_directories.is_empty() {
            return Err(CatalogError::Invalid(
                "console emulator profile needs at least one granted ROM directory".into(),
            ));
        }
        let mut rom_directories = BTreeSet::new();
        for directory in &self.rom_directories {
            validate_private_absolute_path("console ROM directory", directory)?;
            if !rom_directories.insert(directory) {
                return Err(CatalogError::Invalid(
                    "console emulator profile has duplicate granted ROM directories".into(),
                ));
            }
        }
        // Only the durable shape of a storage access grant is checked here. What
        // a tree URI may actually name — which provider, which volume, and the
        // folder it resolves to — is the runner's to decide, against the device.
        if self.rom_trees.len() > MAX_CONSOLE_ROM_TREES {
            return Err(CatalogError::Invalid(
                "console emulator profile has too many granted ROM folders".into(),
            ));
        }
        let mut rom_trees = BTreeSet::new();
        for tree in &self.rom_trees {
            if tree.len() > MAX_CONSOLE_ROM_TREE_LENGTH
                || !tree.starts_with("content://")
                || tree.chars().any(|character| {
                    character.is_control() || character.is_whitespace() || !character.is_ascii()
                })
            {
                return Err(CatalogError::Invalid(
                    "console granted ROM folder must be a content URI".into(),
                ));
            }
            if !rom_trees.insert(tree) {
                return Err(CatalogError::Invalid(
                    "console emulator profile has duplicate granted ROM folders".into(),
                ));
            }
        }
        Ok(())
    }

    /// The runner that starts this profile's games.
    pub fn runner_id(&self) -> &'static str {
        match self.emulator {
            ConsoleEmulator::RetroArch => RETROARCH_RUNNER_ID,
            ConsoleEmulator::Ppsspp => PPSSPP_RUNNER_ID,
        }
    }
}

impl ConsoleRomInventoryEntry {
    /// The path here is private host data. This verifies its durable shape; the
    /// runner canonicalises it, rechecks it against a live grant and re-reads the
    /// file before it hands anything to an emulator.
    pub fn validate(&self) -> Result<(), CatalogError> {
        validate_opaque_runner_token("profile id", &self.profile_id, MAX_PROFILE_ID_LENGTH)?;
        validate_opaque_runner_token("game reference", &self.game_ref, MAX_GAME_REF_LENGTH)?;
        validate_display_text(
            "console game title",
            &self.title,
            MAX_CONSOLE_GAME_TITLE_LENGTH,
        )?;
        validate_private_absolute_path("console ROM", &self.rom_path)?;
        validate_opaque_runner_token(
            "console game fingerprint",
            &self.fingerprint,
            MAX_CONSOLE_FINGERPRINT_LENGTH,
        )
    }
}

/// A ROM has to be inside one of its profile's granted folders *and* be a file
/// that profile's console recognises. The second half is checked here as well as
/// in the runner, because a catalog written by hand is the one path that does not
/// go through a scan.
fn validate_console_inventory_scope(
    entry: &ConsoleRomInventoryEntry,
    profile: &ConsoleEmulatorProfile,
) -> Result<(), CatalogError> {
    if !profile.system.recognises_rom(&entry.rom_path) {
        return Err(CatalogError::Invalid(
            "console ROM is not a file this console uses".into(),
        ));
    }
    if profile.rom_directories.iter().any(|directory| {
        entry.rom_path.as_path() != directory.as_path() && entry.rom_path.starts_with(directory)
    }) {
        Ok(())
    } else {
        Err(CatalogError::Invalid(
            "console ROM is outside the profile's granted directories".into(),
        ))
    }
}

const MAX_CONSOLE_PROFILE_NAME_LENGTH: usize = 120;
const MAX_CONSOLE_ROM_TREES: usize = 8;
const MAX_CONSOLE_ROM_TREE_LENGTH: usize = 2_048;
const MAX_CONSOLE_GAME_TITLE_LENGTH: usize = 512;
const MAX_CONSOLE_FINGERPRINT_LENGTH: usize = 256;

fn validate_winlator_inventory_scope(
    entry: &WinlatorShortcutInventoryEntry,
    profile: &WinlatorProfile,
) -> Result<(), CatalogError> {
    if profile.shortcut_directories.iter().any(|directory| {
        entry.shortcut_path.as_path() != directory.as_path()
            && entry.shortcut_path.starts_with(directory)
    }) {
        Ok(())
    } else {
        Err(CatalogError::Invalid(
            "Winlator shortcut is outside the profile's granted directories".into(),
        ))
    }
}

const MAX_RUNNER_PROFILE_NAME_LENGTH: usize = 120;
const MAX_RUNNER_GAME_TITLE_LENGTH: usize = 512;
const MAX_RUNNER_PLATFORM_LENGTH: usize = 64;
const MAX_RUNNER_STATUS_MESSAGE_LENGTH: usize = 512;
/// The `host-files` grant-id and cursor grammars the plugin host already
/// applies to a component's answers. Persisting anything looser would let the
/// catalog be the weak side of a boundary the host checks twice.
const MAX_DIRECTORY_GRANT_ID_LENGTH: usize = 256;
const MAX_RUNNER_EXTERNAL_ID_LENGTH: usize = 256;
const MAX_RUNNER_CURSOR_LENGTH: usize = 512;
/// More folders than any emulator library needs, and few enough that a
/// per-invocation grant resolution stays a small map.
const MAX_RUNNER_GRANTED_DIRECTORIES: usize = 32;
/// A scope naming more ids than this is not a scope. It is the same bound on
/// every scope kind because the point is the size, not the meaning.
const MAX_GRANT_SCOPE_VALUES: usize = 64;

impl RunnerProfile {
    /// Validate only the durable shape of a profile. Whether the application
    /// still exists, whether a granted folder is still readable and whether the
    /// owning plugin is still installed are all live questions: the host asks
    /// them again immediately before an import or a launch, because a catalog
    /// that refused to load over an unplugged drive would take the whole
    /// library with it.
    pub fn validate(&self) -> Result<(), CatalogError> {
        validate_opaque_runner_token("profile id", &self.id, MAX_PROFILE_ID_LENGTH)?;
        validate_opaque_runner_token("plugin id", &self.plugin_id, MAX_RUNNER_ID_LENGTH)?;
        validate_display_text(
            "runner profile name",
            &self.display_name,
            MAX_RUNNER_PROFILE_NAME_LENGTH,
        )?;
        validate_private_absolute_path("runner application", &self.application)?;
        if self.game_directories.len() > MAX_RUNNER_GRANTED_DIRECTORIES {
            return Err(CatalogError::Invalid(
                "runner profile has more granted folders than Orivo will hold".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for directory in &self.game_directories {
            validate_opaque_runner_token(
                "directory grant id",
                &directory.id,
                MAX_DIRECTORY_GRANT_ID_LENGTH,
            )?;
            validate_private_absolute_path("runner game directory", &directory.path)?;
            if !ids.insert(directory.id.as_str()) {
                return Err(CatalogError::Invalid(
                    "runner profile has duplicate directory grant ids".into(),
                ));
            }
            if !paths.insert(directory.path.as_path()) {
                return Err(CatalogError::Invalid(
                    "runner profile has duplicate granted folders".into(),
                ));
            }
        }
        if let Some(message) = self.status_message.as_deref() {
            validate_display_text(
                "runner profile status message",
                message,
                MAX_RUNNER_STATUS_MESSAGE_LENGTH,
            )?;
        }
        if let Some(cursor) = self.import_cursor.as_deref() {
            validate_opaque_runner_token("import cursor", cursor, MAX_RUNNER_CURSOR_LENGTH)?;
        }
        if let Some(fingerprint) = self.package_fingerprint.as_deref()
            && (fingerprint.len() != 64
                || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(CatalogError::Invalid(
                "runner profile package fingerprint must be a SHA-256 digest".into(),
            ));
        }
        Ok(())
    }

    pub fn granted_directory(&self, id: &str) -> Option<&RunnerGrantedDirectory> {
        self.game_directories
            .iter()
            .find(|directory| directory.id == id)
    }
}

impl RunnerGameInventoryEntry {
    pub fn validate(&self) -> Result<(), CatalogError> {
        validate_opaque_runner_token("profile id", &self.profile_id, MAX_PROFILE_ID_LENGTH)?;
        validate_opaque_runner_token("game reference", &self.game_ref, MAX_GAME_REF_LENGTH)?;
        validate_opaque_runner_token(
            "external provider id",
            &self.provider_id,
            MAX_RUNNER_ID_LENGTH,
        )?;
        validate_opaque_runner_token(
            "external id",
            &self.external_id,
            MAX_RUNNER_EXTERNAL_ID_LENGTH,
        )?;
        validate_opaque_runner_token(
            "directory grant id",
            &self.directory_grant_id,
            MAX_DIRECTORY_GRANT_ID_LENGTH,
        )?;
        validate_display_text(
            "runner game title",
            &self.title,
            MAX_RUNNER_GAME_TITLE_LENGTH,
        )?;
        validate_private_absolute_path("runner game file", &self.game_path)?;
        if let Some(platform) = self.platform.as_deref() {
            validate_display_text("runner game platform", platform, MAX_RUNNER_PLATFORM_LENGTH)?;
        }
        Ok(())
    }
}

impl PluginGrantRecord {
    pub fn validate(&self) -> Result<(), CatalogError> {
        validate_opaque_runner_token("plugin id", &self.plugin_id, MAX_RUNNER_ID_LENGTH)?;
        if self.granted_at == 0 {
            return Err(CatalogError::Invalid(
                "a plugin grant must record when it was granted".into(),
            ));
        }
        if self.revoked_at.is_some_and(|at| at < self.granted_at) {
            return Err(CatalogError::Invalid(
                "a plugin grant cannot be revoked before it was granted".into(),
            ));
        }
        // A retired row is history and may predate this rule; a row still in
        // force has to say which package it belongs to, or it is a permission
        // that would follow an identifier rather than the code behind it.
        if self.is_active() {
            let fingerprint = self.package_fingerprint.as_deref().ok_or_else(|| {
                CatalogError::Invalid(
                    "a plugin grant in force must record the package it was given to".into(),
                )
            })?;
            if fingerprint.len() != 64 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(CatalogError::Invalid(
                    "a plugin grant's package fingerprint must be a SHA-256 digest".into(),
                ));
            }
            if self.package_trusted.is_none() {
                return Err(CatalogError::Invalid(
                    "a plugin grant in force must record how its package arrived".into(),
                ));
            }
        }
        // The pairing is checked again against the manifest before any
        // invocation. It is checked here too so a hand-edited catalog cannot
        // persist a shape the resolver has never been asked to reason about.
        let values = match (&self.capability, &self.scope) {
            (PluginCapability::LibraryRead, CapabilityScope::LibraryGames(ids))
            | (PluginCapability::FilesRead, CapabilityScope::DirectoryGrants(ids))
            | (PluginCapability::Secrets, CapabilityScope::SecretNames(ids))
            | (PluginCapability::RunnerPrepare, CapabilityScope::RunnerProfiles(ids))
            | (PluginCapability::NetworkFetch, CapabilityScope::Domains(ids)) => ids.len(),
            (PluginCapability::Notifications, CapabilityScope::Notifications) => 0,
            _ => {
                return Err(CatalogError::Invalid(
                    "plugin grant scope does not match its capability".into(),
                ));
            }
        };
        if values > MAX_GRANT_SCOPE_VALUES {
            return Err(CatalogError::Invalid(
                "plugin grant scope names more values than Orivo will hold".into(),
            ));
        }
        Ok(())
    }
}

/// The value a directory grant carries in its scope: `<profile id>:<slot>`.
///
/// Not the slot alone, because the slot is the component's and not the user's.
/// A plugin hard-codes the id it asks `host-files` for, so every profile it owns
/// names its folder the same way; keyed by the slot, one profile's permission
/// answers for another's and revoking a folder on one leaves it reachable
/// through the other. Both halves are opaque tokens, so the pair is one too.
pub fn directory_grant_key(profile_id: &str, slot: &str) -> String {
    format!("{profile_id}:{slot}")
}

/// The slot half of a key, if it belongs to this profile.
pub fn directory_grant_slot<'key>(key: &'key str, profile_id: &str) -> Option<&'key str> {
    key.strip_prefix(profile_id)?.strip_prefix(':')
}

fn scope_for(
    capability: PluginCapability,
    values: BTreeSet<String>,
) -> Result<CapabilityScope, CatalogError> {
    match capability {
        PluginCapability::FilesRead => Ok(CapabilityScope::DirectoryGrants(values)),
        PluginCapability::RunnerPrepare => Ok(CapabilityScope::RunnerProfiles(values)),
        _ => Err(CatalogError::Invalid(
            "that capability is not scoped by value".into(),
        )),
    }
}

fn validate_runner_inventory_scope(
    entry: &RunnerGameInventoryEntry,
    profile: &RunnerProfile,
) -> Result<(), CatalogError> {
    let directory = profile
        .granted_directory(&entry.directory_grant_id)
        .ok_or_else(|| {
            CatalogError::Invalid(
                "runner game was resolved under a folder this profile no longer grants".into(),
            )
        })?;
    if entry.game_path != directory.path && entry.game_path.starts_with(&directory.path) {
        Ok(())
    } else {
        Err(CatalogError::Invalid(
            "runner game file is outside the folder it was granted under".into(),
        ))
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
        LaunchTarget::Direct | LaunchTarget::Steam { .. } | LaunchTarget::Provider { .. } => None,
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
    if incoming.home_image_path.is_none() {
        incoming.home_image_path = existing.home_image_path.clone();
    }
    if incoming.landscape_image_path.is_none() {
        incoming.landscape_image_path = existing.landscape_image_path.clone();
    }
    if incoming.logo_path.is_none() {
        incoming.logo_path = existing.logo_path.clone();
    }
    incoming.hidden = existing.hidden;
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
    catalog.schema_version = SCHEMA_VERSION_V6;
}

/// Rewrite path-bearing Direct-game ids to opaque `local:<sha256>` identities
/// and return the `old id -> new id` map so every store keyed by game id can be
/// re-keyed with it before anything is persisted.
fn migrate_v6_to_v7(catalog: &mut Catalog) -> Result<BTreeMap<String, String>, CatalogError> {
    let path_backed = catalog
        .games
        .iter()
        .enumerate()
        .filter_map(|(index, game)| {
            is_path_backed_direct_game(game).then(|| {
                let executable = game
                    .executable_path
                    .as_deref()
                    .expect("path-backed Direct games have an executable");
                (index, game.id.clone(), local_game_id(executable))
            })
        })
        .collect::<Vec<_>>();

    let mut base_counts = BTreeMap::<String, usize>::new();
    for (_, _, base_id) in &path_backed {
        *base_counts.entry(base_id.clone()).or_default() += 1;
    }

    // Reserve every provider/runner identity before assigning local ids. A
    // collision can therefore never overwrite or merge an unrelated record.
    let path_backed_indexes = path_backed
        .iter()
        .map(|(index, _, _)| *index)
        .collect::<BTreeSet<_>>();
    let mut assigned = catalog
        .games
        .iter()
        .enumerate()
        .filter(|(index, _)| !path_backed_indexes.contains(index))
        .map(|(_, game)| game.id.clone())
        .collect::<BTreeSet<_>>();
    let mut rewritten_ids = BTreeMap::<String, String>::new();

    for (index, old_id, base_id) in path_backed {
        let path = catalog.games[index]
            .executable_path
            .as_deref()
            .expect("path-backed Direct games have an executable");
        let base_is_unique = base_counts.get(&base_id) == Some(&1);
        let next_id = assign_local_game_id(path, &old_id, base_id, base_is_unique, &assigned)?;
        assigned.insert(next_id.clone());
        rewritten_ids.insert(old_id, next_id.clone());
        catalog.games[index].id = next_id;
    }

    // Wine keeps only a typed catalog reference to a Direct origin. Rewriting
    // it in this same candidate catalog makes the migration atomic: validation
    // fails before the caller creates its backup and persists anything.
    for entry in &mut catalog.wine_inventory {
        if let Some(origin) = entry.origin_direct_game_id.as_mut()
            && let Some(rewritten) = rewritten_ids.get(origin)
        {
            *origin = rewritten.clone();
        }
    }
    catalog.schema_version = SCHEMA_VERSION_V7;
    Ok(rewritten_ids)
}

/// v8's three tables — runner profiles, the private runner inventory and the
/// grant ledger — all deserialise from serde defaults, so a v7 document already
/// reads as one with no third-party runner configured. Nothing existing is
/// rewritten: Direct, Steam, provider and native-runner records are left
/// byte-for-byte, and a v7 card pointing at a third-party runner stays a card
/// whose profile has yet to be created.
fn migrate_v7_to_v8(catalog: &mut Catalog) {
    catalog.schema_version = CURRENT_SCHEMA_VERSION;
}

/// Salted retries exist only to break an identity collision, and a SHA-256
/// namespace makes even one collision unreachable in practice. The bound
/// guarantees the search terminates: a pathological catalog gets a real error
/// instead of a loop that can never advance.
const MAX_LOCAL_ID_COLLISION_ATTEMPTS: u32 = 1_024;

fn assign_local_game_id(
    executable_path: &Path,
    old_id: &str,
    base_id: String,
    base_is_unique: bool,
    assigned: &BTreeSet<String>,
) -> Result<String, CatalogError> {
    let mut next_id = if base_is_unique && !assigned.contains(&base_id) {
        base_id
    } else {
        local_game_id_with_salt(executable_path, old_id.as_bytes(), 0)
    };
    let mut nonce = 1_u32;
    while assigned.contains(&next_id) {
        if nonce > MAX_LOCAL_ID_COLLISION_ATTEMPTS {
            // The old id is never quoted here: legacy ids could be executable
            // paths, which is precisely what this migration removes.
            return Err(CatalogError::Invalid(format!(
                "could not derive a unique local game id after {MAX_LOCAL_ID_COLLISION_ATTEMPTS} attempts"
            )));
        }
        next_id = local_game_id_with_salt(executable_path, old_id.as_bytes(), nonce);
        nonce += 1;
    }
    Ok(next_id)
}

fn is_path_backed_direct_game(game: &Game) -> bool {
    game.source == GameSource::Local
        && matches!(&game.launch_target, LaunchTarget::Direct)
        && game
            .executable_path
            .as_deref()
            .is_some_and(Path::is_absolute)
}

/// Derive an opaque identity without ever serialising the source path into an
/// IPC-visible field. The domain tag keeps this namespace separate from media
/// and executable-content hashes used elsewhere.
pub fn local_game_id(executable_path: &Path) -> String {
    local_game_id_with_salt(executable_path, &[], 0)
}

fn local_game_id_with_salt(executable_path: &Path, salt: &[u8], nonce: u32) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orivo-local-game-id-v1\0");
    digest.update(executable_path.to_string_lossy().as_bytes());
    if !salt.is_empty() || nonce != 0 {
        digest.update(b"\0collision\0");
        digest.update(salt);
        digest.update(nonce.to_le_bytes());
    }
    format!("local:{:x}", digest.finalize())
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
        let id = local_game_id(&executable_path);

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
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hidden: false,
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
            (source, Some(source_id), LaunchTarget::Provider { provider, app_ref })
                if source.provider_token() == Some(provider.as_str()) =>
            {
                validate_provider_target(source_id, app_ref)?;
                // A connected-account record describes ownership, not a local
                // installation. Keeping every executable-style field empty is
                // what stops a provider response from ever being read back as
                // a path or an argument list.
                if self.executable_path.is_some()
                    || self.installation_path.is_some()
                    || self.working_directory.is_some()
                    || !self.arguments.is_empty()
                {
                    return Err(CatalogError::Invalid(format!(
                        "connected-source game {} cannot contain executable launch fields",
                        self.id
                    )));
                }
            }
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
const MAX_SOURCE_ID_LENGTH: usize = 256;
const MAX_PROVIDER_APP_REF_LENGTH: usize = 512;

/// Connected-store identities reuse the runner grammar on purpose. A provider
/// answer is untrusted input, and this is the boundary that keeps a hostile or
/// merely malformed response from ever reaching a URI, a filesystem path or a
/// process argument.
fn validate_provider_target(source_id: &str, app_ref: &str) -> Result<(), CatalogError> {
    validate_opaque_runner_token("source id", source_id, MAX_SOURCE_ID_LENGTH)?;
    validate_opaque_runner_token(
        "provider launch reference",
        app_ref,
        MAX_PROVIDER_APP_REF_LENGTH,
    )
}

/// The same grammar, exposed so a connector can drop an unusable provider
/// record while it is still a wire value instead of failing a whole sync.
pub fn is_valid_provider_reference(value: &str) -> bool {
    validate_opaque_runner_token("provider reference", value, MAX_PROVIDER_APP_REF_LENGTH).is_ok()
}

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

/// Turn a user-picked application into the file a process can actually be
/// started from. Shared with the third-party runner host, which is handed a
/// macOS bundle by the native picker exactly as local game import is.
pub(crate) fn resolve_executable(path: &Path) -> Result<PathBuf, CatalogError> {
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
        // `CFBundleExecutable` is a string in a file the host does not own, and
        // joining it is how `../../../bin/sh` — or an absolute path, which
        // `join` substitutes outright — becomes the program Orivo starts. A
        // bundle names one ordinary entry of its own `MacOS` folder or nothing.
        if let Some(executable_name) = executable_name.filter(|name| is_single_component(name)) {
            let executable = path.join("Contents/MacOS").join(executable_name);
            // A name inside the bundle is still only a name. `Contents/MacOS`
            // may hold a symbolic link, and following one out is the same escape
            // as spelling a path in the plist, so what comes back has to be a
            // regular file that still lives in this bundle.
            if let Ok(canonical) = fs::canonicalize(&executable)
                && fs::symlink_metadata(&executable)
                    .is_ok_and(|metadata| metadata.file_type().is_file())
                && fs::canonicalize(path).is_ok_and(|bundle| canonical.starts_with(&bundle))
            {
                return Ok(executable);
            }
        }
    }

    Err(CatalogError::Invalid(format!(
        "could not resolve an executable from {}",
        path.display()
    )))
}

/// One ordinary path component: no separator, no root, no `.` or `..`, and
/// nothing a platform reads as a drive or a stream.
fn is_single_component(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
        && !value.contains(['/', '\\', ':'])
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
    use crate::game_detail::{
        GameMediaAsset, GameMediaKind, GameMediaOrigin, GameStateDocument, GameStateStore,
    };

    #[test]
    fn a_source_resync_retracts_a_value_the_provider_no_longer_publishes() {
        // The regression this exists for: the Epic connector once filled the
        // genre with the studio name. Fixing the connector changed nothing for
        // the games already imported, because the stale key was merged forward
        // on every re-sync and could never be cleared.
        let mut catalog = Catalog::default();
        let mut first = provider_game(GameSource::Epic, "Sugar", "Hogwarts Legacy");
        first.extra.insert(
            SOURCE_GENRE_KEY.to_string(),
            serde_json::json!("Warner Bros."),
        );
        first.extra.insert(
            "orivo_store_landscape_url".to_string(),
            serde_json::json!("https://example.invalid/x.jpg"),
        );
        catalog.upsert_source(first).unwrap();

        // The fixed connector sends no genre at all.
        let second = provider_game(GameSource::Epic, "Sugar", "Hogwarts Legacy");
        catalog.upsert_source(second).unwrap();

        let stored = &catalog.games[0];
        assert!(
            !stored.extra.contains_key(SOURCE_GENRE_KEY),
            "a provider-owned key the sync omitted must not survive"
        );
        assert!(
            stored.extra.contains_key("orivo_store_landscape_url"),
            "a key Orivo owns must survive a re-sync"
        );
    }

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
    fn upgrades_v6_path_ids_and_wine_origins_without_exposing_the_path() {
        let path = temporary_catalog_path("v6-local-id");
        let v6 = r#"{
  "schema_version": 6,
  "future_catalog_field": { "keep": true },
  "games": [
    {
      "id": "/Games/Windows/Blue Prince/BLUE PRINCE.exe",
      "title": "Blue Prince",
      "executable_path": "/Games/Windows/Blue Prince/BLUE PRINCE.exe",
      "future_game_field": "preserved"
    },
    {
      "id": "runner:wine-game-blue-prince",
      "title": "Blue Prince (Wine)",
      "source": "local",
      "launch_target": {
        "kind": "runner",
        "runner_id": "com.orivo.wine-staging",
        "profile_id": "wine-profile-1",
        "game_ref": "wine-game-blue-prince"
      }
    },
    {
      "id": "steam:480",
      "title": "Spacewar",
      "source": "steam",
      "source_id": "480",
      "launch_target": { "kind": "steam", "app_id": 480 }
    }
  ],
  "wine_profiles": [
    {
      "id": "wine-profile-1",
      "display_name": "Windows games",
      "wine_binary": "/Applications/Wine Staging.app/Contents/Resources/wine/bin/wine",
      "prefix": "/Users/orivo/Library/Application Support/Orivo/wine-prefixes/wine-profile-1",
      "game_directories": ["/Games/Windows"],
      "enabled": true
    }
  ],
  "wine_inventory": [
    {
      "profile_id": "wine-profile-1",
      "game_ref": "wine-game-blue-prince",
      "title": "Blue Prince",
      "executable_path": "/Games/Windows/Blue Prince/BLUE PRINCE.exe",
      "fingerprint": "sha256:abc123",
      "origin_direct_game_id": "/Games/Windows/Blue Prince/BLUE PRINCE.exe"
    }
  ]
}"#;
        fs::write(&path, v6).unwrap();

        let loaded = Catalog::load_with_migration(&path).unwrap();
        let direct = loaded
            .catalog
            .games
            .iter()
            .find(|game| matches!(&game.launch_target, LaunchTarget::Direct))
            .unwrap();

        assert_eq!(loaded.migrated_from, Some(SCHEMA_VERSION_V6));
        assert!(direct.id.starts_with("local:"));
        assert_eq!(direct.id.len(), "local:".len() + 64);
        assert!(!direct.id.contains("Games"));
        assert_eq!(
            loaded.catalog.wine_inventory[0]
                .origin_direct_game_id
                .as_deref(),
            Some(direct.id.as_str())
        );
        assert!(
            loaded
                .catalog
                .games
                .iter()
                .any(|game| game.id == "steam:480")
        );
        assert_eq!(
            direct.extra.get("future_game_field"),
            Some(&serde_json::Value::String("preserved".into()))
        );
        assert_eq!(
            loaded.catalog.extra.get("future_catalog_field"),
            Some(&serde_json::json!({ "keep": true }))
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), v6);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn v7_local_id_migration_is_idempotent() {
        let path = temporary_catalog_path("v7-idempotent");
        let mut catalog = Catalog {
            schema_version: SCHEMA_VERSION_V6,
            games: vec![direct_windows_game()],
            ..Catalog::default()
        };
        catalog.schema_version = SCHEMA_VERSION_V6;
        fs::write(&path, serde_json::to_string_pretty(&catalog).unwrap()).unwrap();

        let first = Catalog::load_with_migration(&path).unwrap().catalog;
        first.save_atomically(&path).unwrap();
        let second = Catalog::load_with_migration(&path).unwrap();

        assert_eq!(second.migrated_from, None);
        assert_eq!(second.catalog, first);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn v7_migration_keeps_same_path_records_distinct_without_title_merging() {
        let path = temporary_catalog_path("v7-collision");
        let mut first = direct_windows_game();
        first.id = "/Games/Windows/Blue Prince/BLUE PRINCE.exe".into();
        first.title = "First record".into();
        let mut second = first.clone();
        second.id = "legacy-local-blue-prince".into();
        second.title = "Second record".into();
        let catalog = Catalog {
            schema_version: SCHEMA_VERSION_V6,
            games: vec![first, second],
            ..Catalog::default()
        };
        fs::write(&path, serde_json::to_string_pretty(&catalog).unwrap()).unwrap();

        let migrated = Catalog::load_with_migration(&path).unwrap().catalog;
        let ids = migrated
            .games
            .iter()
            .map(|game| game.id.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(migrated.games.len(), 2);
        assert_eq!(ids.len(), 2);
        assert!(
            ids.iter()
                .all(|id| id.starts_with("local:") && id.len() == 70)
        );
        assert_eq!(migrated.games[0].title, "First record");
        assert_eq!(migrated.games[1].title, "Second record");
        fs::remove_file(path).unwrap();
    }

    fn temporary_migration_directory(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "orivo-migration-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn write_v6_catalog(path: &Path) {
        let catalog = Catalog {
            schema_version: SCHEMA_VERSION_V6,
            games: vec![direct_windows_game(), steam_game("Spacewar")],
            ..Catalog::default()
        };
        fs::write(path, serde_json::to_string_pretty(&catalog).unwrap()).unwrap();
    }

    fn imported_media(id: &str, kind: GameMediaKind, file: &str) -> GameMediaAsset {
        GameMediaAsset {
            id: id.into(),
            kind,
            title: "Imported".into(),
            source_url: None,
            poster_url: None,
            origin: GameMediaOrigin::Imported,
            local_file: Some(file.into()),
            mime_type: Some("image/png".into()),
            byte_size: 1_024,
            extra: BTreeMap::new(),
        }
    }

    fn migrated_direct_id() -> String {
        local_game_id(Path::new("/Games/Windows/Blue Prince/BLUE PRINCE.exe"))
    }

    fn state_document(path: &Path) -> GameStateDocument {
        GameStateStore::load(path.to_path_buf())
            .unwrap()
            .snapshot()
            .unwrap()
    }

    #[test]
    fn v7_migration_rekeys_wishlist_selection_and_imported_media_with_the_catalog() {
        let directory = temporary_migration_directory("state-rekey");
        let catalog_path = directory.join("catalog.json");
        let state_path = directory.join("game-state.json");
        write_v6_catalog(&catalog_path);

        let state = GameStateStore::load(state_path.clone()).unwrap();
        state.set_wishlist("local-blue-prince", true).unwrap();
        state
            .register_and_select_media(
                "local-blue-prince",
                imported_media("media:import-1", GameMediaKind::Wallpaper, "import-1.png"),
            )
            .unwrap();
        state
            .register_media(
                "local-blue-prince",
                imported_media("media:import-2", GameMediaKind::Cover, "import-2.png"),
            )
            .unwrap();
        state.set_wishlist("steam:480", true).unwrap();
        drop(state);

        let loaded = Catalog::load_with_migration(&catalog_path).unwrap();
        loaded.commit_migration(&catalog_path, &state_path).unwrap();

        let migrated_id = migrated_direct_id();
        assert_eq!(loaded.catalog.games[0].id, migrated_id);
        assert_eq!(
            loaded.rewritten_game_ids.get("local-blue-prince"),
            Some(&migrated_id)
        );

        let document = state_document(&state_path);
        assert!(!document.games.contains_key("local-blue-prince"));
        let migrated = document.games.get(&migrated_id).unwrap();
        assert!(migrated.wishlisted);
        assert_eq!(
            migrated.selected_media.get(&GameMediaKind::Wallpaper),
            Some(&"media:import-1".to_string())
        );
        assert_eq!(migrated.media.len(), 2);
        assert_eq!(
            migrated.media["media:import-2"].local_file.as_deref(),
            Some("import-2.png")
        );
        // Ids the migration never touched keep their own state.
        assert!(document.games["steam:480"].wishlisted);

        // The orphaned-quota regression: imported files stay reachable from the
        // live game id instead of pinning the media quota forever.
        let protected = GameStateStore::load(state_path)
            .unwrap()
            .protected_local_files()
            .unwrap();
        assert!(protected.contains("import-1.png"));
        assert!(protected.contains("import-2.png"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn v7_game_state_rekey_is_idempotent_across_two_runs() {
        let directory = temporary_migration_directory("state-idempotent");
        let catalog_path = directory.join("catalog.json");
        let state_path = directory.join("game-state.json");
        write_v6_catalog(&catalog_path);

        let state = GameStateStore::load(state_path.clone()).unwrap();
        state.set_wishlist("local-blue-prince", true).unwrap();
        state
            .register_and_select_media(
                "local-blue-prince",
                imported_media("media:import-1", GameMediaKind::Wallpaper, "import-1.png"),
            )
            .unwrap();
        drop(state);

        let first = Catalog::load_with_migration(&catalog_path).unwrap();
        first.commit_migration(&catalog_path, &state_path).unwrap();
        let after_first = state_document(&state_path);

        // Replaying the identical rewrite must not duplicate, drop, or
        // double-rewrite anything.
        StagedGameState::stage(&state_path, &first.rewritten_game_ids)
            .unwrap()
            .commit()
            .unwrap();
        assert_eq!(state_document(&state_path), after_first);

        // A second startup no longer migrates, and committing again is a no-op.
        let second = Catalog::load_with_migration(&catalog_path).unwrap();
        assert_eq!(second.migrated_from, None);
        assert!(second.rewritten_game_ids.is_empty());
        second.commit_migration(&catalog_path, &state_path).unwrap();

        let after_second = state_document(&state_path);
        assert_eq!(after_second, after_first);
        assert_eq!(after_second.games.len(), 1);
        assert!(after_second.games.contains_key(&migrated_direct_id()));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn v7_game_state_rekey_merges_entries_present_under_both_ids() {
        let directory = temporary_migration_directory("state-merge");
        let catalog_path = directory.join("catalog.json");
        let state_path = directory.join("game-state.json");
        write_v6_catalog(&catalog_path);
        let migrated_id = migrated_direct_id();

        let state = GameStateStore::load(state_path.clone()).unwrap();
        state.set_wishlist("local-blue-prince", true).unwrap();
        state
            .register_and_select_media(
                "local-blue-prince",
                imported_media("media:old-wallpaper", GameMediaKind::Wallpaper, "old.png"),
            )
            .unwrap();
        state
            .register_and_select_media(
                &migrated_id,
                imported_media("media:new-wallpaper", GameMediaKind::Wallpaper, "new.png"),
            )
            .unwrap();
        state
            .register_and_select_media(
                &migrated_id,
                imported_media("media:new-cover", GameMediaKind::Cover, "new-cover.png"),
            )
            .unwrap();
        drop(state);

        Catalog::load_with_migration(&catalog_path)
            .unwrap()
            .commit_migration(&catalog_path, &state_path)
            .unwrap();

        let document = state_document(&state_path);
        assert_eq!(document.games.len(), 1);
        let merged = document.games.get(&migrated_id).unwrap();
        // Precedence: the migrated entry wins the kinds it selects, the
        // pre-existing entry keeps every kind the winner leaves free, and no
        // registration is lost.
        assert!(merged.wishlisted);
        assert_eq!(
            merged.selected_media.get(&GameMediaKind::Wallpaper),
            Some(&"media:old-wallpaper".to_string())
        );
        assert_eq!(
            merged.selected_media.get(&GameMediaKind::Cover),
            Some(&"media:new-cover".to_string())
        );
        assert_eq!(merged.media.len(), 3);

        let protected = GameStateStore::load(state_path)
            .unwrap()
            .protected_local_files()
            .unwrap();
        for file in ["old.png", "new.png", "new-cover.png"] {
            assert!(protected.contains(file));
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_failed_catalog_write_leaves_the_catalog_and_game_state_pre_migration() {
        let directory = temporary_migration_directory("state-rollback");
        let catalog_path = directory.join("catalog.json");
        let state_path = directory.join("game-state.json");
        write_v6_catalog(&catalog_path);

        let state = GameStateStore::load(state_path.clone()).unwrap();
        state.set_wishlist("local-blue-prince", true).unwrap();
        state
            .register_and_select_media(
                "local-blue-prince",
                imported_media("media:import-1", GameMediaKind::Wallpaper, "import-1.png"),
            )
            .unwrap();
        drop(state);
        let before = fs::read_to_string(&state_path).unwrap();

        // A regular file cannot become a parent directory, so publishing the
        // catalog fails after the game state has already been written.
        let blocked_parent = directory.join("blocked");
        fs::write(&blocked_parent, b"not a directory").unwrap();
        let loaded = Catalog::load_with_migration(&catalog_path).unwrap();
        assert!(
            loaded
                .commit_migration(&blocked_parent.join("catalog.json"), &state_path)
                .is_err()
        );

        assert_eq!(fs::read_to_string(&state_path).unwrap(), before);
        let document = state_document(&state_path);
        assert!(document.games.contains_key("local-blue-prince"));
        assert!(!document.games.contains_key(&migrated_direct_id()));
        let on_disk: Catalog =
            serde_json::from_str(&fs::read_to_string(&catalog_path).unwrap()).unwrap();
        assert_eq!(on_disk.schema_version, SCHEMA_VERSION_V6);
        assert_eq!(on_disk.games[0].id, "local-blue-prince");
        assert!(!directory.join("game-state.json.migrating").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_missing_game_state_document_is_not_a_migration_failure() {
        let directory = temporary_migration_directory("state-missing");
        let catalog_path = directory.join("catalog.json");
        let state_path = directory.join("game-state.json");
        write_v6_catalog(&catalog_path);

        Catalog::load_with_migration(&catalog_path)
            .unwrap()
            .commit_migration(&catalog_path, &state_path)
            .unwrap();

        assert!(!state_path.exists());
        let reloaded = Catalog::load(&catalog_path).unwrap();
        assert_eq!(reloaded.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(reloaded.games[0].id, migrated_direct_id());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn local_id_assignment_errors_instead_of_looping_when_every_nonce_collides() {
        let executable = Path::new("/Games/Windows/Blue Prince/BLUE PRINCE.exe");
        let old_id = "local-blue-prince";
        let base_id = local_game_id(executable);
        let mut assigned = BTreeSet::from([base_id.clone()]);
        for nonce in 0..=MAX_LOCAL_ID_COLLISION_ATTEMPTS {
            assigned.insert(local_game_id_with_salt(
                executable,
                old_id.as_bytes(),
                nonce,
            ));
        }

        assert!(matches!(
            assign_local_game_id(executable, old_id, base_id, true, &assigned),
            Err(CatalogError::Invalid(message))
                if message.contains("unique local game id") && !message.contains("Blue Prince")
        ));
    }

    #[test]
    fn new_local_game_identity_is_opaque_and_stable() {
        let path = Path::new("/Users/private/Games/Nightfall/Nightfall");
        let first = Game::from_executable(path).unwrap();
        let second = Game::from_executable(path).unwrap();

        assert_eq!(first.id, second.id);
        assert!(first.id.starts_with("local:"));
        assert_eq!(first.id.len(), 70);
        assert!(!first.id.contains("private"));
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
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hidden: false,
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
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hidden: false,
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
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hidden: false,
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
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hidden: false,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        }
    }

    fn provider_game(source: GameSource, source_id: &str, title: &str) -> Game {
        let provider = source.provider_token().expect("a connected source");
        Game {
            id: format!("{provider}:{source_id}"),
            title: title.into(),
            executable_path: None,
            source,
            source_id: Some(source_id.into()),
            launch_target: LaunchTarget::Provider {
                provider: provider.into(),
                app_ref: source_id.into(),
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
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hidden: false,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn a_connected_store_record_needs_a_matching_provider_and_no_launch_paths() {
        assert!(
            provider_game(GameSource::Epic, "Sugar", "Fall Guys")
                .validate()
                .is_ok()
        );

        // The provider token has to agree with the source, or a GOG record
        // could describe itself as an Epic launch.
        let mut mismatched = provider_game(GameSource::Gog, "1207658924", "The Witcher");
        mismatched.launch_target = LaunchTarget::Provider {
            provider: "epic".into(),
            app_ref: "1207658924".into(),
        };
        assert!(mismatched.validate().is_err());

        // A path or an argument list can never ride along on a store record.
        let mut with_path = provider_game(GameSource::Epic, "Sugar", "Fall Guys");
        with_path.executable_path = Some(PathBuf::from("/tmp/anything"));
        assert!(with_path.validate().is_err());

        let mut with_arguments = provider_game(GameSource::Epic, "Sugar", "Fall Guys");
        with_arguments.arguments = vec!["--exec".into()];
        assert!(with_arguments.validate().is_err());

        // And a reference outside the opaque grammar never becomes a URI.
        let mut traversal = provider_game(GameSource::Ubisoft, "5416", "Anno 1800");
        traversal.launch_target = LaunchTarget::Provider {
            provider: "ubisoft".into(),
            app_ref: "../../etc/passwd".into(),
        };
        assert!(traversal.validate().is_err());
    }

    #[test]
    fn resyncing_a_store_refreshes_a_card_instead_of_duplicating_it() {
        let mut catalog = Catalog::default();
        let mut first = provider_game(GameSource::Epic, "Sugar", "Fall Guys");
        first.play_time_seconds = 7_200;
        first.home_image_path = Some(PathBuf::from("/cache/chosen-wallpaper.jpg"));
        first.description = Some("A chaotic obstacle course.".into());
        assert!(catalog.upsert_source(first).unwrap());

        // A later sync that arrives without play time, without the chosen
        // wallpaper and without a description must not undo any of them.
        let refreshed = provider_game(GameSource::Epic, "Sugar", "Fall Guys: Season 5");
        assert!(!catalog.upsert_source(refreshed).unwrap());

        assert_eq!(catalog.games.len(), 1);
        let game = &catalog.games[0];
        assert_eq!(game.title, "Fall Guys: Season 5");
        assert_eq!(game.play_time_seconds, 7_200);
        assert_eq!(
            game.home_image_path.as_deref(),
            Some(Path::new("/cache/chosen-wallpaper.jpg"))
        );
        assert_eq!(
            game.description.as_deref(),
            Some("A chaotic obstacle course.")
        );
        assert!(catalog.validate().is_ok());
    }

    #[test]
    fn the_same_game_owned_on_two_stores_stays_two_records() {
        let mut catalog = Catalog::default();
        assert!(
            catalog
                .upsert_source(provider_game(GameSource::Xbox, "1017535743", "Minecraft"))
                .unwrap()
        );
        assert!(
            catalog
                .upsert_source(provider_game(
                    GameSource::MicrosoftStore,
                    "1017535743",
                    "Minecraft"
                ))
                .unwrap()
        );

        assert_eq!(catalog.games.len(), 2);
        assert!(catalog.validate().is_ok());
    }

    #[test]
    fn upsert_source_refuses_a_record_from_a_source_it_does_not_own() {
        let mut catalog = Catalog::default();
        assert!(matches!(
            catalog.upsert_source(steam_game("Spacewar")),
            Err(CatalogError::Invalid(_))
        ));
        assert!(catalog.games.is_empty());
    }

    #[test]
    fn forgetting_one_store_leaves_every_other_library_intact() {
        let mut catalog = Catalog::default();
        catalog.add(steam_game("Spacewar")).unwrap();
        catalog
            .upsert_source(provider_game(GameSource::Gog, "1207658924", "The Witcher"))
            .unwrap();
        catalog
            .upsert_source(provider_game(GameSource::Gog, "1495134320", "Cyberpunk"))
            .unwrap();
        catalog
            .upsert_source(provider_game(GameSource::Epic, "Sugar", "Fall Guys"))
            .unwrap();

        assert_eq!(catalog.remove_source_games(GameSource::Gog), 2);
        assert_eq!(catalog.games.len(), 2);
        assert!(
            catalog
                .games
                .iter()
                .all(|game| game.source != GameSource::Gog)
        );
        assert!(catalog.validate().is_ok());
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
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hidden: false,
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
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hidden: false,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        }
    }

    fn winlator_reference_profile() -> WinlatorProfile {
        WinlatorProfile {
            id: "winlator-profile-1".into(),
            display_name: "Winlator".into(),
            distribution: WinlatorDistribution::Cmod,
            container_id: None,
            shortcut_directories: vec![PathBuf::from(
                "/storage/emulated/0/Download/Winlator/Frontend",
            )],
            shortcut_trees: Vec::new(),
            enabled: true,
            last_imported_at: Some(1_721_553_600_000),
        }
    }

    fn winlator_shortcut_entry(game_ref: &str) -> WinlatorShortcutInventoryEntry {
        WinlatorShortcutInventoryEntry {
            profile_id: "winlator-profile-1".into(),
            game_ref: game_ref.into(),
            title: "Windows Example".into(),
            shortcut_path: PathBuf::from(
                "/storage/emulated/0/Download/Winlator/Frontend/Example.desktop",
            ),
            fingerprint: "sha256:abc123".into(),
            container_id: Some(2),
            imported_at: Some(1_721_553_600_000),
        }
    }

    fn winlator_runner_game(game_ref: &str, title: &str) -> Game {
        Game {
            id: format!("runner:winlator:{game_ref}"),
            title: title.into(),
            executable_path: None,
            source: GameSource::Local,
            source_id: None,
            launch_target: LaunchTarget::Runner {
                runner_id: WINLATOR_RUNNER_ID.into(),
                game_ref: game_ref.into(),
                profile_id: "winlator-profile-1".into(),
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
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hidden: false,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn persists_a_winlator_profile_inventory_and_runner_card() {
        let mut catalog = Catalog::default();
        assert!(
            catalog
                .upsert_winlator_profile(winlator_reference_profile())
                .unwrap()
        );
        assert!(
            catalog
                .upsert_winlator_inventory(winlator_shortcut_entry("shortcut:abc"))
                .unwrap()
        );
        assert!(
            catalog
                .upsert_runner(winlator_runner_game("shortcut:abc", "Windows Example"))
                .unwrap()
        );
        catalog.validate().unwrap();

        let card = &catalog.games[0];
        // The card is the only thing a view model is built from, so the shortcut
        // path must not be reachable through it.
        assert!(card.executable_path.is_none());
        assert!(card.installation_path.is_none());
        assert!(card.arguments.is_empty());
        assert_eq!(
            catalog
                .winlator_inventory_entry("winlator-profile-1", "shortcut:abc")
                .map(|entry| entry.container_id),
            Some(Some(2))
        );
    }

    #[test]
    fn refuses_a_winlator_shortcut_outside_the_profile_grant() {
        let mut catalog = Catalog::default();
        catalog
            .upsert_winlator_profile(winlator_reference_profile())
            .unwrap();
        let mut entry = winlator_shortcut_entry("shortcut:abc");
        entry.shortcut_path = PathBuf::from("/storage/emulated/0/Download/Elsewhere.desktop");
        assert!(catalog.upsert_winlator_inventory(entry).is_err());
        assert!(catalog.winlator_inventory.is_empty());
    }

    /// A Winlator inventory entry points at a shortcut Winlator wrote, never at
    /// an executable Orivo could be tricked into treating as one.
    #[test]
    fn refuses_a_winlator_inventory_entry_that_is_not_a_desktop_file() {
        let mut catalog = Catalog::default();
        catalog
            .upsert_winlator_profile(winlator_reference_profile())
            .unwrap();
        let mut entry = winlator_shortcut_entry("shortcut:abc");
        entry.shortcut_path =
            PathBuf::from("/storage/emulated/0/Download/Winlator/Frontend/Example.exe");
        assert!(catalog.upsert_winlator_inventory(entry).is_err());
    }

    #[test]
    fn refuses_a_container_id_that_is_not_a_container_number() {
        let mut catalog = Catalog::default();
        for container_id in [Some(0), Some(10_000)] {
            let mut profile = winlator_reference_profile();
            profile.container_id = container_id;
            assert!(
                catalog.upsert_winlator_profile(profile).is_err(),
                "accepted container id {container_id:?}"
            );
        }
    }

    #[test]
    fn refuses_a_winlator_profile_without_a_granted_shortcut_directory() {
        let mut catalog = Catalog::default();
        let mut profile = winlator_reference_profile();
        profile.shortcut_directories.clear();
        assert!(catalog.upsert_winlator_profile(profile).is_err());
    }

    /// A Winlator card without its private inventory entry would be a launch
    /// target the host cannot resolve, so the catalog refuses to hold one.
    #[test]
    fn refuses_a_winlator_card_without_its_private_inventory_entry() {
        let mut catalog = Catalog::default();
        catalog
            .upsert_winlator_profile(winlator_reference_profile())
            .unwrap();
        assert!(
            catalog
                .upsert_runner(winlator_runner_game("shortcut:abc", "Windows Example"))
                .is_err()
        );
        assert!(
            catalog
                .add(winlator_runner_game("shortcut:abc", "Windows Example"))
                .is_err()
        );
    }

    /// Winlator support adds two optional arrays rather than a schema version:
    /// a v7 file written before this change stays readable, and reading then
    /// writing it must not disturb anything a user already had.
    #[test]
    fn a_v7_catalog_written_without_winlator_fields_still_loads_and_round_trips() {
        let path = temporary_catalog_path("winlator-round-trip");
        fs::write(
            &path,
            format!(
                r#"{{"schema_version":{CURRENT_SCHEMA_VERSION},"games":[],"wine_profiles":[],"wine_inventory":[]}}"#
            ),
        )
        .unwrap();

        let loaded = Catalog::load_with_migration(&path).unwrap();
        assert_eq!(loaded.migrated_from, None);
        assert!(loaded.catalog.winlator_profiles.is_empty());
        assert!(loaded.catalog.winlator_inventory.is_empty());

        let mut catalog = loaded.catalog;
        catalog
            .upsert_winlator_profile(winlator_reference_profile())
            .unwrap();
        catalog
            .upsert_winlator_inventory(winlator_shortcut_entry("shortcut:abc"))
            .unwrap();
        catalog.save_atomically(&path).unwrap();

        let reloaded = Catalog::load(&path).unwrap();
        assert_eq!(reloaded, catalog);
        assert_eq!(reloaded.schema_version, CURRENT_SCHEMA_VERSION);
        fs::remove_file(&path).ok();
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
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hidden: false,
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
    // -----------------------------------------------------------------------
    // Schema v8: third-party runner profiles, inventory and the grant ledger
    // -----------------------------------------------------------------------

    const FIXTURE_PLUGIN: &str = "com.orivo.fixture-runner";

    fn runner_profile() -> RunnerProfile {
        RunnerProfile {
            id: "fixture-profile-1".into(),
            plugin_id: FIXTURE_PLUGIN.into(),
            display_name: "Fixture Runner".into(),
            application: PathBuf::from("/Applications/Fixture Emulator.app"),
            game_directories: vec![RunnerGrantedDirectory {
                id: "fixture-games".into(),
                path: PathBuf::from("/Games/Roms"),
                device: None,
                inode: None,
            }],
            settings: RunnerProfileSettings::default(),
            status: RunnerProfileStatus::Valid,
            status_message: None,
            enabled: true,
            import_cursor: None,
            import_complete: false,
            last_imported_at: None,
            package_fingerprint: None,
        }
    }

    fn runner_inventory_entry(game_ref: &str) -> RunnerGameInventoryEntry {
        RunnerGameInventoryEntry {
            profile_id: "fixture-profile-1".into(),
            game_ref: game_ref.into(),
            title: "Alpha Quest".into(),
            provider_id: FIXTURE_PLUGIN.into(),
            external_id: game_ref.into(),
            game_path: PathBuf::from(format!("/Games/Roms/{game_ref}.rom")),
            directory_grant_id: "fixture-games".into(),
            platform: Some("fixture".into()),
            imported_at: Some(1_721_553_600_000),
        }
    }

    fn third_party_runner_game(game_ref: &str) -> Game {
        Game {
            id: format!("runner:{FIXTURE_PLUGIN}:fixture-profile-1:{game_ref}"),
            title: "Alpha Quest".into(),
            executable_path: None,
            source: GameSource::Local,
            source_id: None,
            launch_target: LaunchTarget::Runner {
                runner_id: FIXTURE_PLUGIN.into(),
                game_ref: game_ref.into(),
                profile_id: "fixture-profile-1".into(),
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
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hidden: false,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        }
    }

    /// A release-signed package, which is the ordinary case and the one whose
    /// grants survive an update.
    fn package_identity() -> PluginPackageIdentity {
        PluginPackageIdentity {
            fingerprint: "a".repeat(64),
            trusted: true,
        }
    }

    fn files_grant(ids: &[&str], granted_at: u64) -> PluginGrantRecord {
        let identity = package_identity();
        PluginGrantRecord {
            plugin_id: FIXTURE_PLUGIN.into(),
            capability: PluginCapability::FilesRead,
            scope: CapabilityScope::DirectoryGrants(
                ids.iter().map(|id| (*id).to_string()).collect(),
            ),
            granted_at,
            revoked_at: None,
            package_fingerprint: Some(identity.fingerprint),
            package_trusted: Some(identity.trusted),
        }
    }

    fn catalog_with_runner_game() -> Catalog {
        let mut catalog = Catalog::default();
        assert!(catalog.upsert_runner_profile(runner_profile()).unwrap());
        assert!(
            catalog
                .upsert_runner_inventory(runner_inventory_entry("alpha"))
                .unwrap()
        );
        assert!(
            catalog
                .upsert_runner(third_party_runner_game("alpha"))
                .unwrap()
        );
        catalog.validate().unwrap();
        catalog
    }

    /// A v7 document as the previous build wrote them: opaque `local:` ids, a
    /// Wine profile with its private inventory, a Steam record and a native
    /// runner card. This is the input the v8 migration has to leave alone.
    fn v7_catalog() -> Catalog {
        let mut direct = direct_windows_game();
        direct.id = migrated_direct_id();
        Catalog {
            schema_version: SCHEMA_VERSION_V7,
            games: vec![
                direct,
                steam_game("Spacewar"),
                wine_runner_game("rom:sha256:abc123", "Windows Example"),
            ],
            wine_profiles: vec![wine_profile()],
            wine_inventory: vec![wine_inventory_entry("rom:sha256:abc123")],
            ..Catalog::default()
        }
    }

    fn write_catalog(path: &Path, catalog: &Catalog) {
        fs::write(path, serde_json::to_string_pretty(catalog).unwrap() + "\n").unwrap();
    }

    /// The migration is additive, so the one thing worth asserting is that it
    /// took nothing with it: every record a v7 build wrote comes back identical,
    /// and the three new tables arrive empty rather than invented.
    #[test]
    fn a_v7_catalog_reaches_v8_without_changing_anything_it_already_held() {
        let directory = temporary_migration_directory("v8-additive");
        let path = directory.join("catalog.json");
        let source = v7_catalog();
        write_catalog(&path, &source);

        let loaded = Catalog::load_with_migration(&path).unwrap();
        assert_eq!(loaded.migrated_from, Some(SCHEMA_VERSION_V7));
        assert!(loaded.rewritten_game_ids.is_empty());
        assert_eq!(loaded.catalog.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.catalog.games, source.games);
        assert_eq!(loaded.catalog.wine_profiles, source.wine_profiles);
        assert_eq!(loaded.catalog.wine_inventory, source.wine_inventory);
        assert!(loaded.catalog.runner_profiles.is_empty());
        assert!(loaded.catalog.runner_inventory.is_empty());
        assert!(loaded.catalog.plugin_grants.is_empty());
        loaded.catalog.validate().unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_v6_catalog_still_reaches_v8_in_one_load() {
        let directory = temporary_migration_directory("v8-chain");
        let path = directory.join("catalog.json");
        write_v6_catalog(&path);

        let loaded = Catalog::load_with_migration(&path).unwrap();
        assert_eq!(loaded.migrated_from, Some(SCHEMA_VERSION_V6));
        assert_eq!(loaded.catalog.schema_version, CURRENT_SCHEMA_VERSION);
        // The v7 id rewrite still happens on the way through.
        assert_eq!(loaded.catalog.games[0].id, migrated_direct_id());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_migrated_v7_catalog_is_published_with_its_backup_and_reloads_as_v8() {
        let directory = temporary_migration_directory("v8-publish");
        let path = directory.join("catalog.json");
        let state_path = directory.join("game-state.json");
        let backup = directory.join("catalog.json.v7.bak");
        write_catalog(&path, &v7_catalog());
        fs::copy(&path, &backup).unwrap();

        let loaded = Catalog::load_with_migration(&path).unwrap();
        loaded
            .commit_migration_with_backup(&path, &state_path, &backup)
            .unwrap();

        let reloaded = Catalog::load_with_migration(&path).unwrap();
        assert_eq!(reloaded.migrated_from, None);
        assert_eq!(reloaded.catalog, loaded.catalog);
        fs::remove_dir_all(directory).unwrap();
    }

    /// The plan's exit test for the migration. A build that cannot publish what
    /// it migrated must leave the library exactly as the build that wrote it did,
    /// and say so — a catalog readable only by the version that migrated it is
    /// worse than one that never migrated.
    ///
    /// The failure is injected where a migration can really fail once the
    /// document itself is sound: the dependent `game-state.json` rewrite. It
    /// lives in its own read-only directory here, so staging it fails while the
    /// catalog beside it is still recoverable.
    #[cfg(unix)]
    #[test]
    fn a_migration_that_cannot_be_published_is_restored_from_its_backup() {
        use std::os::unix::fs::PermissionsExt;

        let directory = temporary_migration_directory("v8-restore");
        let path = directory.join("catalog.json");
        let state_directory = directory.join("state");
        let state_path = state_directory.join("game-state.json");
        let backup = directory.join("catalog.json.v6.bak");
        // A v6 document, because only a migration that rewrites game ids has a
        // game-state rewrite to fail at.
        write_v6_catalog(&path);
        fs::copy(&path, &backup).unwrap();
        fs::create_dir_all(&state_directory).unwrap();
        let state = GameStateStore::load(state_path.clone()).unwrap();
        state.set_wishlist("local-blue-prince", true).unwrap();
        drop(state);
        let state_before = fs::read(&state_path).unwrap();
        let before = fs::read(&path).unwrap();
        fs::set_permissions(&state_directory, fs::Permissions::from_mode(0o555)).unwrap();

        let error = Catalog::load_with_migration(&path)
            .unwrap()
            .commit_migration_with_backup(&path, &state_path, &backup)
            .expect_err("the game state cannot be staged in a read-only directory");
        assert!(
            error.to_string().contains("previous catalog was restored"),
            "{error}"
        );

        // Byte for byte what the older build wrote, on both sides of the pair.
        assert_eq!(fs::read(&path).unwrap(), before);
        let on_disk: Catalog = serde_json::from_slice(&before).unwrap();
        assert_eq!(on_disk.schema_version, SCHEMA_VERSION_V6);
        assert_eq!(fs::read(&state_path).unwrap(), state_before);

        fs::set_permissions(&state_directory, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    /// A document v8 refuses is refused while reading it, before a backup is
    /// even taken — which is the earliest and safest place to stop. The file is
    /// left exactly as it was found.
    #[test]
    fn a_v7_document_v8_cannot_validate_is_refused_before_anything_is_written() {
        let directory = temporary_migration_directory("v8-invalid");
        let path = directory.join("catalog.json");
        // A runner inventory entry with no profile behind it: the shape a
        // half-applied rollback between a v8 build and a v7 one leaves.
        let source = Catalog {
            runner_inventory: vec![runner_inventory_entry("alpha")],
            ..v7_catalog()
        };
        write_catalog(&path, &source);
        let before = fs::read(&path).unwrap();

        assert!(
            Catalog::load_with_migration(&path)
                .unwrap_err()
                .to_string()
                .contains("references an unknown profile")
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::remove_dir_all(directory).unwrap();
    }

    /// The restore itself, against a catalog that really was replaced. The
    /// commit path cannot leave a half-written file — the publish is a rename —
    /// so this is where the recovery is proven to move bytes.
    #[test]
    fn restoring_a_backup_puts_the_previous_catalog_back() {
        let directory = temporary_migration_directory("v8-restore-bytes");
        let path = directory.join("catalog.json");
        let backup = directory.join("catalog.json.v7.bak");
        let source = v7_catalog();
        write_catalog(&path, &source);
        fs::copy(&path, &backup).unwrap();

        fs::write(&path, b"{ this is not a catalog").unwrap();
        assert!(Catalog::load(&path).is_err());

        restore_catalog_backup(&backup, &path).unwrap();
        let restored: Catalog = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(restored, source);
        assert!(!directory.join("catalog.json.restoring").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_empty_backup_is_not_something_to_restore_from() {
        let directory = temporary_migration_directory("v8-empty-backup");
        let path = directory.join("catalog.json");
        let backup = directory.join("catalog.json.v7.bak");
        write_catalog(&path, &v7_catalog());
        fs::write(&backup, b"").unwrap();

        assert!(restore_catalog_backup(&backup, &path).is_err());
        // Read the document rather than loading it: `load` migrates, and what
        // matters here is that the bytes on disk were left alone.
        let on_disk: Catalog = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(on_disk.schema_version, SCHEMA_VERSION_V7);
        fs::remove_dir_all(directory).unwrap();
    }

    /// Promise 6: removing a plugin keeps the games it imported. A card whose
    /// profile is gone is an orphan that explains itself, not a catalog that
    /// refuses to load.
    #[test]
    fn a_third_party_runner_card_survives_the_loss_of_its_profile() {
        let mut catalog = catalog_with_runner_game();
        catalog.runner_profiles.clear();
        catalog.runner_inventory.clear();

        catalog.validate().unwrap();
        assert_eq!(catalog.games.len(), 1);
    }

    #[test]
    fn a_runner_card_whose_profile_exists_needs_its_private_inventory_entry() {
        let mut catalog = catalog_with_runner_game();
        catalog.runner_inventory.clear();

        assert!(
            catalog
                .validate()
                .unwrap_err()
                .to_string()
                .contains("missing its private inventory entry")
        );
    }

    #[test]
    fn a_runner_card_cannot_borrow_another_plugins_profile() {
        let mut catalog = catalog_with_runner_game();
        catalog.games[0].launch_target = LaunchTarget::Runner {
            runner_id: "com.orivo.other-runner".into(),
            game_ref: "alpha".into(),
            profile_id: "fixture-profile-1".into(),
        };

        assert!(
            catalog
                .validate()
                .unwrap_err()
                .to_string()
                .contains("belongs to another plugin")
        );
    }

    #[test]
    fn an_inventory_entry_outside_its_granted_folder_is_refused() {
        let mut catalog = Catalog::default();
        catalog.upsert_runner_profile(runner_profile()).unwrap();
        let mut entry = runner_inventory_entry("alpha");
        entry.game_path = PathBuf::from("/Games/Elsewhere/alpha.rom");

        assert!(
            catalog
                .upsert_runner_inventory(entry)
                .unwrap_err()
                .to_string()
                .contains("outside the folder it was granted under")
        );
    }

    #[test]
    fn an_inventory_entry_naming_a_folder_the_profile_does_not_grant_is_refused() {
        let mut catalog = Catalog::default();
        catalog.upsert_runner_profile(runner_profile()).unwrap();
        let mut entry = runner_inventory_entry("alpha");
        entry.directory_grant_id = "other-games".into();

        assert!(
            catalog
                .upsert_runner_inventory(entry)
                .unwrap_err()
                .to_string()
                .contains("no longer grants")
        );
    }

    /// The external reference is the key an idempotent import relies on, so two
    /// game references cannot claim the same one.
    #[test]
    fn two_game_references_cannot_claim_one_external_reference() {
        let mut catalog = catalog_with_runner_game();
        let mut second = runner_inventory_entry("beta");
        second.external_id = "alpha".into();

        assert!(
            catalog
                .upsert_runner_inventory(second)
                .unwrap_err()
                .to_string()
                .contains("already belongs to another game reference")
        );
    }

    #[test]
    fn a_profile_cannot_change_the_plugin_that_owns_it() {
        let mut catalog = catalog_with_runner_game();
        let stolen = RunnerProfile {
            plugin_id: "com.orivo.other-runner".into(),
            ..runner_profile()
        };

        assert!(
            catalog
                .upsert_runner_profile(stolen)
                .unwrap_err()
                .to_string()
                .contains("cannot change owner plugin")
        );
    }

    /// The ledger is an account of what was allowed and when, so putting a new
    /// scope in force retires the previous row rather than editing it.
    #[test]
    fn granting_a_capability_again_retires_the_row_it_replaces() {
        let mut catalog = Catalog::default();
        catalog
            .grant_plugin_capability(files_grant(&["a"], 10))
            .unwrap();
        catalog
            .grant_plugin_capability(files_grant(&["a", "b"], 20))
            .unwrap();

        assert_eq!(catalog.plugin_grants.len(), 2);
        assert_eq!(catalog.plugin_grants[0].revoked_at, Some(20));
        assert!(catalog.plugin_grants[1].is_active());
        assert_eq!(
            catalog
                .active_plugin_grant(FIXTURE_PLUGIN, PluginCapability::FilesRead)
                .map(|grant| grant.granted_at),
            Some(20)
        );
    }

    #[test]
    fn revoking_a_capability_keeps_its_row_and_its_dates() {
        let mut catalog = Catalog::default();
        catalog
            .grant_plugin_capability(files_grant(&["a"], 10))
            .unwrap();

        assert!(
            catalog
                .revoke_plugin_capability(FIXTURE_PLUGIN, PluginCapability::FilesRead, 30)
                .unwrap()
        );
        assert_eq!(catalog.plugin_grants.len(), 1);
        assert_eq!(catalog.plugin_grants[0].granted_at, 10);
        assert_eq!(catalog.plugin_grants[0].revoked_at, Some(30));
        assert!(
            catalog
                .active_plugin_grant(FIXTURE_PLUGIN, PluginCapability::FilesRead)
                .is_none()
        );
    }

    #[test]
    fn a_grant_whose_scope_does_not_match_its_capability_is_refused() {
        let mut catalog = Catalog::default();
        let mismatched = PluginGrantRecord {
            scope: CapabilityScope::Notifications,
            ..files_grant(&["a"], 10)
        };

        assert!(
            catalog
                .grant_plugin_capability(mismatched)
                .unwrap_err()
                .to_string()
                .contains("does not match its capability")
        );
    }

    #[test]
    fn a_grant_that_records_no_date_is_refused() {
        let mut catalog = Catalog::default();

        assert!(
            catalog
                .grant_plugin_capability(files_grant(&["a"], 0))
                .unwrap_err()
                .to_string()
                .contains("when it was granted")
        );
    }

    /// Revoking one folder is not deleting it. The profile keeps it, the games
    /// inside it stay in the library, and only the permission changes.
    #[test]
    fn revoking_one_folder_leaves_the_profile_and_its_games_intact() {
        let mut catalog = catalog_with_runner_game();
        catalog
            .allow_plugin_scope_value(
                FIXTURE_PLUGIN,
                PluginCapability::FilesRead,
                &directory_grant_key("fixture-profile-1", "fixture-games"),
                &package_identity(),
                10,
            )
            .unwrap();

        assert!(
            catalog
                .revoke_runner_directory("fixture-profile-1", "fixture-games", 40)
                .unwrap()
        );
        assert!(
            catalog
                .active_plugin_grant(FIXTURE_PLUGIN, PluginCapability::FilesRead)
                .is_none()
        );
        assert_eq!(
            catalog
                .runner_profile("fixture-profile-1")
                .unwrap()
                .game_directories
                .len(),
            1
        );
        assert_eq!(catalog.runner_inventory.len(), 1);
        assert_eq!(catalog.games.len(), 1);
    }

    /// The slot is the component's, so both profiles name their folder the
    /// same way. Keyed by profile *and* slot, revoking one says nothing about
    /// the other — which is the whole point of the composite key.
    #[test]
    fn revoking_one_profiles_folder_does_not_touch_another_profiles() {
        let mut catalog = catalog_with_runner_game();
        let second = RunnerProfile {
            id: "fixture-profile-2".into(),
            game_directories: vec![RunnerGrantedDirectory {
                id: "fixture-games".into(),
                path: PathBuf::from("/Games/MoreRoms"),
                device: None,
                inode: None,
            }],
            ..runner_profile()
        };
        catalog.upsert_runner_profile(second).unwrap();
        for profile in ["fixture-profile-1", "fixture-profile-2"] {
            catalog
                .allow_plugin_scope_value(
                    FIXTURE_PLUGIN,
                    PluginCapability::FilesRead,
                    &directory_grant_key(profile, "fixture-games"),
                    &package_identity(),
                    10,
                )
                .unwrap();
        }

        assert!(
            catalog
                .revoke_runner_directory("fixture-profile-1", "fixture-games", 40)
                .unwrap()
        );
        match &catalog
            .active_plugin_grant(FIXTURE_PLUGIN, PluginCapability::FilesRead)
            .expect("the other profile keeps what it was allowed")
            .scope
        {
            CapabilityScope::DirectoryGrants(keys) => assert_eq!(
                keys.iter().cloned().collect::<Vec<_>>(),
                vec![directory_grant_key("fixture-profile-2", "fixture-games")]
            ),
            scope => panic!("unexpected scope {scope:?}"),
        }
    }

    /// Allowing something is about that one thing. Nothing here restates a
    /// scope from what the catalog holds, so a folder taken away stays away.
    #[test]
    fn allowing_one_value_does_not_bring_back_another() {
        let mut catalog = catalog_with_runner_game();
        let identity = package_identity();
        let first = directory_grant_key("fixture-profile-1", "fixture-games");
        let second = directory_grant_key("fixture-profile-1", "extra");
        catalog
            .allow_plugin_scope_value(
                FIXTURE_PLUGIN,
                PluginCapability::FilesRead,
                &first,
                &identity,
                10,
            )
            .unwrap();
        catalog
            .revoke_plugin_scope_value(FIXTURE_PLUGIN, PluginCapability::FilesRead, &first, 20)
            .unwrap();
        catalog
            .allow_plugin_scope_value(
                FIXTURE_PLUGIN,
                PluginCapability::FilesRead,
                &second,
                &identity,
                30,
            )
            .unwrap();

        match &catalog
            .active_plugin_grant(FIXTURE_PLUGIN, PluginCapability::FilesRead)
            .unwrap()
            .scope
        {
            CapabilityScope::DirectoryGrants(keys) => {
                assert_eq!(keys.iter().cloned().collect::<Vec<_>>(), vec![second]);
            }
            scope => panic!("unexpected scope {scope:?}"),
        }
    }

    /// A permission given to one package does not join one given to another.
    /// The scope starts over rather than accumulating across an identity it
    /// never belonged to.
    #[test]
    fn a_value_allowed_to_another_package_does_not_join_this_ones_scope() {
        let mut catalog = Catalog::default();
        let signed = package_identity();
        let hand_loaded = PluginPackageIdentity {
            fingerprint: "b".repeat(64),
            trusted: false,
        };
        catalog
            .allow_plugin_scope_value(
                FIXTURE_PLUGIN,
                PluginCapability::FilesRead,
                "one",
                &signed,
                10,
            )
            .unwrap();
        catalog
            .allow_plugin_scope_value(
                FIXTURE_PLUGIN,
                PluginCapability::FilesRead,
                "two",
                &hand_loaded,
                20,
            )
            .unwrap();

        let active = catalog
            .active_plugin_grant(FIXTURE_PLUGIN, PluginCapability::FilesRead)
            .unwrap();
        assert!(!active.applies_to(&signed));
        assert!(active.applies_to(&hand_loaded));
        match &active.scope {
            CapabilityScope::DirectoryGrants(keys) => {
                assert_eq!(keys.iter().cloned().collect::<Vec<_>>(), vec!["two"]);
            }
            scope => panic!("unexpected scope {scope:?}"),
        }
    }

    /// A signed package may be updated under its signature and keep what it
    /// was allowed. One that arrived unsigned has no signer to vouch for a new
    /// build, so only the bytes it was allowed to count.
    #[test]
    fn a_grant_follows_the_signature_it_was_given_under() {
        let signed = package_identity();
        let updated = PluginPackageIdentity {
            fingerprint: "c".repeat(64),
            trusted: true,
        };
        let hand_loaded = PluginPackageIdentity {
            fingerprint: "b".repeat(64),
            trusted: false,
        };
        let grant = files_grant(&["one"], 10);
        assert!(grant.applies_to(&signed));
        assert!(grant.applies_to(&updated));
        assert!(!grant.applies_to(&hand_loaded));

        let unsigned_grant = PluginGrantRecord {
            package_fingerprint: Some(hand_loaded.fingerprint.clone()),
            package_trusted: Some(false),
            ..files_grant(&["one"], 10)
        };
        assert!(unsigned_grant.applies_to(&hand_loaded));
        assert!(!unsigned_grant.applies_to(&signed));
        assert!(!unsigned_grant.applies_to(&PluginPackageIdentity {
            fingerprint: "d".repeat(64),
            trusted: false,
        }));
    }

    #[test]
    fn a_grant_in_force_must_say_which_package_it_belongs_to() {
        let mut catalog = Catalog::default();
        let anonymous = PluginGrantRecord {
            package_fingerprint: None,
            package_trusted: None,
            ..files_grant(&["one"], 10)
        };

        assert!(
            catalog
                .grant_plugin_capability(anonymous)
                .unwrap_err()
                .to_string()
                .contains("record the package it was given to")
        );
    }

    /// Every permission goes; every profile and every game stays. That is the
    /// plan's promise 6 for a plugin that is removed.
    #[test]
    fn revoking_a_plugins_grants_leaves_its_profiles_and_games() {
        let mut catalog = catalog_with_runner_game();
        catalog
            .allow_plugin_scope_value(
                FIXTURE_PLUGIN,
                PluginCapability::FilesRead,
                &directory_grant_key("fixture-profile-1", "fixture-games"),
                &package_identity(),
                10,
            )
            .unwrap();
        catalog
            .allow_plugin_scope_value(
                FIXTURE_PLUGIN,
                PluginCapability::RunnerPrepare,
                "fixture-profile-1",
                &package_identity(),
                10,
            )
            .unwrap();

        assert!(catalog.revoke_plugin_grants(FIXTURE_PLUGIN, 50).unwrap());
        assert!(catalog.plugin_grants.iter().all(|grant| !grant.is_active()));
        assert_eq!(catalog.runner_profiles.len(), 1);
        assert_eq!(catalog.runner_inventory.len(), 1);
        assert_eq!(catalog.games.len(), 1);
    }

    /// Deleting a profile takes back what that profile authorised, and only
    /// that: another profile of the same plugin keeps its own folder even
    /// though both named it under the same slot.
    #[test]
    fn deleting_a_profile_takes_back_only_what_it_authorised() {
        let mut catalog = catalog_with_runner_game();
        let second = RunnerProfile {
            id: "fixture-profile-2".into(),
            game_directories: vec![RunnerGrantedDirectory {
                id: "fixture-games".into(),
                path: PathBuf::from("/Games/MoreRoms"),
                device: None,
                inode: None,
            }],
            ..runner_profile()
        };
        catalog.upsert_runner_profile(second).unwrap();
        for profile in ["fixture-profile-1", "fixture-profile-2"] {
            catalog
                .allow_plugin_scope_value(
                    FIXTURE_PLUGIN,
                    PluginCapability::FilesRead,
                    &directory_grant_key(profile, "fixture-games"),
                    &package_identity(),
                    10,
                )
                .unwrap();
            catalog
                .allow_plugin_scope_value(
                    FIXTURE_PLUGIN,
                    PluginCapability::RunnerPrepare,
                    profile,
                    &package_identity(),
                    10,
                )
                .unwrap();
        }

        assert!(
            catalog
                .remove_runner_profile("fixture-profile-1", 50)
                .unwrap()
        );
        assert_eq!(catalog.runner_profiles.len(), 1);
        assert!(catalog.runner_inventory.is_empty());
        assert!(catalog.games.is_empty());
        match &catalog
            .active_plugin_grant(FIXTURE_PLUGIN, PluginCapability::FilesRead)
            .expect("the other profile's folder is still allowed")
            .scope
        {
            CapabilityScope::DirectoryGrants(keys) => assert_eq!(
                keys.iter().cloned().collect::<Vec<_>>(),
                vec![directory_grant_key("fixture-profile-2", "fixture-games")]
            ),
            scope => panic!("unexpected scope {scope:?}"),
        }
        match &catalog
            .active_plugin_grant(FIXTURE_PLUGIN, PluginCapability::RunnerPrepare)
            .expect("the other profile may still be prepared")
            .scope
        {
            CapabilityScope::RunnerProfiles(ids) => {
                assert_eq!(
                    ids.iter().cloned().collect::<Vec<_>>(),
                    vec!["fixture-profile-2"]
                );
            }
            scope => panic!("unexpected scope {scope:?}"),
        }
    }

    #[test]
    fn a_plugin_capability_cannot_be_in_force_twice() {
        let mut catalog = Catalog::default();
        catalog.plugin_grants = vec![files_grant(&["a"], 10), files_grant(&["b"], 20)];

        assert!(
            catalog
                .validate()
                .unwrap_err()
                .to_string()
                .contains("granted twice at once")
        );
    }
    /// `CFBundleExecutable` is a string inside a file the host does not own. A
    /// bundle that names its executable with a path walks straight out of
    /// itself, and the only thing that stops it is refusing anything but a
    /// single ordinary component.
    #[test]
    fn a_bundle_naming_its_executable_outside_itself_is_refused() {
        let root = temporary_migration_directory("bundle-escape");
        let bundle = root.join("Escape.app");
        fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        fs::write(bundle.join("Contents/MacOS/Escape"), b"").unwrap();
        for name in ["../../../bin/sh", "/bin/sh", "..", "sub/dir", ""] {
            fs::write(
                bundle.join("Contents/Info.plist"),
                format!(
                    "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
                     <key>CFBundleExecutable</key><string>{name}</string></dict></plist>"
                ),
            )
            .unwrap();
            assert!(
                resolve_executable(&bundle).is_err(),
                "{name} should not resolve out of the bundle"
            );
        }

        // The ordinary case still works.
        fs::write(
            bundle.join("Contents/Info.plist"),
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
             <key>CFBundleExecutable</key><string>Escape</string></dict></plist>",
        )
        .unwrap();
        assert_eq!(
            resolve_executable(&bundle).unwrap(),
            bundle.join("Contents/MacOS/Escape")
        );
        fs::remove_dir_all(root).unwrap();
    }
    /// A name inside the bundle is still only a name: `Contents/MacOS` may hold
    /// a symbolic link, and following it out of the bundle is the same escape as
    /// spelling a path in the plist.
    #[cfg(unix)]
    #[test]
    fn a_bundle_executable_that_links_out_of_the_bundle_is_refused() {
        let root = temporary_migration_directory("bundle-link");
        let bundle = root.join("Linked.app");
        fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        fs::write(root.join("elsewhere"), b"").unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), bundle.join("Contents/MacOS/Linked"))
            .unwrap();
        fs::write(
            bundle.join("Contents/Info.plist"),
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
             <key>CFBundleExecutable</key><string>Linked</string></dict></plist>",
        )
        .unwrap();

        assert!(resolve_executable(&bundle).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
