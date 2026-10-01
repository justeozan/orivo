//! Local Steam account credentials and owned-library retrieval.
//!
//! This module deliberately keeps Steam secrets out of the catalog and out of
//! Tauri's IPC payloads. The desktop login flow stores only the resulting
//! credential in the operating-system keychain; the frontend can learn the
//! connection state and library counts, never a token, cookie, or API key.

use crate::sources;
use futures_util::{StreamExt, stream};
use keyring::{Entry, Error as KeyringError};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Mutex, OnceLock},
    time::Duration,
};

// Version the service name whenever the keychain access contract changes.
// The original entry was created by unsigned development binaries whose
// ad-hoc identifier changes after every Rust rebuild, making macOS reject the
// next binary even though the item itself still exists.
const KEYRING_SERVICE: &str = "io.orivo.desktop.steam.v2";
const LEGACY_KEYRING_SERVICE: &str = "io.orivo.desktop.steam";
const KEYRING_ACCOUNT: &str = "primary-library";
/// Steam's entry in the shared, secret-free connection directory. Rendering
/// Settings reads this instead of the keychain, so opening the page no longer
/// makes macOS ask for the keychain password.
const CONNECTION_KEY: &str = "steam";
/// The keychain is opened at most once per run: a status check followed by a
/// sync would otherwise prompt twice.
static CREDENTIAL_CACHE: OnceLock<Mutex<Option<StoredSteamCredential>>> = OnceLock::new();

fn credential_cache() -> &'static Mutex<Option<StoredSteamCredential>> {
    CREDENTIAL_CACHE.get_or_init(|| Mutex::new(None))
}

/// Persona names resolved this run, keyed by steam id. A Settings repaint
/// asks for status often enough that one lookup per account per run is the
/// difference between a name and a hundred profile requests.
static PERSONA_CACHE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();

fn persona_cache() -> &'static Mutex<BTreeMap<String, String>> {
    PERSONA_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}
const OWNED_GAMES_ENDPOINT: &str = "https://api.steampowered.com/IPlayerService/GetOwnedGames/v1/";
/// Steam's public profile view: the one profile lookup that needs no
/// credential, so reading a display name never opens the keychain.
const PROFILE_ENDPOINT: &str = "https://steamcommunity.com/profiles/";
const STORE_APP_DETAILS_ENDPOINT: &str = "https://store.steampowered.com/api/appdetails";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const PERSONA_REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const STORE_REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_RETRIES: usize = 3;
const MAX_CONCURRENT_STORE_REQUESTS: usize = 4;
const MAX_STEAM_ID_LENGTH: usize = 20;
const MAX_SECRET_LENGTH: usize = 4_096;
/// Steam caps a persona name far below this; anything longer is a document
/// that was not a profile.
const MAX_PERSONA_NAME_LENGTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedSteamGame {
    pub app_id: u32,
    pub title: String,
    pub play_time_seconds: u64,
}

/// Public, presentation-safe metadata returned by Steam's Store endpoint.
/// This request contains no account credential and is best-effort: a delisted
/// app must never make an otherwise valid owned-library sync fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamStoreGameMetadata {
    pub app_id: u32,
    pub short_description: Option<String>,
    pub genre: Option<String>,
    pub platforms: Option<SteamStorePlatforms>,
}

/// Steam Store's public availability flags. Keep the wire shape separate from
/// the application host so the matching decision always happens locally.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SteamStorePlatforms {
    pub windows: bool,
    pub macos: bool,
    pub linux: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamAccountStatus {
    pub connected: bool,
    pub steam_id: String,
    pub method: String,
    /// The profile's display name, resolved without any credential. Empty
    /// whenever Steam has nothing to show, in which case the id stands in.
    pub persona_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamAccountConnectedEvent {
    pub steam_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "method", rename_all = "snake_case")]
enum StoredSteamCredential {
    Web {
        steam_id: String,
        access_token: String,
    },
    ApiKey {
        steam_id: String,
        api_key: String,
    },
}

impl StoredSteamCredential {
    fn steam_id(&self) -> &str {
        match self {
            Self::Web { steam_id, .. } | Self::ApiKey { steam_id, .. } => steam_id,
        }
    }

    fn method(&self) -> &'static str {
        match self {
            Self::Web { .. } => "web",
            Self::ApiKey { .. } => "api_key",
        }
    }
}

#[derive(Debug)]
pub enum SteamAccountError {
    NotConnected,
    Keychain,
    InvalidCredential,
    SessionExpired,
    RateLimited,
    Network,
    InvalidResponse,
}

impl fmt::Display for SteamAccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConnected => f.write_str("Connect a Steam account before syncing its library."),
            Self::Keychain => f.write_str(
                "Orivo could not access the secure Steam connection in the system keychain. Connect Steam again.",
            ),
            Self::InvalidCredential => f.write_str("Steam rejected the account connection. Connect Steam again."),
            Self::SessionExpired => f.write_str(
                "Your Steam session expired. Reconnect Steam, then try the sync again.",
            ),
            Self::RateLimited => f.write_str(
                "Steam is temporarily limiting library requests. Try syncing again in a moment.",
            ),
            Self::Network => f.write_str("Steam could not be reached. Check your connection and try again."),
            Self::InvalidResponse => f.write_str(
                "Steam returned an unreadable library response. Reconnect Steam and try again.",
            ),
        }
    }
}

impl std::error::Error for SteamAccountError {}

#[derive(Debug, Deserialize)]
struct OwnedGamesEnvelope {
    #[serde(default)]
    response: OwnedGamesResponse,
}

#[derive(Debug, Default, Deserialize)]
struct OwnedGamesResponse {
    #[serde(default)]
    games: Vec<OwnedGameResponse>,
}

#[derive(Debug, Deserialize)]
struct OwnedGameResponse {
    #[serde(rename = "appid")]
    app_id: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    playtime_forever: u64,
}

#[derive(Debug, Deserialize)]
struct StoreAppDetailsEntry {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    data: Option<StoreAppDetails>,
}

#[derive(Debug, Default, Deserialize)]
struct StoreAppDetails {
    #[serde(default)]
    short_description: String,
    #[serde(default)]
    genres: Vec<StoreGenre>,
    #[serde(default)]
    platforms: Option<StorePlatforms>,
}

#[derive(Debug, Deserialize)]
struct StoreGenre {
    #[serde(default)]
    description: String,
}

#[derive(Debug, Default, Deserialize)]
struct StorePlatforms {
    #[serde(default)]
    windows: bool,
    #[serde(default, rename = "mac")]
    macos: bool,
    #[serde(default)]
    linux: bool,
}

#[derive(Debug, Deserialize)]
struct WebLoginPayload {
    #[serde(rename = "steamId")]
    steam_id: String,
    #[serde(rename = "accessToken")]
    access_token: String,
}

/// Report the connection **without opening the keychain**.
///
/// The steam id and method are recorded in the secret-free directory when the
/// account is connected, so the Settings page can render from that. Only a real
/// sync, which needs the token itself, reaches the keychain.
pub fn account_status() -> Result<SteamAccountStatus, SteamAccountError> {
    if let Some(credential) = cached_credential() {
        return Ok(connected_status(&credential));
    }
    if let Some(record) = sources::remembered_named_connection(CONNECTION_KEY) {
        let (steam_id, method) = record
            .split_once('\u{1}')
            .unwrap_or((record.as_str(), "web"));
        return Ok(SteamAccountStatus {
            connected: true,
            steam_id: steam_id.to_string(),
            method: method.to_string(),
            persona_name: String::new(),
        });
    }
    Ok(SteamAccountStatus {
        connected: false,
        steam_id: String::new(),
        method: String::new(),
        persona_name: String::new(),
    })
}

fn connected_status(credential: &StoredSteamCredential) -> SteamAccountStatus {
    SteamAccountStatus {
        connected: true,
        steam_id: credential.steam_id().to_string(),
        method: credential.method().to_string(),
        persona_name: String::new(),
    }
}

/// Add the profile's display name to a status report, best effort.
///
/// The lookup rides on Steam's public profile view rather than on the
/// credential, so it never opens the keychain and never asks the user for
/// anything. A private profile, a missing profile, or an offline Steam all
/// read as "no name": the report still comes back, with the raw id as it was
/// before.
pub async fn resolve_persona_name(mut status: SteamAccountStatus) -> SteamAccountStatus {
    if !status.connected || status.steam_id.is_empty() {
        return status;
    }
    if let Some(name) = persona_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&status.steam_id).cloned())
    {
        status.persona_name = name;
        return status;
    }
    let name = fetch_persona_name(&status.steam_id)
        .await
        .unwrap_or_default();
    if !name.is_empty() {
        if let Ok(mut cache) = persona_cache().lock() {
            cache.insert(status.steam_id.clone(), name.clone());
        }
    }
    status.persona_name = name;
    status
}

async fn fetch_persona_name(steam_id: &str) -> Option<String> {
    // The id reaches the URL, so it is validated the same way a credential is.
    let steam_id = validate_steam_id(steam_id).ok()?;
    let client = reqwest::Client::builder()
        .timeout(PERSONA_REQUEST_TIMEOUT)
        .user_agent("Orivo/0.2 Steam library sync")
        .build()
        .ok()?;
    let response = client
        .get(format!("{PROFILE_ENDPOINT}{steam_id}?xml=1"))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    persona_name_from_profile(&response.text().await.ok()?, &steam_id)
}

/// Read the persona name out of Steam's public profile view.
///
/// A visible profile answers with a small XML document that names the account
/// it belongs to; a private or missing profile comes back as an `<error>`
/// document, and a sign-in wall comes back as HTML. Neither repeats the id the
/// request asked for, so both read as "no name".
fn persona_name_from_profile(body: &str, steam_id: &str) -> Option<String> {
    if profile_tag(body, "error").is_some() {
        return None;
    }
    if profile_tag(body, "steamID64")?.trim() != steam_id {
        return None;
    }
    sanitize_persona_name(&profile_tag(body, "steamID")?)
}

/// The text between `<tag>` and `</tag>`, with one round of CDATA unwrapped.
fn profile_tag(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let rest = body.get(body.find(&open)? + open.len()..)?;
    let raw = rest.get(..rest.find(&close)?)?.trim();
    let text = raw
        .strip_prefix("<![CDATA[")
        .and_then(|value| value.strip_suffix("]]>"))
        .unwrap_or(raw);
    Some(text.to_string())
}

fn sanitize_persona_name(raw: &str) -> Option<String> {
    let collapsed = raw
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() || collapsed.chars().count() > MAX_PERSONA_NAME_LENGTH {
        return None;
    }
    Some(collapsed)
}

fn cached_credential() -> Option<StoredSteamCredential> {
    credential_cache().lock().ok()?.clone()
}

fn remember_connection(credential: &StoredSteamCredential) {
    if let Ok(mut cache) = credential_cache().lock() {
        *cache = Some(credential.clone());
    }
    sources::remember_named_connection(
        CONNECTION_KEY,
        &format!("{}\u{1}{}", credential.steam_id(), credential.method()),
    );
}

/// Parse the JSON returned by Tauri's `eval_with_callback`. Depending on the
/// runtime, the callback can receive either the JSON string returned by our
/// script or that string wrapped once more as a JSON value.
pub fn web_login_from_eval(result: &str) -> Option<(String, String)> {
    let payload = serde_json::from_str::<serde_json::Value>(result)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| result.to_string());
    let payload = serde_json::from_str::<WebLoginPayload>(&payload).ok()?;
    let steam_id = validate_steam_id(&payload.steam_id).ok()?;
    let access_token = validate_web_token(&payload.access_token).ok()?;
    Some((steam_id, access_token))
}

/// Android's WebView answers neither `WebviewWindow::url` nor an eval callback
/// once the sign-in window has navigated, so the Store page hands the login
/// back through a navigation to a host that resolves nowhere. The navigation
/// handler reads it and blocks the load, so nothing leaves the device.
pub const ANDROID_RELAY_HOST: &str = "orivo.invalid";
pub const ANDROID_RELAY_PATH: &str = "/steam-login";

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn web_login_from_relay_url(url: &reqwest::Url) -> Option<(String, String)> {
    if url.scheme() != "https"
        || url.host_str() != Some(ANDROID_RELAY_HOST)
        || url.path() != ANDROID_RELAY_PATH
    {
        return None;
    }
    let mut steam_id = String::new();
    let mut access_token = String::new();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "steamid" => steam_id = value.into_owned(),
            "token" => access_token = value.into_owned(),
            _ => {}
        }
    }
    let steam_id = validate_steam_id(&steam_id).ok()?;
    let access_token = validate_web_token(&access_token).ok()?;
    Some((steam_id, access_token))
}

pub fn save_web_login(steam_id: String, access_token: String) -> Result<(), SteamAccountError> {
    let steam_id = validate_steam_id(&steam_id)?;
    let access_token = validate_web_token(&access_token)?;
    save_credential(StoredSteamCredential::Web {
        steam_id,
        access_token,
    })
}

/// Check a supplied API key with Steam before it replaces an existing secure
/// connection. This avoids persisting an invalid key (or overwriting a valid
/// web login) just because the user mistyped one character.
pub async fn connect_api_key(
    steam_id: String,
    api_key: String,
) -> Result<SteamAccountStatus, SteamAccountError> {
    let credential = api_key_credential(steam_id, api_key)?;
    fetch_owned_games_for_credential(&credential).await?;
    let steam_id = credential.steam_id().to_string();
    save_credential(credential)?;
    Ok(resolve_persona_name(SteamAccountStatus {
        connected: true,
        steam_id,
        method: "api_key".to_string(),
        persona_name: String::new(),
    })
    .await)
}

pub fn disconnect() -> Result<(), SteamAccountError> {
    // Forget first, so a keychain that will not open cannot leave Steam stuck
    // reading as connected with no way back.
    if let Ok(mut cache) = credential_cache().lock() {
        *cache = None;
    }
    sources::forget_named_connection(CONNECTION_KEY);
    let entry = credential_entry(KEYRING_SERVICE)?;
    match entry.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(error) => Err(keychain_error("delete", error)),
    }
}

pub async fn fetch_owned_games() -> Result<Vec<OwnedSteamGame>, SteamAccountError> {
    let credential = load_credential()?.ok_or(SteamAccountError::NotConnected)?;
    fetch_owned_games_for_credential(&credential).await
}

/// Fetch Store metadata with a small concurrency limit so a larger library
/// remains responsive and Steam is not flooded with one request per app at
/// once. Missing or unavailable store pages simply retain the owned-game
/// fallback already returned by `GetOwnedGames`.
pub async fn fetch_store_metadata(
    app_ids: impl IntoIterator<Item = u32>,
) -> BTreeMap<u32, SteamStoreGameMetadata> {
    let ids = app_ids
        .into_iter()
        .filter(|app_id| *app_id > 0)
        .collect::<std::collections::BTreeSet<_>>();
    if ids.is_empty() {
        return BTreeMap::new();
    }

    let Ok(client) = reqwest::Client::builder()
        .timeout(STORE_REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Orivo/0.2 Steam library metadata")
        .build()
    else {
        return BTreeMap::new();
    };

    let mut pending = stream::iter(ids.into_iter().map(|app_id| {
        let client = client.clone();
        async move { fetch_store_metadata_for_app(&client, app_id).await }
    }))
    .buffer_unordered(MAX_CONCURRENT_STORE_REQUESTS);
    let mut metadata = BTreeMap::new();
    while let Some(result) = pending.next().await {
        if let Some(game) = result {
            metadata.insert(game.app_id, game);
        }
    }
    metadata
}

async fn fetch_owned_games_for_credential(
    credential: &StoredSteamCredential,
) -> Result<Vec<OwnedSteamGame>, SteamAccountError> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        // Steam expects these credentials in the query string. A redirect to
        // another origin must never forward them implicitly.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Orivo/0.2 Steam library sync")
        .build()
        .map_err(|_| SteamAccountError::Network)?;

    let mut last_error = SteamAccountError::Network;
    for attempt in 0..MAX_RETRIES {
        let mut parameters = vec![
            ("steamid", credential.steam_id().to_string()),
            ("include_appinfo", "1".into()),
            ("include_played_free_games", "1".into()),
            ("include_free_sub", "1".into()),
            ("skip_unvetted_apps", "0".into()),
            ("language", "english".into()),
        ];
        match &credential {
            StoredSteamCredential::Web { access_token, .. } => {
                parameters.push(("access_token", access_token.clone()));
            }
            StoredSteamCredential::ApiKey { api_key, .. } => {
                parameters.push(("key", api_key.clone()));
            }
        }

        let response = match client
            .get(OWNED_GAMES_ENDPOINT)
            .query(&parameters)
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                last_error = SteamAccountError::Network;
                if attempt + 1 < MAX_RETRIES {
                    retry_delay(attempt).await;
                    continue;
                }
                return Err(last_error);
            }
        };

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            last_error = SteamAccountError::RateLimited;
            if attempt + 1 < MAX_RETRIES {
                retry_delay(attempt).await;
                continue;
            }
            return Err(last_error);
        }
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(match credential {
                StoredSteamCredential::Web { .. } => SteamAccountError::SessionExpired,
                StoredSteamCredential::ApiKey { .. } => SteamAccountError::InvalidCredential,
            });
        }
        if !response.status().is_success() {
            return Err(SteamAccountError::InvalidResponse);
        }

        let payload = response
            .json::<OwnedGamesEnvelope>()
            .await
            .map_err(|_| SteamAccountError::InvalidResponse)?;
        return Ok(normalize_owned_games(payload.response.games));
    }

    Err(last_error)
}

async fn retry_delay(attempt: usize) {
    let seconds = 1_u64 << attempt.min(2);
    tokio::time::sleep(Duration::from_secs(seconds)).await;
}

fn normalize_owned_games(games: Vec<OwnedGameResponse>) -> Vec<OwnedSteamGame> {
    let mut games_by_id = BTreeMap::new();
    for game in games {
        if game.app_id == 0 {
            continue;
        }
        let title = game.name.trim();
        let title = if title.is_empty() {
            format!("Steam app {}", game.app_id)
        } else {
            title.chars().take(512).collect()
        };
        games_by_id.insert(
            game.app_id,
            OwnedSteamGame {
                app_id: game.app_id,
                title,
                play_time_seconds: game.playtime_forever.saturating_mul(60),
            },
        );
    }

    let mut games = games_by_id.into_values().collect::<Vec<_>>();
    games.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| left.app_id.cmp(&right.app_id))
    });
    games
}

async fn fetch_store_metadata_for_app(
    client: &reqwest::Client,
    app_id: u32,
) -> Option<SteamStoreGameMetadata> {
    let response = client
        .get(STORE_APP_DETAILS_ENDPOINT)
        .query(&[
            ("appids", app_id.to_string()),
            ("l", "english".into()),
            ("cc", "US".into()),
        ])
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload = response
        .json::<BTreeMap<String, StoreAppDetailsEntry>>()
        .await
        .ok()?;
    metadata_from_store_payload(app_id, payload)
}

fn metadata_from_store_payload(
    app_id: u32,
    payload: BTreeMap<String, StoreAppDetailsEntry>,
) -> Option<SteamStoreGameMetadata> {
    let entry = payload.get(&app_id.to_string())?;
    if !entry.success {
        return None;
    }
    let data = entry.data.as_ref()?;
    let short_description = normalize_store_text(&data.short_description);
    let genre = data
        .genres
        .iter()
        .find_map(|genre| normalize_store_text(&genre.description));
    let platforms = data
        .platforms
        .as_ref()
        .map(|platforms| SteamStorePlatforms {
            windows: platforms.windows,
            macos: platforms.macos,
            linux: platforms.linux,
        });
    (short_description.is_some() || genre.is_some() || platforms.is_some()).then_some(
        SteamStoreGameMetadata {
            app_id,
            short_description,
            genre,
            platforms,
        },
    )
}

fn normalize_store_text(value: &str) -> Option<String> {
    let mut plain = String::with_capacity(value.len());
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => plain.push(character),
            _ => {}
        }
    }
    let plain = plain
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&nbsp;", " ");
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    (!plain.is_empty()).then(|| plain.chars().take(600).collect())
}

fn load_credential() -> Result<Option<StoredSteamCredential>, SteamAccountError> {
    if let Some(credential) = cached_credential() {
        return Ok(Some(credential));
    }
    match load_credential_from(KEYRING_SERVICE) {
        Ok(Some(credential)) => {
            remember_connection(&credential);
            Ok(Some(credential))
        }
        Ok(None) => {
            let recovered =
                legacy_credential_or_disconnected(load_credential_from(LEGACY_KEYRING_SERVICE))?;
            match recovered {
                Some(credential) => {
                    remember_connection(&credential);
                    Ok(Some(credential))
                }
                None => {
                    // The directory claimed a connection the keychain does not
                    // have. Stop offering a sync that could never complete.
                    sources::forget_named_connection(CONNECTION_KEY);
                    Ok(None)
                }
            }
        }
        Err(error) => Err(error),
    }
}

fn legacy_credential_or_disconnected(
    legacy: Result<Option<StoredSteamCredential>, SteamAccountError>,
) -> Result<Option<StoredSteamCredential>, SteamAccountError> {
    match legacy {
        Ok(Some(credential)) => {
            // Keep a valid legacy connection when macOS still authorizes it,
            // but move it to the stable v2 namespace for future runs.
            save_credential(credential.clone())?;
            Ok(Some(credential))
        }
        Ok(None) => Ok(None),
        Err(error) => {
            // A prior unsigned development build can leave a Keychain ACL
            // that this build is not allowed to read or replace. Treat it as
            // disconnected so the user can sign in into the v2 item.
            eprintln!(
                "Steam keychain legacy credential is unavailable; allowing a fresh connection: {error}"
            );
            Ok(None)
        }
    }
}

fn load_credential_from(service: &str) -> Result<Option<StoredSteamCredential>, SteamAccountError> {
    let entry = credential_entry(service)?;
    let encoded = match entry.get_password() {
        Ok(value) => value,
        Err(KeyringError::NoEntry) => return Ok(None),
        Err(error) => return Err(keychain_error("read", error)),
    };
    let credential = serde_json::from_str::<StoredSteamCredential>(&encoded).map_err(|_| {
        // The stored value is deliberately never logged. Its shape is enough
        // to diagnose a damaged entry without exposing a Steam token or key.
        eprintln!("Steam keychain credential has an invalid stored format");
        SteamAccountError::InvalidCredential
    })?;
    Ok(Some(credential))
}

fn save_credential(credential: StoredSteamCredential) -> Result<(), SteamAccountError> {
    let encoded = serde_json::to_string(&credential).map_err(|_| SteamAccountError::Keychain)?;
    credential_entry(KEYRING_SERVICE)?
        .set_password(&encoded)
        .map_err(|error| keychain_error("write", error))?;
    remember_connection(&credential);
    Ok(())
}

fn api_key_credential(
    steam_id: String,
    api_key: String,
) -> Result<StoredSteamCredential, SteamAccountError> {
    let steam_id = validate_steam_id(&steam_id)?;
    let api_key = api_key.trim();
    if api_key.len() != 32 || !api_key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SteamAccountError::InvalidCredential);
    }
    Ok(StoredSteamCredential::ApiKey {
        steam_id,
        api_key: api_key.to_ascii_uppercase(),
    })
}

fn credential_entry(service: &str) -> Result<Entry, SteamAccountError> {
    Entry::new(service, KEYRING_ACCOUNT).map_err(|error| keychain_error("open", error))
}

fn keychain_error(operation: &str, error: KeyringError) -> SteamAccountError {
    // `keyring::Error` includes an OSStatus category but never the password
    // value. Retaining it in stderr makes a denied ACL or a locked keychain
    // observable from the native-process logs without weakening secrecy.
    eprintln!("Steam keychain {operation} failed: {error}");
    SteamAccountError::Keychain
}

fn validate_steam_id(value: &str) -> Result<String, SteamAccountError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_STEAM_ID_LENGTH
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || value.parse::<u64>().ok().filter(|id| *id > 0).is_none()
    {
        return Err(SteamAccountError::InvalidCredential);
    }
    Ok(value.to_string())
}

fn validate_web_token(value: &str) -> Result<String, SteamAccountError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_SECRET_LENGTH
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(SteamAccountError::InvalidCredential);
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_direct_and_json_wrapped_web_login_results() {
        let direct = r#"{"steamId":"76561198000000000","accessToken":"token-value"}"#;
        let wrapped = serde_json::to_string(direct).unwrap();

        assert_eq!(
            web_login_from_eval(direct),
            Some(("76561198000000000".into(), "token-value".into()))
        );
        assert_eq!(
            web_login_from_eval(&wrapped),
            Some(("76561198000000000".into(), "token-value".into()))
        );
    }

    #[test]
    fn a_web_login_is_read_only_from_the_relay_navigation_orivo_owns() {
        let relay = reqwest::Url::parse(
            "https://orivo.invalid/steam-login?steamid=76561198000000000&token=token-value",
        )
        .unwrap();
        let elsewhere = reqwest::Url::parse(
            "https://evil.example/steam-login?steamid=76561198000000000&token=token-value",
        )
        .unwrap();

        assert_eq!(
            web_login_from_relay_url(&relay),
            Some(("76561198000000000".into(), "token-value".into()))
        );
        assert!(web_login_from_relay_url(&elsewhere).is_none());
        assert!(
            web_login_from_relay_url(
                &reqwest::Url::parse("https://orivo.invalid/steam-login?steamid=nope&token=t")
                    .unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn rejects_malformed_or_oversized_web_login_secrets() {
        assert!(web_login_from_eval("not json").is_none());
        assert!(
            web_login_from_eval(r#"{"steamId":"invalid","accessToken":"token-value"}"#).is_none()
        );
        assert!(validate_web_token(&"x".repeat(MAX_SECRET_LENGTH + 1)).is_err());
    }

    #[test]
    fn inaccessible_legacy_keychain_entry_does_not_block_a_fresh_connection() {
        let recovered = legacy_credential_or_disconnected(Err(SteamAccountError::Keychain))
            .expect("an inaccessible legacy item should be recoverable by reconnecting");
        assert!(recovered.is_none());
    }

    #[test]
    fn validates_an_api_key_before_it_can_replace_a_saved_connection() {
        let credential = api_key_credential(
            "76561198000000000".into(),
            "aBcDeF0123456789aBcDeF0123456789".into(),
        )
        .unwrap();

        assert!(matches!(
            credential,
            StoredSteamCredential::ApiKey { ref api_key, .. }
                if api_key == "ABCDEF0123456789ABCDEF0123456789"
        ));
        assert!(api_key_credential("76561198000000000".into(), "not-a-key".into()).is_err());
    }

    #[test]
    fn reads_a_persona_name_out_of_a_public_profile_document() {
        let body = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><profile>
            <steamID64>76561197960435530</steamID64>
            <steamID><![CDATA[Robin]]></steamID>
            <onlineState>offline</onlineState>
            <privacyState>public</privacyState>
        </profile>"#;

        assert_eq!(
            persona_name_from_profile(body, "76561197960435530"),
            Some("Robin".to_string())
        );
    }

    #[test]
    fn reads_a_plain_text_persona_name_and_collapses_its_whitespace() {
        let body = "<profile><steamID64>76561198000000000</steamID64>\
                    <steamID>  Two  Words \n Here </steamID></profile>";

        assert_eq!(
            persona_name_from_profile(body, "76561198000000000"),
            Some("Two Words Here".to_string())
        );
    }

    #[test]
    fn lends_no_name_to_anything_that_is_not_the_profile_asked_for() {
        // A private or missing profile comes back as an error document.
        let error =
            "<profile><error><![CDATA[The specified profile is private.]]></error></profile>";
        assert_eq!(persona_name_from_profile(error, "76561198000000000"), None);

        // Another account's document never lends its name.
        let other = "<profile><steamID64>76561198000000001</steamID64>\
                     <steamID>Someone Else</steamID></profile>";
        assert_eq!(persona_name_from_profile(other, "76561198000000000"), None);

        // A sign-in wall is HTML: it repeats no id at all.
        assert_eq!(
            persona_name_from_profile(
                "<html><head><title>Sign In</title></head></html>",
                "76561198000000000"
            ),
            None
        );

        // Whitespace alone collapses to nothing, which is not a name.
        let blank = "<profile><steamID64>76561198000000000</steamID64>\
                     <steamID><![CDATA[   ]]></steamID></profile>";
        assert_eq!(persona_name_from_profile(blank, "76561198000000000"), None);

        // Neither is a run longer than Steam would ever issue.
        let oversized = format!(
            "<profile><steamID64>76561198000000000</steamID64><steamID>{}</steamID></profile>",
            "x".repeat(MAX_PERSONA_NAME_LENGTH + 1)
        );
        assert_eq!(
            persona_name_from_profile(&oversized, "76561198000000000"),
            None
        );
    }

    #[test]
    fn normalizes_owned_games_without_duplicate_ids() {
        let games = normalize_owned_games(vec![
            OwnedGameResponse {
                app_id: 20,
                name: "Team Fortress Classic".into(),
                playtime_forever: 3,
            },
            OwnedGameResponse {
                app_id: 10,
                name: String::new(),
                playtime_forever: 0,
            },
            OwnedGameResponse {
                app_id: 20,
                name: "Team Fortress Classic refreshed".into(),
                playtime_forever: 4,
            },
        ]);

        assert_eq!(games.len(), 2);
        assert_eq!(games[0].title, "Steam app 10");
        assert_eq!(games[1].title, "Team Fortress Classic refreshed");
        assert_eq!(games[1].play_time_seconds, 240);
    }

    #[test]
    fn extracts_a_real_store_genre_and_plain_short_description() {
        let payload = serde_json::from_str(
            r#"{
              "2698940": {
                "success": true,
                "data": {
                  "short_description": "Welcome to <strong>Motorfest</strong> &amp; drive.",
                  "genres": [{"description": "Racing"}],
                  "platforms": {"windows": true, "mac": true, "linux": false}
                }
              }
            }"#,
        )
        .unwrap();

        let metadata = metadata_from_store_payload(2_698_940, payload).unwrap();
        assert_eq!(metadata.genre.as_deref(), Some("Racing"));
        assert_eq!(
            metadata.short_description.as_deref(),
            Some("Welcome to Motorfest & drive.")
        );
        assert_eq!(
            metadata.platforms,
            Some(SteamStorePlatforms {
                windows: true,
                macos: true,
                linux: false,
            })
        );
    }
}
