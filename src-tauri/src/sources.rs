//! Connected store accounts beyond Steam: Epic Games, GOG, Ubisoft Connect,
//! Xbox, Microsoft Store and Instant Gaming.
//!
//! This module is the shared boundary every connector goes through. It owns
//! three rules that the individual providers are not allowed to bend:
//!
//! 1. **A secret never crosses IPC.** Credentials live in the operating-system
//!    keychain. The WebView learns whether a source is connected and under
//!    which display name — never a token, ticket or cookie.
//! 2. **A provider answer is untrusted input.** Ids are held to the catalog's
//!    opaque-token grammar and artwork URLs to a per-provider host allowlist,
//!    so a compromised or merely reshaped response cannot become a filesystem
//!    path, a process argument or an arbitrary origin in the main window.
//! 3. **A partial answer is still an answer.** One unreadable game never fails
//!    a whole library sync; it is dropped and counted.
//!
//! Two connection styles exist, because only half of these stores publish a
//! usable token API:
//!
//! * [`ConnectStyle::Token`] — Epic, GOG and Microsoft. The sign-in WebView
//!   hands Rust a one-time authorization code, Rust exchanges it for an OAuth
//!   credential, and every later sync runs headlessly from the keychain.
//! * [`ConnectStyle::Session`] — Ubisoft Connect and Instant Gaming, which have
//!   no public account API. The sign-in WebView keeps the session and the sync
//!   itself runs *inside* that authenticated origin, returning only a compact
//!   list of games. No long-lived provider secret is ever copied out of the
//!   WebView, which is strictly safer than extracting and storing a ticket.

use crate::catalog::{self, GameSource};
use keyring::{Entry, Error as KeyringError};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Versioned so a future change to the stored credential shape cannot make an
/// older entry deserialize into something it never meant.
const KEYRING_SERVICE: &str = "io.orivo.desktop.sources.v1";

/// Which stores are connected, and under which display name — and nothing else.
///
/// macOS asks for the keychain password every time an application whose code
/// signature it does not recognise reads an item, so merely *rendering* the
/// Settings list must never touch the keychain. This file is the answer: it
/// holds no secret, so opening Settings costs one small read, and the keychain
/// is opened only when a sync actually needs the token.
pub const CONNECTIONS_FILE: &str = "source-connections.json";

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
pub const USER_AGENT: &str = "Orivo/0.3 library sync";
/// A connected library is enumerated in one request per provider, so a single
/// oversized answer is the only thing worth bounding here.
pub const MAX_LIBRARY_GAMES: usize = 5_000;
/// Refresh slightly early: a token that expires mid-request would otherwise
/// surface to the user as a spurious "reconnect this account".
const TOKEN_REFRESH_MARGIN_MS: u64 = 120_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceProvider {
    Epic,
    Gog,
    Ubisoft,
    Xbox,
    MicrosoftStore,
    InstantGaming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectStyle {
    Token,
    Session,
}

/// Where a stored credential lives. Xbox and Microsoft Store are two libraries
/// behind one Microsoft account, so they deliberately share a single entry:
/// signing in once connects both, and disconnecting either signs the account
/// out of both. The UI says so rather than pretending they are independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialNamespace {
    Epic,
    Gog,
    Microsoft,
    Ubisoft,
    InstantGaming,
}

impl CredentialNamespace {
    fn keychain_account(self) -> &'static str {
        match self {
            Self::Epic => "epic",
            Self::Gog => "gog",
            Self::Microsoft => "microsoft",
            Self::Ubisoft => "ubisoft",
            Self::InstantGaming => "instant-gaming",
        }
    }
}

impl SourceProvider {
    pub fn all() -> [Self; 6] {
        [
            Self::Epic,
            Self::Gog,
            Self::Ubisoft,
            Self::Xbox,
            Self::MicrosoftStore,
            Self::InstantGaming,
        ]
    }

    /// The stable token the WebView addresses this provider by. It is also the
    /// `provider` field of a `LaunchTarget::Provider`, so the two can never
    /// drift apart.
    pub fn token(self) -> &'static str {
        self.catalog_source()
            .provider_token()
            .expect("every connected source has a provider token")
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Self::all()
            .into_iter()
            .find(|provider| provider.token() == token)
    }

    pub fn catalog_source(self) -> GameSource {
        match self {
            Self::Epic => GameSource::Epic,
            Self::Gog => GameSource::Gog,
            Self::Ubisoft => GameSource::Ubisoft,
            Self::Xbox => GameSource::Xbox,
            Self::MicrosoftStore => GameSource::MicrosoftStore,
            Self::InstantGaming => GameSource::InstantGaming,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Epic => "Epic Games",
            Self::Gog => "GOG",
            Self::Ubisoft => "Ubisoft Connect",
            Self::Xbox => "Xbox",
            Self::MicrosoftStore => "Microsoft Store",
            Self::InstantGaming => "Instant Gaming",
        }
    }

    pub fn credential_namespace(self) -> CredentialNamespace {
        match self {
            Self::Epic => CredentialNamespace::Epic,
            Self::Gog => CredentialNamespace::Gog,
            Self::Ubisoft => CredentialNamespace::Ubisoft,
            Self::Xbox | Self::MicrosoftStore => CredentialNamespace::Microsoft,
            Self::InstantGaming => CredentialNamespace::InstantGaming,
        }
    }

    pub fn connect_style(self) -> ConnectStyle {
        match self {
            Self::Epic | Self::Gog | Self::Xbox | Self::MicrosoftStore => ConnectStyle::Token,
            Self::Ubisoft | Self::InstantGaming => ConnectStyle::Session,
        }
    }

    /// Every provider that shares this one's credential entry. Connecting or
    /// disconnecting is reported for all of them at once.
    pub fn siblings(self) -> Vec<Self> {
        let namespace = self.credential_namespace();
        Self::all()
            .into_iter()
            .filter(|provider| provider.credential_namespace() == namespace)
            .collect()
    }

    /// One short sentence describing what connecting this source actually
    /// gives the user, including where it stops. Shown verbatim in Settings.
    pub fn description(self) -> &'static str {
        match self {
            Self::Epic => "Your owned Epic games, with their store artwork.",
            Self::Gog => "Your DRM-free GOG library, with its store artwork.",
            Self::Ubisoft => {
                "Your Ubisoft Connect games. The sign-in window stays signed in and each sync runs inside it."
            }
            Self::Xbox => "Games you have played on Xbox, from your Microsoft account.",
            Self::MicrosoftStore => {
                "The PC side of the same Microsoft account: Game Pass and Microsoft Store titles."
            }
            Self::InstantGaming => {
                "Your Instant Gaming purchases. Keys stay redeemed on the store they belong to, so these entries do not launch."
            }
        }
    }

    /// Whether a synced record can ever be started from Orivo. Instant Gaming
    /// sells keys redeemed elsewhere and Xbox titles are console entitlements,
    /// so neither pretends to be launchable.
    pub fn launchable(self) -> bool {
        matches!(
            self,
            Self::Epic | Self::Gog | Self::Ubisoft | Self::MicrosoftStore
        )
    }

    /// Hosts whose artwork this provider may point the main window at. A URL
    /// that does not match is dropped, not rewritten: a missing cover is a far
    /// better outcome than an arbitrary origin loaded in the app.
    pub fn media_hosts(self) -> &'static [&'static str] {
        match self {
            Self::Epic => &[
                "cdn1.epicgames.com",
                "cdn2.epicgames.com",
                "cdn.epicgames.com",
                "cdn1.unrealengine.com",
                "cdn2.unrealengine.com",
                "cdn.unrealengine.com",
            ],
            Self::Gog => &["gog-statics.com", "gog.com"],
            Self::Ubisoft => &[
                "ubi.com",
                "ubisoft.com",
                "ubistatic-a.akamaihd.net",
                "ubistatic3-a.akamaihd.net",
                "ubistatic19-a.akamaihd.net",
            ],
            Self::Xbox | Self::MicrosoftStore => &[
                "store-images.s-microsoft.com",
                "images-eds.xboxlive.com",
                "images-eds-ssl.xboxlive.com",
                "xboxlive.com",
            ],
            Self::InstantGaming => &["gaming-cdn.com", "instant-gaming.com"],
        }
    }
}

impl fmt::Display for SourceProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Presentation-safe connection state. Nothing here identifies a session: the
/// account label is the display name the provider itself shows its user.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAccountStatus {
    pub provider: String,
    pub label: String,
    pub description: String,
    pub connected: bool,
    pub account_label: String,
    /// `"token"` or `"session"`, so the UI can explain that a session-style
    /// source opens its own window to sync.
    pub style: String,
    /// Providers that share this one's sign-in. Empty when it stands alone.
    pub shares_sign_in_with: Vec<String>,
    pub launchable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSyncResponse {
    pub provider: String,
    pub label: String,
    pub total_games: u32,
    pub imported_games: u32,
    pub updated_games: u32,
    /// Provider entries Orivo refused: an id outside the opaque grammar, an
    /// empty title, or a record past the library ceiling. Surfacing the count
    /// keeps a silently short library from reading as a complete one.
    pub skipped_games: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAccountEvent {
    pub provider: String,
    pub account_label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLoginFailedEvent {
    pub provider: String,
    pub message: String,
}

/// A platform a store says one of its games ships a build for. Deliberately
/// closed and deliberately small: a connector may report only what the library
/// can actually browse by, and nothing a response says becomes a segment on
/// its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourcePlatform {
    Windows,
    Macos,
    Linux,
}

impl SourcePlatform {
    /// The same tokens Steam's own platform answer uses, so the catalog holds
    /// one vocabulary whatever store filled it.
    pub fn token(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Linux => "linux",
        }
    }
}

/// One game as a connector reports it, before the catalog sees it. Every field
/// is already normalised: ids passed the opaque grammar and artwork URLs
/// passed the provider's host allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLibraryGame {
    pub source_id: String,
    pub launch_ref: String,
    pub title: String,
    pub description: Option<String>,
    pub genre: Option<String>,
    /// The studio, which is not a genre. Kept apart so no connector is ever
    /// tempted to bill one as the other again.
    pub developer: Option<String>,
    /// The transparent wordmark, if the store publishes one apart from its
    /// artwork. Drawn over the scene; never used as a wallpaper.
    pub logo_url: Option<String>,
    pub cover_url: Option<String>,
    pub hero_url: Option<String>,
    pub landscape_url: Option<String>,
    pub play_time_seconds: u64,
    pub last_played_at: Option<String>,
    /// Whether the store says this game ships a build that runs natively on
    /// macOS. `None` means the connector has no answer, which is not the same
    /// as "no": only a store that publishes per-platform entitlements can tell.
    pub native_mac: Option<bool>,
    /// Every platform the store says this game ships a build for.
    ///
    /// `native_mac` is the answer a store can give when all it publishes is a
    /// per-platform entitlement list. A store that publishes the whole matrix
    /// fills this instead, and a connector that knows nothing leaves it empty
    /// — empty is "unknown", never "runs nowhere".
    pub platforms: Vec<SourcePlatform>,
    /// What the store's own client reports about this game on this machine.
    /// `None` for a store with no local client to ask.
    pub install: Option<SourceInstallStatus>,
}

/// Local install state a connector observed through its store's own client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInstallStatus {
    pub installed: bool,
    /// A download or repair the store client is still running.
    pub installing: bool,
    /// 0-100. Only meaningful while `installing`.
    pub percent: u8,
    pub install_path: Option<String>,
}

/// What a connector produced for one account, including what it had to drop.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceLibrary {
    pub games: Vec<SourceLibraryGame>,
    pub skipped: u32,
}

impl SourceLibrary {
    /// Accept a provider record only if every mandatory part of it survives
    /// normalisation. Anything else is counted and dropped.
    pub fn push(&mut self, provider: SourceProvider, game: SourceLibraryGame) {
        if self.games.len() >= MAX_LIBRARY_GAMES {
            self.skipped = self.skipped.saturating_add(1);
            return;
        }
        let title = normalize_title(&game.title);
        let (Some(title), true, true) = (
            title,
            catalog::is_valid_provider_reference(&game.source_id),
            catalog::is_valid_provider_reference(&game.launch_ref),
        ) else {
            self.skipped = self.skipped.saturating_add(1);
            return;
        };
        if self
            .games
            .iter()
            .any(|existing| existing.source_id == game.source_id)
        {
            // A provider can list the same entitlement twice (a base game and
            // its edition upgrade share an id on more than one store). The
            // first one wins rather than failing the catalog's identity check.
            return;
        }
        self.games.push(SourceLibraryGame {
            title,
            description: game.description.and_then(|text| normalize_text(&text, 600)),
            genre: game.genre.and_then(|text| normalize_text(&text, 80)),
            developer: game.developer.and_then(|text| normalize_text(&text, 120)),
            logo_url: game
                .logo_url
                .and_then(|url| sanitize_media_url(&url, provider)),
            cover_url: game
                .cover_url
                .and_then(|url| sanitize_media_url(&url, provider)),
            hero_url: game
                .hero_url
                .and_then(|url| sanitize_media_url(&url, provider)),
            landscape_url: game
                .landscape_url
                .and_then(|url| sanitize_media_url(&url, provider)),
            ..game
        });
    }

    pub fn sort_by_title(&mut self) {
        self.games.sort_by(|left, right| {
            left.title
                .to_lowercase()
                .cmp(&right.title.to_lowercase())
                .then_with(|| left.source_id.cmp(&right.source_id))
        });
    }
}

#[derive(Debug)]
pub enum SourceError {
    NotConnected(SourceProvider),
    Keychain(SourceProvider),
    InvalidCredential(SourceProvider),
    SessionExpired(SourceProvider),
    RateLimited(SourceProvider),
    Network(SourceProvider),
    /// The provider answered, but not in a shape this build understands. This
    /// is the honest outcome for the two stores without a published account
    /// API, and it must read as "Orivo has to catch up", not "you did it wrong".
    UnexpectedResponse(SourceProvider),
    Unsupported(SourceProvider),
}

impl SourceError {
    pub fn provider(&self) -> SourceProvider {
        match self {
            Self::NotConnected(provider)
            | Self::Keychain(provider)
            | Self::InvalidCredential(provider)
            | Self::SessionExpired(provider)
            | Self::RateLimited(provider)
            | Self::Network(provider)
            | Self::UnexpectedResponse(provider)
            | Self::Unsupported(provider) => *provider,
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let provider = self.provider().label();
        match self {
            Self::NotConnected(_) => {
                write!(f, "Connect {provider} before syncing its library.")
            }
            Self::Keychain(_) => write!(
                f,
                "Orivo could not read the secure {provider} connection from the system keychain. Connect {provider} again."
            ),
            Self::InvalidCredential(_) => {
                write!(
                    f,
                    "{provider} rejected the account connection. Connect it again."
                )
            }
            Self::SessionExpired(_) => write!(
                f,
                "Your {provider} session expired. Connect {provider} again, then sync."
            ),
            Self::RateLimited(_) => write!(
                f,
                "{provider} is temporarily limiting library requests. Try syncing again in a moment."
            ),
            Self::Network(_) => write!(
                f,
                "{provider} could not be reached. Check your connection and try again."
            ),
            Self::UnexpectedResponse(_) => write!(
                f,
                "{provider} answered in a shape Orivo does not recognise yet. Try again later."
            ),
            Self::Unsupported(_) => {
                write!(f, "{provider} cannot be connected from this build yet.")
            }
        }
    }
}

impl std::error::Error for SourceError {}

/// The persisted shape of a connection. `Session` deliberately carries no
/// secret: the sign-in WebView's own cookie jar is the credential, and this
/// record only remembers that a sign-in completed and under which name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredSourceCredential {
    OAuth {
        #[serde(default)]
        account_id: String,
        #[serde(default)]
        account_label: String,
        access_token: String,
        #[serde(default)]
        refresh_token: String,
        /// Unix milliseconds. Zero means "unknown", which forces a refresh
        /// before the token is used rather than assuming it is still valid.
        #[serde(default)]
        expires_at_ms: u64,
    },
    Session {
        #[serde(default)]
        account_label: String,
    },
}

impl StoredSourceCredential {
    pub fn account_label(&self) -> &str {
        match self {
            Self::OAuth { account_label, .. } | Self::Session { account_label } => account_label,
        }
    }

    pub fn account_id(&self) -> &str {
        match self {
            Self::OAuth { account_id, .. } => account_id,
            Self::Session { .. } => "",
        }
    }

    pub fn access_token(&self) -> Option<&str> {
        match self {
            Self::OAuth { access_token, .. } => Some(access_token),
            Self::Session { .. } => None,
        }
    }

    pub fn refresh_token(&self) -> Option<&str> {
        match self {
            Self::OAuth { refresh_token, .. } => {
                (!refresh_token.is_empty()).then_some(refresh_token.as_str())
            }
            Self::Session { .. } => None,
        }
    }

    pub fn needs_refresh(&self) -> bool {
        match self {
            Self::OAuth { expires_at_ms, .. } => {
                *expires_at_ms == 0
                    || *expires_at_ms <= unix_millis().saturating_add(TOKEN_REFRESH_MARGIN_MS)
            }
            Self::Session { .. } => false,
        }
    }
}

pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// Turn a provider's `expires_in` (seconds) into the absolute instant the
/// credential store keeps. Relative lifetimes are useless once persisted.
pub fn expiry_from_seconds(seconds: u64) -> u64 {
    unix_millis().saturating_add(seconds.saturating_mul(1_000))
}

// ---------------------------------------------------------------------------
// Connection directory (no secrets)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ConnectionsDocument {
    /// Keyed by credential namespace, because that is what a sign-in actually
    /// connects: signing into Xbox connects Microsoft Store in the same move.
    #[serde(default)]
    accounts: BTreeMap<String, ConnectionRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ConnectionRecord {
    #[serde(default)]
    account_label: String,
}

static CONNECTIONS_PATH: OnceLock<PathBuf> = OnceLock::new();
static CONNECTIONS: OnceLock<Mutex<ConnectionsDocument>> = OnceLock::new();
/// The keychain is opened at most once per store per run. Without this, a
/// connect immediately followed by a sync would prompt twice on macOS.
static CREDENTIAL_CACHE: OnceLock<Mutex<BTreeMap<String, StoredSourceCredential>>> =
    OnceLock::new();

/// Point the connection directory at the app's data directory. Called once
/// during setup; before that, and in tests, the directory is in-memory only.
pub fn set_connections_path(path: PathBuf) {
    let document = read_connections_document(&path);
    let _ = CONNECTIONS_PATH.set(path);
    let _ = CONNECTIONS.set(Mutex::new(document));
}

fn connections() -> &'static Mutex<ConnectionsDocument> {
    CONNECTIONS.get_or_init(|| Mutex::new(ConnectionsDocument::default()))
}

fn credential_cache() -> &'static Mutex<BTreeMap<String, StoredSourceCredential>> {
    CREDENTIAL_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn read_connections_document(path: &Path) -> ConnectionsDocument {
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<ConnectionsDocument>(&contents).ok())
        .unwrap_or_default()
}

/// A directory that cannot be written is not worth failing a sign-in over: the
/// connection still works this run, it just has to be re-checked next launch.
fn write_connections_document(document: &ConnectionsDocument) {
    let Some(path) = CONNECTIONS_PATH.get() else {
        return;
    };
    let Ok(json) = serde_json::to_string_pretty(document) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let temporary = path.with_extension("json.tmp");
    if fs::write(&temporary, json + "\n").is_ok() {
        let _ = fs::rename(&temporary, path);
    }
}

/// Record that a store is connected, under a key of the caller's choosing.
/// Steam predates this module and keeps its own credential, but it has exactly
/// the same macOS problem, so it shares the directory under the key `steam`.
pub fn remember_named_connection(key: &str, account_label: &str) {
    if let Ok(mut document) = connections().lock() {
        document.accounts.insert(
            key.to_string(),
            ConnectionRecord {
                account_label: account_label.to_string(),
            },
        );
        write_connections_document(&document);
    }
}

pub fn forget_named_connection(key: &str) {
    if let Ok(mut document) = connections().lock() {
        document.accounts.remove(key);
        write_connections_document(&document);
    }
    if let Ok(mut cache) = credential_cache().lock() {
        cache.remove(key);
    }
}

pub fn remembered_named_connection(key: &str) -> Option<String> {
    let document = connections().lock().ok()?;
    document
        .accounts
        .get(key)
        .map(|record| record.account_label.clone())
}

fn remember_connection(provider: SourceProvider, account_label: &str) {
    remember_named_connection(
        provider.credential_namespace().keychain_account(),
        account_label,
    );
}

fn forget_connection(provider: SourceProvider) {
    forget_named_connection(provider.credential_namespace().keychain_account());
}

fn remembered_connection(provider: SourceProvider) -> Option<String> {
    remembered_named_connection(provider.credential_namespace().keychain_account())
}

/// Read a store's connection state **without opening the keychain**.
///
/// This is what Settings renders. It is deliberately not an authority on
/// whether the token still works — only a sync can know that — and a sync that
/// finds the credential gone clears this entry.
pub fn status(provider: SourceProvider) -> Result<SourceAccountStatus, SourceError> {
    let mut status = status_from_credential(provider, None);
    if let Some(account_label) = remembered_connection(provider) {
        status.connected = true;
        status.account_label = account_label;
    }
    Ok(status)
}

pub fn status_from_credential(
    provider: SourceProvider,
    credential: Option<&StoredSourceCredential>,
) -> SourceAccountStatus {
    let siblings = provider
        .siblings()
        .into_iter()
        .filter(|sibling| *sibling != provider)
        .map(|sibling| sibling.token().to_string())
        .collect();
    SourceAccountStatus {
        provider: provider.token().to_string(),
        label: provider.label().to_string(),
        description: provider.description().to_string(),
        connected: credential.is_some(),
        account_label: credential
            .map(|credential| credential.account_label().to_string())
            .unwrap_or_default(),
        style: match provider.connect_style() {
            ConnectStyle::Token => "token".to_string(),
            ConnectStyle::Session => "session".to_string(),
        },
        shares_sign_in_with: siblings,
        launchable: provider.launchable(),
    }
}

/// Read the credential, opening the keychain at most once per store per run.
///
/// Every later call in the same session is served from memory, so a connect
/// followed by a sync — or several syncs in a row — costs exactly one macOS
/// keychain prompt instead of one per operation.
pub fn load_credential(
    provider: SourceProvider,
) -> Result<Option<StoredSourceCredential>, SourceError> {
    let key = provider
        .credential_namespace()
        .keychain_account()
        .to_string();
    if let Ok(cache) = credential_cache().lock()
        && let Some(credential) = cache.get(&key)
    {
        return Ok(Some(credential.clone()));
    }

    let entry = credential_entry(provider)?;
    let encoded = match entry.get_password() {
        Ok(value) => value,
        Err(KeyringError::NoEntry) => {
            // The directory said connected but the item is gone — the user
            // cleared their keychain. Stop claiming a connection that no sync
            // could ever complete.
            forget_connection(provider);
            return Ok(None);
        }
        Err(error) => return Err(keychain_error(provider, "read", error)),
    };
    match serde_json::from_str::<StoredSourceCredential>(&encoded) {
        Ok(credential) => {
            cache_credential(&key, &credential);
            // A credential found in the keychain is the authority: re-assert it
            // in the directory in case the file was lost or never written.
            remember_connection(provider, credential.account_label());
            Ok(Some(credential))
        }
        Err(_) => {
            // The stored value is never logged. Its shape alone is enough to
            // diagnose a damaged entry without exposing a provider token.
            eprintln!(
                "{} keychain credential has an invalid stored format",
                provider.label()
            );
            Err(SourceError::InvalidCredential(provider))
        }
    }
}

pub fn save_credential(
    provider: SourceProvider,
    credential: &StoredSourceCredential,
) -> Result<(), SourceError> {
    let encoded = serde_json::to_string(credential).map_err(|_| SourceError::Keychain(provider))?;
    credential_entry(provider)?
        .set_password(&encoded)
        .map_err(|error| keychain_error(provider, "write", error))?;
    let key = provider
        .credential_namespace()
        .keychain_account()
        .to_string();
    cache_credential(&key, credential);
    remember_connection(provider, credential.account_label());
    Ok(())
}

fn cache_credential(key: &str, credential: &StoredSourceCredential) {
    if let Ok(mut cache) = credential_cache().lock() {
        cache.insert(key.to_string(), credential.clone());
    }
}

pub fn require_credential(provider: SourceProvider) -> Result<StoredSourceCredential, SourceError> {
    load_credential(provider)?.ok_or(SourceError::NotConnected(provider))
}

pub fn disconnect(provider: SourceProvider) -> Result<(), SourceError> {
    // Forget first: even if the keychain refuses to open, the store must stop
    // reading as connected, or Settings would keep offering a sync that cannot
    // work and the user could never get out of it.
    forget_connection(provider);
    let entry = credential_entry(provider)?;
    match entry.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(keychain_error(provider, "delete", error)),
    }
}

fn credential_entry(provider: SourceProvider) -> Result<Entry, SourceError> {
    Entry::new(
        KEYRING_SERVICE,
        provider.credential_namespace().keychain_account(),
    )
    .map_err(|error| keychain_error(provider, "open", error))
}

fn keychain_error(provider: SourceProvider, operation: &str, error: KeyringError) -> SourceError {
    // `keyring::Error` carries an OS status category but never the stored
    // value, so retaining it in stderr makes a denied ACL or a locked keychain
    // observable from the native process without weakening secrecy.
    eprintln!("{} keychain {operation} failed: {error}", provider.label());
    SourceError::Keychain(provider)
}

/// The shared HTTP client every headless connector uses. Redirects are refused
/// so a bearer token can never be forwarded to another origin implicitly.
pub fn http_client(provider: SourceProvider) -> Result<reqwest::Client, SourceError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(USER_AGENT)
        .build()
        .map_err(|_| SourceError::Network(provider))
}

/// Map an HTTP status onto the one sentence the user should read. Anything
/// that is not an auth or throttling problem is reported as an unexpected
/// answer, because a 5xx from a store is not something the user can fix.
pub fn error_for_status(provider: SourceProvider, status: reqwest::StatusCode) -> SourceError {
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            SourceError::SessionExpired(provider)
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => SourceError::RateLimited(provider),
        _ => SourceError::UnexpectedResponse(provider),
    }
}

/// Accept an artwork URL only if it is HTTPS and its host is one this provider
/// is allowed to serve images from. Protocol-relative URLs (`//host/path`),
/// which GOG still returns, are upgraded to HTTPS before the check rather than
/// being silently dropped.
pub fn sanitize_media_url(url: &str, provider: SourceProvider) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.len() > 2_048 || trimmed.chars().any(char::is_control) {
        return None;
    }
    let absolute = if let Some(rest) = trimmed.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        trimmed.to_string()
    };
    let parsed = reqwest::Url::parse(&absolute).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    let allowed = provider
        .media_hosts()
        .iter()
        .any(|candidate| host == *candidate || host.ends_with(&format!(".{candidate}")));
    allowed.then(|| parsed.to_string())
}

/// Provider titles are display text, not identifiers: collapse whitespace,
/// bound the length, and refuse control characters outright.
pub fn normalize_title(value: &str) -> Option<String> {
    normalize_text(value, 512)
}

pub fn normalize_text(value: &str, max_characters: usize) -> Option<String> {
    // A control character is a boundary, not nothing: dropping it outright glued
    // the two halves of a store description together and shipped
    // "BUILD YOUR OWN VIKING LEGENDBecome Eivor" to the library. Turning it into
    // a space first lets the collapse below do the rest.
    let collapsed = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let collapsed = collapsed.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then(|| collapsed.chars().take(max_characters).collect())
}

/// The tags a store description uses to end one block of prose and start the
/// next. Everything else is inline and joins the words around it.
fn is_block_tag(tag: &str) -> bool {
    let name: String = tag
        .trim_start_matches('/')
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        name.as_str(),
        "br" | "p"
            | "div"
            | "li"
            | "ul"
            | "ol"
            | "tr"
            | "td"
            | "th"
            | "table"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "blockquote"
            | "hr"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
    )
}

/// Strip the HTML a store description can carry so the library never renders a
/// provider's markup. Tags are removed, not escaped, and the few entities that
/// actually appear are decoded.
pub fn normalize_html_text(value: &str, max_characters: usize) -> Option<String> {
    let mut plain = String::with_capacity(value.len());
    let mut tag = String::new();
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                // A tag that ended a block of prose leaves a space behind it, or
                // "</h3><p>" welds a heading onto the paragraph under it. An
                // inline tag leaves nothing: "word<b>s</b>" is one word, and a
                // space there would break it in half.
                if is_block_tag(&tag) {
                    plain.push(' ');
                }
            }
            _ if !in_tag => plain.push(character),
            _ => tag.push(character),
        }
    }
    let plain = plain
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    normalize_text(&plain, max_characters)
}

// ---------------------------------------------------------------------------
// Session-style sync
// ---------------------------------------------------------------------------
//
// Ubisoft Connect and Instant Gaming publish no account API, so their sync runs
// inside the authenticated sign-in window instead of in Rust. The window's own
// script reads that store's library from that store's own origin and leaves a
// compact result on `window.__orivoSourceSync`; Rust only ever reads that one
// object back. Nothing about the session — no cookie, ticket or header — is
// copied out, which is why this is safer than extracting a long-lived secret.
//
// The work is asynchronous and an evaluated expression cannot await, so the
// exchange is deliberately two-step: a start script kicks the fetch off, and a
// poll script reads the slot until it settles.

/// The single window property both session connectors publish their result on.
/// It is the contract the in-page scripts implement and `SESSION_POLL_SCRIPT`
/// reads; the tests assert the two agree, which is its only Rust-side use.
#[cfg_attr(not(test), allow(dead_code))]
pub const SESSION_RESULT_PROPERTY: &str = "__orivoSourceSync";

/// Read the slot back. Returns `{"status":"idle"}` when the start script has
/// not run in this document yet — a navigation resets it, and "idle" tells the
/// caller to start again rather than to report a failure.
pub const SESSION_POLL_SCRIPT: &str = r#"
JSON.stringify(window.__orivoSourceSync || { status: 'idle' })
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSyncState {
    /// The start script has not run in the current document.
    Idle,
    Pending,
    /// The user is not signed in yet. For a connect flow this simply means
    /// "keep waiting"; for a re-sync it means the session lapsed.
    SignedOut,
    Ready,
    /// The store answered, in a shape this build does not understand.
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct SessionSyncPayload {
    #[serde(default)]
    status: String,
    #[serde(default, rename = "accountLabel")]
    account_label: String,
    #[serde(default)]
    games: Vec<SessionSyncGame>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct SessionSyncGame {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    cover: String,
    #[serde(default)]
    hero: String,
    #[serde(default)]
    genre: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Clone)]
pub struct SessionSyncResult {
    pub state: SessionSyncState,
    pub account_label: String,
    pub library: SourceLibrary,
}

/// Parse the poll script's answer. Depending on the runtime, an evaluated
/// result can arrive as the JSON string the script returned or as that string
/// wrapped once more as a JSON value; both are accepted.
pub fn session_result_from_eval(provider: SourceProvider, result: &str) -> SessionSyncResult {
    let payload = serde_json::from_str::<serde_json::Value>(result)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| result.to_string());
    let Ok(payload) = serde_json::from_str::<SessionSyncPayload>(&payload) else {
        return SessionSyncResult {
            state: SessionSyncState::Failed,
            account_label: String::new(),
            library: SourceLibrary::default(),
        };
    };

    let state = match payload.status.as_str() {
        "idle" => SessionSyncState::Idle,
        "pending" => SessionSyncState::Pending,
        "signed-out" => SessionSyncState::SignedOut,
        "ok" => SessionSyncState::Ready,
        "unsupported" => SessionSyncState::Unsupported,
        _ => SessionSyncState::Failed,
    };

    let mut library = SourceLibrary::default();
    if state == SessionSyncState::Ready {
        for game in payload.games {
            library.push(
                provider,
                SourceLibraryGame {
                    source_id: game.id.clone(),
                    launch_ref: game.id,
                    title: game.title,
                    description: (!game.description.is_empty()).then_some(game.description),
                    genre: (!game.genre.is_empty()).then_some(game.genre),
                    developer: None,
                    logo_url: None,
                    cover_url: (!game.cover.is_empty()).then_some(game.cover.clone()),
                    hero_url: (!game.hero.is_empty())
                        .then_some(game.hero.clone())
                        .or_else(|| (!game.cover.is_empty()).then_some(game.cover.clone())),
                    landscape_url: (!game.hero.is_empty())
                        .then_some(game.hero)
                        .or_else(|| (!game.cover.is_empty()).then_some(game.cover)),
                    play_time_seconds: 0,
                    last_played_at: None,
                    native_mac: None,
                    platforms: Vec::new(),
                    install: None,
                },
            );
        }
        library.sort_by_title();
    }

    SessionSyncResult {
        state,
        account_label: normalize_text(&payload.account_label, 120)
            .unwrap_or_else(|| provider.label().to_string()),
        library,
    }
}

/// Turn a settled session state into the error the user should read. `Ready` is
/// not an error, so it maps to `None`.
pub fn session_error(provider: SourceProvider, state: SessionSyncState) -> Option<SourceError> {
    match state {
        SessionSyncState::Ready => None,
        SessionSyncState::SignedOut => Some(SourceError::SessionExpired(provider)),
        SessionSyncState::Unsupported => Some(SourceError::UnexpectedResponse(provider)),
        SessionSyncState::Idle | SessionSyncState::Pending | SessionSyncState::Failed => {
            Some(SourceError::Network(provider))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provider_token_round_trips_through_the_catalog_source() {
        for provider in SourceProvider::all() {
            assert_eq!(SourceProvider::from_token(provider.token()), Some(provider));
            assert_eq!(
                GameSource::from_provider_token(provider.token()),
                Some(provider.catalog_source())
            );
        }
    }

    #[test]
    fn rendering_settings_never_opens_the_keychain() {
        // This is the whole point of the connection directory. macOS asks for
        // the keychain password every time an unrecognised binary reads an
        // item, so a status call that touched the keychain made opening
        // Settings prompt once per store, every time.
        for provider in SourceProvider::all() {
            forget_connection(provider);
        }
        for provider in SourceProvider::all() {
            let status = status(provider).expect("a status never fails on a missing entry");
            assert!(!status.connected);
            assert_eq!(status.account_label, "");
        }

        remember_connection(SourceProvider::Epic, "player-one");
        let epic = status(SourceProvider::Epic).unwrap();
        assert!(epic.connected);
        assert_eq!(epic.account_label, "player-one");
        // Epic stands alone, so nothing else may be dragged along with it.
        assert!(!status(SourceProvider::Gog).unwrap().connected);

        // One sign-in connects both Microsoft surfaces, and forgetting either
        // signs the account out of both — the directory is keyed by the
        // credential, not by the surface.
        remember_connection(SourceProvider::Xbox, "Gamertag");
        assert!(status(SourceProvider::MicrosoftStore).unwrap().connected);
        forget_connection(SourceProvider::MicrosoftStore);
        assert!(!status(SourceProvider::Xbox).unwrap().connected);

        forget_connection(SourceProvider::Epic);
    }

    #[test]
    fn xbox_and_microsoft_store_share_one_microsoft_sign_in() {
        assert_eq!(
            SourceProvider::Xbox.credential_namespace(),
            SourceProvider::MicrosoftStore.credential_namespace()
        );
        assert_eq!(SourceProvider::Xbox.siblings().len(), 2);
        assert_eq!(SourceProvider::Epic.siblings(), vec![SourceProvider::Epic]);
    }

    #[test]
    fn artwork_urls_outside_a_provider_host_allowlist_are_dropped() {
        assert!(
            sanitize_media_url("https://cdn1.epicgames.com/a/b.png", SourceProvider::Epic)
                .is_some()
        );
        // GOG still answers with protocol-relative image URLs.
        assert_eq!(
            sanitize_media_url("//images-2.gog-statics.com/a.jpg", SourceProvider::Gog),
            Some("https://images-2.gog-statics.com/a.jpg".to_string())
        );
        assert!(
            sanitize_media_url("http://cdn1.epicgames.com/a.png", SourceProvider::Epic).is_none()
        );
        assert!(sanitize_media_url("https://evil.example/a.png", SourceProvider::Epic).is_none());
        // A host that merely *contains* an allowed name must not pass.
        assert!(
            sanitize_media_url("https://gog.com.evil.example/a.jpg", SourceProvider::Gog).is_none()
        );
    }

    #[test]
    fn a_malformed_provider_record_is_skipped_instead_of_failing_the_sync() {
        let mut library = SourceLibrary::default();
        library.push(
            SourceProvider::Gog,
            SourceLibraryGame {
                source_id: "1207658924".into(),
                launch_ref: "1207658924".into(),
                title: "  The Witcher   ".into(),
                description: None,
                genre: None,
                developer: None,
                logo_url: None,
                cover_url: None,
                hero_url: None,
                landscape_url: None,
                play_time_seconds: 0,
                last_played_at: None,
                native_mac: None,
                platforms: Vec::new(),
                install: None,
            },
        );
        library.push(
            SourceProvider::Gog,
            SourceLibraryGame {
                // A path separator can never become a launch reference.
                source_id: "../../etc/passwd".into(),
                launch_ref: "../../etc/passwd".into(),
                title: "Traversal".into(),
                description: None,
                genre: None,
                developer: None,
                logo_url: None,
                cover_url: None,
                hero_url: None,
                landscape_url: None,
                play_time_seconds: 0,
                last_played_at: None,
                native_mac: None,
                platforms: Vec::new(),
                install: None,
            },
        );
        library.push(
            SourceProvider::Gog,
            SourceLibraryGame {
                source_id: "12".into(),
                launch_ref: "12".into(),
                title: "   ".into(),
                description: None,
                genre: None,
                developer: None,
                logo_url: None,
                cover_url: None,
                hero_url: None,
                landscape_url: None,
                play_time_seconds: 0,
                last_played_at: None,
                native_mac: None,
                platforms: Vec::new(),
                install: None,
            },
        );

        assert_eq!(library.games.len(), 1);
        assert_eq!(library.games[0].title, "The Witcher");
        assert_eq!(library.skipped, 2);
    }

    #[test]
    fn a_duplicate_entitlement_does_not_break_the_catalog_identity_check() {
        let mut library = SourceLibrary::default();
        for title in ["Anno 1800", "Anno 1800 Deluxe"] {
            library.push(
                SourceProvider::Ubisoft,
                SourceLibraryGame {
                    source_id: "5416".into(),
                    launch_ref: "5416".into(),
                    title: title.into(),
                    description: None,
                    genre: None,
                    developer: None,
                    logo_url: None,
                    cover_url: None,
                    hero_url: None,
                    landscape_url: None,
                    play_time_seconds: 0,
                    last_played_at: None,
                    native_mac: None,
                    platforms: Vec::new(),
                    install: None,
                },
            );
        }

        assert_eq!(library.games.len(), 1);
        assert_eq!(library.skipped, 0);
    }

    #[test]
    fn store_descriptions_never_carry_markup_into_the_library() {
        assert_eq!(
            normalize_html_text("<p>Play <b>now</b> &amp; win</p>", 600).as_deref(),
            Some("Play now & win")
        );
    }

    #[test]
    fn a_description_keeps_the_break_between_its_headline_and_its_body() {
        // Xbox hands back a headline and a paragraph separated by newlines, and
        // the library used to show them welded together.
        assert_eq!(
            normalize_text("BUILD YOUR OWN VIKING LEGEND\r\n\r\nBecome Eivor.", 600).as_deref(),
            Some("BUILD YOUR OWN VIKING LEGEND Become Eivor.")
        );
        assert_eq!(
            normalize_html_text(
                "<h3>BUILD YOUR OWN VIKING LEGEND</h3><p>Become Eivor.</p>",
                600
            )
            .as_deref(),
            Some("BUILD YOUR OWN VIKING LEGEND Become Eivor.")
        );
        assert_eq!(
            normalize_html_text("Raid<br>Settle<br/>Conquer", 600).as_deref(),
            Some("Raid Settle Conquer")
        );
    }

    #[test]
    fn an_inline_tag_does_not_cut_a_word_in_half() {
        // Only block tags stand for a break: an inline one joins what it wraps,
        // and a title is not allowed to arrive as "Assassin s Creed".
        assert_eq!(
            normalize_html_text("Assassin<b>'s</b> Creed<sup>®</sup>", 600).as_deref(),
            Some("Assassin's Creed®")
        );
        assert_eq!(
            normalize_html_text("<p>Save <b>50</b>% today</p>", 600).as_deref(),
            Some("Save 50% today")
        );
    }

    #[test]
    fn a_session_result_is_read_from_the_window_slot_in_both_wire_shapes() {
        let direct = r#"{"status":"ok","accountLabel":"Player","games":[
            {"id":"5416","title":"Anno 1800","cover":"https://static3.cdn.ubi.com/a.jpg"},
            {"id":"../etc","title":"Traversal","cover":""},
            {"id":"742","title":"Rayman","cover":"https://evil.example/a.jpg"}
        ]}"#;
        let wrapped = serde_json::to_string(direct).unwrap();

        let result = session_result_from_eval(SourceProvider::Ubisoft, &wrapped);
        assert_eq!(result.state, SessionSyncState::Ready);
        assert_eq!(result.account_label, "Player");
        assert_eq!(result.library.games.len(), 2);
        assert_eq!(result.library.skipped, 1);
        assert_eq!(
            result.library.games[0].cover_url.as_deref(),
            Some("https://static3.cdn.ubi.com/a.jpg")
        );
        // An artwork host outside the allowlist is dropped, but the game the
        // user owns is still imported.
        assert_eq!(result.library.games[1].title, "Rayman");
        assert!(result.library.games[1].cover_url.is_none());
    }

    #[test]
    fn every_unsettled_session_state_maps_to_one_readable_sentence() {
        assert!(session_error(SourceProvider::Ubisoft, SessionSyncState::Ready).is_none());
        assert!(matches!(
            session_error(SourceProvider::Ubisoft, SessionSyncState::SignedOut),
            Some(SourceError::SessionExpired(_))
        ));
        assert!(matches!(
            session_error(SourceProvider::InstantGaming, SessionSyncState::Unsupported),
            Some(SourceError::UnexpectedResponse(_))
        ));
        assert_eq!(
            session_result_from_eval(SourceProvider::InstantGaming, "not json").state,
            SessionSyncState::Failed
        );
        assert_eq!(
            session_result_from_eval(SourceProvider::InstantGaming, r#"{"status":"idle"}"#).state,
            SessionSyncState::Idle
        );
    }

    #[test]
    fn an_unknown_expiry_forces_a_refresh_before_the_token_is_used() {
        let credential = StoredSourceCredential::OAuth {
            account_id: "1".into(),
            account_label: "Player".into(),
            access_token: "token".into(),
            refresh_token: "refresh".into(),
            expires_at_ms: 0,
        };
        assert!(credential.needs_refresh());

        let fresh = StoredSourceCredential::OAuth {
            account_id: "1".into(),
            account_label: "Player".into(),
            access_token: "token".into(),
            refresh_token: "refresh".into(),
            expires_at_ms: expiry_from_seconds(3_600),
        };
        assert!(!fresh.needs_refresh());
        assert!(
            !StoredSourceCredential::Session {
                account_label: "Player".into()
            }
            .needs_refresh()
        );
    }
}
