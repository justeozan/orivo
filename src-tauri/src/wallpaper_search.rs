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
//! Search is built around Steam Store, a keyless source that returns real game
//! artwork (store header, page background, library hero, screenshots) from
//! Steam's public endpoints — the same free source Playnite leans on. Wikimedia
//! Commons and Openverse are keyless fallbacks whose artwork quality varies.
//! IGDB and Google Images are optional higher-quality sources: they need
//! credentials, which a user can store in Settings or set as environment
//! variables. A value saved in Settings wins over an environment variable.
//! When an optional provider is not configured the command still answers,
//! with a `not-configured` phase and a copy explaining what to set.

use crate::game_detail::{GameMediaView, validate_opaque_id};
use crate::game_media::GameMediaService;
use crate::wallpaper_credentials::{WallpaperCredentialsDto, WallpaperCredentialsService};
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
const WIKIMEDIA_API_URL: &str = "https://commons.wikimedia.org/w/api.php";
const OPENVERSE_API_URL: &str = "https://api.openverse.org/v1/images/";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CANDIDATES: usize = 16;
const REGISTRY_CAPACITY: usize = 512;
/// A freshly minted IGDB token is reused until it is about to expire.
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------
// View shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WallpaperSource {
    /// Built-in, keyless: Steam's public store search + app details, returning
    /// real game header, background, hero and screenshots.
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperSearchView {
    phase: WallpaperSearchPhase,
    source: WallpaperSource,
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

/// Resolves one credential: a value saved in Settings wins, then an
/// environment variable, then a `not-configured` answer. The accessors are
/// injectable so every path is testable without touching process state.
fn credential_for(
    stored: &str,
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
            "This source is not configured yet — set {key} in Settings or as an environment variable."
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

fn mint_candidate_id(source: WallpaperSource, url: &str, sequence: u64) -> String {
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
    digest.update(url.as_bytes());
    format!("wp:{:x}:{sequence}", digest.finalize())
}

static CANDIDATE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

fn https_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_prefix("//").unwrap_or(trimmed);
    if trimmed.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    }
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

/// Builds (title, thumbnail, full) triples from IGDB artwork/screenshot URLs.
fn build_igdb_candidates(images: &[IgdbImage], game_name: &str) -> Vec<(String, String, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut built = Vec::new();
    for (index, image) in images.iter().enumerate() {
        let base = https_url(&image.url);
        if !base.starts_with("https://") || !seen.insert(base.clone()) {
            continue;
        }
        let thumbnail = swap_igdb_size(&base, "screenshot_big");
        let full = swap_igdb_size(&base, "1080p");
        let title = format!("{game_name} — artwork {}", index + 1);
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
}

fn parse_google_search(body: &str) -> Result<Vec<(String, String, String)>, WallpaperSearchError> {
    let response: GoogleSearchResponse = serde_json::from_str(body)?;
    if let Some(error) = response.error
        && !error.message.trim().is_empty()
    {
        return Err(WallpaperSearchError::Network(format!(
            "Google Images reported: {}",
            error.message
        )));
    }
    let mut seen = std::collections::HashSet::new();
    let mut built = Vec::new();
    for item in response.items.unwrap_or_default() {
        let full = https_url(&item.link);
        if !full.starts_with("https://") || !seen.insert(full.clone()) {
            continue;
        }
        let thumbnail = item
            .image
            .and_then(|image| Some(https_url(&image.thumbnail_link)))
            .filter(|url| url.starts_with("https://"))
            .unwrap_or_else(|| full.clone());
        built.push((item.title, thumbnail, full));
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
}

fn parse_wikimedia_search(body: &str) -> Result<Vec<(String, String, String)>, WallpaperSearchError> {
    let response: WikimediaSearchResponse = serde_json::from_str(body)?;
    let mut seen = std::collections::HashSet::new();
    let mut built = Vec::new();
    for page in response.query.unwrap_or_default().pages.into_values() {
        let Some(info) = page.imageinfo.into_iter().next() else {
            continue;
        };
        let full = https_url(&info.url);
        if !full.starts_with("https://") || !seen.insert(full.clone()) {
            continue;
        }
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
        let thumbnail = if info.thumburl.trim().is_empty() {
            full.clone()
        } else {
            https_url(&info.thumburl)
        };
        built.push((title, thumbnail, full));
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
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    thumbnail: Option<String>,
}

fn parse_openverse_search(body: &str) -> Result<Vec<(String, String, String)>, WallpaperSearchError> {
    let response: OpenverseSearchResponse = serde_json::from_str(body)?;
    let mut seen = std::collections::HashSet::new();
    let mut built = Vec::new();
    for item in response.results {
        let full = https_url(&item.url);
        if !full.starts_with("https://") || !seen.insert(full.clone()) {
            continue;
        }
        let thumbnail = item
            .thumbnail
            .as_deref()
            .map(https_url)
            .filter(|url| url.starts_with("https://"))
            .unwrap_or_else(|| full.clone());
        let title = if item.title.trim().is_empty() {
            "Openverse image".into()
        } else {
            item.title.trim().to_owned()
        };
        built.push((title, thumbnail, full));
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

#[derive(Debug, PartialEq, Deserialize)]
struct SteamAppData {
    #[serde(default)]
    name: String,
    #[serde(default)]
    header_image: String,
    #[serde(default)]
    background: String,
    #[serde(default)]
    library_hero: String,
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

/// Builds (title, thumbnail, full) triples from a Steam app's key art and
/// screenshots. Steam images are already https, so `https_url` is a no-op that
/// keeps the shape honest.
fn build_steam_candidates(data: &SteamAppData, fallback_name: &str) -> Vec<(String, String, String)> {
    let game = if data.name.trim().is_empty() {
        fallback_name.trim()
    } else {
        data.name.trim()
    };
    let mut seen = std::collections::HashSet::new();
    let mut built = Vec::new();
    let mut push = |title: String, raw_thumbnail: &str, raw_full: &str| {
        let full = https_url(raw_full);
        if !full.starts_with("https://") || !seen.insert(full.clone()) {
            return;
        }
        let thumbnail = https_url(raw_thumbnail);
        let thumbnail = if thumbnail.starts_with("https://") {
            thumbnail
        } else {
            full.clone()
        };
        built.push((title, thumbnail, full));
    };
    if !data.header_image.trim().is_empty() {
        push(
            format!("{game} — header"),
            &data.header_image,
            &data.header_image,
        );
    }
    if !data.background.trim().is_empty() {
        push(
            format!("{game} — store background"),
            &data.background,
            &data.background,
        );
    }
    if !data.library_hero.trim().is_empty() {
        push(
            format!("{game} — library hero"),
            &data.library_hero,
            &data.library_hero,
        );
    }
    for (index, screenshot) in data.screenshots.iter().enumerate() {
        if screenshot.path_full.trim().is_empty() {
            continue;
        }
        let thumbnail = if screenshot.path_thumbnail.trim().is_empty() {
            &screenshot.path_full
        } else {
            &screenshot.path_thumbnail
        };
        push(
            format!("{game} — screenshot {}", index + 1),
            thumbnail,
            &screenshot.path_full,
        );
    }
    built
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
        query: &str,
        offset: u32,
    ) -> WallpaperSearchView {
        let query = query.trim().to_owned();
        if query.is_empty() {
            return WallpaperSearchView {
                phase: WallpaperSearchPhase::Error,
                source,
                query,
                message: "Type a search query first.".into(),
                candidates: Vec::new(),
            };
        }
        let outcome = match source {
            WallpaperSource::Igdb => self.search_igdb(&query, offset).await,
            WallpaperSource::GoogleImages => self.search_google(&query, offset).await,
            WallpaperSource::SteamStore => self.search_steam_store(&query, offset).await,
            WallpaperSource::Wikimedia => self.search_wikimedia(&query, offset).await,
            WallpaperSource::Openverse => self.search_openverse(&query, offset).await,
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
                        };
                        let stored = WallpaperCandidate {
                            id: candidate.id,
                            title: candidate.title,
                            thumbnail_url: candidate.thumbnail_url,
                            url: candidate.url,
                        };
                        if let Ok(mut registry) = self.inner.registry.lock() {
                            registry.insert(stored);
                        }
                        view
                    })
                    .collect::<Vec<_>>();
                WallpaperSearchView {
                    phase: WallpaperSearchPhase::Ready,
                    source,
                    query,
                    message: if views.is_empty() {
                        "No wallpapers matched that search.".into()
                    } else {
                        format!("{} result(s)", views.len())
                    },
                    candidates: views,
                }
            }
            Err(WallpaperSearchError::NotConfigured(message)) => WallpaperSearchView {
                phase: WallpaperSearchPhase::NotConfigured,
                source,
                query,
                message,
                candidates: Vec::new(),
            },
            Err(other) => WallpaperSearchView {
                phase: WallpaperSearchPhase::Error,
                source,
                query,
                message: other.to_string(),
                candidates: Vec::new(),
            },
        }
    }

    /// Best-effort cover/hero image URL for a title, via the keyless Steam
    /// Store. Used to give a freshly imported local or Wine game artwork with
    /// no API keys required. Returns `None` when nothing matches.
    pub async fn top_artwork_url(&self, query: &str) -> Option<String> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        let candidates = self.search_steam_store(query, 0).await.ok()?;
        candidates.into_iter().next().map(|candidate| candidate.url)
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
        query: &str,
        offset: u32,
    ) -> Result<Vec<WallpaperCandidate>, WallpaperSearchError> {
        let stored = self.stored();
        let client_id = credential_for(&stored.igdb_client_id, IGDB_CLIENT_ID_ENV, &system_env)?;
        let client_secret =
            credential_for(&stored.igdb_client_secret, IGDB_CLIENT_SECRET_ENV, &system_env)?;
        let token = self.igdb_token(&client_id, &client_secret).await?;
        let games = self
            .igdb_query_games(&client_id, &token, query)
            .await?;
        let Some(top) = games.into_iter().next() else {
            return Ok(Vec::new());
        };
        let mut images = self
            .igdb_query_media(&client_id, &token, "artworks", top.id, 20, offset)
            .await?;
        let mut screenshots = self
            .igdb_query_media(&client_id, &token, "screenshots", top.id, 24, offset)
            .await?;
        images.append(&mut screenshots);
        let game_name = if top.name.trim().is_empty() { query } else { top.name.trim() };
        Ok(build_igdb_candidates(&images, game_name)
            .into_iter()
            .map(|(title, thumbnail_url, url)| WallpaperCandidate {
                id: mint_candidate_id(
                    WallpaperSource::Igdb,
                    &url,
                    CANDIDATE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
                ),
                title,
                thumbnail_url,
                url,
            })
            .collect())
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
            return Err(WallpaperSearchError::Network(format!(
                "IGDB rejected the credentials (status {status})."
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
        query: &str,
        offset: u32,
    ) -> Result<Vec<WallpaperCandidate>, WallpaperSearchError> {
        let stored = self.stored();
        let key = credential_for(&stored.google_api_key, GOOGLE_API_KEY_ENV, &system_env)?;
        let cse_id = credential_for(&stored.google_cse_id, GOOGLE_CSE_ID_ENV, &system_env)?;
        let response = self
            .inner
            .http
            .get(GOOGLE_SEARCH_URL)
            .query(&[
                ("key", key.as_str()),
                ("cx", cse_id.as_str()),
                ("q", query),
                ("searchType", "image"),
                ("num", "10"),
                ("start", (offset + 1).to_string().as_str()),
                ("imgSize", "large"),
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
        Ok(parse_google_search(&body)?
            .into_iter()
            .map(|(title, thumbnail_url, url)| WallpaperCandidate {
                id: mint_candidate_id(
                    WallpaperSource::GoogleImages,
                    &url,
                    CANDIDATE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
                ),
                title,
                thumbnail_url,
                url,
            })
            .collect())
    }

    /// The built-in, keyless game-artwork source, mirroring Playnite: Steam's
    /// public store search returns matching apps, then the app details endpoint
    /// yields the header, page background, library hero and screenshots.
    async fn search_steam_store(
        &self,
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
        let apps = parse_steam_store_search(&body)?;
        let mut candidates = Vec::new();
        for (app_id, app_name) in apps {
            let Ok(Some(data)) = self.steam_app_details(app_id).await else {
                continue;
            };
            for (title, thumbnail_url, url) in build_steam_candidates(&data, &app_name) {
                if candidates.len() >= MAX_CANDIDATES + offset as usize {
                    break;
                }
                candidates.push(WallpaperCandidate {
                    id: mint_candidate_id(
                        WallpaperSource::SteamStore,
                        &url,
                        CANDIDATE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
                    ),
                    title,
                    thumbnail_url,
                    url,
                });
            }
        }
        Ok(candidates.into_iter().skip(offset as usize).collect())
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

    /// The built-in, keyless source: a Wikimedia Commons file search. The
    /// query is widened with "wallpaper" so the file namespace returns actual
    /// artwork rather than box art and in-game screenshots alone.
    async fn search_wikimedia(
        &self,
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
                ("gsrsearch", format!("{query} wallpaper")),
                ("gsrnamespace", "6".to_owned()),
                ("gsrlimit", "16".to_owned()),
                ("gsroffset", offset.to_string()),
                ("prop", "imageinfo".to_owned()),
                ("iiprop", "url".to_owned()),
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
        Ok(parse_wikimedia_search(&body)?
            .into_iter()
            .map(|(title, thumbnail_url, url)| WallpaperCandidate {
                id: mint_candidate_id(
                    WallpaperSource::Wikimedia,
                    &url,
                    CANDIDATE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
                ),
                title,
                thumbnail_url,
                url,
            })
            .collect())
    }

    /// The second built-in, keyless source: Openverse's openly licensed image
    /// index. Large results lean toward wallpaper-sized artwork.
    async fn search_openverse(
        &self,
        query: &str,
        offset: u32,
    ) -> Result<Vec<WallpaperCandidate>, WallpaperSearchError> {
        let page = offset / 16 + 1;
        let response = self
            .inner
            .http
            .get(OPENVERSE_API_URL)
            .query(&[
                ("q", format!("{query} wallpaper")),
                ("license_type", "all".to_owned()),
                ("size", "large".to_owned()),
                ("page_size", "16".to_owned()),
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
        Ok(parse_openverse_search(&body)?
            .into_iter()
            .map(|(title, thumbnail_url, url)| WallpaperCandidate {
                id: mint_candidate_id(
                    WallpaperSource::Openverse,
                    &url,
                    CANDIDATE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
                ),
                title,
                thumbnail_url,
                url,
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Search a wallpaper provider. Never fails on a missing credential — it
/// answers with a `not-configured` phase instead, so the page can explain.
#[tauri::command]
pub async fn search_wallpapers(
    source: WallpaperSource,
    query: String,
    offset: u32,
    service: State<'_, WallpaperSearchService>,
) -> Result<WallpaperSearchView, String> {
    Ok(service.search(source, &query, offset).await)
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

    #[test]
    fn credentials_missing_is_not_configured() {
        let never = |_: &str| None;
        assert!(matches!(
            credential_for("", "ORIVO_IGDB_CLIENT_ID", &never),
            Err(WallpaperSearchError::NotConfigured(message))
                if message.contains("ORIVO_IGDB_CLIENT_ID")
        ));
        let blank = |_: &str| Some("   ".to_owned());
        assert!(matches!(
            credential_for("", "ORIVO_GOOGLE_SEARCH_API_KEY", &blank),
            Err(WallpaperSearchError::NotConfigured(_))
        ));
        let present = |_: &str| Some("abc123".to_owned());
        assert_eq!(
            credential_for("", "ORIVO_IGDB_CLIENT_SECRET", &present).unwrap(),
            "abc123"
        );
    }

    #[test]
    fn a_stored_credential_beats_an_environment_variable() {
        let env = |_: &str| Some("from-env".to_owned());
        assert_eq!(
            credential_for("from-settings", "ORIVO_IGDB_CLIENT_ID", &env).unwrap(),
            "from-settings"
        );
        assert_eq!(
            credential_for("  spaced  ", "ORIVO_IGDB_CLIENT_ID", &env).unwrap(),
            "spaced"
        );
        // An empty stored value falls through to the environment.
        assert_eq!(
            credential_for("   ", "ORIVO_IGDB_CLIENT_ID", &env).unwrap(),
            "from-env"
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

    #[test]
    fn normalises_igdb_and_google_urls_to_https() {
        assert_eq!(
            https_url("//images.igdb.com/igdb/image/upload/t_thumb/a.jpg"),
            "https://images.igdb.com/igdb/image/upload/t_thumb/a.jpg"
        );
        assert_eq!(
            https_url("https://lh3.googleusercontent.com/a"),
            "https://lh3.googleusercontent.com/a"
        );
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

    #[test]
    fn parses_igdb_artwork_and_builds_candidates() {
        let images = parse_igdb_images(
            r#"[{"id":1,"url":"//images.igdb.com/igdb/image/upload/t_thumb/a.jpg"},{"id":2}]"#,
        )
        .unwrap();
        let built = build_igdb_candidates(&images, "Elden Ring");
        assert_eq!(built.len(), 1);
        let (title, thumbnail, full) = &built[0];
        assert_eq!(title, "Elden Ring — artwork 1");
        assert!(thumbnail.ends_with("/t_screenshot_big/a.jpg"));
        assert!(full.ends_with("/t_1080p/a.jpg"));
    }

    #[test]
    fn parses_google_search_results() {
        let built = parse_google_search(
            r#"{"items":[{"title":"Elden Ring Wallpaper","link":"https://wall.example/full.jpg","image":{"thumbnailLink":"https://encrypted-tbn0.gstatic.com/thumb.jpg"}},{"title":"No image","link":"https://wall.example/other.jpg"}]}"#,
        )
        .unwrap();
        assert_eq!(built.len(), 2);
        let (title, thumbnail, full) = &built[0];
        assert_eq!(title, "Elden Ring Wallpaper");
        assert_eq!(thumbnail, "https://encrypted-tbn0.gstatic.com/thumb.jpg");
        assert_eq!(full, "https://wall.example/full.jpg");
        // Without a thumbnail link the full image is used as its own preview.
        let (_, thumbnail, full) = &built[1];
        assert_eq!(thumbnail, full);
    }

    #[test]
    fn parses_wikimedia_commons_file_results() {
        let built = parse_wikimedia_search(
            r#"{"query":{"pages":{"1":{"title":"File:Elden Ring art.jpg","imageinfo":[{"url":"//upload.wikimedia.org/full.jpg","thumburl":"//upload.wikimedia.org/thumb.jpg"}]},"2":{"title":"File:No_media.png","imageinfo":[]},"3":{"title":"File:Only full.png","imageinfo":[{"url":"//upload.wikimedia.org/full-only.png","thumburl":""}]}}}}"#,
        )
        .unwrap();
        assert_eq!(built.len(), 2);
        let (title, thumbnail, full) = &built[0];
        assert_eq!(title, "Elden Ring art");
        assert_eq!(thumbnail, "https://upload.wikimedia.org/thumb.jpg");
        assert_eq!(full, "https://upload.wikimedia.org/full.jpg");
        let (title, thumbnail, full) = &built[1];
        assert_eq!(title, "Only full");
        assert_eq!(thumbnail, full);
    }

    #[test]
    fn parses_openverse_results() {
        let built = parse_openverse_search(
            r#"{"results":[{"id":"a1","title":"Elden Ring Wallpaper","url":"https://wall.example/full.jpg","thumbnail":"https://wall.example/thumb.jpg"},{"id":"a2","title":"No thumb","url":"https://wall.example/other.jpg","thumbnail":null},{"id":"a3","title":"","url":"https://wall.example/untitled.jpg","thumbnail":null}]}"#,
        )
        .unwrap();
        assert_eq!(built.len(), 3);
        let (title, thumbnail, full) = &built[0];
        assert_eq!(title, "Elden Ring Wallpaper");
        assert_eq!(thumbnail, "https://wall.example/thumb.jpg");
        assert_eq!(full, "https://wall.example/full.jpg");
        let (title, thumbnail, full) = &built[1];
        assert_eq!(title, "No thumb");
        assert_eq!(thumbnail, full);
        let (title, _, _) = &built[2];
        assert_eq!(title, "Openverse image");
    }

    #[test]
    fn parses_steam_store_search_apps() {
        let apps = parse_steam_store_search(
            r#"{"items":[{"type":"app","id":1245620,"name":"ELDEN RING"},{"type":"sub","id":999,"name":"A package"},{"type":"app","id":0,"name":""},{"type":"app","id":3,"name":"Hollow Knight"}]}"#,
        )
        .unwrap();
        assert_eq!(apps, vec![(1245620, "ELDEN RING".to_owned()), (3, "Hollow Knight".to_owned())]);
    }

    #[test]
    fn parses_steam_app_details_and_builds_candidates() {
        let data = parse_steam_app_details(
            r#"{"1245620":{"success":true,"data":{"name":"Elden Ring","header_image":"https://shared.akamai.steamstatic.com/header.jpg","background":"https://store.akamai.steamstatic.com/bg","library_hero":"https://shared.akamai.steamstatic.com/hero.jpg","screenshots":[{"path_full":"https://shared.akamai.steamstatic.com/ss.1920x1080.jpg","path_thumbnail":"https://shared.akamai.steamstatic.com/ss.600x338.jpg"},{"path_full":"","path_thumbnail":""}]}},"0":{"success":false,"data":null}}"#,
        )
        .unwrap()
        .unwrap();
        let built = build_steam_candidates(&data, "Elden Ring");
        assert_eq!(built.len(), 4);
        let (title, thumbnail, full) = &built[0];
        assert_eq!(title, "Elden Ring — header");
        assert_eq!(thumbnail, "https://shared.akamai.steamstatic.com/header.jpg");
        let (title, _, _) = &built[1];
        assert_eq!(title, "Elden Ring — store background");
        let (title, _, _) = &built[2];
        assert_eq!(title, "Elden Ring — library hero");
        let (title, thumbnail, full) = &built[3];
        assert_eq!(title, "Elden Ring — screenshot 1");
        assert_eq!(thumbnail, "https://shared.akamai.steamstatic.com/ss.600x338.jpg");
        assert_eq!(full, "https://shared.akamai.steamstatic.com/ss.1920x1080.jpg");
        // A failed lookup yields nothing rather than an error.
        assert_eq!(parse_steam_app_details(r#"{"999":{"success":false}}"#).unwrap(), None);
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
        let first = mint_candidate_id(WallpaperSource::Igdb, "https://a.example/x.jpg", 1);
        let second = mint_candidate_id(WallpaperSource::Igdb, "https://a.example/x.jpg", 2);
        let third = mint_candidate_id(WallpaperSource::GoogleImages, "https://a.example/x.jpg", 2);
        assert!(first.starts_with("wp:"));
        assert!(first != second);
        assert!(second != third);
        for id in [&first, &second, &third] {
            assert!(validate_opaque_id("candidate id", id).is_ok());
        }
    }
}
