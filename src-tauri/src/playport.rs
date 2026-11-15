//! Playport, on an iPhone or an iPad that has it.
//!
//! [Playport](https://playport.dev) runs Windows games on a stock iPhone —
//! Wine, the FEX translator and DXMT inside one app, on the phone's own CPU and
//! GPU — and installs them from the same Steam, GOG and Epic Games accounts
//! Orivo reads libraries from. An iOS build of Orivo runs no PC game, so on a
//! device that has Playport it is the one row of the Install menu that leads to
//! playing the copy the player already owns (`store_choices`).
//!
//! Orivo asks two things of it, and its URL scheme allows nothing more:
//!
//! - whether it is installed: `canOpenURL` on `playport://`, which iOS only
//!   answers for a scheme the asking app declared beforehand
//!   (`LSApplicationQueriesSchemes` in `src-tauri/Info.ios.plist`);
//! - to come to the front: `playport://`. Its handler reads nothing from the
//!   URL — Playport's own interface is the only way into it, by its decision
//!   0012 — so Orivo cannot name the game, and the row does not claim to. The
//!   game is in Playport's Library, under the account that owns it.
//!
//! The URL carries nothing, so an app that answered the scheme in Playport's
//! place would learn nothing from Orivo either.
//!
//! Playport is sideloaded, never installed from the App Store, so Orivo offers
//! it only on a device where someone already installed it, and names, links or
//! recommends it nowhere else.
//!
//! What the row says about a game comes from Playport's public compatibility
//! list, which its maintainers keep from the games they and its players have
//! run. It is read when a row is about to be offered and kept for hours; a list
//! that cannot be read costs the row its verdict and nothing else.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::Value;
use tauri::AppHandle;

use crate::{
    catalog::{Game, LaunchTarget},
    store::{StoreHttp, UrlOpener},
    store_choices::normalise,
};

/// The scheme Playport registers. Opening it only brings Playport forward.
const URL: &str = "playport://";

const COMPATIBILITY_LIST: &str = "https://playport.dev/compatibility/games.json";
/// The list moves with Playport's releases, which come days apart.
const LIST_KEPT_FOR: Duration = Duration::from_secs(6 * 60 * 60);
/// A list that could not be read is asked for again, but not on every tap.
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);
/// A verdict is worth a short wait, not the menu: the store requests it runs
/// beside are allowed twelve seconds, and a GOG or Epic game has nothing else
/// to wait for.
const LIST_WAIT: Duration = Duration::from_secs(4);
/// GOG product ids are integers; anything longer than a `u64` is not one.
const MAX_GOG_PRODUCT_ID_LENGTH: usize = 20;

/// A game the way Playport's list names it: Steam and GOG games by their
/// store's id, Epic Games ones by title, because the list carries no Epic id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayportGame {
    Steam { app_id: u32 },
    Gog { product_id: String },
    Epic { title: String },
}

/// What Playport's compatibility list says about one game, in its own terms.
///
/// The list once told "works" from "issues"; since November 2026 it says
/// "runs" for both, and its own page still reads the two old words as "runs".
/// Orivo does the same, so an older copy of the list says the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compatibility {
    /// "Boots to a menu or further on the phone."
    Runs,
    /// "Does not reach a menu yet."
    Broken,
    /// Nobody has told the list about this game either way.
    Unlisted,
}

/// The game Playport would install for this library entry, when it is one
/// Playport can take: a Windows game from Steam, GOG or Epic Games. A store
/// that published its platforms without Windows has given Playport nothing to
/// run. One that published none is given the benefit of the doubt, since
/// nearly every PC game ships for Windows, and the list says how it goes.
pub fn game_for(game: &Game, platforms: &[String]) -> Option<PlayportGame> {
    if !platforms.is_empty() && !platforms.iter().any(|platform| platform == "windows") {
        return None;
    }
    match &game.launch_target {
        LaunchTarget::Steam { app_id } if *app_id > 0 => {
            Some(PlayportGame::Steam { app_id: *app_id })
        }
        LaunchTarget::Provider { provider, app_ref } if provider == "gog" => {
            let product_id = app_ref.trim();
            (!product_id.is_empty()
                && product_id.len() <= MAX_GOG_PRODUCT_ID_LENGTH
                && product_id.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| PlayportGame::Gog {
                product_id: product_id.to_string(),
            })
        }
        LaunchTarget::Provider { provider, .. } if provider == "epic" => {
            let title = game.title.trim();
            (!title.is_empty()).then(|| PlayportGame::Epic {
                title: title.to_string(),
            })
        }
        _ => None,
    }
}

/// One entry of the list, as far as Orivo reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listed {
    store: String,
    id: Option<String>,
    name: String,
    verdict: Compatibility,
}

/// Every entry Orivo can use. An entry it cannot read is skipped rather than
/// failing the whole list: a status Playport adds later is no reason to lose
/// the verdicts it already gave.
fn read_list(payload: &Value) -> Option<Vec<Listed>> {
    let games = payload.get("games")?.as_array()?;
    Some(
        games
            .iter()
            .filter_map(|entry| {
                let verdict = match entry.get("status")?.as_str()? {
                    "runs" | "works" | "issues" => Compatibility::Runs,
                    "broken" => Compatibility::Broken,
                    _ => return None,
                };
                // The list writes ids as strings today; a number is the same id.
                let id = entry.get("id").and_then(|id| {
                    id.as_str()
                        .map(str::to_string)
                        .or_else(|| id.as_u64().map(|id| id.to_string()))
                });
                Some(Listed {
                    store: entry.get("store")?.as_str()?.to_string(),
                    id,
                    name: entry.get("name")?.as_str()?.to_string(),
                    verdict,
                })
            })
            .collect(),
    )
}

/// What the list says about one game: the entry with the same store and id,
/// or, for Epic Games, the same title.
fn verdict(list: &[Listed], game: &PlayportGame) -> Compatibility {
    list.iter()
        .find(|entry| match game {
            PlayportGame::Steam { app_id } => {
                entry.store == "steam" && entry.id.as_deref() == Some(app_id.to_string().as_str())
            }
            PlayportGame::Gog { product_id } => {
                entry.store == "gog" && entry.id.as_deref() == Some(product_id.as_str())
            }
            PlayportGame::Epic { title } => {
                entry.store == "epic" && normalise(&entry.name) == normalise(title)
            }
        })
        .map_or(Compatibility::Unlisted, |entry| entry.verdict)
}

type Kept = Option<(Instant, Option<Arc<Vec<Listed>>>)>;

/// Playport's compatibility list, read once and kept: `LIST_KEPT_FOR` when it
/// was read, `RETRY_AFTER` when it could not be.
pub struct CompatibilityList {
    kept: Mutex<Kept>,
}

/// The one list every menu reads.
pub static COMPATIBILITY: CompatibilityList = CompatibilityList::new();

impl CompatibilityList {
    pub const fn new() -> Self {
        Self {
            kept: Mutex::new(None),
        }
    }

    /// The list's verdict on one game, or `None` when the list could not be
    /// read: the row then says where it leads and nothing about how it goes.
    pub async fn verdict(
        &self,
        http: &dyn StoreHttp,
        game: &PlayportGame,
    ) -> Option<Compatibility> {
        self.verdict_at(http, game, Instant::now()).await
    }

    async fn verdict_at(
        &self,
        http: &dyn StoreHttp,
        game: &PlayportGame,
        now: Instant,
    ) -> Option<Compatibility> {
        let list = match self.kept_at(now) {
            Some(kept) => kept,
            None => {
                let read = tokio::time::timeout(LIST_WAIT, http.get_json(COMPATIBILITY_LIST))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .and_then(|payload| read_list(&payload))
                    .map(Arc::new);
                if let Ok(mut kept) = self.kept.lock() {
                    *kept = Some((now, read.clone()));
                }
                read
            }
        };
        list.map(|list| verdict(&list, game))
    }

    /// What is kept and still fresh: `Some(None)` is a failure too recent to
    /// ask again, `None` that there is nothing worth using.
    fn kept_at(&self, now: Instant) -> Option<Option<Arc<Vec<Listed>>>> {
        let kept = self.kept.lock().ok()?;
        let (at, list) = kept.as_ref()?;
        let kept_for = if list.is_some() {
            LIST_KEPT_FOR
        } else {
            RETRY_AFTER
        };
        (now.saturating_duration_since(*at) < kept_for).then(|| list.clone())
    }
}

/// Whether Playport is installed on this device. Only an iOS build can ask.
/// `canOpenURL:` itself may be called from any thread, but `objc2` hands out
/// the `UIApplication` it belongs to on the main thread only, so the question
/// is taken there.
pub async fn installed(app: &AppHandle) -> bool {
    #[cfg(target_os = "ios")]
    {
        let (answer, answered) = tokio::sync::oneshot::channel();
        if app
            .run_on_main_thread(move || {
                let _ = answer.send(ios::can_open());
            })
            .is_err()
        {
            return false;
        }
        answered.await.unwrap_or(false)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = app;
        false
    }
}

/// Bring Playport forward, if it is still there. A menu offered a minute ago
/// can be answered after the app was deleted, and a URL no app answers opens
/// nothing without saying so.
pub fn open(installed: bool, opener: &dyn UrlOpener) -> Result<(), String> {
    if !installed {
        return Err("Playport is no longer installed on this device.".to_string());
    }
    opener.open(URL)
}

/// For the WebView, which keeps a GOG or Epic Games game's Install live only
/// while Playport is there to take it: on iOS those stores have no other
/// answer to give.
#[tauri::command]
pub async fn is_playport_installed(app: AppHandle) -> bool {
    installed(&app).await
}

#[cfg(target_os = "ios")]
mod ios {
    use objc2::MainThreadMarker;
    use objc2_foundation::{NSString, NSURL};
    use objc2_ui_kit::UIApplication;

    /// `canOpenURL:`. Off the main thread there is no `UIApplication` to ask,
    /// so the answer there is `false`.
    pub fn can_open() -> bool {
        let Some(main_thread) = MainThreadMarker::new() else {
            return false;
        };
        let Some(url) = NSURL::URLWithString(&NSString::from_str(super::URL)) else {
            return false;
        };
        UIApplication::sharedApplication(main_thread).canOpenURL(&url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::GameSource;
    use serde_json::json;
    use std::{
        collections::BTreeMap,
        future::Future,
        pin::Pin,
        sync::atomic::{AtomicUsize, Ordering},
    };

    fn game(title: &str, source: GameSource, launch_target: LaunchTarget) -> Game {
        Game {
            id: format!("test:{title}"),
            title: title.to_string(),
            executable_path: None,
            source,
            source_id: None,
            launch_target,
            alternate_launch_targets: Vec::new(),
            installation_path: None,
            working_directory: None,
            arguments: Vec::new(),
            description: None,
            metadata: None,
            artwork_path: None,
            artwork_source_path: None,
            cover_path: None,
            cover_source_path: None,
            home_image_path: None,
            landscape_image_path: None,
            logo_path: None,
            hidden: false,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        }
    }

    fn steam(app_id: u32) -> Game {
        game(
            "Hollow Knight",
            GameSource::Steam,
            LaunchTarget::Steam { app_id },
        )
    }

    fn provider(title: &str, provider: &str, app_ref: &str) -> Game {
        let source = GameSource::from_provider_token(provider).unwrap();
        game(
            title,
            source,
            LaunchTarget::Provider {
                provider: provider.to_string(),
                app_ref: app_ref.to_string(),
            },
        )
    }

    fn platforms(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn takes_a_windows_game_from_each_store_playport_signs_in_to() {
        assert_eq!(
            game_for(&steam(367_520), &platforms(&["windows", "macos", "linux"])),
            Some(PlayportGame::Steam { app_id: 367_520 })
        );
        assert_eq!(
            game_for(
                &provider("BioShock", "gog", "2022341186"),
                &platforms(&["windows"])
            ),
            Some(PlayportGame::Gog {
                product_id: "2022341186".into()
            })
        );
        assert_eq!(
            game_for(
                &provider("Super Meat Boy", "epic", "ns:item:SuperMeatBoy"),
                &platforms(&["windows"])
            ),
            Some(PlayportGame::Epic {
                title: "Super Meat Boy".into()
            })
        );
    }

    #[test]
    fn a_game_that_published_no_platforms_is_given_the_benefit_of_the_doubt() {
        assert!(game_for(&steam(367_520), &[]).is_some());
    }

    #[test]
    fn refuses_a_game_with_nothing_for_windows() {
        // GOG sells Linux-only titles: Playport would have nothing to run.
        assert_eq!(
            game_for(
                &provider("Linux Only", "gog", "1234"),
                &platforms(&["linux"])
            ),
            None
        );
        assert_eq!(game_for(&steam(42), &platforms(&["macos"])), None);
    }

    #[test]
    fn refuses_the_stores_playport_does_not_sign_in_to() {
        let ubisoft = provider("Anno 1800", "ubisoft", "4553");
        assert_eq!(game_for(&ubisoft, &platforms(&["windows"])), None);
        let local = game("Celeste", GameSource::Local, LaunchTarget::Direct);
        assert_eq!(game_for(&local, &[]), None);
    }

    #[test]
    fn refuses_an_identity_that_is_not_one() {
        assert_eq!(game_for(&steam(0), &[]), None);
        assert_eq!(game_for(&provider("BioShock", "gog", "12ab"), &[]), None);
        assert_eq!(game_for(&provider("BioShock", "gog", ""), &[]), None);
        assert_eq!(game_for(&provider("  ", "epic", "ns:item:app"), &[]), None);
    }

    fn list() -> Value {
        json!({
            "comment": "Playport's public compatibility list",
            "updated": "2026-10-13",
            "games": [
                { "name": "Among Us", "store": "steam", "id": "945360", "status": "runs" },
                { "name": "Baldur's Gate: Enhanced Edition", "store": "gog", "id": "1207666353", "status": "runs" },
                { "name": "Castle Crashers", "store": "steam", "id": "204360", "status": "broken" },
                { "name": "Death's Door", "store": "epic", "status": "runs" },
                // The two words the list used before "runs", which mean it.
                { "name": "Hollow Knight", "store": "steam", "id": 367520, "status": "works" },
                { "name": "Portal 2", "store": "steam", "id": "620", "status": "issues" },
                { "name": "Need for Speed: The Run", "store": "other", "status": "broken" },
                { "name": "Some Future Game", "store": "steam", "id": "1", "status": "loads-to-menu" },
                { "name": "No Status", "store": "steam", "id": "2" },
            ],
        })
    }

    #[test]
    fn reads_every_entry_it_understands_and_skips_the_rest() {
        let read = read_list(&list()).unwrap();
        assert_eq!(read.len(), 7);
        assert!(read.iter().all(|entry| entry.id.as_deref() != Some("1")));
        assert_eq!(read_list(&json!({ "games": "not a list" })), None);
        assert_eq!(read_list(&json!([])), None);
    }

    #[test]
    fn finds_a_game_by_its_store_and_id() {
        let read = read_list(&list()).unwrap();
        assert_eq!(
            verdict(&read, &PlayportGame::Steam { app_id: 945_360 }),
            Compatibility::Runs
        );
        assert_eq!(
            verdict(&read, &PlayportGame::Steam { app_id: 204_360 }),
            Compatibility::Broken
        );
        // A numeric id is the same id.
        assert_eq!(
            verdict(&read, &PlayportGame::Steam { app_id: 367_520 }),
            Compatibility::Runs
        );
        // "issues", from before the list said "runs", is read as "runs".
        assert_eq!(
            verdict(&read, &PlayportGame::Steam { app_id: 620 }),
            Compatibility::Runs
        );
        assert_eq!(
            verdict(
                &read,
                &PlayportGame::Gog {
                    product_id: "1207666353".into()
                }
            ),
            Compatibility::Runs
        );
    }

    #[test]
    fn an_id_from_another_store_is_another_game() {
        let read = read_list(&list()).unwrap();
        assert_eq!(
            verdict(
                &read,
                &PlayportGame::Gog {
                    product_id: "945360".into()
                }
            ),
            Compatibility::Unlisted
        );
        assert_eq!(
            verdict(&read, &PlayportGame::Steam { app_id: 1 }),
            Compatibility::Unlisted
        );
    }

    #[test]
    fn finds_an_epic_game_by_its_title_since_the_list_has_no_epic_id() {
        let read = read_list(&list()).unwrap();
        assert_eq!(
            verdict(
                &read,
                &PlayportGame::Epic {
                    title: "DEATH'S DOOR".into()
                }
            ),
            Compatibility::Runs
        );
        // The same title under another store says nothing about Epic's copy.
        assert_eq!(
            verdict(
                &read,
                &PlayportGame::Epic {
                    title: "Among Us".into()
                }
            ),
            Compatibility::Unlisted
        );
    }

    struct CountingHttp {
        answer: Result<Value, String>,
        requests: AtomicUsize,
    }

    impl CountingHttp {
        fn new(answer: Result<Value, String>) -> Self {
            Self {
                answer,
                requests: AtomicUsize::new(0),
            }
        }
    }

    impl StoreHttp for CountingHttp {
        fn get_json<'a>(
            &'a self,
            url: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            let answer = if url == COMPATIBILITY_LIST {
                self.answer.clone()
            } else {
                Err(format!("unexpected request {url}"))
            };
            Box::pin(async move { answer })
        }
    }

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        tauri::async_runtime::block_on(future)
    }

    const HOLLOW_KNIGHT: PlayportGame = PlayportGame::Steam { app_id: 367_520 };

    #[test]
    fn reads_the_list_once_for_every_menu_that_follows() {
        let http = CountingHttp::new(Ok(list()));
        let kept = CompatibilityList::new();
        let start = Instant::now();
        assert_eq!(
            block_on(kept.verdict_at(&http, &HOLLOW_KNIGHT, start)),
            Some(Compatibility::Runs)
        );
        assert_eq!(
            block_on(kept.verdict_at(
                &http,
                &PlayportGame::Steam { app_id: 204_360 },
                start + Duration::from_secs(60)
            )),
            Some(Compatibility::Broken)
        );
        assert_eq!(http.requests.load(Ordering::SeqCst), 1);

        block_on(kept.verdict_at(&http, &HOLLOW_KNIGHT, start + LIST_KEPT_FOR));
        assert_eq!(http.requests.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_list_that_cannot_be_read_costs_the_verdict_and_is_asked_for_again_later() {
        let http = CountingHttp::new(Err("offline".into()));
        let kept = CompatibilityList::new();
        let start = Instant::now();
        assert_eq!(
            block_on(kept.verdict_at(&http, &HOLLOW_KNIGHT, start)),
            None
        );
        // Not on every tap...
        assert_eq!(
            block_on(kept.verdict_at(&http, &HOLLOW_KNIGHT, start + Duration::from_secs(10))),
            None
        );
        assert_eq!(http.requests.load(Ordering::SeqCst), 1);
        // ...but once the wait is over.
        block_on(kept.verdict_at(&http, &HOLLOW_KNIGHT, start + RETRY_AFTER));
        assert_eq!(http.requests.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn an_unreadable_answer_is_no_list() {
        let http = CountingHttp::new(Ok(json!({ "games": null })));
        let kept = CompatibilityList::new();
        assert_eq!(block_on(kept.verdict(&http, &HOLLOW_KNIGHT)), None);
    }

    struct RecordingOpener(Mutex<Vec<String>>);

    impl UrlOpener for RecordingOpener {
        fn open(&self, url: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(url.to_string());
            Ok(())
        }
    }

    #[test]
    fn opens_playport_with_a_url_that_carries_nothing() {
        let opener = RecordingOpener(Mutex::new(Vec::new()));
        assert_eq!(open(true, &opener), Ok(()));
        assert_eq!(*opener.0.lock().unwrap(), vec!["playport://".to_string()]);
    }

    #[test]
    fn says_so_when_playport_has_gone_since_the_menu_was_offered() {
        let opener = RecordingOpener(Mutex::new(Vec::new()));
        assert_eq!(
            open(false, &opener),
            Err("Playport is no longer installed on this device.".to_string())
        );
        assert!(opener.0.lock().unwrap().is_empty());
    }
}
