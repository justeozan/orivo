//! Moonlight from the Mac App Store edition.
//!
//! The App Sandbox will not let Orivo start another app with arguments:
//! `NSWorkspace` drops them, and a process Orivo spawns itself inherits the
//! sandbox, so Moonlight would read its pairing from Orivo's container and
//! find none. What the sandbox does allow is a script the user installed in
//! `~/Library/Application Scripts/<bundle id>/`, which macOS runs *outside* the
//! sandbox through `NSUserUnixTask`. That folder is read-only to the app: the
//! user confirms it once in an open panel (`install`), and that confirmation is
//! the permission. Deleting the script takes it back.
//!
//! The script ([`script_body`]) only ever starts Moonlight, with `list`, `pair`
//! or `stream`, and the arguments Orivo passes are checked here first
//! ([`validate_arguments`]). Moonlight keeps its own pairing and settings, so a
//! player who already streams with it has nothing to redo.
//!
//! Measured on 2026-11-17 against a real host, from a sandboxed build with the
//! edition's entitlements: `list` answers in 1.3 to 1.7 s and a stream starts.
//! The other ways out of the sandbox each cost the player something on every
//! launch (a Shortcuts window coming forward, a Terminal window left open) or
//! need an entitlement exception App Review rarely grants.
//!
//! Only the `app-store` build uses this. The Developer ID build is not
//! sandboxed and starts Moonlight directly (`gamestream.rs`).

use crate::catalog::{Catalog, Game, GameSource, LaunchTarget};
use crate::gamestream::{self, GameStreamFeedError, HostStatusView};
#[cfg(test)]
use crate::gamestream::HostState;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Duration;

/// The script's file name inside the app's Application Scripts folder.
pub const SCRIPT_NAME: &str = "orivo-moonlight";
/// The second line of the script. A script without it, or with an older
/// number, is offered for reinstallation rather than run.
const SCRIPT_MARKER: &str = "# orivo-moonlight-bridge 1";
/// The provider token of a streamed game, and the `GameSource` it maps to.
pub const PROVIDER: &str = "stream";
const ID_PREFIX: &str = "stream-";
/// Printed by the script when no Moonlight is installed.
const MISSING_MARKER: &str = "orivo:no-moonlight";
/// Printed by the script's probe when Moonlight is there.
const PRESENT_MARKER: &str = "orivo:moonlight";
/// The script kills a `list` after 20 s itself; this is the host's own
/// deadline on top of it.
const LIST_TIMEOUT: Duration = Duration::from_secs(25);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_TITLE_CHARS: usize = 120;
/// Hex doubles the length, and the catalog's provider references stop at 512.
const MAX_APP_NAME_BYTES: usize = 250;

/// The script Orivo writes into its Application Scripts folder.
pub fn script_body() -> String {
    format!(
        r#"#!/bin/sh
{SCRIPT_MARKER}
# Installed by Orivo, the game library. The Mac App Store keeps Orivo in a
# sandbox that cannot start another app with arguments, so macOS runs this
# script for it, outside the sandbox. It only ever starts Moonlight, with one
# of the commands below. Delete this file to take the permission back.
case "$1" in
  --orivo-probe|list|pair|stream) ;;
  *) exit 64 ;;
esac
for bundle in "/Applications/Moonlight.app" "$HOME/Applications/Moonlight.app"; do
  client="$bundle/Contents/MacOS/Moonlight"
  [ -x "$client" ] || continue
  case "$1" in
    --orivo-probe) echo "{PRESENT_MARKER}"; exit 0 ;;
    list) exec /usr/bin/perl -e 'alarm 20; exec @ARGV or exit 127' "$client" "$@" ;;
    pair)
      /usr/bin/pkill -f "Moonlight.app/Contents/MacOS/Moonlight pair " 2>/dev/null
      exec /usr/bin/perl -e 'alarm 180; exec @ARGV or exit 127' "$client" "$@" ;;
    *) exec "$client" "$@" ;;
  esac
done
echo "{MISSING_MARKER}"
exit 69
"#
    )
}

/// Whether an installed script is the one this build would write.
pub fn is_current_script(text: &str) -> bool {
    text.lines().nth(1) == Some(SCRIPT_MARKER)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    /// Not the Mac App Store edition, or no Application Scripts folder.
    Unsupported,
    /// The script is missing or out of date: the setup has to install it.
    NotInstalled,
    /// The player closed the panel.
    Cancelled,
    /// The player chose another folder than the one macOS gives Orivo.
    WrongFolder,
    /// The script ran and found no Moonlight.
    MoonlightMissing,
    /// An argument Orivo refuses to pass on.
    InvalidArgument,
    /// The script did not finish inside the deadline.
    TimedOut,
    /// The script ran and failed.
    Failed(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(formatter, "Streaming through Moonlight is not available in this edition."),
            Self::NotInstalled => write!(formatter, "Allow Orivo to start Moonlight first."),
            Self::Cancelled => write!(formatter, "Nothing was installed."),
            Self::WrongFolder => write!(formatter, "Choose the folder Orivo opened, then Open."),
            Self::MoonlightMissing => write!(formatter, "Install Moonlight first."),
            Self::InvalidArgument => write!(formatter, "Orivo refused to pass that to Moonlight."),
            Self::TimedOut => write!(formatter, "Moonlight did not answer in time."),
            Self::Failed(message) => write!(formatter, "Moonlight could not be started: {message}"),
        }
    }
}

/// The only command lines the bridge passes on. Everything else is refused
/// before the script is asked, and the script refuses it again.
pub fn validate_arguments(arguments: &[String]) -> Result<(), BridgeError> {
    let host_ok = |host: &String| gamestream::normalise_host(host).is_ok_and(|valid| &valid == host);
    let ok = match arguments {
        [verb] => verb == "--orivo-probe",
        [verb, host] => verb == "list" && host_ok(host),
        [verb, host, flag, pin] => {
            verb == "pair"
                && host_ok(host)
                && flag == "--pin"
                && pin.len() == 4
                && pin.bytes().all(|byte| byte.is_ascii_digit())
        }
        [verb, host, app] => verb == "stream" && host_ok(host) && valid_app_name(app),
        _ => false,
    };
    if ok { Ok(()) } else { Err(BridgeError::InvalidArgument) }
}

/// An application name as the host listed it, safe to pass as one argument.
fn valid_app_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.len() <= MAX_APP_NAME_BYTES
        && !name.starts_with('-')
        && !name.chars().any(char::is_control)
}

/// What Settings is told about the bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoonlightBridgeView {
    /// This build reaches Moonlight through the script.
    pub available: bool,
    /// The current script is installed.
    pub installed: bool,
    /// Whether the script found Moonlight. Unknown until it is installed.
    pub moonlight: Option<bool>,
}

// ---------------------------------------------------------------------------
// Stream games in the catalog
// ---------------------------------------------------------------------------

/// The provider reference of an application name: hex, because the catalog's
/// reference grammar has no room for the spaces game names are made of.
pub fn app_ref(name: &str) -> String {
    let mut encoded = String::from("x:");
    for byte in name.as_bytes() {
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}

/// The application name back from [`app_ref`].
pub fn app_name(app_ref: &str) -> Option<String> {
    let hex = app_ref.strip_prefix("x:")?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok().filter(|name| valid_app_name(name))
}

fn game_id(name: &str) -> String {
    let digest = Sha256::digest(name.as_bytes());
    format!("{ID_PREFIX}{}", &format!("{digest:x}")[..24])
}

/// What the host streams that is not a game of its own.
fn not_a_game(name: &str) -> bool {
    matches!(name.trim().to_ascii_lowercase().as_str(), "desktop" | "steam big picture")
}

/// One library card per application the PC lists.
pub fn games_from(names: &[String]) -> Vec<Game> {
    let mut games = BTreeMap::new();
    for name in names {
        if !valid_app_name(name) || not_a_game(name) {
            continue;
        }
        let title: String = name.chars().take(MAX_TITLE_CHARS).collect();
        let reference = app_ref(name);
        let game = Game {
            id: game_id(name),
            title: title.trim().to_string(),
            executable_path: None,
            source: GameSource::Stream,
            source_id: Some(reference.clone()),
            launch_target: LaunchTarget::Provider {
                provider: PROVIDER.to_string(),
                app_ref: reference,
            },
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
        };
        if game.validate().is_ok() {
            games.insert(game.id.clone(), game);
        }
    }
    games.into_values().collect()
}

/// Bring the catalog's stream cards in line with what the PC lists. Returns
/// whether anything changed. What the player made of a card (artwork, play
/// time, hidden) survives a refresh.
pub fn apply(catalog: &mut Catalog, games: Vec<Game>) -> bool {
    let mut incoming: BTreeMap<String, Game> =
        games.into_iter().map(|game| (game.id.clone(), game)).collect();
    let mut changed = false;
    catalog.games.retain_mut(|existing| {
        if existing.source != GameSource::Stream {
            return true;
        }
        let Some(fresh) = incoming.remove(&existing.id) else {
            changed = true;
            return false;
        };
        if existing.title != fresh.title || existing.launch_target != fresh.launch_target {
            existing.title = fresh.title;
            existing.source_id = fresh.source_id;
            existing.launch_target = fresh.launch_target;
            changed = true;
        }
        true
    });
    for (_, game) in incoming {
        catalog.games.push(game);
        changed = true;
    }
    changed
}

/// The state of the PC from its reachability and the bridge's `list`.
pub fn host_status(host: &str, reachable: bool, listing: &Result<Vec<String>, BridgeError>) -> HostStatusView {
    let as_feed = match listing {
        Ok(names) => Ok(names.clone()),
        Err(BridgeError::TimedOut) => Err(GameStreamFeedError::Unreachable(host.to_owned())),
        Err(BridgeError::MoonlightMissing | BridgeError::NotInstalled | BridgeError::Unsupported) => {
            Err(GameStreamFeedError::ClientUnavailable)
        }
        Err(_) => Err(GameStreamFeedError::NotPaired(host.to_owned())),
    };
    HostStatusView {
        state: gamestream::host_state(reachable, &as_feed),
        host: host.to_owned(),
        games: listing
            .as_ref()
            .map(|names| names.iter().filter(|name| !not_a_game(name)).count())
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Running the script
// ---------------------------------------------------------------------------

/// Whether this build goes through the script at all.
pub const AVAILABLE: bool = cfg!(all(target_os = "macos", feature = "app-store"));

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use block2::RcBlock;
    use objc2::AnyThread;
    use objc2::rc::Retained;
    use objc2_foundation::{
        NSArray, NSError, NSFileHandle, NSFileManager, NSSearchPathDirectory,
        NSSearchPathDomainMask, NSString, NSURL, NSUserUnixTask,
    };
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::thread;

    /// `~/Library/Application Scripts/<bundle id>`, as macOS names it for
    /// this app. It exists for every sandboxed app; the app may read it but
    /// not write it.
    pub fn scripts_dir() -> Option<PathBuf> {
        let manager = NSFileManager::defaultManager();
        let url = manager
            .URLForDirectory_inDomain_appropriateForURL_create_error(
                NSSearchPathDirectory::ApplicationScriptsDirectory,
                NSSearchPathDomainMask::UserDomainMask,
                None,
                false,
            )
            .ok()?;
        url.to_file_path()
    }

    pub fn script_path() -> Option<PathBuf> {
        scripts_dir().map(|dir| dir.join(SCRIPT_NAME))
    }

    pub fn installed() -> bool {
        script_path()
            .and_then(|path| fs::read_to_string(path).ok())
            .is_some_and(|text| is_current_script(&text))
    }

    /// Show the folder in an open panel and, once the player confirms it,
    /// write the script. Must run on the main thread.
    pub fn install() -> Result<(), BridgeError> {
        let dir = scripts_dir().ok_or(BridgeError::Unsupported)?;
        let picked = rfd::FileDialog::new()
            .set_title("Click Open to let Orivo start Moonlight")
            .set_directory(&dir)
            .pick_folder()
            .ok_or(BridgeError::Cancelled)?;
        let same = |a: &PathBuf, b: &PathBuf| match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => a == b,
        };
        if !same(&picked, &dir) {
            return Err(BridgeError::WrongFolder);
        }
        let path = dir.join(SCRIPT_NAME);
        fs::write(&path, script_body()).map_err(|error| BridgeError::Failed(error.to_string()))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .map_err(|error| BridgeError::Failed(error.to_string()))?;
        Ok(())
    }

    /// The task, its output handle and its completion block, kept alive until
    /// the script ends. Only ever released on another thread, which Foundation
    /// objects allow.
    struct Running {
        _task: Retained<NSUserUnixTask>,
        _output: Retained<NSFileHandle>,
        _block: RcBlock<dyn Fn(*mut NSError)>,
    }
    // SAFETY: the fields are never used after the move, only released, and
    // releasing an NSObject is thread-safe.
    unsafe impl Send for Running {}

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    /// Run the script with `arguments`. With a deadline, wait for it and
    /// return what it printed; without one, return as soon as it started
    /// (a stream runs for as long as the player plays).
    pub fn run(arguments: &[String], deadline: Option<Duration>) -> Result<String, BridgeError> {
        validate_arguments(arguments)?;
        let path = script_path().ok_or(BridgeError::Unsupported)?;
        if !installed() {
            return Err(BridgeError::NotInstalled);
        }
        let output_path = std::env::temp_dir().join(format!(
            "orivo-moonlight-{}-{}.out",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&output_path, b"").map_err(|error| BridgeError::Failed(error.to_string()))?;
        let output_text = output_path.to_string_lossy().into_owned();

        let url = NSURL::from_file_path(&path).ok_or(BridgeError::NotInstalled)?;
        let task = NSUserUnixTask::initWithURL_error(NSUserUnixTask::alloc(), &url)
            .map_err(|_| BridgeError::NotInstalled)?;
        let output = NSFileHandle::fileHandleForWritingAtPath(&NSString::from_str(&output_text))
            .ok_or_else(|| BridgeError::Failed("no output file".into()))?;
        task.setStandardOutput(Some(&output));
        let words: Vec<Retained<NSString>> =
            arguments.iter().map(|argument| NSString::from_str(argument)).collect();
        let words = NSArray::from_retained_slice(&words);

        let (sender, receiver) = mpsc::channel::<Option<String>>();
        let block = RcBlock::new(move |error: *mut NSError| {
            // SAFETY: Foundation passes either null or a valid NSError.
            let message = unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string());
            let _ = sender.send(message);
        });
        // SAFETY: the block is valid, and kept alive by `Running` until the
        // completion handler has run.
        unsafe { task.executeWithArguments_completionHandler(Some(&words), RcBlock::as_ptr(&block)) };
        let running = Running {
            _task: task,
            _output: output,
            _block: block,
        };

        let Some(deadline) = deadline else {
            thread::spawn(move || {
                let _ = receiver.recv();
                drop(running);
                let _ = fs::remove_file(output_path);
            });
            return Ok(String::new());
        };
        let outcome = receiver.recv_timeout(deadline);
        let text = fs::read_to_string(&output_path).unwrap_or_default();
        match outcome {
            Ok(error) => {
                drop(running);
                let _ = fs::remove_file(&output_path);
                if text.contains(MISSING_MARKER) {
                    return Err(BridgeError::MoonlightMissing);
                }
                match error {
                    Some(message) => Err(BridgeError::Failed(message)),
                    None => Ok(text),
                }
            }
            Err(_) => {
                // The script carries on; let it end in the background.
                thread::spawn(move || {
                    let _ = receiver.recv();
                    drop(running);
                    let _ = fs::remove_file(output_path);
                });
                Err(BridgeError::TimedOut)
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    use super::*;
    pub fn installed() -> bool {
        false
    }
    pub fn install() -> Result<(), BridgeError> {
        Err(BridgeError::Unsupported)
    }
    pub fn run(_arguments: &[String], _deadline: Option<Duration>) -> Result<String, BridgeError> {
        Err(BridgeError::Unsupported)
    }
}

fn require_available() -> Result<(), BridgeError> {
    if AVAILABLE { Ok(()) } else { Err(BridgeError::Unsupported) }
}

/// Whether a stream game's Play button can work right now.
pub fn launchable() -> bool {
    AVAILABLE && native::installed()
}

/// The bridge's state, asking the script whether Moonlight is there.
pub fn view() -> MoonlightBridgeView {
    if !AVAILABLE {
        return MoonlightBridgeView {
            available: false,
            installed: false,
            moonlight: None,
        };
    }
    let installed = native::installed();
    let moonlight = installed.then(|| {
        native::run(&["--orivo-probe".to_string()], Some(PROBE_TIMEOUT))
            .is_ok_and(|text| text.contains(PRESENT_MARKER))
    });
    MoonlightBridgeView {
        available: true,
        installed,
        moonlight,
    }
}

pub fn install() -> Result<(), BridgeError> {
    require_available()?;
    native::install()
}

/// The applications the PC lists for Moonlight on this Mac.
pub fn list(host: &str) -> Result<Vec<String>, BridgeError> {
    require_available()?;
    let text = native::run(&["list".into(), host.into()], Some(LIST_TIMEOUT))?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .take(2_000)
        .map(str::to_owned)
        .collect())
}

/// Start Moonlight's pairing with `pin`. It waits, in the background, for the
/// PIN to be typed on the PC (three minutes at most).
pub fn pair(host: &str, pin: &str) -> Result<(), BridgeError> {
    require_available()?;
    native::run(&["pair".into(), host.into(), "--pin".into(), pin.into()], None).map(|_| ())
}

/// Start streaming `app` from `host`. Returns once Moonlight has been asked.
pub fn stream(host: &str, app: &str) -> Result<(), BridgeError> {
    require_available()?;
    native::run(&["stream".into(), host.into(), app.into()], None).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn the_script_names_its_version_on_the_second_line() {
        let body = script_body();
        assert!(body.starts_with("#!/bin/sh\n"));
        assert!(is_current_script(&body));
        assert!(!is_current_script("#!/bin/sh\n# orivo-moonlight-bridge 0\n"));
        assert!(!is_current_script(""));
    }

    #[test]
    fn the_script_starts_moonlight_and_nothing_else() {
        let body = script_body();
        assert!(body.contains("--orivo-probe|list|pair|stream) ;;"));
        assert!(body.contains("*) exit 64 ;;"));
        assert_eq!(body.matches("exec ").count(), 5, "list, pair and stream only:\n{body}");
        assert!(!body.contains("eval"));
    }

    #[test]
    fn only_the_four_command_lines_are_passed_on() {
        assert!(validate_arguments(&strings(&["--orivo-probe"])).is_ok());
        assert!(validate_arguments(&strings(&["list", "100.72.203.120"])).is_ok());
        assert!(validate_arguments(&strings(&["list", "astra.local"])).is_ok());
        assert!(validate_arguments(&strings(&["pair", "astra.local", "--pin", "0427"])).is_ok());
        assert!(validate_arguments(&strings(&["stream", "astra.local", "Kena - Bridge of Spirits"])).is_ok());

        for refused in [
            strings(&[]),
            strings(&["quit"]),
            strings(&["list", "-astra"]),
            strings(&["list", "https://astra.local:47990"]),
            strings(&["pair", "astra.local", "--pin", "12345"]),
            strings(&["pair", "astra.local", "--pin", "12a4"]),
            strings(&["stream", "astra.local", "--help"]),
            strings(&["stream", "astra.local", "Two\nlines"]),
            strings(&["stream", "astra.local", "  "]),
            strings(&["list", "astra.local", "extra"]),
        ] {
            assert_eq!(validate_arguments(&refused), Err(BridgeError::InvalidArgument), "{refused:?}");
        }
    }

    #[test]
    fn a_game_name_survives_the_catalog_reference_both_ways() {
        for name in ["TUNIC", "Kena - Bridge of Spirits", "Clair Obscur: Expedition 33", "Ghost of Tsushima: Director's Cut", "Pokémon"] {
            let reference = app_ref(name);
            assert!(crate::catalog::is_valid_provider_reference(&reference), "{reference}");
            assert_eq!(app_name(&reference).as_deref(), Some(name));
        }
        assert_eq!(app_name("x:2d2d68656c70"), None, "--help is refused on the way back");
        assert_eq!(app_name("x:abc"), None);
        assert_eq!(app_name("TUNIC"), None);
    }

    #[test]
    fn the_pc_s_list_becomes_stream_cards_without_desktop_or_big_picture() {
        let games = games_from(&strings(&["Desktop", "Steam Big Picture", "TUNIC", "Rain World", "TUNIC"]));
        let titles: Vec<&str> = games.iter().map(|game| game.title.as_str()).collect();
        assert_eq!(titles.len(), 2);
        assert!(titles.contains(&"TUNIC") && titles.contains(&"Rain World"));
        for game in &games {
            assert_eq!(game.source, GameSource::Stream);
            assert!(game.validate().is_ok());
            let LaunchTarget::Provider { provider, app_ref } = &game.launch_target else {
                panic!("a stream card launches through its provider");
            };
            assert_eq!(provider, PROVIDER);
            assert_eq!(app_name(app_ref).as_deref(), Some(game.title.as_str()));
        }
    }

    #[test]
    fn a_refresh_keeps_what_the_player_made_of_a_card_and_drops_what_the_pc_no_longer_lists() {
        let mut catalog = Catalog::default();
        assert!(apply(&mut catalog, games_from(&strings(&["TUNIC", "Rain World"]))));
        let tunic = catalog.games.iter_mut().find(|game| game.title == "TUNIC").unwrap();
        tunic.play_time_seconds = 3_600;
        tunic.hidden = true;

        assert!(!apply(&mut catalog, games_from(&strings(&["TUNIC", "Rain World"]))), "nothing changed");
        assert!(apply(&mut catalog, games_from(&strings(&["TUNIC", "Nine Sols"]))));
        let titles: Vec<&str> = catalog.games.iter().map(|game| game.title.as_str()).collect();
        assert_eq!(titles.len(), 2);
        assert!(titles.contains(&"Nine Sols") && !titles.contains(&"Rain World"));
        let tunic = catalog.games.iter().find(|game| game.title == "TUNIC").unwrap();
        assert_eq!(tunic.play_time_seconds, 3_600);
        assert!(tunic.hidden);
    }

    #[test]
    fn the_pc_state_reads_the_same_as_the_desktop_setup() {
        let listed = Ok(strings(&["Desktop", "TUNIC"]));
        let ready = host_status("astra.local", true, &listed);
        assert_eq!(ready.state, HostState::Paired);
        assert_eq!(ready.games, 1, "Desktop is not counted as a game");

        assert_eq!(host_status("astra.local", false, &listed).state, HostState::Unreachable);
        assert_eq!(
            host_status("astra.local", true, &Err(BridgeError::Failed("exit 1".into()))).state,
            HostState::NotPaired
        );
        assert_eq!(host_status("astra.local", true, &Err(BridgeError::TimedOut)).state, HostState::Unreachable);
        assert_eq!(
            host_status("astra.local", true, &Err(BridgeError::MoonlightMissing)).state,
            HostState::ClientMissing
        );
    }

    #[test]
    fn outside_the_app_store_edition_nothing_is_run() {
        if AVAILABLE {
            return;
        }
        assert_eq!(list("astra.local"), Err(BridgeError::Unsupported));
        assert_eq!(stream("astra.local", "TUNIC"), Err(BridgeError::Unsupported));
        assert!(!launchable());
        assert!(!view().available);
    }
}
