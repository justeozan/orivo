//! The reference runner component: a real third-party plugin, on purpose.
//!
//! Orivo's host is only as trustworthy as the thing it refuses, so this fixture
//! is built to misbehave on request. Every behaviour is selected by the opaque
//! game reference the host passes in, which means one small component covers the
//! nominal path *and* each way a plugin can try to escape its sandbox:
//!
//! | game reference        | what the component does                          |
//! | -------------------- | ------------------------------------------------ |
//! | `fixture:ok`         | returns the launch intent the host asked for      |
//! | `fixture:spin`       | never returns — fuel, deadline and cancellation   |
//! | `fixture:grow`       | allocates until the memory ceiling refuses it     |
//! | `fixture:recurse`    | recurses until a stack ceiling refuses it         |
//! | `fixture:bad-mode`   | returns a launch mode the host does not recognise |
//! | `fixture:bad-target` | answers about a different profile and game        |
//! | `fixture:bad-runner` | claims to be preparing another runner's launch     |
//! | `fixture:chatty`     | floods the host journal after earning a refusal    |
//! | `fixture:bad-id`     | returns a reference that is really a path         |
//! | `fixture:deny`       | reads a directory grant it was never given        |
//! | `fixture:escape`     | reads `../` out of the folder it *was* given      |
//! | `fixture:read-NAME`  | reads `NAME.rom` by name, whatever the host put there |
//! | `fixture:fail`       | returns a plain WIT error                         |
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
        Ok(RunnerGamePage {
            page: PageInfo {
                complete: next_cursor.is_none(),
                next_cursor,
            },
            games,
        })
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
        // Deep, non-tail recursion with a real frame. Wasm frames live on the
        // native stack, so this is the one misbehaviour that reaches past the
        // host's own limits and into the thread it was called on.
        "fixture:recurse" => {
            // `black_box` and an observed result keep this a real recursion:
            // with the frame unused and the answer discarded, LLVM deletes the
            // whole descent and the component returns in 2,307 fuel.
            fn descend(depth: u32) -> u64 {
                let mut scratch = [0u64; 64];
                scratch[(depth % 64) as usize] = u64::from(depth);
                let deeper = if depth == 0 { 0 } else { descend(depth - 1) };
                core::hint::black_box(scratch)
                    .iter()
                    .fold(deeper, |total, value| total.wrapping_add(*value))
            }
            host_journal::log(JournalLevel::Warning, "fixture is about to recurse");
            if descend(u32::MAX) == u64::MAX {
                host_journal::log(JournalLevel::Debug, "unreachable");
            }
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
