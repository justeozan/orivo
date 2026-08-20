//! Epic Games Store account connector.
//!
//! Epic publishes an OAuth flow its own launcher uses: a sign-in page redirects
//! to `/id/api/redirect`, which answers with a one-time authorization code in
//! plain JSON. Orivo reads that code inside the dedicated, capability-free
//! sign-in window, exchanges it for a token in Rust, and keeps the result in
//! the system keychain. No password, no cookie and no Orivo server take part.
//!
//! The client credentials below are the ones the Epic Games Launcher itself
//! presents; they identify the application, not the user, and are public.

use crate::sources::{
    self, SourceError, SourceLibrary, SourceLibraryGame, SourceProvider, StoredSourceCredential,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

const PROVIDER: SourceProvider = SourceProvider::Epic;
const CLIENT_ID: &str = "34a02cf8f4414e29b15921876da36f9a";
const CLIENT_SECRET: &str = "daafbccc737745039dffe53d94fc76cf";
const REDIRECT_URL: &str = "https://www.epicgames.com/id/api/redirect?clientId=34a02cf8f4414e29b15921876da36f9a&responseType=code";
const OAUTH_TOKEN_ENDPOINT: &str =
    "https://account-public-service-prod.ol.epicgames.com/account/api/oauth/token";
/// Epic's entitlement list is platform-scoped. The Windows list is the whole
/// library — nearly every Epic game ships a Windows build — and the Mac list is
/// exactly the subset that has a native macOS build. Asking for both is how
/// Orivo can say "runs natively on this Mac" without guessing.
const LAUNCHER_ASSETS_ENDPOINT: &str =
    "https://launcher-public-service-prod06.ol.epicgames.com/launcher/api/public/assets";
const WINDOWS_PLATFORM: &str = "Windows";
const MAC_PLATFORM: &str = "Mac";
const CATALOG_HOST: &str = "https://catalog-public-service-prod06.ol.epicgames.com";
/// Epic answers a bulk catalog request per namespace. Keeping the batch small
/// avoids a request URL long enough for the service to reject outright.
const CATALOG_BATCH_SIZE: usize = 30;
const MAX_AUTHORIZATION_CODE_LENGTH: usize = 256;

/// Read the one-time authorization code Epic prints as JSON once sign-in
/// completes. This runs only in the capability-free sign-in window and returns
/// one short string, never the page.
pub const AUTHORIZATION_EXTRACTION_SCRIPT: &str = r#"
(() => {
  const body = document.body ? document.body.innerText : '';
  let code = '';
  try {
    const payload = JSON.parse(body);
    if (payload && typeof payload.authorizationCode === 'string') {
      code = payload.authorizationCode;
    }
  } catch (error) {
    const match = /"authorizationCode"\s*:\s*"([A-Za-z0-9]+)"/.exec(body);
    code = match ? match[1] : '';
  }
  return JSON.stringify({ code });
})()
"#;

pub fn login_url() -> String {
    format!(
        "https://www.epicgames.com/id/login?redirectUrl={}",
        urlencode(REDIRECT_URL)
    )
}

/// The exact page a code may be read from. Sign-in itself roams across Epic's
/// identity hosts and any embedded challenge; extraction does not.
pub fn is_authorization_page(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some("www.epicgames.com") | Some("epicgames.com")
        )
        && url.path() == "/id/api/redirect"
}

pub fn authorization_code_from_eval(result: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Payload {
        #[serde(default)]
        code: String,
    }

    let payload = serde_json::from_str::<serde_json::Value>(result)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| result.to_string());
    let payload = serde_json::from_str::<Payload>(&payload).ok()?;
    let code = payload.code.trim();
    (!code.is_empty()
        && code.len() <= MAX_AUTHORIZATION_CODE_LENGTH
        && code.bytes().all(|byte| byte.is_ascii_alphanumeric()))
    .then(|| code.to_string())
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    account_id: String,
    #[serde(default, rename = "displayName")]
    display_name: String,
}

/// Exchange the one-time code for a credential and persist it. The exchange is
/// performed before anything is written, so a rejected code can never replace a
/// working connection.
pub async fn connect(code: String) -> Result<StoredSourceCredential, SourceError> {
    let credential = request_token(&[
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("token_type", "eg1"),
    ])
    .await?;
    sources::save_credential(PROVIDER, &credential)?;
    Ok(credential)
}

async fn request_token(form: &[(&str, &str)]) -> Result<StoredSourceCredential, SourceError> {
    let client = sources::http_client(PROVIDER)?;
    let response = client
        .post(OAUTH_TOKEN_ENDPOINT)
        .basic_auth(CLIENT_ID, Some(CLIENT_SECRET))
        .form(form)
        .send()
        .await
        .map_err(|_| SourceError::Network(PROVIDER))?;
    if !response.status().is_success() {
        return Err(match response.status() {
            reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::UNAUTHORIZED => {
                SourceError::InvalidCredential(PROVIDER)
            }
            status => sources::error_for_status(PROVIDER, status),
        });
    }
    let payload = response
        .json::<TokenResponse>()
        .await
        .map_err(|_| SourceError::UnexpectedResponse(PROVIDER))?;
    if payload.access_token.is_empty() {
        return Err(SourceError::UnexpectedResponse(PROVIDER));
    }
    Ok(StoredSourceCredential::OAuth {
        account_id: payload.account_id,
        account_label: sources::normalize_text(&payload.display_name, 120)
            .unwrap_or_else(|| "Epic Games account".to_string()),
        access_token: payload.access_token,
        refresh_token: payload.refresh_token,
        expires_at_ms: sources::expiry_from_seconds(payload.expires_in),
    })
}

/// Return a usable access token, refreshing first when the stored one is close
/// to expiry. A refresh that Epic rejects is reported as an expired session so
/// the user is told to reconnect rather than shown a generic failure.
async fn access_token() -> Result<String, SourceError> {
    let credential = sources::require_credential(PROVIDER)?;
    if !credential.needs_refresh() {
        return credential
            .access_token()
            .map(str::to_owned)
            .ok_or(SourceError::InvalidCredential(PROVIDER));
    }
    let refresh_token = credential
        .refresh_token()
        .ok_or(SourceError::SessionExpired(PROVIDER))?
        .to_owned();
    let refreshed = request_token(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.as_str()),
        ("token_type", "eg1"),
    ])
    .await
    .map_err(|error| match error {
        SourceError::InvalidCredential(provider) => SourceError::SessionExpired(provider),
        other => other,
    })?;
    sources::save_credential(PROVIDER, &refreshed)?;
    refreshed
        .access_token()
        .map(str::to_owned)
        .ok_or(SourceError::InvalidCredential(PROVIDER))
}

#[derive(Debug, Deserialize)]
struct LauncherAsset {
    #[serde(default, rename = "appName")]
    app_name: String,
    #[serde(default, rename = "catalogItemId")]
    catalog_item_id: String,
    #[serde(default)]
    namespace: String,
}

#[derive(Debug, Default, Deserialize)]
struct CatalogItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    developer: String,
    #[serde(default, rename = "keyImages")]
    key_images: Vec<CatalogImage>,
    #[serde(default)]
    categories: Vec<CatalogCategory>,
}

#[derive(Debug, Deserialize)]
struct CatalogImage {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    url: String,
}

#[derive(Debug, Deserialize)]
struct CatalogCategory {
    #[serde(default)]
    path: String,
}

pub async fn fetch_library() -> Result<SourceLibrary, SourceError> {
    let token = access_token().await?;
    let client = sources::http_client(PROVIDER)?;
    let assets = fetch_assets(&client, &token, WINDOWS_PLATFORM).await?;
    // Native-Mac is an enhancement to a library, never a precondition for one:
    // if Epic will not answer for the Mac platform right now, every game simply
    // goes unmarked rather than the sync failing.
    let native_mac = fetch_assets(&client, &token, MAC_PLATFORM)
        .await
        .map(|assets| {
            assets
                .into_iter()
                .map(|asset| asset.app_name)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    // Locally installed games are joined in by app name, which is exactly the
    // `source_id` the catalog already stores for an Epic entry.
    let installations = crate::epic_install::installations();

    // Group by namespace: Epic's bulk catalog endpoint is namespace-scoped, so
    // one request per namespace batch replaces one request per game.
    let mut by_namespace: BTreeMap<String, Vec<LauncherAsset>> = BTreeMap::new();
    let mut library = SourceLibrary::default();
    let mut seen = BTreeSet::new();
    for asset in assets {
        // `ue` is the Unreal Engine marketplace namespace: plugins and assets,
        // never games. Everything else is decided by the catalog categories.
        if asset.namespace.is_empty() || asset.namespace == "ue" || asset.app_name.is_empty() {
            continue;
        }
        if !seen.insert((asset.namespace.clone(), asset.app_name.clone())) {
            continue;
        }
        by_namespace
            .entry(asset.namespace.clone())
            .or_default()
            .push(asset);
    }

    for (namespace, assets) in by_namespace {
        for batch in assets.chunks(CATALOG_BATCH_SIZE) {
            // Metadata is an enhancement. A namespace Epic will not describe
            // right now must not remove those games from the library.
            let items = fetch_catalog_items(&client, &token, &namespace, batch)
                .await
                .unwrap_or_default();
            for asset in batch {
                let Some(item) = items.get(&asset.catalog_item_id) else {
                    continue;
                };
                if !is_game(item) {
                    continue;
                }
                library.push(
                    PROVIDER,
                    library_game(
                        &namespace,
                        asset,
                        item,
                        native_mac.contains(&asset.app_name),
                        installations
                            .get(&asset.app_name)
                            .map(crate::epic_install::status_for),
                    ),
                );
            }
        }
    }

    library.sort_by_title();
    Ok(library)
}

async fn fetch_assets(
    client: &reqwest::Client,
    token: &str,
    platform: &str,
) -> Result<Vec<LauncherAsset>, SourceError> {
    let response = client
        .get(format!("{LAUNCHER_ASSETS_ENDPOINT}/{platform}"))
        .query(&[("label", "Live")])
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| SourceError::Network(PROVIDER))?;
    if !response.status().is_success() {
        return Err(sources::error_for_status(PROVIDER, response.status()));
    }
    response
        .json::<Vec<LauncherAsset>>()
        .await
        .map_err(|_| SourceError::UnexpectedResponse(PROVIDER))
}

async fn fetch_catalog_items(
    client: &reqwest::Client,
    token: &str,
    namespace: &str,
    batch: &[LauncherAsset],
) -> Result<BTreeMap<String, CatalogItem>, SourceError> {
    // The namespace is interpolated into the path, so it is checked against the
    // same opaque grammar the catalog uses before it can shape a request.
    if !crate::catalog::is_valid_provider_reference(namespace) {
        return Ok(BTreeMap::new());
    }
    let mut query = vec![
        ("includeDLCDetails", "false".to_string()),
        ("includeMainGameDetails", "false".to_string()),
        ("country", "US".to_string()),
        ("locale", "en-US".to_string()),
    ];
    for asset in batch {
        if crate::catalog::is_valid_provider_reference(&asset.catalog_item_id) {
            query.push(("id", asset.catalog_item_id.clone()));
        }
    }
    let response = client
        .get(format!(
            "{CATALOG_HOST}/catalog/api/shared/namespace/{namespace}/bulk/items"
        ))
        .query(&query)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| SourceError::Network(PROVIDER))?;
    if !response.status().is_success() {
        return Err(sources::error_for_status(PROVIDER, response.status()));
    }
    response
        .json::<BTreeMap<String, CatalogItem>>()
        .await
        .map_err(|_| SourceError::UnexpectedResponse(PROVIDER))
}

/// Epic's entitlement list also carries add-ons, engine builds and the store's
/// own applications. Only entries the catalog itself files under `games`, and
/// that are not an add-on, belong in a game library.
fn is_game(item: &CatalogItem) -> bool {
    let mut is_game = false;
    for category in &item.categories {
        match category.path.as_str() {
            "games" => is_game = true,
            "addons" | "digitalextras" | "plugins" | "engines" | "assets" | "projects" => {
                return false;
            }
            _ => {}
        }
    }
    is_game
}

fn library_game(
    namespace: &str,
    asset: &LauncherAsset,
    item: &CatalogItem,
    native_mac: bool,
    install: Option<crate::epic_install::EpicInstallStatus>,
) -> SourceLibraryGame {
    SourceLibraryGame {
        native_mac: Some(native_mac),
        install: install.map(Into::into),
        source_id: asset.app_name.clone(),
        // The Epic launcher URI needs all three parts, so the launch reference
        // keeps them joined rather than storing three loose fields.
        launch_ref: format!("{}:{}:{}", namespace, asset.catalog_item_id, asset.app_name),
        title: item.title.clone(),
        description: sources::normalize_html_text(&item.description, 600),
        // Epic's bulk catalog carries no genre — `categories` files an entry as
        // `games`, not as an RPG — so the library takes none from here. The
        // studio used to be put in this field for want of anything better,
        // which is how "Ubisoft Montréal" ended up billed as a genre.
        genre: None,
        developer: (!item.developer.trim().is_empty()).then(|| item.developer.clone()),
        cover_url: key_image(item, &["DieselGameBoxTall", "OfferImageTall", "Thumbnail"]),
        // Epic ships the wordmark apart from the artwork precisely so a client
        // can lay one over the other. Orivo now does, so it is worth taking —
        // as a logo. It is still never a wallpaper: a transparent PNG makes a
        // terrible one, and no wallpaper reads better than a bad one.
        logo_url: key_image(item, &["DieselGameBoxLogo"]),
        hero_url: key_image(item, &["DieselGameBox", "OfferImageWide"]),
        landscape_url: key_image(item, &["DieselGameBox", "OfferImageWide"]),
        play_time_seconds: 0,
        last_played_at: None,
    }
}

fn key_image(item: &CatalogItem, preference: &[&str]) -> Option<String> {
    preference.iter().find_map(|wanted| {
        item.key_images
            .iter()
            .find(|image| image.kind == *wanted && !image.url.is_empty())
            .map(|image| image.url.clone())
    })
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
    fn a_code_is_read_only_from_epics_own_redirect_page() {
        let redirect =
            reqwest::Url::parse("https://www.epicgames.com/id/api/redirect?clientId=x").unwrap();
        let login = reqwest::Url::parse("https://www.epicgames.com/id/login").unwrap();
        let lookalike =
            reqwest::Url::parse("https://epicgames.com.evil.example/id/api/redirect").unwrap();

        assert!(is_authorization_page(&redirect));
        assert!(!is_authorization_page(&login));
        assert!(!is_authorization_page(&lookalike));
    }

    #[test]
    fn accepts_direct_and_json_wrapped_authorization_results() {
        let direct = r#"{"code":"6a1f2b3c4d5e6f7a8b9c0d1e2f3a4b5c"}"#;
        let wrapped = serde_json::to_string(direct).unwrap();

        assert_eq!(
            authorization_code_from_eval(direct).as_deref(),
            Some("6a1f2b3c4d5e6f7a8b9c0d1e2f3a4b5c")
        );
        assert_eq!(
            authorization_code_from_eval(&wrapped).as_deref(),
            Some("6a1f2b3c4d5e6f7a8b9c0d1e2f3a4b5c")
        );
        assert!(authorization_code_from_eval(r#"{"code":""}"#).is_none());
        assert!(authorization_code_from_eval(r#"{"code":"a b"}"#).is_none());
        assert!(authorization_code_from_eval("not json").is_none());
    }

    #[test]
    fn only_catalog_games_reach_the_library() {
        let game = CatalogItem {
            categories: vec![CatalogCategory {
                path: "games".into(),
            }],
            ..CatalogItem::default()
        };
        let add_on = CatalogItem {
            categories: vec![
                CatalogCategory {
                    path: "games".into(),
                },
                CatalogCategory {
                    path: "addons".into(),
                },
            ],
            ..CatalogItem::default()
        };
        let application = CatalogItem {
            categories: vec![CatalogCategory {
                path: "applications".into(),
            }],
            ..CatalogItem::default()
        };

        assert!(is_game(&game));
        assert!(!is_game(&add_on));
        assert!(!is_game(&application));
    }

    #[test]
    fn builds_a_three_part_launch_reference_the_epic_client_understands() {
        let asset = LauncherAsset {
            app_name: "Sugar".into(),
            catalog_item_id: "a1b2c3".into(),
            namespace: "d5241c".into(),
        };
        let item = CatalogItem {
            title: "Fall Guys".into(),
            key_images: vec![CatalogImage {
                kind: "DieselGameBoxTall".into(),
                url: "https://cdn1.epicgames.com/tall.png".into(),
            }],
            categories: vec![CatalogCategory {
                path: "games".into(),
            }],
            ..CatalogItem::default()
        };

        let game = library_game("d5241c", &asset, &item, false, None);
        assert_eq!(game.source_id, "Sugar");
        assert_eq!(game.launch_ref, "d5241c:a1b2c3:Sugar");
        assert_eq!(
            game.cover_url.as_deref(),
            Some("https://cdn1.epicgames.com/tall.png")
        );
        assert!(crate::catalog::is_valid_provider_reference(
            &game.launch_ref
        ));
        assert_eq!(game.native_mac, Some(false));
        assert!(game.install.is_none());
    }

    #[test]
    fn a_mac_entitlement_and_a_running_download_travel_with_the_library_record() {
        let asset = LauncherAsset {
            app_name: "Sugar".into(),
            catalog_item_id: "a1b2c3".into(),
            namespace: "d5241c".into(),
        };
        let item = CatalogItem {
            title: "Fall Guys".into(),
            categories: vec![CatalogCategory {
                path: "games".into(),
            }],
            ..CatalogItem::default()
        };
        let install = crate::epic_install::EpicInstallStatus {
            app_name: "Sugar".into(),
            state: crate::epic_install::EpicInstallState::Installing,
            percent: 42,
            installed_bytes: 42,
            total_bytes: 100,
            install_path: Some("/Users/Shared/Epic/Fall Guys".into()),
        };

        let game = library_game("d5241c", &asset, &item, true, Some(install));
        assert_eq!(game.native_mac, Some(true));
        let install = game.install.unwrap();
        assert!(install.installing);
        assert!(!install.installed);
        assert_eq!(install.percent, 42);
    }

    #[test]
    fn the_login_url_carries_an_encoded_redirect() {
        let url = login_url();
        assert!(url.starts_with("https://www.epicgames.com/id/login?redirectUrl=https%3A%2F%2F"));
        assert!(!url.contains("redirectUrl=https://"));
    }
}
