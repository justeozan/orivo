use crate::catalog::{Game, LaunchTarget};
use std::{
    io,
    process::{Command, ExitStatus, Stdio},
};

#[derive(Debug)]
pub enum LaunchError {
    MissingExecutable(std::path::PathBuf),
    PermissionDenied(std::path::PathBuf),
    SteamNotInstalled,
    #[cfg(not(target_os = "macos"))]
    SteamUnsupported,
    SteamDispatchFailed(ExitStatus),
    /// Third-party runner execution is intentionally unavailable until the
    /// plugin host can resolve a validated profile into a typed launch intent.
    /// The built-in Wine-Staging adapter is resolved earlier by the trusted
    /// runner host and never reaches this generic fallback.
    RunnerUnavailable {
        runner_id: String,
    },
    Process(io::Error),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingExecutable(path) => {
                write!(f, "executable does not exist: {}", path.display())
            }
            Self::PermissionDenied(path) => {
                write!(f, "executable is not readable: {}", path.display())
            }
            Self::SteamNotInstalled => {
                write!(
                    f,
                    "This Steam game is no longer installed locally. Refresh Steam and try again."
                )
            }
            #[cfg(not(target_os = "macos"))]
            Self::SteamUnsupported => {
                write!(f, "Steam launching is not supported on this platform yet")
            }
            Self::SteamDispatchFailed(status) => {
                write!(f, "Steam could not accept the launch request ({status})")
            }
            Self::RunnerUnavailable { runner_id } => {
                write!(
                    f,
                    "The {} runner is not available yet. Configure or enable its plugin and try again.",
                    runner_id
                )
            }
            Self::Process(error) => write!(f, "process could not be started: {error}"),
        }
    }
}

impl std::error::Error for LaunchError {}

pub fn launch(game: &Game) -> Result<(), LaunchError> {
    match &game.launch_target {
        LaunchTarget::Direct => {
            let _ = launch_direct(game)?;
            Ok(())
        }
        LaunchTarget::Steam { app_id } => {
            if !game
                .installation_path
                .as_ref()
                .is_some_and(|path| path.is_dir())
            {
                return Err(LaunchError::SteamNotInstalled);
            }
            launch_steam(*app_id)
        }
        LaunchTarget::Runner { runner_id, .. } => {
            // Do not turn runner ids, game references, or profiles into a
            // process invocation here. The forthcoming plugin host will
            // validate a typed LaunchIntent against grants and the selected
            // profile before it asks this service to start anything.
            Err(LaunchError::RunnerUnavailable {
                runner_id: runner_id.clone(),
            })
        }
    }
}

/// Dispatch a fixed Steam install URI for a validated catalog AppID. Unlike a
/// normal launch this intentionally works for a game that is owned but not
/// installed yet; Steam owns its confirmation, disk selection, and download
/// lifecycle.
pub fn install_steam(app_id: u32) -> Result<(), LaunchError> {
    dispatch_steam_uri(steam_install_uri(app_id))
}

fn launch_direct(game: &Game) -> Result<std::process::Child, LaunchError> {
    let executable = game
        .executable_path
        .as_ref()
        .ok_or_else(|| LaunchError::MissingExecutable(std::path::PathBuf::new()))?;
    if !executable.is_file() {
        return Err(LaunchError::MissingExecutable(executable.clone()));
    }
    if !is_executable(executable) {
        return Err(LaunchError::PermissionDenied(executable.clone()));
    }

    let mut command = Command::new(executable);
    command
        .args(&game.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(directory) = game.working_directory.as_ref() {
        command.current_dir(directory);
    }

    command.spawn().map_err(LaunchError::Process)
}

/// Steam is invoked through its registered macOS bundle with a URL assembled
/// from a validated catalog app id. We wait for `/usr/bin/open` to accept the
/// URI, not for the game process itself (`-W` would block on the game). This
/// lets the UI report a missing or unregistered Steam bundle accurately.
#[cfg(target_os = "macos")]
fn launch_steam(app_id: u32) -> Result<(), LaunchError> {
    dispatch_steam_uri(steam_launch_uri(app_id))
}

#[cfg(target_os = "macos")]
fn dispatch_steam_uri(uri: String) -> Result<(), LaunchError> {
    let status = steam_uri_command(&uri)
        .spawn()
        .map_err(LaunchError::Process)?
        .wait()
        .map_err(LaunchError::Process)?;
    if status.success() {
        Ok(())
    } else {
        Err(LaunchError::SteamDispatchFailed(status))
    }
}

#[cfg(target_os = "macos")]
fn steam_uri_command(uri: &str) -> Command {
    let mut command = Command::new("/usr/bin/open");
    command
        .args(["-b", "com.valvesoftware.steam", uri])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(not(target_os = "macos"))]
fn launch_steam(_app_id: u32) -> Result<(), LaunchError> {
    Err(LaunchError::SteamUnsupported)
}

#[cfg(not(target_os = "macos"))]
fn dispatch_steam_uri(_uri: String) -> Result<(), LaunchError> {
    Err(LaunchError::SteamUnsupported)
}

fn steam_launch_uri(app_id: u32) -> String {
    format!("steam://run/{app_id}")
}

fn steam_install_uri(app_id: u32) -> String {
    format!("steam://install/{app_id}")
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.metadata()
        .map(|metadata| !metadata.permissions().readonly())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::GameSource;
    use std::{collections::BTreeMap, path::PathBuf};

    #[test]
    fn reports_missing_executable_before_spawning() {
        let game = Game::from_executable("/definitely/not/a/game").unwrap();

        assert!(matches!(
            launch(&game),
            Err(LaunchError::MissingExecutable(_))
        ));
    }

    #[test]
    fn constructs_a_steam_uri_only_from_a_numeric_app_id() {
        assert_eq!(steam_launch_uri(480), "steam://run/480");
        assert_eq!(steam_install_uri(480), "steam://install/480");
    }

    #[test]
    fn refuses_a_steam_record_when_its_installation_is_gone() {
        let game = Game {
            id: "steam:480".into(),
            title: "Spacewar".into(),
            executable_path: None,
            source: GameSource::Steam,
            source_id: Some("480".into()),
            launch_target: LaunchTarget::Steam { app_id: 480 },
            installation_path: Some(PathBuf::from("/definitely/not/a/Steam/game")),
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
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        };

        assert!(matches!(launch(&game), Err(LaunchError::SteamNotInstalled)));
    }

    #[test]
    fn refuses_runner_targets_until_a_plugin_host_validates_them() {
        let game = Game {
            id: "runner:com.orivo.ryujinx:abc123".into(),
            title: "Example Switch Game".into(),
            executable_path: None,
            source: GameSource::Local,
            source_id: None,
            launch_target: LaunchTarget::Runner {
                runner_id: "com.orivo.ryujinx".into(),
                game_ref: "rom:sha256:abc123".into(),
                profile_id: "profile-7f3b".into(),
            },
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
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        };

        assert!(matches!(
            launch(&game),
            Err(LaunchError::RunnerUnavailable { runner_id }) if runner_id == "com.orivo.ryujinx"
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pins_the_steam_bundle_instead_of_using_the_default_url_handler() {
        use std::ffi::OsStr;

        let command = steam_uri_command(&steam_launch_uri(480));
        let arguments = command.get_args().collect::<Vec<_>>();
        assert_eq!(command.get_program(), OsStr::new("/usr/bin/open"));
        assert_eq!(
            arguments,
            vec![
                OsStr::new("-b"),
                OsStr::new("com.valvesoftware.steam"),
                OsStr::new("steam://run/480"),
            ]
        );
    }
}
