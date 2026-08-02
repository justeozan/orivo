//! Game media pipeline: preview, apply, export, import and download control.
//!
//! Owned by the Media agent. Commands are registered by the shell in `lib.rs`.
//!
//! Every remote fetch lives here, in Rust. The WebView can only name an opaque
//! game id, an opaque media id, and a closed media kind: it can never supply a
//! URL, a filesystem path, a MIME type, or a command. Downloads are limited by
//! a hardcoded host allowlist, a bounded redirect chain that is re-validated at
//! every hop, declared-MIME *and* magic-byte agreement, per-kind size caps that
//! are enforced while streaming, a durable cache quota, and a small concurrency
//! budget with in-flight deduplication. Selection is committed in a single
//! atomic state mutation, so a failed apply always leaves the previous
//! selection in place.

use crate::game_detail::{
    GameDetailError, GameDetailService, GameMediaAsset, GameMediaKind, GameMediaOrigin,
    GameMediaView, valid_opaque_file_name, validate_opaque_id,
};
use futures_util::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    future::Future,
    io::{Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};
use tauri::State;
use tokio::sync::Semaphore;

/// Media lives in its own `$APPDATA` subdirectory so the asset protocol scope,
/// the quota, and the cache sweep all describe exactly one flat directory.
pub const MEDIA_DIRECTORY: &str = "game-media";

/// The URI scheme `GameMediaAsset::view` mints for offline media. The shell
/// registers it against `MEDIA_DIRECTORY`; the WebView never sees a real path.
pub const GAME_MEDIA_URI_SCHEME: &str = "game-media";

pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_VIDEO_BYTES: u64 = 250 * 1024 * 1024;
pub const MEDIA_QUOTA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_REDIRECTS: usize = 3;
pub const MAX_CONCURRENT_DOWNLOADS: usize = 2;

const MAX_MEDIA_URL_BYTES: usize = 2048;
const MAGIC_PROBE_BYTES: usize = 16;
const MAGIC_MINIMUM_BYTES: usize = 12;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const INFLIGHT_POLL: Duration = Duration::from_millis(5);
const INFLIGHT_DEADLINE: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// The only hosts Orivo will ever fetch media from. Every hop of a redirect
/// chain is re-checked against this list, so a 302 cannot walk the fetch onto
/// an arbitrary origin.
pub const ALLOWED_MEDIA_HOSTS: [&str; 7] = [
    "cdn.cloudflare.steamstatic.com",
    "cdn.akamai.steamstatic.com",
    "shared.akamai.steamstatic.com",
    "shared.cloudflare.steamstatic.com",
    "steamcdn-a.akamaihd.net",
    "media.steampowered.com",
    "video.akamai.steamstatic.com",
];

/// Which host policy a download ran under. This matters for redirects: every
/// hop is re-validated against the same policy that admitted the first one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPolicy {
    /// The Steam catalog allowlist. Used by every pipeline download.
    Steam,
    /// Wallpaper imports from search results. Google Images legitimately
    /// points at the image's origin site, so no fixed allowlist can cover it;
    /// the host must instead be a *public domain name* (never an IP literal or
    /// a special-use pseudo-domain) and every other guard — https only, magic
    /// bytes, size caps, quota — still applies. The WebView can never name
    /// this URL; it only ever names an opaque candidate id.
    SearchResult,
}

/// `SearchResult` hosts must look like real public domains: several labels of
/// alphanumerics and hyphens, at least one non-numeric label (which rules out
/// IPv4 literals), and a TLD that is not a special-use pseudo-domain.
fn is_public_domain_host(host: &str) -> bool {
    let labels = host.split('.').collect::<Vec<_>>();
    if labels.len() < 2 {
        return false;
    }
    let mut has_named_label = false;
    for label in &labels {
        if label.is_empty() || !label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return false;
        }
        if !label.bytes().all(|byte| byte.is_ascii_digit()) {
            has_named_label = true;
        }
    }
    if !has_named_label {
        return false;
    }
    let tld = labels
        .last()
        .unwrap_or(&"")
        .to_ascii_lowercase();
    !matches!(
        tld.as_str(),
        "local" | "localhost" | "internal" | "invalid" | "test" | "home" | "lan" | "arpa"
    )
}

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Deliberately message-shaped and `Clone`: a deduplicated follower receives a
/// copy of the leader's outcome, and no variant can carry a host path or an
/// internal URL into the WebView.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameMediaError {
    Invalid(String),
    NotFound,
    Unsupported(String),
    TooLarge,
    QuotaExceeded,
    Cancelled,
    Network(String),
    Storage(String),
    Busy,
}

impl std::fmt::Display for GameMediaError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "This media request is invalid: {message}"),
            Self::NotFound => formatter.write_str("This artwork is no longer available."),
            Self::Unsupported(message) => {
                write!(formatter, "This file is not supported: {message}")
            }
            Self::TooLarge => {
                formatter.write_str("This artwork is too large for Orivo to store safely.")
            }
            Self::QuotaExceeded => formatter.write_str(
                "Orivo's artwork storage is full. Remove some saved artwork and try again.",
            ),
            Self::Cancelled => formatter.write_str("This artwork download was cancelled."),
            Self::Network(message) => write!(formatter, "The download did not finish: {message}"),
            Self::Storage(message) => write!(formatter, "Artwork could not be saved: {message}"),
            Self::Busy => {
                formatter.write_str("Orivo is already downloading artwork. Try again shortly.")
            }
        }
    }
}

impl std::error::Error for GameMediaError {}

impl From<GameDetailError> for GameMediaError {
    fn from(error: GameDetailError) -> Self {
        match error {
            GameDetailError::NotFound => Self::NotFound,
            GameDetailError::Invalid(message) => Self::Invalid(message),
            // `GameDetailError`'s own `Display` is already path-free.
            other => Self::Storage(other.to_string()),
        }
    }
}

impl From<std::io::Error> for GameMediaError {
    fn from(error: std::io::Error) -> Self {
        Self::Storage(error.kind().to_string())
    }
}

// ---------------------------------------------------------------------------
// URL policy
// ---------------------------------------------------------------------------

/// A URL that has already passed the scheme, host allowlist and shape checks.
/// Nothing else in this module accepts a bare string, so an unvalidated origin
/// cannot reach the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMediaUrl {
    url: String,
    host: String,
}

impl RemoteMediaUrl {
    pub fn as_str(&self) -> &str {
        &self.url
    }

    pub fn host(&self) -> &str {
        &self.host
    }
}

/// Parse without a URL crate so the rules stay explicit: https only, no
/// credentials, no alternate port, a host admitted by the policy, and a
/// printable ASCII path with no traversal.
pub fn validate_remote_media_url(raw: &str) -> Result<RemoteMediaUrl, GameMediaError> {
    validate_download_url(raw, DownloadPolicy::Steam)
}

/// The same validator under an explicit download policy. `SearchResult` swaps
/// the Steam allowlist for a public-domain-name check; everything else is
/// shared.
pub fn validate_download_url(
    raw: &str,
    policy: DownloadPolicy,
) -> Result<RemoteMediaUrl, GameMediaError> {
    if raw.is_empty() || raw.len() > MAX_MEDIA_URL_BYTES {
        return Err(GameMediaError::Invalid(
            "media url has an unusable length".into(),
        ));
    }
    if !raw.is_ascii() || raw.bytes().any(|byte| byte <= 0x20 || byte == 0x7f) {
        return Err(GameMediaError::Invalid(
            "media url contains unsupported characters".into(),
        ));
    }
    let Some(remainder) = strip_https_scheme(raw) else {
        return Err(GameMediaError::Invalid("media url must use https".into()));
    };
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let path = &remainder[authority_end..];
    if authority.is_empty() || authority.contains('@') {
        return Err(GameMediaError::Invalid(
            "media url has an unusable host".into(),
        ));
    }
    let host = match authority.split_once(':') {
        Some((host, "443")) => host,
        Some(_) => {
            return Err(GameMediaError::Invalid(
                "media url may not select a custom port".into(),
            ));
        }
        None => authority,
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let allowed = match policy {
        DownloadPolicy::Steam => ALLOWED_MEDIA_HOSTS.contains(&host.as_str()),
        DownloadPolicy::SearchResult => is_public_domain_host(&host),
    };
    if !allowed {
        return Err(GameMediaError::Invalid(
            "media url host is not allowed".into(),
        ));
    }
    if path.contains("..") || path.contains('\\') {
        return Err(GameMediaError::Invalid(
            "media url path is not allowed".into(),
        ));
    }
    Ok(RemoteMediaUrl {
        url: format!("https://{host}{path}"),
        host,
    })
}

fn strip_https_scheme(raw: &str) -> Option<&str> {
    let (scheme, remainder) = raw.split_once("://")?;
    scheme.eq_ignore_ascii_case("https").then_some(remainder)
}

/// Resolve a `Location` header against the hop that produced it. Only absolute
/// https targets and absolute paths are accepted, and the result goes back
/// through the full allowlist check.
pub fn resolve_redirect(
    base: &RemoteMediaUrl,
    location: &str,
) -> Result<RemoteMediaUrl, GameMediaError> {
    resolve_redirect_with_policy(base, location, DownloadPolicy::Steam)
}

/// Redirect resolution that honours the policy the download started under, so
/// a search-result wallpaper is never walked onto a Steam-only validation.
pub fn resolve_redirect_with_policy(
    base: &RemoteMediaUrl,
    location: &str,
    policy: DownloadPolicy,
) -> Result<RemoteMediaUrl, GameMediaError> {
    let location = location.trim();
    if location.is_empty() {
        return Err(GameMediaError::Invalid("redirect has no target".into()));
    }
    if let Some(rest) = location.strip_prefix("//") {
        return validate_download_url(&format!("https://{rest}"), policy);
    }
    if location.starts_with('/') {
        return validate_download_url(&format!("https://{}{location}", base.host()), policy);
    }
    validate_download_url(location, policy)
}

/// Bundled artwork shipped inside the app. It is already offline, so applying
/// it never touches the network.
fn is_bundled_media_reference(value: &str) -> bool {
    value.starts_with("/media/")
        && !value.contains("..")
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
}

// ---------------------------------------------------------------------------
// Content policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaFormat {
    Jpeg,
    Png,
    Webp,
    Gif,
    Mp4,
}

impl MediaFormat {
    /// Sniff the leading bytes. A declared MIME alone is never trusted.
    pub fn from_magic(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < MAGIC_MINIMUM_BYTES {
            return None;
        }
        if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            return Some(Self::Jpeg);
        }
        if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
            return Some(Self::Png);
        }
        if bytes.starts_with(b"RIFF") && bytes[8..12] == *b"WEBP" {
            return Some(Self::Webp);
        }
        if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            return Some(Self::Gif);
        }
        if bytes[4..8] == *b"ftyp" {
            return Some(Self::Mp4);
        }
        None
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
            Self::Gif => "image/gif",
            Self::Mp4 => "video/mp4",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Gif => "gif",
            Self::Mp4 => "mp4",
        }
    }

    fn is_video(self) -> bool {
        self == Self::Mp4
    }

    /// The declared type must name the same format the bytes actually are.
    pub fn matches_declared(self, declared: &str) -> bool {
        let declared = declared
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match self {
            Self::Jpeg => declared == "image/jpeg" || declared == "image/jpg",
            Self::Png => declared == "image/png",
            Self::Webp => declared == "image/webp",
            Self::Gif => declared == "image/gif",
            Self::Mp4 => declared == "video/mp4",
        }
    }

    /// Video slots take MP4 only; every picture slot takes images only.
    pub fn permits(self, kind: GameMediaKind) -> bool {
        match kind {
            GameMediaKind::Video => self.is_video(),
            GameMediaKind::Wallpaper | GameMediaKind::Icon | GameMediaKind::Cover => {
                !self.is_video()
            }
        }
    }

    fn from_extension(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "webp" => Some(Self::Webp),
            "gif" => Some(Self::Gif),
            "mp4" => Some(Self::Mp4),
            _ => None,
        }
    }
}

fn kind_size_cap(kind: GameMediaKind, limits: &MediaLimits) -> u64 {
    match kind {
        GameMediaKind::Video => limits.max_video_bytes,
        _ => limits.max_image_bytes,
    }
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

pub type MediaByteStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, GameMediaError>> + Send>>;
pub type MediaFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One HTTP GET with redirects *not* followed. Following is this module's job,
/// so every hop can be re-validated before another request goes out.
pub struct MediaResponse {
    pub status: u16,
    pub location: Option<String>,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    pub body: MediaByteStream,
}

impl std::fmt::Debug for MediaResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MediaResponse")
            .field("status", &self.status)
            .field("content_type", &self.content_type)
            .field("content_length", &self.content_length)
            .finish_non_exhaustive()
    }
}

pub trait MediaTransport: Send + Sync + 'static {
    fn get<'a>(
        &'a self,
        url: &'a RemoteMediaUrl,
    ) -> MediaFuture<'a, Result<MediaResponse, GameMediaError>>;
}

/// The production transport. `Policy::none` is essential: reqwest must not
/// follow a redirect on its own, or an off-allowlist hop would never be seen.
pub struct HttpMediaTransport {
    client: reqwest::Client,
}

impl HttpMediaTransport {
    pub fn new() -> Result<Self, GameMediaError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .https_only(true)
            .build()
            .map_err(|_| GameMediaError::Network("the downloader could not start".into()))?;
        Ok(Self { client })
    }
}

impl MediaTransport for HttpMediaTransport {
    fn get<'a>(
        &'a self,
        url: &'a RemoteMediaUrl,
    ) -> MediaFuture<'a, Result<MediaResponse, GameMediaError>> {
        Box::pin(async move {
            let response = self
                .client
                .get(url.as_str())
                .send()
                .await
                .map_err(|_| GameMediaError::Network("the host could not be reached".into()))?;
            let status = response.status().as_u16();
            let header = |name: reqwest::header::HeaderName| {
                response
                    .headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned)
            };
            let location = header(reqwest::header::LOCATION);
            let content_type = header(reqwest::header::CONTENT_TYPE);
            let content_length = response.content_length();
            let body = response.bytes_stream().map(|chunk| {
                chunk
                    .map(|bytes| bytes.to_vec())
                    .map_err(|_| GameMediaError::Network("the transfer was interrupted".into()))
            });
            Ok(MediaResponse {
                status,
                location,
                content_type,
                content_length,
                body: Box::pin(body),
            })
        })
    }
}

// ---------------------------------------------------------------------------
// Native pickers
// ---------------------------------------------------------------------------

/// Injected so import/export can be tested without a desktop session. The
/// WebView never supplies either path; only the native dialog does.
pub trait MediaFilePicker: Send + Sync + 'static {
    fn choose_import(&self, kind: GameMediaKind) -> Option<PathBuf>;
    fn choose_export(&self, suggested_file_name: &str) -> Option<PathBuf>;
}

#[derive(Debug, Default)]
pub struct NativeFilePicker;

impl MediaFilePicker for NativeFilePicker {
    fn choose_import(&self, kind: GameMediaKind) -> Option<PathBuf> {
        let dialog = rfd::FileDialog::new().set_title(match kind {
            GameMediaKind::Video => "Choose a video for this game",
            GameMediaKind::Icon => "Choose an icon for this game",
            GameMediaKind::Cover => "Choose a cover for this game",
            GameMediaKind::Wallpaper => "Choose a wallpaper for this game",
        });
        let dialog = match kind {
            GameMediaKind::Video => dialog.add_filter("Video", &["mp4"]),
            _ => dialog.add_filter("Image", &["jpg", "jpeg", "png", "webp", "gif"]),
        };
        dialog.pick_file()
    }

    fn choose_export(&self, suggested_file_name: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Export this artwork")
            .set_file_name(suggested_file_name)
            .save_file()
    }
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct MediaLimits {
    pub max_image_bytes: u64,
    pub max_video_bytes: u64,
    pub quota_bytes: u64,
    pub max_redirects: usize,
    pub max_concurrent_downloads: usize,
}

impl Default for MediaLimits {
    fn default() -> Self {
        Self {
            max_image_bytes: MAX_IMAGE_BYTES,
            max_video_bytes: MAX_VIDEO_BYTES,
            quota_bytes: MEDIA_QUOTA_BYTES,
            max_redirects: MAX_REDIRECTS,
            max_concurrent_downloads: MAX_CONCURRENT_DOWNLOADS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMedia {
    pub file_name: String,
    pub mime_type: String,
    pub byte_size: u64,
}

#[derive(Debug, Default)]
struct InFlightDownload {
    outcome: Mutex<Option<Result<StoredMedia, GameMediaError>>>,
}

/// Both maps are keyed by the same `<game>\u{1f}<media>` download key. A cancel
/// flag that outlived its download would silently cancel the next request for
/// the same game, so a flag is created with its download and dies with it.
#[derive(Debug, Default)]
struct DownloadRegistry {
    inflight: BTreeMap<String, Arc<InFlightDownload>>,
    cancels: BTreeMap<String, Arc<AtomicBool>>,
    /// Bytes promised to downloads that are already streaming. Without this,
    /// two concurrent transfers both measure the same free space and the quota
    /// can be overshot by a whole file.
    reserved_bytes: u64,
}

struct MediaServiceInner {
    detail: Arc<GameDetailService>,
    root: PathBuf,
    transport: Arc<dyn MediaTransport>,
    picker: Arc<dyn MediaFilePicker>,
    limits: MediaLimits,
    downloads: Mutex<DownloadRegistry>,
    permits: Semaphore,
}

/// Tauri-managed state. Cloning shares one registry, so the concurrency budget
/// and the in-flight table are global rather than per-command.
#[derive(Clone)]
pub struct GameMediaService {
    inner: Arc<MediaServiceInner>,
}

impl std::fmt::Debug for GameMediaService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GameMediaService")
            .field("limits", &self.inner.limits)
            .finish_non_exhaustive()
    }
}

impl GameMediaService {
    /// `media_root` must be the app-owned `$APPDATA/game-media` directory. It
    /// is created here so the asset scope always exists before first use.
    pub fn new(
        detail: Arc<GameDetailService>,
        media_root: PathBuf,
    ) -> Result<Self, GameMediaError> {
        Self::with_parts(
            detail,
            media_root,
            Arc::new(HttpMediaTransport::new()?),
            Arc::new(NativeFilePicker),
            MediaLimits::default(),
        )
    }

    pub fn with_parts(
        detail: Arc<GameDetailService>,
        media_root: PathBuf,
        transport: Arc<dyn MediaTransport>,
        picker: Arc<dyn MediaFilePicker>,
        limits: MediaLimits,
    ) -> Result<Self, GameMediaError> {
        fs::create_dir_all(&media_root)?;
        let permits = Semaphore::new(limits.max_concurrent_downloads.max(1));
        Ok(Self {
            inner: Arc::new(MediaServiceInner {
                detail,
                root: media_root,
                transport,
                picker,
                limits,
                downloads: Mutex::new(DownloadRegistry::default()),
                permits,
            }),
        })
    }

    pub fn media_root(&self) -> &Path {
        &self.inner.root
    }

    pub fn limits(&self) -> MediaLimits {
        self.inner.limits
    }

    /// Apply. Downloads only when the chosen media is not already offline, then
    /// commits registration and selection in one validated state mutation.
    pub async fn apply(
        &self,
        game_id: &str,
        media_id: &str,
    ) -> Result<Vec<GameMediaView>, GameMediaError> {
        validate_opaque_id("game id", game_id)?;
        validate_opaque_id("media id", media_id)?;
        let asset = self
            .inner
            .detail
            .media_asset(game_id, media_id)?
            .ok_or(GameMediaError::NotFound)?;
        asset.validate()?;

        let asset = match (asset.local_file.as_deref(), asset.source_url.as_deref()) {
            (Some(file), _) => {
                if !valid_opaque_file_name(file) || !is_regular_file(&self.inner.root.join(file)) {
                    return Err(GameMediaError::NotFound);
                }
                asset
            }
            (None, Some(source)) if is_bundled_media_reference(source) => asset,
            (None, Some(source)) => {
                let url = validate_remote_media_url(source)?;
                let stored = self
                    .download(game_id, media_id, url, asset.kind, DownloadPolicy::Steam)
                    .await?;
                GameMediaAsset {
                    origin: GameMediaOrigin::Downloaded,
                    local_file: Some(stored.file_name),
                    mime_type: Some(stored.mime_type),
                    byte_size: stored.byte_size,
                    ..asset
                }
            }
            (None, None) => return Err(GameMediaError::NotFound),
        };

        // One mutation: it either lands with the new selection or leaves the
        // previous selection untouched on disk and in memory.
        self.inner
            .detail
            .state()
            .register_and_select_media(game_id, asset)?;
        Ok(self.inner.detail.media_views(game_id)?)
    }

    /// Cancelling only affects downloads that are already running; it can never
    /// pre-arm a cancellation for a future request. Every running transfer for
    /// the game is flagged, and each flag is discarded with its own download.
    pub fn cancel(&self, game_id: &str) -> Result<(), GameMediaError> {
        validate_opaque_id("game id", game_id)?;
        let registry = self.inner.lock_downloads()?;
        let prefix = download_key_prefix(game_id);
        for (_, flag) in registry
            .cancels
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
        {
            flag.store(true, Ordering::Release);
        }
        Ok(())
    }

    /// Export writes a copy through the native save dialog. A dismissed dialog
    /// is a silent success: the user did not fail, they simply declined.
    pub fn export(&self, game_id: &str, media_id: &str) -> Result<(), GameMediaError> {
        validate_opaque_id("game id", game_id)?;
        validate_opaque_id("media id", media_id)?;
        let asset = self
            .inner
            .detail
            .media_asset(game_id, media_id)?
            .ok_or(GameMediaError::NotFound)?;
        let file_name = asset.local_file.as_deref().ok_or_else(|| {
            GameMediaError::Invalid("this artwork is not saved offline yet".into())
        })?;
        if !valid_opaque_file_name(file_name) {
            return Err(GameMediaError::Invalid(
                "artwork reference is not opaque".into(),
            ));
        }
        let source = self.inner.root.join(file_name);
        if !is_regular_file(&source) {
            return Err(GameMediaError::NotFound);
        }
        let extension = Path::new(file_name)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("bin");
        let suggested = format!("{}.{extension}", suggested_file_stem(&asset.title));

        let Some(destination) = self.inner.picker.choose_export(&suggested) else {
            return Ok(());
        };
        if destination.is_dir() {
            return Err(GameMediaError::Invalid(
                "choose a file name for this artwork".into(),
            ));
        }
        let parent = destination
            .parent()
            .filter(|parent| parent.is_dir())
            .ok_or_else(|| GameMediaError::Invalid("choose a writable folder".into()))?;
        let staging = parent.join(format!(
            ".orivo-export-{}-{}.part",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let outcome = copy_file_atomically(&source, &staging, &destination);
        if outcome.is_err() {
            let _ = fs::remove_file(&staging);
        }
        outcome
    }

    /// Import validates the picked file by magic bytes before it is copied into
    /// the app-owned directory, and registers it as durable user data.
    pub fn import(
        &self,
        game_id: &str,
        kind: GameMediaKind,
    ) -> Result<Vec<GameMediaView>, GameMediaError> {
        validate_opaque_id("game id", game_id)?;
        if !self.inner.detail.contains(game_id)? {
            return Err(GameMediaError::NotFound);
        }
        let Some(selected) = self.inner.picker.choose_import(kind) else {
            return Ok(self.inner.detail.media_views(game_id)?);
        };
        let metadata = fs::symlink_metadata(&selected)
            .map_err(|_| GameMediaError::Invalid("that file could not be read".into()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(GameMediaError::Invalid(
                "choose a regular image or video file".into(),
            ));
        }
        let cap = kind_size_cap(kind, &self.inner.limits);
        if metadata.len() == 0 {
            return Err(GameMediaError::Invalid("that file is empty".into()));
        }
        if metadata.len() > cap {
            return Err(GameMediaError::TooLarge);
        }
        let declared = selected
            .extension()
            .and_then(|value| value.to_str())
            .and_then(MediaFormat::from_extension)
            .map(|format| format.mime().to_owned());
        let reservation = self.inner.reserve_budget(metadata.len())?;
        let budget = reservation.budget();

        let mut file = fs::File::open(&selected)
            .map_err(|_| GameMediaError::Invalid("that file could not be read".into()))?;
        let mut staging = MediaStaging::begin(&self.inner.root, kind, declared, cap, budget)?;
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|_| GameMediaError::Storage("the file could not be read".into()))?;
            if read == 0 {
                break;
            }
            staging.push(&buffer[..read])?;
        }
        let stored = staging.finish()?;

        let asset = GameMediaAsset {
            id: imported_media_id(&stored.file_name),
            kind,
            title: imported_media_title(kind),
            source_url: None,
            poster_url: None,
            origin: GameMediaOrigin::Imported,
            local_file: Some(stored.file_name),
            mime_type: Some(stored.mime_type),
            byte_size: stored.byte_size,
            extra: BTreeMap::new(),
        };
        self.inner.detail.state().register_media(game_id, asset)?;
        Ok(self.inner.detail.media_views(game_id)?)
    }

    /// Evict the least recently modified cache files until the quota has room.
    /// Selected and imported media are never candidates.
    pub fn prune_cache(&self, needed: u64) -> Result<u64, GameMediaError> {
        self.inner.prune(needed)
    }

    /// User-initiated wallpaper import from a search result. The `url` is
    /// produced by the host's own search adapters (never by the WebView) and
    /// still has to pass the full `SearchResult` policy plus magic-byte and
    /// size validation before it is registered as durable wallpaper media. The
    /// import *replaces* the current wallpaper: "Use this wallpaper" downloads
    /// and applies the candidate in one step.
    pub async fn download_wallpaper(
        &self,
        game_id: &str,
        url: &str,
        title: &str,
    ) -> Result<Vec<GameMediaView>, GameMediaError> {
        validate_opaque_id("game id", game_id)?;
        if !self.inner.detail.contains(game_id)? {
            return Err(GameMediaError::NotFound);
        }
        let url = validate_download_url(url, DownloadPolicy::SearchResult)?;
        // The registry key needs a stable, opaque media id so an identical
        // in-flight fetch is joined rather than repeated.
        let synthetic_media_id = wallpaper_download_key(url.as_str());
        let stored = self
            .download(
                game_id,
                &synthetic_media_id,
                url,
                GameMediaKind::Wallpaper,
                DownloadPolicy::SearchResult,
            )
            .await?;
        let asset = GameMediaAsset {
            id: downloaded_media_id(&stored.file_name),
            kind: GameMediaKind::Wallpaper,
            title: sanitize_media_title(title),
            source_url: None,
            poster_url: None,
            origin: GameMediaOrigin::Downloaded,
            local_file: Some(stored.file_name),
            mime_type: Some(stored.mime_type),
            byte_size: stored.byte_size,
            extra: BTreeMap::new(),
        };
        self.inner
            .detail
            .state()
            .register_and_select_media(game_id, asset)?;
        Ok(self.inner.detail.media_views(game_id)?)
    }

    async fn download(
        &self,
        game_id: &str,
        media_id: &str,
        url: RemoteMediaUrl,
        kind: GameMediaKind,
        policy: DownloadPolicy,
    ) -> Result<StoredMedia, GameMediaError> {
        let key = format!("{}{media_id}", download_key_prefix(game_id));
        let cancel = Arc::new(AtomicBool::new(false));
        let follower = {
            let mut registry = self.inner.lock_downloads()?;
            match registry.inflight.get(&key) {
                Some(existing) => Some(Arc::clone(existing)),
                None => {
                    registry
                        .inflight
                        .insert(key.clone(), Arc::new(InFlightDownload::default()));
                    registry.cancels.insert(key.clone(), Arc::clone(&cancel));
                    None
                }
            }
        };
        // An identical request already running is joined rather than repeated.
        if let Some(leader) = follower {
            return await_inflight(leader).await;
        }

        // From here the leader owns the in-flight entry. The guard publishes an
        // outcome even if this future is dropped mid-transfer, so followers are
        // never left spinning on an entry nobody will ever complete.
        let lease = InFlightLease::new(Arc::clone(&self.inner), key);
        let outcome = self.run_download(url, kind, &cancel, policy).await;
        lease.release(outcome.clone())?;
        outcome
    }

    async fn run_download(
        &self,
        url: RemoteMediaUrl,
        kind: GameMediaKind,
        cancel: &AtomicBool,
        policy: DownloadPolicy,
    ) -> Result<StoredMedia, GameMediaError> {
        let _permit = self
            .inner
            .permits
            .acquire()
            .await
            .map_err(|_| GameMediaError::Busy)?;
        if cancel.load(Ordering::Acquire) {
            return Err(GameMediaError::Cancelled);
        }
        let cap = kind_size_cap(kind, &self.inner.limits);
        // Held for the whole transfer: a concurrent download sees these bytes
        // as already spent instead of measuring the same free space twice.
        let reservation = self
            .inner
            .reserve_budget(cap.min(self.inner.limits.quota_bytes))?;
        let budget = reservation.budget();

        let mut current = url;
        let mut redirects = 0_usize;
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(GameMediaError::Cancelled);
            }
            let response = self.inner.transport.get(&current).await?;
            match response.status {
                200 => {
                    return self
                        .stream_to_disk(response, kind, cap, budget, cancel)
                        .await;
                }
                301 | 302 | 303 | 307 | 308 => {
                    if redirects >= self.inner.limits.max_redirects {
                        return Err(GameMediaError::Invalid(
                            "the download was redirected too many times".into(),
                        ));
                    }
                    redirects += 1;
                    let location = response
                        .location
                        .ok_or_else(|| GameMediaError::Invalid("redirect has no target".into()))?;
                    // Every hop is re-validated under the same policy that
                    // admitted the first URL: https, host policy, safe path.
                    // reqwest is never allowed to follow on its own.
                    current = resolve_redirect_with_policy(&current, &location, policy)?;
                }
                status => {
                    return Err(GameMediaError::Network(format!(
                        "the host returned status {status}"
                    )));
                }
            }
        }
    }

    async fn stream_to_disk(
        &self,
        response: MediaResponse,
        kind: GameMediaKind,
        cap: u64,
        budget: u64,
        cancel: &AtomicBool,
    ) -> Result<StoredMedia, GameMediaError> {
        let declared = response.content_type.clone().ok_or_else(|| {
            GameMediaError::Unsupported("the host did not declare a media type".into())
        })?;
        if let Some(length) = response.content_length {
            if length > cap {
                return Err(GameMediaError::TooLarge);
            }
            if length > budget {
                return Err(GameMediaError::QuotaExceeded);
            }
        }
        let mut staging = MediaStaging::begin(&self.inner.root, kind, Some(declared), cap, budget)?;
        let mut body = response.body;
        // Nothing is buffered in memory: caps trip on the chunk that crosses
        // them and the stream is dropped immediately.
        while let Some(chunk) = body.next().await {
            if cancel.load(Ordering::Acquire) {
                return Err(GameMediaError::Cancelled);
            }
            staging.push(&chunk?)?;
        }
        if cancel.load(Ordering::Acquire) {
            return Err(GameMediaError::Cancelled);
        }
        staging.finish()
    }
}

impl MediaServiceInner {
    fn lock_downloads(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, DownloadRegistry>, GameMediaError> {
        self.downloads
            .lock()
            .map_err(|_| GameMediaError::Storage("the download registry is unavailable".into()))
    }

    fn finish_download(
        &self,
        key: &str,
        outcome: Result<StoredMedia, GameMediaError>,
    ) -> Result<(), GameMediaError> {
        let mut registry = self.lock_downloads()?;
        if let Some(entry) = registry.inflight.remove(key)
            && let Ok(mut slot) = entry.outcome.lock()
        {
            *slot = Some(outcome);
        }
        // The flag was minted for this download alone, so it leaves with it and
        // can never pre-cancel the next request for the same game.
        registry.cancels.remove(key);
        Ok(())
    }

    /// Remaining durable budget after making room for `needed` bytes, with the
    /// bytes already promised to running downloads subtracted. Measuring and
    /// reserving happen under one lock so two transfers cannot both claim the
    /// same free space and overshoot the quota by a whole file.
    fn reserve_budget(self: &Arc<Self>, needed: u64) -> Result<BudgetReservation, GameMediaError> {
        let mut registry = self.lock_downloads()?;
        let usage = self.prune(needed)?;
        let budget = self
            .limits
            .quota_bytes
            .saturating_sub(usage)
            .saturating_sub(registry.reserved_bytes);
        registry.reserved_bytes = registry.reserved_bytes.saturating_add(needed);
        drop(registry);
        Ok(BudgetReservation {
            inner: Arc::clone(self),
            reserved: needed,
            budget,
        })
    }

    /// Returns the resulting usage. Protected files (currently selected media
    /// and every imported file) are removed from the candidate set first, so a
    /// sweep can never delete the artwork a game is using.
    fn prune(&self, needed: u64) -> Result<u64, GameMediaError> {
        let protected = self.detail.state().protected_local_files()?;
        let mut entries = Vec::new();
        let mut usage = 0_u64;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) if metadata.is_file() => metadata,
                _ => continue,
            };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            usage = usage.saturating_add(metadata.len());
            // `.part` files belong to a download that is still running.
            if protected.contains(&name) || name.ends_with(".part") {
                continue;
            }
            entries.push((
                metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                name,
                metadata.len(),
            ));
        }
        if usage.saturating_add(needed) <= self.limits.quota_bytes {
            return Ok(usage);
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        for (_, name, size) in entries {
            if usage.saturating_add(needed) <= self.limits.quota_bytes {
                break;
            }
            if fs::remove_file(self.root.join(&name)).is_ok() {
                usage = usage.saturating_sub(size);
            }
        }
        Ok(usage)
    }
}

/// Ownership of one in-flight entry. A leader whose future is dropped — a
/// cancelled command, a closed window — would otherwise leave the entry behind
/// and hold every later request for that media behind the follower deadline.
struct InFlightLease {
    inner: Arc<MediaServiceInner>,
    key: String,
    released: bool,
}

impl InFlightLease {
    fn new(inner: Arc<MediaServiceInner>, key: String) -> Self {
        Self {
            inner,
            key,
            released: false,
        }
    }

    fn release(
        mut self,
        outcome: Result<StoredMedia, GameMediaError>,
    ) -> Result<(), GameMediaError> {
        self.released = true;
        self.inner.finish_download(&self.key, outcome)
    }
}

impl Drop for InFlightLease {
    fn drop(&mut self) {
        if !self.released {
            // Publishing an outcome releases the followers immediately instead
            // of leaving them to spin until the in-flight deadline.
            let _ = self
                .inner
                .finish_download(&self.key, Err(GameMediaError::Cancelled));
        }
    }
}

/// The quota share one transfer is allowed to consume, returned to the pool as
/// soon as the transfer ends however it ends.
struct BudgetReservation {
    inner: Arc<MediaServiceInner>,
    reserved: u64,
    budget: u64,
}

impl BudgetReservation {
    fn budget(&self) -> u64 {
        self.budget
    }
}

impl Drop for BudgetReservation {
    fn drop(&mut self) {
        if let Ok(mut registry) = self.inner.downloads.lock() {
            registry.reserved_bytes = registry.reserved_bytes.saturating_sub(self.reserved);
        }
    }
}

fn download_key_prefix(game_id: &str) -> String {
    format!("{game_id}\u{1f}")
}

/// A regular file, never a symlink. The media directory only ever holds files
/// this app wrote, so anything else in it is either stale or planted.
fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_file())
}

/// A deduplicated follower polls for the leader's outcome. Polling keeps the
/// wakeup free of notification races; downloads are far slower than the tick.
async fn await_inflight(leader: Arc<InFlightDownload>) -> Result<StoredMedia, GameMediaError> {
    let deadline = std::time::Instant::now() + INFLIGHT_DEADLINE;
    loop {
        if let Ok(slot) = leader.outcome.lock()
            && let Some(outcome) = slot.clone()
        {
            return outcome;
        }
        if std::time::Instant::now() >= deadline {
            return Err(GameMediaError::Busy);
        }
        tokio::time::sleep(INFLIGHT_POLL).await;
    }
}

// ---------------------------------------------------------------------------
// Staging: bounded, validated, content addressed, atomic
// ---------------------------------------------------------------------------

struct StagedFile {
    path: PathBuf,
    file: fs::File,
    committed: bool,
}

impl StagedFile {
    fn create(root: &Path) -> Result<Self, GameMediaError> {
        fs::create_dir_all(root)?;
        let path = root.join(format!(
            "download-{}-{}.part",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        Ok(Self {
            path,
            file,
            committed: false,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), GameMediaError> {
        self.file.write_all(bytes)?;
        Ok(())
    }

    fn commit(mut self, target: &Path) -> Result<(), GameMediaError> {
        self.file.sync_all()?;
        fs::rename(&self.path, target)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if !self.committed {
            // A cancelled, oversized or malformed transfer must leave nothing
            // behind: the visible name only ever appears after `commit`.
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// The shared validation core for both remote downloads and local imports.
struct MediaStaging {
    root: PathBuf,
    kind: GameMediaKind,
    declared: Option<String>,
    cap: u64,
    budget: u64,
    staged: StagedFile,
    header: Vec<u8>,
    format: Option<MediaFormat>,
    total: u64,
    digest: Sha256,
}

impl MediaStaging {
    fn begin(
        root: &Path,
        kind: GameMediaKind,
        declared: Option<String>,
        cap: u64,
        budget: u64,
    ) -> Result<Self, GameMediaError> {
        if budget == 0 {
            return Err(GameMediaError::QuotaExceeded);
        }
        if let Some(declared) = declared.as_deref()
            && !declared_type_is_supported(declared)
        {
            return Err(GameMediaError::Unsupported(
                "that media type is not supported".into(),
            ));
        }
        Ok(Self {
            root: root.to_path_buf(),
            kind,
            declared,
            cap,
            budget,
            staged: StagedFile::create(root)?,
            header: Vec::with_capacity(MAGIC_PROBE_BYTES),
            format: None,
            total: 0,
            digest: Sha256::new(),
        })
    }

    fn push(&mut self, chunk: &[u8]) -> Result<(), GameMediaError> {
        if chunk.is_empty() {
            return Ok(());
        }
        self.total = self.total.saturating_add(chunk.len() as u64);
        if self.total > self.cap {
            return Err(GameMediaError::TooLarge);
        }
        if self.total > self.budget {
            return Err(GameMediaError::QuotaExceeded);
        }
        if self.header.len() < MAGIC_PROBE_BYTES {
            let take = (MAGIC_PROBE_BYTES - self.header.len()).min(chunk.len());
            self.header.extend_from_slice(&chunk[..take]);
        }
        if self.format.is_none() && self.header.len() >= MAGIC_MINIMUM_BYTES {
            self.format = Some(self.resolve_format()?);
        }
        self.digest.update(chunk);
        self.staged.write(chunk)
    }

    /// Magic bytes decide the format; the declared type only gets to agree.
    fn resolve_format(&self) -> Result<MediaFormat, GameMediaError> {
        let format = MediaFormat::from_magic(&self.header).ok_or_else(|| {
            GameMediaError::Unsupported("the file content is not a supported image or video".into())
        })?;
        if let Some(declared) = self.declared.as_deref()
            && !format.matches_declared(declared)
        {
            return Err(GameMediaError::Unsupported(
                "the declared media type does not match the file content".into(),
            ));
        }
        if !format.permits(self.kind) {
            return Err(GameMediaError::Unsupported(
                "that file cannot be used for this artwork slot".into(),
            ));
        }
        Ok(format)
    }

    fn finish(mut self) -> Result<StoredMedia, GameMediaError> {
        if self.total == 0 {
            return Err(GameMediaError::Unsupported("the file is empty".into()));
        }
        let format = match self.format {
            Some(format) => format,
            None => self.resolve_format()?,
        };
        let file_name = format!(
            "{:x}.{}",
            std::mem::take(&mut self.digest).finalize(),
            format.extension()
        );
        if !valid_opaque_file_name(&file_name) {
            return Err(GameMediaError::Storage(
                "the generated artwork name is not opaque".into(),
            ));
        }
        let target = self.root.join(&file_name);
        self.staged.commit(&target)?;
        Ok(StoredMedia {
            file_name,
            mime_type: format.mime().to_owned(),
            byte_size: self.total,
        })
    }
}

fn declared_type_is_supported(declared: &str) -> bool {
    [
        MediaFormat::Jpeg,
        MediaFormat::Png,
        MediaFormat::Webp,
        MediaFormat::Gif,
        MediaFormat::Mp4,
    ]
    .into_iter()
    .any(|format| format.matches_declared(declared))
}

fn copy_file_atomically(
    source: &Path,
    staging: &Path,
    destination: &Path,
) -> Result<(), GameMediaError> {
    let mut input = fs::File::open(source).map_err(|_| GameMediaError::NotFound)?;
    let mut output = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(staging)?;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
    }
    output.sync_all()?;
    drop(output);
    fs::rename(staging, destination)?;
    Ok(())
}

fn imported_media_id(file_name: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orivo-imported-media-v1\0");
    digest.update(file_name.as_bytes());
    format!("media:{:x}", digest.finalize())
}

/// Content-addressed id for media that landed via a wallpaper search download.
fn downloaded_media_id(file_name: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orivo-downloaded-media-v1\0");
    digest.update(file_name.as_bytes());
    format!("media:{:x}", digest.finalize())
}

/// Opaque, stable id used only as the download-registry key for one wallpaper
/// fetch. Deduplicates simultaneous imports of the same URL.
fn wallpaper_download_key(url: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orivo-wallpaper-download-v1\0");
    digest.update(url.as_bytes());
    format!("wallpaper:{:x}", digest.finalize())
}

/// Search titles come from third-party APIs; they must be bounded, control-free
/// display text or the registration fails. A search source title is never
/// trusted raw.
fn sanitize_media_title(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Searched wallpaper".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn imported_media_title(kind: GameMediaKind) -> String {
    match kind {
        GameMediaKind::Wallpaper => "Imported wallpaper",
        GameMediaKind::Video => "Imported video",
        GameMediaKind::Icon => "Imported icon",
        GameMediaKind::Cover => "Imported cover",
    }
    .to_owned()
}

fn suggested_file_stem(title: &str) -> String {
    let stem = title
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();
    let stem = stem
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if stem.is_empty() {
        "orivo-artwork".to_owned()
    } else {
        stem.chars().take(64).collect()
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Apply: download when needed, then switch the active media atomically. Any
/// failure leaves the previous selection in place.
#[tauri::command]
pub async fn select_game_media(
    game_id: String,
    media_id: String,
    media: State<'_, GameMediaService>,
) -> Result<Vec<GameMediaView>, String> {
    let service = media.inner().clone();
    service
        .apply(&game_id, &media_id)
        .await
        .map_err(|error| error.to_string())
}

/// Export a saved copy through the native save dialog. Dismissing the dialog
/// is a silent success.
#[tauri::command]
pub fn export_game_media(
    game_id: String,
    media_id: String,
    media: State<'_, GameMediaService>,
) -> Result<(), String> {
    media
        .export(&game_id, &media_id)
        .map_err(|error| error.to_string())
}

/// Import artwork through the native file picker, validate it, and register it.
#[tauri::command]
pub fn import_game_media(
    game_id: String,
    kind: GameMediaKind,
    media: State<'_, GameMediaService>,
) -> Result<Vec<GameMediaView>, String> {
    media
        .import(&game_id, kind)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn cancel_game_media_download(
    game_id: String,
    media: State<'_, GameMediaService>,
) -> Result<(), String> {
    media.cancel(&game_id).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_detail::{
        GameDetailRecord, GameMediaAsset, GameSourceView, GameStateStore, GameSummaryView,
        PlatformView, PrimaryAction,
    };
    use std::sync::atomic::AtomicUsize;

    const ALLOWED_HOST: &str = "cdn.cloudflare.steamstatic.com";

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "orivo-media-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn png_bytes(length: usize) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.resize(length.max(MAGIC_MINIMUM_BYTES), 0x42);
        bytes
    }

    fn jpeg_bytes(length: usize) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe0];
        bytes.resize(length.max(MAGIC_MINIMUM_BYTES), 0x24);
        bytes
    }

    fn mp4_bytes(length: usize) -> Vec<u8> {
        let mut bytes = vec![0x00, 0x00, 0x00, 0x18];
        bytes.extend_from_slice(b"ftypisom");
        bytes.resize(length.max(MAGIC_MINIMUM_BYTES), 0x11);
        bytes
    }

    #[derive(Clone)]
    enum Reply {
        Redirect {
            status: u16,
            location: String,
        },
        Body {
            content_type: Option<String>,
            content_length: Option<u64>,
            chunks: Vec<Vec<u8>>,
            delay: Duration,
            hook: Option<Arc<dyn Fn(usize) + Send + Sync>>,
        },
    }

    impl Reply {
        fn body(content_type: &str, chunks: Vec<Vec<u8>>) -> Self {
            Self::Body {
                content_type: Some(content_type.to_owned()),
                content_length: None,
                chunks,
                delay: Duration::ZERO,
                hook: None,
            }
        }
    }

    #[derive(Default)]
    struct FakeTransport {
        replies: Mutex<BTreeMap<String, Reply>>,
        requests: Mutex<Vec<String>>,
    }

    impl FakeTransport {
        fn with(url: &str, reply: Reply) -> Arc<Self> {
            let transport = Arc::new(Self::default());
            transport.set(url, reply);
            transport
        }

        fn set(&self, url: &str, reply: Reply) {
            self.replies.lock().unwrap().insert(url.to_owned(), reply);
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl MediaTransport for FakeTransport {
        fn get<'a>(
            &'a self,
            url: &'a RemoteMediaUrl,
        ) -> MediaFuture<'a, Result<MediaResponse, GameMediaError>> {
            let key = url.as_str().to_owned();
            self.requests.lock().unwrap().push(key.clone());
            let reply = self.replies.lock().unwrap().get(&key).cloned();
            Box::pin(async move {
                match reply {
                    Some(Reply::Redirect { status, location }) => Ok(MediaResponse {
                        status,
                        location: Some(location),
                        content_type: None,
                        content_length: None,
                        body: Box::pin(futures_util::stream::empty()),
                    }),
                    Some(Reply::Body {
                        content_type,
                        content_length,
                        chunks,
                        delay,
                        hook,
                    }) => {
                        let body = futures_util::stream::unfold(0_usize, move |index| {
                            let chunks = chunks.clone();
                            let hook = hook.clone();
                            async move {
                                if index >= chunks.len() {
                                    return None;
                                }
                                if !delay.is_zero() {
                                    tokio::time::sleep(delay).await;
                                }
                                if let Some(hook) = hook.as_ref() {
                                    hook(index);
                                }
                                Some((Ok(chunks[index].clone()), index + 1))
                            }
                        });
                        Ok(MediaResponse {
                            status: 200,
                            location: None,
                            content_type,
                            content_length,
                            body: Box::pin(body),
                        })
                    }
                    None => Err(GameMediaError::Network("no route".into())),
                }
            })
        }
    }

    #[derive(Default)]
    struct FakePicker {
        import: Mutex<Option<PathBuf>>,
        export: Mutex<Option<PathBuf>>,
        export_calls: AtomicUsize,
    }

    impl MediaFilePicker for FakePicker {
        fn choose_import(&self, _kind: GameMediaKind) -> Option<PathBuf> {
            self.import.lock().unwrap().clone()
        }

        fn choose_export(&self, _suggested_file_name: &str) -> Option<PathBuf> {
            self.export_calls.fetch_add(1, Ordering::Relaxed);
            self.export.lock().unwrap().clone()
        }
    }

    fn remote_media(id: &str, kind: GameMediaKind, url: &str) -> GameMediaAsset {
        GameMediaAsset {
            id: id.into(),
            kind,
            title: "Example artwork".into(),
            source_url: Some(url.into()),
            poster_url: None,
            origin: GameMediaOrigin::Provider,
            local_file: None,
            mime_type: None,
            byte_size: 0,
            extra: BTreeMap::new(),
        }
    }

    fn record(media: Vec<GameMediaAsset>) -> GameDetailRecord {
        GameDetailRecord {
            summary: GameSummaryView {
                id: "local:aaa".into(),
                title: "Example".into(),
                source: GameSourceView::Local,
                short_description: "Short description".into(),
                cover_url: "/media/example-cover.jpg".into(),
                hero_url: "/media/example-hero.jpg".into(),
                landscape_url: "/media/example-landscape.jpg".into(),
                genres: Vec::new(),
                tags: Vec::new(),
                supported_platforms: vec![PlatformView::Macos],
                owned: true,
                launchable: true,
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
            media,
            related_games: Vec::new(),
            primary_action: PrimaryAction::Play,
        }
    }

    struct Harness {
        root: TempRoot,
        service: GameMediaService,
        detail: Arc<GameDetailService>,
        transport: Arc<FakeTransport>,
        picker: Arc<FakePicker>,
    }

    fn harness(
        label: &str,
        media: Vec<GameMediaAsset>,
        transport: Arc<FakeTransport>,
        limits: MediaLimits,
    ) -> Harness {
        let root = TempRoot::new(label);
        let state = Arc::new(GameStateStore::in_memory_for_tests());
        let detail = Arc::new(GameDetailService::new(state));
        detail.upsert_record(record(media)).unwrap();
        let picker = Arc::new(FakePicker::default());
        let service = GameMediaService::with_parts(
            Arc::clone(&detail),
            root.path().to_path_buf(),
            Arc::clone(&transport) as Arc<dyn MediaTransport>,
            Arc::clone(&picker) as Arc<dyn MediaFilePicker>,
            limits,
        )
        .unwrap();
        Harness {
            root,
            service,
            detail,
            transport,
            picker,
        }
    }

    fn cache_files(root: &Path) -> Vec<String> {
        let mut names = fs::read_dir(root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        tauri::async_runtime::block_on(future)
    }

    #[test]
    fn rejects_url_and_path_shaped_ids_from_the_webview() {
        let url = format!("https://{ALLOWED_HOST}/apps/1/cover.png");
        let transport = FakeTransport::with(&url, Reply::body("image/png", vec![png_bytes(32)]));
        let test = harness(
            "ids",
            vec![remote_media("media:cover", GameMediaKind::Cover, &url)],
            transport,
            MediaLimits::default(),
        );

        for hostile in [
            "../../secret",
            "/Users/private/game",
            "https://example.com/a",
            "game\0id",
            "media/../../etc/passwd",
        ] {
            assert!(matches!(
                block_on(test.service.apply(hostile, "media:cover")),
                Err(GameMediaError::Invalid(_))
            ));
            assert!(matches!(
                block_on(test.service.apply("local:aaa", hostile)),
                Err(GameMediaError::Invalid(_))
            ));
        }
        assert!(test.transport.requests().is_empty());
    }

    #[test]
    fn rejects_sources_that_are_not_https() {
        for hostile in [
            "http://cdn.cloudflare.steamstatic.com/a.png",
            "file:///etc/passwd",
            "data:image/png;base64,AAAA",
            "ftp://cdn.cloudflare.steamstatic.com/a.png",
            "//cdn.cloudflare.steamstatic.com/a.png",
        ] {
            assert!(validate_remote_media_url(hostile).is_err(), "{hostile}");
        }
        assert!(validate_remote_media_url(&format!("https://{ALLOWED_HOST}/a.png")).is_ok());
    }

    #[test]
    fn rejects_hosts_outside_the_allowlist() {
        for hostile in [
            "https://evil.example/a.png",
            "https://cdn.cloudflare.steamstatic.com.evil.example/a.png",
            "https://user@cdn.cloudflare.steamstatic.com/a.png",
            "https://cdn.cloudflare.steamstatic.com:8443/a.png",
            "https://127.0.0.1/a.png",
            "https://cdn.cloudflare.steamstatic.com/../../etc/passwd",
        ] {
            assert!(validate_remote_media_url(hostile).is_err(), "{hostile}");
        }
    }

    #[test]
    fn rejects_a_redirect_chain_that_leaves_the_allowlist() {
        let start = format!("https://{ALLOWED_HOST}/apps/1/cover.png");
        let transport = FakeTransport::with(
            &start,
            Reply::Redirect {
                status: 302,
                location: "https://evil.example/payload.png".into(),
            },
        );
        let test = harness(
            "redirect-host",
            vec![remote_media("media:cover", GameMediaKind::Cover, &start)],
            transport,
            MediaLimits::default(),
        );

        assert!(matches!(
            block_on(test.service.apply("local:aaa", "media:cover")),
            Err(GameMediaError::Invalid(_))
        ));
        // The off-allowlist hop is never requested.
        assert_eq!(test.transport.requests(), vec![start]);
        assert!(cache_files(test.root.path()).is_empty());
    }

    #[test]
    fn follows_at_most_three_redirects() {
        let url = |index: usize| format!("https://{ALLOWED_HOST}/hop/{index}.png");
        let transport = Arc::new(FakeTransport::default());
        for index in 0..4 {
            transport.set(
                &url(index),
                Reply::Redirect {
                    status: 302,
                    location: url(index + 1),
                },
            );
        }
        transport.set(&url(4), Reply::body("image/png", vec![png_bytes(32)]));
        let test = harness(
            "redirect-depth",
            vec![remote_media("media:cover", GameMediaKind::Cover, &url(0))],
            Arc::clone(&transport),
            MediaLimits::default(),
        );

        assert!(matches!(
            block_on(test.service.apply("local:aaa", "media:cover")),
            Err(GameMediaError::Invalid(_))
        ));
        assert_eq!(test.transport.requests().len(), 4);

        // Exactly three redirects still resolve.
        let shallow = harness(
            "redirect-depth-ok",
            vec![remote_media("media:cover", GameMediaKind::Cover, &url(1))],
            transport,
            MediaLimits::default(),
        );
        assert!(block_on(shallow.service.apply("local:aaa", "media:cover")).is_ok());
    }

    #[test]
    fn rejects_declared_type_and_magic_byte_mismatch() {
        let url = format!("https://{ALLOWED_HOST}/apps/1/cover.png");
        let transport = FakeTransport::with(&url, Reply::body("image/png", vec![jpeg_bytes(64)]));
        let test = harness(
            "magic",
            vec![remote_media("media:cover", GameMediaKind::Cover, &url)],
            transport,
            MediaLimits::default(),
        );

        assert!(matches!(
            block_on(test.service.apply("local:aaa", "media:cover")),
            Err(GameMediaError::Unsupported(_))
        ));
        assert!(cache_files(test.root.path()).is_empty());
    }

    #[test]
    fn rejects_a_video_payload_in_an_image_slot_and_undeclared_types() {
        let url = format!("https://{ALLOWED_HOST}/apps/1/cover.png");
        let transport = FakeTransport::with(&url, Reply::body("video/mp4", vec![mp4_bytes(64)]));
        let test = harness(
            "slot",
            vec![remote_media("media:cover", GameMediaKind::Cover, &url)],
            Arc::clone(&transport),
            MediaLimits::default(),
        );
        assert!(matches!(
            block_on(test.service.apply("local:aaa", "media:cover")),
            Err(GameMediaError::Unsupported(_))
        ));

        transport.set(
            &url,
            Reply::Body {
                content_type: None,
                content_length: None,
                chunks: vec![png_bytes(32)],
                delay: Duration::ZERO,
                hook: None,
            },
        );
        assert!(matches!(
            block_on(test.service.apply("local:aaa", "media:cover")),
            Err(GameMediaError::Unsupported(_))
        ));
    }

    #[test]
    fn size_limits_trip_mid_stream_instead_of_after_buffering() {
        let url = format!("https://{ALLOWED_HOST}/apps/1/cover.png");
        let consumed = Arc::new(AtomicUsize::new(0));
        let observer = Arc::clone(&consumed);
        let transport = FakeTransport::with(
            &url,
            Reply::Body {
                content_type: Some("image/png".into()),
                content_length: None,
                chunks: (0..40)
                    .map(|index| {
                        if index == 0 {
                            png_bytes(64)
                        } else {
                            vec![0x42; 64]
                        }
                    })
                    .collect(),
                delay: Duration::ZERO,
                hook: Some(Arc::new(move |_| {
                    observer.fetch_add(1, Ordering::Relaxed);
                })),
            },
        );
        let limits = MediaLimits {
            max_image_bytes: 200,
            ..MediaLimits::default()
        };
        let test = harness(
            "midstream",
            vec![remote_media("media:cover", GameMediaKind::Cover, &url)],
            transport,
            limits,
        );

        assert_eq!(
            block_on(test.service.apply("local:aaa", "media:cover")),
            Err(GameMediaError::TooLarge)
        );
        // Aborted while streaming: far fewer than the 40 available chunks.
        assert!(consumed.load(Ordering::Relaxed) <= 5);
        assert!(cache_files(test.root.path()).is_empty());
    }

    #[test]
    fn rejects_a_declared_length_over_the_cap_before_streaming() {
        let url = format!("https://{ALLOWED_HOST}/apps/1/hero.mp4");
        let transport = FakeTransport::with(
            &url,
            Reply::Body {
                content_type: Some("video/mp4".into()),
                content_length: Some(MAX_VIDEO_BYTES + 1),
                chunks: vec![mp4_bytes(64)],
                delay: Duration::ZERO,
                hook: None,
            },
        );
        let test = harness(
            "declared-length",
            vec![remote_media("media:video", GameMediaKind::Video, &url)],
            transport,
            MediaLimits::default(),
        );

        assert_eq!(
            block_on(test.service.apply("local:aaa", "media:video")),
            Err(GameMediaError::TooLarge)
        );
    }

    #[test]
    fn enforces_the_durable_quota_without_evicting_selected_media() {
        let first = format!("https://{ALLOWED_HOST}/apps/1/a.png");
        let second = format!("https://{ALLOWED_HOST}/apps/1/b.png");
        let transport = Arc::new(FakeTransport::default());
        transport.set(&first, Reply::body("image/png", vec![png_bytes(64)]));
        transport.set(&second, Reply::body("image/png", vec![jpeg_bytes(64)]));
        transport.set(
            &second.replace("b.png", "c.png"),
            Reply::body("image/png", vec![png_bytes(96)]),
        );
        let limits = MediaLimits {
            quota_bytes: 100,
            ..MediaLimits::default()
        };
        let test = harness(
            "quota",
            vec![
                remote_media("media:a", GameMediaKind::Cover, &first),
                remote_media("media:b", GameMediaKind::Wallpaper, &second),
            ],
            transport,
            limits,
        );

        assert!(block_on(test.service.apply("local:aaa", "media:a")).is_ok());
        let selected = cache_files(test.root.path());
        assert_eq!(selected.len(), 1);

        // The second download does not fit and the only cached file is the
        // active selection, so the sweep must refuse rather than delete it.
        let error = block_on(test.service.apply("local:aaa", "media:b")).unwrap_err();
        assert!(
            matches!(
                error,
                GameMediaError::QuotaExceeded | GameMediaError::Unsupported(_)
            ),
            "{error:?}"
        );
        assert_eq!(cache_files(test.root.path()), selected);
        assert!(
            test.detail
                .state()
                .protected_local_files()
                .unwrap()
                .contains(&selected[0])
        );
    }

    #[test]
    fn the_cache_sweep_evicts_unprotected_files_only() {
        let url = format!("https://{ALLOWED_HOST}/apps/1/a.png");
        let transport = FakeTransport::with(&url, Reply::body("image/png", vec![png_bytes(64)]));
        let limits = MediaLimits {
            quota_bytes: 128,
            ..MediaLimits::default()
        };
        let test = harness(
            "sweep",
            vec![remote_media("media:a", GameMediaKind::Cover, &url)],
            transport,
            limits,
        );
        fs::write(
            test.root.path().join("stale-cache-file.png"),
            vec![0x00; 120],
        )
        .unwrap();

        assert!(block_on(test.service.apply("local:aaa", "media:a")).is_ok());
        let files = cache_files(test.root.path());
        assert!(!files.contains(&"stale-cache-file.png".to_owned()));
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn deduplicates_identical_in_flight_downloads() {
        let url = format!("https://{ALLOWED_HOST}/apps/1/cover.png");
        let transport = FakeTransport::with(
            &url,
            Reply::Body {
                content_type: Some("image/png".into()),
                content_length: None,
                chunks: vec![png_bytes(64), vec![0x11; 64]],
                delay: Duration::from_millis(30),
                hook: None,
            },
        );
        let test = harness(
            "dedup",
            vec![remote_media("media:cover", GameMediaKind::Cover, &url)],
            transport,
            MediaLimits::default(),
        );

        let (left, right) = block_on(futures_util::future::join(
            test.service.apply("local:aaa", "media:cover"),
            test.service.apply("local:aaa", "media:cover"),
        ));

        assert!(left.is_ok(), "{left:?}");
        assert!(right.is_ok(), "{right:?}");
        assert_eq!(test.transport.requests(), vec![url]);
        assert_eq!(cache_files(test.root.path()).len(), 1);
    }

    #[test]
    fn never_runs_more_than_two_downloads_at_once() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let transport = Arc::new(FakeTransport::default());
        let media = (0..4)
            .map(|index| {
                let url = format!("https://{ALLOWED_HOST}/apps/1/{index}.png");
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                transport.set(
                    &url,
                    Reply::Body {
                        content_type: Some("image/png".into()),
                        content_length: None,
                        chunks: vec![png_bytes(64), vec![index as u8; 64], vec![0x33; 64]],
                        delay: Duration::from_millis(20),
                        hook: Some(Arc::new(move |chunk| {
                            match chunk {
                                0 => {
                                    let running = active.fetch_add(1, Ordering::SeqCst) + 1;
                                    peak.fetch_max(running, Ordering::SeqCst);
                                }
                                2 => {
                                    active.fetch_sub(1, Ordering::SeqCst);
                                }
                                _ => {}
                            };
                        })),
                    },
                );
                remote_media(&format!("media:cover{index}"), GameMediaKind::Cover, &url)
            })
            .collect::<Vec<_>>();
        let test = harness("concurrency", media, transport, MediaLimits::default());

        let ids = (0..4)
            .map(|index| format!("media:cover{index}"))
            .collect::<Vec<_>>();
        let outcomes = block_on(futures_util::future::join_all(
            ids.iter().map(|id| test.service.apply("local:aaa", id)),
        ));

        assert!(outcomes.iter().all(Result::is_ok), "{outcomes:?}");
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cancellation_stops_a_download_and_keeps_the_previous_selection() {
        let bundled = GameMediaAsset {
            id: "media:bundled".into(),
            kind: GameMediaKind::Cover,
            title: "Bundled cover".into(),
            source_url: Some("/media/example-cover.jpg".into()),
            poster_url: None,
            origin: GameMediaOrigin::Bundled,
            local_file: None,
            mime_type: None,
            byte_size: 0,
            extra: BTreeMap::new(),
        };
        let url = format!("https://{ALLOWED_HOST}/apps/1/cover.png");
        // The user cancels while the transfer is running: the hook fires from
        // inside the stream, so the abort is observed deterministically.
        let cancelling_service: Arc<std::sync::OnceLock<GameMediaService>> =
            Arc::new(std::sync::OnceLock::new());
        let observer = Arc::clone(&cancelling_service);
        let transport = FakeTransport::with(
            &url,
            Reply::Body {
                content_type: Some("image/png".into()),
                content_length: None,
                chunks: vec![png_bytes(64), vec![0x11; 64], vec![0x12; 64]],
                delay: Duration::ZERO,
                hook: Some(Arc::new(move |index| {
                    if index == 0
                        && let Some(service) = observer.get()
                    {
                        service.cancel("local:aaa").unwrap();
                    }
                })),
            },
        );
        let test = harness(
            "cancel",
            vec![
                bundled,
                remote_media("media:cover", GameMediaKind::Cover, &url),
            ],
            transport,
            MediaLimits::default(),
        );
        cancelling_service.set(test.service.clone()).ok();

        assert!(block_on(test.service.apply("local:aaa", "media:bundled")).is_ok());
        let outcome = block_on(test.service.apply("local:aaa", "media:cover"));

        assert_eq!(outcome, Err(GameMediaError::Cancelled));
        assert_eq!(
            test.detail
                .state()
                .selected_media("local:aaa")
                .unwrap()
                .get(&GameMediaKind::Cover)
                .map(String::as_str),
            Some("media:bundled")
        );
        assert!(cache_files(test.root.path()).is_empty());
    }

    #[test]
    fn writes_are_atomic_and_content_addressed() {
        let url = format!("https://{ALLOWED_HOST}/apps/1/cover.png");
        let payload = png_bytes(96);
        let transport = FakeTransport::with(
            &url,
            Reply::body(
                "image/png",
                vec![payload[..32].to_vec(), payload[32..].to_vec()],
            ),
        );
        let test = harness(
            "atomic",
            vec![remote_media("media:cover", GameMediaKind::Cover, &url)],
            transport,
            MediaLimits::default(),
        );

        let views = block_on(test.service.apply("local:aaa", "media:cover")).unwrap();
        let mut digest = Sha256::new();
        digest.update(&payload);
        let expected = format!("{:x}.png", digest.finalize());

        assert_eq!(cache_files(test.root.path()), vec![expected.clone()]);
        assert_eq!(fs::read(test.root.path().join(&expected)).unwrap(), payload);
        assert!(valid_opaque_file_name(&expected));
        let applied = views
            .iter()
            .find(|view| view.id == "media:cover")
            .expect("applied view");
        assert!(applied.selected);
        assert!(applied.available_offline);
        assert_eq!(applied.preview_url, format!("game-media:{expected}"));
    }

    #[test]
    fn importing_a_wallpaper_selects_it() {
        let url = format!("https://{ALLOWED_HOST}/apps/1/wallpaper.png");
        let transport = FakeTransport::with(&url, Reply::body("image/png", vec![png_bytes(64)]));
        let test = harness("import-selects", vec![], transport, MediaLimits::default());

        let views = block_on(test.service.download_wallpaper("local:aaa", &url, "Wallpaper")).unwrap();
        let imported = views
            .iter()
            .find(|view| view.kind == GameMediaKind::Wallpaper)
            .expect("imported view");
        assert!(imported.selected);
        assert!(imported.available_offline);
        assert_eq!(
            test.detail
                .state()
                .selected_media("local:aaa")
                .unwrap()
                .get(&GameMediaKind::Wallpaper),
            Some(&imported.id)
        );
    }

    #[test]
    fn a_failed_apply_preserves_the_previous_selection() {
        let good = format!("https://{ALLOWED_HOST}/apps/1/good.png");
        let bad = format!("https://{ALLOWED_HOST}/apps/1/bad.png");
        let transport = Arc::new(FakeTransport::default());
        transport.set(&good, Reply::body("image/png", vec![png_bytes(64)]));
        transport.set(&bad, Reply::body("image/png", vec![vec![0x00; 64]]));
        let test = harness(
            "preserve",
            vec![
                remote_media("media:good", GameMediaKind::Cover, &good),
                remote_media("media:bad", GameMediaKind::Cover, &bad),
            ],
            transport,
            MediaLimits::default(),
        );

        assert!(block_on(test.service.apply("local:aaa", "media:good")).is_ok());
        assert!(block_on(test.service.apply("local:aaa", "media:bad")).is_err());
        assert!(block_on(test.service.apply("local:aaa", "media:missing")).is_err());

        assert_eq!(
            test.detail
                .state()
                .selected_media("local:aaa")
                .unwrap()
                .get(&GameMediaKind::Cover)
                .map(String::as_str),
            Some("media:good")
        );
        assert_eq!(cache_files(test.root.path()).len(), 1);
    }

    #[test]
    fn export_cancellation_is_a_silent_success() {
        let url = format!("https://{ALLOWED_HOST}/apps/1/cover.png");
        let transport = FakeTransport::with(&url, Reply::body("image/png", vec![png_bytes(64)]));
        let test = harness(
            "export",
            vec![remote_media("media:cover", GameMediaKind::Cover, &url)],
            transport,
            MediaLimits::default(),
        );
        block_on(test.service.apply("local:aaa", "media:cover")).unwrap();

        // Dismissed dialog: no error, nothing written.
        *test.picker.export.lock().unwrap() = None;
        assert_eq!(test.service.export("local:aaa", "media:cover"), Ok(()));
        assert_eq!(test.picker.export_calls.load(Ordering::Relaxed), 1);

        let destination = TempRoot::new("export-target");
        let target = destination.path().join("artwork.png");
        *test.picker.export.lock().unwrap() = Some(target.clone());
        assert_eq!(test.service.export("local:aaa", "media:cover"), Ok(()));
        assert_eq!(fs::read(&target).unwrap(), png_bytes(64));
        assert_eq!(cache_files(destination.path()), vec!["artwork.png"]);
    }

    #[test]
    fn import_validates_content_before_registering() {
        let source = TempRoot::new("import-source");
        let picked = source.path().join("chosen.png");
        let transport = Arc::new(FakeTransport::default());
        let test = harness("import", Vec::new(), transport, MediaLimits::default());

        // A file whose extension lies about its content is rejected.
        fs::write(&picked, mp4_bytes(64)).unwrap();
        *test.picker.import.lock().unwrap() = Some(picked.clone());
        assert!(matches!(
            test.service.import("local:aaa", GameMediaKind::Cover),
            Err(GameMediaError::Unsupported(_))
        ));
        assert!(cache_files(test.root.path()).is_empty());

        fs::write(&picked, png_bytes(64)).unwrap();
        let views = test
            .service
            .import("local:aaa", GameMediaKind::Cover)
            .unwrap();
        let imported = views
            .iter()
            .find(|view| view.origin == GameMediaOrigin::Imported)
            .expect("imported view");
        assert!(imported.available_offline);
        assert_eq!(cache_files(test.root.path()).len(), 1);
        // Imported artwork is user data and is never a sweep candidate.
        assert_eq!(
            test.detail.state().protected_local_files().unwrap().len(),
            1
        );

        // A dismissed picker is a silent no-op.
        *test.picker.import.lock().unwrap() = None;
        let unchanged = test
            .service
            .import("local:aaa", GameMediaKind::Cover)
            .unwrap();
        assert_eq!(unchanged.len(), views.len());
    }

    #[test]
    fn import_rejects_unknown_games_and_oversized_files() {
        let source = TempRoot::new("import-limits");
        let picked = source.path().join("chosen.png");
        fs::write(&picked, png_bytes(4096)).unwrap();
        let transport = Arc::new(FakeTransport::default());
        let limits = MediaLimits {
            max_image_bytes: 128,
            ..MediaLimits::default()
        };
        let test = harness("import-limits", Vec::new(), transport, limits);
        *test.picker.import.lock().unwrap() = Some(picked);

        assert_eq!(
            test.service.import("local:zzz", GameMediaKind::Cover),
            Err(GameMediaError::NotFound)
        );
        assert_eq!(
            test.service.import("local:aaa", GameMediaKind::Cover),
            Err(GameMediaError::TooLarge)
        );
        assert!(cache_files(test.root.path()).is_empty());
    }

    #[test]
    fn views_never_expose_filesystem_paths_secrets_or_source_urls() {
        let url = format!("https://{ALLOWED_HOST}/apps/1/cover.png");
        let transport = FakeTransport::with(&url, Reply::body("image/png", vec![png_bytes(64)]));
        let test = harness(
            "dto",
            vec![remote_media("media:cover", GameMediaKind::Cover, &url)],
            transport,
            MediaLimits::default(),
        );

        let views = block_on(test.service.apply("local:aaa", "media:cover")).unwrap();
        let json = serde_json::to_string(&views).unwrap();

        assert!(!json.contains(&test.root.path().to_string_lossy().into_owned()));
        assert!(!json.contains(std::env::temp_dir().to_str().unwrap()));
        assert!(!json.contains("/Users/"));
        assert!(!json.contains("sourceUrl"));
        assert!(!json.contains("localFile"));
        assert!(!json.contains("https://"));
        assert!(json.contains("game-media:"));
    }

    #[test]
    fn a_dropped_leader_releases_its_in_flight_entry() {
        let url = format!("https://{ALLOWED_HOST}/apps/1/cover.png");
        let transport = FakeTransport::with(
            &url,
            Reply::Body {
                content_type: Some("image/png".into()),
                content_length: None,
                chunks: vec![png_bytes(64), vec![0x11; 64]],
                delay: Duration::from_millis(250),
                hook: None,
            },
        );
        let test = harness(
            "dropped-leader",
            vec![remote_media("media:cover", GameMediaKind::Cover, &url)],
            transport,
            MediaLimits::default(),
        );

        // The command future is abandoned mid-transfer, as it is when the
        // WebView navigates away or the invoke is cancelled.
        let abandoned = block_on(async {
            tokio::time::timeout(
                Duration::from_millis(20),
                test.service.apply("local:aaa", "media:cover"),
            )
            .await
        });
        assert!(abandoned.is_err());

        {
            let registry = test.service.inner.lock_downloads().unwrap();
            assert!(
                registry.inflight.is_empty(),
                "the entry outlived its leader"
            );
            assert!(registry.cancels.is_empty());
            assert_eq!(registry.reserved_bytes, 0);
        }
        // Nothing partial is left in the media directory either.
        assert!(cache_files(test.root.path()).is_empty());

        // The retry runs as a new leader instead of spinning against an entry
        // nobody will ever complete.
        let started = std::time::Instant::now();
        let retried = block_on(test.service.apply("local:aaa", "media:cover"));
        assert!(retried.is_ok(), "{retried:?}");
        assert!(started.elapsed() < INFLIGHT_DEADLINE);
        assert_eq!(cache_files(test.root.path()).len(), 1);
    }

    #[test]
    fn a_cancel_flag_never_survives_into_the_next_download() {
        let slow = format!("https://{ALLOWED_HOST}/apps/1/slow.png");
        let quick = format!("https://{ALLOWED_HOST}/apps/1/quick.png");
        let transport = Arc::new(FakeTransport::default());
        transport.set(
            &slow,
            Reply::Body {
                content_type: Some("image/png".into()),
                content_length: None,
                chunks: vec![png_bytes(64), vec![0x11; 64]],
                delay: Duration::from_millis(120),
                hook: None,
            },
        );
        transport.set(&quick, Reply::body("image/png", vec![png_bytes(96)]));
        let test = harness(
            "cancel-reuse",
            vec![
                remote_media("media:slow", GameMediaKind::Cover, &slow),
                remote_media("media:quick", GameMediaKind::Wallpaper, &quick),
            ],
            transport,
            MediaLimits::default(),
        );

        // The user cancels the running download, then asks for different
        // artwork for the same game while the first one is still unwinding.
        let (cancelled, requested_after) = block_on(async {
            let running = test.service.apply("local:aaa", "media:slow");
            let after_cancel = async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                test.service.cancel("local:aaa").unwrap();
                test.service.apply("local:aaa", "media:quick").await
            };
            futures_util::future::join(running, after_cancel).await
        });

        assert_eq!(cancelled, Err(GameMediaError::Cancelled));
        // A flag minted for the cancelled transfer must not decide this one.
        assert!(requested_after.is_ok(), "{requested_after:?}");
        assert!(
            test.service
                .inner
                .lock_downloads()
                .unwrap()
                .cancels
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_planted_in_the_media_directory_is_never_used() {
        let transport = Arc::new(FakeTransport::default());
        let test = harness(
            "apply-symlink",
            Vec::new(),
            transport,
            MediaLimits::default(),
        );
        let outside = TempRoot::new("apply-symlink-target");
        let target = outside.path().join("private.png");
        fs::write(&target, png_bytes(64)).unwrap();
        std::os::unix::fs::symlink(&target, test.root.path().join("planted.png")).unwrap();

        test.detail
            .state()
            .register_media(
                "local:aaa",
                GameMediaAsset {
                    id: "media:planted".into(),
                    kind: GameMediaKind::Cover,
                    title: "Planted".into(),
                    source_url: None,
                    poster_url: None,
                    origin: GameMediaOrigin::Imported,
                    local_file: Some("planted.png".into()),
                    mime_type: Some("image/png".into()),
                    byte_size: 64,
                    extra: BTreeMap::new(),
                },
            )
            .unwrap();

        assert_eq!(
            block_on(test.service.apply("local:aaa", "media:planted")),
            Err(GameMediaError::NotFound)
        );
        assert_eq!(
            test.service.export("local:aaa", "media:planted"),
            Err(GameMediaError::NotFound)
        );
        assert_eq!(test.picker.export_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cancelling_without_a_download_is_a_no_op_and_ids_are_still_validated() {
        let transport = Arc::new(FakeTransport::default());
        let test = harness("cancel-noop", Vec::new(), transport, MediaLimits::default());

        assert_eq!(test.service.cancel("local:aaa"), Ok(()));
        assert!(matches!(
            test.service.cancel("../../etc/passwd"),
            Err(GameMediaError::Invalid(_))
        ));
    }
}
