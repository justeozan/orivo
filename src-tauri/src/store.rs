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
    collections::BTreeMap,
    fs,
    future::Future,
    io::{self, Write as _},
    path::{Path, PathBuf},
    pin::Pin,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// Derived cache document. The plan named a SQLite database; this crate has no
/// SQLite dependency, so the same contract is met by a versioned JSON document
/// with an atomic writer. Unknown or corrupt content is discarded and rebuilt.
const STORE_CACHE_FILE: &str = "store-cache.json";
const STORE_CACHE_SCHEMA_VERSION: u32 = 1;
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
const MAX_STEAM_REFRESH_APPS: usize = 10;
const APPLE_REFRESH_TERM: &str = "game";
const APPLE_REFRESH_LIMIT: usize = 10;

/// Host-side configuration only. The WebView cannot influence any of these.
const STEAM_API_KEY_ENV: &str = "ORIVO_STEAM_WEB_API_KEY";
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
    Ubisoft,
    Microsoft,
    Apple,
    GooglePlay,
    InstantGaming,
}

impl StoreProviderId {
    const ALL: [Self; 6] = [
        Self::Steam,
        Self::Ubisoft,
        Self::Microsoft,
        Self::Apple,
        Self::GooglePlay,
        Self::InstantGaming,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Steam => "Steam",
            Self::Ubisoft => "Ubisoft",
            Self::Microsoft => "Microsoft/Xbox",
            Self::Apple => "Apple App Store",
            Self::GooglePlay => "Google Play",
            Self::InstantGaming => "Instant Gaming",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::Steam => "steam",
            Self::Ubisoft => "ubisoft",
            Self::Microsoft => "microsoft",
            Self::Apple => "apple",
            Self::GooglePlay => "google-play",
            Self::InstantGaming => "instant-gaming",
        }
    }
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
            .or_else(|| find_offer_url(&editorial_games(), offer_id))
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
// Editorial baseline. Mirrors `src/store-model.ts` so the shell and the
// WebView agree on identifiers and artwork before any network call happens.
// ---------------------------------------------------------------------------

struct EditorialSeed {
    app_id: &'static str,
    title: &'static str,
    file: &'static str,
    hero_file: Option<&'static str>,
    landscape_file: &'static str,
    description: &'static str,
    genres: &'static [&'static str],
    tags: &'static [&'static str],
    platforms: &'static [GamePlatform],
    reasons: &'static [&'static str],
}

const EDITORIAL_SEEDS: &[EditorialSeed] = &[
    EditorialSeed {
        app_id: "1245620",
        title: "Elden Ring",
        file: "elden-ring.jpg",
        hero_file: Some("elden-ring-wallpaper.png"),
        landscape_file: "elden-ring.jpg",
        description: "Explore a vast open world shaped by discovery, difficult encounters, and player choice.",
        genres: &["Action", "RPG"],
        tags: &["Open World", "Strong Stories", "Long Sessions"],
        platforms: &[GamePlatform::Windows],
        reasons: &[
            "Matches action RPGs",
            "Tagged open world",
            "Single-player campaign",
        ],
    },
    EditorialSeed {
        app_id: "1091500",
        title: "Cyberpunk 2077",
        file: "cyberpunk-2077.jpg",
        hero_file: Some("cyberpunk-2077.webp"),
        landscape_file: "cyberpunk-2077.webp",
        description: "Build a mercenary's story across the dense districts and shifting alliances of Night City.",
        genres: &["Action", "RPG"],
        tags: &["Strong Stories", "Open World", "Single-player"],
        platforms: &[GamePlatform::Windows, GamePlatform::Macos],
        reasons: &[
            "Story-rich campaign",
            "Matches action RPGs",
            "Available on macOS",
        ],
    },
    EditorialSeed {
        app_id: "1086940",
        title: "Baldur's Gate 3",
        file: "baldurs-gate-3.jpg",
        hero_file: None,
        landscape_file: "baldurs-gate-3.jpg",
        description: "Shape a party-driven adventure where combat, conversation, and exploration share the stage.",
        genres: &["RPG", "Strategy"],
        tags: &["Strong Stories", "Choices Matter", "Co-op"],
        platforms: &[GamePlatform::Windows, GamePlatform::Macos],
        reasons: &[
            "Available on macOS",
            "Story-rich campaign",
            "Supports co-op",
        ],
    },
    EditorialSeed {
        app_id: "1145350",
        title: "Hades II",
        file: "hades-2.jpg",
        hero_file: None,
        landscape_file: "hades-2.jpg",
        description: "Battle beyond the Underworld in focused runs that reveal more of the story each time.",
        genres: &["Action", "Roguelike"],
        tags: &["Short Sessions", "Replayable", "Strong Stories"],
        platforms: &[GamePlatform::Windows, GamePlatform::Macos],
        reasons: &[
            "Works in short sessions",
            "Available on macOS",
            "Replayable runs",
        ],
    },
    EditorialSeed {
        app_id: "1174180",
        title: "Red Dead Redemption 2",
        file: "red-dead-redemption-2.jpg",
        hero_file: None,
        landscape_file: "red-dead-redemption-2.jpg",
        description: "Travel with an outlaw gang through a changing frontier and a long-form character story.",
        genres: &["Action", "Adventure"],
        tags: &["Strong Stories", "Open World", "Atmospheric"],
        platforms: &[GamePlatform::Windows],
        reasons: &[
            "Story-rich campaign",
            "Tagged atmospheric",
            "Single-player adventure",
        ],
    },
    EditorialSeed {
        app_id: "292030",
        title: "The Witcher 3: Wild Hunt",
        file: "the-witcher-3-wild-hunt.jpg",
        hero_file: None,
        landscape_file: "the-witcher-3-wild-hunt.jpg",
        description: "Track monsters and follow interwoven quests across a broad fantasy world.",
        genres: &["RPG", "Adventure"],
        tags: &["Strong Stories", "Open World", "Choices Matter"],
        platforms: &[GamePlatform::Windows],
        reasons: &[
            "Matches RPGs",
            "Story-rich campaign",
            "Tagged choices matter",
        ],
    },
    EditorialSeed {
        app_id: "2420110",
        title: "Horizon Forbidden West",
        file: "horizon-forbidden-west.jpg",
        hero_file: None,
        landscape_file: "horizon-forbidden-west.jpg",
        description: "Cross a colorful frontier of machine encounters, ruins, and character-led quests.",
        genres: &["Action", "Adventure"],
        tags: &["Strong Stories", "Open World", "Exploration"],
        platforms: &[GamePlatform::Windows],
        reasons: &[
            "Story-rich campaign",
            "Tagged exploration",
            "Open-world adventure",
        ],
    },
    EditorialSeed {
        app_id: "1593500",
        title: "God of War",
        file: "god-of-war.jpg",
        hero_file: None,
        landscape_file: "god-of-war.jpg",
        description: "Follow Kratos and Atreus through a focused journey across the Norse realms.",
        genres: &["Action", "Adventure"],
        tags: &["Strong Stories", "Single-player", "Cinematic"],
        platforms: &[GamePlatform::Windows],
        reasons: &[
            "Story-rich campaign",
            "Single-player adventure",
            "Matches action games",
        ],
    },
    EditorialSeed {
        app_id: "1016920",
        title: "Unrailed!",
        file: "unrailed.jpg",
        hero_file: None,
        landscape_file: "unrailed.jpg",
        description: "Build a railway together in quick procedural rounds before the train outruns the track.",
        genres: &["Co-op", "Strategy"],
        tags: &["Short Sessions", "Relaxing", "Local Co-op"],
        platforms: &[
            GamePlatform::Windows,
            GamePlatform::Macos,
            GamePlatform::Linux,
        ],
        reasons: &[
            "Works in short sessions",
            "Available on macOS",
            "Supports local co-op",
        ],
    },
    EditorialSeed {
        app_id: "655350",
        title: "Astro Duel 2",
        file: "astro-duel-2.jpg",
        hero_file: None,
        landscape_file: "astro-duel-2.jpg",
        description: "Switch between ship combat and on-foot action in compact competitive matches.",
        genres: &["Action", "Arcade"],
        tags: &["Short Sessions", "Local Multiplayer", "Campaign"],
        platforms: &[GamePlatform::Windows, GamePlatform::Macos],
        reasons: &[
            "Works in short sessions",
            "Available on macOS",
            "Supports local multiplayer",
        ],
    },
];

fn editorial_games() -> Vec<CachedGame> {
    EDITORIAL_SEEDS
        .iter()
        .map(|seed| {
            let id = format!("steam:{}", seed.app_id);
            CachedGame {
                offers: vec![CachedOffer {
                    id: format!("offer_ed_{}", seed.app_id),
                    game_id: id.clone(),
                    provider: StoreProviderId::Steam,
                    price_minor: None,
                    currency: None,
                    region: DEFAULT_REGION.to_string(),
                    verified_at_epoch_ms: None,
                    availability: OfferAvailability::Unknown,
                    discount_percent: 0,
                    url: steam_app_url(seed.app_id),
                }],
                id,
                title: seed.title.to_string(),
                short_description: seed.description.to_string(),
                cover_url: format!("/media/igdb/covers/{}", seed.file),
                hero_url: format!("/media/igdb/heroes/{}", seed.hero_file.unwrap_or(seed.file)),
                landscape_url: format!("/media/igdb/landscapes/{}", seed.landscape_file),
                genres: seed.genres.iter().map(|value| value.to_string()).collect(),
                tags: seed.tags.iter().map(|value| value.to_string()).collect(),
                supported_platforms: seed.platforms.to_vec(),
                editorial_reasons: seed.reasons.iter().map(|value| value.to_string()).collect(),
            }
        })
        .collect()
}

fn steam_app_url(app_id: &str) -> String {
    format!("https://store.steampowered.com/app/{app_id}/")
}

/// The baseline status for a provider Orivo has no authorized feed for. These
/// are factual statements about configuration, not placeholders for data.
fn unconfigured_status(provider: StoreProviderId) -> CachedProviderStatus {
    let (health, message) = match provider {
        StoreProviderId::Steam => (
            ProviderHealth::NotConfigured,
            "Connect a host-side Steam Web API key for live catalog updates.",
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
    let facts = normalize(&[game.tags.join(" "), game.genres.join(" ")].join(" "));
    match category {
        StoreCategory::ShortSessions => facts.contains("short session"),
        StoreCategory::StrongStories => {
            facts.contains("strong stor")
                || facts.contains("story rich")
                || facts.contains("story-rich")
        }
        StoreCategory::Relaxing => facts.contains("relaxing") || facts.contains("cozy"),
        StoreCategory::ForYou | StoreCategory::AllGames => true,
    }
}

fn matches_request(
    game: &CachedGame,
    category: StoreCategory,
    providers: &[StoreProviderId],
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
        .filter(|game| matches_request(game, request.category, &request.providers, &query))
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
    pub steam_api_key: Option<String>,
    pub region: String,
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            steam_api_key: None,
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
    let mut games = editorial_games();
    let mut statuses = Vec::new();

    let (steam_outcome, steam_status) = refresh_steam(http, config, now_ms).await;
    apply_outcome(&mut games, steam_outcome);
    statuses.push(steam_status);

    let (apple_outcome, apple_status) = refresh_apple(http, config, now_ms).await;
    apply_outcome(&mut games, apple_outcome);
    statuses.push(apple_status);

    // Ubisoft, Microsoft, Google Play, and Instant Gaming have no official
    // catalog contract available to Orivo. They perform no network call and
    // never contribute a price.
    for provider in [
        StoreProviderId::Ubisoft,
        StoreProviderId::Microsoft,
        StoreProviderId::GooglePlay,
        StoreProviderId::InstantGaming,
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

fn steam_api_key() -> Option<String> {
    std::env::var(STEAM_API_KEY_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Steam refresh. Without a host-configured Web API key Orivo performs no
/// Steam network request at all and reports `not-configured`.
async fn refresh_steam(
    http: &dyn StoreHttp,
    config: &RefreshConfig,
    now_ms: u64,
) -> (ProviderOutcome, CachedProviderStatus) {
    if config.steam_api_key.is_none() {
        return (
            ProviderOutcome::default(),
            unconfigured_status(StoreProviderId::Steam),
        );
    }

    let mut outcome = ProviderOutcome::default();
    let mut failures = 0usize;
    let mut attempts = 0usize;
    for seed in EDITORIAL_SEEDS.iter().take(MAX_STEAM_REFRESH_APPS) {
        attempts += 1;
        let url = format!(
            "https://store.steampowered.com/api/appdetails?appids={}&cc={}&l=en",
            seed.app_id,
            config.region.to_lowercase()
        );
        let Ok(payload) = http.get_json(&url).await else {
            failures += 1;
            continue;
        };
        match steam_offer_from_payload(seed, &payload, &config.region, now_ms) {
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
    } else if failures < attempts {
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

fn steam_offer_from_payload(
    seed: &EditorialSeed,
    payload: &serde_json::Value,
    region: &str,
    now_ms: u64,
) -> Option<CachedOffer> {
    let entry = payload.get(seed.app_id)?;
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
    let url = steam_app_url(seed.app_id);
    validate_store_url(&url).ok()?;

    Some(CachedOffer {
        id: format!("offer_ed_{}", seed.app_id),
        game_id: format!("steam:{}", seed.app_id),
        provider: StoreProviderId::Steam,
        price_minor,
        currency,
        region: region.to_string(),
        verified_at_epoch_ms: Some(now_ms),
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

/// Serve immediately from editorial content plus whatever the cache already
/// holds. This never performs a network request.
#[tauri::command]
pub fn get_store_home(app: AppHandle) -> Result<StoreHomeView, String> {
    let directory = app_data_dir(&app)?;
    Ok(store_home(
        &StoreCache::new(&directory),
        &directory,
        now_epoch_ms(),
    ))
}

fn store_home(cache: &StoreCache, app_data_dir: &Path, now_ms: u64) -> StoreHomeView {
    let document = cache.read();
    let games = if document.games.is_empty() {
        editorial_games()
    } else {
        document.games
    };
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
    request: StoreBrowseRequest,
) -> Result<StoreBrowsePage, String> {
    let directory = app_data_dir(&app)?;
    Ok(browse(
        &StoreCache::new(&directory),
        &directory,
        &request,
        now_epoch_ms(),
    ))
}

fn browse(
    cache: &StoreCache,
    app_data_dir: &Path,
    request: &StoreBrowseRequest,
    now_ms: u64,
) -> StoreBrowsePage {
    let document = cache.read();
    let games = if document.games.is_empty() {
        editorial_games()
    } else {
        document.games
    };
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
        steam_api_key: steam_api_key(),
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
        }
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
                "ubisoft",
                "microsoft",
                "apple",
                "google-play",
                "instant-gaming"
            ])
            .as_array()
            .unwrap()
            .clone()
        );

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
            (StoreCategory::ShortSessions, "short-sessions"),
            (StoreCategory::StrongStories, "strong-stories"),
            (StoreCategory::Relaxing, "relaxing"),
            (StoreCategory::AllGames, "all-games"),
        ] {
            assert_eq!(serde_json::to_value(category).unwrap(), expected);
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
        assert_eq!(request.limit, 12);
        assert!(request.cursor.is_none());
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

        // A discarded cache is never fatal: the store still serves editorial.
        let home = store_home(&cache, &directory.root, NOW_MS);
        assert_eq!(home.games.len(), EDITORIAL_SEEDS.len());
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

        // Editorial offers ship with no price at all.
        for game in editorial_games() {
            for offer in &game.offers {
                assert_eq!(offer.price_minor, None);
                assert_eq!(offer.availability, OfferAvailability::Unknown);
            }
        }
    }

    #[test]
    fn a_steam_payload_without_a_price_reports_unknown_availability() {
        let seed = &EDITORIAL_SEEDS[0];
        let payload = serde_json::json!({
            seed.app_id: { "success": true, "data": { "name": seed.title } }
        });
        let offer = steam_offer_from_payload(seed, &payload, "US", NOW_MS).unwrap();
        assert_eq!(offer.price_minor, None);
        assert_eq!(offer.availability, OfferAvailability::Unknown);

        let priced = serde_json::json!({
            seed.app_id: {
                "success": true,
                "data": {
                    "is_free": false,
                    "price_overview": { "currency": "EUR", "final": 4_199, "discount_percent": 30 }
                }
            }
        });
        let offer = steam_offer_from_payload(seed, &priced, "FR", NOW_MS).unwrap();
        assert_eq!(offer.price_minor, Some(4_199));
        assert_eq!(offer.currency.as_deref(), Some("EUR"));
        assert_eq!(offer.discount_percent, 30);
        assert_eq!(offer.availability, OfferAvailability::Available);
    }

    // -- providers -----------------------------------------------------------

    #[test]
    fn one_failing_provider_never_fails_the_response() {
        // Steam is configured and answers; Apple's request fails.
        let http = FakeHttp::default().with(
            "store.steampowered.com/api/appdetails",
            serde_json::json!({
                "1245620": {
                    "success": true,
                    "data": { "price_overview": { "currency": "USD", "final": 3_999, "discount_percent": 0 } }
                }
            }),
        );
        let config = RefreshConfig {
            steam_api_key: Some("0".repeat(32)),
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
        // Only the first seed has a canned answer, so Steam degrades partially
        // but still contributes its verified offer.
        assert_eq!(
            status(StoreProviderId::Steam).health,
            ProviderHealth::Degraded
        );
        assert!(!document.games.is_empty());
        let elden_ring = document
            .games
            .iter()
            .find(|game| game.id == "steam:1245620")
            .unwrap();
        assert_eq!(elden_ring.offers[0].price_minor, Some(3_999));
        assert!(!elden_ring.offers[0].to_dto(NOW_MS).stale);

        // A seed with no answer keeps its priceless editorial offer.
        let witcher = document
            .games
            .iter()
            .find(|game| game.id == "steam:292030")
            .unwrap();
        assert_eq!(witcher.offers[0].price_minor, None);
    }

    #[test]
    fn providers_without_a_feed_never_reach_the_network_or_invent_a_price() {
        let http = FakeHttp::default();
        let document = block_on(refresh_all(&http, &RefreshConfig::default(), NOW_MS));

        for provider in [
            StoreProviderId::Ubisoft,
            StoreProviderId::Microsoft,
            StoreProviderId::GooglePlay,
            StoreProviderId::InstantGaming,
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
            assert!(
                !document
                    .games
                    .iter()
                    .flat_map(|game| game.offers.iter())
                    .any(|offer| offer.provider == provider),
                "{provider:?} contributed an offer without a feed"
            );
        }

        // Steam is not configured here, so it must not have been requested.
        let requested = http.requested.lock().unwrap().clone();
        assert!(
            requested.iter().all(|url| url.contains("itunes.apple.com")),
            "an unconfigured provider performed a request: {requested:?}"
        );
        assert_eq!(
            document
                .provider_statuses
                .iter()
                .find(|status| status.provider == StoreProviderId::Steam)
                .unwrap()
                .health,
            ProviderHealth::NotConfigured
        );
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
    fn editorial_offers_stay_openable_without_a_cache_file() {
        let directory = TestDirectory::new("no-cache");
        let opener = RecordingOpener::default();
        let offer_id = &editorial_games()[0].offers[0].id;
        open_offer_with(&directory.cache(), offer_id, &opener).unwrap();
        assert_eq!(
            opener.opened.lock().unwrap()[0],
            "https://store.steampowered.com/app/1245620/"
        );
    }

    #[test]
    fn the_allowlist_covers_the_six_supported_stores() {
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
        let home = store_home(&directory.cache(), &directory.root, NOW_MS);
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

        let home = store_home(&directory.cache(), &directory.root, NOW_MS);
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
            &StoreBrowseRequest {
                category: StoreCategory::ShortSessions,
                providers: vec![StoreProviderId::Steam],
                query: String::new(),
                cursor: None,
                limit: 2,
            },
            NOW_MS,
        );
        assert_eq!(page.games.len(), 2);
        assert_eq!(page.next_cursor.as_deref(), Some("store_2"));
        for game in &page.games {
            assert!(game.tags.iter().any(|tag| tag == "Short Sessions"));
        }
        assert_eq!(page.provider_statuses.len(), StoreProviderId::ALL.len());

        let next = browse(
            &cache,
            &directory.root,
            &StoreBrowseRequest {
                category: StoreCategory::ShortSessions,
                providers: Vec::new(),
                query: String::new(),
                cursor: page.next_cursor.clone(),
                limit: 2,
            },
            NOW_MS,
        );
        assert!(next.next_cursor.is_none());
        assert_eq!(next.games.len(), 1);

        // An unknown provider filter yields an empty page, not an error.
        let empty = browse(
            &cache,
            &directory.root,
            &StoreBrowseRequest {
                category: StoreCategory::AllGames,
                providers: vec![StoreProviderId::InstantGaming],
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
            &StoreBrowseRequest {
                category: StoreCategory::AllGames,
                providers: Vec::new(),
                query: "  HÁDES ".to_string(),
                cursor: None,
                limit: 10,
            },
            NOW_MS,
        );
        assert_eq!(searched.games.len(), 1);
        assert_eq!(searched.games[0].title, "Hades II");
    }

    #[test]
    fn browse_limits_are_clamped_and_bad_cursors_are_ignored() {
        let directory = TestDirectory::new("clamp");
        let request = StoreBrowseRequest {
            category: StoreCategory::AllGames,
            providers: Vec::new(),
            query: String::new(),
            cursor: Some("store_not-a-number".to_string()),
            limit: 10_000,
        };
        let page = browse(&directory.cache(), &directory.root, &request, NOW_MS);
        assert_eq!(page.games.len(), EDITORIAL_SEEDS.len());
        assert_eq!(parse_cursor(Some("nonsense")), 0);
        assert_eq!(parse_cursor(Some("store_4")), 4);
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
