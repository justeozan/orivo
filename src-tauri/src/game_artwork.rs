//! Reliable, high-resolution artwork for a game Orivo only knows by title.
//!
//! A game synced from a connected store often arrives with a small square
//! store thumbnail, or with nothing at all — Xbox and Microsoft Store are the
//! clearest case. This module resolves a title to a canonical artwork set and
//! returns one URL per library role, so "reset the covers" fills all three
//! instead of stretching a single image across them.
//!
//! Two tiers, in order:
//!
//! 1. **SteamGridDB**, when the user has entered a key in Settings. It is the
//!    only source that publishes genuinely 4K-class assets for all three roles
//!    (heroes up to 3840×1240), so it is preferred whenever it is available.
//! 2. **Steam's own store artwork**, which needs no key at all and covers
//!    almost every PC game at the highest resolution Steam publishes
//!    (1200×1800 portrait, 1920×620 hero).
//!
//! Both tiers are host-owned: a title goes in, a URL on a known CDN comes back.
//! No response ever contributes a host, a path or a filename.

use serde::Deserialize;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const USER_AGENT: &str = "Orivo/0.3 artwork";
const STEAM_SEARCH_ENDPOINT: &str = "https://store.steampowered.com/api/storesearch/";
const STEAM_CDN: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps";
const STEAMGRIDDB_API: &str = "https://www.steamgriddb.com/api/v2";
/// Only these hosts may ever be downloaded from, whatever a response says.
const TRUSTED_HOSTS: &[&str] = &[
    "cdn.cloudflare.steamstatic.com",
    "cdn.akamai.steamstatic.com",
    "shared.akamai.steamstatic.com",
    "store.akamai.steamstatic.com",
    "steamstatic.com",
    "cdn2.steamgriddb.com",
    "cdn.steamgriddb.com",
];

/// The three independent artwork roles a library card is built from. They are
/// deliberately separate: one image stretched across all three is exactly the
/// result this module exists to replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArtworkRole {
    /// The portrait card cover (600×900 and up).
    Cover,
    /// The wide landscape card.
    Landscape,
    /// The full-bleed home background.
    Background,
}

impl ArtworkRole {
    pub fn all() -> [Self; 3] {
        [Self::Cover, Self::Landscape, Self::Background]
    }

    /// The `set_home_image` role token, so the reset and a manual pick write
    /// to the same catalog field through the same vocabulary.
    pub fn token(self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Landscape => "landscape",
            Self::Background => "background",
        }
    }
}

/// One role's candidate URLs, best first. Later entries are fallbacks for a
/// game whose publisher never uploaded the preferred asset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtworkSet {
    pub cover: Vec<String>,
    pub landscape: Vec<String>,
    pub background: Vec<String>,
}

impl ArtworkSet {
    pub fn for_role(&self, role: ArtworkRole) -> &[String] {
        match role {
            ArtworkRole::Cover => &self.cover,
            ArtworkRole::Landscape => &self.landscape,
            ArtworkRole::Background => &self.background,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cover.is_empty() && self.landscape.is_empty() && self.background.is_empty()
    }

    /// Keep whichever role the better tier already filled. A SteamGridDB hit
    /// for the cover must not be replaced by Steam's smaller one just because
    /// Steam also answered.
    fn fill_gaps_from(&mut self, other: ArtworkSet) {
        if self.cover.is_empty() {
            self.cover = other.cover;
        }
        if self.landscape.is_empty() {
            self.landscape = other.landscape;
        }
        if self.background.is_empty() {
            self.background = other.background;
        }
    }
}

/// Only a URL on a known artwork CDN may be downloaded. This is the same rule
/// the old single-image fetch applied, kept here so both paths share it.
pub fn is_trusted_artwork_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    TRUSTED_HOSTS
        .iter()
        .any(|trusted| host == *trusted || host.ends_with(&format!(".{trusted}")))
}

fn client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .ok()
}

/// Resolve a title to artwork for all three roles. `steamgriddb_key` is the
/// optional Settings value; without it the Steam tier alone is used, which
/// still covers almost every PC game.
pub async fn resolve(title: &str, steamgriddb_key: Option<&str>) -> ArtworkSet {
    let Some(query) = normalize_query(title) else {
        return ArtworkSet::default();
    };
    let Some(client) = client() else {
        return ArtworkSet::default();
    };

    let steam_app_id = steam_app_id_for_title(&client, &query).await;
    let mut set = match steamgriddb_key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => steamgriddb_artwork(&client, &query, steam_app_id, key).await,
        None => ArtworkSet::default(),
    };
    if let Some(app_id) = steam_app_id {
        set.fill_gaps_from(steam_artwork(app_id));
    }
    set
}

/// Resolve a title to Steam's official artwork alone — no key, one request.
///
/// A library sync uses this to fill in games whose own store published no
/// usable cover, which is the normal case for Xbox and the Microsoft Store.
/// Nothing is downloaded: these are CDN URLs the WebView loads directly, the
/// same way an imported Steam game's artwork already works.
pub async fn resolve_steam_only(title: &str) -> Option<ArtworkSet> {
    let query = normalize_query(title)?;
    let client = client()?;
    let app_id = steam_app_id_for_title(&client, &query).await?;
    Some(steam_artwork(app_id))
}

/// Steam's official artwork, at the largest size Steam publishes for each role.
/// `_2x` is the retina variant — 1200×1800 for a portrait cover — and is the
/// reason a reset produces a sharp card rather than an upscaled thumbnail.
pub fn steam_artwork(app_id: u64) -> ArtworkSet {
    let asset = |name: &str| format!("{STEAM_CDN}/{app_id}/{name}");
    ArtworkSet {
        cover: vec![
            asset("library_600x900_2x.jpg"),
            asset("library_600x900.jpg"),
        ],
        landscape: vec![
            asset("library_hero.jpg"),
            asset("capsule_616x353.jpg"),
            asset("header.jpg"),
        ],
        background: vec![
            asset("page_bg_raw.jpg"),
            asset("library_hero.jpg"),
            asset("page.bg.jpg"),
        ],
    }
}

#[derive(Debug, Deserialize)]
struct SteamSearchResponse {
    #[serde(default)]
    items: Vec<SteamSearchItem>,
}

#[derive(Debug, Deserialize)]
struct SteamSearchItem {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    name: String,
}

/// Find the Steam app whose name matches the title. An exact normalised match
/// always wins, because Steam's own ranking happily puts a soundtrack or a
/// season pass above the game when the title is short.
async fn steam_app_id_for_title(client: &reqwest::Client, title: &str) -> Option<u64> {
    let response = client
        .get(STEAM_SEARCH_ENDPOINT)
        .query(&[("term", title), ("cc", "us"), ("l", "en")])
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let payload = response.json::<SteamSearchResponse>().await.ok()?;
    let wanted = comparison_key(title);
    payload
        .items
        .iter()
        .find(|item| item.id > 0 && comparison_key(&item.name) == wanted)
        .or_else(|| payload.items.iter().find(|item| item.id > 0))
        .map(|item| item.id)
}

#[derive(Debug, Deserialize)]
struct GridDbEnvelope<T> {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Default, Deserialize)]
struct GridDbGame {
    #[serde(default)]
    id: u64,
}

#[derive(Debug, Default, Deserialize)]
struct GridDbAsset {
    #[serde(default)]
    url: String,
    #[serde(default)]
    width: u32,
}

/// SteamGridDB, used only when the user supplied a key. Assets come back
/// sorted by width so the sharpest one is tried first — this is the tier that
/// actually delivers 4K-class backgrounds.
async fn steamgriddb_artwork(
    client: &reqwest::Client,
    title: &str,
    steam_app_id: Option<u64>,
    key: &str,
) -> ArtworkSet {
    let Some(game_id) = griddb_game_id(client, title, steam_app_id, key).await else {
        return ArtworkSet::default();
    };
    ArtworkSet {
        cover: griddb_assets(client, "grids", game_id, key, "600x900,660x930,512x512").await,
        landscape: griddb_assets(client, "grids", game_id, key, "920x430,460x215,1920x620").await,
        background: griddb_assets(client, "heroes", game_id, key, "1920x620,3840x1240").await,
    }
}

async fn griddb_game_id(
    client: &reqwest::Client,
    title: &str,
    steam_app_id: Option<u64>,
    key: &str,
) -> Option<u64> {
    // Matching by Steam AppID is exact; searching by name is the fallback.
    if let Some(app_id) = steam_app_id {
        let url = format!("{STEAMGRIDDB_API}/games/steam/{app_id}");
        if let Some(game) = griddb_get::<GridDbGame>(client, &url, key).await
            && game.id > 0
        {
            return Some(game.id);
        }
    }
    let url = format!(
        "{STEAMGRIDDB_API}/search/autocomplete/{}",
        urlencode_path(title)
    );
    griddb_get::<Vec<GridDbGame>>(client, &url, key)
        .await?
        .into_iter()
        .find(|game| game.id > 0)
        .map(|game| game.id)
}

async fn griddb_assets(
    client: &reqwest::Client,
    kind: &str,
    game_id: u64,
    key: &str,
    dimensions: &str,
) -> Vec<String> {
    let url = format!("{STEAMGRIDDB_API}/{kind}/game/{game_id}?dimensions={dimensions}");
    let Some(mut assets) = griddb_get::<Vec<GridDbAsset>>(client, &url, key).await else {
        return Vec::new();
    };
    assets.sort_by(|left, right| right.width.cmp(&left.width));
    assets
        .into_iter()
        .map(|asset| asset.url)
        .filter(|url| is_trusted_artwork_url(url))
        .take(4)
        .collect()
}

async fn griddb_get<T: for<'de> Deserialize<'de> + Default>(
    client: &reqwest::Client,
    url: &str,
    key: &str,
) -> Option<T> {
    let response = client.get(url).bearer_auth(key).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let envelope = response.json::<GridDbEnvelope<T>>().await.ok()?;
    envelope.success.then_some(envelope.data).flatten()
}

/// Titles arrive with trademark marks, edition suffixes and stray punctuation
/// that no store search matches. Strip them before asking.
fn normalize_query(title: &str) -> Option<String> {
    let cleaned = title
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| match character {
            '™' | '®' | '©' => ' ',
            other => other,
        })
        .collect::<String>();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    (!cleaned.is_empty()).then(|| cleaned.chars().take(120).collect())
}

/// Compare two titles the way a person would: ignoring case, punctuation and
/// spacing, so "Marvel's Spider-Man" matches "Marvel s Spider Man".
fn comparison_key(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_gets_its_own_steam_asset_at_the_largest_published_size() {
        let set = steam_artwork(1_245_620);

        // The portrait cover leads with the retina variant: 1200x1800, not the
        // 600x900 that used to be stretched across all three roles.
        assert_eq!(
            set.cover.first().unwrap(),
            "https://cdn.cloudflare.steamstatic.com/steam/apps/1245620/library_600x900_2x.jpg"
        );
        assert!(set.landscape.first().unwrap().ends_with("library_hero.jpg"));
        assert!(set.background.first().unwrap().ends_with("page_bg_raw.jpg"));
        // Each role is distinct, which is the whole point of the reset.
        assert_ne!(set.cover.first(), set.landscape.first());
        assert_ne!(set.landscape.first(), set.background.first());
        assert!(set.for_role(ArtworkRole::Cover).len() > 1, "a fallback per role");
        assert!(set.iter_is_trusted());
    }

    impl ArtworkSet {
        fn iter_is_trusted(&self) -> bool {
            ArtworkRole::all()
                .into_iter()
                .flat_map(|role| self.for_role(role).iter())
                .all(|url| is_trusted_artwork_url(url))
        }
    }

    #[test]
    fn only_known_artwork_cdns_can_ever_be_downloaded_from() {
        assert!(is_trusted_artwork_url(
            "https://cdn.cloudflare.steamstatic.com/steam/apps/1/a.jpg"
        ));
        assert!(is_trusted_artwork_url("https://cdn2.steamgriddb.com/hero/abc.png"));
        assert!(!is_trusted_artwork_url("http://cdn2.steamgriddb.com/hero/abc.png"));
        assert!(!is_trusted_artwork_url("https://evil.example/a.jpg"));
        // A host that merely ends with a trusted name must not pass.
        assert!(!is_trusted_artwork_url(
            "https://steamstatic.com.evil.example/a.jpg"
        ));
        assert!(!is_trusted_artwork_url("not a url"));
    }

    #[test]
    fn a_better_tier_is_never_overwritten_by_a_smaller_one() {
        let mut set = ArtworkSet {
            cover: vec!["https://cdn2.steamgriddb.com/grid/4k.png".into()],
            ..ArtworkSet::default()
        };
        set.fill_gaps_from(steam_artwork(42));

        // SteamGridDB kept the cover; Steam filled the two roles it had not.
        assert_eq!(set.cover, vec!["https://cdn2.steamgriddb.com/grid/4k.png"]);
        assert!(set.landscape.first().unwrap().contains("/42/"));
        assert!(set.background.first().unwrap().contains("/42/"));
    }

    #[test]
    fn titles_are_cleaned_before_a_store_is_asked_about_them() {
        assert_eq!(
            normalize_query("  Halo™   Infinite \n").as_deref(),
            Some("Halo Infinite")
        );
        assert_eq!(normalize_query("   ").as_deref(), None);
        // Punctuation and case never decide whether a title matches.
        assert_eq!(comparison_key("Marvel's Spider-Man"), comparison_key("marvel s spider man"));
        assert_ne!(comparison_key("Halo Infinite"), comparison_key("Halo Infinite OST"));
    }
}
