//! Store catalog: derived, rebuildable cache plus provider adapters.
//!
//! Owned by the Store agent. Commands are registered by the shell in `lib.rs`.
//!
//! Three boundaries are deliberate here:
//!
//! * **Persistence is derived only.** Everything this module writes lives in a
//!   single versioned JSON document that can be deleted at any time without
//!   losing user data. All of it is funnelled through [`StoreCache`], so the
//!   later move to SQLite touches that struct and nothing else.
//! * **Providers are isolated.** A provider that fails, times out, or has no
//!   authorized feed degrades into its own [`ProviderStatus`]; it can never
//!   fail the whole response. Prices are only ever reported when a provider
//!   returned them. A missing price stays `null`, never a guess.
//! * **The WebView never names a destination.** [`open_store_offer`] accepts an
//!   opaque offer identifier, resolves the URL host-side, and then requires
//!   `https` plus a host on a hardcoded allowlist before anything is opened.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    future::Future,
    io::{self, Write as _},
    path::{Path, PathBuf},
    pin::Pin,
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

/// Derived cache document. The plan named a SQLite database; this crate has no
/// SQLite dependency, so the same contract is met by a versioned JSON document
/// with an atomic writer. Unknown or corrupt content is discarded and rebuilt.
const STORE_CACHE_FILE: &str = "store-cache.json";
/// Bumped when the meaning of a cached row changes. Version 2 is the curated
/// catalogue with `curation` copy: a version 1 document holds the old
/// ten-game editorial shelf, which `store_home` would otherwise keep serving
/// (with no French copy) until a refresh happened to succeed.
const STORE_CACHE_SCHEMA_VERSION: u32 = 2;
const PREFERENCES_FILE: &str = "preferences.json";

/// An offer older than this is reported as stale rather than presented as a
/// current price.
const OFFER_FRESHNESS: Duration = Duration::from_secs(24 * 60 * 60);
const MIN_PERSONALIZED_PLAYED_GAMES: usize = 3;
const MAX_BROWSE_LIMIT: usize = 60;
const DEFAULT_BROWSE_LIMIT: usize = 24;
const MAX_QUERY_LENGTH: usize = 200;
const MAX_OFFER_ID_LENGTH: usize = 128;
const MAX_RECOMMENDATION_REASONS: usize = 3;
const BROWSE_CURSOR_PREFIX: &str = "store_";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const USER_AGENT: &str = "Orivo/0.3 (+https://orivo.io)";

/// The whole curated catalogue is refreshable, with headroom for the rows a
/// later catalogue adds. The cap is a guard against an unbounded fan-out, not
/// a reason for the last games on the shelf to keep a stale price.
const MAX_STEAM_REFRESH_APPS: usize = 128;
/// Steam answers a comma-separated `appids` list in one response as long as a
/// `filters` value is supplied, so the catalogue costs a handful of requests
/// instead of one per game. See `refresh_steam` for what was measured.
const STEAM_REFRESH_BATCH: usize = 20;
/// Only paid between the per-app fallback requests, which is the path that can
/// actually fan out. The batched path never sleeps.
const STEAM_REFRESH_PAUSE: Duration = Duration::from_millis(250);
/// After this many consecutive unanswered fallback requests, Steam is not
/// reachable rather than slow, and the refresh stops instead of spending a
/// request timeout per remaining game.
const STEAM_REFRESH_GIVE_UP_AFTER: usize = 5;
const APPLE_REFRESH_TERM: &str = "game";
const APPLE_REFRESH_LIMIT: usize = 10;

/// Host-side configuration only. The WebView cannot influence any of these.
const STORE_REGION_ENV: &str = "ORIVO_STORE_REGION";
const DEFAULT_REGION: &str = "US";

/// The only hosts Orivo will ever hand to the platform opener. Anything else,
/// including a redirect target or a value recovered from a tampered cache
/// file, is refused.
const ALLOWED_OFFER_HOSTS: &[&str] = &[
    "store.steampowered.com",
    "store.ubisoft.com",
    "apps.microsoft.com",
    "apps.apple.com",
    "play.google.com",
    "www.instant-gaming.com",
    "store.epicgames.com",
    "www.gog.com",
    "www.humblebundle.com",
    "www.fanatical.com",
    "www.greenmangaming.com",
    "store.playstation.com",
    "www.nintendo.com",
];

/// The only remote image origin in the application CSP. A provider artwork URL
/// on any other host cannot render anyway, and persisting it would let a store
/// response choose an origin the WebView contacts.
const ALLOWED_ARTWORK_HOSTS: &[&str] = &["cdn.cloudflare.steamstatic.com"];

// ---------------------------------------------------------------------------
// Contract types. Field names and enum spellings mirror `src/contracts.ts`.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StoreProviderId {
    Steam,
    InstantGaming,
    Epic,
    Gog,
    Humble,
    Fanatical,
    GreenManGaming,
    Ubisoft,
    Microsoft,
    /// `PlayStation` kebab-cases to `play-station`, which is not the contract
    /// spelling, so the wire name is pinned explicitly.
    #[serde(rename = "playstation")]
    PlayStation,
    Nintendo,
    Apple,
    GooglePlay,
}

impl StoreProviderId {
    const ALL: [Self; 13] = [
        Self::Steam,
        Self::InstantGaming,
        Self::Epic,
        Self::Gog,
        Self::Humble,
        Self::Fanatical,
        Self::GreenManGaming,
        Self::Ubisoft,
        Self::Microsoft,
        Self::PlayStation,
        Self::Nintendo,
        Self::Apple,
        Self::GooglePlay,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Steam => "Steam",
            Self::InstantGaming => "Instant Gaming",
            Self::Epic => "Epic Games Store",
            Self::Gog => "GOG",
            Self::Humble => "Humble Bundle",
            Self::Fanatical => "Fanatical",
            Self::GreenManGaming => "Green Man Gaming",
            Self::Ubisoft => "Ubisoft",
            Self::Microsoft => "Microsoft/Xbox",
            Self::PlayStation => "PlayStation Store",
            Self::Nintendo => "Nintendo eShop",
            Self::Apple => "Apple App Store",
            Self::GooglePlay => "Google Play",
        }
    }

    /// The token the frontend builds its filter route from.
    ///
    /// Test-only, and deliberately written out by hand rather than derived:
    /// its whole job is to be compared against what `serde` actually
    /// serialises, so a rename that changes the wire format and not this list
    /// fails the test instead of silently breaking every saved Store URL.
    #[cfg(test)]
    fn slug(self) -> &'static str {
        match self {
            Self::Steam => "steam",
            Self::InstantGaming => "instant-gaming",
            Self::Epic => "epic",
            Self::Gog => "gog",
            Self::Humble => "humble",
            Self::Fanatical => "fanatical",
            Self::GreenManGaming => "green-man-gaming",
            Self::Ubisoft => "ubisoft",
            Self::Microsoft => "microsoft",
            Self::PlayStation => "playstation",
            Self::Nintendo => "nintendo",
            Self::Apple => "apple",
            Self::GooglePlay => "google-play",
        }
    }

    /// The hardware platform a storefront sells for. Apple and Google Play sell
    /// for mobile devices, which the store platform filter does not model, so
    /// they map to `None` rather than to a guess.
    fn platform(self) -> Option<StorePlatformId> {
        match self {
            Self::Steam
            | Self::InstantGaming
            | Self::Epic
            | Self::Gog
            | Self::Humble
            | Self::Fanatical
            | Self::GreenManGaming
            | Self::Ubisoft => Some(StorePlatformId::Pc),
            Self::Microsoft => Some(StorePlatformId::Xbox),
            Self::PlayStation => Some(StorePlatformId::PlayStation),
            Self::Nintendo => Some(StorePlatformId::Switch),
            Self::Apple | Self::GooglePlay => None,
        }
    }
}

/// The platform pill group in the store filter bar. It describes hardware, not
/// a storefront: several providers map onto the same platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StorePlatformId {
    Pc,
    /// Pinned for the same reason as [`StoreProviderId::PlayStation`].
    #[serde(rename = "playstation")]
    PlayStation,
    Xbox,
    Switch,
    Emulators,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderHealth {
    Available,
    Degraded,
    Unavailable,
    NotConfigured,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StoreCategory {
    #[default]
    ForYou,
    GoodForBrain,
    ShortSessions,
    StrongStories,
    Relaxing,
    AllGames,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfferAvailability {
    Available,
    Unavailable,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameSourceKind {
    Steam,
    Wine,
    Local,
    Showcase,
    #[default]
    Store,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GamePlatform {
    Windows,
    Macos,
    Linux,
    Ios,
    Android,
}

impl GamePlatform {
    /// A game's declared platform expressed as a store filter platform. Mobile
    /// platforms have no pill in the filter bar, so they map to `None`; nothing
    /// is invented to make a game match a filter it does not satisfy.
    fn store_platform(self) -> Option<StorePlatformId> {
        match self {
            Self::Windows | Self::Macos | Self::Linux => Some(StorePlatformId::Pc),
            Self::Ios | Self::Android => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecommendationMode {
    Editorial,
    Personalized,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreOffer {
    pub id: String,
    pub game_id: String,
    pub provider: StoreProviderId,
    pub provider_label: String,
    /// Minor currency units exactly as the provider reported them. `None` when
    /// no provider returned a price; it is never inferred.
    pub price_minor: Option<i64>,
    pub currency: Option<String>,
    pub region: String,
    pub verified_at: Option<String>,
    pub availability: OfferAvailability,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub provider: StoreProviderId,
    pub label: String,
    pub health: ProviderHealth,
    pub message: String,
    pub refreshed_at: Option<String>,
}

/// One "fit" read-out row on a store card: a French label and a 1-5 strength.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreFitStat {
    pub label: String,
    pub value: i64,
}

/// One row of the store hero's right-hand panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreHighlight {
    pub icon: String,
    pub title: String,
    pub text: String,
}

/// The French editorial copy Orivo writes about a catalogue game. It is
/// carried through from the generated catalogue untouched: the shell neither
/// authors nor edits it, it only makes sure the card that renders it gets it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreCuration {
    #[serde(default, deserialize_with = "lenient_list")]
    pub genres: Vec<String>,
    #[serde(default)]
    pub duration: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default, deserialize_with = "lenient_list")]
    pub stats: Vec<StoreFitStat>,
    #[serde(default)]
    pub tagline: String,
    #[serde(default)]
    pub hero_title: String,
    #[serde(default)]
    pub hero_lead: String,
    #[serde(default, deserialize_with = "lenient_list")]
    pub highlights: Vec<StoreHighlight>,
    /// A curator's own category assignment. It is the authority for the store
    /// rails; keyword matching is only the fallback for a game without one.
    #[serde(default, deserialize_with = "lenient_list")]
    pub categories: Vec<StoreCategory>,
    #[serde(default, deserialize_with = "lenient_list")]
    pub platforms: Vec<StorePlatformId>,
}

/// Deserialize a list, dropping only the entries this build cannot understand.
/// A catalogue written by a newer generator — a category or a storefront this
/// binary has never heard of — must degrade to the rows it does understand
/// rather than emptying the whole store.
fn lenient_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let rows = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| serde_json::from_value(row).ok())
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameSummary {
    pub id: String,
    pub title: String,
    pub source: GameSourceKind,
    pub short_description: String,
    pub cover_url: String,
    pub hero_url: String,
    pub landscape_url: String,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub supported_platforms: Vec<GamePlatform>,
    pub owned: bool,
    pub launchable: bool,
    pub wishlisted: bool,
    pub play_time_seconds: u64,
    pub last_played_at: Option<String>,
    pub recommendation_reasons: Vec<String>,
    pub offers: Vec<StoreOffer>,
    /// Present only for the catalogue games Orivo has written copy for, which
    /// is what `curation?:` means in `src/contracts.ts`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curation: Option<StoreCuration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreHomeView {
    pub games: Vec<GameSummary>,
    pub provider_statuses: Vec<ProviderStatus>,
    pub recommendation_mode: RecommendationMode,
    pub recommendation_heading: String,
    pub refreshed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreBrowseRequest {
    #[serde(default)]
    pub category: StoreCategory,
    #[serde(default)]
    pub providers: Vec<StoreProviderId>,
    #[serde(default)]
    pub platforms: Vec<StorePlatformId>,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreBrowsePage {
    pub games: Vec<GameSummary>,
    pub next_cursor: Option<String>,
    pub provider_statuses: Vec<ProviderStatus>,
}

// ---------------------------------------------------------------------------
// Cached representation. Everything here is derived and rebuildable.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedOffer {
    id: String,
    game_id: String,
    provider: StoreProviderId,
    #[serde(default)]
    price_minor: Option<i64>,
    #[serde(default)]
    currency: Option<String>,
    region: String,
    #[serde(default)]
    verified_at_epoch_ms: Option<u64>,
    #[serde(default)]
    availability: OfferAvailability,
    #[serde(default)]
    discount_percent: u32,
    /// Resolved host-side only. It is never projected into a view model; the
    /// WebView addresses an offer exclusively by its opaque `id`.
    url: String,
}

impl CachedOffer {
    fn to_dto(&self, now_ms: u64) -> StoreOffer {
        StoreOffer {
            id: self.id.clone(),
            game_id: self.game_id.clone(),
            provider: self.provider,
            provider_label: self.provider.label().to_string(),
            price_minor: self.price_minor,
            currency: self.currency.clone(),
            region: self.region.clone(),
            verified_at: self.verified_at_epoch_ms.map(iso8601_from_epoch_ms),
            availability: self.availability,
            stale: is_stale(self.verified_at_epoch_ms, now_ms),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedGame {
    id: String,
    title: String,
    #[serde(default)]
    short_description: String,
    #[serde(default)]
    cover_url: String,
    #[serde(default)]
    hero_url: String,
    #[serde(default)]
    landscape_url: String,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    supported_platforms: Vec<GamePlatform>,
    /// Factual editorial copy used when no personalized signal exists.
    #[serde(default)]
    editorial_reasons: Vec<String>,
    #[serde(default)]
    offers: Vec<CachedOffer>,
    /// Carried from the generated catalogue and handed to the WebView
    /// unchanged, so the cards keep the exact French copy the design approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    curation: Option<StoreCuration>,
}

impl CachedGame {
    fn to_summary(&self, now_ms: u64, reasons: Vec<String>) -> GameSummary {
        GameSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            source: GameSourceKind::Store,
            short_description: self.short_description.clone(),
            cover_url: self.cover_url.clone(),
            hero_url: self.hero_url.clone(),
            landscape_url: self.landscape_url.clone(),
            genres: self.genres.clone(),
            tags: self.tags.clone(),
            supported_platforms: self.supported_platforms.clone(),
            owned: false,
            launchable: false,
            wishlisted: false,
            play_time_seconds: 0,
            last_played_at: None,
            recommendation_reasons: reasons,
            offers: self
                .offers
                .iter()
                .map(|offer| offer.to_dto(now_ms))
                .collect(),
            curation: self.curation.clone(),
        }
    }

    fn supports_macos(&self) -> bool {
        self.supported_platforms.contains(&GamePlatform::Macos)
    }

    fn best_discount_percent(&self) -> u32 {
        self.offers
            .iter()
            .map(|offer| offer.discount_percent)
            .max()
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedProviderStatus {
    provider: StoreProviderId,
    health: ProviderHealth,
    message: String,
    #[serde(default)]
    refreshed_at_epoch_ms: Option<u64>,
}

impl CachedProviderStatus {
    fn to_dto(&self) -> ProviderStatus {
        ProviderStatus {
            provider: self.provider,
            label: self.provider.label().to_string(),
            health: self.health,
            message: self.message.clone(),
            refreshed_at: self.refreshed_at_epoch_ms.map(iso8601_from_epoch_ms),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoreCacheDocument {
    schema_version: u32,
    #[serde(default)]
    refreshed_at_epoch_ms: Option<u64>,
    #[serde(default)]
    games: Vec<CachedGame>,
    #[serde(default)]
    provider_statuses: Vec<CachedProviderStatus>,
}

impl Default for StoreCacheDocument {
    fn default() -> Self {
        Self {
            schema_version: STORE_CACHE_SCHEMA_VERSION,
            refreshed_at_epoch_ms: None,
            games: Vec::new(),
            provider_statuses: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Persistence. The single seam between the store and its storage engine.
// ---------------------------------------------------------------------------

/// The only component in this module that touches the filesystem for cache
/// data. Swapping the JSON document for SQLite means reimplementing these four
/// methods and nothing else.
#[derive(Debug, Clone)]
pub struct StoreCache {
    path: PathBuf,
}

impl StoreCache {
    pub fn new(app_data_dir: impl AsRef<Path>) -> Self {
        Self {
            path: app_data_dir.as_ref().join(STORE_CACHE_FILE),
        }
    }

    /// Read the cache. A missing, unreadable, corrupt, or unknown-version
    /// document is not an error: it is discarded and the caller rebuilds it.
    fn read(&self) -> StoreCacheDocument {
        let Ok(encoded) = fs::read_to_string(&self.path) else {
            return StoreCacheDocument::default();
        };
        match serde_json::from_str::<StoreCacheDocument>(&encoded) {
            Ok(document) if document.schema_version == STORE_CACHE_SCHEMA_VERSION => document,
            _ => StoreCacheDocument::default(),
        }
    }

    /// Replace the cache atomically so a crash mid-write can never leave a
    /// half-written document behind.
    fn write(&self, document: &StoreCacheDocument) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "Orivo could not resolve its store cache directory.".to_string())?;
        fs::create_dir_all(parent).map_err(cache_error)?;
        let temporary = parent.join(format!(
            ".{STORE_CACHE_FILE}.{}.{}.tmp",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let result = (|| -> Result<(), io::Error> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            serde_json::to_writer(&mut file, document).map_err(io::Error::other)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(cache_error)
    }

    /// Resolve an opaque offer identifier to its provider URL. Falls back to
    /// the built-in editorial offers so that deleting the cache never breaks
    /// the store.
    fn offer_url(&self, offer_id: &str) -> Option<String> {
        find_offer_url(&self.read().games, offer_id)
            .or_else(|| find_offer_url(catalog_games(), offer_id))
    }
}

fn find_offer_url(games: &[CachedGame], offer_id: &str) -> Option<String> {
    games
        .iter()
        .flat_map(|game| game.offers.iter())
        .find(|offer| offer.id == offer_id)
        .map(|offer| offer.url.clone())
}

fn cache_error(error: impl std::fmt::Display) -> String {
    format!("Orivo could not update its store cache: {error}")
}

// ---------------------------------------------------------------------------
// The catalogue. One curated list, generated once by
// `scripts/build-store-catalog.mjs`, written to `src/store-catalog.generated.ts`
// for the WebView and to the JSON sibling embedded here for the shell. Neither
// side can drift onto a catalogue of its own: this is the same bytes.
// ---------------------------------------------------------------------------

const STORE_CATALOG_JSON: &str = include_str!("../resources/store-catalog.json");

static STORE_CATALOG: OnceLock<Vec<CachedGame>> = OnceLock::new();

/// One row of the generated catalogue, in the exact shape the generator emits
/// (`GameSummary` from `src/contracts.ts`). Only the fields the shell can act
/// on are read; `owned`, `wishlisted` and the play-time fields are library
/// facts the store never sources from a catalogue file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogSeed {
    id: String,
    title: String,
    #[serde(default)]
    short_description: String,
    #[serde(default)]
    cover_url: String,
    #[serde(default)]
    hero_url: String,
    #[serde(default)]
    landscape_url: String,
    #[serde(default, deserialize_with = "lenient_list")]
    genres: Vec<String>,
    #[serde(default, deserialize_with = "lenient_list")]
    tags: Vec<String>,
    #[serde(default, deserialize_with = "lenient_list")]
    supported_platforms: Vec<GamePlatform>,
    #[serde(default, deserialize_with = "lenient_list")]
    recommendation_reasons: Vec<String>,
    #[serde(default, deserialize_with = "lenient_list")]
    offers: Vec<CatalogSeedOffer>,
    #[serde(default)]
    curation: Option<StoreCuration>,
}

/// A catalogue offer. The price the generator recorded is a price a provider
/// actually quoted, so it is carried through byte for byte — including the
/// nulls. Nothing here is converted, rounded, or filled in.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogSeedOffer {
    id: String,
    provider: StoreProviderId,
    #[serde(default)]
    price_minor: Option<i64>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    verified_at: Option<String>,
    #[serde(default)]
    availability: OfferAvailability,
}

impl CatalogSeed {
    fn into_cached_game(self) -> Option<CachedGame> {
        if self.id.trim().is_empty() || self.title.trim().is_empty() {
            return None;
        }
        // Only a Steam catalogue id names a page Orivo can state as a fact. A
        // row keyed any other way still sells, it just has nowhere to send the
        // shopper until the catalogue carries that storefront's identifier.
        let app_id = steam_app_id(&self.id);
        let offers = self
            .offers
            .into_iter()
            .map(|offer| CachedOffer {
                id: offer.id,
                game_id: self.id.clone(),
                provider: offer.provider,
                price_minor: offer.price_minor,
                currency: offer.currency,
                region: offer.region.unwrap_or_else(|| DEFAULT_REGION.to_string()),
                verified_at_epoch_ms: offer.verified_at.as_deref().and_then(epoch_ms_from_iso8601),
                availability: offer.availability,
                discount_percent: 0,
                // Orivo only opens a page it can name from a fact it holds.
                // The Steam app page is one; the other storefronts sell the
                // same game under identifiers Orivo was never given, so those
                // rows carry their quoted price and no destination rather than
                // a search URL dressed up as a product page.
                url: match (offer.provider, app_id.as_deref()) {
                    (StoreProviderId::Steam, Some(app_id)) => steam_app_url(app_id),
                    _ => String::new(),
                },
            })
            .collect();
        Some(CachedGame {
            id: self.id,
            title: self.title,
            short_description: self.short_description,
            cover_url: validated_artwork_url(&self.cover_url),
            hero_url: validated_artwork_url(&self.hero_url),
            landscape_url: validated_artwork_url(&self.landscape_url),
            genres: self.genres,
            tags: self.tags,
            supported_platforms: self.supported_platforms,
            editorial_reasons: self.recommendation_reasons,
            offers,
            curation: self.curation,
        })
    }
}

/// The numeric part of a `steam:<appid>` catalogue identifier.
fn steam_app_id(game_id: &str) -> Option<String> {
    let app_id = game_id.strip_prefix("steam:")?;
    (!app_id.is_empty() && app_id.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| app_id.to_string())
}

/// The embedded catalogue, parsed once. A row this build cannot read is
/// dropped rather than taking the shelf down with it; the test below pins the
/// count, so a catalogue that stops parsing fails the build, not the user.
fn catalog_games() -> &'static [CachedGame] {
    STORE_CATALOG.get_or_init(|| parse_catalog(STORE_CATALOG_JSON))
}

/// The presentation a Store card carries. A host feature that installs a
/// Store game reuses it so the library card keeps the catalogue's artwork and
/// copy instead of falling back to the Windows binary's own icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePresentation {
    pub title: String,
    pub short_description: String,
    pub cover_url: String,
    pub hero_url: String,
    pub landscape_url: String,
}

pub fn presentation_for(game_id: &str) -> Option<StorePresentation> {
    catalog_games()
        .iter()
        .find(|game| game.id == game_id)
        .map(|game| StorePresentation {
            title: game.title.clone(),
            short_description: game.short_description.clone(),
            cover_url: game.cover_url.clone(),
            hero_url: game.hero_url.clone(),
            landscape_url: game.landscape_url.clone(),
        })
}

fn parse_catalog(encoded: &str) -> Vec<CachedGame> {
    serde_json::from_str::<Vec<serde_json::Value>>(encoded)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| serde_json::from_value::<CatalogSeed>(row).ok())
        .filter_map(CatalogSeed::into_cached_game)
        .collect()
}

fn steam_app_url(app_id: &str) -> String {
    format!("https://store.steampowered.com/app/{app_id}/")
}

/// The baseline status for a provider Orivo has no authorized feed for. These
/// are factual statements about configuration, not placeholders for data.
fn unconfigured_status(provider: StoreProviderId) -> CachedProviderStatus {
    let (health, message) = match provider {
        // Steam's storefront price endpoint is public, so there is nothing to
        // configure: the prices are simply the ones the catalogue was built
        // with until the first refresh lands.
        StoreProviderId::Steam => (
            ProviderHealth::Degraded,
            "Steam prices are the ones this catalogue shipped with until the next refresh.",
        ),
        StoreProviderId::Ubisoft => (
            ProviderHealth::Unavailable,
            "No authorized Ubisoft catalog feed is configured, so no Ubisoft prices are shown.",
        ),
        StoreProviderId::Microsoft => (
            ProviderHealth::Unavailable,
            "A licensed XStore context is required before Microsoft Store prices can be read.",
        ),
        StoreProviderId::Apple => (
            ProviderHealth::Degraded,
            "Live App Store search will appear when a network connection is available.",
        ),
        StoreProviderId::GooglePlay => (
            ProviderHealth::NotConfigured,
            "Registered third-party Google Play access is required; no public catalog feed exists.",
        ),
        StoreProviderId::InstantGaming => (
            ProviderHealth::Unavailable,
            "No authorized Instant Gaming feed is configured, so no Instant Gaming prices are shown.",
        ),
        StoreProviderId::Epic => (
            ProviderHealth::Unavailable,
            "No authorized Epic Games Store feed is configured, so no Epic prices are shown.",
        ),
        StoreProviderId::Gog => (
            ProviderHealth::Unavailable,
            "No authorized GOG catalog feed is configured, so no GOG prices are shown.",
        ),
        StoreProviderId::Humble => (
            ProviderHealth::Unavailable,
            "No authorized Humble Bundle feed is configured, so no Humble Bundle prices are shown.",
        ),
        StoreProviderId::Fanatical => (
            ProviderHealth::Unavailable,
            "No authorized Fanatical feed is configured, so no Fanatical prices are shown.",
        ),
        StoreProviderId::GreenManGaming => (
            ProviderHealth::Unavailable,
            "No authorized Green Man Gaming feed is configured, so no Green Man Gaming prices are shown.",
        ),
        StoreProviderId::PlayStation => (
            ProviderHealth::NotConfigured,
            "Registered PlayStation Store partner access is required; no public catalog feed exists.",
        ),
        StoreProviderId::Nintendo => (
            ProviderHealth::NotConfigured,
            "Registered Nintendo eShop partner access is required; no public catalog feed exists.",
        ),
    };
    CachedProviderStatus {
        provider,
        health,
        message: message.to_string(),
        refreshed_at_epoch_ms: None,
    }
}

fn default_provider_statuses() -> Vec<CachedProviderStatus> {
    StoreProviderId::ALL
        .into_iter()
        .map(unconfigured_status)
        .collect()
}

// ---------------------------------------------------------------------------
// Recommendations. Rule-based, explainable, and derived only from facts the
// backend can point at: played genres, declared platforms, existing tags, and
// a discount a provider actually reported.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayedGame {
    pub title: String,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub play_time_seconds: u64,
    pub last_played_at: Option<String>,
}

impl PlayedGame {
    fn is_played(&self) -> bool {
        self.play_time_seconds > 0 || self.last_played_at.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryProfile {
    played_count: usize,
    genre_weights: BTreeMap<String, u64>,
    short_session_affinity: bool,
    story_affinity: bool,
}

impl LibraryProfile {
    pub fn from_played(games: &[PlayedGame]) -> Self {
        let played: Vec<&PlayedGame> = games.iter().filter(|game| game.is_played()).collect();
        let mut genre_weights: BTreeMap<String, u64> = BTreeMap::new();
        let mut short_sessions = 0usize;
        let mut stories = 0usize;
        for game in &played {
            for genre in &game.genres {
                let key = normalize(genre);
                if key.is_empty() {
                    continue;
                }
                *genre_weights.entry(key).or_default() += 1;
            }
            let facts = normalize(&[game.tags.join(" "), game.genres.join(" ")].join(" "));
            // Under ten recorded hours is treated as a short-session library
            // signal; it is a fact about the recorded play time, not a claim
            // about the player.
            if facts.contains("short session") || game.play_time_seconds < 10 * 60 * 60 {
                short_sessions += 1;
            }
            if facts.contains("story")
                || facts.contains("rpg")
                || facts.contains("adventure")
                || facts.contains("narrative")
            {
                stories += 1;
            }
        }
        let played_count = played.len();
        Self {
            played_count,
            genre_weights,
            short_session_affinity: played_count > 0 && short_sessions * 2 > played_count,
            story_affinity: played_count > 0 && stories * 2 > played_count,
        }
    }

    pub fn is_personalized(&self) -> bool {
        self.played_count >= MIN_PERSONALIZED_PLAYED_GAMES
    }

    fn genre_weight(&self, genre: &str) -> u64 {
        self.genre_weights
            .get(&normalize(genre))
            .copied()
            .unwrap_or(0)
    }

    /// Score plus the factual reasons behind it. Reasons are always derived
    /// from a matched fact, so the UI can never show an unexplained pick.
    fn score(&self, game: &CachedGame) -> (i64, Vec<String>) {
        let mut score = 0i64;
        let mut reasons = Vec::new();

        let mut matched: Vec<(&String, u64)> = game
            .genres
            .iter()
            .map(|genre| (genre, self.genre_weight(genre)))
            .filter(|(_, weight)| *weight > 0)
            .collect();
        matched.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
        for (genre, weight) in &matched {
            score += 4 * (*weight as i64);
            reasons.push(format!(
                "Because you play {} games",
                genre.to_lowercase().trim()
            ));
        }

        if game.supports_macos() {
            score += 3;
            reasons.push("Runs natively on macOS".to_string());
        }

        let facts = normalize(&[game.tags.join(" "), game.genres.join(" ")].join(" "));
        if self.short_session_affinity && facts.contains("short session") {
            score += 3;
            reasons.push("Works in short sessions".to_string());
        }
        if self.story_affinity && (facts.contains("strong stor") || facts.contains("story")) {
            score += 3;
            reasons.push("Story-driven, like games you have played".to_string());
        }

        let discount = game.best_discount_percent();
        if discount > 0 {
            score += 2;
            reasons.push(format!("Discounted by {discount}% right now"));
        }

        reasons.truncate(MAX_RECOMMENDATION_REASONS);
        (score, reasons)
    }
}

/// Rank the pool and attach reasons. With fewer than three played games there
/// is nothing to personalize from, so the editorial order and copy are used.
fn recommend(
    games: &[CachedGame],
    profile: &LibraryProfile,
    now_ms: u64,
) -> (Vec<GameSummary>, RecommendationMode, String) {
    if !profile.is_personalized() {
        let summaries = games
            .iter()
            .map(|game| game.to_summary(now_ms, game.editorial_reasons.clone()))
            .collect();
        return (
            summaries,
            RecommendationMode::Editorial,
            "Editorial picks".to_string(),
        );
    }

    let mut scored: Vec<(i64, &CachedGame, Vec<String>)> = games
        .iter()
        .map(|game| {
            let (score, mut reasons) = profile.score(game);
            if reasons.is_empty() {
                reasons = game.editorial_reasons.clone();
            }
            (score, game, reasons)
        })
        .collect();
    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.title.cmp(&right.1.title))
    });
    let summaries = scored
        .into_iter()
        .map(|(_, game, reasons)| game.to_summary(now_ms, reasons))
        .collect();
    (
        summaries,
        RecommendationMode::Personalized,
        "Picked from the games you play".to_string(),
    )
}

/// Read the library snapshot tolerantly. This deliberately does not link
/// against the catalog module: the store only needs a few public facts, and a
/// missing or unreadable snapshot simply means "no personalization".
fn read_played_games(app_data_dir: &Path) -> Vec<PlayedGame> {
    #[derive(Deserialize)]
    struct LibrarySnapshot {
        #[serde(default)]
        games: Vec<LibraryGameSnapshot>,
    }
    #[derive(Deserialize)]
    struct LibraryGameSnapshot {
        #[serde(default)]
        title: String,
        #[serde(default)]
        play_time_seconds: u64,
        #[serde(default)]
        last_played_at: Option<String>,
        /// Genres arrive as the catalog's Steam storefront extra, a
        /// comma-separated string. Anything else is ignored.
        #[serde(default, rename = "orivo_steam_genre")]
        steam_genre: Option<String>,
    }

    let Ok(encoded) = fs::read_to_string(app_data_dir.join("catalog.json")) else {
        return Vec::new();
    };
    let Ok(snapshot) = serde_json::from_str::<LibrarySnapshot>(&encoded) else {
        return Vec::new();
    };
    snapshot
        .games
        .into_iter()
        .map(|game| PlayedGame {
            title: game.title,
            genres: game
                .steam_genre
                .unwrap_or_default()
                .split(',')
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect(),
            tags: Vec::new(),
            play_time_seconds: game.play_time_seconds,
            last_played_at: game.last_played_at,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Browsing. Pure filtering over the cached pool; it never touches the network.
// ---------------------------------------------------------------------------

fn normalize(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| character.to_lowercase())
        .map(fold_diacritic)
        .collect::<String>()
        .trim()
        .to_string()
}

fn fold_diacritic(character: char) -> char {
    match character {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'ç' => 'c',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

fn matches_category(game: &CachedGame, category: StoreCategory) -> bool {
    if matches!(category, StoreCategory::ForYou | StoreCategory::AllGames) {
        return true;
    }
    // A curator placed the catalogue games on their shelves by hand. That
    // assignment is the answer where it exists; keyword matching is only for
    // the rows a live provider contributed, which nobody has curated.
    if let Some(curation) = &game.curation
        && !curation.categories.is_empty()
    {
        return curation.categories.contains(&category);
    }
    // The catalogue speaks French and a provider feed speaks English, so both
    // vocabularies are read. Nothing is translated: these are the words that
    // actually appear in a tag or a genre.
    let facts = normalize(&[game.tags.join(" "), game.genres.join(" ")].join(" "));
    let says = |needles: &[&str]| needles.iter().any(|needle| facts.contains(needle));
    match category {
        StoreCategory::GoodForBrain => says(&[
            "puzzle",
            "strategy",
            "strategie",
            "logic",
            "logique",
            "thinking",
            "reflexion",
            "cartes",
            "enquete",
            "cerveau",
        ]),
        StoreCategory::ShortSessions => says(&["short session", "courte", "arcade", "roguelike"]),
        StoreCategory::StrongStories => says(&[
            "strong stor",
            "story rich",
            "story-rich",
            "recit",
            "narratif",
            "histoire",
        ]),
        StoreCategory::Relaxing => {
            says(&["relaxing", "relaxant", "cozy", "detente", "contemplatif"])
        }
        StoreCategory::ForYou | StoreCategory::AllGames => true,
    }
}

// ---------------------------------------------------------------------------
// Ownership. The store never sells a game the library already holds, and the
// decision is made here rather than in the WebView: a page cannot be handed a
// row it should not have been offered in the first place.
// ---------------------------------------------------------------------------

/// One library entry reduced to the two facts ownership turns on. Built by the
/// shell from the same rows `get_library` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedGame {
    pub id: String,
    pub title: String,
}

/// A title reduced to a comparable key. The same game reaches the library and
/// the catalogue under different identifiers — a Steam import against a store
/// row — so the title is the fallback match. Mirrors `ownershipKey` in
/// `src/store-model.ts`.
fn ownership_key(title: &str) -> String {
    normalize(title)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OwnedIndex {
    ids: BTreeSet<String>,
    titles: BTreeSet<String>,
}

impl OwnedIndex {
    fn new(library: &[OwnedGame]) -> Self {
        let mut index = Self::default();
        for game in library {
            let id = game.id.trim();
            if !id.is_empty() {
                index.ids.insert(id.to_string());
            }
            let title = ownership_key(&game.title);
            if !title.is_empty() {
                index.titles.insert(title);
            }
        }
        index
    }

    fn owns(&self, game: &CachedGame) -> bool {
        self.ids.contains(game.id.trim()) || self.titles.contains(&ownership_key(&game.title))
    }
}

/// The catalogue minus everything the library already holds.
fn without_owned(games: Vec<CachedGame>, library: &[OwnedGame]) -> Vec<CachedGame> {
    if library.is_empty() {
        return games;
    }
    let owned = OwnedIndex::new(library);
    games.into_iter().filter(|game| !owned.owns(game)).collect()
}

/// A game satisfies a platform filter when one of its offers comes from a
/// storefront for that platform, or when the game itself declares a platform
/// that maps onto it. Anything Orivo cannot map stays unmatched rather than
/// being assumed to fit.
fn matches_platforms(game: &CachedGame, platforms: &[StorePlatformId]) -> bool {
    if platforms.is_empty() {
        return true;
    }
    game.offers
        .iter()
        .filter_map(|offer| offer.provider.platform())
        .chain(
            game.supported_platforms
                .iter()
                .filter_map(|platform| platform.store_platform()),
        )
        .any(|platform| platforms.contains(&platform))
}

fn matches_request(
    game: &CachedGame,
    category: StoreCategory,
    providers: &[StoreProviderId],
    platforms: &[StorePlatformId],
    query: &str,
) -> bool {
    if !matches_category(game, category) {
        return false;
    }
    if !providers.is_empty()
        && !game
            .offers
            .iter()
            .any(|offer| providers.contains(&offer.provider))
    {
        return false;
    }
    if !matches_platforms(game, platforms) {
        return false;
    }
    if query.is_empty() {
        return true;
    }
    normalize(
        &[
            game.title.clone(),
            game.short_description.clone(),
            game.genres.join(" "),
            game.tags.join(" "),
        ]
        .join(" "),
    )
    .contains(query)
}

fn parse_cursor(cursor: Option<&str>) -> usize {
    cursor
        .and_then(|value| value.strip_prefix(BROWSE_CURSOR_PREFIX))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
}

fn browse_pool(
    games: &[CachedGame],
    request: &StoreBrowseRequest,
    profile: &LibraryProfile,
    now_ms: u64,
) -> (Vec<GameSummary>, Option<String>) {
    let query = normalize(
        &request
            .query
            .chars()
            .take(MAX_QUERY_LENGTH)
            .collect::<String>(),
    );
    let filtered: Vec<CachedGame> = games
        .iter()
        .filter(|game| {
            matches_request(
                game,
                request.category,
                &request.providers,
                &request.platforms,
                &query,
            )
        })
        .cloned()
        .collect();
    let (ranked, _, _) = recommend(&filtered, profile, now_ms);

    let limit = match request.limit {
        0 => DEFAULT_BROWSE_LIMIT,
        value => value.min(MAX_BROWSE_LIMIT),
    };
    let offset = parse_cursor(request.cursor.as_deref()).min(ranked.len());
    let page: Vec<GameSummary> = ranked.iter().skip(offset).take(limit).cloned().collect();
    let next_offset = offset + page.len();
    let next_cursor =
        (next_offset < ranked.len()).then(|| format!("{BROWSE_CURSOR_PREFIX}{next_offset}"));
    (page, next_cursor)
}

// ---------------------------------------------------------------------------
// Providers. Every adapter returns an outcome; none of them can return an
// error to the caller, so one failing provider never fails the response.
// ---------------------------------------------------------------------------

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The store's only outbound network seam. Tests inject a fake so no test in
/// this module can reach the network.
pub trait StoreHttp: Send + Sync {
    fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<serde_json::Value, String>>;

    /// The politeness pause between fallback requests, behind the same seam as
    /// the requests themselves so that no test in this module ever waits on a
    /// real clock.
    fn pause<'a>(&'a self, duration: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async move { tokio::time::sleep(duration).await })
    }
}

pub struct ReqwestStoreHttp {
    client: reqwest::Client,
}

impl ReqwestStoreHttp {
    pub fn new() -> Result<Self, String> {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::limited(2))
            .build()
            .map(|client| Self { client })
            .map_err(|error| format!("Orivo could not start a store request: {error}"))
    }
}

impl StoreHttp for ReqwestStoreHttp {
    fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<serde_json::Value, String>> {
        Box::pin(async move {
            let response = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|error| format!("request failed: {error}"))?;
            if !response.status().is_success() {
                return Err(format!("provider returned HTTP {}", response.status()));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
            {
                return Err("provider response exceeds the supported size".to_string());
            }
            let bytes = response
                .bytes()
                .await
                .map_err(|error| format!("response was interrupted: {error}"))?;
            if bytes.len() > MAX_RESPONSE_BYTES {
                return Err("provider response exceeds the supported size".to_string());
            }
            serde_json::from_slice(&bytes)
                .map_err(|_| "provider returned unreadable JSON".to_string())
        })
    }
}

#[derive(Debug, Clone, Default)]
struct ProviderOutcome {
    games: Vec<CachedGame>,
    offers: Vec<CachedOffer>,
}

#[derive(Debug, Clone)]
pub struct RefreshConfig {
    /// The storefront region every price is quoted in. There is no API key
    /// here on purpose: every endpoint this module calls is public.
    pub region: String,
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            region: DEFAULT_REGION.to_string(),
        }
    }
}

/// Refresh every provider, keeping each provider's failure inside its own
/// status. The returned document is always complete enough to serve.
async fn refresh_all(
    http: &dyn StoreHttp,
    config: &RefreshConfig,
    now_ms: u64,
) -> StoreCacheDocument {
    let mut games = catalog_games().to_vec();
    let mut statuses = Vec::new();

    let (steam_outcome, steam_status) = refresh_steam(http, config, &games, now_ms).await;
    apply_outcome(&mut games, steam_outcome);
    statuses.push(steam_status);

    let (apple_outcome, apple_status) = refresh_apple(http, config, now_ms).await;
    apply_outcome(&mut games, apple_outcome);
    statuses.push(apple_status);

    // Every remaining storefront has no official catalog contract available to
    // Orivo. They perform no network call and never contribute a price.
    for provider in [
        StoreProviderId::InstantGaming,
        StoreProviderId::Epic,
        StoreProviderId::Gog,
        StoreProviderId::Humble,
        StoreProviderId::Fanatical,
        StoreProviderId::GreenManGaming,
        StoreProviderId::Ubisoft,
        StoreProviderId::Microsoft,
        StoreProviderId::PlayStation,
        StoreProviderId::Nintendo,
        StoreProviderId::GooglePlay,
    ] {
        statuses.push(unconfigured_status(provider));
    }
    statuses.sort_by_key(|status| {
        StoreProviderId::ALL
            .iter()
            .position(|p| *p == status.provider)
    });

    StoreCacheDocument {
        schema_version: STORE_CACHE_SCHEMA_VERSION,
        refreshed_at_epoch_ms: Some(now_ms),
        games,
        provider_statuses: statuses,
    }
}

fn apply_outcome(games: &mut Vec<CachedGame>, outcome: ProviderOutcome) {
    for incoming in outcome.games {
        match games.iter_mut().find(|game| game.id == incoming.id) {
            Some(existing) => {
                existing.short_description = incoming.short_description;
                if !incoming.genres.is_empty() {
                    existing.genres = incoming.genres;
                }
                if !incoming.supported_platforms.is_empty() {
                    existing.supported_platforms = incoming.supported_platforms;
                }
            }
            None => games.push(incoming),
        }
    }
    for offer in outcome.offers {
        let Some(game) = games.iter_mut().find(|game| game.id == offer.game_id) else {
            continue;
        };
        match game
            .offers
            .iter_mut()
            .find(|existing| existing.id == offer.id)
        {
            Some(existing) => *existing = offer,
            None => game.offers.push(offer),
        }
    }
}

/// One catalogue game the Steam storefront can be asked about. The offer
/// identifier is the catalogue's own, so a refreshed price *replaces* the
/// price the catalogue shipped with instead of appearing beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SteamRefreshTarget {
    app_id: String,
    game_id: String,
    offer_id: String,
}

fn steam_refresh_targets(games: &[CachedGame]) -> Vec<SteamRefreshTarget> {
    games
        .iter()
        .filter_map(|game| {
            let app_id = steam_app_id(&game.id)?;
            let offer = game
                .offers
                .iter()
                .find(|offer| offer.provider == StoreProviderId::Steam)?;
            Some(SteamRefreshTarget {
                app_id,
                game_id: game.id.clone(),
                offer_id: offer.id.clone(),
            })
        })
        .take(MAX_STEAM_REFRESH_APPS)
        .collect()
}

fn steam_batch_url(app_ids: &str, region: &str) -> String {
    format!(
        "https://store.steampowered.com/api/appdetails?appids={app_ids}&cc={}&l=french&filters=price_overview",
        region.to_lowercase()
    )
}

fn steam_app_details_url(app_id: &str, region: &str) -> String {
    format!(
        "https://store.steampowered.com/api/appdetails?appids={app_id}&cc={}&l=french",
        region.to_lowercase()
    )
}

/// Steam refresh over the *public* storefront endpoint. It takes no Web API
/// key, which is why nothing here is gated on one.
///
/// What `store.steampowered.com/api/appdetails` actually does, measured
/// against the live endpoint before this was written:
///
/// * `appids=a,b,c` on its own answers `HTTP 400` with a `null` body;
/// * `appids=a,b,c&filters=price_overview` answers `200` with **every**
///   requested id keyed in one object — all 47 catalogue games came back in a
///   single response — so the shelf costs a handful of requests, not 47;
/// * a free game answers `{"success": true, "data": []}`: no price, which
///   stays `None` rather than being read as zero;
/// * an unknown id answers `{"success": false}`.
///
/// The batched call is therefore the normal path. Any id a batch did not
/// answer for is asked about on its own, in the unfiltered single-app form,
/// spaced by [`STEAM_REFRESH_PAUSE`] — so a Steam that starts answering one id
/// at a time still refreshes the catalogue, just more slowly.
async fn refresh_steam(
    http: &dyn StoreHttp,
    config: &RefreshConfig,
    games: &[CachedGame],
    now_ms: u64,
) -> (ProviderOutcome, CachedProviderStatus) {
    let targets = steam_refresh_targets(games);
    if targets.is_empty() {
        return (
            ProviderOutcome::default(),
            unconfigured_status(StoreProviderId::Steam),
        );
    }

    let mut outcome = ProviderOutcome::default();
    let mut unanswered: Vec<&SteamRefreshTarget> = Vec::new();

    for batch in targets.chunks(STEAM_REFRESH_BATCH) {
        let app_ids = batch
            .iter()
            .map(|target| target.app_id.as_str())
            .collect::<Vec<&str>>()
            .join(",");
        let Ok(payload) = http
            .get_json(&steam_batch_url(&app_ids, &config.region))
            .await
        else {
            unanswered.extend(batch.iter());
            continue;
        };
        for target in batch {
            match steam_offer_from_payload(target, &payload, &config.region, now_ms) {
                Some(offer) => outcome.offers.push(offer),
                None => unanswered.push(target),
            }
        }
    }

    let mut failures = 0usize;
    let mut unreachable_in_a_row = 0usize;
    for target in unanswered {
        // Politeness, not correctness: the fallback is the only path that can
        // fan out into one request per game.
        http.pause(STEAM_REFRESH_PAUSE).await;
        let Ok(payload) = http
            .get_json(&steam_app_details_url(&target.app_id, &config.region))
            .await
        else {
            failures += 1;
            unreachable_in_a_row += 1;
            // Steam has stopped answering altogether. Walking the rest of the
            // catalogue would only spend a request timeout per game to learn
            // the same thing, so the remaining games keep the price they have
            // and the status says so.
            if unreachable_in_a_row >= STEAM_REFRESH_GIVE_UP_AFTER {
                break;
            }
            continue;
        };
        unreachable_in_a_row = 0;
        match steam_offer_from_payload(target, &payload, &config.region, now_ms) {
            Some(offer) => outcome.offers.push(offer),
            None => failures += 1,
        }
    }

    let status = if failures == 0 {
        CachedProviderStatus {
            provider: StoreProviderId::Steam,
            health: ProviderHealth::Available,
            message: "Live Steam storefront prices are up to date.".to_string(),
            refreshed_at_epoch_ms: Some(now_ms),
        }
    } else if !outcome.offers.is_empty() {
        CachedProviderStatus {
            provider: StoreProviderId::Steam,
            health: ProviderHealth::Degraded,
            message: "Some Steam prices could not be refreshed and are shown as unverified."
                .to_string(),
            refreshed_at_epoch_ms: Some(now_ms),
        }
    } else {
        CachedProviderStatus {
            provider: StoreProviderId::Steam,
            health: ProviderHealth::Degraded,
            message: "Steam could not be reached, so its prices are shown as unverified."
                .to_string(),
            refreshed_at_epoch_ms: None,
        }
    };
    (outcome, status)
}

/// Read one app out of a storefront payload. Both shapes are handled by the
/// same reader: the filtered batch response carries `price_overview` alone,
/// the single-app response carries the whole listing.
fn steam_offer_from_payload(
    target: &SteamRefreshTarget,
    payload: &serde_json::Value,
    region: &str,
    now_ms: u64,
) -> Option<CachedOffer> {
    let entry = payload.get(&target.app_id)?;
    if !entry.get("success").and_then(serde_json::Value::as_bool)? {
        return None;
    }
    let data = entry.get("data")?;
    let is_free = data
        .get("is_free")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let price = data.get("price_overview");
    let price_minor = if is_free {
        Some(0)
    } else {
        price
            .and_then(|value| value.get("final"))
            .and_then(serde_json::Value::as_i64)
    };
    let currency = price
        .and_then(|value| value.get("currency"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let discount_percent = price
        .and_then(|value| value.get("discount_percent"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let url = steam_app_url(&target.app_id);
    validate_store_url(&url).ok()?;

    Some(CachedOffer {
        id: target.offer_id.clone(),
        game_id: target.game_id.clone(),
        provider: StoreProviderId::Steam,
        price_minor,
        currency,
        region: region.to_string(),
        // A refresh that quoted nothing is not a verified price. Keeping the
        // timestamp off it is what makes the card say "unverified" instead of
        // presenting a blank as today's price.
        verified_at_epoch_ms: price_minor.is_some().then_some(now_ms),
        // A payload with no price is reported as unknown, never as free.
        availability: if price_minor.is_some() {
            OfferAvailability::Available
        } else {
            OfferAvailability::Unknown
        },
        discount_percent,
        url,
    })
}

/// Apple refresh via the public iTunes Search API.
async fn refresh_apple(
    http: &dyn StoreHttp,
    config: &RefreshConfig,
    now_ms: u64,
) -> (ProviderOutcome, CachedProviderStatus) {
    let url = format!(
        "https://itunes.apple.com/search?media=software&entity=software&limit={APPLE_REFRESH_LIMIT}&country={}&term={APPLE_REFRESH_TERM}",
        config.region.to_uppercase()
    );
    let payload = match http.get_json(&url).await {
        Ok(payload) => payload,
        Err(_) => {
            return (
                ProviderOutcome::default(),
                CachedProviderStatus {
                    provider: StoreProviderId::Apple,
                    health: ProviderHealth::Degraded,
                    message:
                        "The App Store could not be reached, so no App Store prices are shown."
                            .to_string(),
                    refreshed_at_epoch_ms: None,
                },
            );
        }
    };

    let mut outcome = ProviderOutcome::default();
    let results = payload
        .get("results")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    for result in results {
        if let Some((game, offer)) = apple_entry(&result, &config.region, now_ms) {
            outcome.games.push(game);
            outcome.offers.push(offer);
        }
    }

    let status = if outcome.games.is_empty() {
        CachedProviderStatus {
            provider: StoreProviderId::Apple,
            health: ProviderHealth::Degraded,
            message: "The App Store returned no usable results for this region.".to_string(),
            refreshed_at_epoch_ms: Some(now_ms),
        }
    } else {
        CachedProviderStatus {
            provider: StoreProviderId::Apple,
            health: ProviderHealth::Available,
            message: "Live App Store results are up to date.".to_string(),
            refreshed_at_epoch_ms: Some(now_ms),
        }
    };
    (outcome, status)
}

fn apple_entry(
    result: &serde_json::Value,
    region: &str,
    now_ms: u64,
) -> Option<(CachedGame, CachedOffer)> {
    let track_id = result.get("trackId").and_then(serde_json::Value::as_i64)?;
    let title = result
        .get("trackName")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())?;
    let url = result
        .get("trackViewUrl")
        .and_then(serde_json::Value::as_str)?;
    // A provider URL is validated before it is ever written to the cache.
    validate_store_url(url).ok()?;

    // Artwork is handed straight to the WebView as an image source, so it is
    // validated exactly like the offer URL rather than trusted because it came
    // back in the same response.
    let artwork = result
        .get("artworkUrl512")
        .or_else(|| result.get("artworkUrl100"))
        .and_then(serde_json::Value::as_str)
        .map(validated_artwork_url)
        .unwrap_or_default();
    let description = result
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(220)
        .collect::<String>();
    let genres = result
        .get("genres")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    let platform = match result.get("kind").and_then(serde_json::Value::as_str) {
        Some("mac-software") => GamePlatform::Macos,
        _ => GamePlatform::Ios,
    };
    // `price` is reported in major units. A missing price stays missing.
    let price_minor = result
        .get("price")
        .and_then(serde_json::Value::as_f64)
        .filter(|value| *value >= 0.0)
        .map(|value| (value * 100.0).round() as i64);
    let currency = result
        .get("currency")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    let game_id = format!("apple:{track_id}");
    let offer = CachedOffer {
        id: format!("offer_apple_{track_id}"),
        game_id: game_id.clone(),
        provider: StoreProviderId::Apple,
        price_minor,
        currency,
        region: region.to_string(),
        verified_at_epoch_ms: Some(now_ms),
        availability: if price_minor.is_some() {
            OfferAvailability::Available
        } else {
            OfferAvailability::Unknown
        },
        discount_percent: 0,
        url: url.to_string(),
    };
    let game = CachedGame {
        id: game_id,
        title: title.to_string(),
        short_description: description,
        cover_url: artwork.clone(),
        hero_url: artwork.clone(),
        landscape_url: artwork,
        genres,
        tags: vec!["App Store".to_string()],
        supported_platforms: vec![platform],
        editorial_reasons: vec!["Listed on the App Store".to_string()],
        offers: Vec::new(),
        // Orivo writes editorial copy about the games it curates, and it has
        // written none about a live App Store result.
        curation: None,
    };
    Some((game, offer))
}

// ---------------------------------------------------------------------------
// Opening an offer. The WebView supplies an identifier and nothing else.
// ---------------------------------------------------------------------------

/// Platform hand-off seam. If the shell later adds `tauri-plugin-opener`, only
/// this implementation changes; the validation above it stays identical.
pub trait UrlOpener: Send + Sync {
    fn open(&self, url: &str) -> Result<(), String>;
}

pub struct SystemUrlOpener;

impl UrlOpener for SystemUrlOpener {
    fn open(&self, url: &str) -> Result<(), String> {
        // A fixed absolute binary, one argument, and no shell. The argument is
        // already known to start with `https://`, so it can never be read as a
        // flag or a path.
        #[cfg(target_os = "macos")]
        let mut command = {
            let mut command = std::process::Command::new("/usr/bin/open");
            command.arg(url);
            command
        };
        #[cfg(target_os = "linux")]
        let mut command = {
            let mut command = std::process::Command::new("/usr/bin/xdg-open");
            command.arg(url);
            command
        };
        // An unqualified program name would let Windows resolve `rundll32.exe`
        // through its search order, which includes the current directory.
        #[cfg(target_os = "windows")]
        let mut command = {
            let system_root = std::env::var("SystemRoot")
                .ok()
                .filter(|root| Path::new(root).is_absolute())
                .unwrap_or_else(|| "C:\\Windows".to_string());
            let mut command = std::process::Command::new(format!(
                "{}\\System32\\rundll32.exe",
                system_root.trim_end_matches('\\')
            ));
            command.arg("url.dll,FileProtocolHandler").arg(url);
            command
        };
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|_| "Orivo could not open this store page.".to_string())
    }
}

fn validate_offer_id(offer_id: &str) -> Result<(), String> {
    let trimmed = offer_id.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_OFFER_ID_LENGTH
        || !trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err("This store offer is no longer available.".to_string());
    }
    Ok(())
}

/// Only `https` and only a host on the hardcoded allowlist. Credentials in the
/// authority and non-default ports are refused as well.
fn validate_store_url(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| unopenable())?;
    if parsed.scheme() != "https" {
        return Err(unopenable());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(unopenable());
    }
    if parsed.port().is_some() {
        return Err(unopenable());
    }
    let host = parsed
        .host_str()
        .ok_or_else(unopenable)?
        .to_ascii_lowercase();
    if !ALLOWED_OFFER_HOSTS.contains(&host.as_str()) {
        return Err(unopenable());
    }
    Ok(parsed.to_string())
}

fn unopenable() -> String {
    "Orivo will only open verified store pages.".to_string()
}

/// Artwork the WebView is allowed to load: a bundled `/media/` asset, or an
/// image host the application CSP already permits. A provider is never allowed
/// to name an arbitrary origin — that is a request the WebView would make on
/// the user's behalf — so an unusable value degrades to no artwork.
fn validated_artwork_url(value: &str) -> String {
    let candidate = value.trim();
    if candidate.is_empty() {
        return String::new();
    }
    if candidate.starts_with("/media/")
        && !candidate.contains("..")
        && !candidate.contains('\\')
        && !candidate.chars().any(char::is_control)
    {
        return candidate.to_string();
    }
    let Ok(parsed) = reqwest::Url::parse(candidate) else {
        return String::new();
    };
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
    {
        return String::new();
    }
    match parsed.host_str().map(str::to_ascii_lowercase) {
        Some(host) if ALLOWED_ARTWORK_HOSTS.contains(&host.as_str()) => parsed.to_string(),
        _ => String::new(),
    }
}

fn open_offer_with(
    cache: &StoreCache,
    offer_id: &str,
    opener: &dyn UrlOpener,
) -> Result<(), String> {
    validate_offer_id(offer_id)?;
    let url = cache
        .offer_url(offer_id.trim())
        .ok_or_else(|| "This store offer is no longer available.".to_string())?;
    let validated = validate_store_url(&url)?;
    opener.open(&validated)
}

// ---------------------------------------------------------------------------
// Commands. Registered by the shell in `lib.rs`.
// ---------------------------------------------------------------------------

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|error| format!("Orivo could not resolve its data directory: {error}"))
}

/// Serve immediately from the embedded catalogue plus whatever the cache
/// already holds, minus everything the library owns. This never performs a
/// network request, so a slow or failing refresh cannot delay or fail it.
#[tauri::command]
pub fn get_store_home(
    app: AppHandle,
    state: State<'_, crate::AppState>,
) -> Result<StoreHomeView, String> {
    let directory = app_data_dir(&app)?;
    let library = crate::owned_library_games(&app, &state)?;
    Ok(store_home(
        &StoreCache::new(&directory),
        &directory,
        &library,
        now_epoch_ms(),
    ))
}

fn store_home(
    cache: &StoreCache,
    app_data_dir: &Path,
    library: &[OwnedGame],
    now_ms: u64,
) -> StoreHomeView {
    let document = cache.read();
    let games = if document.games.is_empty() {
        catalog_games().to_vec()
    } else {
        document.games
    };
    let games = without_owned(games, library);
    let statuses = if document.provider_statuses.is_empty() {
        default_provider_statuses()
    } else {
        document.provider_statuses
    };
    let profile = LibraryProfile::from_played(&read_played_games(app_data_dir));
    let (games, mode, heading) = recommend(&games, &profile, now_ms);
    StoreHomeView {
        games,
        provider_statuses: statuses.iter().map(CachedProviderStatus::to_dto).collect(),
        recommendation_mode: mode,
        recommendation_heading: heading,
        refreshed_at: document.refreshed_at_epoch_ms.map(iso8601_from_epoch_ms),
    }
}

#[tauri::command]
pub fn browse_store_games(
    app: AppHandle,
    state: State<'_, crate::AppState>,
    request: StoreBrowseRequest,
) -> Result<StoreBrowsePage, String> {
    let directory = app_data_dir(&app)?;
    let library = crate::owned_library_games(&app, &state)?;
    Ok(browse(
        &StoreCache::new(&directory),
        &directory,
        &library,
        &request,
        now_epoch_ms(),
    ))
}

fn browse(
    cache: &StoreCache,
    app_data_dir: &Path,
    library: &[OwnedGame],
    request: &StoreBrowseRequest,
    now_ms: u64,
) -> StoreBrowsePage {
    let document = cache.read();
    let games = if document.games.is_empty() {
        catalog_games().to_vec()
    } else {
        document.games
    };
    let games = without_owned(games, library);
    let statuses = if document.provider_statuses.is_empty() {
        default_provider_statuses()
    } else {
        document.provider_statuses
    };
    let profile = LibraryProfile::from_played(&read_played_games(app_data_dir));
    let (games, next_cursor) = browse_pool(&games, request, &profile, now_ms);
    StoreBrowsePage {
        games,
        next_cursor,
        provider_statuses: statuses.iter().map(CachedProviderStatus::to_dto).collect(),
    }
}

/// Background refresh. Provider failures are recorded as provider statuses, so
/// this only returns an error when the derived cache itself cannot be written.
#[tauri::command]
pub async fn refresh_store_sources(app: AppHandle) -> Result<(), String> {
    let directory = app_data_dir(&app)?;
    let config = RefreshConfig {
        region: resolve_region(&directory),
    };
    let http = ReqwestStoreHttp::new()?;
    let document = refresh_all(&http, &config, now_epoch_ms()).await;
    StoreCache::new(&directory).write(&document)
}

/// The WebView passes an opaque identifier. The destination is resolved and
/// validated here; no URL, path, or command ever crosses this boundary.
#[tauri::command]
pub fn open_store_offer(app: AppHandle, offer_id: String) -> Result<(), String> {
    let directory = app_data_dir(&app)?;
    open_offer_with(&StoreCache::new(&directory), &offer_id, &SystemUrlOpener)
}

/// Read the stored region tolerantly. An explicit host override wins; a
/// missing or automatic preference falls back to the default region.
fn resolve_region(app_data_dir: &Path) -> String {
    if let Ok(region) = std::env::var(STORE_REGION_ENV)
        && is_region_code(&region)
    {
        return region.to_uppercase();
    }
    #[derive(Deserialize)]
    struct StoredPreferences {
        #[serde(default, rename = "storeRegion")]
        store_region: String,
    }
    let region = fs::read_to_string(app_data_dir.join(PREFERENCES_FILE))
        .ok()
        .and_then(|encoded| serde_json::from_str::<StoredPreferences>(&encoded).ok())
        .map(|preferences| preferences.store_region)
        .unwrap_or_default();
    if is_region_code(&region) {
        region.to_uppercase()
    } else {
        DEFAULT_REGION.to_string()
    }
}

fn is_region_code(value: &str) -> bool {
    value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_alphabetic())
}

// ---------------------------------------------------------------------------
// Time helpers.
// ---------------------------------------------------------------------------

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn is_stale(verified_at_epoch_ms: Option<u64>, now_ms: u64) -> bool {
    match verified_at_epoch_ms {
        None => true,
        Some(verified) => now_ms.saturating_sub(verified) > OFFER_FRESHNESS.as_millis() as u64,
    }
}

/// Format an instant as the UTC ISO-8601 string the contract expects. Written
/// here rather than pulling in a date crate for one call site.
fn iso8601_from_epoch_ms(epoch_ms: u64) -> String {
    let seconds = epoch_ms / 1_000;
    let days = (seconds / 86_400) as i64;
    let time_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time_of_day / 3_600,
        (time_of_day % 3_600) / 60,
        time_of_day % 60
    )
}

/// Read back the same UTC ISO-8601 form, which is how a catalogue timestamp
/// survives into the cache unchanged. Only that form is accepted: a value that
/// is not a timestamp Orivo wrote is no timestamp at all, and the offer it
/// belongs to is then reported as never verified rather than as fresh.
fn epoch_ms_from_iso8601(value: &str) -> Option<u64> {
    fn field(value: &str, range: std::ops::Range<usize>) -> Option<i64> {
        let slice = value.get(range)?;
        slice
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| slice.parse::<i64>().ok())
            .flatten()
    }

    let bytes = value.as_bytes();
    if bytes.len() < 20
        || !value.ends_with('Z')
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = field(value, 0..4)?;
    let month = field(value, 5..7)?;
    let day = field(value, 8..10)?;
    let hour = field(value, 11..13)?;
    let minute = field(value, 14..16)?;
    let second = field(value, 17..19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Milliseconds are optional and read to at most three digits, which is the
    // precision the generator writes.
    let milliseconds = match bytes[19] {
        b'.' => {
            let digits: String = value[20..]
                .chars()
                .take_while(char::is_ascii_digit)
                .take(3)
                .collect();
            format!("{digits:0<3}").parse::<i64>().ok()?
        }
        b'Z' => 0,
        _ => return None,
    };

    let seconds = days_from_civil(year, month as u32, day as u32) * 86_400
        + hour * 3_600
        + minute * 60
        + second;
    u64::try_from(seconds * 1_000 + milliseconds).ok()
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_position = if month > 2 { month - 3 } else { month + 9 } as i64;
    let day_of_year = (153 * month_position + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_position + 2) / 5 + 1) as u32;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
    const NOW_MS: u64 = 1_785_000_000_000;

    // -- test harness --------------------------------------------------------

    struct TestDirectory {
        root: PathBuf,
    }

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "orivo-store-{label}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn cache(&self) -> StoreCache {
            StoreCache::new(&self.root)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// Minimal executor. The crate does not enable a Tokio runtime feature and
    /// every fake future here is immediately ready.
    fn block_on<F: Future>(future: F) -> F::Output {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::task::{Context, Poll, Wake, Waker};

        struct ThreadWaker {
            thread: std::thread::Thread,
            woken: AtomicBool,
        }
        impl Wake for ThreadWaker {
            fn wake(self: Arc<Self>) {
                self.wake_by_ref();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.woken.store(true, Ordering::SeqCst);
                self.thread.unpark();
            }
        }

        let waker_state = Arc::new(ThreadWaker {
            thread: std::thread::current(),
            woken: AtomicBool::new(false),
        });
        let waker = Waker::from(Arc::clone(&waker_state));
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => {
                    while !waker_state.woken.swap(false, Ordering::SeqCst) {
                        std::thread::park_timeout(Duration::from_millis(10));
                    }
                }
            }
        }
    }

    /// Canned HTTP. Any URL without a canned response fails, which is how the
    /// per-provider isolation tests simulate an outage.
    #[derive(Default)]
    struct FakeHttp {
        responses: Vec<(String, serde_json::Value)>,
        requested: Mutex<Vec<String>>,
    }

    impl FakeHttp {
        fn with(mut self, fragment: &str, payload: serde_json::Value) -> Self {
            self.responses.push((fragment.to_string(), payload));
            self
        }
    }

    impl StoreHttp for FakeHttp {
        fn get_json<'a>(
            &'a self,
            url: &'a str,
        ) -> BoxFuture<'a, Result<serde_json::Value, String>> {
            let owned = url.to_string();
            Box::pin(async move {
                self.requested.lock().unwrap().push(owned.clone());
                self.responses
                    .iter()
                    .find(|(fragment, _)| owned.contains(fragment.as_str()))
                    .map(|(_, payload)| payload.clone())
                    .ok_or_else(|| "network unavailable".to_string())
            })
        }

        /// No test waits on a real clock, and none of them needs a Tokio timer
        /// to exercise the per-app fallback.
        fn pause<'a>(&'a self, _duration: Duration) -> BoxFuture<'a, ()> {
            Box::pin(async {})
        }
    }

    #[derive(Default)]
    struct RecordingOpener {
        opened: Mutex<Vec<String>>,
    }

    impl UrlOpener for RecordingOpener {
        fn open(&self, url: &str) -> Result<(), String> {
            self.opened.lock().unwrap().push(url.to_string());
            Ok(())
        }
    }

    fn offer_fixture(id: &str, url: &str) -> CachedOffer {
        CachedOffer {
            id: id.to_string(),
            game_id: "steam:1".to_string(),
            provider: StoreProviderId::Steam,
            price_minor: Some(1_999),
            currency: Some("USD".to_string()),
            region: "US".to_string(),
            verified_at_epoch_ms: Some(NOW_MS),
            availability: OfferAvailability::Available,
            discount_percent: 0,
            url: url.to_string(),
        }
    }

    fn game_fixture(offers: Vec<CachedOffer>) -> CachedGame {
        CachedGame {
            id: "steam:1".to_string(),
            title: "Fixture".to_string(),
            short_description: "A fixture.".to_string(),
            cover_url: "/media/igdb/covers/fixture.jpg".to_string(),
            hero_url: "/media/igdb/heroes/fixture.jpg".to_string(),
            landscape_url: "/media/igdb/landscapes/fixture.jpg".to_string(),
            genres: vec!["Strategy".to_string()],
            tags: vec!["Short Sessions".to_string()],
            supported_platforms: vec![GamePlatform::Macos],
            editorial_reasons: vec!["Editorial".to_string()],
            offers,
            curation: None,
        }
    }

    /// A library the store must not sell back to the user.
    fn owned(id: &str, title: &str) -> OwnedGame {
        OwnedGame {
            id: id.to_string(),
            title: title.to_string(),
        }
    }

    /// The curated catalogue the shell ships with. Pinned so that a generator
    /// change which silently drops games fails here instead of on a shelf.
    const CATALOG_SIZE: usize = 47;

    /// The first catalogue game, as the Steam refresh addresses it.
    fn catalog_refresh_target() -> SteamRefreshTarget {
        steam_refresh_targets(catalog_games())
            .into_iter()
            .next()
            .expect("the catalogue has a refreshable Steam game")
    }

    fn played(title: &str, genre: &str) -> PlayedGame {
        PlayedGame {
            title: title.to_string(),
            genres: vec![genre.to_string()],
            tags: Vec::new(),
            play_time_seconds: 3_600,
            last_played_at: Some("2026-07-01T00:00:00Z".to_string()),
        }
    }

    // -- contract shape ------------------------------------------------------

    #[test]
    fn dtos_serialize_to_the_typescript_contract() {
        let offer = offer_fixture("offer_ed_1", "https://store.steampowered.com/app/1/");
        let summary = game_fixture(vec![offer]).to_summary(NOW_MS, vec!["Because".into()]);
        let value = serde_json::to_value(&summary).unwrap();
        let object = value.as_object().unwrap();

        for key in [
            "id",
            "title",
            "source",
            "shortDescription",
            "coverUrl",
            "heroUrl",
            "landscapeUrl",
            "genres",
            "tags",
            "supportedPlatforms",
            "owned",
            "launchable",
            "wishlisted",
            "playTimeSeconds",
            "lastPlayedAt",
            "recommendationReasons",
            "offers",
        ] {
            assert!(object.contains_key(key), "GameSummary is missing {key}");
        }
        // `curation?:` is optional in `src/contracts.ts`, so a game without
        // editorial copy must not carry the key at all.
        assert!(!object.contains_key("curation"));
        assert_eq!(object.len(), 17, "GameSummary has unexpected fields");
        assert_eq!(object["source"], "store");
        assert_eq!(object["supportedPlatforms"], serde_json::json!(["macos"]));

        let offer = &value["offers"][0];
        for key in [
            "id",
            "gameId",
            "provider",
            "providerLabel",
            "priceMinor",
            "currency",
            "region",
            "verifiedAt",
            "availability",
            "stale",
        ] {
            assert!(
                offer.as_object().unwrap().contains_key(key),
                "StoreOffer is missing {key}"
            );
        }
        assert_eq!(offer.as_object().unwrap().len(), 10);
        assert_eq!(offer["provider"], "steam");
        assert_eq!(offer["providerLabel"], "Steam");
        assert_eq!(offer["availability"], "available");
        assert_eq!(offer["verifiedAt"], "2026-07-25T17:20:00Z");

        // A curated catalogue game carries it, camelCased, exactly as the
        // catalogue wrote it.
        let curated = catalog_games()
            .iter()
            .find(|game| game.id == "steam:1608230")
            .expect("Planet of Lana is in the catalogue")
            .to_summary(NOW_MS, Vec::new());
        let curated = serde_json::to_value(&curated).unwrap();
        assert_eq!(curated.as_object().unwrap().len(), 18);
        let curation = curated["curation"].as_object().unwrap();
        for key in [
            "genres",
            "duration",
            "mode",
            "stats",
            "tagline",
            "heroTitle",
            "heroLead",
            "highlights",
            "categories",
            "platforms",
        ] {
            assert!(curation.contains_key(key), "StoreCuration is missing {key}");
        }
        assert_eq!(curation.len(), 10);
        assert_eq!(curated["curation"]["duration"], "3-4h");
        assert_eq!(curated["curation"]["mode"], "Solo");
        assert_eq!(
            curated["curation"]["tagline"],
            "Histoire émouvante sans violence."
        );
        assert_eq!(curated["curation"]["stats"][0]["label"], "Réflexion");
        assert_eq!(curated["curation"]["stats"][0]["value"], 5);
        assert_eq!(curated["curation"]["highlights"][0]["icon"], "heart");
        assert_eq!(
            curated["curation"]["categories"],
            serde_json::json!(["strong-stories", "short-sessions"])
        );
        assert_eq!(
            curated["curation"]["platforms"],
            serde_json::json!(["pc", "xbox", "playstation"])
        );
    }

    #[test]
    fn enum_spellings_match_the_contract() {
        let providers: Vec<serde_json::Value> = StoreProviderId::ALL
            .iter()
            .map(|provider| serde_json::to_value(provider).unwrap())
            .collect();
        assert_eq!(
            providers,
            serde_json::json!([
                "steam",
                "instant-gaming",
                "epic",
                "gog",
                "humble",
                "fanatical",
                "green-man-gaming",
                "ubisoft",
                "microsoft",
                "playstation",
                "nintendo",
                "apple",
                "google-play"
            ])
            .as_array()
            .unwrap()
            .clone()
        );
        // The serialized name is the contract; the slug is what the frontend
        // builds its filter route from. They must not drift apart.
        for provider in StoreProviderId::ALL {
            assert_eq!(
                serde_json::to_value(provider).unwrap(),
                provider.slug(),
                "{provider:?} serializes differently from its slug"
            );
        }

        for (health, expected) in [
            (ProviderHealth::Available, "available"),
            (ProviderHealth::Degraded, "degraded"),
            (ProviderHealth::Unavailable, "unavailable"),
            (ProviderHealth::NotConfigured, "not-configured"),
        ] {
            assert_eq!(serde_json::to_value(health).unwrap(), expected);
        }
        for (category, expected) in [
            (StoreCategory::ForYou, "for-you"),
            (StoreCategory::GoodForBrain, "good-for-brain"),
            (StoreCategory::ShortSessions, "short-sessions"),
            (StoreCategory::StrongStories, "strong-stories"),
            (StoreCategory::Relaxing, "relaxing"),
            (StoreCategory::AllGames, "all-games"),
        ] {
            assert_eq!(serde_json::to_value(category).unwrap(), expected);
        }
        for (platform, expected) in [
            (StorePlatformId::Pc, "pc"),
            (StorePlatformId::PlayStation, "playstation"),
            (StorePlatformId::Xbox, "xbox"),
            (StorePlatformId::Switch, "switch"),
            (StorePlatformId::Emulators, "emulators"),
        ] {
            assert_eq!(serde_json::to_value(platform).unwrap(), expected);
            assert_eq!(
                serde_json::from_value::<StorePlatformId>(serde_json::json!(expected)).unwrap(),
                platform
            );
        }
        assert_eq!(
            serde_json::to_value(RecommendationMode::Personalized).unwrap(),
            "personalized"
        );
        assert_eq!(serde_json::to_value(GamePlatform::Ios).unwrap(), "ios");
    }

    #[test]
    fn browse_requests_deserialize_from_the_frontend_payload() {
        let request: StoreBrowseRequest = serde_json::from_value(serde_json::json!({
            "category": "short-sessions",
            "providers": ["steam", "google-play"],
            "platforms": ["pc", "playstation"],
            "query": "rail",
            "cursor": null,
            "limit": 12
        }))
        .unwrap();
        assert_eq!(request.category, StoreCategory::ShortSessions);
        assert_eq!(
            request.providers,
            vec![StoreProviderId::Steam, StoreProviderId::GooglePlay]
        );
        assert_eq!(
            request.platforms,
            vec![StorePlatformId::Pc, StorePlatformId::PlayStation]
        );
        assert_eq!(request.limit, 12);
        assert!(request.cursor.is_none());

        // The platform filter is additive: a payload written before it existed
        // still deserializes and still means "no platform filter".
        let legacy: StoreBrowseRequest = serde_json::from_value(serde_json::json!({
            "category": "good-for-brain",
            "providers": ["epic", "gog", "humble", "fanatical", "green-man-gaming"],
            "query": "",
            "cursor": null,
            "limit": 0
        }))
        .unwrap();
        assert_eq!(legacy.category, StoreCategory::GoodForBrain);
        assert_eq!(
            legacy.providers,
            vec![
                StoreProviderId::Epic,
                StoreProviderId::Gog,
                StoreProviderId::Humble,
                StoreProviderId::Fanatical,
                StoreProviderId::GreenManGaming
            ]
        );
        assert!(legacy.platforms.is_empty());
    }

    #[test]
    fn provider_statuses_expose_a_reason_for_every_provider() {
        let statuses = default_provider_statuses();
        assert_eq!(statuses.len(), StoreProviderId::ALL.len());
        for status in &statuses {
            assert!(!status.message.trim().is_empty());
            assert!(matches!(
                status.health,
                ProviderHealth::NotConfigured
                    | ProviderHealth::Unavailable
                    | ProviderHealth::Degraded
            ));
            assert!(!status.provider.slug().is_empty());
        }
    }

    // -- cache ---------------------------------------------------------------

    #[test]
    fn cache_round_trips_and_rebuilds_after_corruption() {
        let directory = TestDirectory::new("cache");
        let cache = directory.cache();
        assert!(cache.read().games.is_empty());

        let document = StoreCacheDocument {
            schema_version: STORE_CACHE_SCHEMA_VERSION,
            refreshed_at_epoch_ms: Some(NOW_MS),
            games: vec![game_fixture(vec![offer_fixture(
                "offer_ed_1",
                "https://store.steampowered.com/app/1/",
            )])],
            provider_statuses: default_provider_statuses(),
        };
        cache.write(&document).unwrap();
        assert_eq!(cache.read(), document);

        fs::write(directory.root.join(STORE_CACHE_FILE), b"{ not json").unwrap();
        assert_eq!(cache.read(), StoreCacheDocument::default());

        fs::write(
            directory.root.join(STORE_CACHE_FILE),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": STORE_CACHE_SCHEMA_VERSION + 9,
                "games": [{ "id": "steam:1", "title": "Future" }]
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(cache.read(), StoreCacheDocument::default());

        // A discarded cache is never fatal: the store still serves the whole
        // embedded catalogue.
        let home = store_home(&cache, &directory.root, &[], NOW_MS);
        assert_eq!(home.games.len(), CATALOG_SIZE);
        assert_eq!(home.recommendation_mode, RecommendationMode::Editorial);
        assert!(home.refreshed_at.is_none());
    }

    #[test]
    fn cache_writes_leave_no_temporary_files_behind() {
        let directory = TestDirectory::new("atomic");
        directory
            .cache()
            .write(&StoreCacheDocument::default())
            .unwrap();
        let leftovers: Vec<PathBuf> = fs::read_dir(&directory.root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files remained: {leftovers:?}"
        );
    }

    // -- offers --------------------------------------------------------------

    #[test]
    fn offers_older_than_a_day_are_marked_stale() {
        let fresh = CachedOffer {
            verified_at_epoch_ms: Some(NOW_MS - DAY_MS / 2),
            ..offer_fixture("offer_a", "https://store.steampowered.com/app/1/")
        };
        let old = CachedOffer {
            verified_at_epoch_ms: Some(NOW_MS - DAY_MS - 1),
            ..offer_fixture("offer_b", "https://store.steampowered.com/app/1/")
        };
        let never = CachedOffer {
            verified_at_epoch_ms: None,
            ..offer_fixture("offer_c", "https://store.steampowered.com/app/1/")
        };

        assert!(!fresh.to_dto(NOW_MS).stale);
        assert!(old.to_dto(NOW_MS).stale);
        assert!(never.to_dto(NOW_MS).stale);
        assert!(never.to_dto(NOW_MS).verified_at.is_none());
    }

    #[test]
    fn a_missing_price_stays_missing() {
        let offer = CachedOffer {
            price_minor: None,
            currency: None,
            availability: OfferAvailability::Unknown,
            ..offer_fixture("offer_a", "https://store.steampowered.com/app/1/")
        };
        let dto = offer.to_dto(NOW_MS);
        assert_eq!(dto.price_minor, None);
        assert_eq!(dto.currency, None);
        assert_eq!(dto.availability, OfferAvailability::Unknown);

        let value = serde_json::to_value(&dto).unwrap();
        assert!(value["priceMinor"].is_null());
        assert!(value["currency"].is_null());

        // The catalogue quotes a price only where a shop quoted one. Every
        // other row keeps its nulls all the way to the wire; a console
        // storefront Orivo has no feed for is exactly such a row.
        let mut priceless = 0usize;
        for game in catalog_games() {
            for offer in &game.offers {
                if offer.price_minor.is_none() {
                    priceless += 1;
                    assert_eq!(offer.currency, None);
                    let value = serde_json::to_value(offer.to_dto(NOW_MS)).unwrap();
                    assert!(value["priceMinor"].is_null());
                    assert!(value["currency"].is_null());
                } else {
                    assert!(offer.currency.is_some(), "a price arrived with no currency");
                }
            }
        }
        assert!(priceless > 0, "the catalogue quotes a price for everything");
    }

    #[test]
    fn a_steam_payload_without_a_price_reports_unknown_availability() {
        let target = catalog_refresh_target();
        let payload = serde_json::json!({
            target.app_id.clone(): { "success": true, "data": { "name": "Planet of Lana" } }
        });
        let offer = steam_offer_from_payload(&target, &payload, "US", NOW_MS).unwrap();
        assert_eq!(offer.price_minor, None);
        assert_eq!(offer.currency, None);
        assert_eq!(offer.availability, OfferAvailability::Unknown);

        // The filtered batch shape a free game answers with: still no price,
        // and still not read as "free".
        let free = serde_json::json!({
            target.app_id.clone(): { "success": true, "data": [] }
        });
        let offer = steam_offer_from_payload(&target, &free, "FR", NOW_MS).unwrap();
        assert_eq!(offer.price_minor, None);
        assert_eq!(offer.availability, OfferAvailability::Unknown);

        let priced = serde_json::json!({
            target.app_id.clone(): {
                "success": true,
                "data": {
                    "is_free": false,
                    "price_overview": { "currency": "EUR", "final": 4_199, "discount_percent": 30 }
                }
            }
        });
        let offer = steam_offer_from_payload(&target, &priced, "FR", NOW_MS).unwrap();
        assert_eq!(offer.price_minor, Some(4_199));
        assert_eq!(offer.currency.as_deref(), Some("EUR"));
        assert_eq!(offer.discount_percent, 30);
        assert_eq!(offer.availability, OfferAvailability::Available);
    }

    /// A refresh that comes back without a price must leave the offer's price
    /// null on the wire rather than reporting a stale or invented one.
    #[test]
    fn a_refresh_that_quotes_no_price_leaves_the_offer_null() {
        let target = catalog_refresh_target();
        let http = FakeHttp::default().with(
            "store.steampowered.com/api/appdetails",
            serde_json::json!({ target.app_id.clone(): { "success": true, "data": [] } }),
        );
        let document = block_on(refresh_all(&http, &RefreshConfig::default(), NOW_MS));

        let game = document
            .games
            .iter()
            .find(|game| game.id == target.game_id)
            .expect("the refreshed game stays in the catalogue");
        let offer = game
            .offers
            .iter()
            .find(|offer| offer.id == target.offer_id)
            .expect("the catalogue's own Steam offer was refreshed in place");
        assert_eq!(offer.price_minor, None);
        assert_eq!(offer.currency, None);
        assert_eq!(offer.availability, OfferAvailability::Unknown);

        let dto = serde_json::to_value(offer.to_dto(NOW_MS)).unwrap();
        assert!(dto["priceMinor"].is_null());
        assert!(dto["currency"].is_null());
        assert_eq!(dto["stale"], true);

        // The refresh replaced the shipped offer rather than adding a second
        // one, so there is no way for the card to fall back to the old price.
        assert_eq!(
            game.offers
                .iter()
                .filter(|offer| offer.provider == StoreProviderId::Steam)
                .count(),
            1
        );
    }

    // -- providers -----------------------------------------------------------

    #[test]
    fn one_failing_provider_never_fails_the_response() {
        // Steam answers for one catalogue game; Apple's request fails.
        let target = catalog_refresh_target();
        let http = FakeHttp::default().with(
            "store.steampowered.com/api/appdetails",
            serde_json::json!({
                target.app_id.clone(): {
                    "success": true,
                    "data": { "price_overview": { "currency": "USD", "final": 3_999, "discount_percent": 0 } }
                }
            }),
        );
        let config = RefreshConfig {
            region: "US".to_string(),
        };
        let document = block_on(refresh_all(&http, &config, NOW_MS));

        assert_eq!(document.provider_statuses.len(), StoreProviderId::ALL.len());
        let status = |provider: StoreProviderId| {
            document
                .provider_statuses
                .iter()
                .find(|status| status.provider == provider)
                .unwrap()
                .clone()
        };
        assert_eq!(
            status(StoreProviderId::Apple).health,
            ProviderHealth::Degraded
        );
        assert!(!status(StoreProviderId::Apple).message.is_empty());
        // Only one catalogue game has a canned answer, so Steam degrades
        // partially but still contributes its verified offer.
        assert_eq!(
            status(StoreProviderId::Steam).health,
            ProviderHealth::Degraded
        );
        assert!(!document.games.is_empty());
        let refreshed = document
            .games
            .iter()
            .find(|game| game.id == target.game_id)
            .unwrap();
        let steam_offer = refreshed
            .offers
            .iter()
            .find(|offer| offer.id == target.offer_id)
            .unwrap();
        assert_eq!(steam_offer.price_minor, Some(3_999));
        assert_eq!(steam_offer.currency.as_deref(), Some("USD"));
        assert!(!steam_offer.to_dto(NOW_MS).stale);

        // A catalogue game with no answer keeps the price it shipped with,
        // reported as the older quote it is rather than as today's.
        let untouched = document
            .games
            .iter()
            .find(|game| game.id != target.game_id && steam_app_id(&game.id).is_some())
            .unwrap();
        let shipped = catalog_games()
            .iter()
            .find(|game| game.id == untouched.id)
            .unwrap();
        assert_eq!(untouched.offers, shipped.offers);
    }

    #[test]
    fn providers_without_a_feed_never_reach_the_network_or_invent_a_price() {
        let http = FakeHttp::default();
        let document = block_on(refresh_all(&http, &RefreshConfig::default(), NOW_MS));

        for provider in [
            StoreProviderId::InstantGaming,
            StoreProviderId::Epic,
            StoreProviderId::Gog,
            StoreProviderId::Humble,
            StoreProviderId::Fanatical,
            StoreProviderId::GreenManGaming,
            StoreProviderId::Ubisoft,
            StoreProviderId::Microsoft,
            StoreProviderId::PlayStation,
            StoreProviderId::Nintendo,
            StoreProviderId::GooglePlay,
        ] {
            let status = document
                .provider_statuses
                .iter()
                .find(|status| status.provider == provider)
                .unwrap();
            assert!(matches!(
                status.health,
                ProviderHealth::Unavailable | ProviderHealth::NotConfigured
            ));
            assert!(status.message.len() > 10);
            // Such a provider may carry the price the catalogue was built with,
            // but this refresh must not have touched it: byte for byte, the
            // offers are the ones that shipped.
            let shipped = catalog_games()
                .iter()
                .find(|game| game.id == status_game_id(&document, provider))
                .map(|game| game.offers.clone());
            if let Some(shipped) = shipped {
                let live = document
                    .games
                    .iter()
                    .find(|game| game.id == status_game_id(&document, provider))
                    .unwrap();
                assert_eq!(live.offers, shipped);
            }
        }

        // Steam's storefront endpoint needs no key, so it *is* requested, and
        // no other unreachable provider is.
        let requested = http.requested.lock().unwrap().clone();
        assert!(
            requested
                .iter()
                .any(|url| url.contains("store.steampowered.com/api/appdetails")),
            "the public Steam price endpoint was never called: {requested:?}"
        );
        assert!(
            requested.iter().all(|url| url.contains("itunes.apple.com")
                || url.contains("store.steampowered.com/api/appdetails")),
            "a provider without a feed performed a request: {requested:?}"
        );
        // Steam was unreachable in this test, so it reports that rather than a
        // configuration problem it no longer has.
        let steam = document
            .provider_statuses
            .iter()
            .find(|status| status.provider == StoreProviderId::Steam)
            .unwrap();
        assert_eq!(steam.health, ProviderHealth::Degraded);
        assert!(steam.refreshed_at_epoch_ms.is_none());
    }

    /// The first catalogue game carrying an offer from `provider`, if any.
    fn status_game_id(document: &StoreCacheDocument, provider: StoreProviderId) -> String {
        document
            .games
            .iter()
            .find(|game| game.offers.iter().any(|offer| offer.provider == provider))
            .map(|game| game.id.clone())
            .unwrap_or_default()
    }

    #[test]
    fn apple_results_become_offers_and_reject_unlisted_hosts() {
        let http = FakeHttp::default().with(
            "itunes.apple.com/search",
            serde_json::json!({
                "resultCount": 3,
                "results": [
                    {
                        "trackId": 1,
                        "trackName": "Fixture Quest",
                        "trackViewUrl": "https://apps.apple.com/us/app/fixture/id1",
                        "price": 4.99,
                        "currency": "USD",
                        "genres": ["Games", "Adventure"],
                        "kind": "software"
                    },
                    {
                        "trackId": 2,
                        "trackName": "No Price",
                        "trackViewUrl": "https://apps.apple.com/us/app/no-price/id2",
                        "genres": ["Games"]
                    },
                    {
                        "trackId": 3,
                        "trackName": "Phishing",
                        "trackViewUrl": "https://apps.apple.com.evil.example/app",
                        "price": 0.0
                    }
                ]
            }),
        );
        let (outcome, status) = block_on(refresh_apple(&http, &RefreshConfig::default(), NOW_MS));

        assert_eq!(status.health, ProviderHealth::Available);
        assert_eq!(outcome.games.len(), 2, "an unlisted host was accepted");
        assert_eq!(outcome.offers[0].price_minor, Some(499));
        assert_eq!(outcome.offers[1].price_minor, None);
        assert_eq!(outcome.offers[1].availability, OfferAvailability::Unknown);
    }

    // -- opening offers ------------------------------------------------------

    #[test]
    fn open_store_offer_resolves_only_known_ids_and_allowlisted_hosts() {
        let directory = TestDirectory::new("open");
        let cache = directory.cache();
        cache
            .write(&StoreCacheDocument {
                games: vec![
                    game_fixture(vec![offer_fixture(
                        "offer_good",
                        "https://store.steampowered.com/app/1/",
                    )]),
                    CachedGame {
                        id: "steam:2".to_string(),
                        offers: vec![
                            CachedOffer {
                                game_id: "steam:2".to_string(),
                                ..offer_fixture(
                                    "offer_http",
                                    "http://store.steampowered.com/app/2/",
                                )
                            },
                            CachedOffer {
                                game_id: "steam:2".to_string(),
                                ..offer_fixture("offer_host", "https://evil.example.com/app/2/")
                            },
                            CachedOffer {
                                game_id: "steam:2".to_string(),
                                ..offer_fixture(
                                    "offer_lookalike",
                                    "https://store.steampowered.com.evil.example/app/2/",
                                )
                            },
                            CachedOffer {
                                game_id: "steam:2".to_string(),
                                ..offer_fixture("offer_file", "file:///etc/passwd")
                            },
                            CachedOffer {
                                game_id: "steam:2".to_string(),
                                ..offer_fixture(
                                    "offer_creds",
                                    "https://user:pass@store.steampowered.com/app/2/",
                                )
                            },
                        ],
                        ..game_fixture(Vec::new())
                    },
                ],
                ..StoreCacheDocument::default()
            })
            .unwrap();

        let opener = RecordingOpener::default();
        open_offer_with(&cache, "offer_good", &opener).unwrap();
        assert_eq!(
            opener.opened.lock().unwrap().clone(),
            vec!["https://store.steampowered.com/app/1/".to_string()]
        );

        for offer_id in [
            "offer_http",
            "offer_host",
            "offer_lookalike",
            "offer_file",
            "offer_creds",
            "offer_unknown",
            "",
            "   ",
            "https://evil.example.com",
            "../../etc/passwd",
        ] {
            assert!(
                open_offer_with(&cache, offer_id, &opener).is_err(),
                "{offer_id} was accepted"
            );
        }
        assert_eq!(opener.opened.lock().unwrap().len(), 1);
    }

    #[test]
    fn catalogue_offers_stay_openable_without_a_cache_file() {
        let directory = TestDirectory::new("no-cache");
        let opener = RecordingOpener::default();
        let game = &catalog_games()[0];
        let steam_offer = game
            .offers
            .iter()
            .find(|offer| offer.provider == StoreProviderId::Steam)
            .expect("every catalogue game is sold on Steam");
        open_offer_with(&directory.cache(), &steam_offer.id, &opener).unwrap();
        assert_eq!(
            opener.opened.lock().unwrap()[0],
            format!(
                "https://store.steampowered.com/app/{}/",
                steam_app_id(&game.id).unwrap()
            )
        );

        // The other storefronts sell the same game under identifiers Orivo was
        // never given, so those rows quote their price and refuse to open a
        // page Orivo would have had to guess at.
        for offer in game
            .offers
            .iter()
            .filter(|offer| offer.provider != StoreProviderId::Steam)
        {
            assert!(
                offer.url.is_empty(),
                "{:?} was given a guessed URL",
                offer.provider
            );
            assert!(open_offer_with(&directory.cache(), &offer.id, &opener).is_err());
        }
        assert_eq!(opener.opened.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_allowlist_covers_every_supported_storefront() {
        // Written out a second time on purpose: the allowlist is a security
        // boundary, so a host may only appear in it deliberately.
        for (provider, host) in [
            (StoreProviderId::Steam, "store.steampowered.com"),
            (StoreProviderId::InstantGaming, "www.instant-gaming.com"),
            (StoreProviderId::Epic, "store.epicgames.com"),
            (StoreProviderId::Gog, "www.gog.com"),
            (StoreProviderId::Humble, "www.humblebundle.com"),
            (StoreProviderId::Fanatical, "www.fanatical.com"),
            (StoreProviderId::GreenManGaming, "www.greenmangaming.com"),
            (StoreProviderId::Ubisoft, "store.ubisoft.com"),
            (StoreProviderId::Microsoft, "apps.microsoft.com"),
            (StoreProviderId::PlayStation, "store.playstation.com"),
            (StoreProviderId::Nintendo, "www.nintendo.com"),
            (StoreProviderId::Apple, "apps.apple.com"),
            (StoreProviderId::GooglePlay, "play.google.com"),
        ] {
            assert!(
                ALLOWED_OFFER_HOSTS.contains(&host),
                "{provider:?} has no allowlisted storefront host"
            );
        }
        assert_eq!(ALLOWED_OFFER_HOSTS.len(), StoreProviderId::ALL.len());
        for host in ALLOWED_OFFER_HOSTS {
            assert!(validate_store_url(&format!("https://{host}/page")).is_ok());
            assert!(validate_store_url(&format!("http://{host}/page")).is_err());
            assert!(validate_store_url(&format!("https://{host}:8443/page")).is_err());
        }
        assert!(validate_store_url("javascript:alert(1)").is_err());
        assert!(validate_store_url("not a url").is_err());
    }

    #[test]
    fn view_models_never_carry_a_url_or_a_filesystem_path() {
        let directory = TestDirectory::new("leaks");
        let home = store_home(&directory.cache(), &directory.root, &[], NOW_MS);
        let encoded = serde_json::to_string(&home).unwrap();
        assert!(
            !encoded.contains("http"),
            "a provider URL leaked into the view"
        );
        assert!(!encoded.contains(&directory.root.to_string_lossy().to_string()));
        assert!(!encoded.contains("url\":"));

        // The editorial baseline can never leak, so the same assertion has to
        // be made against what a live provider actually puts in the cache.
        let directory = TestDirectory::new("leaks-live");
        let live = apple_result(
            "https://is1-ssl.mzstatic.com/image/thumb/Purple/v4/artwork/512x512bb.jpg",
        );
        let (game, offer) = apple_entry(&live, "US", NOW_MS).expect("a live App Store entry");
        directory
            .cache()
            .write(&StoreCacheDocument {
                games: vec![CachedGame {
                    offers: vec![offer],
                    ..game
                }],
                ..StoreCacheDocument::default()
            })
            .unwrap();

        let home = store_home(&directory.cache(), &directory.root, &[], NOW_MS);
        let encoded = serde_json::to_string(&home).unwrap();
        // Without this the assertions below would pass on an empty view.
        assert!(encoded.contains("Example App"), "{encoded}");
        assert!(
            !encoded.contains("http"),
            "a live provider URL leaked into the view: {encoded}"
        );
        assert!(!encoded.contains("mzstatic"));
        assert!(!encoded.contains("url\":"));
    }

    #[test]
    fn provider_artwork_urls_are_validated_before_they_are_cached() {
        for hostile in [
            "https://is1-ssl.mzstatic.com/image/thumb/artwork.jpg",
            "javascript:alert(1)",
            "data:image/png;base64,AAAA",
            "http://cdn.cloudflare.steamstatic.com/steam/apps/1/cover.jpg",
            "https://cdn.cloudflare.steamstatic.com.evil.example/cover.jpg",
            "https://user:secret@cdn.cloudflare.steamstatic.com/cover.jpg",
            "https://cdn.cloudflare.steamstatic.com:8443/cover.jpg",
            "/media/../../etc/passwd",
            "not a url",
        ] {
            let (game, _) =
                apple_entry(&apple_result(hostile), "US", NOW_MS).expect("entry stays usable");
            assert_eq!(game.cover_url, "", "{hostile} was accepted");
            assert_eq!(game.hero_url, "", "{hostile} was accepted");
            assert_eq!(game.landscape_url, "", "{hostile} was accepted");
        }

        // The two forms the WebView is actually allowed to load still pass.
        let allowed = "https://cdn.cloudflare.steamstatic.com/steam/apps/1/cover.jpg";
        let (game, _) = apple_entry(&apple_result(allowed), "US", NOW_MS).unwrap();
        assert_eq!(game.cover_url, allowed);
        assert_eq!(
            validated_artwork_url("/media/igdb/covers/a.jpg"),
            "/media/igdb/covers/a.jpg"
        );
    }

    fn apple_result(artwork_url: &str) -> serde_json::Value {
        serde_json::json!({
            "trackId": 42,
            "trackName": "Example App",
            "trackViewUrl": "https://apps.apple.com/us/app/example/id42",
            "artworkUrl512": artwork_url,
            "description": "An example listing.",
            "genres": ["Games"],
            "kind": "mac-software",
            "price": 9.99,
            "currency": "USD",
        })
    }

    // -- recommendations -----------------------------------------------------

    #[test]
    fn recommendations_switch_at_three_played_games() {
        let games = vec![
            game_fixture(vec![offer_fixture(
                "offer_a",
                "https://store.steampowered.com/app/1/",
            )]),
            CachedGame {
                id: "steam:2".to_string(),
                title: "Windows Only".to_string(),
                genres: vec!["Racing".to_string()],
                tags: vec!["Long Sessions".to_string()],
                supported_platforms: vec![GamePlatform::Windows],
                ..game_fixture(Vec::new())
            },
        ];

        let library = vec![played("One", "Strategy"), played("Two", "Strategy")];
        let profile = LibraryProfile::from_played(&library);
        assert!(!profile.is_personalized());
        let (summaries, mode, heading) = recommend(&games, &profile, NOW_MS);
        assert_eq!(mode, RecommendationMode::Editorial);
        assert_eq!(heading, "Editorial picks");
        assert_eq!(summaries[0].recommendation_reasons, vec!["Editorial"]);

        let library = vec![
            played("One", "Strategy"),
            played("Two", "Strategy"),
            played("Three", "Strategy"),
        ];
        let profile = LibraryProfile::from_played(&library);
        assert!(profile.is_personalized());
        let (summaries, mode, heading) = recommend(&games, &profile, NOW_MS);
        assert_eq!(mode, RecommendationMode::Personalized);
        assert_eq!(heading, "Picked from the games you play");
        assert_eq!(summaries[0].id, "steam:1");
        assert!(
            summaries[0]
                .recommendation_reasons
                .contains(&"Because you play strategy games".to_string())
        );
        assert!(
            summaries[0]
                .recommendation_reasons
                .contains(&"Runs natively on macOS".to_string())
        );
        // Every reason must point at a fact, never at a claim about the player.
        for summary in &summaries {
            assert!(!summary.recommendation_reasons.is_empty());
            for reason in &summary.recommendation_reasons {
                let lowered = reason.to_lowercase();
                assert!(!lowered.contains("ai"));
                assert!(!lowered.contains("we think"));
                assert!(!lowered.contains("you will love"));
            }
        }
    }

    #[test]
    fn a_live_discount_raises_a_recommendation_and_is_explained() {
        let discounted = CachedGame {
            id: "steam:3".to_string(),
            title: "Discounted".to_string(),
            genres: vec!["Racing".to_string()],
            supported_platforms: vec![GamePlatform::Windows],
            offers: vec![CachedOffer {
                discount_percent: 40,
                ..offer_fixture("offer_d", "https://store.steampowered.com/app/3/")
            }],
            ..game_fixture(Vec::new())
        };
        let profile = LibraryProfile::from_played(&[
            played("One", "Racing"),
            played("Two", "Racing"),
            played("Three", "Racing"),
        ]);
        let (score, reasons) = profile.score(&discounted);
        assert!(score > 0);
        assert!(reasons.contains(&"Discounted by 40% right now".to_string()));
    }

    #[test]
    fn a_library_snapshot_is_read_tolerantly() {
        let directory = TestDirectory::new("library");
        assert!(read_played_games(&directory.root).is_empty());

        fs::write(directory.root.join("catalog.json"), b"not json").unwrap();
        assert!(read_played_games(&directory.root).is_empty());

        fs::write(
            directory.root.join("catalog.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 7,
                "games": [
                    {
                        "id": "steam:1",
                        "title": "Played",
                        "play_time_seconds": 7_200,
                        "orivo_steam_genre": "Strategy, Simulation",
                        "unknown_future_field": true
                    },
                    { "id": "steam:2", "title": "Never played" }
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let played_games = read_played_games(&directory.root);
        assert_eq!(played_games.len(), 2);
        assert_eq!(played_games[0].genres, vec!["Strategy", "Simulation"]);
        let profile = LibraryProfile::from_played(&played_games);
        assert_eq!(profile.played_count, 1);
        assert!(!profile.is_personalized());
    }

    // -- browsing ------------------------------------------------------------

    #[test]
    fn browsing_filters_paginates_and_never_touches_the_network() {
        let directory = TestDirectory::new("browse");
        let cache = directory.cache();

        let page = browse(
            &cache,
            &directory.root,
            &[],
            &StoreBrowseRequest {
                category: StoreCategory::ShortSessions,
                providers: vec![StoreProviderId::Steam],
                platforms: Vec::new(),
                query: String::new(),
                cursor: None,
                limit: 2,
            },
            NOW_MS,
        );
        assert_eq!(page.games.len(), 2);
        assert_eq!(page.next_cursor.as_deref(), Some("store_2"));
        for game in &page.games {
            // The curator's own shelf assignment is what put it here.
            assert!(game.curation.as_ref().is_some_and(|curation| {
                curation.categories.contains(&StoreCategory::ShortSessions)
            }));
        }
        assert_eq!(page.provider_statuses.len(), StoreProviderId::ALL.len());

        // Paging walks the whole shelf and stops exactly once.
        let short_session_games = catalog_games()
            .iter()
            .filter(|game| matches_category(game, StoreCategory::ShortSessions))
            .count();
        let rest = browse(
            &cache,
            &directory.root,
            &[],
            &StoreBrowseRequest {
                category: StoreCategory::ShortSessions,
                providers: Vec::new(),
                platforms: Vec::new(),
                query: String::new(),
                cursor: page.next_cursor.clone(),
                limit: MAX_BROWSE_LIMIT,
            },
            NOW_MS,
        );
        assert_eq!(rest.games.len(), short_session_games - 2);
        assert!(rest.next_cursor.is_none());

        // A provider nobody sells these games on yields an empty page, not an
        // error.
        let empty = browse(
            &cache,
            &directory.root,
            &[],
            &StoreBrowseRequest {
                category: StoreCategory::AllGames,
                providers: vec![StoreProviderId::InstantGaming],
                platforms: Vec::new(),
                query: String::new(),
                cursor: None,
                limit: 10,
            },
            NOW_MS,
        );
        assert!(empty.games.is_empty());
        assert!(empty.next_cursor.is_none());

        // Query matching folds case and diacritics.
        let searched = browse(
            &cache,
            &directory.root,
            &[],
            &StoreBrowseRequest {
                category: StoreCategory::AllGames,
                providers: Vec::new(),
                platforms: Vec::new(),
                query: "  HÁDES ".to_string(),
                cursor: None,
                limit: 10,
            },
            NOW_MS,
        );
        assert_eq!(searched.games.len(), 1);
        assert_eq!(searched.games[0].title, "Hades");
    }

    #[test]
    fn browse_limits_are_clamped_and_bad_cursors_are_ignored() {
        let directory = TestDirectory::new("clamp");
        let request = StoreBrowseRequest {
            category: StoreCategory::AllGames,
            providers: Vec::new(),
            platforms: Vec::new(),
            query: String::new(),
            cursor: Some("store_not-a-number".to_string()),
            limit: 10_000,
        };
        let page = browse(&directory.cache(), &directory.root, &[], &request, NOW_MS);
        assert_eq!(page.games.len(), MAX_BROWSE_LIMIT.min(CATALOG_SIZE));
        assert_eq!(parse_cursor(Some("nonsense")), 0);
        assert_eq!(parse_cursor(Some("store_4")), 4);
    }

    #[test]
    fn good_for_brain_selects_thinking_games_only() {
        let puzzler = CachedGame {
            id: "steam:10".to_string(),
            genres: vec!["Puzzle".to_string()],
            tags: vec!["Logic".to_string()],
            ..game_fixture(Vec::new())
        };
        let strategist = CachedGame {
            id: "steam:11".to_string(),
            genres: vec!["Strategy".to_string()],
            tags: vec!["Thinking".to_string()],
            ..game_fixture(Vec::new())
        };
        let shooter = CachedGame {
            id: "steam:12".to_string(),
            genres: vec!["Action".to_string()],
            tags: vec!["Fast Paced".to_string()],
            ..game_fixture(Vec::new())
        };

        assert!(matches_category(&puzzler, StoreCategory::GoodForBrain));
        assert!(matches_category(&strategist, StoreCategory::GoodForBrain));
        assert!(!matches_category(&shooter, StoreCategory::GoodForBrain));
        // The new category is a filter, not a reordering of the other ones.
        assert!(matches_category(&shooter, StoreCategory::AllGames));
        assert!(matches_category(&shooter, StoreCategory::ForYou));
        assert!(!matches_category(&puzzler, StoreCategory::Relaxing));

        // A curated catalogue game is placed by its curator, not by a keyword
        // that happens to be in an English tag. The catalogue speaks French.
        let curated = CachedGame {
            id: "steam:13".to_string(),
            genres: vec!["Aventure".to_string()],
            tags: vec!["Récits forts".to_string()],
            curation: Some(StoreCuration {
                categories: vec![StoreCategory::GoodForBrain],
                ..StoreCuration::default()
            }),
            ..game_fixture(Vec::new())
        };
        assert!(matches_category(&curated, StoreCategory::GoodForBrain));
        assert!(!matches_category(&curated, StoreCategory::StrongStories));
        // French facts still read for anything nobody curated.
        let french = CachedGame {
            genres: vec!["Réflexion".to_string()],
            tags: vec!["Bon pour le cerveau".to_string()],
            curation: None,
            ..game_fixture(Vec::new())
        };
        assert!(matches_category(&french, StoreCategory::GoodForBrain));

        // It reaches the real pool too, so an empty rail would be a regression.
        let directory = TestDirectory::new("brain");
        let page = browse(
            &directory.cache(),
            &directory.root,
            &[],
            &StoreBrowseRequest {
                category: StoreCategory::GoodForBrain,
                providers: Vec::new(),
                platforms: Vec::new(),
                query: String::new(),
                cursor: None,
                limit: 10,
            },
            NOW_MS,
        );
        assert!(!page.games.is_empty());
        for game in &page.games {
            assert!(
                game.curation.as_ref().is_some_and(|curation| curation
                    .categories
                    .contains(&StoreCategory::GoodForBrain)),
                "{} is not on the thinking shelf",
                game.title
            );
        }
    }

    #[test]
    fn platform_filters_apply_alongside_provider_filters() {
        for (provider, expected) in [
            (StoreProviderId::Steam, Some(StorePlatformId::Pc)),
            (StoreProviderId::Epic, Some(StorePlatformId::Pc)),
            (StoreProviderId::Gog, Some(StorePlatformId::Pc)),
            (StoreProviderId::Humble, Some(StorePlatformId::Pc)),
            (StoreProviderId::Fanatical, Some(StorePlatformId::Pc)),
            (StoreProviderId::GreenManGaming, Some(StorePlatformId::Pc)),
            (StoreProviderId::InstantGaming, Some(StorePlatformId::Pc)),
            (StoreProviderId::Ubisoft, Some(StorePlatformId::Pc)),
            (StoreProviderId::Microsoft, Some(StorePlatformId::Xbox)),
            (
                StoreProviderId::PlayStation,
                Some(StorePlatformId::PlayStation),
            ),
            (StoreProviderId::Nintendo, Some(StorePlatformId::Switch)),
            // Mobile storefronts have no pill, so they are never guessed into
            // one.
            (StoreProviderId::Apple, None),
            (StoreProviderId::GooglePlay, None),
        ] {
            assert_eq!(provider.platform(), expected, "{provider:?}");
        }

        let steam_game = game_fixture(vec![offer_fixture(
            "offer_pc",
            "https://store.steampowered.com/app/1/",
        )]);
        let console_game = CachedGame {
            id: "psn:1".to_string(),
            supported_platforms: Vec::new(),
            offers: vec![CachedOffer {
                game_id: "psn:1".to_string(),
                provider: StoreProviderId::PlayStation,
                ..offer_fixture("offer_psn", "https://store.playstation.com/app/1/")
            }],
            ..game_fixture(Vec::new())
        };
        // No offer at all, but the game declares Windows support.
        let declared_only = CachedGame {
            id: "steam:20".to_string(),
            supported_platforms: vec![GamePlatform::Windows],
            offers: Vec::new(),
            ..game_fixture(Vec::new())
        };
        let mobile_only = CachedGame {
            id: "apple:1".to_string(),
            supported_platforms: vec![GamePlatform::Ios],
            offers: Vec::new(),
            ..game_fixture(Vec::new())
        };

        // An empty filter never excludes anything.
        assert!(matches_platforms(&mobile_only, &[]));

        assert!(matches_platforms(&steam_game, &[StorePlatformId::Pc]));
        assert!(matches_platforms(&declared_only, &[StorePlatformId::Pc]));
        assert!(!matches_platforms(&console_game, &[StorePlatformId::Pc]));
        assert!(matches_platforms(
            &console_game,
            &[StorePlatformId::PlayStation]
        ));
        // Multi-select is a union.
        assert!(matches_platforms(
            &console_game,
            &[StorePlatformId::Pc, StorePlatformId::PlayStation]
        ));
        // Nothing maps onto emulators, so it excludes rather than invents.
        assert!(!matches_platforms(
            &steam_game,
            &[StorePlatformId::Emulators]
        ));
        assert!(!matches_platforms(&mobile_only, &[StorePlatformId::Pc]));

        // Both filters are applied, not one or the other.
        assert!(matches_request(
            &steam_game,
            StoreCategory::AllGames,
            &[StoreProviderId::Steam],
            &[StorePlatformId::Pc],
            ""
        ));
        assert!(!matches_request(
            &steam_game,
            StoreCategory::AllGames,
            &[StoreProviderId::Steam],
            &[StorePlatformId::Switch],
            ""
        ));
        assert!(!matches_request(
            &steam_game,
            StoreCategory::AllGames,
            &[StoreProviderId::Epic],
            &[StorePlatformId::Pc],
            ""
        ));

        // End to end through the real pool: every catalogue game is sold on
        // PC, and only the ones with a Nintendo offer answer a Switch filter.
        let directory = TestDirectory::new("platforms");
        let request = |platforms: Vec<StorePlatformId>| StoreBrowseRequest {
            category: StoreCategory::AllGames,
            providers: Vec::new(),
            platforms,
            query: String::new(),
            cursor: None,
            limit: MAX_BROWSE_LIMIT,
        };
        let pc = browse(
            &directory.cache(),
            &directory.root,
            &[],
            &request(vec![StorePlatformId::Pc]),
            NOW_MS,
        );
        assert_eq!(pc.games.len(), CATALOG_SIZE);
        let expected_switch = catalog_games()
            .iter()
            .filter(|game| {
                game.offers
                    .iter()
                    .any(|offer| offer.provider == StoreProviderId::Nintendo)
            })
            .count();
        let switch = browse(
            &directory.cache(),
            &directory.root,
            &[],
            &request(vec![StorePlatformId::Switch]),
            NOW_MS,
        );
        assert_eq!(switch.games.len(), expected_switch);
        assert!(
            switch.games.len() < CATALOG_SIZE,
            "the filter matched everything"
        );
        for game in &switch.games {
            assert!(
                game.offers
                    .iter()
                    .any(|offer| offer.provider == StoreProviderId::Nintendo),
                "{} has no Switch storefront",
                game.title
            );
        }
        assert_eq!(switch.provider_statuses.len(), StoreProviderId::ALL.len());

        // A machine nothing in the catalogue is sold for empties the page
        // without erroring.
        let emulators = browse(
            &directory.cache(),
            &directory.root,
            &[],
            &request(vec![StorePlatformId::Emulators]),
            NOW_MS,
        );
        assert!(emulators.games.is_empty());
        assert!(emulators.next_cursor.is_none());
    }

    // -- the embedded catalogue ----------------------------------------------

    #[test]
    fn the_embedded_catalogue_is_the_curated_forty_seven_games() {
        let games = catalog_games();
        assert_eq!(
            games.len(),
            CATALOG_SIZE,
            "the embedded catalogue is not the curated catalogue"
        );

        // It is the same catalogue the WebView bundles, not a copy that has
        // been allowed to drift: same order, same ids, same titles.
        assert_eq!(games[0].id, "steam:1608230");
        assert_eq!(games[0].title, "Planet of Lana");

        let mut seen = BTreeSet::new();
        for game in games {
            assert!(seen.insert(game.id.clone()), "{} appears twice", game.id);
            assert!(
                steam_app_id(&game.id).is_some(),
                "{} is not addressable",
                game.id
            );
            assert!(!game.title.trim().is_empty());
            assert!(!game.short_description.trim().is_empty());
            // Artwork is a bundled asset path, never a remote origin the WebView
            // would then have to fetch from.
            for artwork in [&game.cover_url, &game.hero_url, &game.landscape_url] {
                assert!(
                    artwork.starts_with("/media/store/"),
                    "{artwork} is not bundled"
                );
            }
            assert!(!game.offers.is_empty(), "{} is sold nowhere", game.id);
            assert!(
                game.curation.is_some(),
                "{} lost its French copy on the way in",
                game.id
            );
            // None of the old hard-coded shelf survives anywhere.
            for banned in ["Elden Ring", "Cyberpunk 2077", "Unrailed!", "Astro Duel 2"] {
                assert_ne!(game.title, banned, "the old editorial shelf is still here");
            }
        }
    }

    #[test]
    fn a_catalogue_row_keeps_the_price_and_the_timestamp_it_was_given() {
        // Read straight out of the embedded document so the assertion is about
        // the bytes on disk, not about a fixture that mirrors them.
        let raw: Vec<serde_json::Value> = serde_json::from_str(STORE_CATALOG_JSON).unwrap();
        assert_eq!(raw.len(), CATALOG_SIZE);

        for row in &raw {
            let game = catalog_games()
                .iter()
                .find(|game| game.id == row["id"].as_str().unwrap())
                .expect("every row is embedded");
            for (index, offer) in row["offers"].as_array().unwrap().iter().enumerate() {
                let cached = &game.offers[index];
                assert_eq!(cached.id, offer["id"].as_str().unwrap());
                assert_eq!(cached.price_minor, offer["priceMinor"].as_i64());
                assert_eq!(
                    cached.currency.as_deref(),
                    offer["currency"].as_str(),
                    "{} changed currency",
                    cached.id
                );
                assert_eq!(cached.region, offer["region"].as_str().unwrap());
                assert_eq!(
                    serde_json::to_value(cached.availability).unwrap(),
                    offer["availability"]
                );
                // A null stays null; a quoted instant survives the round trip
                // to the second the contract renders.
                match offer["verifiedAt"].as_str() {
                    None => assert_eq!(cached.verified_at_epoch_ms, None),
                    Some(verified_at) => {
                        let epoch_ms = cached.verified_at_epoch_ms.expect("a parsed timestamp");
                        assert_eq!(
                            iso8601_from_epoch_ms(epoch_ms),
                            format!("{}Z", verified_at.split('.').next().unwrap())
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_catalogue_row_this_build_cannot_read_is_dropped_not_fatal() {
        let games = parse_catalog(
            &serde_json::json!([
                {
                    "id": "steam:1",
                    "title": "Readable",
                    "supportedPlatforms": ["windows", "holodeck"],
                    "offers": [
                        {
                            "id": "offer_steam_steam1",
                            "provider": "steam",
                            "priceMinor": 1_999,
                            "currency": "EUR",
                            "region": "FR",
                            "verifiedAt": "2026-08-02T18:10:02.752Z",
                            "availability": "available"
                        },
                        {
                            "id": "offer_future_steam1",
                            "provider": "some-future-shop",
                            "priceMinor": 1,
                            "currency": "EUR"
                        }
                    ],
                    "curation": { "categories": ["relaxing", "time-travel"], "duration": "2h" }
                },
                { "title": "No id at all" },
                { "id": "steam:8", "offers": [] },
                {
                    "id": "apple:7",
                    "title": "Sold somewhere Orivo cannot address yet",
                    "offers": [
                        { "id": "offer_apple_7", "provider": "apple", "priceMinor": 299, "currency": "EUR" }
                    ]
                }
            ])
            .to_string(),
        );

        assert_eq!(games.len(), 2, "a readable row was thrown away");
        assert_eq!(games[0].supported_platforms, vec![GamePlatform::Windows]);
        assert_eq!(games[0].offers.len(), 1, "an unknown shop was accepted");
        assert_eq!(games[0].offers[0].price_minor, Some(1_999));
        assert_eq!(
            games[0].offers[0].url,
            "https://store.steampowered.com/app/1/"
        );

        // A row Orivo cannot address still sells at the price it was given; it
        // simply has no page to open.
        assert_eq!(games[1].id, "apple:7");
        assert_eq!(games[1].offers[0].price_minor, Some(299));
        assert!(games[1].offers[0].url.is_empty());

        let curation = games[0].curation.clone().unwrap();
        assert_eq!(curation.categories, vec![StoreCategory::Relaxing]);
        assert_eq!(curation.duration, "2h");

        // A document that is not a catalogue at all leaves an empty shelf
        // rather than taking the process down.
        assert!(parse_catalog("not json").is_empty());
        assert!(parse_catalog("{}").is_empty());
    }

    // -- ownership -----------------------------------------------------------

    #[test]
    fn the_store_never_sells_a_game_the_library_already_owns() {
        let directory = TestDirectory::new("owned");
        let catalogue = catalog_games();
        let by_id = &catalogue[0];
        let by_title = &catalogue[1];

        let library = vec![
            // Same id as the catalogue row.
            owned(&by_id.id, "Something else entirely"),
            // Same game, imported from Steam under a different source id and
            // written in caps with an edition suffix stripped by the key.
            owned("steam-import:9999", &by_title.title.to_uppercase()),
            owned("local:1", "A game that is not in the store"),
        ];

        let home = store_home(&directory.cache(), &directory.root, &library, NOW_MS);
        assert_eq!(home.games.len(), CATALOG_SIZE - 2);
        assert!(!home.games.iter().any(|game| game.id == by_id.id));
        assert!(!home.games.iter().any(|game| game.title == by_title.title));

        let page = browse(
            &directory.cache(),
            &directory.root,
            &library,
            &StoreBrowseRequest {
                category: StoreCategory::AllGames,
                providers: Vec::new(),
                platforms: Vec::new(),
                query: String::new(),
                cursor: None,
                limit: MAX_BROWSE_LIMIT,
            },
            NOW_MS,
        );
        assert_eq!(page.games.len(), CATALOG_SIZE - 2);
        assert!(!page.games.iter().any(|game| game.id == by_id.id));
        assert!(!page.games.iter().any(|game| game.title == by_title.title));

        // The exclusion also holds for what a refresh wrote into the cache,
        // not only for the built-in shelf.
        directory
            .cache()
            .write(&StoreCacheDocument {
                games: catalogue.to_vec(),
                provider_statuses: default_provider_statuses(),
                ..StoreCacheDocument::default()
            })
            .unwrap();
        let cached_home = store_home(&directory.cache(), &directory.root, &library, NOW_MS);
        assert_eq!(cached_home.games.len(), CATALOG_SIZE - 2);

        // An empty library changes nothing.
        assert_eq!(
            store_home(&directory.cache(), &directory.root, &[], NOW_MS)
                .games
                .len(),
            CATALOG_SIZE
        );
    }

    #[test]
    fn ownership_matches_on_a_normalised_title() {
        assert_eq!(ownership_key("Planet of Lana"), "planetoflana");
        // Case, punctuation, spacing and accents are all folded, because the
        // same game is written every one of those ways across sources.
        assert_eq!(ownership_key("  PLANET  of Lana!  "), "planetoflana");
        assert_eq!(ownership_key("Wilmot's Warehouse"), "wilmotswarehouse");
        assert_eq!(ownership_key("Gris"), ownership_key("GRIS"));
        assert_eq!(ownership_key("Célèste"), "celeste");
        // Two different games never collide into one key.
        assert_ne!(ownership_key("Mini Metro"), ownership_key("Mini Motorways"));
        // A blank title is no ownership signal at all.
        assert!(ownership_key("   ").is_empty());

        let index = OwnedIndex::new(&[owned("steam:1", ""), owned("", "Fixture")]);
        assert!(index.owns(&game_fixture(Vec::new())));
        assert!(index.owns(&CachedGame {
            id: "other:2".to_string(),
            ..game_fixture(Vec::new())
        }));
        assert!(!index.owns(&CachedGame {
            id: "other:2".to_string(),
            title: "Unowned".to_string(),
            ..game_fixture(Vec::new())
        }));
    }

    // -- refresh -------------------------------------------------------------

    #[test]
    fn the_whole_catalogue_refreshes_in_batches_over_the_public_endpoint() {
        let targets = steam_refresh_targets(catalog_games());
        assert_eq!(
            targets.len(),
            CATALOG_SIZE,
            "the shelf is only partly priced"
        );
        assert!(
            MAX_STEAM_REFRESH_APPS >= CATALOG_SIZE,
            "the refresh cap is below the catalogue"
        );

        // Steam answers a comma-separated `appids` list when `filters` is set,
        // so the catalogue costs one request per batch and no more.
        let priced: serde_json::Map<String, serde_json::Value> = targets
            .iter()
            .map(|target| {
                (
                    target.app_id.clone(),
                    serde_json::json!({
                        "success": true,
                        "data": { "price_overview": { "currency": "EUR", "final": 1_999, "discount_percent": 10 } }
                    }),
                )
            })
            .collect();
        let http = FakeHttp::default().with(
            "store.steampowered.com/api/appdetails",
            serde_json::Value::Object(priced),
        );
        let config = RefreshConfig {
            region: "FR".to_string(),
        };
        let (outcome, status) = block_on(refresh_steam(&http, &config, catalog_games(), NOW_MS));

        assert_eq!(outcome.offers.len(), CATALOG_SIZE);
        assert_eq!(status.health, ProviderHealth::Available);
        let requested = http.requested.lock().unwrap().clone();
        assert_eq!(
            requested.len(),
            CATALOG_SIZE.div_ceil(STEAM_REFRESH_BATCH),
            "the refresh did not batch: {requested:?}"
        );
        for url in &requested {
            // The public storefront endpoint, in the user's region and
            // language, with the filter that makes a multi-id list work.
            assert!(url.starts_with("https://store.steampowered.com/api/appdetails?appids="));
            assert!(url.contains("&cc=fr"));
            assert!(url.contains("&l=french"));
            assert!(url.contains("&filters=price_overview"));
            assert!(!url.contains("key="), "a key was sent to a public endpoint");
        }
        for offer in &outcome.offers {
            assert_eq!(offer.price_minor, Some(1_999));
            assert_eq!(offer.currency.as_deref(), Some("EUR"));
            assert_eq!(offer.region, "FR");
            assert_eq!(offer.discount_percent, 10);
        }
        // The refreshed offers replace the shipped ones instead of doubling up.
        let mut games = catalog_games().to_vec();
        apply_outcome(&mut games, outcome);
        for game in &games {
            assert_eq!(
                game.offers
                    .iter()
                    .filter(|offer| offer.provider == StoreProviderId::Steam)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn a_batch_that_answers_for_one_app_falls_back_to_one_request_per_app() {
        let targets = steam_refresh_targets(catalog_games());
        // The shape Steam returns when a multi-id request is not honoured: the
        // first id only. Every other app has to be asked for on its own.
        let http = FakeHttp::default().with(
            "store.steampowered.com/api/appdetails",
            serde_json::json!({
                targets[0].app_id.clone(): {
                    "success": true,
                    "data": { "price_overview": { "currency": "EUR", "final": 499, "discount_percent": 75 } }
                }
            }),
        );
        let (outcome, status) = block_on(refresh_steam(
            &http,
            &RefreshConfig::default(),
            catalog_games(),
            NOW_MS,
        ));

        let requested = http.requested.lock().unwrap().clone();
        let batches = CATALOG_SIZE.div_ceil(STEAM_REFRESH_BATCH);
        // One request per batch, then one per app the batches left unanswered.
        assert_eq!(requested.len(), batches + CATALOG_SIZE - 1);
        let singles: Vec<&String> = requested
            .iter()
            .filter(|url| !url.contains("filters=price_overview"))
            .collect();
        assert_eq!(singles.len(), CATALOG_SIZE - 1);
        for url in singles {
            assert!(!url.contains(','), "a fallback request was still batched");
        }

        // The one app that did answer keeps its live price; the rest degrade
        // into an honest status instead of a guess.
        assert_eq!(outcome.offers.len(), 1);
        assert_eq!(outcome.offers[0].price_minor, Some(499));
        assert_eq!(status.health, ProviderHealth::Degraded);
    }

    #[test]
    fn a_steam_that_answers_nothing_is_not_walked_game_by_game() {
        let http = FakeHttp::default();
        let (outcome, status) = block_on(refresh_steam(
            &http,
            &RefreshConfig::default(),
            catalog_games(),
            NOW_MS,
        ));

        let requested = http.requested.lock().unwrap().len();
        let batches = CATALOG_SIZE.div_ceil(STEAM_REFRESH_BATCH);
        assert_eq!(
            requested,
            batches + STEAM_REFRESH_GIVE_UP_AFTER,
            "an unreachable Steam was still asked about every game"
        );
        assert!(outcome.offers.is_empty());
        assert_eq!(status.health, ProviderHealth::Degraded);
        assert!(status.refreshed_at_epoch_ms.is_none());
    }

    #[test]
    fn a_failing_refresh_never_blocks_or_empties_the_store() {
        let directory = TestDirectory::new("refresh-fails");
        let http = FakeHttp::default();

        // Nothing answers, so every provider degrades...
        let document = block_on(refresh_all(&http, &RefreshConfig::default(), NOW_MS));
        assert_eq!(document.games.len(), CATALOG_SIZE);
        directory.cache().write(&document).unwrap();

        // ...and the home view is still the whole catalogue, with the prices
        // the catalogue shipped with rather than none at all.
        let home = store_home(&directory.cache(), &directory.root, &[], NOW_MS);
        assert_eq!(home.games.len(), CATALOG_SIZE);
        assert!(
            home.games
                .iter()
                .flat_map(|game| game.offers.iter())
                .any(|offer| offer.price_minor.is_some())
        );
        // The French copy survives the trip through the cache document, which
        // is the only path the card's text can be lost on.
        assert!(home.games.iter().all(|game| game.curation.is_some()));
        let lana = home
            .games
            .iter()
            .find(|game| game.id == "steam:1608230")
            .unwrap();
        assert_eq!(
            lana.curation.as_ref().unwrap().tagline,
            "Histoire émouvante sans violence."
        );
        assert_eq!(lana.curation.as_ref().unwrap().stats.len(), 3);
        assert!(
            home.provider_statuses
                .iter()
                .all(|status| !status.message.trim().is_empty())
        );
    }

    #[test]
    fn catalogue_timestamps_survive_the_round_trip() {
        assert_eq!(
            epoch_ms_from_iso8601("2026-07-25T17:20:00Z"),
            Some(1_785_000_000_000)
        );
        assert_eq!(
            epoch_ms_from_iso8601("2026-07-25T17:20:00.752Z"),
            Some(1_785_000_000_752)
        );
        assert_eq!(
            iso8601_from_epoch_ms(epoch_ms_from_iso8601("2026-08-02T18:10:02.752Z").unwrap()),
            "2026-08-02T18:10:02Z"
        );
        assert_eq!(epoch_ms_from_iso8601("1970-01-01T00:00:00Z"), Some(0));
        for hostile in [
            "",
            "2026-08-02",
            "2026-08-02T18:10:02",
            "2026-08-02T18:10:02+02:00",
            "2026-13-02T18:10:02Z",
            "2026-08-02T25:10:02Z",
            "not a timestamp at all",
            "1969-12-31T23:59:59Z",
        ] {
            assert_eq!(
                epoch_ms_from_iso8601(hostile),
                None,
                "{hostile} was accepted"
            );
        }
    }

    // -- misc ----------------------------------------------------------------

    #[test]
    fn the_region_falls_back_when_the_preference_is_missing_or_automatic() {
        let directory = TestDirectory::new("region");
        assert_eq!(resolve_region(&directory.root), DEFAULT_REGION);

        fs::write(
            directory.root.join(PREFERENCES_FILE),
            br#"{"startPage":"library","storeRegion":"automatic","motion":"system"}"#,
        )
        .unwrap();
        assert_eq!(resolve_region(&directory.root), DEFAULT_REGION);

        fs::write(
            directory.root.join(PREFERENCES_FILE),
            br#"{"storeRegion":"fr"}"#,
        )
        .unwrap();
        assert_eq!(resolve_region(&directory.root), "FR");
    }

    #[test]
    fn timestamps_render_as_utc_iso_8601() {
        assert_eq!(iso8601_from_epoch_ms(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            iso8601_from_epoch_ms(1_767_225_600_000),
            "2026-01-01T00:00:00Z"
        );
        assert_eq!(
            iso8601_from_epoch_ms(1_709_164_800_000),
            "2024-02-29T00:00:00Z"
        );
    }
}
