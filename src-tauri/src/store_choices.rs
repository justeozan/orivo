//! Where a game can be got on a device that cannot run it.
//!
//! An iPhone or an iPad runs no PC game, so "Install" there cannot hand a
//! download to Steam the way it does on a computer: `steam://` has no handler
//! on iOS, and the launcher answers `SteamUnsupported` — which is what the
//! button used to show. What it can honestly offer is the game's own iOS
//! version, when the App Store sells one, and the game's store page otherwise.
//!
//! Finding the iOS version is a search, so it is strict on purpose: a wrong
//! answer sends the player to somebody else's app, while a missed one only
//! leaves them the store page. The App Store entry has to be a game, carry the
//! same title give or take an edition, and come from a studio Steam names as
//! the game's developer or publisher. A port published by a porting house
//! (Netflix's Hades, Playdigious's Dead Cells) is missed by that last rule, and
//! that is the trade.

use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::Value;

use crate::store::StoreHttp;

const APP_STORE_SEARCH: &str = "https://itunes.apple.com/search";
/// The App Store's own genre id for Games.
const GAMES_GENRE_ID: &str = "6014";
/// Long enough to answer the tap that follows the menu, short enough that a
/// game Apple lists tomorrow is found on the next visit.
const REMEMBERED_FOR: Duration = Duration::from_secs(15 * 60);
const REMEMBERED_GAMES: usize = 32;

/// What a release is rather than which game it is. A title that differs from
/// the other by one of these, and only that, is the same game.
const EDITIONS: &[&str] = &[
    "directors cut",
    "complete edition",
    "definitive edition",
    "enhanced edition",
    "remastered",
    "goty edition",
    "game of the year edition",
    "deluxe edition",
    "special edition",
    "anniversary edition",
    "mobile",
    "pocket edition",
];

/// Words every other studio name carries, which identify nobody.
const STUDIO_NOISE: &[&str] = &[
    "a", "ab", "and", "bv", "co", "corp", "corporation", "digital", "entertainment", "game",
    "games", "gmbh", "group", "inc", "interactive", "kk", "limited", "llc", "ltd", "media", "of",
    "oy", "plc", "productions", "pty", "publishing", "s", "sa", "sas", "software", "spa", "srl",
    "studio", "studios", "the",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreChoice {
    /// The game's own iOS version on the App Store.
    AppStore {
        track_id: u64,
        seller: String,
    },
    /// The game's page on the Steam store.
    SteamPage { app_id: u32 },
}

/// One row of the menu "Install" opens. The id is all the WebView sends back.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoreChoiceView {
    pub id: String,
    pub label: String,
    pub detail: String,
}

impl StoreChoice {
    pub fn id(&self) -> &'static str {
        match self {
            Self::AppStore { .. } => "app-store",
            Self::SteamPage { .. } => "steam-page",
        }
    }

    pub fn view(&self) -> StoreChoiceView {
        let (label, detail) = match self {
            Self::AppStore { seller, .. } => {
                ("iPhone & iPad version".to_string(), format!("App Store · {seller}"))
            }
            Self::SteamPage { .. } => (
                "View on Steam".to_string(),
                "Its page on the Steam store".to_string(),
            ),
        };
        StoreChoiceView {
            id: self.id().to_string(),
            label,
            detail,
        }
    }

    /// Built from an identifier, never from a URL a response carried: the
    /// search result's own `trackViewUrl` is not trusted with the opener.
    pub fn url(&self) -> String {
        match self {
            Self::AppStore { track_id, .. } => format!("https://apps.apple.com/app/id{track_id}"),
            Self::SteamPage { app_id } => format!("https://store.steampowered.com/app/{app_id}/"),
        }
    }
}

/// Everything "Install" can offer for a Steam game here: its iOS version when
/// one is found, then its Steam page, which is always there.
pub async fn choices_for_steam_game(
    http: &dyn StoreHttp,
    title: &str,
    app_id: u32,
    region: &str,
) -> Vec<StoreChoice> {
    let mut choices = Vec::new();
    if let Some(native) = find_ios_version(http, title, app_id, region).await {
        choices.push(native);
    }
    choices.push(StoreChoice::SteamPage { app_id });
    choices
}

async fn find_ios_version(
    http: &dyn StoreHttp,
    title: &str,
    app_id: u32,
    region: &str,
) -> Option<StoreChoice> {
    // The library records no studio for a Steam game, so the store says who
    // made it before the App Store is asked who sells it.
    let details = http
        .get_json(&crate::store::steam_app_details_url(&app_id.to_string(), region))
        .await
        .ok()?;
    let (steam_name, studios) = steam_studios(&details, app_id);
    let search = http.get_json(&app_store_search_url(title, region)).await.ok()?;
    app_store_match(&search, title, &studios).or_else(|| {
        steam_name
            .filter(|name| normalise(name) != normalise(title))
            .and_then(|name| app_store_match(&search, &name, &studios))
    })
}

/// The public iTunes Search API, the same one the Store's App Store shelf
/// reads, for iPhone and iPad apps alike.
pub fn app_store_search_url(title: &str, region: &str) -> String {
    reqwest::Url::parse_with_params(
        APP_STORE_SEARCH,
        &[
            ("term", title),
            ("country", region),
            ("media", "software"),
            ("entity", "software,iPadSoftware"),
            ("limit", "15"),
        ],
    )
    .map(|url| url.to_string())
    .unwrap_or_default()
}

/// The Steam name and every developer and publisher of one `appdetails`
/// answer. An app Steam does not know has no studio, and so no match.
fn steam_studios(payload: &Value, app_id: u32) -> (Option<String>, Vec<String>) {
    let Some(data) = payload
        .get(app_id.to_string())
        .filter(|entry| entry.get("success").and_then(Value::as_bool) == Some(true))
        .and_then(|entry| entry.get("data"))
    else {
        return (None, Vec::new());
    };
    let name = data.get("name").and_then(Value::as_str).map(str::to_string);
    let studios = ["developers", "publishers"]
        .iter()
        .filter_map(|key| data.get(*key).and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    (name, studios)
}

/// The first App Store result that is this game: see the module header for
/// why each of the three conditions is there.
pub fn app_store_match(payload: &Value, title: &str, studios: &[String]) -> Option<StoreChoice> {
    let wanted = studio_words(studios);
    if wanted.is_empty() {
        return None;
    }
    payload
        .get("results")?
        .as_array()?
        .iter()
        .find_map(|result| {
            if !is_game(result) || !same_game(result.get("trackName")?.as_str()?, title) {
                return None;
            }
            let artist = result.get("artistName").and_then(Value::as_str).unwrap_or("");
            let seller = result
                .get("sellerName")
                .and_then(Value::as_str)
                .filter(|seller| !seller.trim().is_empty())
                .unwrap_or(artist);
            if studio_words(&[artist.to_string(), seller.to_string()]).is_disjoint(&wanted) {
                return None;
            }
            Some(StoreChoice::AppStore {
                track_id: result.get("trackId")?.as_u64()?,
                seller: seller.trim().to_string(),
            })
        })
}

fn is_game(result: &Value) -> bool {
    result
        .get("genreIds")
        .and_then(Value::as_array)
        .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(GAMES_GENRE_ID)))
        || result.get("primaryGenreName").and_then(Value::as_str) == Some("Games")
}

fn same_game(candidate: &str, title: &str) -> bool {
    let (candidate, title) = (normalise(candidate), normalise(title));
    if candidate.is_empty() || title.is_empty() {
        return false;
    }
    let differs_by_an_edition = |longer: &str, shorter: &str| {
        longer
            .strip_prefix(shorter)
            .and_then(|rest| rest.strip_prefix(' '))
            .is_some_and(|edition| EDITIONS.contains(&edition))
    };
    candidate == title
        || differs_by_an_edition(&candidate, &title)
        || differs_by_an_edition(&title, &candidate)
}

/// Lower case, `&` read as "and", marks and apostrophes dropped, everything
/// else that is not a letter or a digit a single space.
pub fn normalise(text: &str) -> String {
    let lowered = text.to_lowercase().replace('&', " and ");
    let spaced: String = lowered
        .chars()
        .filter(|character| !matches!(character, '\'' | '’' | '™' | '®' | '©'))
        .map(|character| if character.is_alphanumeric() { character } else { ' ' })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn studio_words(names: &[String]) -> HashSet<String> {
    names
        .iter()
        .flat_map(|name| {
            normalise(name)
                .split(' ')
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|word| !word.is_empty() && !STUDIO_NOISE.contains(&word.as_str()))
        .collect()
}

type Remembered = HashMap<String, (Instant, Vec<StoreChoice>)>;

fn remembered() -> &'static Mutex<Remembered> {
    static REMEMBERED: OnceLock<Mutex<Remembered>> = OnceLock::new();
    REMEMBERED.get_or_init(Default::default)
}

/// Keep what the menu offered, so the tap that answers it opens exactly that
/// and the WebView never has to carry a destination.
pub fn remember(game_id: &str, choices: &[StoreChoice]) {
    let Ok(mut remembered) = remembered().lock() else {
        return;
    };
    let now = Instant::now();
    remembered.retain(|_, (at, _)| now.duration_since(*at) < REMEMBERED_FOR);
    if remembered.len() >= REMEMBERED_GAMES
        && let Some(oldest) = remembered
            .iter()
            .min_by_key(|(_, (at, _))| *at)
            .map(|(id, _)| id.clone())
    {
        remembered.remove(&oldest);
    }
    remembered.insert(game_id.to_string(), (now, choices.to_vec()));
}

pub fn recall(game_id: &str, choice_id: &str) -> Option<StoreChoice> {
    let remembered = remembered().lock().ok()?;
    let (at, choices) = remembered.get(game_id)?;
    if at.elapsed() >= REMEMBERED_FOR {
        return None;
    }
    choices.iter().find(|choice| choice.id() == choice_id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{future::Future, pin::Pin};

    fn result(name: &str, artist: &str, seller: &str, genre: &str, track_id: u64) -> Value {
        json!({
            "kind": "software",
            "trackId": track_id,
            "trackName": name,
            "artistName": artist,
            "sellerName": seller,
            "primaryGenreName": if genre == GAMES_GENRE_ID { "Games" } else { "Utilities" },
            "genreIds": [genre, "7001"],
            "trackViewUrl": "https://apps.apple.com.evil.example/app",
        })
    }

    fn search(results: Vec<Value>) -> Value {
        json!({ "resultCount": results.len(), "results": results })
    }

    fn studios(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn finds_the_ios_version_of_the_same_game_by_the_same_studio() {
        let payload = search(vec![
            result("Stardew Valley Guide", "Some Fan", "Some Fan", GAMES_GENRE_ID, 1),
            result("Stardew Valley", "ConcernedApe", "ConcernedApe LLC", GAMES_GENRE_ID, 1_406_710_800),
        ]);
        assert_eq!(
            app_store_match(&payload, "Stardew Valley", &studios(&["ConcernedApe"])),
            Some(StoreChoice::AppStore {
                track_id: 1_406_710_800,
                seller: "ConcernedApe LLC".into(),
            })
        );
    }

    #[test]
    fn a_publisher_counts_as_much_as_the_developer() {
        let payload = search(vec![result(
            "DEATH STRANDING DIRECTOR'S CUT",
            "505 Games (US), Inc.",
            "505 Games (US), Inc.",
            GAMES_GENRE_ID,
            6_449_748_961,
        )]);
        let found = app_store_match(
            &payload,
            "DEATH STRANDING DIRECTOR'S CUT",
            &studios(&["KOJIMA PRODUCTIONS", "505 Games"]),
        );
        assert!(matches!(found, Some(StoreChoice::AppStore { track_id: 6_449_748_961, .. })));
    }

    #[test]
    fn an_edition_on_either_side_is_still_the_same_game() {
        assert!(same_game("Death Stranding Director's Cut", "DEATH STRANDING"));
        assert!(same_game("Ori", "Ori Definitive Edition"));
        assert!(same_game("Monument Valley™", "Monument Valley"));
        assert!(!same_game("Stray Souls", "Stray"));
        assert!(!same_game("Control Center", "Control"));
    }

    #[test]
    fn refuses_a_namesake_from_another_studio() {
        // An app called "Journey" by somebody else is the classic false match.
        let payload = search(vec![result("Journey", "Daily Journal Apps", "Daily Journal Apps", GAMES_GENRE_ID, 7)]);
        assert_eq!(app_store_match(&payload, "Journey", &studios(&["thatgamecompany"])), None);
    }

    #[test]
    fn refuses_an_app_that_is_not_a_game() {
        let payload = search(vec![result("Elden Ring", "FromSoftware", "FromSoftware", "6002", 9)]);
        assert_eq!(app_store_match(&payload, "Elden Ring", &studios(&["FromSoftware, Inc."])), None);
    }

    #[test]
    fn a_studio_name_made_only_of_common_words_matches_nobody() {
        let payload = search(vec![result("Hades", "Netflix, Inc.", "Netflix, Inc.", GAMES_GENRE_ID, 3)]);
        assert_eq!(app_store_match(&payload, "Hades", &studios(&["Games Studio Inc."])), None);
        assert_eq!(app_store_match(&payload, "Hades", &studios(&[])), None);
    }

    #[test]
    fn opens_pages_built_from_identifiers_only() {
        let native = StoreChoice::AppStore { track_id: 42, seller: "Studio".into() };
        assert_eq!(native.url(), "https://apps.apple.com/app/id42");
        assert_eq!(
            StoreChoice::SteamPage { app_id: 1_245_620 }.url(),
            "https://store.steampowered.com/app/1245620/"
        );
    }

    #[test]
    fn searches_iphone_and_ipad_apps_with_the_title_encoded() {
        let url = app_store_search_url("Baldur's Gate 3 & More", "fr");
        assert!(url.starts_with("https://itunes.apple.com/search?"));
        assert!(url.contains("term=Baldur%27s+Gate+3+%26+More"));
        assert!(url.contains("country=fr"));
        assert!(url.contains("entity=software%2CiPadSoftware"));
    }

    #[test]
    fn reads_every_developer_and_publisher_steam_names() {
        let payload = json!({
            "1245620": { "success": true, "data": {
                "name": "ELDEN RING",
                "developers": ["FromSoftware, Inc."],
                "publishers": ["FromSoftware, Inc.", "Bandai Namco Entertainment"],
            }}
        });
        let (name, found) = steam_studios(&payload, 1_245_620);
        assert_eq!(name.as_deref(), Some("ELDEN RING"));
        assert_eq!(found.len(), 3);
        assert_eq!(steam_studios(&json!({ "1245620": { "success": false } }), 1_245_620).1.len(), 0);
    }

    struct FakeHttp {
        steam: Result<Value, String>,
        apple: Result<Value, String>,
    }

    impl StoreHttp for FakeHttp {
        fn get_json<'a>(
            &'a self,
            url: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
            let answer = if url.starts_with("https://store.steampowered.com/api/appdetails") {
                self.steam.clone()
            } else if url.starts_with(APP_STORE_SEARCH) {
                self.apple.clone()
            } else {
                Err(format!("unexpected request {url}"))
            };
            Box::pin(async move { answer })
        }
    }

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        tauri::async_runtime::block_on(future)
    }

    #[test]
    fn offers_the_ios_version_first_and_the_steam_page_after_it() {
        let http = FakeHttp {
            steam: Ok(json!({ "413150": { "success": true, "data": {
                "name": "Stardew Valley", "developers": ["ConcernedApe"], "publishers": ["ConcernedApe"]
            }}})),
            apple: Ok(search(vec![result("Stardew Valley", "ConcernedApe", "ConcernedApe LLC", GAMES_GENRE_ID, 5)])),
        };
        let choices = block_on(choices_for_steam_game(&http, "Stardew Valley", 413_150, "US"));
        assert_eq!(
            choices,
            vec![
                StoreChoice::AppStore { track_id: 5, seller: "ConcernedApe LLC".into() },
                StoreChoice::SteamPage { app_id: 413_150 },
            ]
        );
    }

    #[test]
    fn an_unreachable_store_still_leaves_the_steam_page() {
        let http = FakeHttp {
            steam: Err("offline".into()),
            apple: Err("offline".into()),
        };
        let choices = block_on(choices_for_steam_game(&http, "Elden Ring", 1_245_620, "US"));
        assert_eq!(choices, vec![StoreChoice::SteamPage { app_id: 1_245_620 }]);
    }

    #[test]
    fn the_answer_to_a_menu_is_the_choice_it_offered() {
        let choices = [
            StoreChoice::AppStore { track_id: 8, seller: "Studio".into() },
            StoreChoice::SteamPage { app_id: 10 },
        ];
        remember("steam:10", &choices);
        assert_eq!(recall("steam:10", "app-store"), Some(choices[0].clone()));
        assert_eq!(recall("steam:10", "steam-page"), Some(choices[1].clone()));
        assert_eq!(recall("steam:10", "something-else"), None);
        assert_eq!(recall("steam:11", "steam-page"), None);
    }
}
