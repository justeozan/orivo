//! Wallpaper search across IGDB and Google Images, plus the download that turns
//! a chosen candidate into durable wallpaper media.
//!
//! Owned by the Media agent. Commands are registered by the shell in `lib.rs`.
//!
//! Security model: the WebView never names a URL. `search_wallpapers` returns
//! opaque candidate ids minted into an in-memory registry, and thumbnails for
//! display; `import_wallpaper_candidate` resolves the id and asks the `game_media`
//! pipeline to download the image, which re-validates the URL (https, public
//! domain, no credentials/ports/traversal), the magic bytes, the declared MIME
//! and the size and quota budgets before anything is registered. The registry
//! is bounded and short-lived, so a stale id simply fails with "search again".
//!
//! Every search is scoped to one `WallpaperCategory`. The picker shows four
//! separate rows — portrait cover, wide key art, atmospheric background, and the
//! game's wordmark — and fetches each independently, because a row is only
//! useful if everything in it has the shape the row promises. A 16:9 screenshot
//! is never allowed to land in the portrait row, so each source narrows per row
//! rather than returning one mixed pile: Steam builds each row from different
//! asset paths, IGDB and SteamGridDB ask a different endpoint, and the sources
//! that report pixel dimensions are bucketed by measured ratio.
//!
//! Three guards decide whether a row shows real artwork or a pile of gameplay
//! stills, and they are what this module is actually about:
//!
//! 1. **Ask a typed endpoint, not a search box.** IGDB separates `/covers`,
//!    `/artworks` and `/screenshots`; SteamGridDB separates grids, heroes and
//!    logos and tags every upload with a style. Where a typed endpoint exists,
//!    no keyword is needed and no gameplay still can leak in.
//! 2. **Name the shape in the query, and quote the game.** For the keyword-only
//!    sources the noun ("cover", "wallpaper", "logo") is what separates box art
//!    from gameplay, and quoting the title is what stops a fuzzy engine from
//!    answering about the sequel. Both come from a per-row template a user can
//!    edit in Settings, the way Playnite exposes one search term per media
//!    field.
//! 3. **Re-read the caption.** Ratio and pixel count cannot tell a 1920x1080
//!    screenshot from a 1920x1080 piece of key art. `keyword_score` can, so the
//!    measured rows sort on what the result calls itself before they sort on
//!    size.
//!
//! Search is built around Steam Store, a keyless source that returns real game
//! artwork from Steam's public endpoints — the same free source Playnite leans
//! on. Wikimedia Commons and Openverse are keyless fallbacks whose artwork
//! quality varies. SteamGridDB, IGDB and Google Images are optional
//! higher-quality sources: they need credentials, which a user can store in
//! Settings or set as environment variables. A value saved in Settings wins
//! over an environment variable. When an optional provider is not configured
//! the command still answers, with a `not-configured` phase and a copy
//! explaining what to set.
//!
//! SteamGridDB is the only source that can answer "high resolution, and without
//! the title burned into the picture" directly: its `styles` filter has a
//! literal `no_logo` value, and the wordmark is published separately so the app
//! can composite the real one over a clean scene.

use crate::game_detail::{GameMediaView, validate_opaque_id};
use crate::game_media::GameMediaService;
use crate::wallpaper_credentials::{WallpaperCredentialsDto, WallpaperCredentialsService};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};
use tauri::State;

pub const IGDB_CLIENT_ID_ENV: &str = "ORIVO_IGDB_CLIENT_ID";
pub const IGDB_CLIENT_SECRET_ENV: &str = "ORIVO_IGDB_CLIENT_SECRET";
pub const GOOGLE_API_KEY_ENV: &str = "ORIVO_GOOGLE_SEARCH_API_KEY";
pub const GOOGLE_CSE_ID_ENV: &str = "ORIVO_GOOGLE_SEARCH_CSE_ID";
pub const STEAMGRIDDB_API_KEY_ENV: &str = "ORIVO_STEAMGRIDDB_API_KEY";

const IGDB_TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const IGDB_API_URL: &str = "https://api.igdb.com/v4";
const GOOGLE_SEARCH_URL: &str = "https://www.googleapis.com/customsearch/v1";
const STEAM_STORE_SEARCH_URL: &str = "https://store.steampowered.com/api/storesearch/";
/// Steam's per-app asset CDN. Every Steam row is built from predictable paths
/// under this one host, which is also the only Steam asset host the WebView's
/// CSP allows — a second host would paint nothing.
const STEAM_ITEM_ASSETS_URL: &str =
    "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps";
const WIKIMEDIA_API_URL: &str = "https://commons.wikimedia.org/w/api.php";
const OPENVERSE_API_URL: &str = "https://api.openverse.org/v1/images/";
const STEAMGRIDDB_API_URL: &str = "https://www.steamgriddb.com/api/v2";

/// Wikimedia's user-agent policy answers an anonymous or generic client with a
/// 403, so the client names the app and a contact address.
const HTTP_USER_AGENT: &str = concat!("Orivo/", env!("CARGO_PKG_VERSION"), " (contact@oneiby.com)");

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CANDIDATES: usize = 16;
const REGISTRY_CAPACITY: usize = 512;
/// A freshly minted IGDB token is reused until it is about to expire.
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(300);
/// How many store hits may supply portrait covers. Store search is relevance
/// ordered, so the extra hits are editions and sequels of the same title.
const STEAM_MAX_APPS: usize = 4;
const IGDB_MEDIA_LIMIT: usize = 24;
/// How many autocomplete hits SteamGridDB is asked to rank before one is picked.
const STEAMGRIDDB_MAX_GAMES: usize = 8;
/// How many IGDB game hits are considered before the best-matching title wins.
const IGDB_MAX_GAMES: usize = 8;
/// An anonymous Openverse client is refused with a 401 above 20 per page.
const OPENVERSE_PAGE_SIZE: u32 = 20;

// ---------------------------------------------------------------------------
// View shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WallpaperSource {
    /// Built-in, keyless: Steam's public store search plus its per-app asset
    /// CDN, returning real cover, key art, page background and screenshots.
    SteamStore,
    /// Built-in, keyless: Wikimedia Commons file search.
    Wikimedia,
    /// Built-in, keyless: the Openverse image index.
    Openverse,
    /// Optional; needs a Twitch client id and secret.
    Igdb,
    /// Optional; needs a Google Custom Search JSON API key and engine id.
    GoogleImages,
    /// Optional; needs a SteamGridDB API key. The only source with a real
    /// "no logo" filter, and the only one publishing 4K-class key art.
    SteamGridDb,
}

/// The shape one row is asking for. This is the whole reason a search is not a
/// single mixed list: a row is only trustworthy if every image in it fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WallpaperCategory {
    /// Portrait box art, around 2:3.
    Cover,
    /// Wide key art, around 16:9.
    Landscape,
    /// Atmospheric backgrounds.
    Background,
    /// The game's wordmark on transparency. Not artwork: the app composites it
    /// over the hero, which is the whole reason the other three rows are
    /// allowed to prefer art with no title burned into it.
    Logo,
}

impl WallpaperCategory {
    fn slug(self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Landscape => "landscape",
            Self::Background => "background",
            Self::Logo => "logo",
        }
    }

    /// The ratio a row is aiming at. Ranking sorts by distance from this, so a
    /// 2:3 cover beats a 3:4 one and a true 16:9 beats a 1.75:1 capsule.
    ///
    /// Wordmarks have no canonical ratio at all — they are as wide as the words
    /// are — so the logo row aims at the middle of its band and effectively
    /// sorts on caption and size instead.
    fn target_ratio(self) -> f64 {
        match self {
            Self::Cover => 0.667,
            Self::Landscape | Self::Background => 1.778,
            Self::Logo => 2.5,
        }
    }

    /// The narrowest image this row will take, applied to every source alike —
    /// the typed endpoints included, which is the one rule they do not get to
    /// bypass.
    ///
    /// The background row is the reason this exists. It is a full-bleed image
    /// behind the whole page, so it is the one slot where a 1920-wide picture
    /// visibly falls apart, and it now asks for 4K and nothing else. That has a
    /// real cost, stated plainly: Steam's screenshots (1920x1080), its page
    /// backgrounds (1438 wide) and IGDB's `t_1080p` renders can never qualify,
    /// so on a game whose publisher uploaded no 4K art the row comes back
    /// empty rather than filled with something smaller.
    ///
    /// The card rows keep a low floor on purpose: they are drawn small, and
    /// `header.jpg` at 460 wide is a genuinely useful last resort for an old
    /// game with no library art at all.
    fn min_width(self) -> u32 {
        match self {
            Self::Cover => 600,
            Self::Landscape => 460,
            Self::Background => 3840,
            Self::Logo => LOGO_MIN_WIDTH,
        }
    }

    /// An empty row has to say which row is empty, because the other three may
    /// well have filled at the same time.
    fn empty_message(self) -> &'static str {
        match self {
            Self::Cover => "No cover art matched that search.",
            Self::Landscape => "No wide key art matched that search.",
            Self::Background => "No 4K background matched that search.",
            Self::Logo => "No logo matched that search.",
        }
    }

    /// The default search term for this row, in Playnite's shape: the game's
    /// name as a quoted phrase, plus the noun that names the shape being asked
    /// for.
    ///
    /// Both halves earn their place. The quotes are the anti-mismatch guard —
    /// every keyword engine here treats a quoted run as a required phrase, so
    /// "Doom" stops answering with Doom Eternal art. The noun is the
    /// anti-gameplay guard: "cover" and "wallpaper" pull genuinely different
    /// pictures out of the same index, which is why each row asks its own
    /// question rather than filtering one shared pile.
    pub fn default_term(self) -> &'static str {
        match self {
            Self::Cover => "\"{name}\" box art cover",
            Self::Landscape => "\"{name}\" key art",
            Self::Background => "\"{name}\" wallpaper",
            Self::Logo => "\"{name}\" logo transparent",
        }
    }

    /// Words that name what this row is *not* after. Google takes them as a
    /// real `excludeTerms` parameter; every other keyword source gets them as a
    /// scoring penalty in `keyword_score`.
    fn unwanted_words(self) -> &'static [&'static str] {
        match self {
            Self::Cover => &["screenshot", "gameplay", "wallpaper"],
            Self::Landscape => &["screenshot", "gameplay"],
            Self::Background => &["screenshot", "gameplay"],
            Self::Logo => &["screenshot", "gameplay", "wallpaper"],
        }
    }

    /// Words that name what this row *is* after, used to lift a well-captioned
    /// result above an identically shaped one.
    fn wanted_words(self) -> &'static [&'static str] {
        match self {
            Self::Cover => &["box art", "boxart", "cover", "packshot"],
            Self::Landscape => &["key art", "keyart", "artwork", "banner", "hero"],
            Self::Background => &["wallpaper", "key art", "keyart", "artwork", "background"],
            Self::Logo => &["logo", "wordmark", "transparent"],
        }
    }
}

/// Expands a per-row search term template against the game the user typed.
///
/// `{name}` is the only variable, spelled in any case so a template copied out
/// of Playnite (`{Name}`) works unchanged. A template that never mentions the
/// name would search for the same thing whatever game is open, so the name is
/// appended rather than silently dropped.
fn expand_term(template: &str, name: &str) -> String {
    let name = name.trim();
    let mut expanded = String::with_capacity(template.len() + name.len());
    let mut rest = template;
    let mut substituted = false;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|offset| open + offset) else {
            break;
        };
        expanded.push_str(&rest[..open]);
        if rest[open + 1..close].trim().eq_ignore_ascii_case("name") {
            expanded.push_str(name);
            substituted = true;
        } else {
            // An unknown variable is left verbatim: it is more likely a typo the
            // user can see and fix than something to quietly delete.
            expanded.push_str(&rest[open..=close]);
        }
        rest = &rest[close + 1..];
    }
    expanded.push_str(rest);
    let expanded = expanded.split_whitespace().collect::<Vec<_>>().join(" ");
    if substituted || name.is_empty() {
        expanded
    } else if expanded.is_empty() {
        name.to_owned()
    } else {
        format!("\"{name}\" {expanded}")
    }
}

/// A row either answered or could not be asked. There is no longer a
/// `not-configured` phase: a missing key removes one source from a row of six
/// rather than blanking it, so "you have not set this up" is at most a sentence
/// appended to an empty row's message, never a state the row is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WallpaperSearchPhase {
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperCandidateView {
    id: String,
    title: String,
    thumbnail_url: String,
    category: WallpaperCategory,
}

/// One row's answer. There is no source on it: a row is filled from every
/// source at once, so naming one would be a lie about where its tiles came from.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperSearchView {
    phase: WallpaperSearchPhase,
    category: WallpaperCategory,
    query: String,
    message: String,
    candidates: Vec<WallpaperCandidateView>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum WallpaperSearchError {
    NotConfigured(String),
    Invalid(String),
    Network(String),
}

impl std::fmt::Display for WallpaperSearchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured(message) => write!(formatter, "{message}"),
            Self::Invalid(message) => formatter.write_str(message),
            Self::Network(message) => formatter.write_str(message),
        }
    }
}

impl From<serde_json::Error> for WallpaperSearchError {
    fn from(error: serde_json::Error) -> Self {
        Self::Invalid(format!(
            "the search service returned malformed data ({error})"
        ))
    }
}

/// Where a user fills these in. Named once so every "not configured" message
/// points at the same place.
const CREDENTIALS_PANEL: &str = "Settings → Plugins → Wallpaper Searcher";

/// Resolves one credential: a value saved in Settings wins, then an
/// environment variable, then a `not-configured` answer. `label` is the field
/// name as it reads in Settings, because the env var is an escape hatch for
/// development and means nothing to someone looking at the form. The accessors
/// are injectable so every path is testable without touching process state.
fn credential_for(
    stored: &str,
    label: &str,
    key: &str,
    get_env: &impl Fn(&str) -> Option<String>,
) -> Result<String, WallpaperSearchError> {
    let value = if !stored.trim().is_empty() {
        stored.trim().to_owned()
    } else {
        get_env(key).unwrap_or_default()
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(WallpaperSearchError::NotConfigured(format!(
            "{label} is missing — add it under {CREDENTIALS_PANEL} (or set {key}). This source needs every one of its keys."
        )))
    } else {
        Ok(trimmed.to_owned())
    }
}

fn system_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

// ---------------------------------------------------------------------------
// Candidate registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct WallpaperCandidate {
    id: String,
    title: String,
    thumbnail_url: String,
    url: String,
}

/// Bounded, insertion-ordered cache so a search result can be imported a moment
/// after the search. Oldest entries are evicted once the cap is reached.
#[derive(Debug, Default)]
struct CandidateRegistry {
    entries: BTreeMap<String, WallpaperCandidate>,
    order: VecDeque<String>,
}

impl CandidateRegistry {
    fn insert(&mut self, candidate: WallpaperCandidate) {
        if self.entries.contains_key(&candidate.id) {
            return;
        }
        self.order.push_back(candidate.id.clone());
        self.entries.insert(candidate.id.clone(), candidate);
        while self.order.len() > REGISTRY_CAPACITY
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
        }
    }

    fn get(&self, id: &str) -> Option<&WallpaperCandidate> {
        self.entries.get(id)
    }
}

/// The category is part of the hash because the rows are fetched separately and
/// legitimately overlap — a 1920x1080 screenshot belongs in both the landscape
/// and the background row — and two tiles on screen must never share an id.
fn mint_candidate_id(
    source: WallpaperSource,
    category: WallpaperCategory,
    url: &str,
    sequence: u64,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orivo-wallpaper-candidate-v1\0");
    digest.update(match source {
        WallpaperSource::Igdb => "igdb".as_bytes(),
        WallpaperSource::GoogleImages => "google-images".as_bytes(),
        WallpaperSource::SteamStore => "steam-store".as_bytes(),
        WallpaperSource::Wikimedia => "wikimedia".as_bytes(),
        WallpaperSource::Openverse => "openverse".as_bytes(),
        WallpaperSource::SteamGridDb => "steam-grid-db".as_bytes(),
    });
    digest.update(b"\0");
    digest.update(category.slug().as_bytes());
    digest.update(b"\0");
    digest.update(url.as_bytes());
    format!("wp:{:x}:{sequence}", digest.finalize())
}

static CANDIDATE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Turns (title, thumbnail, full) triples into registry candidates: rejects
/// anything that is not an absolute web URL, drops repeats, and mints one
/// opaque id per row.
fn to_candidates(
    category: WallpaperCategory,
    entries: impl IntoIterator<Item = (WallpaperSource, (String, String, String))>,
) -> Vec<WallpaperCandidate> {
    let mut seen = std::collections::HashSet::new();
    let mut built = Vec::new();
    for (source, (title, raw_thumbnail, raw_url)) in entries {
        let Some(url) = https_url(&raw_url) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        let thumbnail_url = https_url(&raw_thumbnail).unwrap_or_else(|| url.clone());
        built.push(WallpaperCandidate {
            id: mint_candidate_id(
                source,
                category,
                &url,
                CANDIDATE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            ),
            title,
            thumbnail_url,
            url,
        });
    }
    built
}

// ---------------------------------------------------------------------------
// Ratio bucketing
// ---------------------------------------------------------------------------

/// Below this an image is a thumbnail or an icon, whatever its shape.
const MIN_CANDIDATE_WIDTH: u32 = 300;
const MIN_CANDIDATE_HEIGHT: u32 = 200;
const MIN_CANDIDATE_PIXELS: u64 = 150_000;
const COVER_MIN_RATIO: f64 = 0.56;
const COVER_MAX_RATIO: f64 = 0.80;
const COVER_MIN_HEIGHT: u32 = 420;
const LANDSCAPE_MIN_RATIO: f64 = 1.70;
const LANDSCAPE_MAX_RATIO: f64 = 1.90;
const LANDSCAPE_MIN_WIDTH: u32 = 600;
const BACKGROUND_MIN_RATIO: f64 = 1.60;
const BACKGROUND_MAX_RATIO: f64 = 6.00;
const BACKGROUND_MIN_WIDTH: u32 = 1280;
/// A wordmark is as wide as the words are, so the logo row bounds only what a
/// wordmark can never be: thumbnail-sized, or taller than it is wide. The lower
/// bound sits just below square rather than at it, because a stacked two-line
/// wordmark is a real shape — but it stays well clear of 2:3, so a portrait
/// cover can never present itself as a logo.
const LOGO_MIN_WIDTH: u32 = 200;
const LOGO_MIN_HEIGHT: u32 = 80;
const LOGO_MIN_RATIO: f64 = 0.90;
const LOGO_MAX_RATIO: f64 = 8.00;

/// Which rows an image of this size may appear in. Wikimedia, Openverse and
/// Google all report real pixel dimensions, so the row is decided by measuring
/// rather than by trusting a keyword in the query.
///
/// Landscape and background overlap on purpose: a 1920x1080 screenshot honestly
/// is both wide key art and a background. Cover never overlaps with either —
/// nothing portrait is allowed to leak into a wide row, or the reverse.
fn categories_for_size(width: u32, height: u32) -> Vec<WallpaperCategory> {
    if width < MIN_CANDIDATE_WIDTH
        || height < MIN_CANDIDATE_HEIGHT
        || u64::from(width) * u64::from(height) < MIN_CANDIDATE_PIXELS
    {
        return Vec::new();
    }
    let ratio = f64::from(width) / f64::from(height);
    let mut categories = Vec::new();
    if (COVER_MIN_RATIO..=COVER_MAX_RATIO).contains(&ratio) {
        if height >= COVER_MIN_HEIGHT {
            categories.push(WallpaperCategory::Cover);
        }
        return categories;
    }
    if (LANDSCAPE_MIN_RATIO..=LANDSCAPE_MAX_RATIO).contains(&ratio) && width >= LANDSCAPE_MIN_WIDTH
    {
        categories.push(WallpaperCategory::Landscape);
    }
    if (BACKGROUND_MIN_RATIO..=BACKGROUND_MAX_RATIO).contains(&ratio)
        && width >= BACKGROUND_MIN_WIDTH
    {
        categories.push(WallpaperCategory::Background);
    }
    categories
}

/// A search hit that carries its size and where it came from, so it can be
/// bucketed and ranked against hits from every other source before it ever
/// reaches a row.
#[derive(Debug, Clone, PartialEq)]
struct SizedCandidate {
    source: WallpaperSource,
    title: String,
    thumbnail_url: String,
    url: String,
    width: u32,
    height: u32,
    /// Whether the shape came from an endpoint that only holds that shape.
    ///
    /// SteamGridDB's `/heroes`, IGDB's `/covers` and Steam's `library_hero`
    /// path cannot return anything but what they are, so their proportions are
    /// a fact rather than a measurement — and several of them (a 3.1:1 hero) sit
    /// outside the ratio band a *measured* result has to fall inside, precisely
    /// because that band exists to catch a keyword engine returning a fan
    /// mock-up. Trusting the endpoint is what lets the good asset through
    /// without loosening the guard on the sources that need it.
    trusted_shape: bool,
}

impl SizedCandidate {
    fn ratio(&self) -> f64 {
        f64::from(self.width) / f64::from(self.height)
    }

    fn pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    fn into_triple(self) -> (String, String, String) {
        (self.title, self.thumbnail_url, self.url)
    }
}

/// A wordmark cannot be bucketed by ratio the way the artwork rows can, because
/// it has no canonical shape and its transparency is invisible to a size check.
/// All this rules out is the two things a logo is never: a portrait crop, and
/// something too small to read.
fn logo_fits(width: u32, height: u32) -> bool {
    if width < LOGO_MIN_WIDTH || height < LOGO_MIN_HEIGHT {
        return false;
    }
    let ratio = f64::from(width) / f64::from(height);
    (LOGO_MIN_RATIO..=LOGO_MAX_RATIO).contains(&ratio)
}

/// Whether an image of this size may appear in this row.
fn fits_category(width: u32, height: u32, category: WallpaperCategory) -> bool {
    if category == WallpaperCategory::Logo {
        return logo_fits(width, height);
    }
    categories_for_size(width, height).contains(&category)
}

/// How well a result's own caption reads as the thing this row is asking for.
///
/// This is the guard the geometry cannot provide: a 1920x1080 gameplay still
/// and a 1920x1080 piece of key art are the same rectangle, and no ratio or
/// pixel count separates them. What does separate them is what the page calls
/// the picture, and every keyword source here returns that text. Positive for
/// the row's own nouns, sharply negative for the ones it is avoiding — the
/// penalty outweighs a single match, so "Elden Ring gameplay screenshot" sinks
/// below a plainly titled piece of art even when it is larger.
fn keyword_score(title: &str, category: WallpaperCategory) -> i32 {
    let lowered = title.to_lowercase();
    let wanted: i32 = category
        .wanted_words()
        .iter()
        .filter(|word| lowered.contains(**word))
        .count() as i32;
    let unwanted: i32 = category
        .unwanted_words()
        .iter()
        .filter(|word| lowered.contains(**word))
        .count() as i32;
    wanted * 2 - unwanted * 3
}

/// Whether this hit may appear in this row at all.
///
/// Two rules, and only one of them is negotiable. The width floor binds every
/// source alike — it is what "backgrounds in 4K only" means, and a typed
/// endpoint does not get to opt out of it. The shape band binds only measured
/// results, because a typed endpoint's proportions are already known to be
/// right and several of them sit outside a band drawn to catch fan mock-ups.
fn admits(entry: &SizedCandidate, category: WallpaperCategory) -> bool {
    entry.width >= category.min_width()
        && (entry.trusted_shape || fits_category(entry.width, entry.height, category))
}

/// Merges every source's hits into one row, best first, with repeats dropped.
///
/// The order is: a typed endpoint's art before a keyword engine's guess, then
/// what the result calls itself, then how close it is to the row's ideal ratio,
/// then size. Caption before ratio is deliberate — it is the only signal that
/// tells key art from a screenshot of the same dimensions. Ratio before size
/// keeps a true 16:9 4K wallpaper ahead of a letterboxed 3840x1240 hero in the
/// background row, which is the right answer for an image drawn full-bleed.
///
/// Deduplication is by URL, so the same asset reached through two sources — a
/// Steam CDN path that Google also indexed — occupies one tile and not two.
fn rank_for_category(
    entries: Vec<SizedCandidate>,
    category: WallpaperCategory,
) -> Vec<SizedCandidate> {
    let target = category.target_ratio();
    let mut kept = entries
        .into_iter()
        .filter(|entry| admits(entry, category))
        .collect::<Vec<_>>();
    kept.sort_by(|left, right| {
        right
            .trusted_shape
            .cmp(&left.trusted_shape)
            .then_with(|| {
                keyword_score(&right.title, category).cmp(&keyword_score(&left.title, category))
            })
            .then_with(|| {
                (left.ratio() - target)
                    .abs()
                    .total_cmp(&(right.ratio() - target).abs())
            })
            .then_with(|| right.pixels().cmp(&left.pixels()))
    });
    let mut seen = std::collections::HashSet::new();
    kept.retain(|entry| seen.insert(entry.url.clone()));
    kept
}

// ---------------------------------------------------------------------------
// IGDB adapter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CachedIgdbToken {
    token: String,
    expires_at: SystemTime,
}

impl CachedIgdbToken {
    fn is_fresh(&self) -> bool {
        self.expires_at
            .duration_since(SystemTime::now())
            .is_ok_and(|remaining| remaining > TOKEN_REFRESH_MARGIN)
    }
}

#[derive(Deserialize)]
struct IgdbTokenResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}

#[derive(Deserialize)]
struct IgdbTokenError {
    #[serde(default)]
    message: String,
}

/// Twitch answers a rejected token request with
/// `{"status":400,"message":"invalid client"}` — "invalid client" for an
/// unknown client id, "invalid client secret" for the other half. Repeating
/// that message is what tells a user which field to fix; it quotes Twitch, not
/// the credential, so nothing secret is echoed back.
fn igdb_token_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<IgdbTokenError>(body)
        .ok()
        .map(|error| error.message.trim().to_owned())
        .filter(|message| !message.is_empty());
    match detail {
        Some(message) => format!(
            "IGDB rejected the credentials (status {status}): {message}. Check the Client ID and Client Secret under {CREDENTIALS_PANEL} — both come from the same Twitch application."
        ),
        None => format!("IGDB rejected the credentials (status {status})."),
    }
}

#[derive(Debug, Clone, Deserialize)]
struct IgdbGame {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    name: String,
}

/// `width` and `height` describe the *original* upload, which is exactly what
/// `t_original` serves — so asking for both together is what lets an IGDB hit
/// be measured against the row's width floor honestly.
#[derive(Debug, Clone, Deserialize)]
struct IgdbImage {
    #[serde(default)]
    url: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

/// IGDB is one of two sources whose media is already sorted by shape, so a row
/// asks the endpoint that holds its shape instead of filtering a mixed pile:
/// `/covers` is portrait box art, `/artworks` is the wide key art an editor
/// picked, `/screenshots` is captured gameplay.
///
/// The order matters for the background row. `/artworks` first and
/// `/screenshots` only behind it is the difference between a row of publisher
/// key art with a few stills at the end, and the row of gameplay captures that
/// asking `/screenshots` alone produces. IGDB publishes no wordmarks at all, so
/// the logo row has no endpoint here and says so rather than guessing.
fn igdb_endpoints(category: WallpaperCategory) -> &'static [&'static str] {
    match category {
        WallpaperCategory::Cover => &["covers"],
        WallpaperCategory::Landscape => &["artworks"],
        WallpaperCategory::Background => &["artworks", "screenshots"],
        WallpaperCategory::Logo => &[],
    }
}

/// The (thumbnail, full) size tokens for a row.
///
/// Every row takes `original` for the image it will actually download. The
/// renders below it are all capped — `t_1080p` is 1920 wide whatever was
/// uploaded — so a row asking for 4K could never be satisfied by one, and the
/// `width`/`height` IGDB reports describe the original anyway. Thumbnails stay
/// on the small renders, which is the whole reason they exist.
fn igdb_size_tokens(category: WallpaperCategory) -> (&'static str, &'static str) {
    match category {
        WallpaperCategory::Cover => ("cover_big", "original"),
        WallpaperCategory::Landscape | WallpaperCategory::Background | WallpaperCategory::Logo => {
            ("screenshot_big", "original")
        }
    }
}

/// What one tile is called, taken from the endpoint rather than the row, so a
/// background row that mixes both can label each tile honestly.
fn igdb_media_noun(endpoint: &str) -> &'static str {
    match endpoint {
        "covers" => "cover",
        "screenshots" => "screenshot",
        _ => "artwork",
    }
}

/// IGDB image URLs embed a Cloudinary-style size token. Swap it for the size we
/// want; an unrecognised shape is kept untouched so nothing is silently lost.
fn swap_igdb_size(url: &str, size: &str) -> String {
    let Some(start) = url.find("/t_") else {
        return url.to_owned();
    };
    let rest = &url[start + 2..];
    let Some(offset) = rest.find('/') else {
        return url.to_owned();
    };
    format!("{}/t_{size}{}", &url[..start], &rest[offset..])
}

/// Lowercased alphanumerics only, so "Elden Ring" and "EldenRing.exe" compare
/// equal and punctuation or casing never break a title match.
fn normalized_title_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A simple fuzzy title check: exact normalized equality, or containment when
/// the shorter key is substantial enough (three characters) to not match by
/// accident. Candidate titles carry suffixes ("Elden Ring — header"), so
/// containment is the shape a correct hit actually takes.
/// Orders Steam store hits for an explicit, user-typed search: hits whose own
/// title matches what was typed come first, the store's own ranking follows.
///
/// Deliberately a sort and not a filter. Store search is fuzzy but the user
/// typed this query into a search box, so Steam's ranking beats an empty row —
/// "GTA V" only ever returns "Grand Theft Auto V Enhanced", which no substring
/// test accepts, and filtering there blanks all three rows of the default
/// source. `top_artwork_url` keeps the strict guard instead, because that path
/// assigns art with nobody watching, and a wrong answer is worse than none.
fn rank_steam_apps(query: &str, apps: Vec<(u64, String)>) -> Vec<(u64, String)> {
    let (titled, rest): (Vec<_>, Vec<_>) = apps
        .into_iter()
        .partition(|(_, name)| loose_title_match(query, name));
    titled
        .into_iter()
        .chain(rest)
        .take(STEAM_MAX_APPS)
        .collect()
}

fn loose_title_match(query: &str, candidate: &str) -> bool {
    let query_key = normalized_title_key(query);
    let candidate_key = normalized_title_key(candidate);
    if query_key.is_empty() || candidate_key.is_empty() {
        return false;
    }
    if query_key == candidate_key {
        return true;
    }
    let (short, long) = if query_key.len() <= candidate_key.len() {
        (query_key.as_str(), candidate_key.as_str())
    } else {
        (candidate_key.as_str(), query_key.as_str())
    };
    short.len() >= 3 && long.contains(short)
}

/// Picks the entry a title search was actually about.
///
/// Both catalogue APIs here rank fuzzily, and taking the first hit is how a
/// search for "Doom" ends up showing Doom Eternal's art. An exact normalised
/// title always wins; a loose containment match is next; the API's own first
/// choice is the last resort, because these are entries a user typed a query
/// for and an empty row helps nobody.
fn pick_best_named<T>(query: &str, entries: Vec<T>, name_of: impl Fn(&T) -> &str) -> Option<T> {
    let wanted = normalized_title_key(query);
    let mut exact = None;
    let mut loose = None;
    let mut first = None;
    for (index, entry) in entries.into_iter().enumerate() {
        let name = name_of(&entry).to_owned();
        if exact.is_none() && !wanted.is_empty() && normalized_title_key(&name) == wanted {
            exact = Some(entry);
            break;
        }
        if loose.is_none() && loose_title_match(query, &name) {
            loose = Some(entry);
        } else if index == 0 {
            first = Some(entry);
        }
    }
    exact.or(loose).or(first)
}

/// Normalises a search result URL to https, or rejects it.
///
/// Rejecting matters: the shared client is `https_only`, so a value that is not
/// a URL at all fails at request time with nothing to show for it. Anything
/// that is not an absolute web URL — a bare filename, a `data:` payload, a
/// scheme-only string — is dropped here instead.
fn https_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let lowered = trimmed.to_ascii_lowercase();
    let authority = if let Some(rest) = trimmed.strip_prefix("//") {
        rest
    } else if lowered.starts_with("https://") {
        &trimmed["https://".len()..]
    } else if lowered.starts_with("http://") {
        &trimmed["http://".len()..]
    } else {
        return None;
    };
    // A scheme with no host ("https://", "///a") is not a URL either.
    if authority.is_empty() || authority.starts_with('/') {
        return None;
    }
    Some(format!("https://{authority}"))
}

/// Escape a user query for a `search "…"` clause: backslash and quote first,
/// then drop anything a statement parser would choke on.
fn apicalypse_search(query: &str) -> String {
    query
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn parse_igdb_games(body: &str) -> Result<Vec<IgdbGame>, WallpaperSearchError> {
    let games: Vec<IgdbGame> = serde_json::from_str(body)?;
    Ok(games.into_iter().filter(|game| game.id > 0).collect())
}

fn parse_igdb_images(body: &str) -> Result<Vec<IgdbImage>, WallpaperSearchError> {
    let images: Vec<IgdbImage> = serde_json::from_str(body)?;
    Ok(images
        .into_iter()
        .filter(|image| !image.url.trim().is_empty())
        .collect())
}

/// Builds (title, thumbnail, full) triples from one IGDB media endpoint, at the
/// sizes that row wants.
fn build_igdb_candidates(
    images: &[IgdbImage],
    game_name: &str,
    category: WallpaperCategory,
    endpoint: &str,
) -> Vec<SizedCandidate> {
    let (thumbnail_size, full_size) = igdb_size_tokens(category);
    let noun = igdb_media_noun(endpoint);
    let mut built = Vec::new();
    for (index, image) in images.iter().enumerate() {
        let Some(base) = https_url(&image.url) else {
            continue;
        };
        built.push(SizedCandidate {
            source: WallpaperSource::Igdb,
            title: format!("{game_name} — {noun} {}", index + 1),
            thumbnail_url: swap_igdb_size(&base, thumbnail_size),
            url: swap_igdb_size(&base, full_size),
            width: image.width,
            height: image.height,
            // `/covers` is portrait and `/artworks` is wide by construction;
            // only `/screenshots` is a shape a caption could lie about, and it
            // is a capture of the game either way.
            trusted_shape: true,
        });
    }
    built
}

// ---------------------------------------------------------------------------
// Google adapter
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GoogleSearchResponse {
    #[serde(default)]
    items: Option<Vec<GoogleSearchItem>>,
    #[serde(default)]
    error: Option<GoogleApiError>,
}

#[derive(Deserialize)]
struct GoogleApiError {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct GoogleSearchItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    image: Option<GoogleSearchImage>,
}

#[derive(Deserialize)]
struct GoogleSearchImage {
    #[serde(default, rename = "thumbnailLink")]
    thumbnail_link: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

/// Custom Search has no aspect or orientation parameter at all, so the shape is
/// still decided here from the width and height each item reports. What it does
/// have is three filters worth more than any wording: `exactTerms` makes the
/// game's name a required phrase, `excludeTerms` drops the words that name the
/// wrong kind of picture, and `imgColorType=trans` returns only images with an
/// alpha channel — the same lever Playnite pulls (`tbs=ic:trans`) to fetch a
/// wordmark rather than a poster of one.
fn google_exclude_terms(category: WallpaperCategory) -> String {
    category.unwanted_words().join(" ")
}

/// The alpha-channel filter, set for the wordmark row alone. A logo composited
/// over the hero has to have a transparent background or it paints a box.
fn google_colour_type(category: WallpaperCategory) -> Option<&'static str> {
    (category == WallpaperCategory::Logo).then_some("trans")
}

fn google_image_size(category: WallpaperCategory) -> &'static str {
    match category {
        WallpaperCategory::Cover => "large",
        WallpaperCategory::Landscape => "xlarge",
        WallpaperCategory::Background => "huge",
        // A wordmark is a small, wide file; asking for "huge" returns almost
        // nothing at all.
        WallpaperCategory::Logo => "medium",
    }
}

fn parse_google_search(body: &str) -> Result<Vec<SizedCandidate>, WallpaperSearchError> {
    let response: GoogleSearchResponse = serde_json::from_str(body)?;
    if let Some(error) = response.error
        && !error.message.trim().is_empty()
    {
        return Err(WallpaperSearchError::Network(format!(
            "Google Images reported: {}",
            error.message
        )));
    }
    let mut built = Vec::new();
    for item in response.items.unwrap_or_default() {
        let Some(full) = https_url(&item.link) else {
            continue;
        };
        // An item without an `image` block reports no size, and an unmeasurable
        // image cannot be placed in a row honestly.
        let Some(image) = item.image else {
            continue;
        };
        let thumbnail = https_url(&image.thumbnail_link).unwrap_or_else(|| full.clone());
        built.push(SizedCandidate {
            source: WallpaperSource::GoogleImages,
            title: item.title,
            thumbnail_url: thumbnail,
            url: full,
            width: image.width,
            height: image.height,
            // A web index returns whatever the page held, so the shape is a
            // measurement and the caption is a claim. Both are checked.
            trusted_shape: false,
        });
    }
    Ok(built)
}

// ---------------------------------------------------------------------------
// Wikimedia Commons adapter
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct WikimediaSearchResponse {
    #[serde(default)]
    query: Option<WikimediaQuery>,
}

#[derive(Deserialize, Default)]
struct WikimediaQuery {
    #[serde(default)]
    pages: BTreeMap<String, WikimediaPage>,
}

#[derive(Deserialize)]
struct WikimediaPage {
    #[serde(default)]
    title: String,
    #[serde(default)]
    imageinfo: Vec<WikimediaImageInfo>,
}

#[derive(Deserialize)]
struct WikimediaImageInfo {
    #[serde(default)]
    url: String,
    #[serde(default)]
    thumburl: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

/// CirrusSearch understands `filew:`/`fileh:` with strict `>` and `<` only —
/// `filew:>=1920` is rejected outright — and has no aspect field: `fileaspect:`
/// is not an error, it silently returns nothing, so it is never emitted. The
/// search therefore narrows on the minimum edge lengths a row needs and leaves
/// the ratio to `categories_for_size`. `filetype:bitmap` keeps SVG and PDF out.
/// `term` arrives already expanded from the row's template, so the noun and the
/// quoted title come from the same place every other keyword source gets them.
fn wikimedia_query(term: &str, category: WallpaperCategory) -> String {
    let (min_width, min_height) = match category {
        WallpaperCategory::Cover => (MIN_CANDIDATE_WIDTH - 1, COVER_MIN_HEIGHT - 1),
        WallpaperCategory::Landscape => (LANDSCAPE_MIN_WIDTH - 1, MIN_CANDIDATE_HEIGHT - 1),
        WallpaperCategory::Background => (BACKGROUND_MIN_WIDTH - 1, MIN_CANDIDATE_HEIGHT - 1),
        WallpaperCategory::Logo => (LOGO_MIN_WIDTH - 1, LOGO_MIN_HEIGHT - 1),
    };
    format!("{term} filetype:bitmap filew:>{min_width} fileh:>{min_height}")
}

fn parse_wikimedia_search(body: &str) -> Result<Vec<SizedCandidate>, WallpaperSearchError> {
    let response: WikimediaSearchResponse = serde_json::from_str(body)?;
    let mut built = Vec::new();
    for page in response.query.unwrap_or_default().pages.into_values() {
        let Some(info) = page.imageinfo.into_iter().next() else {
            continue;
        };
        let Some(full) = https_url(&info.url) else {
            continue;
        };
        // Titles arrive as "File:Sky_art.png"; the caption reads better as
        // "Sky art" without the extension.
        let base = page
            .title
            .strip_prefix("File:")
            .unwrap_or(&page.title)
            .replace('_', " ");
        let title = base
            .rfind('.')
            .filter(|dot| *dot > 0 && !base[*dot + 1..].contains('.'))
            .map(|dot| base[..dot].trim())
            .filter(|trimmed| !trimmed.is_empty())
            .unwrap_or(base.trim())
            .to_owned();
        let thumbnail = https_url(&info.thumburl).unwrap_or_else(|| full.clone());
        built.push(SizedCandidate {
            source: WallpaperSource::Wikimedia,
            title,
            thumbnail_url: thumbnail,
            url: full,
            width: info.width,
            height: info.height,
            trusted_shape: false,
        });
    }
    Ok(built)
}

// ---------------------------------------------------------------------------
// Openverse adapter
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct OpenverseSearchResponse {
    #[serde(default)]
    results: Vec<OpenverseImage>,
}

#[derive(Deserialize)]
struct OpenverseImage {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

/// Openverse's `aspect_ratio` takes only `tall`, `square` or `wide` and is a
/// plain height-versus-width test rather than a ratio band, so it narrows the
/// fetch and nothing more — every result is re-measured here.
fn openverse_aspect_ratio(category: WallpaperCategory) -> &'static str {
    match category {
        WallpaperCategory::Cover => "tall",
        WallpaperCategory::Landscape | WallpaperCategory::Background | WallpaperCategory::Logo => {
            "wide"
        }
    }
}

fn parse_openverse_search(body: &str) -> Result<Vec<SizedCandidate>, WallpaperSearchError> {
    let response: OpenverseSearchResponse = serde_json::from_str(body)?;
    let mut built = Vec::new();
    for item in response.results {
        let Some(full) = https_url(&item.url) else {
            continue;
        };
        let thumbnail = item
            .thumbnail
            .as_deref()
            .and_then(https_url)
            .unwrap_or_else(|| full.clone());
        let title = if item.title.trim().is_empty() {
            "Openverse image".into()
        } else {
            item.title.trim().to_owned()
        };
        built.push(SizedCandidate {
            source: WallpaperSource::Openverse,
            title,
            thumbnail_url: thumbnail,
            url: full,
            width: item.width,
            height: item.height,
            trusted_shape: false,
        });
    }
    Ok(built)
}

// ---------------------------------------------------------------------------
// Steam Store adapter
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SteamStoreSearchResponse {
    #[serde(default)]
    items: Vec<SteamStoreItem>,
}

#[derive(Deserialize)]
struct SteamStoreItem {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    id: u64,
    #[serde(default)]
    name: String,
}

/// One asset on Steam's per-app CDN.
///
/// `optional` marks the paths that genuinely 404 on some apps — the library art
/// on anything predating Steam's 2019 redesign, `capsule_616x353`, `page_bg_raw`
/// and both wordmarks — which are probed before they reach a row so a miss
/// paints as nothing rather than as a broken tile. The card rows each keep a
/// sibling that is always present (`header`, `page_bg_generated_v6b`), so they
/// are never left empty by a probe.
///
/// Every path here has a size Steam publishes it at, which is why Steam needs no
/// API call to take part in a row with a width floor.
#[derive(Debug, Clone, PartialEq)]
struct SteamAsset {
    title: String,
    thumbnail_url: String,
    url: String,
    width: u32,
    height: u32,
    optional: bool,
}

impl SteamAsset {
    fn into_candidate(self) -> SizedCandidate {
        SizedCandidate {
            source: WallpaperSource::SteamStore,
            title: self.title,
            thumbnail_url: self.thumbnail_url,
            url: self.url,
            width: self.width,
            height: self.height,
            trusted_shape: true,
        }
    }
}

fn steam_asset_url(app_id: u64, file: &str) -> String {
    format!("{STEAM_ITEM_ASSETS_URL}/{app_id}/{file}")
}

/// The art one app can supply to one row, entirely from predictable CDN paths
/// and therefore at no API cost.
///
/// Portrait is the reason this exists: appdetails carries no portrait field, so
/// `library_600x900_2x` is the only 2:3 asset Steam has. The wide rows are
/// built here too because the curated library art beats anything the API would
/// name.
///
/// `library_hero` leads both wide rows, and that ordering is the point.
/// It is the full-bleed scene Steam's own client paints behind a game, published
/// at 1920x620 with a 3840x1240 `_2x` sibling, and — unlike the capsule and the
/// header — it carries no title, because Steam composites `logo.png` over it the
/// same way this app does. The capsule and header stay as fallbacks for apps
/// old enough to predate the library art, and `page_bg_generated_v6b` stays last
/// because it is only a darkened, stretched header.
fn steam_category_assets(category: WallpaperCategory, app_id: u64, game: &str) -> Vec<SteamAsset> {
    let hero = |suffix: &str, label: &str, width: u32, height: u32| SteamAsset {
        title: format!("{game} — {label}"),
        thumbnail_url: steam_asset_url(app_id, "library_hero.jpg"),
        url: steam_asset_url(app_id, &format!("library_hero{suffix}.jpg")),
        width,
        height,
        // Apps that predate Steam's library redesign have neither. Portal
        // (app 400) is the live example: `library_hero.jpg` is there, the `_2x`
        // is not.
        optional: true,
    };
    match category {
        WallpaperCategory::Cover => vec![SteamAsset {
            title: format!("{game} — cover"),
            // The filename lies: `library_600x900.jpg` is the 300x450 copy and
            // the `_2x` sibling is the real 600x900 — served at 1200x1800.
            thumbnail_url: steam_asset_url(app_id, "library_600x900.jpg"),
            url: steam_asset_url(app_id, "library_600x900_2x.jpg"),
            width: 1200,
            height: 1800,
            optional: false,
        }],
        WallpaperCategory::Landscape => vec![
            hero("_2x", "key art (4K)", 3840, 1240),
            hero("", "key art", 1920, 620),
            SteamAsset {
                title: format!("{game} — capsule"),
                thumbnail_url: steam_asset_url(app_id, "capsule_616x353.jpg"),
                url: steam_asset_url(app_id, "capsule_616x353.jpg"),
                width: 616,
                height: 353,
                optional: true,
            },
            SteamAsset {
                title: format!("{game} — header"),
                thumbnail_url: steam_asset_url(app_id, "header.jpg"),
                url: steam_asset_url(app_id, "header.jpg"),
                width: 460,
                height: 215,
                optional: false,
            },
        ],
        // The 4K library art is the only thing Steam publishes that clears the
        // background row's width floor. `page_bg_raw` is around 1438 wide and
        // `page_bg_generated_v6b` is a stretched header, so neither could ever
        // qualify and neither is requested — which also means this row costs one
        // probe rather than four.
        WallpaperCategory::Background => vec![hero("_2x", "background (4K)", 3840, 1240)],
        // Both wordmark paths genuinely 404 on plenty of apps, so this row is
        // allowed to come back empty rather than offering a broken tile. A
        // wordmark's size varies per game, so it is reported at the row's floor:
        // the probe, not a guess, is what decides it is real.
        WallpaperCategory::Logo => vec![
            SteamAsset {
                title: format!("{game} — logo"),
                thumbnail_url: steam_asset_url(app_id, "logo.png"),
                url: steam_asset_url(app_id, "logo.png"),
                width: LOGO_MIN_WIDTH,
                height: LOGO_MIN_HEIGHT,
                optional: true,
            },
            SteamAsset {
                title: format!("{game} — library logo"),
                thumbnail_url: steam_asset_url(app_id, "library_logo.png"),
                url: steam_asset_url(app_id, "library_logo.png"),
                width: LOGO_MIN_WIDTH,
                height: LOGO_MIN_HEIGHT,
                optional: true,
            },
        ],
    }
}

/// A missing asset on Steam's CDN is a clean 404 with an HTML body, never a
/// placeholder image, so the status and content type together are enough to
/// tell a real asset from a miss.
fn steam_asset_response_is_image(status: u16, content_type: &str) -> bool {
    (200..300).contains(&status)
        && content_type
            .trim()
            .to_ascii_lowercase()
            .starts_with("image/")
}

/// Store search returns apps plus packages ("sub") and bundles; only the app
/// hits are usable for artwork.
fn parse_steam_store_search(body: &str) -> Result<Vec<(u64, String)>, WallpaperSearchError> {
    let response: SteamStoreSearchResponse = serde_json::from_str(body)?;
    Ok(response
        .items
        .into_iter()
        .filter(|item| item.kind == "app" && item.id > 0 && !item.name.trim().is_empty())
        .map(|item| (item.id, item.name.trim().to_owned()))
        .collect())
}

/// Which store hits may supply art for a row.
///
/// The portrait row spreads across every loosely matching hit, because Steam
/// has exactly one cover per app and a one-tile row is not a row; the extra
/// hits are the editions and sequels of the same title that store search ranks
/// next, and they cost nothing since covers come from a path. The wide rows
/// stay on the single best match: they already carry its library art, capsule
/// and header, and mixing games into them would just dilute the answer. The
/// logo row stays single for a different reason — another game's wordmark is the
/// most obviously wrong thing a card can wear.
fn steam_apps_for_category(
    matched: &[(u64, String)],
    category: WallpaperCategory,
) -> &[(u64, String)] {
    match category {
        WallpaperCategory::Cover => &matched[..matched.len().min(STEAM_MAX_APPS)],
        WallpaperCategory::Landscape | WallpaperCategory::Background | WallpaperCategory::Logo => {
            &matched[..matched.len().min(1)]
        }
    }
}

// ---------------------------------------------------------------------------
// SteamGridDB adapter
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GridDbEnvelope<T> {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    data: Option<T>,
    #[serde(default)]
    errors: Vec<String>,
}

#[derive(Deserialize)]
struct GridDbGame {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    name: String,
}

#[derive(Clone, Deserialize)]
struct GridDbAsset {
    #[serde(default)]
    url: String,
    #[serde(default)]
    thumb: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

/// One SteamGridDB fetch: an asset kind, the dimensions that row accepts, and
/// the styles it will take.
struct GridDbRequest {
    kind: &'static str,
    dimensions: &'static str,
    styles: &'static str,
}

/// What each row asks SteamGridDB for, in preference order.
///
/// This is the source that answers the actual question — high resolution, and
/// without the title burned into the picture — because `styles` is a real API
/// filter and `no_logo` is a real value in it.
///
/// The style vocabulary is **per asset kind**, and getting that wrong is not a
/// silent mismatch: grids accept `alternate | blurred | white_logo | material |
/// no_logo`, but heroes accept only `alternate | blurred | material`, and
/// sending `no_logo` to `/heroes` is answered with a flat 400. That is exactly
/// what made every background row fail. Heroes need no de-logoed variant
/// anyway: a hero is the scene Steam composites a separate wordmark over, so it
/// has no title in it by construction, and `alternate` alone already excludes
/// the `material` style that does.
///
/// Grids are the Steam-shaped capsules (portrait at 600x900, wide at 920x430);
/// heroes are the full-bleed scenes, published as high as 3840x1240. The wide
/// row asks for heroes first because they are both larger and cleaner, and only
/// then for wide grids. Logos have no dimension vocabulary — a wordmark is as
/// wide as the words are — and their own style set again (`official | white |
/// black | custom`).
fn steamgriddb_requests(category: WallpaperCategory) -> Vec<GridDbRequest> {
    let grid = |dimensions| GridDbRequest {
        kind: "grids",
        dimensions,
        styles: crate::game_artwork::GRID_STYLES,
    };
    let hero = |dimensions| GridDbRequest {
        kind: "heroes",
        dimensions,
        styles: crate::game_artwork::HERO_STYLES,
    };
    match category {
        WallpaperCategory::Cover => vec![grid("600x900,660x930")],
        WallpaperCategory::Landscape => vec![hero("3840x1240,1920x620"), grid("920x430,460x215")],
        // The background row takes 4K and nothing else, so there is one hero
        // dimension it can use and asking for 1920x620 as well would only fetch
        // tiles the row is about to discard.
        WallpaperCategory::Background => vec![hero("3840x1240")],
        WallpaperCategory::Logo => vec![GridDbRequest {
            kind: "logos",
            dimensions: "",
            styles: crate::game_artwork::LOGO_STYLES,
        }],
    }
}

fn steamgriddb_noun(category: WallpaperCategory) -> &'static str {
    match category {
        WallpaperCategory::Cover => "cover",
        WallpaperCategory::Landscape => "key art",
        WallpaperCategory::Background => "background",
        WallpaperCategory::Logo => "logo",
    }
}

/// SteamGridDB reports a failure in an envelope with `success: false` and a
/// list of reasons rather than an HTTP status, so a bad key reads as an empty
/// row unless the body is inspected.
fn parse_griddb<T: for<'de> Deserialize<'de> + Default>(
    body: &str,
) -> Result<Option<T>, WallpaperSearchError> {
    let envelope: GridDbEnvelope<T> = serde_json::from_str(body)?;
    if !envelope.success {
        let reason = envelope
            .errors
            .into_iter()
            .find(|error| !error.trim().is_empty())
            .unwrap_or_else(|| "the request was refused".to_owned());
        return Err(WallpaperSearchError::Network(format!(
            "SteamGridDB reported: {reason}"
        )));
    }
    Ok(envelope.data)
}

/// Percent-encodes a title for the autocomplete path segment. The search term
/// is the only user-controlled part of any SteamGridDB URL, and it lands in a
/// path rather than a query, so it is encoded here rather than by the client.
fn urlencode_path(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// `already` is how many tiles the row already holds, so a row fed by two
/// requests — heroes then wide grids — numbers its tiles continuously instead
/// of restarting at 1 halfway through and captioning two different pictures
/// identically.
fn build_griddb_candidates(
    assets: Vec<GridDbAsset>,
    game_name: &str,
    category: WallpaperCategory,
    already: usize,
) -> Vec<SizedCandidate> {
    let noun = steamgriddb_noun(category);
    assets
        .into_iter()
        .enumerate()
        .filter_map(|(index, asset)| {
            let url = https_url(&asset.url)?;
            let thumbnail = https_url(&asset.thumb).unwrap_or_else(|| url.clone());
            Some(SizedCandidate {
                source: WallpaperSource::SteamGridDb,
                title: format!("{game_name} — {noun} {}", already + index + 1),
                thumbnail_url: thumbnail,
                url,
                width: asset.width,
                height: asset.height,
                // A grid is a grid and a hero is a hero: the endpoint and the
                // `dimensions` filter together already pin the shape.
                trusted_shape: true,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

struct WallpaperSearchInner {
    http: reqwest::Client,
    registry: Mutex<CandidateRegistry>,
    igdb_token: Mutex<Option<CachedIgdbToken>>,
}

/// Tauri-managed state. The HTTP client is shared, the registry is global, and
/// the IGDB token is reused across searches until it nears expiry.
#[derive(Clone)]
pub struct WallpaperSearchService {
    inner: Arc<WallpaperSearchInner>,
    credentials: Arc<WallpaperCredentialsService>,
}

impl WallpaperSearchService {
    pub fn new(credentials: Arc<WallpaperCredentialsService>) -> Self {
        let http = reqwest::Client::builder()
            .https_only(true)
            .user_agent(HTTP_USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self::with_parts(http, credentials)
    }

    pub fn with_parts(
        http: reqwest::Client,
        credentials: Arc<WallpaperCredentialsService>,
    ) -> Self {
        Self {
            inner: Arc::new(WallpaperSearchInner {
                http,
                registry: Mutex::new(CandidateRegistry::default()),
                igdb_token: Mutex::new(None),
            }),
            credentials,
        }
    }

    fn stored(&self) -> WallpaperCredentialsDto {
        self.credentials.dto()
    }

    /// The search term one keyword source sends for one row: the user's own
    /// template from Settings when they saved one, the tuned default otherwise.
    ///
    /// Only the keyword sources call this. Steam, IGDB and SteamGridDB resolve
    /// a game and then ask a typed endpoint, so a noun in the query would be
    /// noise at best and would break the title match at worst.
    fn term_for(&self, category: WallpaperCategory, query: &str) -> String {
        let stored = self.stored();
        let template = match category {
            WallpaperCategory::Cover => stored.search_term_cover,
            WallpaperCategory::Landscape => stored.search_term_landscape,
            WallpaperCategory::Background => stored.search_term_background,
            WallpaperCategory::Logo => stored.search_term_logo,
        };
        let template = if template.trim().is_empty() {
            category.default_term()
        } else {
            template.trim()
        };
        expand_term(template, query)
    }

    /// Fills one row from every source at once.
    ///
    /// Picking a source was a question with no good answer: the person asking
    /// wants the best picture, not a provider, and finding it meant opening the
    /// dialog six times. So all six run concurrently, their hits are merged,
    /// filtered against the row's rules and ranked together — a SteamGridDB
    /// hero and a Steam library asset compete on the same row and the better one
    /// wins on its merits.
    ///
    /// A source that is unreachable, refused or simply unconfigured takes the
    /// row down with it under no circumstances: its failure is set aside and the
    /// rest of the row still paints. That is the whole reason a per-source error
    /// is no longer a per-row error. The set-aside reasons are only surfaced
    /// when the row ends up empty, because that is the one time they explain
    /// something the user can act on — usually "add a SteamGridDB key".
    pub async fn search(
        &self,
        category: WallpaperCategory,
        query: &str,
        offset: u32,
    ) -> WallpaperSearchView {
        let query = query.trim().to_owned();
        if query.is_empty() {
            return WallpaperSearchView {
                phase: WallpaperSearchPhase::Error,
                category,
                query,
                message: "Type a search query first.".into(),
                candidates: Vec::new(),
            };
        }
        let outcomes = futures_util::join!(
            self.search_steamgriddb(category, &query),
            self.search_steam_store(category, &query),
            self.search_igdb(category, &query),
            self.search_google(category, &query),
            self.search_wikimedia(category, &query),
            self.search_openverse(category, &query),
        );
        let outcomes = [
            outcomes.0, outcomes.1, outcomes.2, outcomes.3, outcomes.4, outcomes.5,
        ];

        let mut found = Vec::new();
        let mut setbacks = Vec::new();
        for outcome in outcomes {
            match outcome {
                Ok(entries) => found.extend(entries),
                Err(error) => setbacks.push(error.to_string()),
            }
        }

        let ranked = rank_for_category(found, category);
        let total = ranked.len();
        let candidates = to_candidates(
            category,
            ranked
                .into_iter()
                .skip(offset as usize)
                .take(MAX_CANDIDATES)
                .map(|entry| (entry.source, entry.into_triple())),
        );
        let views = candidates
            .into_iter()
            .map(|candidate| {
                let view = WallpaperCandidateView {
                    id: candidate.id.clone(),
                    title: candidate.title.clone(),
                    thumbnail_url: candidate.thumbnail_url.clone(),
                    category,
                };
                if let Ok(mut registry) = self.inner.registry.lock() {
                    registry.insert(candidate);
                }
                view
            })
            .collect::<Vec<_>>();

        let message = if !views.is_empty() {
            format!("{total} result(s)")
        } else if setbacks.is_empty() {
            category.empty_message().to_owned()
        } else {
            // An empty row is the only moment the set-aside reasons matter, and
            // they are what tells someone their SteamGridDB key never saved.
            format!("{} {}", category.empty_message(), setbacks.join(" "))
        };
        WallpaperSearchView {
            phase: WallpaperSearchPhase::Ready,
            category,
            query,
            message,
            candidates: views,
        }
    }

    /// Best-effort key art URL for a title, via the keyless Steam Store. Used to
    /// give a freshly imported local or Wine game artwork with no API keys
    /// required. It asks the landscape row because a library card and a detail
    /// hero are both wide. Returns `None` when nothing matches.
    pub async fn top_artwork_url(&self, query: &str) -> Option<String> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        let candidates = self
            .search_steam_store(WallpaperCategory::Landscape, query)
            .await
            .ok()?;
        // The store search is fuzzy: an unknown title ("Hozy Playtest") can
        // still return a best-effort hit for a completely different game.
        // Only a candidate whose own title loosely matches the query may win;
        // otherwise the game keeps its neutral placeholder instead of wearing
        // another game's art.
        candidates
            .into_iter()
            .find(|candidate| loose_title_match(query, &candidate.title))
            .map(|candidate| candidate.url)
    }

    pub async fn import_candidate(
        &self,
        game_id: &str,
        candidate_id: &str,
        media: &GameMediaService,
    ) -> Result<Vec<GameMediaView>, String> {
        validate_opaque_id("candidate id", candidate_id).map_err(|error| error.to_string())?;
        let candidate = self
            .inner
            .registry
            .lock()
            .map_err(|_| "the wallpaper search is temporarily unavailable".to_owned())?
            .get(candidate_id)
            .cloned()
            .ok_or_else(|| "This search result has expired. Run the search again.".to_owned())?;
        media
            .download_wallpaper(game_id, &candidate.url, &candidate.title)
            .await
            .map_err(|error| error.to_string())
    }

    async fn search_igdb(
        &self,
        category: WallpaperCategory,
        query: &str,
    ) -> Result<Vec<SizedCandidate>, WallpaperSearchError> {
        let stored = self.stored();
        let client_id = credential_for(
            &stored.igdb_client_id,
            "IGDB Client ID",
            IGDB_CLIENT_ID_ENV,
            &system_env,
        )?;
        let client_secret = credential_for(
            &stored.igdb_client_secret,
            "IGDB Client Secret",
            IGDB_CLIENT_SECRET_ENV,
            &system_env,
        )?;
        let endpoints = igdb_endpoints(category);
        if endpoints.is_empty() {
            return Err(WallpaperSearchError::Invalid(
                "IGDB publishes no wordmarks. Use SteamGridDB or Steam Store for the logo.".into(),
            ));
        }
        let token = self.igdb_token(&client_id, &client_secret).await?;
        let games = self.igdb_query_games(&client_id, &token, query).await?;
        // IGDB's `search` is fuzzy and its first hit is regularly a sequel, an
        // expansion or a soundtrack. Taking it unchecked is how a game ends up
        // wearing another game's art with nobody having chosen it.
        let Some(top) = pick_best_named(query, games, |game| game.name.as_str()) else {
            return Ok(Vec::new());
        };
        let game_name = if top.name.trim().is_empty() {
            query
        } else {
            top.name.trim()
        };
        let mut built = Vec::new();
        for endpoint in endpoints {
            let images = self
                .igdb_query_media(&client_id, &token, endpoint, top.id, IGDB_MEDIA_LIMIT)
                .await?;
            built.extend(build_igdb_candidates(
                &images, game_name, category, endpoint,
            ));
        }
        Ok(built)
    }

    /// SteamGridDB: the only source with a real "no logo" filter, and the only
    /// one publishing 4K-class key art. One autocomplete call resolves the game,
    /// then each row asks its own typed endpoint with its own style filter, so
    /// nothing needs to be measured or keyword-guessed afterwards.
    async fn search_steamgriddb(
        &self,
        category: WallpaperCategory,
        query: &str,
    ) -> Result<Vec<SizedCandidate>, WallpaperSearchError> {
        let stored = self.stored();
        let key = credential_for(
            &stored.steamgriddb_api_key,
            "SteamGridDB API Key",
            STEAMGRIDDB_API_KEY_ENV,
            &system_env,
        )?;
        let Some(game) = self.steamgriddb_game(&key, query).await? else {
            return Ok(Vec::new());
        };
        let game_name = if game.name.trim().is_empty() {
            query
        } else {
            game.name.trim()
        };
        let mut built = Vec::new();
        for request in steamgriddb_requests(category) {
            let assets = self.steamgriddb_assets(&key, game.id, &request).await?;
            let next = build_griddb_candidates(assets, game_name, category, built.len());
            built.extend(next);
        }
        Ok(built)
    }

    async fn steamgriddb_game(
        &self,
        key: &str,
        query: &str,
    ) -> Result<Option<GridDbGame>, WallpaperSearchError> {
        let url = format!(
            "{STEAMGRIDDB_API_URL}/search/autocomplete/{}",
            urlencode_path(query)
        );
        let body = self.steamgriddb_get(key, &url).await?;
        let games: Vec<GridDbGame> = parse_griddb(&body)?.unwrap_or_default();
        let games = games
            .into_iter()
            .filter(|game| game.id > 0)
            .take(STEAMGRIDDB_MAX_GAMES)
            .collect::<Vec<_>>();
        Ok(pick_best_named(query, games, |game| game.name.as_str()))
    }

    async fn steamgriddb_assets(
        &self,
        key: &str,
        game_id: u64,
        request: &GridDbRequest,
    ) -> Result<Vec<GridDbAsset>, WallpaperSearchError> {
        let mut url = format!(
            "{STEAMGRIDDB_API_URL}/{}/game/{game_id}?styles={}&nsfw=false&humor=false&types=static",
            request.kind, request.styles
        );
        if !request.dimensions.is_empty() {
            url.push_str("&dimensions=");
            url.push_str(request.dimensions);
        }
        let body = self.steamgriddb_get(key, &url).await?;
        Ok(parse_griddb::<Vec<GridDbAsset>>(&body)?.unwrap_or_default())
    }

    async fn steamgriddb_get(&self, key: &str, url: &str) -> Result<String, WallpaperSearchError> {
        let response = self
            .inner
            .http
            .get(url)
            .bearer_auth(key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|_| {
                WallpaperSearchError::Network("SteamGridDB could not be reached. Try again.".into())
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|_| {
            WallpaperSearchError::Network("SteamGridDB sent an unreadable response.".into())
        })?;
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(WallpaperSearchError::NotConfigured(format!(
                "SteamGridDB refused that API key — check it under {CREDENTIALS_PANEL}."
            )));
        }
        // A game with no entry at all is a 404, which is an empty row and not a
        // failure worth putting in front of anyone.
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(r#"{"success":true,"data":[]}"#.to_owned());
        }
        if !status.is_success() {
            return Err(WallpaperSearchError::Network(format!(
                "SteamGridDB returned status {status}."
            )));
        }
        Ok(body)
    }

    async fn igdb_token(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<String, WallpaperSearchError> {
        if let Ok(cached) = self.inner.igdb_token.lock()
            && let Some(token) = cached.as_ref()
            && token.is_fresh()
        {
            return Ok(token.token.clone());
        }
        let response = self
            .inner
            .http
            .post(IGDB_TOKEN_URL)
            .form(&[
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("grant_type", "client_credentials"),
            ])
            .send()
            .await
            .map_err(|_| {
                WallpaperSearchError::Network("IGDB could not be reached. Try again.".into())
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|_| {
            WallpaperSearchError::Network("IGDB sent an unreadable response.".into())
        })?;
        if !status.is_success() {
            return Err(WallpaperSearchError::Network(igdb_token_error(
                status.as_u16(),
                &body,
            )));
        }
        let parsed: IgdbTokenResponse = serde_json::from_str(&body)?;
        if parsed.access_token.is_empty() {
            return Err(WallpaperSearchError::Network(
                "IGDB returned no access token.".into(),
            ));
        }
        let expires_in = if parsed.expires_in > 0 {
            parsed.expires_in
        } else {
            60
        };
        if let Ok(mut cached) = self.inner.igdb_token.lock() {
            *cached = Some(CachedIgdbToken {
                token: parsed.access_token.clone(),
                expires_at: SystemTime::now() + Duration::from_secs(expires_in),
            });
        }
        Ok(parsed.access_token)
    }

    async fn igdb_query_games(
        &self,
        client_id: &str,
        token: &str,
        query: &str,
    ) -> Result<Vec<IgdbGame>, WallpaperSearchError> {
        let body = format!(
            "search \"{}\"; fields name; limit {IGDB_MAX_GAMES};",
            apicalypse_search(query)
        );
        let text = self.igdb_post(client_id, token, "games", &body).await?;
        parse_igdb_games(&text)
    }

    /// `width` and `height` come back alongside the URL because a row now has a
    /// width floor to enforce, and they describe the original upload — which is
    /// exactly the render `igdb_size_tokens` asks for.
    async fn igdb_query_media(
        &self,
        client_id: &str,
        token: &str,
        endpoint: &str,
        game_id: i64,
        limit: usize,
    ) -> Result<Vec<IgdbImage>, WallpaperSearchError> {
        let body = format!("fields url,width,height; where game = {game_id}; limit {limit};");
        let text = self.igdb_post(client_id, token, endpoint, &body).await?;
        parse_igdb_images(&text)
    }

    async fn igdb_post(
        &self,
        client_id: &str,
        token: &str,
        endpoint: &str,
        body: &str,
    ) -> Result<String, WallpaperSearchError> {
        let response = self
            .inner
            .http
            .post(format!("{IGDB_API_URL}/{endpoint}"))
            .header("Client-ID", client_id)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .map_err(|_| {
                WallpaperSearchError::Network("IGDB could not be reached. Try again.".into())
            })?;
        let status = response.status();
        let text = response.text().await.map_err(|_| {
            WallpaperSearchError::Network("IGDB sent an unreadable response.".into())
        })?;
        if !status.is_success() {
            return Err(WallpaperSearchError::Network(format!(
                "IGDB returned status {status}."
            )));
        }
        Ok(text)
    }

    async fn search_google(
        &self,
        category: WallpaperCategory,
        query: &str,
    ) -> Result<Vec<SizedCandidate>, WallpaperSearchError> {
        let stored = self.stored();
        let key = credential_for(
            &stored.google_api_key,
            "Google API Key",
            GOOGLE_API_KEY_ENV,
            &system_env,
        )?;
        let cse_id = credential_for(
            &stored.google_cse_id,
            "Google Search Engine ID",
            GOOGLE_CSE_ID_ENV,
            &system_env,
        )?;
        let mut parameters = vec![
            ("key", key.clone()),
            ("cx", cse_id.clone()),
            ("q", self.term_for(category, query)),
            // The game's name as a required phrase. `exactTerms` is a stronger
            // guard than quoting inside `q`, because it survives however the
            // user rewrote their template.
            ("exactTerms", query.to_owned()),
            ("excludeTerms", google_exclude_terms(category)),
            ("searchType", "image".to_owned()),
            ("num", "10".to_owned()),
            ("imgSize", google_image_size(category).to_owned()),
            ("safe", "active".to_owned()),
        ];
        if let Some(colour) = google_colour_type(category) {
            parameters.push(("imgColorType", colour.to_owned()));
        }
        let response = self
            .inner
            .http
            .get(GOOGLE_SEARCH_URL)
            .query(&parameters)
            .send()
            .await
            .map_err(|_| {
                WallpaperSearchError::Network(
                    "Google Images could not be reached. Try again.".into(),
                )
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|_| {
            WallpaperSearchError::Network("Google Images sent an unreadable response.".into())
        })?;
        if !status.is_success() {
            return Err(WallpaperSearchError::Network(format!(
                "Google Images returned status {status}."
            )));
        }
        parse_google_search(&body)
    }

    /// The built-in, keyless game-artwork source. One store search resolves the
    /// matching apps, and each row is then built from Steam's predictable asset
    /// paths — every one of which has a published size, so nothing here needs a
    /// second API call.
    ///
    /// `appdetails` used to be called for the screenshots. It is not any more:
    /// a Steam screenshot is 1920x1080, the background row takes 4K only, and
    /// the card rows never wanted gameplay captures in the first place. Dropping
    /// it removes the one rate-limited surface this source had.
    async fn search_steam_store(
        &self,
        category: WallpaperCategory,
        query: &str,
    ) -> Result<Vec<SizedCandidate>, WallpaperSearchError> {
        let response = self
            .inner
            .http
            .get(STEAM_STORE_SEARCH_URL)
            .query(&[("term", query), ("cc", "us"), ("l", "en")])
            .send()
            .await
            .map_err(|_| {
                WallpaperSearchError::Network("Steam Store could not be reached. Try again.".into())
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|_| {
            WallpaperSearchError::Network("Steam Store sent an unreadable response.".into())
        })?;
        if !status.is_success() {
            return Err(WallpaperSearchError::Network(format!(
                "Steam Store returned status {status}."
            )));
        }
        let matched = rank_steam_apps(query, parse_steam_store_search(&body)?);
        let assets = steam_apps_for_category(&matched, category)
            .iter()
            .flat_map(|(app_id, name)| steam_category_assets(category, *app_id, name))
            .collect::<Vec<_>>();
        let present = join_all(assets.iter().map(|asset| self.steam_asset_present(asset))).await;
        Ok(assets
            .into_iter()
            .zip(present)
            .filter(|(_, present)| *present)
            .map(|(asset, _)| asset.into_candidate())
            .collect())
    }

    /// Probes only the paths that are known to 404 on some apps, so a row never
    /// paints a broken tile. The guaranteed siblings are waved straight through.
    async fn steam_asset_present(&self, asset: &SteamAsset) -> bool {
        if !asset.optional {
            return true;
        }
        let Ok(response) = self.inner.http.head(&asset.url).send().await else {
            return false;
        };
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        steam_asset_response_is_image(response.status().as_u16(), &content_type)
    }

    /// The built-in, keyless source: a Wikimedia Commons file search, narrowed
    /// to the edge lengths the row needs and then measured by ratio.
    async fn search_wikimedia(
        &self,
        category: WallpaperCategory,
        query: &str,
    ) -> Result<Vec<SizedCandidate>, WallpaperSearchError> {
        let response = self
            .inner
            .http
            .get(WIKIMEDIA_API_URL)
            .query(&[
                ("action", "query".to_owned()),
                ("generator", "search".to_owned()),
                (
                    "gsrsearch",
                    wikimedia_query(&self.term_for(category, query), category),
                ),
                ("gsrnamespace", "6".to_owned()),
                ("gsrlimit", "32".to_owned()),
                ("prop", "imageinfo".to_owned()),
                // `size` is what makes the ratio bucketing possible at all.
                ("iiprop", "url|size".to_owned()),
                ("iiurlwidth", "640".to_owned()),
                ("format", "json".to_owned()),
            ])
            .send()
            .await
            .map_err(|_| {
                WallpaperSearchError::Network(
                    "Wikimedia Commons could not be reached. Try again.".into(),
                )
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|_| {
            WallpaperSearchError::Network("Wikimedia Commons sent an unreadable response.".into())
        })?;
        if !status.is_success() {
            return Err(WallpaperSearchError::Network(format!(
                "Wikimedia Commons returned status {status}."
            )));
        }
        parse_wikimedia_search(&body)
    }

    /// The second built-in, keyless source: Openverse's openly licensed image
    /// index. Large results lean toward wallpaper-sized artwork.
    async fn search_openverse(
        &self,
        category: WallpaperCategory,
        query: &str,
    ) -> Result<Vec<SizedCandidate>, WallpaperSearchError> {
        let response = self
            .inner
            .http
            .get(OPENVERSE_API_URL)
            .query(&[
                ("q", self.term_for(category, query)),
                // Reuse is the point of the index; `all` is not a documented
                // value for this filter.
                ("license_type", "commercial,modification".to_owned()),
                ("aspect_ratio", openverse_aspect_ratio(category).to_owned()),
                ("size", "large".to_owned()),
                ("page_size", OPENVERSE_PAGE_SIZE.to_string()),
            ])
            .send()
            .await
            .map_err(|_| {
                WallpaperSearchError::Network("Openverse could not be reached. Try again.".into())
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|_| {
            WallpaperSearchError::Network("Openverse sent an unreadable response.".into())
        })?;
        if !status.is_success() {
            return Err(WallpaperSearchError::Network(format!(
                "Openverse returned status {status}."
            )));
        }
        parse_openverse_search(&body)
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Fill one row from every source at once. No source is named, because none is
/// chosen: a missing key, an unreachable host or a refused request quietly
/// removes that one source from the row and the rest still answers.
#[tauri::command]
pub async fn search_wallpapers(
    category: WallpaperCategory,
    query: String,
    offset: u32,
    service: State<'_, WallpaperSearchService>,
) -> Result<WallpaperSearchView, String> {
    Ok(service.search(category, &query, offset).await)
}

/// Turn one opaque search result into durable wallpaper media for a game.
#[tauri::command]
pub async fn import_wallpaper_candidate(
    game_id: String,
    candidate_id: String,
    service: State<'_, WallpaperSearchService>,
    media: State<'_, GameMediaService>,
) -> Result<Vec<GameMediaView>, String> {
    service
        .import_candidate(&game_id, &candidate_id, &media)
        .await
        .map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row. The picker owns the order it paints them in; this list exists
    /// so a per-row invariant — "no row asks for gameplay", "every default term
    /// quotes the game" — is proved for all of them and cannot be quietly
    /// skipped when a row is added.
    const ALL_CATEGORIES: [WallpaperCategory; 4] = [
        WallpaperCategory::Cover,
        WallpaperCategory::Landscape,
        WallpaperCategory::Background,
        WallpaperCategory::Logo,
    ];

    /// A measured hit from a keyword source: shape and caption both have to
    /// earn their place in a row.
    fn sized(url: &str, width: u32, height: u32) -> SizedCandidate {
        SizedCandidate {
            source: WallpaperSource::GoogleImages,
            title: url.to_owned(),
            thumbnail_url: format!("{url}#thumb"),
            url: url.to_owned(),
            width,
            height,
            trusted_shape: false,
        }
    }

    /// A hit from an endpoint that only holds one shape. The URL is derived from
    /// the caption so two typed hits are distinct pictures unless a test means
    /// them to be the same one.
    fn typed(source: WallpaperSource, title: &str, width: u32, height: u32) -> SizedCandidate {
        let slug = normalized_title_key(title);
        SizedCandidate {
            source,
            title: title.to_owned(),
            thumbnail_url: format!("https://a.example/{slug}#thumb"),
            url: format!("https://a.example/{slug}.jpg"),
            width,
            height,
            trusted_shape: true,
        }
    }

    #[test]
    fn artwork_relevance_guard_matches_loosely_and_rejects_unrelated_titles() {
        assert!(loose_title_match("Elden Ring", "Elden Ring — header"));
        assert!(loose_title_match("EldenRing", "ELDEN RING — screenshot 1"));
        assert!(loose_title_match("Rez", "Rez Infinite — header"));
        assert!(!loose_title_match("Hozy Playtest", "Elden Ring — header"));
        assert!(!loose_title_match("", "Elden Ring — header"));
        assert!(!loose_title_match("Hozy Playtest", ""));
    }

    /// A user-typed search ranks by title match but never discards on it —
    /// "GTA V" is exactly the query that returns only a non-matching title, and
    /// dropping it would empty all three rows of the keyless default source.
    #[test]
    fn a_typed_search_ranks_steam_hits_without_ever_emptying_the_row() {
        let abbreviated = rank_steam_apps(
            "GTA V",
            vec![(3240220, "Grand Theft Auto V Enhanced".to_owned())],
        );
        assert_eq!(
            abbreviated,
            vec![(3240220, "Grand Theft Auto V Enhanced".to_owned())],
            "an abbreviation must still reach the store's own top hit"
        );

        // When some hits do match the typed title, they lead.
        let mixed = rank_steam_apps(
            "Elden Ring",
            vec![
                (1, "Hozy Playtest".to_owned()),
                (2, "ELDEN RING NIGHTREIGN".to_owned()),
                (3, "Something Else".to_owned()),
                (4, "Elden Ring".to_owned()),
            ],
        );
        assert_eq!(
            mixed.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![2, 4, 1, 3]
        );

        // The cap still holds, and nothing survives an empty store answer.
        assert_eq!(
            rank_steam_apps(
                "Anything",
                (0..10).map(|id| (id, format!("App {id}"))).collect()
            )
            .len(),
            STEAM_MAX_APPS
        );
        assert!(rank_steam_apps("Anything", Vec::new()).is_empty());
    }

    #[test]
    fn credentials_missing_is_not_configured() {
        let never = |_: &str| None;
        // The message names the Settings field and where to find it; the env
        // var trails behind as the development escape hatch.
        assert!(matches!(
            credential_for("", "IGDB Client ID", "ORIVO_IGDB_CLIENT_ID", &never),
            Err(WallpaperSearchError::NotConfigured(message))
                if message.contains("IGDB Client ID")
                    && message.contains(CREDENTIALS_PANEL)
                    && message.contains("ORIVO_IGDB_CLIENT_ID")
        ));
        let blank = |_: &str| Some("   ".to_owned());
        assert!(matches!(
            credential_for("", "Google API Key", "ORIVO_GOOGLE_SEARCH_API_KEY", &blank),
            Err(WallpaperSearchError::NotConfigured(_))
        ));
        let present = |_: &str| Some("abc123".to_owned());
        assert_eq!(
            credential_for(
                "",
                "IGDB Client Secret",
                "ORIVO_IGDB_CLIENT_SECRET",
                &present
            )
            .unwrap(),
            "abc123"
        );
    }

    /// A half-filled form is the shape a user actually lands in: one key pasted,
    /// the other still empty. The missing half has to name itself.
    #[test]
    fn one_saved_key_without_its_pair_still_reports_the_missing_half() {
        let never = |_: &str| None;
        assert_eq!(
            credential_for("a-client-id", "IGDB Client ID", IGDB_CLIENT_ID_ENV, &never).unwrap(),
            "a-client-id"
        );
        assert!(matches!(
            credential_for("", "IGDB Client Secret", IGDB_CLIENT_SECRET_ENV, &never),
            Err(WallpaperSearchError::NotConfigured(message))
                if message.contains("IGDB Client Secret")
                    && !message.contains("IGDB Client ID")
        ));
    }

    #[test]
    fn a_stored_credential_beats_an_environment_variable() {
        let env = |_: &str| Some("from-env".to_owned());
        assert_eq!(
            credential_for(
                "from-settings",
                "IGDB Client ID",
                "ORIVO_IGDB_CLIENT_ID",
                &env
            )
            .unwrap(),
            "from-settings"
        );
        assert_eq!(
            credential_for("  spaced  ", "IGDB Client ID", "ORIVO_IGDB_CLIENT_ID", &env).unwrap(),
            "spaced"
        );
        // An empty stored value falls through to the environment.
        assert_eq!(
            credential_for("   ", "IGDB Client ID", "ORIVO_IGDB_CLIENT_ID", &env).unwrap(),
            "from-env"
        );
    }

    #[test]
    fn surfaces_the_twitch_reason_a_token_was_refused() {
        let message = igdb_token_error(400, r#"{"status":400,"message":"invalid client"}"#);
        assert!(message.contains("invalid client"));
        assert!(message.contains("status 400"));
        assert!(message.contains(CREDENTIALS_PANEL));
        // A body that carries no reason degrades to the bare status.
        assert_eq!(
            igdb_token_error(503, "<html>gateway</html>"),
            "IGDB rejected the credentials (status 503)."
        );
        assert_eq!(
            igdb_token_error(400, r#"{"message":"   "}"#),
            "IGDB rejected the credentials (status 400)."
        );
    }

    #[test]
    fn swaps_igdb_size_tokens() {
        assert_eq!(
            swap_igdb_size(
                "https://images.igdb.com/igdb/image/upload/t_thumb/abc.jpg",
                "1080p"
            ),
            "https://images.igdb.com/igdb/image/upload/t_1080p/abc.jpg"
        );
        // A shape without a size token is returned untouched.
        assert_eq!(
            swap_igdb_size("https://images.igdb.com/upload/abc.jpg", "1080p"),
            "https://images.igdb.com/upload/abc.jpg"
        );
    }

    /// The client is `https_only`, so anything that is not an absolute web URL
    /// has to be dropped here rather than fail silently at request time.
    #[test]
    fn normalises_urls_to_https_and_rejects_everything_else() {
        assert_eq!(
            https_url("//images.igdb.com/igdb/image/upload/t_thumb/a.jpg").as_deref(),
            Some("https://images.igdb.com/igdb/image/upload/t_thumb/a.jpg")
        );
        assert_eq!(
            https_url("https://lh3.googleusercontent.com/a").as_deref(),
            Some("https://lh3.googleusercontent.com/a")
        );
        // An http URL is upgraded, not prefixed: the old code produced
        // "https://http://…", which is not a URL at all.
        assert_eq!(
            https_url("http://upload.wikimedia.org/a.jpg").as_deref(),
            Some("https://upload.wikimedia.org/a.jpg")
        );
        assert_eq!(
            https_url("  HTTP://Upload.Wikimedia.org/a.jpg  ").as_deref(),
            Some("https://Upload.Wikimedia.org/a.jpg")
        );
        // A bare filename used to become "https://thumb.jpg".
        assert_eq!(https_url("thumb.jpg"), None);
        assert_eq!(https_url(""), None);
        assert_eq!(https_url("   "), None);
        assert_eq!(https_url("https://"), None);
        assert_eq!(https_url("//"), None);
        assert_eq!(https_url("///evil"), None);
        assert_eq!(https_url("data:image/png;base64,AAAA"), None);
        assert_eq!(https_url("ftp://files.example/a.jpg"), None);
    }

    #[test]
    fn escapes_a_search_clause() {
        assert_eq!(apicalypse_search("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(apicalypse_search("two; statements"), "two; statements");
        assert_eq!(apicalypse_search("line\nbreak"), "linebreak");
    }

    #[test]
    fn parses_igdb_game_results() {
        let games =
            parse_igdb_games(r#"[{"id":118600,"name":"Elden Ring"},{"id":0,"name":"junk"}]"#)
                .unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].id, 118600);
        assert_eq!(games[0].name, "Elden Ring");
    }

    /// IGDB is asked a different endpoint per row, so nothing has to be
    /// filtered afterwards — `/covers` cannot return a screenshot.
    ///
    /// The background row is the one that matters here: it leads with the
    /// editor-picked key art and keeps gameplay captures strictly behind it.
    /// Asking `/screenshots` alone, as it used to, is what filled the row with
    /// gameplay.
    #[test]
    fn igdb_asks_one_endpoint_and_one_size_family_per_row() {
        assert_eq!(igdb_endpoints(WallpaperCategory::Cover), ["covers"]);
        assert_eq!(igdb_endpoints(WallpaperCategory::Landscape), ["artworks"]);
        assert_eq!(
            igdb_endpoints(WallpaperCategory::Background),
            ["artworks", "screenshots"]
        );
        // IGDB has no wordmark endpoint at all, and guessing one would mean
        // offering a screenshot as a logo.
        assert!(igdb_endpoints(WallpaperCategory::Logo).is_empty());
        // Every row downloads the original upload. IGDB's other renders are all
        // capped — `t_1080p` is 1920 wide however large the source was — so a
        // row with a 4K floor could never be satisfied by one, and the size IGDB
        // reports describes the original anyway.
        for category in ALL_CATEGORIES {
            assert_eq!(igdb_size_tokens(category).1, "original", "{category:?}");
        }
        assert_eq!(igdb_size_tokens(WallpaperCategory::Cover).0, "cover_big");
        assert_eq!(
            igdb_size_tokens(WallpaperCategory::Landscape).0,
            "screenshot_big"
        );
    }

    #[test]
    fn parses_igdb_artwork_and_builds_candidates_at_the_row_size() {
        let images = parse_igdb_images(
            r#"[{"id":1,"url":"//images.igdb.com/igdb/image/upload/t_thumb/a.jpg","width":3840,"height":2160},{"id":2}]"#,
        )
        .unwrap();
        let built = build_igdb_candidates(
            &images,
            "Elden Ring",
            WallpaperCategory::Landscape,
            "artworks",
        );
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].title, "Elden Ring — artwork 1");
        assert!(built[0].thumbnail_url.ends_with("/t_screenshot_big/a.jpg"));
        // The full image is always the original upload. Every other IGDB render
        // is capped — `t_1080p` is 1920 wide whatever was uploaded — so a row
        // asking for 4K could never be satisfied by one.
        assert!(built[0].url.ends_with("/t_original/a.jpg"));
        // And the reported size is the original's, so it describes the file the
        // row will actually measure and download.
        assert_eq!((built[0].width, built[0].height), (3840, 2160));
        let built =
            build_igdb_candidates(&images, "Elden Ring", WallpaperCategory::Cover, "covers");
        assert_eq!(built[0].title, "Elden Ring — cover 1");
        assert!(built[0].thumbnail_url.ends_with("/t_cover_big/a.jpg"));
        assert!(built[0].url.ends_with("/t_original/a.jpg"));
        // A background row mixes both endpoints, so each tile is named after
        // the endpoint it came from rather than the row.
        let built = build_igdb_candidates(
            &images,
            "Elden Ring",
            WallpaperCategory::Background,
            "screenshots",
        );
        assert_eq!(built[0].title, "Elden Ring — screenshot 1");
        let built = build_igdb_candidates(
            &images,
            "Elden Ring",
            WallpaperCategory::Background,
            "artworks",
        );
        assert_eq!(built[0].title, "Elden Ring — artwork 1");
    }

    #[test]
    fn parses_google_search_results_with_their_measured_size() {
        let built = parse_google_search(
            r#"{"items":[{"title":"Elden Ring Wallpaper","link":"https://wall.example/full.jpg","image":{"thumbnailLink":"https://encrypted-tbn0.gstatic.com/thumb.jpg","width":1920,"height":1080}},{"title":"No image","link":"https://wall.example/other.jpg"},{"title":"No thumb","link":"https://wall.example/third.jpg","image":{"width":600,"height":900}}]}"#,
        )
        .unwrap();
        // The item with no `image` block reports no size, so it cannot be
        // placed in a row and is dropped.
        assert_eq!(built.len(), 2);
        assert_eq!(built[0].title, "Elden Ring Wallpaper");
        assert_eq!(
            built[0].thumbnail_url,
            "https://encrypted-tbn0.gstatic.com/thumb.jpg"
        );
        assert_eq!(built[0].url, "https://wall.example/full.jpg");
        assert_eq!((built[0].width, built[0].height), (1920, 1080));
        assert_eq!(built[1].thumbnail_url, built[1].url);
    }

    /// Custom Search exposes no orientation knob, so the shape is still decided
    /// by measuring. What it does expose is `excludeTerms` and the alpha-channel
    /// filter, which are the two levers that actually change what comes back.
    #[test]
    fn google_narrows_with_the_filters_it_actually_has() {
        // No row asks for gameplay, and the wide row used to ask for it by
        // name — the single biggest reason key art came back as screenshots.
        for category in ALL_CATEGORIES {
            let excluded = google_exclude_terms(category);
            assert!(excluded.contains("screenshot"), "{category:?}: {excluded}");
            assert!(excluded.contains("gameplay"), "{category:?}: {excluded}");
        }
        // A cover is not a wallpaper and a wallpaper is not a cover.
        assert!(google_exclude_terms(WallpaperCategory::Cover).contains("wallpaper"));
        assert!(!google_exclude_terms(WallpaperCategory::Background).contains("wallpaper"));
        // Transparency is asked for by exactly one row: a logo painted over the
        // hero has to have an alpha channel or it draws a box.
        assert_eq!(
            google_colour_type(WallpaperCategory::Logo),
            Some("trans"),
            "the wordmark row must ask for transparency"
        );
        for category in [
            WallpaperCategory::Cover,
            WallpaperCategory::Landscape,
            WallpaperCategory::Background,
        ] {
            assert_eq!(google_colour_type(category), None);
        }
        assert_eq!(google_image_size(WallpaperCategory::Cover), "large");
        assert_eq!(google_image_size(WallpaperCategory::Landscape), "xlarge");
        assert_eq!(google_image_size(WallpaperCategory::Background), "huge");
        assert_eq!(google_image_size(WallpaperCategory::Logo), "medium");
    }

    #[test]
    fn parses_wikimedia_commons_file_results_with_their_size() {
        let built = parse_wikimedia_search(
            r#"{"query":{"pages":{"1":{"title":"File:Elden Ring art.jpg","imageinfo":[{"url":"//upload.wikimedia.org/full.jpg","thumburl":"//upload.wikimedia.org/thumb.jpg","width":1920,"height":1080}]},"2":{"title":"File:No_media.png","imageinfo":[]},"3":{"title":"File:Only full.png","imageinfo":[{"url":"//upload.wikimedia.org/full-only.png","thumburl":"","width":600,"height":900}]}}}}"#,
        )
        .unwrap();
        assert_eq!(built.len(), 2);
        assert_eq!(built[0].title, "Elden Ring art");
        assert_eq!(
            built[0].thumbnail_url,
            "https://upload.wikimedia.org/thumb.jpg"
        );
        assert_eq!(built[0].url, "https://upload.wikimedia.org/full.jpg");
        assert_eq!((built[0].width, built[0].height), (1920, 1080));
        assert_eq!(built[1].title, "Only full");
        assert_eq!(built[1].thumbnail_url, built[1].url);
    }

    /// CirrusSearch rejects `filew:>=1920` outright and answers `fileaspect:`
    /// with a silent zero results, so neither may ever be emitted.
    #[test]
    fn wikimedia_narrows_with_strict_comparisons_only() {
        for category in ALL_CATEGORIES {
            let clause = wikimedia_query("Elden Ring", category);
            assert!(clause.starts_with("Elden Ring "), "{clause}");
            assert!(clause.contains("filetype:bitmap"), "{clause}");
            assert!(!clause.contains("fileaspect"), "{clause}");
            assert!(!clause.contains(">="), "{clause}");
            assert!(!clause.contains("<="), "{clause}");
        }
        assert!(wikimedia_query("Elden Ring", WallpaperCategory::Cover).contains("fileh:>419"));
        assert!(wikimedia_query("Elden Ring", WallpaperCategory::Landscape).contains("filew:>599"));
        assert!(
            wikimedia_query("Elden Ring", WallpaperCategory::Background).contains("filew:>1279")
        );
    }

    #[test]
    fn parses_openverse_results_with_their_size() {
        let built = parse_openverse_search(
            r#"{"results":[{"id":"a1","title":"Elden Ring Wallpaper","url":"https://wall.example/full.jpg","thumbnail":"https://wall.example/thumb.jpg","width":1920,"height":1080},{"id":"a2","title":"No thumb","url":"https://wall.example/other.jpg","thumbnail":null,"width":800,"height":600},{"id":"a3","title":"","url":"https://wall.example/untitled.jpg","thumbnail":null,"width":600,"height":900}]}"#,
        )
        .unwrap();
        assert_eq!(built.len(), 3);
        assert_eq!(built[0].title, "Elden Ring Wallpaper");
        assert_eq!(built[0].thumbnail_url, "https://wall.example/thumb.jpg");
        assert_eq!((built[0].width, built[0].height), (1920, 1080));
        assert_eq!(built[1].thumbnail_url, built[1].url);
        assert_eq!(built[2].title, "Openverse image");
    }

    /// `aspect_ratio` is a coarse height-versus-width test, so it may only
    /// narrow the fetch — it can never be trusted as the row's answer.
    #[test]
    fn openverse_narrows_by_orientation_per_row() {
        assert_eq!(openverse_aspect_ratio(WallpaperCategory::Cover), "tall");
        assert_eq!(openverse_aspect_ratio(WallpaperCategory::Landscape), "wide");
        assert_eq!(
            openverse_aspect_ratio(WallpaperCategory::Background),
            "wide"
        );
        assert_eq!(openverse_aspect_ratio(WallpaperCategory::Logo), "wide");
        // Anonymous requests are refused with a 401 above 20 per page.
        const { assert!(OPENVERSE_PAGE_SIZE <= 20) };
    }

    // -----------------------------------------------------------------------
    // Ratio bucketing
    // -----------------------------------------------------------------------

    #[test]
    fn buckets_by_measured_ratio_at_every_boundary() {
        use WallpaperCategory::{Background, Cover, Landscape};
        // Portrait band, inclusive at both ends.
        assert_eq!(categories_for_size(560, 1000), vec![Cover]);
        assert_eq!(categories_for_size(800, 1000), vec![Cover]);
        assert_eq!(categories_for_size(600, 900), vec![Cover]);
        // Just outside 0.56 is too narrow to be box art.
        assert!(categories_for_size(559, 1000).is_empty());
        // Just past 0.80 lands in the dead zone that belongs to no row.
        assert!(categories_for_size(801, 1000).is_empty());
        assert!(categories_for_size(1000, 1000).is_empty());
        assert!(categories_for_size(1599, 1000).is_empty());
        // 1.60 opens the background band; landscape only starts at 1.70.
        assert_eq!(categories_for_size(1600, 1000), vec![Background]);
        assert_eq!(categories_for_size(1699, 1000), vec![Background]);
        // A true 16:9 is honestly both wide key art and a background.
        assert_eq!(categories_for_size(1700, 1000), vec![Landscape, Background]);
        assert_eq!(categories_for_size(1920, 1080), vec![Landscape, Background]);
        assert_eq!(categories_for_size(1900, 1000), vec![Landscape, Background]);
        // Past 1.90 it is no longer key art, just a wide background.
        assert_eq!(categories_for_size(1901, 1000), vec![Background]);
        assert_eq!(categories_for_size(6000, 1000), vec![Background]);
        // A panorama is not a wallpaper for this app.
        assert!(categories_for_size(6001, 1000).is_empty());
        // A wide image too small for a background is still key art.
        assert_eq!(categories_for_size(680, 400), vec![Landscape]);
        assert_eq!(categories_for_size(616, 353), vec![Landscape]);
        // Below the minimum edge or pixel budget nothing qualifies, whatever
        // the shape.
        assert!(categories_for_size(299, 1000).is_empty());
        assert!(categories_for_size(400, 199).is_empty());
        assert!(categories_for_size(300, 450).is_empty());
        assert!(categories_for_size(500, 299).is_empty());
        assert!(categories_for_size(0, 0).is_empty());
    }

    /// The portrait row is the one that must never be contaminated: no size
    /// that qualifies as a background or key art may also be a cover.
    #[test]
    fn a_wide_image_never_cross_lists_into_the_cover_row() {
        for (width, height) in [(1920, 1080), (1600, 1000), (680, 400), (6000, 1000)] {
            assert!(!categories_for_size(width, height).contains(&WallpaperCategory::Cover));
        }
        for (width, height) in [(600, 900), (560, 1000), (800, 1000)] {
            let buckets = categories_for_size(width, height);
            assert!(!buckets.contains(&WallpaperCategory::Landscape));
            assert!(!buckets.contains(&WallpaperCategory::Background));
        }
    }

    #[test]
    fn ranks_a_row_by_shape_first_then_size() {
        let entries = vec![
            sized("https://a.example/wide.jpg", 5120, 2880),
            sized("https://a.example/off.jpg", 6400, 4000),
            sized("https://a.example/exact.jpg", 3840, 2160),
            sized("https://a.example/portrait.jpg", 600, 900),
        ];
        let ranked = rank_for_category(entries.clone(), WallpaperCategory::Background);
        // 5120x2880 and 3840x2160 share the target ratio, so the larger wins;
        // 1.60 is further from 16:9 and trails both even though it has the most
        // pixels. The portrait entry is gone.
        assert_eq!(
            ranked
                .iter()
                .map(|entry| entry.url.as_str())
                .collect::<Vec<_>>(),
            vec![
                "https://a.example/wide.jpg",
                "https://a.example/exact.jpg",
                "https://a.example/off.jpg",
            ]
        );
        let ranked = rank_for_category(entries, WallpaperCategory::Cover);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].url, "https://a.example/portrait.jpg");
    }

    // -----------------------------------------------------------------------
    // Steam Store
    // -----------------------------------------------------------------------

    #[test]
    fn parses_steam_store_search_apps() {
        let apps = parse_steam_store_search(
            r#"{"items":[{"type":"app","id":1245620,"name":"ELDEN RING"},{"type":"sub","id":999,"name":"A package"},{"type":"app","id":0,"name":""},{"type":"app","id":3,"name":"Hollow Knight"}]}"#,
        )
        .unwrap();
        assert_eq!(
            apps,
            vec![
                (1245620, "ELDEN RING".to_owned()),
                (3, "Hollow Knight".to_owned())
            ]
        );
    }

    /// Every Steam row is a different set of paths on the one allowlisted host,
    /// so nothing from one row can appear in another.
    #[test]
    fn builds_a_distinct_steam_url_set_per_row() {
        let base = "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/1245620";
        let cover = steam_category_assets(WallpaperCategory::Cover, 1245620, "ELDEN RING");
        assert_eq!(cover.len(), 1);
        assert_eq!(cover[0].title, "ELDEN RING — cover");
        assert_eq!(cover[0].url, format!("{base}/library_600x900_2x.jpg"));
        assert_eq!(
            cover[0].thumbnail_url,
            format!("{base}/library_600x900.jpg")
        );
        // The portrait path is the only 2:3 asset Steam has and it is always
        // there, so it is never probed.
        assert!(!cover[0].optional);

        // Both wide rows lead with the library art. That is the whole fix for
        // "key art is a screenshot with the title on it": `library_hero` is the
        // untitled full-bleed scene Steam's own client paints, and the capsule
        // and header — which carry the wordmark baked in — are only fallbacks
        // for apps too old to have it.
        let landscape = steam_category_assets(WallpaperCategory::Landscape, 1245620, "ELDEN RING");
        assert_eq!(
            landscape
                .iter()
                .map(|asset| asset.url.as_str())
                .collect::<Vec<_>>(),
            vec![
                format!("{base}/library_hero_2x.jpg"),
                format!("{base}/library_hero.jpg"),
                format!("{base}/capsule_616x353.jpg"),
                format!("{base}/header.jpg"),
            ]
        );
        // The library art and the capsule 404 on older apps; the header never
        // does, so a row is never left empty by a probe.
        assert!(landscape[0].optional);
        assert!(landscape[2].optional);
        assert!(!landscape[3].optional);

        // The background row takes 4K and nothing else, so the 4K library art
        // is the only thing Steam publishes that can appear in it at all.
        // `library_hero.jpg` is 1920 wide, `page_bg_raw` is about 1438, and
        // `page_bg_generated_v6b` is a stretched header — none can qualify, so
        // none is requested and the row costs one probe rather than four.
        let background =
            steam_category_assets(WallpaperCategory::Background, 1245620, "ELDEN RING");
        assert_eq!(
            background
                .iter()
                .map(|asset| asset.url.as_str())
                .collect::<Vec<_>>(),
            vec![format!("{base}/library_hero_2x.jpg")]
        );
        assert!(background[0].optional);
        assert_eq!((background[0].width, background[0].height), (3840, 1240));

        // The wordmark is a separate asset, which is exactly why the rows above
        // are allowed to prefer art with no title on it. Both paths genuinely
        // 404 on plenty of apps, so both are probed and the row may come back
        // empty rather than offering a broken tile.
        let logo = steam_category_assets(WallpaperCategory::Logo, 1245620, "ELDEN RING");
        assert_eq!(
            logo.iter()
                .map(|asset| asset.url.as_str())
                .collect::<Vec<_>>(),
            vec![
                format!("{base}/logo.png"),
                format!("{base}/library_logo.png"),
            ]
        );
        assert!(logo.iter().all(|asset| asset.optional));

        // No row may reach for a size or version that is not published.
        for category in ALL_CATEGORIES {
            for asset in steam_category_assets(category, 1245620, "ELDEN RING") {
                assert!(
                    asset.url.starts_with(STEAM_ITEM_ASSETS_URL),
                    "{}",
                    asset.url
                );
                assert!(!asset.url.contains("capsule_616x353_2x"), "{}", asset.url);
                assert!(!asset.url.contains("header_2x"), "{}", asset.url);
            }
        }
        let names = ALL_CATEGORIES
            .iter()
            .flat_map(|category| steam_category_assets(*category, 1245620, "X"))
            .map(|asset| asset.url)
            .collect::<std::collections::HashSet<_>>();
        // The two wide rows share the 4K library art on purpose; everything
        // else is unique to its row.
        assert_eq!(names.len(), 7);
    }

    /// Only the portrait row spreads across store hits: it has one asset per
    /// app, while the wide rows already carry the best match's key art plus its
    /// screenshots and would only be diluted by other games.
    #[test]
    fn only_the_cover_row_spreads_across_matching_store_hits() {
        let matched = (0..6)
            .map(|index| (index, format!("Game {index}")))
            .collect::<Vec<_>>();
        assert_eq!(
            steam_apps_for_category(&matched, WallpaperCategory::Cover).len(),
            STEAM_MAX_APPS
        );
        assert_eq!(
            steam_apps_for_category(&matched, WallpaperCategory::Landscape).len(),
            1
        );
        assert_eq!(
            steam_apps_for_category(&matched, WallpaperCategory::Background).len(),
            1
        );
        // Another game's wordmark is the most obviously wrong thing a card can
        // wear, so the logo row never spreads either.
        assert_eq!(
            steam_apps_for_category(&matched, WallpaperCategory::Logo).len(),
            1
        );
        for category in ALL_CATEGORIES {
            assert!(steam_apps_for_category(&[], category).is_empty());
        }
    }

    #[test]
    fn source_names_serialise_for_the_webview() {
        assert_eq!(
            serde_json::to_value(WallpaperSource::SteamStore).unwrap(),
            serde_json::json!("steam-store")
        );
        assert_eq!(
            serde_json::from_value::<WallpaperSource>(serde_json::json!("steam-store")).unwrap(),
            WallpaperSource::SteamStore
        );
        assert_eq!(
            serde_json::to_value(WallpaperSource::Wikimedia).unwrap(),
            serde_json::json!("wikimedia")
        );
        assert_eq!(
            serde_json::to_value(WallpaperSource::Openverse).unwrap(),
            serde_json::json!("openverse")
        );
        assert_eq!(
            serde_json::to_value(WallpaperSource::Igdb).unwrap(),
            serde_json::json!("igdb")
        );
        assert_eq!(
            serde_json::from_value::<WallpaperSource>(serde_json::json!("google-images")).unwrap(),
            WallpaperSource::GoogleImages
        );
        assert_eq!(
            serde_json::to_value(WallpaperSource::SteamGridDb).unwrap(),
            serde_json::json!("steam-grid-db")
        );
        assert_eq!(
            serde_json::from_value::<WallpaperSource>(serde_json::json!("steam-grid-db")).unwrap(),
            WallpaperSource::SteamGridDb
        );
    }

    /// The row name crosses the IPC boundary in both directions — the command
    /// takes one and every view echoes it back — so it has to round-trip.
    #[test]
    fn category_names_round_trip_for_the_webview() {
        for (category, wire) in [
            (WallpaperCategory::Cover, "cover"),
            (WallpaperCategory::Landscape, "landscape"),
            (WallpaperCategory::Background, "background"),
            (WallpaperCategory::Logo, "logo"),
        ] {
            assert_eq!(
                serde_json::to_value(category).unwrap(),
                serde_json::json!(wire)
            );
            assert_eq!(
                serde_json::from_value::<WallpaperCategory>(serde_json::json!(wire)).unwrap(),
                category
            );
            assert_eq!(category.slug(), wire);
        }
        assert!(serde_json::from_value::<WallpaperCategory>(serde_json::json!("hero")).is_err());
    }

    #[test]
    fn surfaces_google_api_errors() {
        let result = parse_google_search(r#"{"error":{"message":"insufficientPermissions"}}"#);
        assert!(matches!(
            result,
            Err(WallpaperSearchError::Network(message)) if message.contains("insufficientPermissions")
        ));
    }

    #[test]
    fn registry_bounds_and_evicts_oldest() {
        let mut registry = CandidateRegistry::default();
        for index in 0..(REGISTRY_CAPACITY + 20) {
            let id = format!("wp:{index}");
            registry.insert(WallpaperCandidate {
                id: id.clone(),
                title: format!("artwork {index}"),
                thumbnail_url: format!("https://images.igdb.com/thumb-{index}.jpg"),
                url: format!("https://images.igdb.com/{index}.jpg"),
            });
            assert!(registry.get(&id).is_some());
        }
        assert_eq!(registry.entries.len(), REGISTRY_CAPACITY);
        // The first entries were evicted, the newest survived.
        assert!(registry.get("wp:0").is_none());
        assert!(registry.get("wp:5").is_none());
        assert!(
            registry
                .get(&format!("wp:{}", REGISTRY_CAPACITY + 19))
                .is_some()
        );
        // Duplicate ids never replace the first registration.
        let id = "wp:keep".to_owned();
        registry.insert(WallpaperCandidate {
            id: id.clone(),
            title: "first".into(),
            thumbnail_url: "https://a.example/first-thumb.jpg".into(),
            url: "https://a.example/first.jpg".into(),
        });
        registry.insert(WallpaperCandidate {
            id,
            title: "second".into(),
            thumbnail_url: "https://a.example/second-thumb.jpg".into(),
            url: "https://a.example/second.jpg".into(),
        });
        assert_eq!(
            registry.get("wp:keep").unwrap().url,
            "https://a.example/first.jpg"
        );
    }

    #[test]
    fn candidate_ids_are_opaque_and_unique() {
        let category = WallpaperCategory::Landscape;
        let first = mint_candidate_id(
            WallpaperSource::Igdb,
            category,
            "https://a.example/x.jpg",
            1,
        );
        let second = mint_candidate_id(
            WallpaperSource::Igdb,
            category,
            "https://a.example/x.jpg",
            2,
        );
        let third = mint_candidate_id(
            WallpaperSource::GoogleImages,
            category,
            "https://a.example/x.jpg",
            2,
        );
        assert!(first.starts_with("wp:"));
        assert!(first != second);
        assert!(second != third);
        for id in [&first, &second, &third] {
            assert!(validate_opaque_id("candidate id", id).is_ok());
        }
    }

    /// A 1920x1080 screenshot legitimately fills both wide rows, and the two
    /// rows are fetched separately, so the same URL must mint two different ids
    /// or importing from one row would resolve the other row's tile.
    #[test]
    fn the_same_url_gets_a_different_id_in_each_row() {
        let url = "https://a.example/x.jpg";
        let ids = ALL_CATEGORIES
            .iter()
            .map(|category| mint_candidate_id(WallpaperSource::SteamStore, *category, url, 7))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), ALL_CATEGORIES.len());
    }

    /// Two rows may hand the same URL to `to_candidates`; within one row a
    /// repeat is dropped, and anything that is not a URL never reaches the
    /// registry at all. The source travels with each entry now, because one row
    /// is filled from several at once.
    #[test]
    fn builds_candidates_per_row_and_drops_unusable_urls() {
        let entries = vec![
            (
                WallpaperSource::Wikimedia,
                (
                    "A".to_owned(),
                    "thumb.jpg".to_owned(),
                    "https://a.example/1.jpg".to_owned(),
                ),
            ),
            (
                WallpaperSource::SteamStore,
                (
                    "B".to_owned(),
                    "//a.example/2t.jpg".to_owned(),
                    "//a.example/2.jpg".to_owned(),
                ),
            ),
            (
                WallpaperSource::Openverse,
                (
                    "C".to_owned(),
                    String::new(),
                    "https://a.example/1.jpg".to_owned(),
                ),
            ),
            (
                WallpaperSource::Openverse,
                ("D".to_owned(), String::new(), "not-a-url.jpg".to_owned()),
            ),
        ];
        let built = to_candidates(WallpaperCategory::Cover, entries.clone());
        assert_eq!(built.len(), 2);
        // A rejected thumbnail falls back to the full image rather than to a
        // string the https-only client cannot request.
        assert_eq!(built[0].thumbnail_url, "https://a.example/1.jpg");
        assert_eq!(built[1].url, "https://a.example/2.jpg");
        assert_eq!(built[1].thumbnail_url, "https://a.example/2t.jpg");
        let other_row = to_candidates(WallpaperCategory::Background, entries);
        assert_eq!(other_row[0].url, built[0].url);
        assert_ne!(other_row[0].id, built[0].id);
    }

    /// An empty row has to name itself: the other two rows may have filled at
    /// the same moment, so "no results" alone would read as a lie.
    #[test]
    fn an_empty_row_says_which_row_is_empty() {
        let messages = ALL_CATEGORIES
            .iter()
            .map(|category| category.empty_message())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(messages.len(), ALL_CATEGORIES.len());
        assert!(WallpaperCategory::Cover.empty_message().contains("cover"));
    }

    // -----------------------------------------------------------------------
    // Search terms
    // -----------------------------------------------------------------------

    /// Every default term quotes the game and names the shape. Those two habits
    /// are the whole reason a keyword source answers with box art instead of
    /// the sequel's gameplay.
    #[test]
    fn every_default_term_quotes_the_game_and_names_the_shape() {
        for category in ALL_CATEGORIES {
            let expanded = expand_term(category.default_term(), "Elden Ring");
            assert!(
                expanded.starts_with("\"Elden Ring\""),
                "{category:?}: {expanded}"
            );
            // The shape noun has to survive expansion, or the row is just a
            // title search and every row returns the same pictures.
            assert!(
                category
                    .wanted_words()
                    .iter()
                    .any(|word| expanded.to_lowercase().contains(word)),
                "{category:?}: {expanded}"
            );
        }
        assert_eq!(
            expand_term(WallpaperCategory::Cover.default_term(), "Elden Ring"),
            "\"Elden Ring\" box art cover"
        );
        assert_eq!(
            expand_term(WallpaperCategory::Background.default_term(), "Elden Ring"),
            "\"Elden Ring\" wallpaper"
        );
    }

    /// `{Name}` is spelled however the user spelled it, because a template
    /// pasted out of Playnite has to keep working.
    #[test]
    fn a_term_template_substitutes_the_name_in_any_casing() {
        assert_eq!(expand_term("\"{Name}\" cover", "Doom"), "\"Doom\" cover");
        assert_eq!(expand_term("{NAME} logo", "Doom"), "Doom logo");
        assert_eq!(expand_term("{ name } art", "Doom"), "Doom art");
        // Whitespace left behind by an empty name is collapsed, not preserved.
        assert_eq!(expand_term("{name}   cover", "Doom"), "Doom cover");
        // An unknown variable is a visible typo, not something to delete
        // silently.
        assert_eq!(
            expand_term("{name} {platform} art", "Doom"),
            "Doom {platform} art"
        );
        // An unclosed brace must not swallow the rest of the template.
        assert_eq!(expand_term("{name art", "Doom"), "\"Doom\" {name art");
    }

    /// A template that forgets the name would search for the same thing however
    /// the library is browsed, so the name is added back rather than dropped.
    #[test]
    fn a_template_that_never_names_the_game_still_searches_for_it() {
        assert_eq!(expand_term("wallpaper 4k", "Doom"), "\"Doom\" wallpaper 4k");
        assert_eq!(expand_term("", "Doom"), "Doom");
        assert_eq!(expand_term("wallpaper", ""), "wallpaper");
    }

    // -----------------------------------------------------------------------
    // Caption scoring
    // -----------------------------------------------------------------------

    /// The bug this exists for: a gameplay still and a piece of key art are the
    /// same 1920x1080 rectangle, so geometry cannot separate them and the
    /// caption has to.
    #[test]
    fn a_gameplay_still_sinks_below_key_art_of_the_same_size() {
        let mut screenshot = sized("https://a.example/s.jpg", 1920, 1080);
        screenshot.title = "Elden Ring gameplay screenshot".into();
        let mut key_art = sized("https://a.example/k.jpg", 1920, 1080);
        key_art.title = "Elden Ring key art".into();
        let ranked = rank_for_category(vec![screenshot, key_art], WallpaperCategory::Landscape);
        assert_eq!(ranked[0].title, "Elden Ring key art");
        assert_eq!(ranked[1].title, "Elden Ring gameplay screenshot");

        // And the penalty is strong enough to beat a size advantage, which is
        // what used to put a 4K screenshot at the head of every wide row.
        let mut big_screenshot = sized("https://a.example/s4k.jpg", 3840, 2160);
        big_screenshot.title = "Elden Ring screenshot".into();
        let mut small_art = sized("https://a.example/k4k.jpg", 3840, 2160);
        small_art.title = "Elden Ring artwork".into();
        assert_eq!(
            rank_for_category(
                vec![big_screenshot, small_art],
                WallpaperCategory::Background
            )[0]
            .title,
            "Elden Ring artwork"
        );
    }

    /// The width floor is the one rule a typed endpoint does not get to bypass,
    /// because "backgrounds in 4K only" would mean nothing if the sources that
    /// know their own shape could ignore it.
    #[test]
    fn the_background_row_takes_four_k_from_every_source_alike() {
        // Steam's 1920x620 hero and SteamGridDB's 1920x620 one are both typed,
        // both correctly shaped — and both too small for this row.
        assert!(!admits(
            &typed(WallpaperSource::SteamStore, "hero", 1920, 620),
            WallpaperCategory::Background
        ));
        assert!(admits(
            &typed(WallpaperSource::SteamGridDb, "hero 4K", 3840, 1240),
            WallpaperCategory::Background
        ));
        // A measured 4K wallpaper qualifies too, so the floor is not a
        // back-door preference for one source.
        assert!(admits(
            &sized("https://a.example/w.jpg", 3840, 2160),
            WallpaperCategory::Background
        ));
        // The card rows keep a low floor: `header.jpg` at 460 wide is a real
        // last resort for a game with no library art at all.
        assert!(admits(
            &typed(WallpaperSource::SteamStore, "header", 460, 215),
            WallpaperCategory::Landscape
        ));
    }

    /// A 3840x1240 hero is 3.1:1, well outside the band a *measured* wide result
    /// has to fall inside. The band exists to catch a keyword engine returning a
    /// fan mock-up, and a hero is not that — so trusting the endpoint is what
    /// lets the best asset through without loosening the guard on the sources
    /// that need it.
    #[test]
    fn a_typed_endpoints_shape_is_trusted_where_a_measured_one_is_not() {
        let hero = typed(WallpaperSource::SteamGridDb, "hero", 3840, 1240);
        assert!(admits(&hero, WallpaperCategory::Landscape));

        let same_shape_from_the_web = SizedCandidate {
            trusted_shape: false,
            ..hero
        };
        assert!(!admits(
            &same_shape_from_the_web,
            WallpaperCategory::Landscape
        ));
        assert!(!fits_category(3840, 1240, WallpaperCategory::Landscape));
    }

    /// Six sources answer one row, so the same asset can arrive twice — a Steam
    /// CDN path that Google also indexed. It occupies one tile, and the typed
    /// copy is the one that leads.
    #[test]
    fn one_row_merges_every_source_and_keeps_each_picture_once() {
        let from_the_store = typed(
            WallpaperSource::SteamStore,
            "Elden Ring — key art",
            3840,
            1240,
        );
        // The same file reached a second way: a Steam CDN path Google indexed.
        let mut from_the_web = sized(&from_the_store.url, 3840, 1240);
        from_the_web.title = "Elden Ring wallpaper".into();
        let ranked = rank_for_category(
            vec![
                from_the_web,
                typed(
                    WallpaperSource::SteamGridDb,
                    "Elden Ring — hero",
                    3840,
                    1240,
                ),
                from_the_store,
            ],
            WallpaperCategory::Background,
        );
        assert_eq!(ranked.len(), 2, "the repeated URL collapsed to one tile");
        assert!(ranked.iter().all(|entry| entry.trusted_shape));
        assert_eq!(
            ranked
                .iter()
                .map(|entry| entry.source)
                .collect::<std::collections::HashSet<_>>(),
            std::collections::HashSet::from([
                WallpaperSource::SteamGridDb,
                WallpaperSource::SteamStore
            ]),
            "both stores are in the same row, competing on merit"
        );
    }

    #[test]
    fn caption_scoring_is_scoped_to_the_row_that_asked() {
        // "wallpaper" earns a point in the background row and costs three in
        // the portrait one — the same word, opposite meanings per row.
        assert!(keyword_score("Elden Ring wallpaper", WallpaperCategory::Background) > 0);
        assert!(keyword_score("Elden Ring wallpaper", WallpaperCategory::Cover) < 0);
        assert!(keyword_score("Elden Ring box art", WallpaperCategory::Cover) > 0);
        assert_eq!(keyword_score("Elden Ring", WallpaperCategory::Cover), 0);
        // Case never decides.
        assert_eq!(
            keyword_score("ELDEN RING GAMEPLAY", WallpaperCategory::Landscape),
            keyword_score("elden ring gameplay", WallpaperCategory::Landscape)
        );
    }

    /// A wordmark has no canonical ratio, so the logo row bounds only what a
    /// logo can never be — and nothing from another row leaks into it.
    #[test]
    fn the_logo_row_admits_wordmark_shapes_only() {
        assert!(fits_category(640, 240, WallpaperCategory::Logo));
        assert!(fits_category(1600, 400, WallpaperCategory::Logo));
        assert!(fits_category(400, 400, WallpaperCategory::Logo));
        // Too small to read.
        assert!(!fits_category(199, 240, WallpaperCategory::Logo));
        assert!(!fits_category(640, 79, WallpaperCategory::Logo));
        // A portrait cover is not a wordmark, and a wordmark is not a cover.
        assert!(!fits_category(600, 900, WallpaperCategory::Logo));
        assert!(!fits_category(640, 240, WallpaperCategory::Cover));
    }

    // -----------------------------------------------------------------------
    // Title matching
    // -----------------------------------------------------------------------

    /// Both catalogue APIs rank fuzzily, and taking hit #1 unchecked is how a
    /// game ends up wearing its sequel's artwork.
    #[test]
    fn the_exactly_named_game_wins_over_a_higher_ranked_relative() {
        let hits = vec!["Doom Eternal", "Doom II", "Doom"];
        assert_eq!(
            pick_best_named("Doom", hits.clone(), |name| name).unwrap(),
            "Doom"
        );
        // With no exact title anywhere, a loose match still beats the API's
        // own first choice.
        let hits = vec!["Some Unrelated Game", "Elden Ring Shadow of the Erdtree"];
        assert_eq!(
            pick_best_named("Elden Ring", hits, |name| name).unwrap(),
            "Elden Ring Shadow of the Erdtree"
        );
        // And with nothing matching at all the API's ranking still answers,
        // because the user typed this query and an empty row helps nobody.
        let hits = vec!["Hollow Knight", "Celeste"];
        assert_eq!(
            pick_best_named("Nothing Like This", hits, |name| name).unwrap(),
            "Hollow Knight"
        );
        assert_eq!(
            pick_best_named("Doom", Vec::<&str>::new(), |name| name),
            None
        );
    }

    // -----------------------------------------------------------------------
    // SteamGridDB
    // -----------------------------------------------------------------------

    /// The reason this source exists: `styles` is a real filter and `no_logo`
    /// is a real value in it, so the artwork rows can ask for pictures with no
    /// title burned in — which no other source here can express.
    ///
    /// The vocabulary is per asset kind, and that is not cosmetic: `no_logo` is
    /// a grid style, heroes reject it with a flat 400, and sending it to
    /// `/heroes` is what made every background row fail. A hero needs none — it
    /// is the scene Steam paints a separate wordmark over.
    #[test]
    fn every_artwork_row_asks_steamgriddb_to_leave_the_title_off() {
        const GRID_VOCABULARY: [&str; 5] =
            ["alternate", "blurred", "white_logo", "material", "no_logo"];
        const HERO_VOCABULARY: [&str; 3] = ["alternate", "blurred", "material"];
        for category in [
            WallpaperCategory::Cover,
            WallpaperCategory::Landscape,
            WallpaperCategory::Background,
        ] {
            for request in steamgriddb_requests(category) {
                let styles = request.styles.split(',').collect::<Vec<_>>();
                let vocabulary: &[&str] = match request.kind {
                    "grids" => &GRID_VOCABULARY,
                    "heroes" => &HERO_VOCABULARY,
                    other => panic!("{other} is not an artwork endpoint"),
                };
                for style in &styles {
                    assert!(
                        vocabulary.contains(style),
                        "{}/{style} is not in that endpoint's vocabulary",
                        request.kind
                    );
                }
                // `white_logo` and `material` are the two styles with the
                // wordmark composited in, and neither may ever be asked for.
                assert!(!styles.contains(&"white_logo"), "{}", request.kind);
                assert!(!styles.contains(&"material"), "{}", request.kind);
                // A grid is a Steam-shaped capsule and can carry a title, so it
                // has to ask for the de-logoed cut by name.
                if request.kind == "grids" {
                    assert!(styles.contains(&"no_logo"), "{category:?}");
                }
            }
        }
        // The wordmark row is the one place a logo is the point.
        let logo = steamgriddb_requests(WallpaperCategory::Logo);
        assert_eq!(logo.len(), 1);
        assert_eq!(logo[0].kind, "logos");
        assert!(logo[0].dimensions.is_empty());
        assert!(logo[0].styles.contains("official"));
    }

    /// The wide rows lead with heroes because they are both larger and cleaner
    /// than the Steam-shaped capsules, and the 4K hero is asked for on its own
    /// because `dimensions` is a set and not an order.
    #[test]
    fn steamgriddb_rows_ask_for_the_largest_clean_asset_first() {
        let cover = steamgriddb_requests(WallpaperCategory::Cover);
        assert_eq!(cover[0].kind, "grids");
        assert!(cover[0].dimensions.starts_with("600x900"));

        let landscape = steamgriddb_requests(WallpaperCategory::Landscape);
        assert_eq!(landscape[0].kind, "heroes");
        assert_eq!(landscape[1].kind, "grids");

        // 4K only, so there is one hero dimension the background row can use
        // and asking for 1920x620 as well would only fetch tiles it discards.
        let background = steamgriddb_requests(WallpaperCategory::Background);
        assert_eq!(background.len(), 1);
        assert_eq!(background[0].dimensions, "3840x1240");
    }

    /// SteamGridDB reports a refused key inside a 200 body, so an envelope that
    /// is not inspected reads as an empty row and hides the real problem.
    #[test]
    fn a_refused_steamgriddb_envelope_is_an_error_and_not_an_empty_row() {
        let parsed = parse_griddb::<Vec<GridDbAsset>>(
            r#"{"success":false,"errors":["Invalid authentication token"]}"#,
        );
        assert_eq!(
            parsed.err().map(|error| error.to_string()),
            Some("SteamGridDB reported: Invalid authentication token".to_owned())
        );
        // A blank reason still has to say something.
        assert!(parse_griddb::<Vec<GridDbAsset>>(r#"{"success":false}"#).is_err());
        let assets = parse_griddb::<Vec<GridDbAsset>>(
            r#"{"success":true,"data":[{"id":1,"url":"https://cdn2.steamgriddb.com/hero/a.png","thumb":"https://cdn2.steamgriddb.com/thumb/a.jpg","width":3840,"height":1240}]}"#,
        )
        .unwrap()
        .unwrap();
        let built = build_griddb_candidates(
            assets.clone(),
            "Elden Ring",
            WallpaperCategory::Background,
            0,
        );
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].title, "Elden Ring — background 1");
        assert_eq!(built[0].url, "https://cdn2.steamgriddb.com/hero/a.png");
        assert_eq!(
            built[0].thumbnail_url,
            "https://cdn2.steamgriddb.com/thumb/a.jpg"
        );
        assert_eq!((built[0].width, built[0].height), (3840, 1240));
        // A row fed by two requests keeps counting rather than captioning two
        // different pictures "key art 1".
        let second =
            build_griddb_candidates(assets, "Elden Ring", WallpaperCategory::Background, 3);
        assert_eq!(second[0].title, "Elden Ring — background 4");
    }

    #[test]
    fn a_search_term_is_encoded_before_it_lands_in_a_steamgriddb_path() {
        assert_eq!(
            urlencode_path("Marvel's Spider-Man"),
            "Marvel%27s%20Spider-Man"
        );
        assert_eq!(urlencode_path("a/../b"), "a%2F..%2Fb");
    }
}
