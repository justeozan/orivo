//! The reference runner component: a real third-party plugin, on purpose.
//!
//! Orivo's host is only as trustworthy as the thing it refuses, so this fixture
//! is built to misbehave on request. Every behaviour is selected by the opaque
//! game reference the host passes in, which means one small component covers the
//! nominal path *and* each way a plugin can try to escape its sandbox:
//!
//! | game reference        | what the component does                          |
//! | -------------------- | ------------------------------------------------ |
//! | `fixture:ok`          | returns the launch intent the host asked for       |
//! | `fixture:spin`        | never returns — fuel, deadline and cancellation    |
//! | `fixture:grow`        | allocates until the memory ceiling refuses it      |
//! | `fixture:recurse`     | fills the wasm stack with real call frames         |
//! | `fixture:shadow-stack`| fills Rust's own stack inside linear memory        |
//! | `fixture:trap`        | executes `unreachable`                            |
//! | `fixture:bad-mode`    | returns a launch mode the host does not recognise  |
//! | `fixture:bad-target`  | answers about a different profile and game         |
//! | `fixture:bad-runner`  | claims to be preparing another runner's launch     |
//! | `fixture:chatty`      | floods the host journal after earning a refusal    |
//! | `fixture:shout`       | logs messages far larger than the host will keep   |
//! | `fixture:churn`       | spends the whole call inside host calls            |
//! | `fixture:bury`        | earns a refusal, then churns to evict it           |
//! | `fixture:bad-id`      | returns a reference that is really a path          |
//! | `fixture:deny`        | reads a directory grant it was never given         |
//! | `fixture:escape`      | reads `../` out of the folder it *was* given       |
//! | `fixture:read-NAME`   | reads `NAME.rom` by name, whatever the host put there |
//! | `fixture:fail`        | returns a plain WIT error                          |
//!
//! `discover-page` takes its selector from the profile id instead, because what
//! it is asked to misbehave about is the *page* it hands back: `fixture:dup`,
//! `fixture:overfill`, `fixture:huge`, `fixture:bad-cursor`,
//! `fixture:loop-cursor` and `fixture:done-cursor`.
//!
//! It reads nothing but the one directory grant named `fixture-games`, and it
//! has no other import: no clock, no random, no network, no WASI.

wit_bindgen::generate!({
    world: "runner-plugin",
    path: "../../../wit",
});

use exports::orivo::plugin::{
    plugin_core::{Guest as CoreGuest, Health, Identity},
    runner::{Guest as RunnerGuest, LaunchIntent, ProfileValidation, RunnerGamePage, RunnerProfile},
};
use orivo::plugin::{
    host_files,
    host_journal::{self, JournalLevel},
    types::{ExtensionKind, ExternalReference, GameCandidate, PageInfo, PageRequest, PluginError,
        PluginErrorCode},
};

const PLUGIN_ID: &str = "com.orivo.fixture-runner";
const PLUGIN_VERSION: &str = "1.0.0";
/// The single directory grant this component is written against. Anything else
/// is the host's business to refuse.
const GAMES_GRANT: &str = "fixture-games";
const ROM_SUFFIX: &str = ".rom";
/// Reads one named entry instead of whatever the listing offered. It is how a
/// host test points the component at something it planted — a symbolic link, a
/// FIFO, a file that outgrew its own metadata.
const READ_PREFIX: &str = "fixture:read-";

struct Fixture;

impl CoreGuest for Fixture {
    fn get_identity() -> Identity {
        Identity {
            id: PLUGIN_ID.into(),
            version: PLUGIN_VERSION.into(),
            extensions: vec![ExtensionKind::Runner],
        }
    }

    fn health_check() -> Result<Health, PluginError> {
        Ok(Health {
            ready: true,
            message: None,
        })
    }
}

impl RunnerGuest for Fixture {
    fn validate_profile(profile: RunnerProfile) -> Result<ProfileValidation, PluginError> {
        misbehave(&profile.id)?;
        Ok(ProfileValidation {
            valid: profile.id.starts_with("fixture"),
            message: (!profile.id.starts_with("fixture"))
                .then(|| "This profile was not created by the fixture runner.".to_string()),
        })
    }

    fn discover_page(
        profile_id: String,
        request: PageRequest,
    ) -> Result<RunnerGamePage, PluginError> {
        misbehave(&profile_id)?;
        let entries = host_files::list_directory(GAMES_GRANT)?;
        // The cursor is the name the last page stopped after. Nothing else is
        // remembered between calls, so resuming is a comparison, not state.
        let after = request.cursor.unwrap_or_default();
        let limit = request.limit.max(1) as usize;
        let mut games = Vec::new();
        let mut next_cursor = None;
        for entry in entries {
            if entry.directory || !entry.name.ends_with(ROM_SUFFIX) {
                continue;
            }
            if !after.is_empty() && entry.name.as_str() <= after.as_str() {
                continue;
            }
            if games.len() == limit {
                next_cursor = Some(after_cursor(&games));
                break;
            }
            let title = host_files::read_file(GAMES_GRANT, &entry.name)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| stem(&entry.name).to_string());
            games.push(GameCandidate {
                reference: ExternalReference {
                    provider_id: PLUGIN_ID.into(),
                    external_id: stem(&entry.name).into(),
                },
                title,
                sort_title: None,
                platform: Some("fixture".into()),
                installed: true,
            });
        }
        host_journal::log(
            JournalLevel::Info,
            &format!("fixture discovered {} games", games.len()),
        );
        Ok(shape_page(&profile_id, games, next_cursor, &after, request.limit))
    }

    fn prepare_launch(
        profile_id: String,
        game_reference: String,
    ) -> Result<LaunchIntent, PluginError> {
        misbehave(&game_reference)?;
        // The controlled intent: opaque ids only, echoed back exactly as the
        // host passed them, and the one launch mode the contract declares.
        let (runner_id, profile_id, game_reference, mode) = match game_reference.as_str() {
            "fixture:bad-target" => (
                PLUGIN_ID,
                "some-other-profile".into(),
                "fixture:ok".into(),
                "default",
            ),
            "fixture:bad-id" => (PLUGIN_ID, profile_id, "../../etc/passwd".into(), "default"),
            "fixture:bad-mode" => (PLUGIN_ID, profile_id, game_reference, "shell"),
            // Names a runner it is not. The host owns which plugin it called.
            "fixture:bad-runner" => (
                "com.orivo.some-other-runner",
                profile_id,
                game_reference,
                "default",
            ),
            _ => (PLUGIN_ID, profile_id, game_reference, "default"),
        };
        Ok(LaunchIntent {
            runner_id: runner_id.into(),
            profile_id,
            game_reference,
            mode: mode.into(),
        })
    }
}

/// The misbehaviours that are not about the returned value. Each one exists so a
/// host test can prove a ceiling actually holds.
fn misbehave(selector: &str) -> Result<(), PluginError> {
    match selector {
        "fixture:spin" => {
            host_journal::log(JournalLevel::Warning, "fixture is about to spin");
            let mut counter: u64 = 0;
            loop {
                counter = counter.wrapping_add(1);
                // Without an observable effect LLVM is free to delete the loop
                // body; the host needs a real backedge to interrupt.
                if counter == u64::MAX {
                    host_journal::log(JournalLevel::Debug, "unreachable");
                }
            }
        }
        // Deep, non-tail recursion whose frames are wasm locals and nothing
        // else. Nothing here has its address taken, so the descent never touches
        // the shadow stack Rust keeps in linear memory: every frame lands on the
        // native stack, which is the one `max_wasm_stack` bounds. That is the
        // ceiling this selector exists to reach.
        "fixture:recurse" => {
            // Work *after* the call, so this cannot become a tail call, and the
            // carried value keeps the frame alive across it. Without both, LLVM
            // turns the descent into a loop and nothing is ever stacked.
            fn descend(depth: u64, carried: u64) -> u64 {
                if depth == 0 {
                    return carried;
                }
                let deeper = descend(depth - 1, carried.wrapping_add(depth));
                deeper.wrapping_mul(3).wrapping_add(depth)
            }
            host_journal::log(JournalLevel::Warning, "fixture is about to recurse");
            if descend(u64::MAX, 1) == 0 {
                host_journal::log(JournalLevel::Debug, "unreachable");
            }
        }
        // The other stack, and a different failure. Taking the address of a
        // local forces Rust to put the frame in linear memory, where its stack
        // is an ordinary region with an ordinary end — reached long before
        // `max_wasm_stack`, and not by the mechanism `fixture:recurse` tests.
        "fixture:shadow-stack" => {
            fn descend(depth: u32) -> u64 {
                let mut scratch = [0u64; 64];
                scratch[(depth % 64) as usize] = u64::from(depth);
                let deeper = if depth == 0 { 0 } else { descend(depth - 1) };
                core::hint::black_box(scratch)
                    .iter()
                    .fold(deeper, |total, value| total.wrapping_add(*value))
            }
            host_journal::log(JournalLevel::Warning, "fixture is about to fill linear memory");
            if descend(u32::MAX) == u64::MAX {
                host_journal::log(JournalLevel::Debug, "unreachable");
            }
        }
        // The bluntest way a component can stop: a trap of its own choosing.
        // Whatever the host reports for this, it must not be an abort.
        "fixture:trap" => {
            core::arch::wasm32::unreachable();
        }
        "fixture:grow" => {
            let mut blocks: Vec<Vec<u8>> = Vec::new();
            loop {
                let mut block = vec![0u8; 4 * 1024 * 1024];
                block[0] = blocks.len() as u8;
                blocks.push(block);
            }
        }
        // Both of these hand the host's refusal straight back, so a test can
        // assert on a typed `permission-denied` instead of on the absence of
        // something in the result.
        "fixture:deny" => {
            host_files::list_directory("fixture-other")?;
        }
        "fixture:escape" => {
            host_files::read_file(GAMES_GRANT, "../secret.txt")?;
        }
        // Earns a refusal, swallows it, then floods the journal. The host's record
        // of the refusal has to survive the flood.
        "fixture:chatty" => {
            let _ = host_files::list_directory("fixture-other");
            let mut sent = 0u32;
            while sent < 1000 {
                host_journal::log(JournalLevel::Info, "chatter");
                sent += 1;
            }
        }
        // Hands the host far more text than it will keep. The truncation bounds
        // what is *stored*; the copy out of guest memory happened before the
        // host saw a single byte of it, so the cost has to be charged somewhere.
        "fixture:shout" => {
            let shout = "x".repeat(64 * 1024);
            let mut sent = 0u32;
            while sent < 64 {
                host_journal::log(JournalLevel::Info, &shout);
                sent += 1;
            }
        }
        // Spends the whole invocation inside host calls rather than computing.
        // Fuel barely moves; only a clock notices this.
        "fixture:churn" => {
            churn();
        }
        // Earns one refusal the host records, then makes enough ordinary calls
        // to scroll it out of a ring that holds both.
        "fixture:bury" => {
            let _ = host_files::list_directory("fixture-other");
            churn();
        }
        other if other.starts_with(READ_PREFIX) => {
            let entry = format!("{}{ROM_SUFFIX}", &other[READ_PREFIX.len()..]);
            host_files::read_file(GAMES_GRANT, &entry)?;
        }
        "fixture:fail" => {
            return Err(PluginError {
                code: PluginErrorCode::Unavailable,
                message: "The fixture runner was asked to fail.".into(),
                retryable: false,
            });
        }
        _ => {}
    }
    Ok(())
}

/// Ordinary host calls, as many as the host will take. Errors are swallowed on
/// purpose: what is being exercised is the *traffic*, not its results.
fn churn() {
    let mut sent = 0u32;
    while sent < 400 {
        let _ = host_files::list_directory(GAMES_GRANT);
        sent += 1;
    }
}

/// The page misbehaviours. Each one is a shape the host has to refuse *after*
/// the call succeeded, which is the half a permission check cannot cover.
fn shape_page(
    selector: &str,
    mut games: Vec<GameCandidate>,
    next_cursor: Option<String>,
    given_cursor: &str,
    limit: u32,
) -> RunnerGamePage {
    let mut complete = next_cursor.is_none();
    let mut next_cursor = next_cursor;
    match selector {
        // The same game twice. An idempotent import would write it twice.
        "fixture:dup" => {
            if let Some(first) = games.first().cloned() {
                games.push(first);
            }
        }
        // More rows than the host asked for, with distinct ids so it is the
        // length that has to be refused rather than a duplicate.
        "fixture:overfill" => {
            let template = games.first().cloned();
            if let Some(template) = template {
                while games.len() <= limit as usize {
                    let mut extra = template.clone();
                    extra.reference.external_id = format!("overfill-{}", games.len());
                    games.push(extra);
                }
            }
        }
        // A title longer than any view model will take.
        "fixture:huge" => {
            if let Some(first) = games.first_mut() {
                first.title = "T".repeat(8 * 1024);
            }
        }
        // A cursor that is really a path.
        "fixture:bad-cursor" => {
            next_cursor = Some("../../etc/passwd".into());
            complete = false;
        }
        // A cursor identical to the one it was handed. A caller that trusts it
        // asks the same question forever.
        "fixture:loop-cursor" => {
            next_cursor = Some(given_cursor.to_string());
            complete = false;
        }
        // Finished, and yet still offering somewhere to continue from.
        "fixture:done-cursor" => {
            next_cursor = Some("fixture-cursor-1".into());
            complete = true;
        }
        _ => {}
    }
    RunnerGamePage {
        page: PageInfo {
            complete,
            next_cursor,
        },
        games,
    }
}

fn stem(name: &str) -> &str {
    name.strip_suffix(ROM_SUFFIX).unwrap_or(name)
}

fn after_cursor(games: &[GameCandidate]) -> String {
    games
        .last()
        .map(|game| format!("{}{ROM_SUFFIX}", game.reference.external_id))
        .unwrap_or_default()
}

export!(Fixture);
