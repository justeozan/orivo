//! GOG account connector.
//!
//! GOG's sign-in is a plain OAuth authorization-code redirect, which makes this
//! the least invasive of the connectors: the code arrives in the redirect URL
//! itself, so nothing is ever evaluated inside the sign-in page. Rust exchanges
//! the code for a token and keeps it in the system keychain.
//!
//! The client credentials below are GOG Galaxy's own public client identifiers.

use crate::sources::{
    self, SourceError, SourceLibrary, SourceLibraryGame, SourcePlatform, SourceProvider,
    StoredSourceCredential,
};
use futures_util::{StreamExt, stream};
use serde::Deserialize;

const PROVIDER: SourceProvider = SourceProvider::Gog;
const CLIENT_ID: &str = "46899977096215655";
const CLIENT_SECRET: &str = "9d85c43b1482497dbbce61f6e4aa173a433796eeae2ca8c5f6129f2dc4de46d9";
const REDIRECT_URI: &str = "https://embed.gog.com/on_login_success?origin=client";
const TOKEN_ENDPOINT: &str = "https://auth.gog.com/token";
const USER_DATA_ENDPOINT: &str = "https://embed.gog.com/userData.json";
const OWNED_GAMES_ENDPOINT: &str = "https://embed.gog.com/user/data/games";
const PRODUCT_ENDPOINT: &str = "https://api.gog.com/products";
/// Product details are public, one request per game. A small fan-out keeps a
/// large library responsive without flooding GOG.
const MAX_CONCURRENT_PRODUCT_REQUESTS: usize = 6;
const MAX_AUTHORIZATION_CODE_LENGTH: usize = 256;

pub fn login_url() -> String {
    format!(
        "https://auth.gog.com/auth?client_id={CLIENT_ID}&redirect_uri={}&response_type=code&layout=client2",
        urlencode(REDIRECT_URI)
    )
}

/// GOG hands the code back in the redirect URL, so the sign-in window never has
/// to run a script: recognising the landing page *is* the extraction.
pub fn authorization_code_from_url(url: &reqwest::Url) -> Option<String> {
    if url.scheme() != "https"
        || url.host_str() != Some("embed.gog.com")
        || url.path() != "/on_login_success"
    {
        return None;
    }
    let code = url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())?;
    let code = code.trim();
    (!code.is_empty()
        && code.len() <= MAX_AUTHORIZATION_CODE_LENGTH
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
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
    user_id: String,
}

#[derive(Debug, Default, Deserialize)]
struct UserData {
    #[serde(default)]
    username: String,
}

pub async fn connect(code: String) -> Result<StoredSourceCredential, SourceError> {
    let mut credential = request_token(&[
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", REDIRECT_URI),
    ])
    .await?;
    // The display name is a separate, best-effort call: a connection is still
    // perfectly usable when GOG will not name the account right now.
    if let Some(username) = fetch_username(&credential).await
        && let StoredSourceCredential::OAuth { account_label, .. } = &mut credential
    {
        *account_label = username;
    }
    sources::save_credential(PROVIDER, &credential)?;
    Ok(credential)
}

async fn request_token(parameters: &[(&str, &str)]) -> Result<StoredSourceCredential, SourceError> {
    let client = sources::http_client(PROVIDER)?;
    let mut query = vec![
        ("client_id", CLIENT_ID.to_string()),
        ("client_secret", CLIENT_SECRET.to_string()),
    ];
    query.extend(
        parameters
            .iter()
            .map(|(key, value)| (*key, (*value).to_string())),
    );
    let response = client
        .get(TOKEN_ENDPOINT)
        .query(&query)
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
        account_id: payload.user_id,
        account_label: "GOG account".to_string(),
        access_token: payload.access_token,
        refresh_token: payload.refresh_token,
        expires_at_ms: sources::expiry_from_seconds(payload.expires_in),
    })
}

async fn fetch_username(credential: &StoredSourceCredential) -> Option<String> {
    let client = sources::http_client(PROVIDER).ok()?;
    let response = client
        .get(USER_DATA_ENDPOINT)
        .bearer_auth(credential.access_token()?)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload = response.json::<UserData>().await.ok()?;
    sources::normalize_text(&payload.username, 120)
}

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
    let mut refreshed = request_token(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.as_str()),
    ])
    .await
    .map_err(|error| match error {
        SourceError::InvalidCredential(provider) => SourceError::SessionExpired(provider),
        other => other,
    })?;
    // A refresh answers with a new token but no profile, so carry the name
    // forward rather than resetting it to the generic placeholder.
    if let StoredSourceCredential::OAuth { account_label, .. } = &mut refreshed {
        let existing = credential.account_label();
        if !existing.is_empty() {
            *account_label = existing.to_string();
        }
    }
    sources::save_credential(PROVIDER, &refreshed)?;
    refreshed
        .access_token()
        .map(str::to_owned)
        .ok_or(SourceError::InvalidCredential(PROVIDER))
}

#[derive(Debug, Default, Deserialize)]
struct OwnedGamesResponse {
    #[serde(default)]
    owned: Vec<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct Product {
    #[serde(default)]
    title: String,
    #[serde(default)]
    slug: String,
    #[serde(default, rename = "game_type")]
    game_type: String,
    #[serde(default)]
    images: ProductImages,
    /// GOG publishes the whole platform matrix on the product record this sync
    /// already fetches, so knowing what a game runs on costs no extra request.
    #[serde(default)]
    content_system_compatibility: ContentSystemCompatibility,
}

/// GOG's own per-platform build flags. All three false is what a delisted or
/// undescribed product returns, so it is read as "GOG did not say" rather than
/// as "this game runs nowhere".
#[derive(Debug, Default, Deserialize)]
struct ContentSystemCompatibility {
    #[serde(default)]
    windows: bool,
    #[serde(default)]
    osx: bool,
    #[serde(default)]
    linux: bool,
}

impl ContentSystemCompatibility {
    fn platforms(&self) -> Vec<SourcePlatform> {
        [
            (SourcePlatform::Windows, self.windows),
            (SourcePlatform::Macos, self.osx),
            (SourcePlatform::Linux, self.linux),
        ]
        .into_iter()
        .filter_map(|(platform, supported)| supported.then_some(platform))
        .collect()
    }

    /// `None` when GOG described no platform at all: an unanswered product must
    /// not be filed as "Windows only".
    fn native_mac(&self) -> Option<bool> {
        (self.windows || self.osx || self.linux).then_some(self.osx)
    }
}

#[derive(Debug, Default, Deserialize)]
struct ProductImages {
    #[serde(default)]
    background: String,
    #[serde(default)]
    logo2x: String,
    #[serde(default)]
    logo: String,
}

pub async fn fetch_library() -> Result<SourceLibrary, SourceError> {
    let token = access_token().await?;
    let client = sources::http_client(PROVIDER)?;
    let response = client
        .get(OWNED_GAMES_ENDPOINT)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|_| SourceError::Network(PROVIDER))?;
    if !response.status().is_success() {
        return Err(sources::error_for_status(PROVIDER, response.status()));
    }
    let owned = response
        .json::<OwnedGamesResponse>()
        .await
        .map_err(|_| SourceError::UnexpectedResponse(PROVIDER))?;

    let product_ids = owned
        .owned
        .into_iter()
        .filter(|id| *id > 0)
        .take(sources::MAX_LIBRARY_GAMES)
        .collect::<Vec<_>>();
    let mut pending = stream::iter(product_ids.into_iter().map(|product_id| {
        let client = client.clone();
        async move {
            fetch_product(&client, product_id)
                .await
                .map(|product| (product_id, product))
        }
    }))
    .buffer_unordered(MAX_CONCURRENT_PRODUCT_REQUESTS);

    let mut library = SourceLibrary::default();
    while let Some(result) = pending.next().await {
        let Some((product_id, product)) = result else {
            // GOG delists products people still own. Losing the metadata for
            // one is not a reason to fail — or silently shorten — a sync.
            library.skipped = library.skipped.saturating_add(1);
            continue;
        };
        if product.game_type.eq_ignore_ascii_case("dlc")
            || product.game_type.eq_ignore_ascii_case("pack")
        {
            continue;
        }
        library.push(PROVIDER, library_game(product_id, &product));
    }

    library.sort_by_title();
    Ok(library)
}

async fn fetch_product(client: &reqwest::Client, product_id: i64) -> Option<Product> {
    let response = client
        .get(format!("{PRODUCT_ENDPOINT}/{product_id}"))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<Product>().await.ok()
}

fn library_game(product_id: i64, product: &Product) -> SourceLibraryGame {
    let id = product_id.to_string();
    let title = if product.title.trim().is_empty() {
        // A delisted product can still be owned. Naming it from its slug beats
        // dropping a game the user paid for.
        sources::normalize_text(&product.slug.replace(['-', '_'], " "), 512)
            .unwrap_or_else(|| format!("GOG product {id}"))
    } else {
        product.title.clone()
    };
    let landscape = first_non_empty(&[&product.images.logo2x, &product.images.logo]);
    SourceLibraryGame {
        source_id: id.clone(),
        launch_ref: id,
        title,
        description: None,
        genre: None,
        developer: None,
        logo_url: None,
        // GOG publishes no portrait capsule, so the wide logo stands in for the
        // cover as well; the library falls back cleanly when it is missing.
        cover_url: landscape.clone(),
        hero_url: first_non_empty(&[&product.images.background, &product.images.logo2x]),
        landscape_url: landscape,
        play_time_seconds: 0,
        last_played_at: None,
        native_mac: product.content_system_compatibility.native_mac(),
        platforms: product.content_system_compatibility.platforms(),
        install: None,
    }
}

fn first_non_empty(candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|value| !value.trim().is_empty())
        .map(|value| (*value).to_string())
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
    fn reads_the_code_only_from_gogs_own_success_redirect() {
        let success =
            reqwest::Url::parse("https://embed.gog.com/on_login_success?origin=client&code=abc123")
                .unwrap();
        let lookalike =
            reqwest::Url::parse("https://embed.gog.com.evil.example/on_login_success?code=abc123")
                .unwrap();
        let insecure =
            reqwest::Url::parse("http://embed.gog.com/on_login_success?code=abc123").unwrap();

        assert_eq!(
            authorization_code_from_url(&success).as_deref(),
            Some("abc123")
        );
        assert!(authorization_code_from_url(&lookalike).is_none());
        assert!(authorization_code_from_url(&insecure).is_none());
        assert!(
            authorization_code_from_url(
                &reqwest::Url::parse("https://embed.gog.com/on_login_success?code=a%20b").unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn files_a_gog_game_under_every_platform_gog_builds_it_for() {
        let product = Product {
            title: "The Witcher 3".into(),
            content_system_compatibility: ContentSystemCompatibility {
                windows: true,
                osx: true,
                linux: false,
            },
            ..Product::default()
        };

        let game = library_game(1_207_658_924, &product);
        assert_eq!(
            game.platforms,
            vec![SourcePlatform::Windows, SourcePlatform::Macos]
        );
        assert_eq!(game.native_mac, Some(true));
    }

    #[test]
    fn a_windows_only_gog_game_is_not_reported_as_a_mac_one() {
        let product = Product {
            title: "Disco Elysium".into(),
            content_system_compatibility: ContentSystemCompatibility {
                windows: true,
                osx: false,
                linux: false,
            },
            ..Product::default()
        };

        let game = library_game(1_207_658_924, &product);
        assert_eq!(game.platforms, vec![SourcePlatform::Windows]);
        assert_eq!(game.native_mac, Some(false));
    }

    #[test]
    fn a_product_gog_described_no_platform_for_stays_unanswered() {
        // All three flags false is a delisted or undescribed product, not a
        // game that runs nowhere. Reading it as "no Mac build" would file a
        // whole delisted back catalogue under Windows.
        let game = library_game(1_207_658_924, &Product::default());

        assert!(game.platforms.is_empty());
        assert_eq!(game.native_mac, None);
    }

    #[test]
    fn a_delisted_product_keeps_a_readable_name() {
        let product = Product {
            title: String::new(),
            slug: "the-witcher-enhanced-edition".into(),
            ..Product::default()
        };

        assert_eq!(
            library_game(1_207_658_924, &product).title,
            "the witcher enhanced edition"
        );
    }

    #[test]
    fn maps_gog_artwork_onto_the_three_library_roles() {
        let product = Product {
            title: "Cyberpunk 2077".into(),
            images: ProductImages {
                background: "//images.gog-statics.com/background.jpg".into(),
                logo2x: "//images.gog-statics.com/logo2x.jpg".into(),
                logo: "//images.gog-statics.com/logo.jpg".into(),
            },
            ..Product::default()
        };

        let game = library_game(1_423_049_311, &product);
        assert_eq!(game.source_id, "1423049311");
        assert_eq!(game.launch_ref, "1423049311");
        assert_eq!(
            game.hero_url.as_deref(),
            Some("//images.gog-statics.com/background.jpg")
        );
        assert_eq!(
            game.landscape_url.as_deref(),
            Some("//images.gog-statics.com/logo2x.jpg")
        );
    }

    #[test]
    fn the_login_url_encodes_its_redirect_uri() {
        let url = login_url();
        assert!(url.contains(
            "redirect_uri=https%3A%2F%2Fembed.gog.com%2Fon_login_success%3Forigin%3Dclient"
        ));
    }
}
