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
/// A host that does not even accept the connection is dead for this attempt;
/// waiting out the whole request timeout for it only stalls the fill behind it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// Pause before the single retry of a request that failed for a reason that
/// is usually gone a moment later (a dropped connection, a 5xx, a 429).
const RETRY_DELAY: Duration = Duration::from_millis(400);
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
    /// The game's own wordmark on transparency, drawn over the scene in place
    /// of the hero title. Not artwork: it is never a wallpaper.
    Logo,
}

impl ArtworkRole {
    pub fn all() -> [Self; 4] {
        [Self::Cover, Self::Landscape, Self::Background, Self::Logo]
    }

    /// The `set_home_image` role token, so the reset and a manual pick write
    /// to the same catalog field through the same vocabulary.
    pub fn token(self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Landscape => "landscape",
            Self::Background => "background",
            Self::Logo => "logo",
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
    pub logo: Vec<String>,
}

impl ArtworkSet {
    pub fn for_role(&self, role: ArtworkRole) -> &[String] {
        match role {
            ArtworkRole::Cover => &self.cover,
            ArtworkRole::Landscape => &self.landscape,
            ArtworkRole::Background => &self.background,
            ArtworkRole::Logo => &self.logo,
        }
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
        // Without this the logo tier never reaches a caller: SteamGridDB is the
        // only source that fills it directly, and a reset with no API key — the
        // documented default — resolved an empty list and left `logo_path`
        // unwritten while reporting success.
        if self.logo.is_empty() {
            self.logo = other.logo;
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

/// The HTTP client artwork searches go through (downloads use
/// `download_client`). Both timeouts matter: without them a single stalled host holds a
/// whole fill hostage, since the fill waits on every card.
pub(crate) fn client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .ok()
}

/// How long one artwork file may take to arrive. Longer than a search: a 4K
/// background from SteamGridDB is several megabytes, and a slow line must not
/// turn a real picture into "nothing found". The connect timeout still drops a
/// dead host quickly.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(45);

/// The client artwork files are downloaded with: the search client's
/// connection rules, with room for a large file.
pub(crate) fn download_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .ok()
}

/// Whether the artwork host answers at all. Any HTTP reply counts, even an
/// error status: the question is only whether the network is there, because an
/// artwork search that comes back empty looks the same offline as it does for a
/// title nobody has art for.
pub async fn is_reachable() -> bool {
    let Some(client) = client() else {
        return false;
    };
    client.head(STEAM_SEARCH_ENDPOINT).send().await.is_ok()
}

/// Resolve a title to artwork for all three roles. `steamgriddb_key` is the
/// optional Settings value; without it the Steam tier alone is used, which
/// still covers almost every PC game.
pub async fn resolve(title: &str, steamgriddb_key: Option<&str>) -> ArtworkSet {
    let queries = query_variants(title);
    if queries.is_empty() {
        return ArtworkSet::default();
    }
    let Some(client) = client() else {
        return ArtworkSet::default();
    };

    let steam_app_id = steam_app_id_for_title(&client, &queries).await;
    let mut set = match steamgriddb_key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => steamgriddb_artwork(&client, &queries, steam_app_id, key).await,
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
    let queries = query_variants(title);
    if queries.is_empty() {
        return None;
    }
    let client = client()?;
    let app_id = steam_app_id_for_title(&client, &queries).await?;
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
        // Steam's own wordmark, the asset its client composites over the hero.
        logo: vec![asset("logo.png"), asset("library_logo.png")],
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

/// Send a request, and once more after a short pause if it failed at the
/// network level or came back 5xx/429. Store search endpoints shed load by
/// failing a request now and then; a second try a moment later almost always
/// goes through, and giving up on the first would leave the card artless.
async fn send_with_retry(request: reqwest::RequestBuilder) -> Option<reqwest::Response> {
    let retry = request.try_clone();
    let first = request.send().await;
    let transient = match &first {
        Ok(response) => {
            response.status().is_server_error()
                || response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
        }
        Err(_) => true,
    };
    match retry {
        Some(retry) if transient => {
            tokio::time::sleep(RETRY_DELAY).await;
            retry.send().await.ok()
        }
        _ => first.ok(),
    }
}

/// One Steam store search, or `None` when the store could not answer.
async fn steam_search(client: &reqwest::Client, term: &str) -> Option<Vec<SteamSearchItem>> {
    let response = send_with_retry(client.get(STEAM_SEARCH_ENDPOINT).query(&[
        ("term", term),
        ("cc", "us"),
        ("l", "en"),
    ]))
    .await?;
    if !response.status().is_success() {
        return None;
    }
    Some(response.json::<SteamSearchResponse>().await.ok()?.items)
}

/// Find the Steam app whose name matches the title, trying each query variant
/// in turn. An exact normalised match against the variant being tried always
/// wins, because Steam's own ranking happily puts a soundtrack or a season
/// pass above the game when the title is short. Failing that, the top result
/// of the first variant that found anything is the answer, but only once every
/// variant has had its chance at an exact match.
pub(crate) async fn steam_app_id_for_title(client: &reqwest::Client, queries: &[String]) -> Option<u64> {
    let mut first_hit = None;
    for query in queries {
        let Some(items) = steam_search(client, query).await else {
            continue;
        };
        let wanted = comparison_key(query);
        if let Some(item) = items
            .iter()
            .find(|item| item.id > 0 && comparison_key(&item.name) == wanted)
        {
            return Some(item.id);
        }
        if first_hit.is_none() {
            first_hit = items.iter().find(|item| item.id > 0).map(|item| item.id);
        }
    }
    first_hit
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

/// The `styles` filter is the reason artwork comes back clean — but the
/// vocabulary is **per asset kind**, and mixing them up is a hard 400.
///
/// SteamGridDB tags every upload with a style. For grids the set is `alternate |
/// blurred | white_logo | material | no_logo`, and two of those — `white_logo`
/// and `material` — are variants with the game's wordmark burned into the
/// picture. Asking a grid unfiltered returns those alongside the rest, which is
/// exactly the "artwork with a logo on it" a card must not wear when the app
/// already composites the real wordmark over the scene. `alternate` is the plain
/// publisher art and `no_logo` is the community's explicitly de-logoed cut, so
/// the two together are the whole usable grid set.
///
/// Heroes accept only `alternate | blurred | material` — there is no `no_logo`
/// for a hero, and sending one is rejected outright rather than ignored. None is
/// needed: a hero is the scene Steam paints a separate wordmark over, so it has
/// no title in it to begin with, and naming `alternate` alone already leaves out
/// the `material` treatment.
///
/// `logos` is the one role that wants the wordmark, and has its own set again
/// (`official | white | black | custom`).
pub const GRID_STYLES: &str = "alternate,no_logo";
pub const HERO_STYLES: &str = "alternate";
pub const LOGO_STYLES: &str = "official,white";
/// Joke uploads and NSFW edits are tagged, and neither belongs on a library
/// card that a reset filled without anyone watching.
const CLEAN_FILTERS: &str = "nsfw=false&humor=false&types=static";

/// SteamGridDB, used only when the user supplied a key. Assets come back
/// sorted by width so the sharpest one is tried first — this is the tier that
/// actually delivers 4K-class backgrounds.
async fn steamgriddb_artwork(
    client: &reqwest::Client,
    queries: &[String],
    steam_app_id: Option<u64>,
    key: &str,
) -> ArtworkSet {
    let Some(game_id) = griddb_game_id(client, queries, steam_app_id, key).await else {
        return ArtworkSet::default();
    };
    ArtworkSet {
        cover: griddb_assets(
            client,
            "grids",
            game_id,
            key,
            "dimensions=600x900,660x930,512x512",
            GRID_STYLES,
        )
        .await,
        landscape: griddb_assets(
            client,
            "grids",
            game_id,
            key,
            "dimensions=920x430,460x215",
            GRID_STYLES,
        )
        .await,
        // Widest first: SteamGridDB sorts by width inside a request, but the
        // `dimensions` filter is a set, not an order. Asking for the 4K hero
        // alone first is what actually gets a 4K background rather than the
        // 1920 one that happens to be more common.
        background: {
            let mut wide = griddb_assets(
                client,
                "heroes",
                game_id,
                key,
                "dimensions=3840x1240",
                HERO_STYLES,
            )
            .await;
            wide.extend(
                griddb_assets(
                    client,
                    "heroes",
                    game_id,
                    key,
                    "dimensions=1920x620",
                    HERO_STYLES,
                )
                .await,
            );
            wide
        },
        // Wordmarks come in every shape there is, so this is the one role that
        // cannot be asked for by dimension.
        logo: griddb_assets(client, "logos", game_id, key, "", LOGO_STYLES).await,
    }
}

async fn griddb_game_id(
    client: &reqwest::Client,
    queries: &[String],
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
    // The first variant SteamGridDB knows a game for wins: they are ordered
    // from most to least specific, so a hit is as precise as it can be.
    for query in queries {
        let url = format!(
            "{STEAMGRIDDB_API}/search/autocomplete/{}",
            urlencode_path(query)
        );
        let found = griddb_get::<Vec<GridDbGame>>(client, &url, key)
            .await
            .and_then(|games| games.into_iter().find(|game| game.id > 0));
        if let Some(game) = found {
            return Some(game.id);
        }
    }
    None
}

async fn griddb_assets(
    client: &reqwest::Client,
    kind: &str,
    game_id: u64,
    key: &str,
    dimensions: &str,
    styles: &str,
) -> Vec<String> {
    let url = griddb_asset_url(kind, game_id, dimensions, styles);
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

/// Builds one asset request. `dimensions` is already a `key=value` pair (or
/// empty for the roles that have no dimension vocabulary), so the only thing
/// left to decide here is where the `?` goes.
fn griddb_asset_url(kind: &str, game_id: u64, dimensions: &str, styles: &str) -> String {
    let mut query = Vec::new();
    if !dimensions.is_empty() {
        query.push(dimensions.to_owned());
    }
    if !styles.is_empty() {
        query.push(format!("styles={styles}"));
    }
    query.push(CLEAN_FILTERS.to_owned());
    format!(
        "{STEAMGRIDDB_API}/{kind}/game/{game_id}?{}",
        query.join("&")
    )
}

async fn griddb_get<T: for<'de> Deserialize<'de> + Default>(
    client: &reqwest::Client,
    url: &str,
    key: &str,
) -> Option<T> {
    let response = send_with_retry(client.get(url).bearer_auth(key)).await?;
    if !response.status().is_success() {
        return None;
    }
    let envelope = response.json::<GridDbEnvelope<T>>().await.ok()?;
    envelope.success.then_some(envelope.data).flatten()
}

/// Words a title keeps lowercase when it is rebuilt from a slug, the way a
/// person would write it: "Mio Memories in Orbit", not "Mio Memories In Orbit".
const SMALL_WORDS: &[&str] = &[
    "a", "an", "and", "as", "at", "by", "for", "in", "of", "on", "or", "the", "to", "vs",
];

/// Edition tags a store appends and the search index does not carry. Longer
/// phrases come before the shorter ones they end with, so "GOTY Edition" is
/// taken off whole rather than leaving "Edition" behind.
const EDITION_SUFFIXES: &[&str] = &[
    "Remastered",
    "Remaster",
    "Definitive Edition",
    "Game of the Year Edition",
    "GOTY Edition",
    "GOTY",
    "Deluxe Edition",
    "Complete Edition",
    "Ultimate Edition",
    "Enhanced Edition",
    "Director's Cut",
    "Gold Edition",
    "Standard Edition",
];

/// The longest query sent to any store.
const MAX_QUERY_CHARS: usize = 120;

/// A title that is really an identifier: a folder or package name such as
/// `mio-memories-in-orbit`. Anything with a space, a capital or other
/// punctuation was written by a person and is left alone, so "Spider-Man" and
/// "half-life" (one dash) are never mistaken for one.
fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '_'
        })
        && (value.contains('_') || value.matches('-').count() >= 2)
}

fn capitalise(word: &str) -> String {
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// The display-quality form of a title.
///
/// Library sources hand over whatever the install left behind: a slug, an
/// underscored folder name, or "Game - Subtitle" with a spaced dash. The same
/// text is shown on the card and searched on, so a title that reads well is
/// also one a store can match. Hyphens inside a word ("Spider-Man", "X-COM 2")
/// are part of the name and are kept.
pub fn tidy_title(raw: &str) -> String {
    let cleaned = raw
        .chars()
        .filter(|character| !character.is_control())
        .filter(|character| !matches!(character, '™' | '®' | '©'))
        .collect::<String>();
    let cleaned = cleaned.trim();

    if is_slug(cleaned) {
        let words = cleaned
            .split(['-', '_'])
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>();
        let last = words.len().saturating_sub(1);
        return words
            .iter()
            .enumerate()
            .map(|(index, word)| {
                if index != 0 && index != last && SMALL_WORDS.contains(word) {
                    (*word).to_owned()
                } else {
                    capitalise(word)
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
    }

    let spaced = cleaned.replace('_', " ");
    let tokens = spaced.split_whitespace().collect::<Vec<_>>();
    let mut tidy = String::new();
    for (index, token) in tokens.iter().enumerate() {
        let separator = matches!(*token, "-" | "\u{2013}" | "\u{2014}")
            && index != 0
            && index + 1 != tokens.len();
        if separator {
            // "A - B" reads as "A: B"; the colon hugs the word before it.
            tidy.push(':');
        } else {
            if !tidy.is_empty() {
                tidy.push(' ');
            }
            tidy.push_str(token);
        }
    }
    tidy
}

/// Drop one trailing edition tag ("Remastered", "Deluxe Edition"…), matched
/// case-insensitively and only as a whole word.
fn strip_edition_suffix(value: &str) -> Option<&str> {
    EDITION_SUFFIXES.iter().find_map(|suffix| {
        let start = value.len().checked_sub(suffix.len())?;
        if !value.is_char_boundary(start) || !value[start..].eq_ignore_ascii_case(suffix) {
            return None;
        }
        let head = value[..start].trim_end();
        (head.len() < start && !head.is_empty()).then_some(head)
    })
}

/// The searches to try for a title, most specific first: the whole tidied
/// title, the same without its edition tag, and the part before the first
/// subtitle separator. Stores index "Uncharted: Legacy of Thieves Collection"
/// and "Uncharted 4: A Thief's End" under neither "Remastered" nor the full
/// subtitle, so asking only once leaves many ordinary games artless.
///
/// A colon is dropped from the queries because Steam's search treats it as
/// noise at best. Empty and duplicate queries are skipped.
pub(crate) fn query_variants(title: &str) -> Vec<String> {
    let tidy = tidy_title(title);
    let collapse = |value: &str| value.split_whitespace().collect::<Vec<_>>().join(" ");

    let mut candidates = Vec::new();
    // A curly apostrophe is still an apostrophe: "Director’s Cut" must lose
    // its edition tag exactly like "Director's Cut" does.
    let full = collapse(&tidy.replace(':', " ").replace('\u{2019}', "'"));
    let mut trimmed = full.as_str();
    while let Some(head) = strip_edition_suffix(trimmed) {
        trimmed = head;
    }
    candidates.push(full.clone());
    candidates.push(trimmed.to_owned());
    if let Some((before, _)) = tidy.split_once(':') {
        let before = collapse(before);
        if before.chars().count() >= 2 {
            candidates.push(before);
        }
    }

    let mut variants: Vec<String> = Vec::new();
    for candidate in candidates {
        let candidate = candidate
            .chars()
            .take(MAX_QUERY_CHARS)
            .collect::<String>()
            .trim()
            .to_owned();
        if !candidate.is_empty() && !variants.contains(&candidate) {
            variants.push(candidate);
        }
    }
    variants
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
        assert!(
            set.for_role(ArtworkRole::Cover).len() > 1,
            "a fallback per role"
        );
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
        assert!(is_trusted_artwork_url(
            "https://cdn2.steamgriddb.com/hero/abc.png"
        ));
        assert!(!is_trusted_artwork_url(
            "http://cdn2.steamgriddb.com/hero/abc.png"
        ));
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

    /// The `styles` filter is what separates "high-resolution artwork" from
    /// "high-resolution artwork with the game's title painted across it".
    /// SteamGridDB tags `white_logo` and `material` uploads as exactly that, and
    /// an unfiltered request returns them mixed in with everything else — which
    /// is wrong for a card the app already composites the real wordmark onto.
    #[test]
    fn artwork_requests_ask_steamgriddb_to_leave_the_title_off() {
        let url = griddb_asset_url("grids", 42, "dimensions=600x900", GRID_STYLES);
        assert!(url.starts_with("https://www.steamgriddb.com/api/v2/grids/game/42?"));
        assert!(url.contains("dimensions=600x900"), "{url}");
        assert!(url.contains("styles=alternate,no_logo"), "{url}");
        // Joke and NSFW edits are tagged too, and a reset applies its answer
        // with nobody watching.
        assert!(url.contains("nsfw=false"), "{url}");
        assert!(url.contains("humor=false"), "{url}");
        // An animated grid is not something the media pipeline can store as a
        // cover.
        assert!(url.contains("types=static"), "{url}");
        assert!(!url.contains("white_logo"), "{url}");
        assert!(!url.contains("material"), "{url}");

        // The wordmark role is the one place a logo is the point, and it has no
        // dimension vocabulary — a wordmark is as wide as the words are.
        let logo = griddb_asset_url("logos", 42, "", LOGO_STYLES);
        assert!(logo.contains("styles=official,white"), "{logo}");
        assert!(!logo.contains("dimensions"), "{logo}");
        assert!(!logo.contains("?&"), "{logo}");
    }

    /// The style vocabulary is per asset kind, and a grid style sent to
    /// `/heroes` is answered with a flat 400 — which is what made every
    /// background request fail. A hero needs no de-logoed variant anyway: it is
    /// the scene Steam paints a separate wordmark over.
    #[test]
    fn a_hero_is_never_asked_for_a_style_only_grids_have() {
        let hero = griddb_asset_url("heroes", 42, "dimensions=3840x1240", HERO_STYLES);
        assert!(hero.contains("styles=alternate"), "{hero}");
        assert!(
            !hero.contains("no_logo"),
            "heroes reject `no_logo` outright: {hero}"
        );
        assert!(!hero.contains("white_logo"), "{hero}");
        assert!(!hero.contains("material"), "{hero}");
        // And every style named for a hero has to be one heroes actually accept.
        for style in HERO_STYLES.split(',') {
            assert!(
                ["alternate", "blurred", "material"].contains(&style),
                "{style} is not in the hero vocabulary"
            );
        }
    }

    #[test]
    fn a_slug_becomes_a_readable_title() {
        assert_eq!(tidy_title("mio-memories-in-orbit"), "Mio Memories in Orbit");
        assert_eq!(tidy_title("the-perfect-pencil"), "The Perfect Pencil");
        // A small word stays capitalised when it ends the title.
        assert_eq!(tidy_title("what-to-do-on"), "What to Do On");
        assert_eq!(tidy_title("my_cool_game"), "My Cool Game");
        // One dash is a name, not a slug.
        assert_eq!(tidy_title("half-life"), "half-life");
    }

    #[test]
    fn a_title_a_person_wrote_is_left_alone_apart_from_separators() {
        assert_eq!(tidy_title("Spider-Man"), "Spider-Man");
        assert_eq!(tidy_title("Half-Life 2"), "Half-Life 2");
        assert_eq!(tidy_title("X-COM 2"), "X-COM 2");
        assert_eq!(
            tidy_title("Assassin's Creed - Mirage"),
            "Assassin's Creed: Mirage"
        );
        assert_eq!(
            tidy_title("Dark Souls \u{2013} Remastered"),
            "Dark Souls: Remastered"
        );
        assert_eq!(
            tidy_title("Dark Souls \u{2014} Remastered"),
            "Dark Souls: Remastered"
        );
        assert_eq!(tidy_title("Old_Game_Name"), "Old Game Name");
        assert_eq!(tidy_title("  Halo\u{2122}   Infinite \n"), "Halo Infinite");
        assert_eq!(
            tidy_title("Forza\u{00ae} Horizon \u{00a9}"),
            "Forza Horizon"
        );
        assert_eq!(tidy_title("   "), "");
    }

    #[test]
    fn a_title_is_searched_under_progressively_looser_names() {
        assert_eq!(
            query_variants("Uncharted 4 A Thief's End - Remastered"),
            vec![
                "Uncharted 4 A Thief's End Remastered",
                "Uncharted 4 A Thief's End"
            ]
        );
        assert_eq!(
            query_variants("Dark Souls: Prepare to Die Edition"),
            vec!["Dark Souls Prepare to Die Edition", "Dark Souls"]
        );
        assert_eq!(
            query_variants("mio-memories-in-orbit")
                .first()
                .map(String::as_str),
            Some("Mio Memories in Orbit")
        );
        // Nothing to loosen: one query, not three copies of it.
        assert_eq!(
            query_variants("Halo\u{2122} Infinite"),
            vec!["Halo Infinite"]
        );
        assert!(query_variants("  ").is_empty());
    }

    #[test]
    fn edition_tags_come_off_whole_words_only() {
        let variants = query_variants("Resident Evil 4 Ultimate Edition");
        assert_eq!(
            variants,
            vec!["Resident Evil 4 Ultimate Edition", "Resident Evil 4"]
        );
        // "GOTY" inside a word is not an edition tag.
        assert_eq!(query_variants("NotGOTY"), vec!["NotGOTY"]);
        assert_eq!(query_variants("Fallout 3 goty edition")[1], "Fallout 3");
        // A title that is only a tag keeps it.
        assert_eq!(query_variants("Remastered"), vec!["Remastered"]);
    }

    #[test]
    fn queries_are_bounded() {
        let long = "word ".repeat(60);
        assert!(
            query_variants(&long)
                .iter()
                .all(|query| query.chars().count() <= 120)
        );
    }

    #[test]
    fn titles_compare_the_way_a_person_would() {
        // Punctuation and case never decide whether a title matches.
        assert_eq!(
            comparison_key("Marvel's Spider-Man"),
            comparison_key("marvel s spider man")
        );
        assert_ne!(
            comparison_key("Halo Infinite"),
            comparison_key("Halo Infinite OST")
        );
    }
}
