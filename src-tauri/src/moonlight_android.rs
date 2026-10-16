//! Streaming a PC game to the phone through Moonlight.
//!
//! On a desktop, the Moonlight plugin lists what a streaming host offers by
//! running Moonlight's command-line client, which a phone does not have. What a
//! phone does have is Moonlight's own app, already paired with the player's PC,
//! and its shortcut entry point: a PC by the name Moonlight shows for it, and a
//! game by the name the PC's streaming host lists it under.
//!
//! So on a phone a PC game — a Steam, Epic or other store game, a Windows game —
//! gains one more way to start: "Stream from <PC>". The player names the PC once
//! (Settings › Libraries & Sources); the game is asked for by its title, which
//! is how a host that imports its library lists it. A title the host does not
//! carry gets Moonlight's own answer, on Moonlight's screen.

use crate::catalog::{Game, GameSource, LaunchTarget};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

/// The launch option id a stream is offered under. It names no launch target:
/// the stream belongs to every PC game, and the host resolves it here.
pub const STREAM_OPTION: &str = "moonlight-stream";
/// Offered beside the stream for an Epic game not yet downloaded to the phone,
/// so one menu answers both "play it here" and "play it from my PC".
pub const INSTALL_ON_PHONE_OPTION: &str = "install-on-phone";
const SETTINGS_FILE: &str = "moonlight-android.json";
const MAX_PC_NAME_CHARS: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSettings {
    /// The name Moonlight shows for the player's PC. Empty: streaming is off.
    #[serde(default)]
    pub pc_name: String,
}

#[derive(Default)]
struct Service {
    data_dir: Option<PathBuf>,
    settings: Option<StreamSettings>,
}

fn service() -> &'static Mutex<Service> {
    static SERVICE: OnceLock<Mutex<Service>> = OnceLock::new();
    SERVICE.get_or_init(|| Mutex::new(Service::default()))
}

pub fn configure(data_dir: PathBuf) {
    if let Ok(mut service) = service().lock() {
        service.data_dir = Some(data_dir);
        service.settings = None;
    }
}

/// The settings, read once and then answered from memory: every game in the
/// library asks while the library view is built.
pub fn settings() -> StreamSettings {
    let Ok(mut service) = service().lock() else {
        return StreamSettings::default();
    };
    if let Some(settings) = &service.settings {
        return settings.clone();
    }
    let settings = service
        .data_dir
        .as_ref()
        .and_then(|dir| fs::read(dir.join(SETTINGS_FILE)).ok())
        .and_then(|bytes| serde_json::from_slice::<StreamSettings>(&bytes).ok())
        .unwrap_or_default();
    service.settings = Some(settings.clone());
    settings
}

/// A name Moonlight could show: printable, short, and trimmed.
pub fn normalise_pc_name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.chars().any(char::is_control) {
        return Err("A PC name cannot contain control characters.".into());
    }
    if name.chars().count() > MAX_PC_NAME_CHARS {
        return Err(format!("A PC name is at most {MAX_PC_NAME_CHARS} characters."));
    }
    Ok(name.to_string())
}

pub fn save(pc_name: &str) -> Result<StreamSettings, String> {
    let settings = StreamSettings {
        pc_name: normalise_pc_name(pc_name)?,
    };
    let mut service = service().lock().map_err(|_| "Orivo is busy. Try again.")?;
    let dir = service
        .data_dir
        .clone()
        .ok_or("Orivo's data folder is not ready yet.")?;
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
    let temporary = dir.join(format!("{SETTINGS_FILE}.tmp"));
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(temporary, dir.join(SETTINGS_FILE)).map_err(|error| error.to_string())?;
    service.settings = Some(settings.clone());
    Ok(settings)
}

/// The PC streams come from, when the player named one.
pub fn pc_name() -> Option<String> {
    let name = settings().pc_name;
    (!name.trim().is_empty()).then_some(name)
}

/// Whether a game is one a PC plays: anything but an app of this phone, a game
/// for one of the phone's console emulators, and the bundled showcase.
pub fn streams(game: &Game) -> bool {
    if game.source == GameSource::Android || game.id.starts_with("showcase-") {
        return false;
    }
    !matches!(
        &game.launch_target,
        LaunchTarget::Runner { runner_id, .. } if crate::catalog::is_console_runner_id(runner_id)
    )
}

/// The stream a phone offers for this game, labelled for the menu, or nothing.
pub fn stream_label(game: &Game) -> Option<String> {
    if !cfg!(target_os = "android") || !streams(game) {
        return None;
    }
    pc_name().map(|pc| format!("Stream from {pc}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn game(source: GameSource, target: LaunchTarget) -> Game {
        Game {
            id: "steam:1".into(),
            title: "Nine Sols".into(),
            executable_path: None,
            source,
            source_id: Some("1".into()),
            launch_target: target,
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

    #[test]
    fn streams_pc_games_and_leaves_the_phone_s_own_alone() {
        assert!(streams(&game(GameSource::Steam, LaunchTarget::Steam { app_id: 1 })));
        assert!(streams(&game(
            GameSource::Epic,
            LaunchTarget::Provider {
                provider: "epic".into(),
                app_ref: "a:b:c".into()
            }
        )));
        assert!(!streams(&game(
            GameSource::Android,
            LaunchTarget::Provider {
                provider: "android".into(),
                app_ref: "com.example.game".into()
            }
        )));
        assert!(!streams(&game(
            GameSource::Local,
            LaunchTarget::Runner {
                runner_id: crate::catalog::RETROARCH_RUNNER_ID.into(),
                game_ref: "rom".into(),
                profile_id: "p".into()
            }
        )));
    }

    #[test]
    fn keeps_a_pc_name_moonlight_could_show() {
        assert_eq!(normalise_pc_name("  DESKTOP-OZAN ").unwrap(), "DESKTOP-OZAN");
        assert!(normalise_pc_name("bad\nname").is_err());
        assert!(normalise_pc_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn remembers_the_pc_and_offers_nothing_off_a_phone() {
        let dir = std::env::temp_dir().join(format!("orivo-moonlight-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        configure(dir.clone());
        assert_eq!(pc_name(), None);
        save(" Gaming PC ").unwrap();
        assert_eq!(pc_name().as_deref(), Some("Gaming PC"));
        configure(dir.clone());
        assert_eq!(settings().pc_name, "Gaming PC");
        // Only a phone hands streams to Moonlight's app.
        assert_eq!(
            stream_label(&game(GameSource::Steam, LaunchTarget::Steam { app_id: 1 })).is_some(),
            cfg!(target_os = "android")
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
