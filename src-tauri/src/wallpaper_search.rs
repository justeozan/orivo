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
//! Every search is scoped to one `WallpaperCategory`. The picker shows three
//! separate rows — portrait cover, wide key art, atmospheric background — and
//! fetches each independently, because a row is only useful if everything in it
//! has the shape the row promises. A 16:9 screenshot is never allowed to land in
//! the portrait row, so each source narrows per row rather than returning one
//! mixed pile: Steam builds each row from different asset paths, IGDB asks a
//! different endpoint, and the sources that report pixel dimensions are bucketed
//! by measured ratio.
//!
//! Search is built around Steam Store, a keyless source that returns real game
//! artwork from Steam's public endpoints — the same free source Playnite leans
//! on. Wikimedia Commons and Openverse are keyless fallbacks whose artwork
//! quality varies. IGDB and Google Images are optional higher-quality sources:
//! they need credentials, which a user can store in Settings or set as
//! environment variables. A value saved in Settings wins over an environment
//! variable. When an optional provider is not configured the command still
//! answers, with a `not-configured` phase and a copy explaining what to set.

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

const IGDB_TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const IGDB_API_URL: &str = "https://api.igdb.com/v4";
const GOOGLE_SEARCH_URL: &str = "https://www.googleapis.com/customsearch/v1";
const STEAM_STORE_SEARCH_URL: &str = "https://store.steampowered.com/api/storesearch/";
const STEAM_STORE_APPDETAILS_URL: &str = "https://store.steampowered.com/api/appdetails";
/// Steam's per-app asset CDN. Every Steam row is built from predictable paths
/// under this one host, which is also the only Steam asset host the WebView's
/// CSP allows — a second host would paint nothing.
const STEAM_ITEM_ASSETS_URL: &str =
    "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps";
const WIKIMEDIA_API_URL: &str = "https://commons.wikimedia.org/w/api.php";
const OPENVERSE_API_URL: &str = "https://api.openverse.org/v1/images/";

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
/// An anonymous Openverse client is refused with a 401 above 20 per page.
const OPENVERSE_PAGE_SIZE: u32 = 20;

// ---------------------------------------------------------------------------
// View shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Atmospheric backgrounds and screenshots.
    Background,
}

impl WallpaperCategory {
    fn slug(self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Landscape => "landscape",
            Self::Background => "background",
        }
    }

    /// The ratio a row is aiming at. Ranking sorts by distance from this, so a
    /// 2:3 cover beats a 3:4 one and a true 16:9 beats a 1.75:1 capsule.
    fn target_ratio(self) -> f64 {
        match self {
            Self::Cover => 0.667,
            Self::Landscape | Self::Background => 1.778,
        }
    }

    /// An empty row has to say which row is empty, because the other two may
    /// well have filled at the same time.
    fn empty_message(self) -> &'static str {
        match self {
            Self::Cover => "No cover art matched that search.",
            Self::Landscape => "No wide key art matched that search.",
            Self::Background => "No backgrounds matched that search.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WallpaperSearchPhase {
    Ready,
    NotConfigured,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperSearchView {
    phase: WallpaperSearchPhase,
    source: WallpaperSource,
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
        Self::Invalid(format!("the search service returned malformed data ({error})"))
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
    source: WallpaperSource,
    category: WallpaperCategory,
    entries: impl IntoIterator<Item = (String, String, String)>,
) -> Vec<WallpaperCandidate> {
    let mut seen = std::collections::HashSet::new();
    let mut built = Vec::new();
    for (title, raw_thumbnail, raw_url) in entries {
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

/// A search hit that carries its measured size, so it can be bucketed and
/// ranked before it ever reaches a row.
#[derive(Debug, Clone, PartialEq)]
struct SizedCandidate {
    title: String,
    thumbnail_url: String,
    url: String,
    width: u32,
    height: u32,
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

/// Keeps only what fits the row, best fit first: closest to the row's target
/// ratio, then largest. Shape wins over size because a row of the wrong shape
/// is worse than a row of smaller images.
fn rank_for_category(
    entries: Vec<SizedCandidate>,
    category: WallpaperCategory,
) -> Vec<SizedCandidate> {
    let target = category.target_ratio();
    let mut kept = entries
        .into_iter()
        .filter(|entry| categories_for_size(entry.width, entry.height).contains(&category))
        .collect::<Vec<_>>();
    kept.sort_by(|left, right| {
        (left.ratio() - target)
            .abs()
            .total_cmp(&(right.ratio() - target).abs())
            .then_with(|| right.pixels().cmp(&left.pixels()))
    });
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
        self.expires_at.duration_since(SystemTime::now())
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

#[derive(Debug, Clone, Deserialize)]
struct IgdbImage {
    #[serde(default)]
    url: String,
}

/// IGDB is the one source whose media is already sorted by shape, so a row asks
/// the endpoint that holds its shape instead of filtering a mixed pile:
/// `/covers` is portrait box art, `/artworks` is the wide key art an editor
/// picked, `/screenshots` is captured gameplay.
fn igdb_endpoint(category: WallpaperCategory) -> &'static str {
    match category {
        WallpaperCategory::Cover => "covers",
        WallpaperCategory::Landscape => "artworks",
        WallpaperCategory::Background => "screenshots",
    }
}

/// The (thumbnail, full) size tokens for a row. Covers have their own token
/// family; the wide rows share the screenshot one.
fn igdb_size_tokens(category: WallpaperCategory) -> (&'static str, &'static str) {
    match category {
        WallpaperCategory::Cover => ("cover_big", "cover_big_2x"),
        WallpaperCategory::Landscape | WallpaperCategory::Background => {
            ("screenshot_big", "1080p")
        }
    }
}

fn igdb_media_noun(category: WallpaperCategory) -> &'static str {
    match category {
        WallpaperCategory::Cover => "cover",
        WallpaperCategory::Landscape => "artwork",
        WallpaperCategory::Background => "screenshot",
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
    titled.into_iter().chain(rest).take(STEAM_MAX_APPS).collect()
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
    Ok(images.into_iter().filter(|image| !image.url.trim().is_empty()).collect())
}

/// Builds (title, thumbnail, full) triples from one IGDB media endpoint, at the
/// sizes that row wants.
fn build_igdb_candidates(
    images: &[IgdbImage],
    game_name: &str,
    category: WallpaperCategory,
) -> Vec<(String, String, String)> {
    let (thumbnail_size, full_size) = igdb_size_tokens(category);
    let noun = igdb_media_noun(category);
    let mut built = Vec::new();
    for (index, image) in images.iter().enumerate() {
        let Some(base) = https_url(&image.url) else {
            continue;
        };
        let thumbnail = swap_igdb_size(&base, thumbnail_size);
        let full = swap_igdb_size(&base, full_size);
        let title = format!("{game_name} — {noun} {}", index + 1);
        built.push((title, thumbnail, full));
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

/// Custom Search has no aspect or orientation parameter at all, so the query
/// text and `imgSize` are the only nudges available and the shape is decided
/// here, from the width and height each item reports.
fn google_query(query: &str, category: WallpaperCategory) -> String {
    match category {
        WallpaperCategory::Cover => format!("{query} box art"),
        WallpaperCategory::Landscape => format!("{query} screenshot 1920x1080"),
        WallpaperCategory::Background => format!("{query} key art wallpaper"),
    }
}

fn google_image_size(category: WallpaperCategory) -> &'static str {
    match category {
        WallpaperCategory::Cover => "large",
        WallpaperCategory::Landscape => "xlarge",
        WallpaperCategory::Background => "huge",
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
            title: item.title,
            thumbnail_url: thumbnail,
            url: full,
            width: image.width,
            height: image.height,
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
fn wikimedia_query(query: &str, category: WallpaperCategory) -> String {
    let (noun, min_width, min_height) = match category {
        WallpaperCategory::Cover => (
            "cover art",
            MIN_CANDIDATE_WIDTH - 1,
            COVER_MIN_HEIGHT - 1,
        ),
        WallpaperCategory::Landscape => (
            "key art",
            LANDSCAPE_MIN_WIDTH - 1,
            MIN_CANDIDATE_HEIGHT - 1,
        ),
        WallpaperCategory::Background => (
            "wallpaper",
            BACKGROUND_MIN_WIDTH - 1,
            MIN_CANDIDATE_HEIGHT - 1,
        ),
    };
    format!("{query} {noun} filetype:bitmap filew:>{min_width} fileh:>{min_height}")
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
            title,
            thumbnail_url: thumbnail,
            url: full,
            width: info.width,
            height: info.height,
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
        WallpaperCategory::Landscape | WallpaperCategory::Background => "wide",
    }
}

fn openverse_query(query: &str, category: WallpaperCategory) -> String {
    match category {
        WallpaperCategory::Cover => format!("{query} cover art"),
        WallpaperCategory::Landscape => format!("{query} key art"),
        WallpaperCategory::Background => format!("{query} wallpaper"),
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
            title,
            thumbnail_url: thumbnail,
            url: full,
            width: item.width,
            height: item.height,
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

#[derive(Deserialize)]
struct SteamAppDetailsEntry {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    data: Option<SteamAppData>,
}

/// Only what the one appdetails call is spent on. The endpoint has no portrait
/// field of any kind and no `library_*` key at all — for app 1245620 the
/// complete image-bearing key list is header_image, capsule_image,
/// capsule_imagev5, background, background_raw, screenshots and movies — and
/// every one of those except the screenshots has a predictable CDN path that
/// costs no request, so the screenshots are the only reason to call it.
#[derive(Debug, PartialEq, Deserialize)]
struct SteamAppData {
    #[serde(default)]
    name: String,
    #[serde(default)]
    screenshots: Vec<SteamScreenshot>,
}

#[derive(Debug, PartialEq, Deserialize)]
struct SteamScreenshot {
    #[serde(default)]
    path_full: String,
    #[serde(default)]
    path_thumbnail: String,
}

/// One asset on Steam's per-app CDN.
///
/// `optional` marks the two paths that genuinely 404 on some apps —
/// `capsule_616x353` and `page_bg_raw` — which are probed before they reach a
/// row so a miss paints as nothing rather than as a broken tile. Each of those
/// is paired with a sibling that is always present (`header`,
/// `page_bg_generated_v6b`), so a row is never left empty by a probe.
#[derive(Debug, Clone, PartialEq)]
struct SteamAsset {
    title: String,
    thumbnail_url: String,
    url: String,
    optional: bool,
}

impl SteamAsset {
    fn into_triple(self) -> (String, String, String) {
        (self.title, self.thumbnail_url, self.url)
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
/// built here too because the curated capsule and the page background beat
/// anything the API would name.
fn steam_category_assets(
    category: WallpaperCategory,
    app_id: u64,
    game: &str,
) -> Vec<SteamAsset> {
    match category {
        WallpaperCategory::Cover => vec![SteamAsset {
            title: format!("{game} — cover"),
            // The filename lies: `library_600x900.jpg` is the 300x450 copy and
            // the `_2x` sibling is the real 600x900.
            thumbnail_url: steam_asset_url(app_id, "library_600x900.jpg"),
            url: steam_asset_url(app_id, "library_600x900_2x.jpg"),
            optional: false,
        }],
        WallpaperCategory::Landscape => vec![
            SteamAsset {
                title: format!("{game} — key art"),
                thumbnail_url: steam_asset_url(app_id, "capsule_616x353.jpg"),
                url: steam_asset_url(app_id, "capsule_616x353.jpg"),
                optional: true,
            },
            SteamAsset {
                title: format!("{game} — header"),
                thumbnail_url: steam_asset_url(app_id, "header.jpg"),
                url: steam_asset_url(app_id, "header.jpg"),
                optional: false,
            },
        ],
        WallpaperCategory::Background => vec![
            SteamAsset {
                title: format!("{game} — page background"),
                thumbnail_url: steam_asset_url(app_id, "page_bg_raw.jpg"),
                url: steam_asset_url(app_id, "page_bg_raw.jpg"),
                optional: true,
            },
            SteamAsset {
                // `page_bg_generated.jpg` without the version suffix 404s on
                // some apps; the `_v6b` name is the one that is always there.
                title: format!("{game} — page background (dimmed)"),
                thumbnail_url: steam_asset_url(app_id, "page_bg_generated_v6b.jpg"),
                url: steam_asset_url(app_id, "page_bg_generated_v6b.jpg"),
                optional: false,
            },
        ],
    }
}

/// The 1920x1080 screenshots from app details — the only true 16:9 assets Steam
/// publishes, and the whole reason the wide rows spend their one API call.
fn steam_screenshot_assets(data: &SteamAppData, fallback_name: &str) -> Vec<SteamAsset> {
    let game = if data.name.trim().is_empty() {
        fallback_name.trim()
    } else {
        data.name.trim()
    };
    data.screenshots
        .iter()
        .enumerate()
        .filter(|(_, screenshot)| !screenshot.path_full.trim().is_empty())
        .map(|(index, screenshot)| {
            let thumbnail = if screenshot.path_thumbnail.trim().is_empty() {
                &screenshot.path_full
            } else {
                &screenshot.path_thumbnail
            };
            SteamAsset {
                title: format!("{game} — screenshot {}", index + 1),
                thumbnail_url: thumbnail.clone(),
                url: screenshot.path_full.clone(),
                optional: false,
            }
        })
        .collect()
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

fn parse_steam_app_details(body: &str) -> Result<Option<SteamAppData>, WallpaperSearchError> {
    let response: BTreeMap<String, SteamAppDetailsEntry> = serde_json::from_str(body)?;
    Ok(response
        .into_values()
        .find(|entry| entry.success && entry.data.is_some())
        .and_then(|entry| entry.data))
}

/// Which store hits may supply art for a row.
///
/// The portrait row spreads across every loosely matching hit, because Steam
/// has exactly one cover per app and a one-tile row is not a row; the extra
/// hits are the editions and sequels of the same title that store search ranks
/// next, and they cost nothing since covers come from a path. The wide rows
/// stay on the single best match: they already carry its capsule, header or
/// page background plus its screenshots, and mixing games into them would just
/// dilute the answer.
fn steam_apps_for_category(
    matched: &[(u64, String)],
    category: WallpaperCategory,
) -> &[(u64, String)] {
    match category {
        WallpaperCategory::Cover => &matched[..matched.len().min(STEAM_MAX_APPS)],
        WallpaperCategory::Landscape | WallpaperCategory::Background => {
            &matched[..matched.len().min(1)]
        }
    }
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

    pub fn with_parts(http: reqwest::Client, credentials: Arc<WallpaperCredentialsService>) -> Self {
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

    pub async fn search(
        &self,
        source: WallpaperSource,
        category: WallpaperCategory,
        query: &str,
        offset: u32,
    ) -> WallpaperSearchView {
        let query = query.trim().to_owned();
        if query.is_empty() {
            return WallpaperSearchView {
                phase: WallpaperSearchPhase::Error,
                source,
                category,
                query,
                message: "Type a search query first.".into(),
                candidates: Vec::new(),
            };
        }
        let outcome = match source {
            WallpaperSource::Igdb => self.search_igdb(category, &query, offset).await,
            WallpaperSource::GoogleImages => self.search_google(category, &query, offset).await,
            WallpaperSource::SteamStore => self.search_steam_store(category, &query, offset).await,
            WallpaperSource::Wikimedia => self.search_wikimedia(category, &query, offset).await,
            WallpaperSource::Openverse => self.search_openverse(category, &query, offset).await,
        };
        match outcome {
            Ok(candidates) => {
                let views = candidates
                    .into_iter()
                    .take(MAX_CANDIDATES)
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
                WallpaperSearchView {
                    phase: WallpaperSearchPhase::Ready,
                    source,
                    category,
                    query,
                    message: if views.is_empty() {
                        category.empty_message().to_owned()
                    } else {
                        format!("{} result(s)", views.len())
                    },
                    candidates: views,
                }
            }
            Err(WallpaperSearchError::NotConfigured(message)) => WallpaperSearchView {
                phase: WallpaperSearchPhase::NotConfigured,
                source,
                category,
                query,
                message,
                candidates: Vec::new(),
            },
            Err(other) => WallpaperSearchView {
                phase: WallpaperSearchPhase::Error,
                source,
                category,
                query,
                message: other.to_string(),
                candidates: Vec::new(),
            },
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
            .search_steam_store(WallpaperCategory::Landscape, query, 0)
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
        validate_opaque_id("candidate id", candidate_id)
            .map_err(|error| error.to_string())?;
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
        offset: u32,
    ) -> Result<Vec<WallpaperCandidate>, WallpaperSearchError> {
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
        let token = self.igdb_token(&client_id, &client_secret).await?;
        let games = self
            .igdb_query_games(&client_id, &token, query)
            .await?;
        let Some(top) = games.into_iter().next() else {
            return Ok(Vec::new());
        };
        let images = self
            .igdb_query_media(
                &client_id,
                &token,
                igdb_endpoint(category),
                top.id,
                IGDB_MEDIA_LIMIT,
                offset,
            )
            .await?;
        let game_name = if top.name.trim().is_empty() { query } else { top.name.trim() };
        Ok(to_candidates(
            WallpaperSource::Igdb,
            category,
            build_igdb_candidates(&images, game_name, category),
        ))
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
        let body = response
            .text()
            .await
            .map_err(|_| WallpaperSearchError::Network("IGDB sent an unreadable response.".into()))?;
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
        let expires_in = if parsed.expires_in > 0 { parsed.expires_in } else { 60 };
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
            "search \"{}\"; fields name; limit 5;",
            apicalypse_search(query)
        );
        let text = self
            .igdb_post(client_id, token, "games", &body)
            .await?;
        parse_igdb_games(&text)
    }

    async fn igdb_query_media(
        &self,
        client_id: &str,
        token: &str,
        endpoint: &str,
        game_id: i64,
        limit: usize,
        offset: u32,
    ) -> Result<Vec<IgdbImage>, WallpaperSearchError> {
        let body = format!(
            "fields url; where game = {game_id}; limit {limit}; offset {offset};"
        );
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
        offset: u32,
    ) -> Result<Vec<WallpaperCandidate>, WallpaperSearchError> {
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
        let response = self
            .inner
            .http
            .get(GOOGLE_SEARCH_URL)
            .query(&[
                ("key", key.as_str()),
                ("cx", cse_id.as_str()),
                ("q", google_query(query, category).as_str()),
                ("searchType", "image"),
                ("num", "10"),
                ("start", (offset + 1).to_string().as_str()),
                ("imgSize", google_image_size(category)),
                ("safe", "active"),
            ])
            .send()
            .await
            .map_err(|_| {
                WallpaperSearchError::Network("Google Images could not be reached. Try again.".into())
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
        Ok(to_candidates(
            WallpaperSource::GoogleImages,
            category,
            rank_for_category(parse_google_search(&body)?, category)
                .into_iter()
                .map(SizedCandidate::into_triple),
        ))
    }

    /// The built-in, keyless game-artwork source. One store search resolves the
    /// matching apps, and each row is then built from Steam's predictable asset
    /// paths. Only the wide rows spend the single appdetails call, and only for
    /// the screenshots — appdetails is the one rate-limited surface here, so the
    /// portrait row never touches it at all.
    async fn search_steam_store(
        &self,
        category: WallpaperCategory,
        query: &str,
        offset: u32,
    ) -> Result<Vec<WallpaperCandidate>, WallpaperSearchError> {
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
        let apps = steam_apps_for_category(&matched, category);
        let Some((top_id, top_name)) = apps.first() else {
            return Ok(Vec::new());
        };
        let mut assets = apps
            .iter()
            .flat_map(|(app_id, name)| steam_category_assets(category, *app_id, name))
            .collect::<Vec<_>>();
        if category != WallpaperCategory::Cover
            && let Ok(Some(data)) = self.steam_app_details(*top_id).await
        {
            assets.extend(steam_screenshot_assets(&data, top_name));
        }
        let present = join_all(assets.iter().map(|asset| self.steam_asset_present(asset))).await;
        Ok(to_candidates(
            WallpaperSource::SteamStore,
            category,
            assets
                .into_iter()
                .zip(present)
                .filter(|(_, present)| *present)
                .map(|(asset, _)| asset.into_triple()),
        )
        .into_iter()
        .skip(offset as usize)
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

    async fn steam_app_details(
        &self,
        app_id: u64,
    ) -> Result<Option<SteamAppData>, WallpaperSearchError> {
        let response = self
            .inner
            .http
            .get(STEAM_STORE_APPDETAILS_URL)
            .query(&[
                ("appids", app_id.to_string()),
                ("l", "en".to_owned()),
                ("cc", "us".to_owned()),
            ])
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
        parse_steam_app_details(&body)
    }

    /// The built-in, keyless source: a Wikimedia Commons file search, narrowed
    /// to the edge lengths the row needs and then measured by ratio.
    async fn search_wikimedia(
        &self,
        category: WallpaperCategory,
        query: &str,
        offset: u32,
    ) -> Result<Vec<WallpaperCandidate>, WallpaperSearchError> {
        let response = self
            .inner
            .http
            .get(WIKIMEDIA_API_URL)
            .query(&[
                ("action", "query".to_owned()),
                ("generator", "search".to_owned()),
                ("gsrsearch", wikimedia_query(query, category)),
                ("gsrnamespace", "6".to_owned()),
                ("gsrlimit", "32".to_owned()),
                ("gsroffset", offset.to_string()),
                ("prop", "imageinfo".to_owned()),
                // `size` is what makes the ratio bucketing possible at all.
                ("iiprop", "url|size".to_owned()),
                ("iiurlwidth", "640".to_owned()),
                ("format", "json".to_owned()),
            ])
            .send()
            .await
            .map_err(|_| {
                WallpaperSearchError::Network("Wikimedia Commons could not be reached. Try again.".into())
            })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|_| WallpaperSearchError::Network("Wikimedia Commons sent an unreadable response.".into()))?;
        if !status.is_success() {
            return Err(WallpaperSearchError::Network(format!(
                "Wikimedia Commons returned status {status}."
            )));
        }
        Ok(to_candidates(
            WallpaperSource::Wikimedia,
            category,
            rank_for_category(parse_wikimedia_search(&body)?, category)
                .into_iter()
                .map(SizedCandidate::into_triple),
        ))
    }

    /// The second built-in, keyless source: Openverse's openly licensed image
    /// index. Large results lean toward wallpaper-sized artwork.
    async fn search_openverse(
        &self,
        category: WallpaperCategory,
        query: &str,
        offset: u32,
    ) -> Result<Vec<WallpaperCandidate>, WallpaperSearchError> {
        let page = offset / OPENVERSE_PAGE_SIZE + 1;
        let response = self
            .inner
            .http
            .get(OPENVERSE_API_URL)
            .query(&[
                ("q", openverse_query(query, category)),
                // Reuse is the point of the index; `all` is not a documented
                // value for this filter.
                ("license_type", "commercial,modification".to_owned()),
                ("aspect_ratio", openverse_aspect_ratio(category).to_owned()),
                ("size", "large".to_owned()),
                ("page_size", OPENVERSE_PAGE_SIZE.to_string()),
                ("page", page.to_string()),
            ])
            .send()
            .await
            .map_err(|_| {
                WallpaperSearchError::Network("Openverse could not be reached. Try again.".into())
            })?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|_| WallpaperSearchError::Network("Openverse sent an unreadable response.".into()))?;
        if !status.is_success() {
            return Err(WallpaperSearchError::Network(format!(
                "Openverse returned status {status}."
            )));
        }
        Ok(to_candidates(
            WallpaperSource::Openverse,
            category,
            rank_for_category(parse_openverse_search(&body)?, category)
                .into_iter()
                .map(SizedCandidate::into_triple),
        ))
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Search one row of a wallpaper provider. Never fails on a missing credential
/// — it answers with a `not-configured` phase instead, so the page can explain.
#[tauri::command]
pub async fn search_wallpapers(
    source: WallpaperSource,
    category: WallpaperCategory,
    query: String,
    offset: u32,
    service: State<'_, WallpaperSearchService>,
) -> Result<WallpaperSearchView, String> {
    Ok(service.search(source, category, &query, offset).await)
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

    const CATEGORIES: [WallpaperCategory; 3] = [
        WallpaperCategory::Cover,
        WallpaperCategory::Landscape,
        WallpaperCategory::Background,
    ];

    fn sized(url: &str, width: u32, height: u32) -> SizedCandidate {
        SizedCandidate {
            title: url.to_owned(),
            thumbnail_url: format!("{url}#thumb"),
            url: url.to_owned(),
            width,
            height,
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
            credential_for("", "IGDB Client Secret", "ORIVO_IGDB_CLIENT_SECRET", &present).unwrap(),
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
            credential_for("from-settings", "IGDB Client ID", "ORIVO_IGDB_CLIENT_ID", &env).unwrap(),
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
        assert_eq!(
            apicalypse_search("two; statements"),
            "two; statements"
        );
        assert_eq!(apicalypse_search("line\nbreak"), "linebreak");
    }

    #[test]
    fn parses_igdb_game_results() {
        let games = parse_igdb_games(
            r#"[{"id":118600,"name":"Elden Ring"},{"id":0,"name":"junk"}]"#,
        )
        .unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].id, 118600);
        assert_eq!(games[0].name, "Elden Ring");
    }

    /// IGDB is asked a different endpoint per row, so nothing has to be
    /// filtered afterwards — `/covers` cannot return a screenshot.
    #[test]
    fn igdb_asks_one_endpoint_and_one_size_family_per_row() {
        assert_eq!(igdb_endpoint(WallpaperCategory::Cover), "covers");
        assert_eq!(igdb_endpoint(WallpaperCategory::Landscape), "artworks");
        assert_eq!(igdb_endpoint(WallpaperCategory::Background), "screenshots");
        assert_eq!(
            igdb_size_tokens(WallpaperCategory::Cover),
            ("cover_big", "cover_big_2x")
        );
        assert_eq!(
            igdb_size_tokens(WallpaperCategory::Landscape),
            ("screenshot_big", "1080p")
        );
        assert_eq!(
            igdb_size_tokens(WallpaperCategory::Background),
            ("screenshot_big", "1080p")
        );
    }

    #[test]
    fn parses_igdb_artwork_and_builds_candidates_at_the_row_size() {
        let images = parse_igdb_images(
            r#"[{"id":1,"url":"//images.igdb.com/igdb/image/upload/t_thumb/a.jpg"},{"id":2}]"#,
        )
        .unwrap();
        let built = build_igdb_candidates(&images, "Elden Ring", WallpaperCategory::Landscape);
        assert_eq!(built.len(), 1);
        let (title, thumbnail, full) = &built[0];
        assert_eq!(title, "Elden Ring — artwork 1");
        assert!(thumbnail.ends_with("/t_screenshot_big/a.jpg"));
        assert!(full.ends_with("/t_1080p/a.jpg"));
        let built = build_igdb_candidates(&images, "Elden Ring", WallpaperCategory::Cover);
        let (title, thumbnail, full) = &built[0];
        assert_eq!(title, "Elden Ring — cover 1");
        assert!(thumbnail.ends_with("/t_cover_big/a.jpg"));
        assert!(full.ends_with("/t_cover_big_2x/a.jpg"));
        let built = build_igdb_candidates(&images, "Elden Ring", WallpaperCategory::Background);
        assert_eq!(built[0].0, "Elden Ring — screenshot 1");
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
        assert_eq!(built[0].thumbnail_url, "https://encrypted-tbn0.gstatic.com/thumb.jpg");
        assert_eq!(built[0].url, "https://wall.example/full.jpg");
        assert_eq!((built[0].width, built[0].height), (1920, 1080));
        assert_eq!(built[1].thumbnail_url, built[1].url);
    }

    /// Custom Search exposes no orientation knob, so the only source-side
    /// nudges are the query wording and `imgSize`.
    #[test]
    fn google_varies_its_query_and_size_per_row() {
        assert_eq!(google_query("Elden Ring", WallpaperCategory::Cover), "Elden Ring box art");
        assert_eq!(
            google_query("Elden Ring", WallpaperCategory::Landscape),
            "Elden Ring screenshot 1920x1080"
        );
        assert_eq!(
            google_query("Elden Ring", WallpaperCategory::Background),
            "Elden Ring key art wallpaper"
        );
        assert_eq!(google_image_size(WallpaperCategory::Cover), "large");
        assert_eq!(google_image_size(WallpaperCategory::Landscape), "xlarge");
        assert_eq!(google_image_size(WallpaperCategory::Background), "huge");
    }

    #[test]
    fn parses_wikimedia_commons_file_results_with_their_size() {
        let built = parse_wikimedia_search(
            r#"{"query":{"pages":{"1":{"title":"File:Elden Ring art.jpg","imageinfo":[{"url":"//upload.wikimedia.org/full.jpg","thumburl":"//upload.wikimedia.org/thumb.jpg","width":1920,"height":1080}]},"2":{"title":"File:No_media.png","imageinfo":[]},"3":{"title":"File:Only full.png","imageinfo":[{"url":"//upload.wikimedia.org/full-only.png","thumburl":"","width":600,"height":900}]}}}}"#,
        )
        .unwrap();
        assert_eq!(built.len(), 2);
        assert_eq!(built[0].title, "Elden Ring art");
        assert_eq!(built[0].thumbnail_url, "https://upload.wikimedia.org/thumb.jpg");
        assert_eq!(built[0].url, "https://upload.wikimedia.org/full.jpg");
        assert_eq!((built[0].width, built[0].height), (1920, 1080));
        assert_eq!(built[1].title, "Only full");
        assert_eq!(built[1].thumbnail_url, built[1].url);
    }

    /// CirrusSearch rejects `filew:>=1920` outright and answers `fileaspect:`
    /// with a silent zero results, so neither may ever be emitted.
    #[test]
    fn wikimedia_narrows_with_strict_comparisons_only() {
        for category in CATEGORIES {
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
        assert_eq!(openverse_aspect_ratio(WallpaperCategory::Background), "wide");
        assert_eq!(
            openverse_query("Elden Ring", WallpaperCategory::Cover),
            "Elden Ring cover art"
        );
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
            sized("https://a.example/wide.jpg", 2560, 1440),
            sized("https://a.example/off.jpg", 1600, 1000),
            sized("https://a.example/exact.jpg", 1920, 1080),
            sized("https://a.example/portrait.jpg", 600, 900),
        ];
        let ranked = rank_for_category(entries.clone(), WallpaperCategory::Background);
        // 2560x1440 and 1920x1080 share the target ratio, so the larger wins;
        // 1.60 is further from 16:9 and trails both. The portrait entry is gone.
        assert_eq!(
            ranked.iter().map(|entry| entry.url.as_str()).collect::<Vec<_>>(),
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
        assert_eq!(apps, vec![(1245620, "ELDEN RING".to_owned()), (3, "Hollow Knight".to_owned())]);
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
        assert_eq!(cover[0].thumbnail_url, format!("{base}/library_600x900.jpg"));
        // The portrait path is the only 2:3 asset Steam has and it is always
        // there, so it is never probed.
        assert!(!cover[0].optional);

        let landscape = steam_category_assets(WallpaperCategory::Landscape, 1245620, "ELDEN RING");
        assert_eq!(
            landscape.iter().map(|asset| asset.url.as_str()).collect::<Vec<_>>(),
            vec![
                format!("{base}/capsule_616x353.jpg"),
                format!("{base}/header.jpg"),
            ]
        );
        // The capsule 404s on some apps; the header never does.
        assert!(landscape[0].optional);
        assert!(!landscape[1].optional);

        let background = steam_category_assets(WallpaperCategory::Background, 1245620, "ELDEN RING");
        assert_eq!(
            background.iter().map(|asset| asset.url.as_str()).collect::<Vec<_>>(),
            vec![
                format!("{base}/page_bg_raw.jpg"),
                format!("{base}/page_bg_generated_v6b.jpg"),
            ]
        );
        assert!(background[0].optional);
        assert!(!background[1].optional);

        // No row may reach for a size or version that is not published.
        for category in CATEGORIES {
            for asset in steam_category_assets(category, 1245620, "ELDEN RING") {
                assert!(asset.url.starts_with(STEAM_ITEM_ASSETS_URL), "{}", asset.url);
                assert!(!asset.url.contains("capsule_616x353_2x"), "{}", asset.url);
                assert!(!asset.url.contains("header_2x"), "{}", asset.url);
                assert!(!asset.url.contains("library_hero"), "{}", asset.url);
            }
        }
        let names = CATEGORIES
            .iter()
            .flat_map(|category| steam_category_assets(*category, 1245620, "X"))
            .map(|asset| asset.url)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(names.len(), 5);
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
        for category in CATEGORIES {
            assert!(steam_apps_for_category(&[], category).is_empty());
        }
    }

    #[test]
    fn parses_steam_app_details_and_builds_only_its_screenshots() {
        let data = parse_steam_app_details(
            r#"{"1245620":{"success":true,"data":{"name":"Elden Ring","header_image":"https://shared.akamai.steamstatic.com/header.jpg","background":"https://store.akamai.steamstatic.com/bg","screenshots":[{"path_full":"https://shared.akamai.steamstatic.com/ss.1920x1080.jpg","path_thumbnail":"https://shared.akamai.steamstatic.com/ss.600x338.jpg"},{"path_full":"","path_thumbnail":""},{"path_full":"https://shared.akamai.steamstatic.com/ss2.1920x1080.jpg","path_thumbnail":""}]}},"0":{"success":false,"data":null}}"#,
        )
        .unwrap()
        .unwrap();
        // The one appdetails call is spent on the 1920x1080 screenshots alone;
        // every other asset comes from a path that costs no request.
        let built = steam_screenshot_assets(&data, "Elden Ring");
        assert_eq!(built.len(), 2);
        assert_eq!(built[0].title, "Elden Ring — screenshot 1");
        assert_eq!(built[0].thumbnail_url, "https://shared.akamai.steamstatic.com/ss.600x338.jpg");
        assert_eq!(built[0].url, "https://shared.akamai.steamstatic.com/ss.1920x1080.jpg");
        assert!(!built[0].optional);
        // A screenshot with no thumbnail previews itself.
        assert_eq!(built[1].title, "Elden Ring — screenshot 3");
        assert_eq!(built[1].thumbnail_url, built[1].url);
        // A nameless payload falls back to the store search's name.
        let data = parse_steam_app_details(
            r#"{"1":{"success":true,"data":{"screenshots":[{"path_full":"https://a.example/s.jpg"}]}}}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(steam_screenshot_assets(&data, "Fallback")[0].title, "Fallback — screenshot 1");
        // A failed lookup yields nothing rather than an error.
        assert_eq!(parse_steam_app_details(r#"{"999":{"success":false}}"#).unwrap(), None);
    }

    /// A missing CDN asset is a 404 with an HTML body, never a placeholder
    /// image, so the content type is what separates a hit from a miss.
    #[test]
    fn treats_a_steam_cdn_miss_as_absent() {
        assert!(steam_asset_response_is_image(200, "image/jpeg"));
        assert!(steam_asset_response_is_image(200, " IMAGE/JPEG "));
        assert!(!steam_asset_response_is_image(404, "text/html"));
        assert!(!steam_asset_response_is_image(200, "text/html"));
        assert!(!steam_asset_response_is_image(500, "image/jpeg"));
        assert!(!steam_asset_response_is_image(200, ""));
    }

    // -----------------------------------------------------------------------
    // Views, ids and registry
    // -----------------------------------------------------------------------

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
    }

    /// The row name crosses the IPC boundary in both directions — the command
    /// takes one and every view echoes it back — so it has to round-trip.
    #[test]
    fn category_names_round_trip_for_the_webview() {
        for (category, wire) in [
            (WallpaperCategory::Cover, "cover"),
            (WallpaperCategory::Landscape, "landscape"),
            (WallpaperCategory::Background, "background"),
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
        let result = parse_google_search(
            r#"{"error":{"message":"insufficientPermissions"}}"#,
        );
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
        assert!(registry.get(&format!("wp:{}", REGISTRY_CAPACITY + 19)).is_some());
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
        let first = mint_candidate_id(WallpaperSource::Igdb, category, "https://a.example/x.jpg", 1);
        let second = mint_candidate_id(WallpaperSource::Igdb, category, "https://a.example/x.jpg", 2);
        let third =
            mint_candidate_id(WallpaperSource::GoogleImages, category, "https://a.example/x.jpg", 2);
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
        let ids = CATEGORIES
            .iter()
            .map(|category| mint_candidate_id(WallpaperSource::SteamStore, *category, url, 7))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), CATEGORIES.len());
    }

    /// Two rows may hand the same URL to `to_candidates`; within one row a
    /// repeat is dropped, and anything that is not a URL never reaches the
    /// registry at all.
    #[test]
    fn builds_candidates_per_row_and_drops_unusable_urls() {
        let entries = vec![
            ("A".to_owned(), "thumb.jpg".to_owned(), "https://a.example/1.jpg".to_owned()),
            ("B".to_owned(), "//a.example/2t.jpg".to_owned(), "//a.example/2.jpg".to_owned()),
            ("C".to_owned(), String::new(), "https://a.example/1.jpg".to_owned()),
            ("D".to_owned(), String::new(), "not-a-url.jpg".to_owned()),
        ];
        let built = to_candidates(
            WallpaperSource::Wikimedia,
            WallpaperCategory::Cover,
            entries.clone(),
        );
        assert_eq!(built.len(), 2);
        // A rejected thumbnail falls back to the full image rather than to a
        // string the https-only client cannot request.
        assert_eq!(built[0].thumbnail_url, "https://a.example/1.jpg");
        assert_eq!(built[1].url, "https://a.example/2.jpg");
        assert_eq!(built[1].thumbnail_url, "https://a.example/2t.jpg");
        let other_row =
            to_candidates(WallpaperSource::Wikimedia, WallpaperCategory::Background, entries);
        assert_eq!(other_row[0].url, built[0].url);
        assert_ne!(other_row[0].id, built[0].id);
    }

    /// An empty row has to name itself: the other two rows may have filled at
    /// the same moment, so "no results" alone would read as a lie.
    #[test]
    fn an_empty_row_says_which_row_is_empty() {
        let messages = CATEGORIES
            .iter()
            .map(|category| category.empty_message())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(messages.len(), CATEGORIES.len());
        assert!(
            WallpaperCategory::Cover
                .empty_message()
                .contains("cover")
        );
    }
}
