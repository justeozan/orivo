//! Quiet artwork top-up at launch.
//!
//! A library fills up from many places — a store sync, a games folder, a
//! runner import — and some of those cards arrive without a cover, a
//! background or a wordmark. Rather than wait for the player to open each
//! detail page and press "Reset the covers", Orivo looks for what is missing a
//! little while after it opens, in the background.
//!
//! It is deliberately gentle. It only ever fills an empty slot, so art a store
//! published or the player picked is never replaced; it looks at a bounded
//! number of games per launch, a few at a time; and it remembers what it could
//! not find, so a title no source has art for is not searched again on every
//! launch.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    AppState, ArtworkWrite, GAME_ARTWORK_PROGRESS_EVENT, GameArtworkProgressEvent,
    MAIN_WINDOW_LABEL, apply_resolved_artwork,
    catalog::{self, Game},
    game_artwork::{self, ArtworkRole},
    wallpaper_credentials::WallpaperCredentialsService,
};

/// The first paint, the library sync and the games-folder scan all happen in
/// the first seconds; the top-up waits until they have settled so it never
/// competes with them for the network.
const FIRST_PASS_DELAY: Duration = Duration::from_secs(20);
/// One launch looks at no more games than this. A big library is topped up
/// over several launches instead of in one burst.
const MAX_GAMES_PER_LAUNCH: usize = 40;
/// Games searched at once. Fewer than a user-requested fill: nobody is waiting
/// on this one, so it stays out of the way of the game they are about to start.
const CONCURRENT_LOOKUPS: usize = 3;
/// How long a game that came back empty-handed is left alone.
const RETRY_AFTER_SECONDS: u64 = 3 * 24 * 60 * 60;
/// The record of what was already looked for, kept beside the catalog.
const ATTEMPTS_FILE: &str = "artwork-attempts.json";

static ATTEMPTS_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The roles the top-up cares about, in the order they are reported. The
/// landscape card is left out on purpose: every consumer falls back from it to
/// the background or the cover, so it is never what makes a card look empty.
const WATCHED_ROLES: [ArtworkRole; 3] = [
    ArtworkRole::Cover,
    ArtworkRole::Background,
    ArtworkRole::Logo,
];

/// Which of the watched roles a game has nothing to show for.
///
/// "Has" follows what the library actually paints, not just the local slots: a
/// Steam game draws its art from Steam's CDN by app id, a connected store
/// publishes URLs of its own, and a Store card carries asset routes. A game
/// served by any of those is not missing that role, and the top-up must not
/// start downloading a second copy over it.
pub(crate) fn missing_roles(game: &Game) -> Vec<ArtworkRole> {
    let has_cover = game.cover_path.is_some()
        || crate::store_artwork_url(game, crate::STORE_COVER_URL_KEY).is_some()
        || crate::steam_store_asset_url(game, "library_600x900.jpg").is_some()
        || crate::source_asset_url(game, catalog::SOURCE_COVER_URL_KEY).is_some();
    // The background slot is shared with the wallpaper the player may have
    // picked by hand (`home_image_path`), so a filled slot there is an answer.
    let has_background = game.home_image_path.is_some()
        || game.artwork_path.is_some()
        || crate::store_artwork_url(game, crate::STORE_HERO_URL_KEY).is_some()
        || crate::steam_store_asset_url(game, "library_hero.jpg").is_some()
        || crate::source_asset_url(game, catalog::SOURCE_HERO_URL_KEY).is_some();
    let has_logo = game.logo_path.is_some()
        || crate::steam_store_asset_url(game, "logo.png").is_some()
        || crate::source_asset_url(game, catalog::SOURCE_LOGO_URL_KEY).is_some();

    WATCHED_ROLES
        .into_iter()
        .filter(|role| match role {
            ArtworkRole::Cover => !has_cover,
            ArtworkRole::Background => !has_background,
            ArtworkRole::Logo => !has_logo,
            ArtworkRole::Landscape => false,
        })
        .collect()
}

/// One unsuccessful search: when, and for which roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Attempt {
    /// Unix seconds.
    at: u64,
    /// Role tokens, sorted, so two lists of the same roles compare equal.
    missing: Vec<String>,
}

/// What earlier launches already looked for without finding anything.
///
/// A success is never stored: once a game has the art, it stops being a
/// candidate on its own. Only the misses need remembering.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
struct AttemptLog {
    games: BTreeMap<String, Attempt>,
}

fn role_tokens(roles: &[ArtworkRole]) -> Vec<String> {
    let mut tokens = roles
        .iter()
        .map(|role| role.token().to_string())
        .collect::<Vec<_>>();
    tokens.sort();
    tokens
}

impl AttemptLog {
    /// A missing or unreadable record is the same as an empty one: the worst
    /// outcome is one extra search, never a failed launch.
    fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Written to a sibling and renamed, so a crash never leaves half a file.
    fn save(&self, path: &Path) -> io::Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let sequence = ATTEMPTS_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".{ATTEMPTS_FILE}.{}.{sequence}.tmp",
            std::process::id()
        ));
        let result = (|| -> io::Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            serde_json::to_writer_pretty(&mut file, self).map_err(io::Error::other)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    /// Whether this game was already searched for exactly these roles recently
    /// enough that asking again would only repeat the answer. A different set
    /// of missing roles is a new question, so it is not held back.
    fn is_cooling_down(&self, game_id: &str, roles: &[ArtworkRole], now: u64) -> bool {
        self.games.get(game_id).is_some_and(|attempt| {
            attempt.missing == role_tokens(roles)
                // A clock set back would otherwise freeze the entry for as long
                // as the difference; a future timestamp is simply not trusted.
                && attempt.at <= now
                && now - attempt.at < RETRY_AFTER_SECONDS
        })
    }

    fn record_miss(&mut self, game_id: &str, roles: &[ArtworkRole], now: u64) {
        self.games.insert(
            game_id.to_string(),
            Attempt {
                at: now,
                missing: role_tokens(roles),
            },
        );
    }

    fn clear(&mut self, game_id: &str) {
        self.games.remove(game_id);
    }

    /// Forget games that left the library, so the record cannot grow forever.
    fn retain_known(&mut self, known: &BTreeSet<&str>) {
        self.games.retain(|id, _| known.contains(id.as_str()));
    }
}

/// A game worth searching for, and the roles it is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    game_id: String,
    title: String,
    roles: Vec<ArtworkRole>,
}

/// Choose who gets searched this launch: the games most recently played first,
/// since those are the cards the player is looking at.
///
/// Hidden games are left alone (the player put them away), as are the bundled
/// showcase entries, which are a fixture rather than part of the library.
fn select_candidates(games: &[Game], log: &AttemptLog, now: u64, limit: usize) -> Vec<Candidate> {
    let mut eligible = games
        .iter()
        .filter(|game| !game.hidden && !game.id.starts_with("showcase-"))
        .filter(|game| !game.title.trim().is_empty())
        .filter_map(|game| {
            let roles = missing_roles(game);
            (!roles.is_empty() && !log.is_cooling_down(&game.id, &roles, now))
                .then_some((game, roles))
        })
        .collect::<Vec<_>>();
    // Timestamps are RFC 3339 in UTC, which sort the same as the instants they
    // name. A game never played (`None`) sorts below any that was.
    eligible.sort_by(|(left, _), (right, _)| right.last_played_at.cmp(&left.last_played_at));
    eligible
        .into_iter()
        .take(limit)
        .map(|(game, roles)| Candidate {
            game_id: game.id.clone(),
            title: game.title.clone(),
            roles,
        })
        .collect()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

enum Outcome {
    /// Some roles were written; these are the ones that are still empty.
    Applied { still_missing: Vec<ArtworkRole> },
    /// The search ran and came back with nothing usable.
    Nothing,
    /// The network went away, or an earlier game already showed it had.
    /// Nothing is recorded: the game was not really searched for.
    Offline,
}

/// Wait out the start-up, then fill what is missing. Runs once per launch.
pub(crate) async fn run(app: AppHandle) {
    tokio::time::sleep(FIRST_PASS_DELAY).await;
    let (Some(state), Some(credentials)) = (
        app.try_state::<AppState>(),
        app.try_state::<Arc<WallpaperCredentialsService>>(),
    ) else {
        return;
    };
    let attempts_path = state.catalog_path.with_file_name(ATTEMPTS_FILE);
    let mut log = AttemptLog::load(&attempts_path);

    let candidates = {
        let Ok(catalog) = state.catalog.read() else {
            return;
        };
        let known = catalog
            .games
            .iter()
            .map(|game| game.id.as_str())
            .collect::<BTreeSet<_>>();
        log.retain_known(&known);
        select_candidates(&catalog.games, &log, unix_now(), MAX_GAMES_PER_LAUNCH)
    };
    if candidates.is_empty() {
        return;
    }
    // Offline is the common reason for a pass to find nothing, and it says
    // nothing about the titles. Ask once before touching any record.
    if !game_artwork::is_reachable().await {
        return;
    }

    let emit_progress = |event: GameArtworkProgressEvent| {
        let _ = app.emit_to(MAIN_WINDOW_LABEL, GAME_ARTWORK_PROGRESS_EVENT, event);
    };
    let mut pending = candidates
        .iter()
        .map(|candidate| candidate.game_id.clone())
        .collect::<Vec<_>>();
    emit_progress(GameArtworkProgressEvent {
        pending: pending.clone(),
        game_id: None,
        found: false,
    });

    let offline = AtomicBool::new(false);
    let offline_ref = &offline;
    let app_ref = &app;
    let state_ref = &*state;
    let credentials_ref = &**credentials;
    let mut results = futures_util::stream::iter(candidates.into_iter().map(
        |candidate| async move {
            // Games still queued when the network drops are not searched.
            if offline_ref.load(Ordering::Acquire) {
                return (candidate, Outcome::Offline);
            }
            let outcome = match apply_resolved_artwork(
                app_ref,
                state_ref,
                credentials_ref,
                &candidate.game_id,
                candidate.title.clone(),
                ArtworkWrite::GapsOnly(candidate.roles.clone()),
            )
            .await
            {
                Ok(response) => Outcome::Applied {
                    still_missing: candidate
                        .roles
                        .iter()
                        .copied()
                        .filter(|role| !response.replaced.iter().any(|done| done == role.token()))
                        .collect(),
                },
                // An empty answer is also what a dropped connection looks like,
                // so a miss only counts against the game if the network is up.
                Err(_) if game_artwork::is_reachable().await => Outcome::Nothing,
                Err(_) => {
                    offline_ref.store(true, Ordering::Release);
                    Outcome::Offline
                }
            };
            (candidate, outcome)
        },
    ))
    .buffer_unordered(CONCURRENT_LOOKUPS);

    while let Some((candidate, outcome)) = results.next().await {
        let now = unix_now();
        let found = match outcome {
            Outcome::Applied { still_missing } if still_missing.is_empty() => {
                log.clear(&candidate.game_id);
                true
            }
            // Part of it landed. What is still empty is remembered like any
            // other miss, so the rest is not searched for again straight away.
            Outcome::Applied { still_missing } => {
                log.record_miss(&candidate.game_id, &still_missing, now);
                true
            }
            Outcome::Nothing => {
                log.record_miss(&candidate.game_id, &candidate.roles, now);
                false
            }
            Outcome::Offline => false,
        };
        let _ = log.save(&attempts_path);
        pending.retain(|id| id != &candidate.game_id);
        emit_progress(GameArtworkProgressEvent {
            pending: pending.clone(),
            game_id: Some(candidate.game_id),
            found,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn game(value: serde_json::Value) -> Game {
        let mut base = json!({"id": "g", "title": "Game"});
        base.as_object_mut()
            .unwrap()
            .extend(value.as_object().unwrap().clone());
        serde_json::from_value(base).expect("a minimal game record")
    }

    fn tokens(game: &Game) -> Vec<&'static str> {
        missing_roles(game)
            .into_iter()
            .map(ArtworkRole::token)
            .collect()
    }

    #[test]
    fn a_bare_game_misses_cover_background_and_logo() {
        assert_eq!(tokens(&game(json!({}))), ["cover", "background", "logo"]);
    }

    #[test]
    fn filled_slots_are_not_missing() {
        let full = game(json!({
            "cover_path": "/c.jpg",
            "home_image_path": "/h.jpg",
            "logo_path": "/l.png",
        }));
        assert!(tokens(&full).is_empty());
        // Only the logo is absent: the common case this pass exists for.
        let no_logo = game(json!({"cover_path": "/c.jpg", "artwork_path": "/a.jpg"}));
        assert_eq!(tokens(&no_logo), ["logo"]);
    }

    #[test]
    fn store_published_urls_count_as_having_the_art() {
        let synced = game(json!({
            "orivo_source_cover_url": "https://cdn.example/c.jpg",
            "orivo_source_hero_url": "https://cdn.example/h.jpg",
            "orivo_source_logo_url": "https://cdn.example/l.png",
        }));
        assert!(tokens(&synced).is_empty());
        // An insecure URL is not served by the library, so it is not "having".
        let insecure = game(json!({"orivo_source_cover_url": "http://cdn.example/c.jpg"}));
        assert!(tokens(&insecure).contains(&"cover"));
        let store_card =
            game(json!({"storeCoverUrl": "/assets/c.jpg", "storeHeroUrl": "/assets/h.jpg"}));
        assert_eq!(tokens(&store_card), ["logo"]);
    }

    #[test]
    fn a_steam_game_is_served_entirely_by_steam() {
        let mut steam = game(json!({}));
        steam.launch_target = catalog::LaunchTarget::Steam { app_id: 620 };
        assert!(tokens(&steam).is_empty(), "{:?}", tokens(&steam));
    }

    #[test]
    fn a_top_up_writes_only_the_roles_still_empty() {
        let no_logo = game(json!({"cover_path": "/c.jpg", "artwork_path": "/a.jpg"}));
        let write = ArtworkWrite::GapsOnly(vec![ArtworkRole::Logo, ArtworkRole::Cover]);
        // The logo is empty and was asked for.
        assert!(write.allows(ArtworkRole::Logo, &no_logo));
        // The cover was asked for but has been filled since: left alone.
        assert!(!write.allows(ArtworkRole::Cover, &no_logo));
        // The background is empty in no sense here, and was never asked for.
        assert!(!write.allows(ArtworkRole::Background, &game(json!({}))));
        // A reset still writes everything.
        assert!(ArtworkWrite::Everything.allows(ArtworkRole::Cover, &no_logo));
        assert_eq!(ArtworkWrite::Everything.roles().len(), 4);
    }

    fn played(id: &str, last_played_at: Option<&str>) -> Game {
        let mut value = json!({"id": id, "title": id});
        if let Some(at) = last_played_at {
            value["last_played_at"] = json!(at);
        }
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn candidates_are_the_most_recently_played_and_bounded() {
        let games = vec![
            played("never", None),
            played("old", Some("2026-01-01T00:00:00Z")),
            played("new", Some("2026-09-01T00:00:00Z")),
            played("mid", Some("2026-05-01T00:00:00Z")),
        ];
        let ids = |limit| {
            select_candidates(&games, &AttemptLog::default(), 1_000, limit)
                .into_iter()
                .map(|candidate| candidate.game_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(10), ["new", "mid", "old", "never"]);
        assert_eq!(ids(2), ["new", "mid"]);
    }

    #[test]
    fn hidden_showcase_and_complete_games_are_not_candidates() {
        let mut hidden = played("hidden", None);
        hidden.hidden = true;
        let showcase = played("showcase-elden-ring", None);
        let mut complete = played("complete", None);
        complete.cover_path = Some("/c.jpg".into());
        complete.artwork_path = Some("/a.jpg".into());
        complete.logo_path = Some("/l.png".into());
        let untitled = game(json!({"id": "untitled", "title": "  "}));
        let wanted = played("wanted", None);
        let picked = select_candidates(
            &[hidden, showcase, complete, untitled, wanted],
            &AttemptLog::default(),
            0,
            40,
        );
        assert_eq!(
            picked
                .iter()
                .map(|candidate| candidate.game_id.as_str())
                .collect::<Vec<_>>(),
            ["wanted"]
        );
        assert_eq!(picked[0].roles, WATCHED_ROLES);
    }

    #[test]
    fn a_recent_miss_for_the_same_roles_is_skipped_until_it_expires() {
        let games = vec![played("a", None)];
        let roles = missing_roles(&games[0]);
        let mut log = AttemptLog::default();
        log.record_miss("a", &roles, 1_000);

        let count = |now| select_candidates(&games, &log, now, 40).len();
        assert_eq!(count(1_000 + RETRY_AFTER_SECONDS - 1), 0);
        assert_eq!(count(1_000 + RETRY_AFTER_SECONDS), 1);
        // A clock that moved backwards does not freeze the entry.
        assert_eq!(count(10), 1);
    }

    #[test]
    fn a_miss_for_other_roles_does_not_hold_the_game_back() {
        let games = vec![played("a", None)];
        let mut log = AttemptLog::default();
        // Earlier only the logo was missing; now cover and background are too.
        log.record_miss("a", &[ArtworkRole::Logo], 1_000);
        assert_eq!(select_candidates(&games, &log, 1_001, 40).len(), 1);
        // Role order does not matter to the comparison.
        let mut reordered = AttemptLog::default();
        reordered.record_miss(
            "a",
            &[
                ArtworkRole::Logo,
                ArtworkRole::Cover,
                ArtworkRole::Background,
            ],
            1_000,
        );
        assert!(select_candidates(&games, &reordered, 1_001, 40).is_empty());
    }

    #[test]
    fn a_success_clears_the_entry_and_departed_games_are_forgotten() {
        let mut log = AttemptLog::default();
        log.record_miss("kept", &[ArtworkRole::Logo], 5);
        log.record_miss("done", &[ArtworkRole::Logo], 5);
        log.record_miss("gone", &[ArtworkRole::Logo], 5);
        log.clear("done");
        log.retain_known(&BTreeSet::from(["kept", "done"]));
        assert_eq!(log.games.keys().collect::<Vec<_>>(), ["kept"]);
    }

    #[test]
    fn the_record_round_trips_through_disk() {
        let directory = std::env::temp_dir().join(format!(
            "orivo-attempts-{}-{}",
            std::process::id(),
            unix_now()
        ));
        let path = directory.join(ATTEMPTS_FILE);
        let mut log = AttemptLog::default();
        log.record_miss("a", &[ArtworkRole::Cover, ArtworkRole::Logo], 42);
        log.save(&path).expect("save");
        let loaded = AttemptLog::load(&path);
        assert!(loaded.is_cooling_down("a", &[ArtworkRole::Logo, ArtworkRole::Cover], 43));
        // Garbage on disk is an empty record, not an error.
        fs::write(&path, "not json").unwrap();
        assert!(AttemptLog::load(&path).games.is_empty());
        let _ = fs::remove_dir_all(&directory);
    }
}
