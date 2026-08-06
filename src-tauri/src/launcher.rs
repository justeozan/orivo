use crate::catalog::{Game, LaunchTarget};
use std::{
    io,
    path::{Path, PathBuf},
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
    /// The store this game was synced from has no client on this machine, or
    /// no client at all on this platform. The message names the store so the
    /// user is not left guessing which connection is at fault.
    ProviderClientMissing {
        label: &'static str,
    },
    /// The store sells entitlements Orivo cannot start: an Xbox console title,
    /// or an Instant Gaming key that lives on whichever store redeemed it.
    ProviderNotLaunchable {
        label: &'static str,
    },
    ProviderDispatchFailed {
        label: &'static str,
        status: ExitStatus,
    },
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
            Self::ProviderClientMissing { label } => write!(
                f,
                "Install the {label} app on this machine to start this game. Orivo keeps it in your library either way."
            ),
            Self::ProviderNotLaunchable { label } => write!(
                f,
                "{label} games cannot be started from Orivo. This entry records what you own."
            ),
            Self::ProviderDispatchFailed { label, status } => {
                write!(f, "{label} could not accept the launch request ({status})")
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
        LaunchTarget::Provider { provider, app_ref } => launch_provider(provider, app_ref),
    }
}

/// Every connected store Orivo can hand a launch to, and how. The mapping is
/// closed and lives here: a provider token that is not in this table simply has
/// no way to reach a process, whatever a sync wrote into the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderClient {
    pub label: &'static str,
    /// The client's URI scheme template. `{ref}` is replaced by the launch
    /// reference after percent-encoding.
    uri_template: &'static str,
    /// Absolute locations that prove the client is installed. Detection is a
    /// plain existence check: Orivo never asks a store where it lives.
    macos_bundles: &'static [&'static str],
    windows_relative_paths: &'static [&'static str],
    /// Set for the Microsoft Store, whose "client" is Windows itself.
    windows_builtin: bool,
}

pub fn provider_client(provider: &str) -> Option<ProviderClient> {
    match provider {
        "epic" => Some(ProviderClient {
            label: "Epic Games",
            uri_template: "com.epicgames.launcher://apps/{ref}?action=launch&silent=true",
            macos_bundles: &["/Applications/Epic Games Launcher.app"],
            windows_relative_paths: &["Epic Games/Launcher/Portal/Binaries/Win32"],
            windows_builtin: false,
        }),
        "gog" => Some(ProviderClient {
            label: "GOG",
            // Galaxy has no documented direct-launch URI, so this opens the
            // game in Galaxy, which is where Play and Install live.
            uri_template: "goggalaxy://openGameView/{ref}",
            macos_bundles: &["/Applications/GOG Galaxy.app"],
            windows_relative_paths: &["GOG Galaxy"],
            windows_builtin: false,
        }),
        "ubisoft" => Some(ProviderClient {
            label: "Ubisoft Connect",
            uri_template: "uplay://launch/{ref}/0",
            // Ubisoft Connect has no macOS build, so this list is empty on
            // purpose: detection then reports the client as missing.
            macos_bundles: &[],
            windows_relative_paths: &["Ubisoft/Ubisoft Game Launcher"],
            windows_builtin: false,
        }),
        "microsoft-store" => Some(ProviderClient {
            label: "Microsoft Store",
            uri_template: "shell:AppsFolder\\{ref}!App",
            macos_bundles: &[],
            windows_relative_paths: &[],
            windows_builtin: true,
        }),
        // Xbox console entitlements and Instant Gaming keys are records of what
        // you own, not something this machine can start.
        _ => None,
    }
}

impl ProviderClient {
    pub fn installed(&self) -> bool {
        if cfg!(target_os = "macos") {
            return self
                .macos_bundles
                .iter()
                .any(|bundle| Path::new(bundle).exists());
        }
        if cfg!(target_os = "windows") {
            if self.windows_builtin {
                return true;
            }
            return self.windows_relative_paths.iter().any(|relative| {
                ["ProgramFiles(x86)", "ProgramFiles", "ProgramW6432"]
                    .iter()
                    .filter_map(std::env::var_os)
                    .any(|root| PathBuf::from(root).join(relative).exists())
            });
        }
        false
    }

    fn uri(&self, app_ref: &str) -> String {
        self.uri_template
            .replace("{ref}", &percent_encode_reference(app_ref))
    }
}

/// Whether the Play button on a connected-store game should be live. This is
/// what keeps Orivo from offering to start something it cannot start.
pub fn provider_launchable(provider: &str) -> bool {
    provider_client(provider).is_some_and(|client| client.installed())
}

fn launch_provider(provider: &str, app_ref: &str) -> Result<(), LaunchError> {
    let client = provider_client(provider).ok_or(LaunchError::ProviderNotLaunchable {
        label: crate::catalog::GameSource::from_provider_token(provider)
            .and_then(|source| match source {
                crate::catalog::GameSource::Xbox => Some("Xbox"),
                crate::catalog::GameSource::InstantGaming => Some("Instant Gaming"),
                _ => None,
            })
            .unwrap_or("This store"),
    })?;
    if !client.installed() {
        return Err(LaunchError::ProviderClientMissing {
            label: client.label,
        });
    }
    dispatch_provider_uri(&client, &client.uri(app_ref))
}

/// Hand the URI to the client through the operating system's own opener, with
/// no shell in between. On macOS the request is pinned to the bundle that was
/// just verified to exist, so a hijacked scheme handler cannot receive it.
#[cfg(target_os = "macos")]
fn dispatch_provider_uri(client: &ProviderClient, uri: &str) -> Result<(), LaunchError> {
    let bundle = client
        .macos_bundles
        .iter()
        .find(|bundle| Path::new(bundle).exists())
        .ok_or(LaunchError::ProviderClientMissing {
            label: client.label,
        })?;
    let status = provider_uri_command(bundle, uri)
        .spawn()
        .map_err(LaunchError::Process)?
        .wait()
        .map_err(LaunchError::Process)?;
    if status.success() {
        Ok(())
    } else {
        Err(LaunchError::ProviderDispatchFailed {
            label: client.label,
            status,
        })
    }
}

#[cfg(target_os = "macos")]
fn provider_uri_command(bundle: &str, uri: &str) -> Command {
    let mut command = Command::new("/usr/bin/open");
    command
        .args(["-a", bundle, uri])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(target_os = "windows")]
fn dispatch_provider_uri(client: &ProviderClient, uri: &str) -> Result<(), LaunchError> {
    // `explorer.exe` resolves a registered scheme without going through a
    // command interpreter, so the reference never reaches a shell parser.
    let status = Command::new("explorer.exe")
        .arg(uri)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(LaunchError::Process)?
        .wait()
        .map_err(LaunchError::Process)?;
    // Explorer reports a non-zero status even on success for shell verbs, so
    // only a spawn failure is treated as a launch failure here.
    let _ = status;
    let _ = client;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn dispatch_provider_uri(client: &ProviderClient, _uri: &str) -> Result<(), LaunchError> {
    Err(LaunchError::ProviderClientMissing {
        label: client.label,
    })
}

/// The launch reference already passed the catalog's opaque grammar, so this
/// only has to encode the few characters a URI gives meaning to — chiefly the
/// colons separating an Epic namespace, catalog item and app name.
fn percent_encode_reference(value: &str) -> String {
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

    #[test]
    fn builds_one_fixed_uri_per_store_from_a_validated_reference() {
        assert_eq!(
            provider_client("epic").unwrap().uri("d5241c:a1b2c3:Sugar"),
            "com.epicgames.launcher://apps/d5241c%3Aa1b2c3%3ASugar?action=launch&silent=true"
        );
        assert_eq!(
            provider_client("gog").unwrap().uri("1207658924"),
            "goggalaxy://openGameView/1207658924"
        );
        assert_eq!(
            provider_client("ubisoft").unwrap().uri("5416"),
            "uplay://launch/5416/0"
        );
        assert_eq!(
            provider_client("microsoft-store")
                .unwrap()
                .uri("Microsoft.MinecraftUWP_8wekyb3d8bbwe"),
            "shell:AppsFolder\\Microsoft.MinecraftUWP_8wekyb3d8bbwe!App"
        );
    }

    #[test]
    fn stores_that_sell_entitlements_never_get_a_launch_path() {
        assert!(provider_client("xbox").is_none());
        assert!(provider_client("instant-gaming").is_none());
        assert!(provider_client("steam").is_none());
        assert!(!provider_launchable("xbox"));
        assert!(!provider_launchable("instant-gaming"));
    }

    #[test]
    fn refuses_an_entitlement_only_store_before_touching_the_operating_system() {
        let game = Game {
            id: "instant-gaming:1234".into(),
            title: "Elden Ring".into(),
            executable_path: None,
            source: GameSource::InstantGaming,
            source_id: Some("1234".into()),
            launch_target: LaunchTarget::Provider {
                provider: "instant-gaming".into(),
                app_ref: "1234".into(),
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
            Err(LaunchError::ProviderNotLaunchable { label }) if label == "Instant Gaming"
        ));
    }

    #[test]
    fn a_reference_can_never_smuggle_a_uri_delimiter_through_the_encoder() {
        assert_eq!(percent_encode_reference("a:b"), "a%3Ab");
        assert_eq!(percent_encode_reference("a b&c"), "a%20b%26c");
        assert_eq!(percent_encode_reference("../etc"), "..%2Fetc");
        assert_eq!(
            percent_encode_reference("Microsoft.MinecraftUWP_8wekyb3d8bbwe"),
            "Microsoft.MinecraftUWP_8wekyb3d8bbwe"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pins_the_verified_store_bundle_rather_than_the_default_url_handler() {
        use std::ffi::OsStr;

        let command = provider_uri_command(
            "/Applications/Epic Games Launcher.app",
            "com.epicgames.launcher://apps/x",
        );
        let arguments = command.get_args().collect::<Vec<_>>();
        assert_eq!(command.get_program(), OsStr::new("/usr/bin/open"));
        assert_eq!(
            arguments,
            vec![
                OsStr::new("-a"),
                OsStr::new("/Applications/Epic Games Launcher.app"),
                OsStr::new("com.epicgames.launcher://apps/x"),
            ]
        );
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
