//! The store media of a game — its trailers and screenshots — for the detail
//! page's gallery.
//!
//! This is what Steam's own store page shows under the title, so it comes from
//! the same public `appdetails` answer. Nothing is downloaded: the WebView
//! loads these CDN URLs directly, the same way a Steam game's cover already
//! works, which is why every URL that leaves this module is checked against the
//! artwork host allowlist first.

use crate::catalog::{Game, LaunchTarget};
use crate::game_artwork;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;

const APP_DETAILS_ENDPOINT: &str = "https://store.steampowered.com/api/appdetails";

/// More than this is a scroll nobody finishes, and the rail renders every tile.
const MAX_SCREENSHOTS: usize = 24;
const MAX_MOVIES: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreMedia {
    pub screenshots: Vec<StoreScreenshot>,
    pub movies: Vec<StoreMovie>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreScreenshot {
    pub thumb_url: String,
    pub full_url: String,
}

/// One trailer. Steam stopped publishing the same formats for every title, so
/// each is optional; a movie is only kept when at least one of them plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreMovie {
    pub title: String,
    pub thumb_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mp4_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hls_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webm_url: Option<String>,
}

/// What Steam already said for each game this session. Reopening a detail page
/// is then instant, and a game with no store media does not ask again.
#[derive(Default)]
pub struct StoreMediaCache {
    answers: Mutex<HashMap<String, StoreMedia>>,
}

impl StoreMediaCache {
    pub fn get(&self, game_id: &str) -> Option<StoreMedia> {
        self.answers.lock().ok()?.get(game_id).cloned()
    }

    fn remember(&self, game_id: &str, media: &StoreMedia) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.insert(game_id.to_owned(), media.clone());
        }
    }
}

/// The Steam app a library game is, when its own record says so: a Steam
/// launch target, a `steam:<id>` identifier, or an artwork URL already pointing
/// into `steam/apps/<id>/`. Only a title search is left to the caller.
pub fn steam_app_id_of(game: &Game) -> Option<u64> {
    if let LaunchTarget::Steam { app_id } = &game.launch_target {
        if *app_id > 0 {
            return Some(u64::from(*app_id));
        }
    }
    steam_app_id_of_id(&game.id).or_else(|| {
        game.extra
            .values()
            .filter_map(Value::as_str)
            .find_map(app_id_from_artwork_url)
    })
}

/// The app in a `steam:<id>` identifier, which is all a Store card (a game
/// outside the library) has to go on.
pub fn steam_app_id_of_id(game_id: &str) -> Option<u64> {
    game_id
        .strip_prefix("steam:")
        .and_then(|id| id.parse::<u64>().ok())
        .filter(|id| *id > 0)
}

/// The id in `https://<steam cdn>/steam/apps/<id>/<asset>`. Other hosts are
/// ignored: a GOG or Epic URL may contain the same path shape by coincidence.
fn app_id_from_artwork_url(url: &str) -> Option<u64> {
    if !game_artwork::is_trusted_artwork_url(url) {
        return None;
    }
    let (_, rest) = url.split_once("/steam/apps/")?;
    let digits = rest.split('/').next()?;
    digits.parse::<u64>().ok().filter(|id| *id > 0)
}

/// Resolve and fetch a game's store media. Never an error: no Steam page,
/// no network and a malformed answer all read as "nothing to show", because the
/// gallery is decoration and the page works without it.
///
/// `known_app_id` is what the game's own record already says; only without it
/// is the title searched on Steam.
pub async fn fetch(
    cache: &StoreMediaCache,
    game_id: &str,
    title: &str,
    known_app_id: Option<u64>,
) -> StoreMedia {
    if let Some(cached) = cache.get(game_id) {
        return cached;
    }
    let Some(client) = game_artwork::client() else {
        return StoreMedia::default();
    };
    let app_id = match known_app_id {
        Some(app_id) => app_id,
        None => {
            let queries = game_artwork::query_variants(title);
            match game_artwork::steam_app_id_for_title(&client, &queries).await {
                Some(app_id) => app_id,
                // No match could be a title Steam does not sell or a search that
                // never reached it; either way it is cheaper to try again next
                // time than to remember a possibly wrong "nothing".
                None => return StoreMedia::default(),
            }
        }
    };

    let Some(body) = fetch_app_details(&client, app_id).await else {
        return StoreMedia::default();
    };
    let media = parse_app_details(&body, app_id);
    cache.remember(game_id, &media);
    media
}

/// Steam's answer for one app, or `None` when it could not be reached or read.
/// A reachable store that has no page for the id is still an answer (the body
/// says `success: false`), so it is cached as empty by the caller.
async fn fetch_app_details(client: &reqwest::Client, app_id: u64) -> Option<Value> {
    let response = client
        .get(APP_DETAILS_ENDPOINT)
        .query(&[("appids", app_id.to_string().as_str()), ("l", "english")])
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<Value>().await.ok()
}

/// Read `screenshots` and `movies` out of an `appdetails` body. The body is
/// keyed by app id, and every URL is dropped unless it is HTTPS on a Steam CDN.
fn parse_app_details(body: &Value, app_id: u64) -> StoreMedia {
    let Some(data) = body
        .get(app_id.to_string())
        .filter(|entry| entry.get("success").and_then(Value::as_bool) == Some(true))
        .and_then(|entry| entry.get("data"))
    else {
        return StoreMedia::default();
    };

    let trusted = |value: Option<&Value>| -> Option<String> {
        value
            .and_then(Value::as_str)
            .filter(|url| game_artwork::is_trusted_artwork_url(url))
            .map(str::to_owned)
    };

    let screenshots = data
        .get("screenshots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|shot| {
            let full_url = trusted(shot.get("path_full"))?;
            // A thumbnail is only a smaller copy: without one, the full image
            // stands in rather than losing the screenshot.
            let thumb_url = trusted(shot.get("path_thumbnail")).unwrap_or_else(|| full_url.clone());
            Some(StoreScreenshot {
                thumb_url,
                full_url,
            })
        })
        .take(MAX_SCREENSHOTS)
        .collect();

    let movies = data
        .get("movies")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|movie| {
            // `max` is the best Steam has; `480` is what older entries carry.
            let sized = |key: &str| {
                let sizes = movie.get(key)?;
                trusted(sizes.get("max")).or_else(|| trusted(sizes.get("480")))
            };
            let mp4_url = sized("mp4");
            let webm_url = sized("webm");
            let hls_url = trusted(movie.get("hls_h264"));
            if mp4_url.is_none() && webm_url.is_none() && hls_url.is_none() {
                return None;
            }
            let title = movie
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or("Trailer")
                .to_owned();
            // No poster, no tile: the rail has nothing to draw for it.
            let thumb_url = trusted(movie.get("thumbnail"))?;
            Some(StoreMovie {
                title,
                thumb_url,
                mp4_url,
                hls_url,
                webm_url,
            })
        })
        .take(MAX_MOVIES)
        .collect();

    StoreMedia {
        screenshots,
        movies,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "1245620": {
                "success": true,
                "data": {
                    "name": "ELDEN RING",
                    "screenshots": [
                        {
                            "id": 0,
                            "path_thumbnail": "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/1245620/ss_1.600x338.jpg?t=1",
                            "path_full": "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/1245620/ss_1.1920x1080.jpg?t=1"
                        },
                        {
                            "id": 1,
                            "path_full": "https://cdn.akamai.steamstatic.com/steam/apps/1245620/ss_2.1920x1080.jpg?t=1"
                        },
                        {
                            "id": 2,
                            "path_thumbnail": "https://evil.example/t.jpg",
                            "path_full": "https://evil.example/f.jpg"
                        },
                        {
                            "id": 3,
                            "path_thumbnail": "http://cdn.akamai.steamstatic.com/steam/apps/1245620/ss_3.600x338.jpg",
                            "path_full": "http://cdn.akamai.steamstatic.com/steam/apps/1245620/ss_3.1920x1080.jpg"
                        }
                    ],
                    "movies": [
                        {
                            "id": 256889598,
                            "name": "Launch Trailer",
                            "thumbnail": "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/256889598/movie.293x165.jpg?t=1",
                            "webm": {
                                "480": "https://video.akamai.steamstatic.com/store_trailers/256889598/movie480_vp9.webm?t=1",
                                "max": "https://video.akamai.steamstatic.com/store_trailers/256889598/movie_max_vp9.webm?t=1"
                            },
                            "mp4": {
                                "480": "https://video.akamai.steamstatic.com/store_trailers/256889598/movie480.mp4?t=1",
                                "max": "https://video.akamai.steamstatic.com/store_trailers/256889598/movie_max.mp4?t=1"
                            },
                            "highlight": true
                        },
                        {
                            "id": 2,
                            "name": "Streamed only",
                            "thumbnail": "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/2/movie.293x165.jpg",
                            "hls_h264": "https://video.akamai.steamstatic.com/store_trailers/2/hls_264_master.m3u8?t=1"
                        },
                        {
                            "id": 3,
                            "name": "Old entry",
                            "thumbnail": "https://cdn.akamai.steamstatic.com/steam/apps/3/movie.293x165.jpg",
                            "mp4": { "480": "https://cdn.akamai.steamstatic.com/steam/apps/3/movie480.mp4" }
                        },
                        {
                            "id": 4,
                            "name": "Off-CDN",
                            "thumbnail": "https://shared.akamai.steamstatic.com/a.jpg",
                            "mp4": { "max": "https://evil.example/movie.mp4" }
                        },
                        {
                            "id": 5,
                            "name": "No poster",
                            "mp4": { "max": "https://video.akamai.steamstatic.com/store_trailers/5/movie_max.mp4" }
                        }
                    ]
                }
            }
        })
    }

    #[test]
    fn reads_screenshots_in_order_and_drops_untrusted_urls() {
        let media = parse_app_details(&sample(), 1245620);
        assert_eq!(media.screenshots.len(), 2);
        assert!(media.screenshots[0].full_url.contains("ss_1.1920x1080"));
        assert!(media.screenshots[0].thumb_url.contains("ss_1.600x338"));
        // A screenshot without a thumbnail uses the full image for both.
        assert_eq!(
            media.screenshots[1].thumb_url,
            media.screenshots[1].full_url
        );
    }

    #[test]
    fn reads_every_playable_format_of_a_movie() {
        let media = parse_app_details(&sample(), 1245620);
        let first = &media.movies[0];
        assert_eq!(first.title, "Launch Trailer");
        assert!(first.mp4_url.as_deref().unwrap().ends_with("movie_max.mp4?t=1"));
        assert!(first
            .webm_url
            .as_deref()
            .unwrap()
            .ends_with("movie_max_vp9.webm?t=1"));
        assert!(first.hls_url.is_none());
        assert!(first.thumb_url.contains("movie.293x165"));
    }

    #[test]
    fn keeps_a_stream_only_movie_and_falls_back_to_the_480_file() {
        let media = parse_app_details(&sample(), 1245620);
        assert_eq!(media.movies.len(), 3);
        let streamed = &media.movies[1];
        assert!(streamed.mp4_url.is_none() && streamed.webm_url.is_none());
        assert!(streamed.hls_url.as_deref().unwrap().contains("hls_264_master"));
        let old = &media.movies[2];
        assert!(old.mp4_url.as_deref().unwrap().ends_with("movie480.mp4"));
    }

    #[test]
    fn an_unknown_or_failed_app_yields_nothing() {
        assert_eq!(
            parse_app_details(&json!({"1": {"success": false}}), 1),
            StoreMedia::default()
        );
        assert_eq!(parse_app_details(&json!(null), 1), StoreMedia::default());
        // A free game's `data` can be an empty array, never an object.
        assert_eq!(
            parse_app_details(&json!({"1": {"success": true, "data": []}}), 1),
            StoreMedia::default()
        );
    }

    #[test]
    fn serialises_camel_case_and_omits_absent_formats() {
        let media = parse_app_details(&sample(), 1245620);
        let value = serde_json::to_value(&media).unwrap();
        assert!(value["screenshots"][0]["thumbUrl"].is_string());
        assert!(value["screenshots"][0]["fullUrl"].is_string());
        assert!(value["movies"][0]["mp4Url"].is_string());
        assert!(value["movies"][1].get("mp4Url").is_none());
        assert!(value["movies"][1]["hlsUrl"].is_string());
    }

    #[test]
    fn resolves_the_steam_app_from_the_record_before_any_search() {
        let game = |value: Value| serde_json::from_value::<Game>(value).unwrap();
        assert_eq!(
            steam_app_id_of(&game(json!({"id": "steam:480", "title": "Spacewar"}))),
            Some(480)
        );
        assert_eq!(
            steam_app_id_of(&game(json!({
                "id": "epic:abc",
                "title": "Some Game",
                "coverUrl": "https://cdn.cloudflare.steamstatic.com/steam/apps/620/library_600x900.jpg"
            }))),
            Some(620)
        );
        assert_eq!(
            steam_app_id_of(&game(json!({"id": "epic:abc", "title": "Some Game"}))),
            None
        );
    }

    #[test]
    fn finds_the_steam_app_in_an_artwork_url_only_on_a_steam_cdn() {
        assert_eq!(
            app_id_from_artwork_url(
                "https://cdn.cloudflare.steamstatic.com/steam/apps/570/library_hero.jpg"
            ),
            Some(570)
        );
        assert_eq!(
            app_id_from_artwork_url("https://evil.example/steam/apps/570/x.jpg"),
            None
        );
        assert_eq!(app_id_from_artwork_url("https://cdn.cloudflare.steamstatic.com/x"), None);
    }
}
