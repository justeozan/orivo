//! Microsoft account connector, shared by the Xbox and Microsoft Store
//! libraries.
//!
//! Both are the same entitlement list seen from two sides, so they are backed
//! by one sign-in and one keychain entry. Xbox shows what you have played on a
//! console; Microsoft Store shows the PC half of the same account, which is
//! what Game Pass and Store purchases land in.
//!
//! The sign-in uses Microsoft's implicit desktop flow with the public Xbox
//! client id, exactly as the console companion apps do. Rust then performs the
//! two Xbox Live handshakes (user authenticate, then XSTS) on every sync, so
//! only the long-lived Microsoft token is ever persisted — the Xbox tokens are
//! derived, short-lived and never leave the process.

use crate::sources::{
    self, SourceError, SourceLibrary, SourceLibraryGame, SourceProvider, StoredSourceCredential,
};
use serde::{Deserialize, Serialize};

/// Errors are reported against whichever library the user asked for, so the
/// message names the surface they are looking at.
const CREDENTIAL_PROVIDER: SourceProvider = SourceProvider::Xbox;
const CLIENT_ID: &str = "000000004C12AE6F";
const REDIRECT_URI: &str = "https://login.live.com/oauth20_desktop.srf";
const SCOPE: &str = "service::user.auth.xboxlive.com::MBI_SSL";
const TOKEN_ENDPOINT: &str = "https://login.live.com/oauth20_token.srf";
const USER_AUTHENTICATE_ENDPOINT: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTHORIZE_ENDPOINT: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const TITLE_HISTORY_HOST: &str = "https://titlehub.xboxlive.com";
const MAX_TOKEN_LENGTH: usize = 8_192;

/// Read the implicit-flow response Microsoft leaves in the redirect fragment.
/// A fragment never reaches a server, which is why this one value has to be
/// read in the page; it is returned to Rust and goes straight to the keychain.
pub const TOKEN_EXTRACTION_SCRIPT: &str = r#"
(() => {
  const hash = window.location.hash || '';
  const parameters = new URLSearchParams(hash.startsWith('#') ? hash.slice(1) : hash);
  return JSON.stringify({
    accessToken: parameters.get('access_token') || '',
    refreshToken: parameters.get('refresh_token') || '',
    expiresIn: parameters.get('expires_in') || ''
  });
})()
"#;

pub fn login_url() -> String {
    format!(
        "https://login.live.com/oauth20_authorize.srf?client_id={CLIENT_ID}&redirect_uri={}&response_type=token&display=touch&scope={}&locale=en",
        urlencode(REDIRECT_URI),
        urlencode(SCOPE)
    )
}

/// The single page a token may be read from.
pub fn is_redirect_page(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("login.live.com")
        && url.path() == "/oauth20_desktop.srf"
}

#[derive(Debug, Deserialize)]
struct ImplicitTokenPayload {
    #[serde(default, rename = "accessToken")]
    access_token: String,
    #[serde(default, rename = "refreshToken")]
    refresh_token: String,
    #[serde(default, rename = "expiresIn")]
    expires_in: String,
}

/// A Microsoft token as the sign-in window returned it, before any Xbox
/// handshake has confirmed that the account can actually be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplicitToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

pub fn token_from_eval(result: &str) -> Option<ImplicitToken> {
    let payload = serde_json::from_str::<serde_json::Value>(result)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| result.to_string());
    let payload = serde_json::from_str::<ImplicitTokenPayload>(&payload).ok()?;
    let access_token = validate_token(&payload.access_token)?;
    Some(ImplicitToken {
        access_token,
        refresh_token: validate_token(&payload.refresh_token).unwrap_or_default(),
        expires_in: payload.expires_in.trim().parse::<u64>().unwrap_or(0),
    })
}

fn validate_token(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= MAX_TOKEN_LENGTH
        && !value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace()))
    .then(|| value.to_string())
}

/// Confirm the account with Xbox Live before the credential replaces a working
/// connection, and use the gamertag it returns as the display name.
pub async fn connect(token: ImplicitToken) -> Result<StoredSourceCredential, SourceError> {
    let client = sources::http_client(CREDENTIAL_PROVIDER)?;
    let identity = authorize(&client, &token.access_token, CREDENTIAL_PROVIDER).await?;
    let credential = StoredSourceCredential::OAuth {
        account_id: identity.xuid,
        account_label: sources::normalize_text(&identity.gamertag, 120)
            .unwrap_or_else(|| "Microsoft account".to_string()),
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at_ms: sources::expiry_from_seconds(token.expires_in),
    };
    sources::save_credential(CREDENTIAL_PROVIDER, &credential)?;
    Ok(credential)
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: u64,
}

async fn access_token(provider: SourceProvider) -> Result<String, SourceError> {
    let credential = sources::require_credential(provider)?;
    if !credential.needs_refresh() {
        return credential
            .access_token()
            .map(str::to_owned)
            .ok_or(SourceError::InvalidCredential(provider));
    }
    let refresh_token = credential
        .refresh_token()
        .ok_or(SourceError::SessionExpired(provider))?
        .to_owned();

    let client = sources::http_client(provider)?;
    let response = client
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPE),
        ])
        .send()
        .await
        .map_err(|_| SourceError::Network(provider))?;
    if !response.status().is_success() {
        return Err(match response.status() {
            reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::UNAUTHORIZED => {
                SourceError::SessionExpired(provider)
            }
            status => sources::error_for_status(provider, status),
        });
    }
    let payload = response
        .json::<RefreshResponse>()
        .await
        .map_err(|_| SourceError::UnexpectedResponse(provider))?;
    if payload.access_token.is_empty() {
        return Err(SourceError::SessionExpired(provider));
    }
    let refreshed = StoredSourceCredential::OAuth {
        account_id: credential.account_id().to_string(),
        account_label: credential.account_label().to_string(),
        access_token: payload.access_token.clone(),
        refresh_token: if payload.refresh_token.is_empty() {
            refresh_token
        } else {
            payload.refresh_token
        },
        expires_at_ms: sources::expiry_from_seconds(payload.expires_in),
    };
    sources::save_credential(CREDENTIAL_PROVIDER, &refreshed)?;
    Ok(payload.access_token)
}

/// The identity Xbox Live returns once both handshakes succeed. `authorization`
/// is the `XBL3.0` header value; it is short lived and derived per sync.
#[derive(Debug, Clone)]
struct XboxIdentity {
    xuid: String,
    gamertag: String,
    authorization: String,
}

#[derive(Debug, Serialize)]
struct UserAuthenticateRequest<'a> {
    #[serde(rename = "RelyingParty")]
    relying_party: &'a str,
    #[serde(rename = "TokenType")]
    token_type: &'a str,
    #[serde(rename = "Properties")]
    properties: UserAuthenticateProperties,
}

#[derive(Debug, Serialize)]
struct UserAuthenticateProperties {
    #[serde(rename = "AuthMethod")]
    auth_method: &'static str,
    #[serde(rename = "SiteName")]
    site_name: &'static str,
    #[serde(rename = "RpsTicket")]
    rps_ticket: String,
}

#[derive(Debug, Serialize)]
struct XstsRequest<'a> {
    #[serde(rename = "RelyingParty")]
    relying_party: &'a str,
    #[serde(rename = "TokenType")]
    token_type: &'a str,
    #[serde(rename = "Properties")]
    properties: XstsProperties,
}

#[derive(Debug, Serialize)]
struct XstsProperties {
    #[serde(rename = "UserTokens")]
    user_tokens: Vec<String>,
    #[serde(rename = "SandboxId")]
    sandbox_id: &'static str,
}

#[derive(Debug, Default, Deserialize)]
struct XboxAuthResponse {
    #[serde(default, rename = "Token")]
    token: String,
    #[serde(default, rename = "DisplayClaims")]
    display_claims: DisplayClaims,
}

#[derive(Debug, Default, Deserialize)]
struct DisplayClaims {
    #[serde(default)]
    xui: Vec<XuiClaim>,
}

#[derive(Debug, Default, Deserialize)]
struct XuiClaim {
    #[serde(default)]
    uhs: String,
    #[serde(default)]
    xid: String,
    #[serde(default)]
    gtg: String,
}

async fn authorize(
    client: &reqwest::Client,
    microsoft_token: &str,
    provider: SourceProvider,
) -> Result<XboxIdentity, SourceError> {
    let user = client
        .post(USER_AUTHENTICATE_ENDPOINT)
        .header("x-xbl-contract-version", "1")
        .json(&UserAuthenticateRequest {
            relying_party: "http://auth.xboxlive.com",
            token_type: "JWT",
            properties: UserAuthenticateProperties {
                auth_method: "RPS",
                site_name: "user.auth.xboxlive.com",
                rps_ticket: format!("t={microsoft_token}"),
            },
        })
        .send()
        .await
        .map_err(|_| SourceError::Network(provider))?;
    if !user.status().is_success() {
        return Err(sources::error_for_status(provider, user.status()));
    }
    let user = user
        .json::<XboxAuthResponse>()
        .await
        .map_err(|_| SourceError::UnexpectedResponse(provider))?;
    if user.token.is_empty() {
        return Err(SourceError::UnexpectedResponse(provider));
    }

    let xsts = client
        .post(XSTS_AUTHORIZE_ENDPOINT)
        .header("x-xbl-contract-version", "1")
        .json(&XstsRequest {
            relying_party: "http://xboxlive.com",
            token_type: "JWT",
            properties: XstsProperties {
                user_tokens: vec![user.token],
                sandbox_id: "RETAIL",
            },
        })
        .send()
        .await
        .map_err(|_| SourceError::Network(provider))?;
    if !xsts.status().is_success() {
        // A 401 here is Microsoft's way of saying the account has no Xbox
        // profile (or is a child account without consent), which reads far
        // better as "reconnect" than as a raw status.
        return Err(sources::error_for_status(provider, xsts.status()));
    }
    let xsts = xsts
        .json::<XboxAuthResponse>()
        .await
        .map_err(|_| SourceError::UnexpectedResponse(provider))?;
    let claim = xsts
        .display_claims
        .xui
        .first()
        .ok_or(SourceError::UnexpectedResponse(provider))?;
    if xsts.token.is_empty() || claim.uhs.is_empty() || claim.xid.is_empty() {
        return Err(SourceError::UnexpectedResponse(provider));
    }

    Ok(XboxIdentity {
        xuid: claim.xid.clone(),
        gamertag: claim.gtg.clone(),
        authorization: format!("XBL3.0 x={};{}", claim.uhs, xsts.token),
    })
}

#[derive(Debug, Default, Deserialize)]
struct TitleHistoryResponse {
    #[serde(default)]
    titles: Vec<TitleEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct TitleEntry {
    #[serde(default, rename = "titleId")]
    title_id: String,
    #[serde(default)]
    pfn: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    devices: Vec<String>,
    #[serde(default, rename = "displayImage")]
    display_image: String,
    #[serde(default)]
    detail: Option<TitleDetail>,
    #[serde(default, rename = "titleHistory")]
    title_history: Option<TitleHistory>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct TitleDetail {
    #[serde(default)]
    description: String,
    #[serde(default)]
    genres: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct TitleHistory {
    #[serde(default, rename = "lastTimePlayed")]
    last_time_played: String,
}

/// Fetch the account's title history and keep only the half that belongs to
/// the requested library. `provider` decides the filter, never the request:
/// both surfaces read the same authoritative list.
pub async fn fetch_library(provider: SourceProvider) -> Result<SourceLibrary, SourceError> {
    let token = access_token(provider).await?;
    let client = sources::http_client(provider)?;
    let identity = authorize(&client, &token, provider).await?;
    if !identity.xuid.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(SourceError::UnexpectedResponse(provider));
    }

    let response = client
        .get(format!(
            "{TITLE_HISTORY_HOST}/users/xuid({})/titles/titlehistory/decoration/detail,image,scid",
            identity.xuid
        ))
        .header("Authorization", identity.authorization)
        .header("x-xbl-contract-version", "2")
        .header("Accept-Language", "en-US")
        .send()
        .await
        .map_err(|_| SourceError::Network(provider))?;
    if !response.status().is_success() {
        return Err(sources::error_for_status(provider, response.status()));
    }
    let payload = response
        .json::<TitleHistoryResponse>()
        .await
        .map_err(|_| SourceError::UnexpectedResponse(provider))?;

    let mut library = SourceLibrary::default();
    for title in payload.titles {
        if !title.kind.is_empty() && !title.kind.eq_ignore_ascii_case("Game") {
            continue;
        }
        if !belongs_to(provider, &title) {
            continue;
        }
        library.push(provider, library_game(provider, &title));
    }
    library.sort_by_title();
    Ok(library)
}

/// One entitlement can legitimately appear in both libraries — Minecraft is on
/// a console and on the PC — so this is a filter, not a partition.
fn belongs_to(provider: SourceProvider, title: &TitleEntry) -> bool {
    let pc = title
        .devices
        .iter()
        .any(|device| matches!(device.as_str(), "PC" | "Win32"));
    match provider {
        SourceProvider::MicrosoftStore => pc,
        // A title with no device list at all is an Xbox entitlement Microsoft
        // did not decorate; keeping it in the console library is the safer of
        // the two mistakes, because it is where it came from.
        SourceProvider::Xbox => !pc || title.devices.is_empty(),
        _ => false,
    }
}

fn library_game(provider: SourceProvider, title: &TitleEntry) -> SourceLibraryGame {
    let detail = title.detail.clone().unwrap_or_default();
    // Only a Microsoft Store title carries a package family name, and that is
    // the one reference Windows can actually start.
    let launch_ref = match provider {
        SourceProvider::MicrosoftStore => title
            .pfn
            .as_deref()
            .map(str::trim)
            .filter(|pfn| !pfn.is_empty())
            .unwrap_or(title.title_id.as_str()),
        _ => title.title_id.as_str(),
    };
    SourceLibraryGame {
        source_id: title.title_id.clone(),
        launch_ref: launch_ref.to_string(),
        title: title.name.clone(),
        description: sources::normalize_html_text(&detail.description, 600),
        genre: detail
            .genres
            .first()
            .and_then(|genre| sources::normalize_text(genre, 80)),
        developer: None,
        logo_url: None,
        cover_url: non_empty(&title.display_image),
        hero_url: non_empty(&title.display_image),
        landscape_url: non_empty(&title.display_image),
        play_time_seconds: 0,
        last_played_at: title
            .title_history
            .as_ref()
            .and_then(|history| normalize_timestamp(&history.last_time_played)),
        native_mac: None,
        install: None,
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

/// Accept only a plain ISO-8601 instant. Anything else is dropped rather than
/// reinterpreted, so a provider string can never become a surprise in a date
/// parser further down.
fn normalize_timestamp(value: &str) -> Option<String> {
    let value = value.trim();
    (value.len() >= 20
        && value.len() <= 64
        && value.is_ascii()
        && value.starts_with(|character: char| character.is_ascii_digit())
        && value.contains('T')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b':' | b'.' | b'+')))
    .then(|| value.to_string())
}

fn urlencode(value: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_read_only_from_microsofts_desktop_redirect() {
        assert!(is_redirect_page(
            &reqwest::Url::parse("https://login.live.com/oauth20_desktop.srf#access_token=a")
                .unwrap()
        ));
        assert!(!is_redirect_page(
            &reqwest::Url::parse("https://login.live.com/oauth20_authorize.srf").unwrap()
        ));
        assert!(!is_redirect_page(
            &reqwest::Url::parse("https://login.live.com.evil.example/oauth20_desktop.srf")
                .unwrap()
        ));
    }

    #[test]
    fn reads_the_implicit_token_out_of_the_redirect_fragment() {
        let direct = r#"{"accessToken":"EwAoA-token","refreshToken":"M.R3_BAY-refresh","expiresIn":"86400"}"#;
        let wrapped = serde_json::to_string(direct).unwrap();

        let token = token_from_eval(&wrapped).expect("a wrapped eval result is still a token");
        assert_eq!(token.access_token, "EwAoA-token");
        assert_eq!(token.refresh_token, "M.R3_BAY-refresh");
        assert_eq!(token.expires_in, 86_400);
        assert!(token_from_eval(r#"{"accessToken":""}"#).is_none());
        assert!(token_from_eval(r#"{"accessToken":"has space"}"#).is_none());
    }

    #[test]
    fn splits_one_account_into_a_console_library_and_a_pc_library() {
        let pc_only = TitleEntry {
            devices: vec!["PC".into()],
            ..TitleEntry::default()
        };
        let console_only = TitleEntry {
            devices: vec!["XboxOne".into(), "XboxSeries".into()],
            ..TitleEntry::default()
        };
        let both = TitleEntry {
            devices: vec!["PC".into(), "XboxOne".into()],
            ..TitleEntry::default()
        };
        let undecorated = TitleEntry::default();

        assert!(belongs_to(SourceProvider::MicrosoftStore, &pc_only));
        assert!(!belongs_to(SourceProvider::Xbox, &pc_only));
        assert!(belongs_to(SourceProvider::Xbox, &console_only));
        assert!(!belongs_to(SourceProvider::MicrosoftStore, &console_only));
        // A cross-platform title is genuinely owned on both sides.
        assert!(belongs_to(SourceProvider::MicrosoftStore, &both));
        assert!(!belongs_to(SourceProvider::Xbox, &both));
        assert!(belongs_to(SourceProvider::Xbox, &undecorated));
    }

    #[test]
    fn a_store_title_launches_by_package_family_name_and_a_console_title_does_not() {
        let title = TitleEntry {
            title_id: "1017535743".into(),
            pfn: Some("Microsoft.MinecraftUWP_8wekyb3d8bbwe".into()),
            name: "Minecraft".into(),
            devices: vec!["PC".into(), "XboxOne".into()],
            display_image: "https://store-images.s-microsoft.com/image/apps.1.jpg".into(),
            title_history: Some(TitleHistory {
                last_time_played: "2026-01-04T18:00:00.0000000Z".into(),
            }),
            ..TitleEntry::default()
        };

        let store = library_game(SourceProvider::MicrosoftStore, &title);
        assert_eq!(store.source_id, "1017535743");
        assert_eq!(store.launch_ref, "Microsoft.MinecraftUWP_8wekyb3d8bbwe");
        assert!(crate::catalog::is_valid_provider_reference(
            &store.launch_ref
        ));
        assert_eq!(
            store.last_played_at.as_deref(),
            Some("2026-01-04T18:00:00.0000000Z")
        );

        let xbox = library_game(SourceProvider::Xbox, &title);
        assert_eq!(xbox.launch_ref, "1017535743");
    }

    #[test]
    fn refuses_a_timestamp_that_is_not_a_plain_instant() {
        assert!(normalize_timestamp("2026-01-04T18:00:00Z").is_some());
        assert!(normalize_timestamp("").is_none());
        assert!(normalize_timestamp("yesterday").is_none());
        assert!(normalize_timestamp("2026-01-04T18:00:00Z<script>").is_none());
    }
}
