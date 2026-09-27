//! A minimal, honest `runner-plugin` component: it declares one directory
//! grant, lists `*.rom` files inside it, and prepares a launch intent that
//! echoes back exactly what the host asked for. It is derived from
//! `src-tauri/fixtures/runner-fixture` with every misbehaviour removed —
//! start here, then read that fixture's source if you need to understand what
//! the host refuses and why.
//!
//! Rename `PLUGIN_ID`, `PLUGIN_VERSION` and `LIBRARY_GRANT` to match your own
//! `manifest.json`, and change `ROM_SUFFIX` to whatever your runner's library
//! files use. Everything else — the shape of `discover-page`'s cursor, the
//! closed launch mode — is the contract in `wit/orivo-plugin.wit`, not a
//! choice this template makes for you.

wit_bindgen::generate!({
    world: "runner-plugin",
    path: "../../../../wit",
});

use exports::orivo::plugin::{
    plugin_core::{Guest as CoreGuest, Health, Identity},
    runner::{Guest as RunnerGuest, LaunchIntent, ProfileValidation, RunnerGamePage, RunnerProfile},
};
use orivo::plugin::{
    host_files,
    types::{ExtensionKind, ExternalReference, GameCandidate, PageInfo, PageRequest, PluginError},
};

const PLUGIN_ID: &str = "com.example.my-runner";
const PLUGIN_VERSION: &str = "0.1.0";
/// Must match the directory-grant id your manifest's `files_read` capability
/// is granted for. The host resolves it to a real folder; the component never
/// sees the path itself.
const LIBRARY_GRANT: &str = "library";
const ROM_SUFFIX: &str = ".rom";

struct Starter;

impl CoreGuest for Starter {
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

impl RunnerGuest for Starter {
    fn validate_profile(profile: RunnerProfile) -> Result<ProfileValidation, PluginError> {
        Ok(ProfileValidation {
            valid: !profile.id.is_empty(),
            message: None,
        })
    }

    fn discover_page(
        _profile_id: String,
        request: PageRequest,
    ) -> Result<RunnerGamePage, PluginError> {
        let entries = host_files::list_directory(LIBRARY_GRANT)?;
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
                next_cursor = Some(entry.name);
                break;
            }
            let external_id = entry
                .name
                .strip_suffix(ROM_SUFFIX)
                .unwrap_or(&entry.name)
                .to_string();
            games.push(GameCandidate {
                reference: ExternalReference {
                    provider_id: PLUGIN_ID.into(),
                    external_id: external_id.clone(),
                },
                title: external_id,
                sort_title: None,
                platform: None,
                installed: true,
            });
        }
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
        // Opaque ids only, echoed back exactly as received. The host resolves
        // the actual executable, working directory and arguments from the
        // profile — never from this return value.
        Ok(LaunchIntent {
            runner_id: PLUGIN_ID.into(),
            profile_id,
            game_reference,
            mode: "default".into(),
        })
    }
}

export!(Starter);
