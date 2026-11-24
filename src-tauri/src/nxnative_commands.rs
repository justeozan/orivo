//! The window and process half of the nxnative proof of concept: the commands
//! the Plugins panel calls, the clone, and the build it follows.
//!
//! Paths stay on this side, as everywhere else in Orivo: the checkout, the game
//! folder and `prod.keys` are chosen through native pickers and kept in
//! `nxnative.json` beside the catalog; the page is told folder names, check
//! states and build events, never a path.
//!
//! Only the Developer ID and development builds on macOS can do any of this.
//! The App Store edition runs in the App Sandbox, where Python, Xcode and
//! Homebrew cannot be started, so it reports `supported: false` and the panel
//! is not offered there.

use crate::{
    AppState, cache_game_media, game_folder,
    nxnative::{self, BuildEvent, BuildOutcome, BuildPlatform, BuildRequest, Check, SystemProbe},
    persist_catalog,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    thread,
};
use tauri::{AppHandle, Emitter, Manager};

const SETTINGS_FILE: &str = "nxnative.json";
/// Where Orivo clones nxnative when the player asks it to.
const MANAGED_CHECKOUT: &str = "nxnative";
/// Orivo's own Python environment for nxnative's modules, beside the clone.
const PYTHON_ENV: &str = "nxnative-python";
const BUILD_EVENT: &str = "nxnative-build";
const FIX_EVENT: &str = "nxnative-fix";
/// The end of stderr kept for a build that fails before JSON mode starts.
const STDERR_TAIL_BYTES: usize = 4 * 1024;

/// The build that is running, by process id, so Cancel can signal it. One at a
/// time: two builds would compete for the same Xcode and the same disk.
static RUNNING_BUILD: Mutex<Option<u32>> = Mutex::new(None);
/// The checklist item being fixed, while a fix (or "Install all") runs. One
/// at a time: two would fight over Homebrew's lock.
static FIXING: Mutex<Option<String>> = Mutex::new(None);

const SUPPORTED: bool = cfg!(all(target_os = "macos", not(feature = "app-store")));

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    checkout: Option<PathBuf>,
    /// The `prod.keys` the player picked once. Its path is passed to the
    /// build; Orivo never opens the file.
    keys: Option<PathBuf>,
    /// The player removed the plugin: its card is offered in the catalogue
    /// instead of the installed list, until they add it back.
    #[serde(default)]
    removed: bool,
}

fn settings_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|directory| directory.join(SETTINGS_FILE))
}

fn load_settings(app: &AppHandle) -> Settings {
    settings_path(app)
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_settings(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_path(app).ok_or("The app data folder is unavailable.")?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

fn python_env(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|directory| directory.join(PYTHON_ENV))
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "nxnative".into())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NxnativeStatus {
    supported: bool,
    /// The checkout's folder name, when one is set and still a checkout.
    checkout: Option<String>,
    checks: Vec<Check>,
    building: bool,
    /// The item being installed, while an install runs.
    fixing: Option<String>,
    keys_chosen: bool,
}

/// The checklist and what is set up. It runs a handful of version queries,
/// so it is computed off the main thread.
#[tauri::command]
pub async fn nxnative_status(app: AppHandle) -> Result<NxnativeStatus, String> {
    if !SUPPORTED {
        return Ok(NxnativeStatus {
            supported: false,
            checkout: None,
            checks: Vec::new(),
            building: false,
            fixing: None,
            keys_chosen: false,
        });
    }
    tauri::async_runtime::spawn_blocking(move || {
        let settings = load_settings(&app);
        let probe = SystemProbe;
        let checkout = settings
            .checkout
            .as_deref()
            .filter(|checkout| nxnative::is_checkout(&probe, checkout));
        let os_version = macos_version();
        NxnativeStatus {
            supported: true,
            checkout: checkout.map(folder_name),
            checks: nxnative::doctor(
                &probe,
                os_version.as_deref(),
                checkout,
                python_env(&app).as_deref(),
            ),
            building: RUNNING_BUILD.lock().is_ok_and(|running| running.is_some()),
            fixing: FIXING.lock().ok().and_then(|fixing| fixing.clone()),
            keys_chosen: settings.keys.as_deref().is_some_and(Path::is_file),
        }
    })
    .await
    .map_err(|error| error.to_string())
}

fn macos_version() -> Option<String> {
    let output = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Point Orivo at a checkout the player already has.
#[tauri::command]
pub fn nxnative_choose_checkout(app: AppHandle) -> Result<Option<String>, String> {
    if !SUPPORTED {
        return Err("nxnative needs the Mac download of Orivo.".into());
    }
    #[cfg(desktop)]
    {
        let Some(folder) = rfd::FileDialog::new()
            .set_title("Choose your nxnative folder")
            .pick_folder()
        else {
            return Ok(None);
        };
        if !nxnative::is_checkout(&SystemProbe, &folder) {
            return Err(format!(
                "This folder has no {}. Choose the folder you cloned nxnative into.",
                nxnative::BUILD_SCRIPT
            ));
        }
        let mut settings = load_settings(&app);
        settings.checkout = Some(folder.clone());
        save_settings(&app, &settings)?;
        Ok(Some(folder_name(&folder)))
    }
    #[cfg(mobile)]
    {
        let _ = app;
        Ok(None)
    }
}

/// Whether the plugin is offered in the installed list: on a Mac that can run
/// it, and not removed by the player. Cheap: it reads one small file.
#[tauri::command]
pub fn nxnative_installed(app: AppHandle) -> bool {
    SUPPORTED && !load_settings(&app).removed
}

/// Remove the plugin: what Orivo made for it (its clone of nxnative, its
/// Python environment) is deleted, and the player's own checkout, `prod.keys`
/// and games are left where they are, only forgotten. Games already built
/// stay in the library and still start.
#[tauri::command]
pub async fn nxnative_remove(app: AppHandle) -> Result<(), String> {
    if !SUPPORTED {
        return Err("nxnative needs the Mac download of Orivo.".into());
    }
    if RUNNING_BUILD.lock().is_ok_and(|running| running.is_some()) {
        return Err("A build is running.".into());
    }
    if FIXING.lock().is_ok_and(|fixing| fixing.is_some()) {
        return Err("An install is running.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        for folder in [managed_checkout(&app), python_env(&app)]
            .into_iter()
            .flatten()
        {
            match std::fs::remove_dir_all(&folder) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        save_settings(
            &app,
            &Settings {
                removed: true,
                ..Settings::default()
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Add the plugin back after a removal.
#[tauri::command]
pub fn nxnative_restore(app: AppHandle) -> Result<(), String> {
    if !SUPPORTED {
        return Err("nxnative needs the Mac download of Orivo.".into());
    }
    let mut settings = load_settings(&app);
    settings.removed = false;
    save_settings(&app, &settings)
}

/// Update on request: bring nxnative up to date (`git pull --ff-only`, as
/// the clone's own fix does; the clone is made when there is none), then look
/// in the games folders now. The same two things happen by themselves, the
/// folders every few minutes; this is the button for "now". The output streams
/// as `nxnative-fix` lines under the nxnative row.
#[tauri::command]
pub fn nxnative_update(app: AppHandle) -> Result<(), String> {
    if !SUPPORTED {
        return Err("nxnative needs the Mac download of Orivo.".into());
    }
    if RUNNING_BUILD.lock().is_ok_and(|running| running.is_some()) {
        return Err("A build is running.".into());
    }
    {
        let mut fixing = FIXING.lock().map_err(|_| "Busy.".to_string())?;
        if fixing.is_some() {
            return Err("An install is already running.".into());
        }
        *fixing = Some("checkout".into());
    }
    thread::spawn(move || {
        let check = "checkout".to_owned();
        let settings = load_settings(&app);
        let target = settings
            .checkout
            .filter(|checkout| nxnative::is_checkout(&SystemProbe, checkout))
            .or_else(|| managed_checkout(&app));
        let _ = app.emit(
            FIX_EVENT,
            FixMessage::Started {
                check: check.clone(),
            },
        );
        let result = match target {
            Some(target) => run_fix_job(&app, &check, FixJob::Clone(target)),
            None => Err("The app data folder is unavailable.".into()),
        };
        let _ = app.emit(
            FIX_EVENT,
            FixMessage::Finished {
                check,
                ok: result.is_ok(),
                message: result.err(),
            },
        );
        if let Ok(mut fixing) = FIXING.lock() {
            *fixing = None;
        }
        let _ = app.emit(FIX_EVENT, FixMessage::Done);
        crate::run_game_folder_scan(&app, crate::ScanRequest::All);
    });
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
enum BuildMessage {
    Started {
        game: String,
    },
    Event {
        event: BuildEvent,
    },
    #[serde(rename_all = "camelCase")]
    Finished {
        outcome: BuildOutcome,
        message: Option<String>,
        /// The library card of the built Mac app, once it is in the library.
        game_id: Option<String>,
    },
}

/// Pick a game and build it. Returns `false` when a picker was cancelled;
/// the build itself reports through `nxnative-build` events.
#[tauri::command]
pub fn nxnative_build(app: AppHandle, platform: BuildPlatform) -> Result<bool, String> {
    if !SUPPORTED {
        return Err("nxnative needs the Mac download of Orivo.".into());
    }
    #[cfg(mobile)]
    {
        let _ = (app, platform);
        Ok(false)
    }
    #[cfg(desktop)]
    {
        if RUNNING_BUILD.lock().is_ok_and(|running| running.is_some()) {
            return Err("A build is already running.".into());
        }
        let probe = SystemProbe;
        let mut settings = load_settings(&app);
        let checkout = settings
            .checkout
            .clone()
            .filter(|checkout| nxnative::is_checkout(&probe, checkout))
            .ok_or("Set up nxnative first.")?;
        let (python, _) = nxnative::find_python(&probe, python_env(&app).as_deref())
            .filter(|(_, version)| *version >= (3, 12))
            .ok_or("Install Python 3.12 or newer first.")?;

        let Some(source) = rfd::FileDialog::new()
            .set_title("Choose the folder that holds one game (its NSPs, its dump, or its .nup)")
            .pick_folder()
        else {
            return Ok(false);
        };
        let source_folder = source.clone();
        // A single .nup in the folder is the container itself.
        let source = single_nup(&source).unwrap_or(source);

        let keys = if nxnative::source_needs_keys(&source) {
            match settings.keys.clone().filter(|keys| keys.is_file()) {
                Some(keys) => Some(keys),
                None => {
                    let Some(keys) = rfd::FileDialog::new()
                        .set_title("Choose your prod.keys")
                        .add_filter("prod.keys", &["keys"])
                        .pick_file()
                    else {
                        return Ok(false);
                    };
                    settings.keys = Some(keys.clone());
                    save_settings(&app, &settings)?;
                    Some(keys)
                }
            }
        } else {
            None
        };

        let request = BuildRequest {
            python,
            checkout,
            source: source.clone(),
            keys,
            platform,
        };
        let mut child = nxnative::build_command(&request)
            .spawn()
            .map_err(|error| format!("The build could not start: {error}"))?;
        {
            let mut running = RUNNING_BUILD
                .lock()
                .map_err(|_| "The build state is unavailable.".to_string())?;
            *running = Some(child.id());
        }
        let _ = app.emit(
            BUILD_EVENT,
            BuildMessage::Started {
                game: folder_name(&source),
            },
        );
        // "Add a game" is the library's "Add a folder": the folder joins the
        // games folders, so what is built or updated in it is found again.
        if platform == BuildPlatform::MacApp {
            crate::remember_game_folder(&app, &source_folder);
        }

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        thread::spawn(move || {
            let stderr_tail = thread::spawn(move || {
                let mut bytes = Vec::new();
                if let Some(mut stderr) = stderr {
                    let _ = stderr.read_to_end(&mut bytes);
                }
                let start = bytes.len().saturating_sub(STDERR_TAIL_BYTES);
                String::from_utf8_lossy(&bytes[start..]).into_owned()
            });

            let mut built: Option<PathBuf> = None;
            let mut last_error: Option<String> = None;
            if let Some(stdout) = stdout {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let Some(event) = nxnative::parse_event(&line) else {
                        continue;
                    };
                    match &event {
                        BuildEvent::Game { out, .. } => built.clone_from(out),
                        BuildEvent::Say {
                            message,
                            error: true,
                        } => last_error = Some(message.clone()),
                        _ => {}
                    }
                    let _ = app.emit(BUILD_EVENT, BuildMessage::Event { event });
                }
            }
            let status = child.wait();
            if let Ok(mut running) = RUNNING_BUILD.lock() {
                *running = None;
            }
            let stderr_tail = stderr_tail.join().unwrap_or_default();
            let outcome = nxnative::outcome(status.ok().and_then(|status| status.code()));

            let mut message = match outcome {
                BuildOutcome::Failed => last_error,
                BuildOutcome::InvalidArguments | BuildOutcome::Crashed => stderr_tail
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .map(str::to_owned),
                _ => None,
            };
            let mut game_id = None;
            if outcome == BuildOutcome::Finished
                && platform == BuildPlatform::MacApp
                && let Some(app_path) = built.filter(|path| path.is_dir())
            {
                match add_built_app(&app, &app_path) {
                    Ok(id) => game_id = Some(id),
                    Err(error) => message = Some(error),
                }
            }
            let _ = app.emit(
                BUILD_EVENT,
                BuildMessage::Finished {
                    outcome,
                    message,
                    game_id,
                },
            );
        });
        Ok(true)
    }
}

/// A folder that holds exactly one `.nup` and no packages is that container.
fn single_nup(folder: &Path) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(Result::ok)
        .take(4096)
        .map(|entry| entry.path())
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect();
    let nups: Vec<&PathBuf> = entries
        .iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("nup"))
        })
        .collect();
    (nups.len() == 1 && !nxnative::source_needs_keys(folder)).then(|| nups[0].clone())
}

/// Stop the running build. SIGTERM is the contract's cancel: the script
/// signals the tools it started, and kills what is left five seconds later.
#[tauri::command]
pub fn nxnative_cancel_build() -> Result<(), String> {
    let running = RUNNING_BUILD
        .lock()
        .map_err(|_| "The build state is unavailable.".to_string())?;
    #[cfg(unix)]
    if let Some(pid) = *running {
        // The child is not reaped until the build thread's `wait` returns, and
        // that thread clears this slot first, so the id cannot name another
        // process yet.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NxnativeGame {
    game_id: String,
    title: String,
    title_id: String,
    /// The app's settings list; empty when it carries none.
    settings: Vec<nxnative::LauncherSetting>,
    /// The player's saved choices that differ from the tested defaults.
    saved: std::collections::BTreeMap<String, String>,
    /// Orivo starts it covering the screen on the player's desktop, so its
    /// Display mode is Orivo's to set, not the player's.
    keeps_space: bool,
}

/// The library's nxnative games, each with its settings and saved choices.
#[tauri::command]
pub async fn nxnative_games(app: AppHandle) -> Result<Vec<NxnativeGame>, String> {
    if !SUPPORTED {
        return Ok(Vec::new());
    }
    let games: Vec<(String, String, PathBuf)> = {
        let state = app.state::<AppState>();
        let catalog = state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        catalog
            .games
            .iter()
            .filter(|game| !game.hidden)
            .filter_map(|game| {
                let bundle = nxnative::bundle_of(game.executable_path.as_deref()?)?;
                Some((game.id.clone(), game.title.clone(), bundle))
            })
            .collect()
    };
    tauri::async_runtime::spawn_blocking(move || {
        games
            .into_iter()
            .filter_map(|(game_id, title, bundle)| {
                let identity = nxnative::inspect_bundle(&bundle)?;
                let settings = nxnative::launcher_settings(&bundle)
                    .map(|list| list.settings)
                    .unwrap_or_default();
                let saved = identity
                    .bundle_id
                    .as_deref()
                    .map(read_saved_choices)
                    .unwrap_or_default();
                Some(NxnativeGame {
                    game_id,
                    title,
                    title_id: identity.title_id,
                    settings,
                    saved,
                    keeps_space: FILL_LIBRARY.is_some(),
                })
            })
            .collect()
    })
    .await
    .map_err(|error| error.to_string())
}

fn read_saved_choices(bundle_id: &str) -> std::collections::BTreeMap<String, String> {
    if !nxnative::valid_bundle_id(bundle_id) {
        return Default::default();
    }
    Command::new("/usr/bin/defaults")
        .args(["export", bundle_id, "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| nxnative::saved_choices(&output.stdout))
        .unwrap_or_default()
}

/// Keep, or forget, one setting's choice in the app's own preferences — the
/// store its settings window and its Finder start read. Choosing the tested
/// default forgets the choice, so a later build's default reaches the player.
#[tauri::command]
pub async fn nxnative_set_choice(
    app: AppHandle,
    game_id: String,
    env: String,
    value: Option<String>,
) -> Result<(), String> {
    if !SUPPORTED {
        return Err("nxnative needs the Mac download of Orivo.".into());
    }
    let bundle = {
        let state = app.state::<AppState>();
        let catalog = state
            .catalog
            .read()
            .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
        catalog
            .games
            .iter()
            .find(|game| game.id == game_id)
            .and_then(|game| nxnative::bundle_of(game.executable_path.as_deref()?))
            .ok_or("This game is not an nxnative app.")?
    };
    tauri::async_runtime::spawn_blocking(move || {
        let identity =
            nxnative::inspect_bundle(&bundle).ok_or("This game is not an nxnative app.")?;
        let bundle_id = identity
            .bundle_id
            .filter(|id| nxnative::valid_bundle_id(id))
            .ok_or("This app has no usable bundle identifier.")?;
        let list = nxnative::launcher_settings(&bundle).ok_or("This app has no settings.")?;
        let setting = list
            .settings
            .iter()
            .find(|setting| setting.env == env)
            .ok_or("This app has no such setting.")?;
        let key = nxnative::saved_choice_key(&setting.env);
        let keep = value.filter(|value| setting.default.as_deref() != Some(value.as_str()));
        let mut command = Command::new("/usr/bin/defaults");
        match &keep {
            Some(value) if nxnative::accepted_value(setting, value) => {
                command.args(["write", &bundle_id, &key, "-string", value]);
            }
            Some(_) => return Err("This build does not offer that value.".to_string()),
            None => {
                command.args(["delete", &bundle_id, &key]);
            }
        }
        // `defaults delete` of a key that is not there fails, and that is fine:
        // the default is what the player asked for and what they have.
        let status = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| error.to_string())?;
        if keep.is_some() && !status.success() {
            return Err("The setting could not be saved.".into());
        }
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Open one of the download pages the checklist points at. A closed set: the
/// page names a link, never a URL.
#[tauri::command]
pub fn nxnative_open_link(link: String) -> Result<(), String> {
    let url = match link.as_str() {
        "xcode" => "https://developer.apple.com/download/all/?q=Xcode%2027.2",
        "homebrew" => "https://brew.sh",
        "nxnative" => "https://github.com/seedds/nxnative",
        _ => return Err("Unknown link.".into()),
    };
    Command::new("/usr/bin/open")
        .arg(url)
        .stdin(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
enum FixMessage {
    Started {
        check: String,
    },
    Line {
        check: String,
        line: String,
    },
    Finished {
        check: String,
        ok: bool,
        message: Option<String>,
    },
    /// The queue (one fix, or "Install all") is over.
    Done,
}

/// How one missing item is fixed.
enum FixJob {
    /// A shell line, run unattended with Homebrew on `PATH`.
    Shell { command: String, shown: String },
    /// A command that needs an administrator: macOS's password sheet.
    Admin(String),
    /// nxnative itself, cloned into Orivo's data folder (or updated).
    Clone(PathBuf),
}

/// Fix one missing item, inside Orivo: its output streams to the panel as
/// `nxnative-fix` lines. The page names the check; the command is the one the
/// checklist holds for it, so nothing the page sends is ever run.
#[tauri::command]
pub fn nxnative_run_fix(app: AppHandle, check: String) -> Result<(), String> {
    if !nxnative::FIX_ORDER.contains(&check.as_str()) {
        return Err("There is nothing to run for this item.".into());
    }
    start_fixes(app, vec![check])
}

/// Fix everything that can be fixed, one item after the other.
#[tauri::command]
pub fn nxnative_install_all(app: AppHandle) -> Result<(), String> {
    start_fixes(
        app,
        nxnative::FIX_ORDER
            .iter()
            .map(|id| (*id).to_owned())
            .collect(),
    )
}

fn start_fixes(app: AppHandle, queue: Vec<String>) -> Result<(), String> {
    if !SUPPORTED {
        return Err("Building Switch games needs the Mac download of Orivo.".into());
    }
    {
        let mut fixing = FIXING.lock().map_err(|_| "Busy.".to_string())?;
        if fixing.is_some() {
            return Err("An install is already running.".into());
        }
        *fixing = Some(String::new());
    }
    let single = queue.len() == 1;
    thread::spawn(move || {
        for check in queue {
            if let Ok(mut fixing) = FIXING.lock() {
                *fixing = Some(check.clone());
            }
            let job = match prepare_fix(&app, &check) {
                // Already fine: "Install all" moves on quietly.
                Ok(None) if !single => continue,
                Ok(None) => Err("Already ready.".to_string()),
                Ok(Some(job)) => Ok(job),
                Err(message) => Err(message),
            };
            let _ = app.emit(
                FIX_EVENT,
                FixMessage::Started {
                    check: check.clone(),
                },
            );
            let result = job
                .and_then(|job| run_fix_job(&app, &check, job))
                .and_then(|()| {
                    // A command that exits 0 has not necessarily fixed the item.
                    match prepare_fix(&app, &check) {
                        Ok(None) => Ok(()),
                        _ => Err("Finished, but this is still not ready.".into()),
                    }
                });
            let _ = app.emit(
                FIX_EVENT,
                FixMessage::Finished {
                    check: check.clone(),
                    ok: result.is_ok(),
                    message: result.err(),
                },
            );
        }
        if let Ok(mut fixing) = FIXING.lock() {
            *fixing = None;
        }
        let _ = app.emit(FIX_EVENT, FixMessage::Done);
    });
    Ok(())
}

/// What fixing `check` takes now, or `None` when it is already ready.
fn prepare_fix(app: &AppHandle, check: &str) -> Result<Option<FixJob>, String> {
    let settings = load_settings(app);
    let probe = SystemProbe;
    let checkout = settings
        .checkout
        .as_deref()
        .filter(|checkout| nxnative::is_checkout(&probe, checkout));
    let env = python_env(app);
    let checks = nxnative::doctor(&probe, macos_version().as_deref(), checkout, env.as_deref());
    let state = |id: &str| {
        checks
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.state)
    };
    let Some(item) = checks.iter().find(|item| item.id == check) else {
        return Err("Unknown item.".into());
    };
    if item.state == nxnative::CheckState::Ok {
        return Ok(None);
    }
    let shown = item.fix.clone().unwrap_or_default();
    let job = match check {
        "checkout" => {
            FixJob::Clone(managed_checkout(app).ok_or("The app data folder is unavailable.")?)
        }
        "xcode" => FixJob::Admin(shown),
        "python-modules" => {
            let env = env.ok_or("The app data folder is unavailable.")?;
            // The environment is made from the system's Python, never from itself.
            let (base, _) = nxnative::find_python(&probe, None)
                .filter(|(_, version)| *version >= (3, 12))
                .ok_or("Install Python 3.12 or newer first.")?;
            FixJob::Shell {
                command: nxnative::python_env_command(&base, &env),
                shown,
            }
        }
        _ if !shown.is_empty() => FixJob::Shell {
            command: shown.clone(),
            shown,
        },
        _ => return Err("Orivo cannot install this one.".into()),
    };
    if let FixJob::Shell { shown, .. } = &job
        && shown.starts_with("brew ")
        && state("homebrew") != Some(nxnative::CheckState::Ok)
    {
        return Err("Install Homebrew first.".into());
    }
    Ok(Some(job))
}

fn run_fix_job(app: &AppHandle, check: &str, job: FixJob) -> Result<(), String> {
    let say = |line: String| {
        let _ = app.emit(
            FIX_EVENT,
            FixMessage::Line {
                check: check.to_owned(),
                line,
            },
        );
    };
    match job {
        FixJob::Shell { command, shown } => {
            say(format!("$ {shown}"));
            let mut process = Command::new("/bin/zsh");
            process
                .arg("-c")
                .arg(nxnative::unattended(&command))
                .env("PATH", nxnative::tool_path())
                .envs(nxnative::FIX_ENV.iter().copied());
            stream(process, &say)
        }
        FixJob::Admin(command) => {
            say(format!("$ {command}  (administrator)"));
            let mut process = Command::new("/usr/bin/osascript");
            process.arg("-e").arg(nxnative::admin_script(&command));
            stream(process, &say)
        }
        FixJob::Clone(target) => {
            let mut process = Command::new("/usr/bin/git");
            if nxnative::is_checkout(&SystemProbe, &target) {
                say("$ git pull --ff-only".into());
                process
                    .arg("-C")
                    .arg(&target)
                    .args(["pull", "--ff-only", "--progress"]);
            } else {
                say(format!(
                    "$ git clone --depth 1 {}",
                    nxnative::REPOSITORY_URL
                ));
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                // A half-made folder from a failed clone would stop the next one.
                let _ = std::fs::remove_dir_all(&target);
                process
                    .args([
                        "clone",
                        "--depth",
                        "1",
                        "--progress",
                        nxnative::REPOSITORY_URL,
                    ])
                    .arg(&target);
            }
            process
                .env("PATH", nxnative::tool_path())
                .env("GIT_TERMINAL_PROMPT", "0");
            stream(process, &say)?;
            let mut settings = load_settings(app);
            settings.checkout = Some(target);
            save_settings(app, &settings)
        }
    }
}

/// Run a process with no input, passing each line of its output and its
/// errors to `say` as it comes. The error is the exit code and its last line.
fn stream(mut process: Command, say: &dyn Fn(String)) -> Result<(), String> {
    let mut child = process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let (sender, receiver) = std::sync::mpsc::channel::<String>();
    let pumps: Vec<_> = [
        child
            .stdout
            .take()
            .map(|out| Box::new(out) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|err| Box::new(err) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|mut reader| {
        let sender = sender.clone();
        thread::spawn(move || {
            let mut splitter = nxnative::LineSplitter::default();
            let mut buffer = [0u8; 4096];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                for line in splitter.push(&buffer[..read]) {
                    let _ = sender.send(line);
                }
            }
            if let Some(line) = splitter.finish() {
                let _ = sender.send(line);
            }
        })
    })
    .collect();
    drop(sender);
    let mut last = None;
    for line in receiver {
        say(line.clone());
        last = Some(line);
    }
    for pump in pumps {
        let _ = pump.join();
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        let code = status
            .code()
            .map_or("signal".to_owned(), |code| format!("exit {code}"));
        Err(match last {
            Some(line) => format!("{line} ({code})"),
            None => code,
        })
    }
}

/// The environment a library card is started with when it is a built
/// nxnative app (`nxnative::console_env`), or `None` for any other game.
pub fn console_env_for(app: &AppHandle, executable: &Path) -> Option<Vec<(&'static str, String)>> {
    let bundle = nxnative::bundle_of(executable)?;
    nxnative::inspect_bundle(&bundle)?;
    let french = reads_french(app).then(|| {
        plist::Value::from_file(bundle.join("Contents/Info.plist"))
            .ok()
            .and_then(|info| {
                let info = info.as_dictionary()?;
                ["CFBundleDisplayName", "CFBundleName"]
                    .iter()
                    .find_map(|key| {
                        info.get(key)
                            .and_then(plist::Value::as_string)
                            .map(str::to_owned)
                    })
            })
            .unwrap_or_else(|| folder_name(&bundle).trim_end_matches(".app").to_owned())
    });
    Some(nxnative::console_env(
        fill_library(app).as_deref(),
        french.as_deref(),
    ))
}

const GAME_WINDOW_EVENT: &str = "game-window";
/// How often the window list is read while a game starts.
const WINDOW_POLL: std::time::Duration = std::time::Duration::from_millis(150);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameWindowMessage {
    game_id: String,
    /// `true`: its window is on screen; `false`: it ended before it had one.
    shown: bool,
}

/// Follow a built app from its start: say when the game's window is on
/// screen, so Orivo steps aside then and not before, or that it ended first.
/// The app's launcher runs the game as its child, so the window to look for
/// is one of the launcher's or of a process it started. The play time is
/// recorded when it ends, as for any game Orivo starts.
pub fn watch_console_start(
    app: AppHandle,
    game_id: String,
    mut child: std::process::Child,
    record: fn(&AppHandle, &str, std::time::Instant),
) {
    let started = std::time::Instant::now();
    thread::spawn(move || {
        let launcher = child.id() as i32;
        let mut shown = false;
        loop {
            if !matches!(child.try_wait(), Ok(None)) {
                break;
            }
            if let Some(owner) = window_of(launcher) {
                // Orivo is still the active app: it hands the game the
                // keyboard, which a game cannot always take by itself.
                activate(owner);
                shown = true;
                break;
            }
            thread::sleep(WINDOW_POLL);
        }
        let _ = app.emit(
            GAME_WINDOW_EVENT,
            GameWindowMessage {
                game_id: game_id.clone(),
                shown,
            },
        );
        let _ = child.wait();
        record(&app, &game_id, started);
    });
}

/// The process, `launcher` or one it started, that has a window on screen at
/// the normal level. The window list's owner ids need no permission; titles
/// would.
#[cfg(target_os = "macos")]
fn window_of(launcher: i32) -> Option<i32> {
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const c_void;
        static kCGWindowOwnerPID: *const c_void;
        static kCGWindowLayer: *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
        fn CFNumberGetValue(number: *const c_void, kind: isize, out: *mut c_void) -> bool;
        fn CFRelease(value: *const c_void);
    }
    const ON_SCREEN_ONLY: u32 = 1 << 0;
    const EXCLUDE_DESKTOP: u32 = 1 << 4;
    const SINT32: isize = 3;
    let number = |dictionary: *const c_void, key: *const c_void| -> Option<i32> {
        let value = unsafe { CFDictionaryGetValue(dictionary, key) };
        let mut out: i32 = 0;
        (!value.is_null()
            && unsafe { CFNumberGetValue(value, SINT32, (&mut out as *mut i32).cast()) })
        .then_some(out)
    };
    let list = unsafe { CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0) };
    if list.is_null() {
        return None;
    }
    let mut found = None;
    for index in 0..unsafe { CFArrayGetCount(list) } {
        let window = unsafe { CFArrayGetValueAtIndex(list, index) };
        let (Some(owner), Some(0)) = (
            number(window, unsafe { kCGWindowOwnerPID }),
            number(window, unsafe { kCGWindowLayer }),
        ) else {
            continue;
        };
        if owner == launcher || parent_of(owner) == Some(launcher) {
            found = Some(owner);
            break;
        }
    }
    unsafe { CFRelease(list) };
    found
}

#[cfg(target_os = "macos")]
fn parent_of(pid: i32) -> Option<i32> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (read == size).then_some(info.pbi_ppid as i32)
}

#[cfg(not(target_os = "macos"))]
fn window_of(_launcher: i32) -> Option<i32> {
    None
}

/// Make `pid` the active app, as clicking its window would.
#[cfg(target_os = "macos")]
fn activate(pid: i32) {
    use objc2::{class, msg_send, runtime::AnyObject};
    // NSApplicationActivateAllWindows
    const ALL_WINDOWS: usize = 1 << 0;
    unsafe {
        let app: *mut AnyObject = msg_send![
            class!(NSRunningApplication),
            runningApplicationWithProcessIdentifier: pid
        ];
        if !app.is_null() {
            let _: bool = msg_send![app, activateWithOptions: ALL_WINDOWS];
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn activate(_pid: i32) {}

/// The library that keeps a game's fullscreen on the player's desktop
/// (`src/nxnative_fill.m`, built by `build.rs`).
#[cfg(target_os = "macos")]
const FILL_LIBRARY: Option<&[u8]> = Some(include_bytes!(concat!(
    env!("OUT_DIR"),
    "/nxnative-fill.dylib"
)));
#[cfg(not(target_os = "macos"))]
const FILL_LIBRARY: Option<&[u8]> = None;

/// `FILL_LIBRARY` as a file a game can load, in Orivo's data folder. Named by
/// its contents, so a game still running keeps the copy it loaded when a newer
/// Orivo writes another.
fn fill_library(app: &AppHandle) -> Option<PathBuf> {
    let bytes = FILL_LIBRARY?;
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    });
    let path = app
        .path()
        .app_data_dir()
        .ok()?
        .join(format!("nxnative-fill-{hash:016x}.dylib"));
    if std::fs::read(&path).ok().as_deref() != Some(bytes) {
        std::fs::create_dir_all(path.parent()?).ok()?;
        let partial = path.with_extension("partial");
        std::fs::write(&partial, bytes).ok()?;
        std::fs::rename(&partial, &path).ok()?;
    }
    Some(path)
}

/// Whether the player reads Orivo in French: the Language setting, or the
/// Mac's first language when it follows the system.
fn reads_french(app: &AppHandle) -> bool {
    use crate::preferences::{Language, PreferencesService};
    match PreferencesService::from_app(app)
        .and_then(|service| service.load())
        .map(|preferences| preferences.language)
        .unwrap_or_default()
    {
        Language::Fr => true,
        Language::En => false,
        Language::System => std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library/Preferences/.GlobalPreferences.plist"))
            .and_then(|path| plist::Value::from_file(path).ok())
            .and_then(|global| {
                global
                    .as_dictionary()?
                    .get("AppleLanguages")?
                    .as_array()?
                    .first()?
                    .as_string()
                    .map(|language| language.starts_with("fr"))
            })
            .unwrap_or(false),
    }
}

/// The checkout Orivo cloned itself, as opposed to one the player chose.
fn managed_checkout(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|directory| directory.join(MANAGED_CHECKOUT))
}

/// Put the built Mac app in the library, as importing it by hand would. A
/// rebuild of a game already there keeps its card.
fn add_built_app(app: &AppHandle, bundle: &Path) -> Result<String, String> {
    let state = app.state::<AppState>();
    let mut game = game_folder::game_for_path(bundle)?;
    let _ = cache_game_media(app, &mut game);
    let id = game.id.clone();
    let _mutation = state
        .catalog_mutation
        .lock()
        .map_err(|_| "The game catalog is temporarily unavailable".to_string())?;
    let mut next = state
        .catalog
        .read()
        .map_err(|_| "The game catalog is temporarily unavailable".to_string())?
        .clone();
    if next.games.iter().any(|existing| existing.id == id) {
        return Ok(id);
    }
    next.add(game).map_err(|error| error.to_string())?;
    persist_catalog(&next, &state.catalog_path).map_err(|error| error.to_string())?;
    *state
        .catalog
        .write()
        .map_err(|_| "The game catalog is temporarily unavailable".to_string())? = next;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    #[test]
    fn settings_saved_before_removal_existed_still_load_as_installed() {
        let old: Settings = serde_json::from_str(r#"{"checkout":"/tmp/nxnative"}"#).unwrap();
        assert!(!old.removed);
        let removed: Settings = serde_json::from_str(r#"{"removed":true}"#).unwrap();
        assert!(removed.removed && removed.checkout.is_none());
    }

    fn run(command: &str) -> (Result<(), String>, Vec<String>) {
        let lines = StdMutex::new(Vec::new());
        let mut process = Command::new("/bin/zsh");
        process
            .arg("-c")
            .arg(nxnative::unattended(command))
            .envs(nxnative::FIX_ENV.iter().copied());
        let result = stream(process, &|line| lines.lock().unwrap().push(line));
        (result, lines.into_inner().unwrap())
    }

    #[test]
    fn a_fix_streams_its_output_and_errors_and_answers_yes() {
        let (result, lines) = run(
            "echo one; printf '10%%\\r50%%\\r'; echo two >&2; read answer; echo \"answer $answer\"",
        );
        assert_eq!(result, Ok(()));
        for expected in ["one", "10%", "50%", "two", "answer y"] {
            assert!(
                lines.iter().any(|line| line == expected),
                "{expected} in {lines:?}"
            );
        }
    }

    #[test]
    fn a_failed_fix_says_its_last_line_and_exit_code() {
        let (result, _) = run("echo 'Error: No such formula'; exit 1");
        assert_eq!(result, Err("Error: No such formula (exit 1)".into()));
    }

    #[test]
    fn a_process_without_a_window_has_none_on_screen() {
        let mut child = Command::new("/bin/sleep").arg("5").spawn().unwrap();
        assert_eq!(window_of(child.id() as i32), None);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_fill_library_is_built_in() {
        let library = FILL_LIBRARY.expect("built by build.rs");
        // A Mach-O 64-bit image, arm64 as the games are.
        assert_eq!(&library[..4], &[0xcf, 0xfa, 0xed, 0xfe]);
        assert_eq!(&library[4..8], &[0x0c, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn homebrew_and_pip_are_told_not_to_ask() {
        let (_, lines) = run("echo \"$NONINTERACTIVE $PIP_NO_INPUT\"");
        assert_eq!(lines, ["1 1"]);
    }
}
