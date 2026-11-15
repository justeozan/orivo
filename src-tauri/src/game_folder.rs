//! Games folders: directories the player chooses once, and every game in them
//! joins the library without another trip through the file dialog.
//!
//! One game per item directly inside a folder:
//!
//! - an executable (`.exe`, and on Windows `.bat`/`.cmd`), or a macOS `.app`;
//! - a shortcut to a game: a Windows `.lnk`, a `.url` that opens a game
//!   launcher (Steam, Epic…), a macOS Finder alias, or a symbolic link;
//! - a folder, whose game is its `.app` on macOS, or its largest eligible
//!   `.exe` found up to three directory levels down. Setup programs,
//!   redistributables and crash handlers are never the game, and the folders
//!   installers and Steam keep their own files in are skipped.
//!
//! A symbolic link is followed once, for the item it is; links met inside a
//! game's folder are never followed. The scan reads names, sizes, listings and
//! shortcut files only. It never runs anything.

use std::{
    collections::BTreeSet,
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    catalog::{Catalog, Game, GameSource, LaunchTarget, local_game_id},
    shortcut,
};

/// Directory listings walked for one scan, across every game folder.
pub const MAX_ENTRIES: usize = 20_000;
/// How many directory levels below a game folder can hold its executable.
/// Four reaches an Unreal game's `Game\Binaries\Win64\Game-Win64-Shipping.exe`.
const EXECUTABLE_LEVELS: usize = 4;

/// Spellings that mark a program as something other than the game, wherever
/// they appear in its name: no game is called "vc_redist" or "CrashHandler".
const TOOL_FRAGMENTS: &[&str] = &[
    "crashhandler",
    "crashreport",
    "crashpad",
    "crashsender",
    "vc_redist",
    "vcredist",
    "dxsetup",
    "dxwebsetup",
    "dotnet",
    // Checksum checkers repackers ship beside the game.
    "quicksfv",
    "rapidcrc",
    // Helpers an engine or a store leaves in the game's folder.
    "errorreporter",
    "browsersubprocess",
    "zfgamebrowser",
    "workshoputility",
    "oalinst",
    "physx_",
];

/// Whole words that mark a program as an installer, an uninstaller or a
/// prerequisite. Compared to words, not to any part of the name, so
/// `CrashBandicoot.exe` and `Reinstalled.exe` stay games.
const TOOL_WORDS: &[&str] = &[
    "setup",
    "install",
    "installer",
    "uninstall",
    "uninstaller",
    "redist",
    "redistributable",
    "prereq",
    "prereqs",
    "prerequisites",
    "sfv",
    "md5",
    "crc",
    "checksum",
];

/// Words of programs that start with the game but are rarely the one to
/// run: a debug build, a bootstrapper, a dedicated server, an editor. One is
/// chosen only when a folder holds nothing better.
const SECOND_CHOICE_WORDS: &[&str] = &[
    "debug",
    "bootstrapper",
    "server",
    "dedicated",
    "editor",
    "benchmark",
    "config",
    "configurator",
    "settings",
    // Emulators a repack runs its game through: the repack's own launcher,
    // when it has one, is what starts the game.
    "ryujinx",
    "yuzu",
    "suyu",
    "sudachi",
    "citron",
    "torzu",
    "cemu",
    "rpcs3",
    "pcsx2",
    "duckstation",
    "ppsspp",
    "retroarch",
    "xenia",
    "vita3k",
    "shadps4",
    "citra",
    "melonds",
];

/// The word after "crash" that makes a crash reporter of the program.
const CRASH_TOOL_WORDS: &[&str] = &["handler", "reporter", "report", "sender", "dump", "pad"];

/// Folders whose executables are never a game of their own: Steam's library
/// (which Steam already lists), shared redistributables and installer caches.
const SKIPPED_DIRECTORIES: &[&str] = &[
    "steamapps",
    "_commonredist",
    "commonredist",
    "redist",
    "directx",
    "__installer",
    "$recycle.bin",
    "system volume information",
    "_redist",
    "md5",
    "support",
    "installers",
    "vcredist",
    "dotnetfx",
    "physx",
    "easyanticheat",
    "battleye",
    // Unreal's own tools, beside the game's `Binaries`.
    "engine",
];

/// Folders whose programs are copies of the game's (a crack, an emulator a
/// repack runs it through): used only when the game's own folder has none.
const SECOND_CHOICE_DIRECTORIES: &[&str] = &[
    "nodvd",
    "crack",
    "goldberg",
    "epic files",
    "steam files",
    "emu",
    "emulators",
    "_emulators",
    "backup",
    "_backup",
];

/// Files the operating system leaves in a folder, which are nobody's game and
/// not worth mentioning when nothing else is found.
const SYSTEM_FILES: &[&str] = &["desktop.ini", "thumbs.db", "icon\r"];

// ---------------------------------------------------------------------------
// The record of chosen folders
// ---------------------------------------------------------------------------

/// Every games folder the player chose, in the order they were added.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameFolders {
    #[serde(default)]
    pub folders: Vec<GameFolder>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameFolder {
    pub path: PathBuf,
    /// Names of the items in this folder already turned into a card, or found
    /// to be one the library already had. An item listed here is never added
    /// again, so a game the player removed from the library stays removed.
    #[serde(default)]
    pub seen: BTreeSet<String>,
}

/// The record as the first version wrote it: one folder, nothing else.
#[derive(Deserialize)]
struct LegacyGameFolder {
    path: PathBuf,
}

/// What the interface may know about a folder: an opaque id and its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameFolderView {
    pub id: String,
    pub label: String,
    pub available: bool,
}

impl GameFolders {
    /// Reads the record. A missing or damaged record is an empty one: it must
    /// not stop the app from starting.
    pub fn load(record: &Path) -> Self {
        let Ok(text) = fs::read_to_string(record) else {
            return Self::default();
        };
        if let Ok(folders) = serde_json::from_str::<Self>(&text)
            && !folders.folders.is_empty()
        {
            return folders;
        }
        match serde_json::from_str::<LegacyGameFolder>(&text) {
            Ok(legacy) => Self {
                folders: vec![GameFolder {
                    path: legacy.path,
                    seen: BTreeSet::new(),
                }],
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, record: &Path) -> io::Result<()> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
        let temporary = record.with_extension("json.tmp");
        if let Some(parent) = record.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&temporary, text)?;
        fs::rename(&temporary, record)
    }

    /// Adds `path`, unless it is already one of the folders. Returns whether
    /// it was added.
    pub fn add(&mut self, path: &Path) -> bool {
        if self
            .folders
            .iter()
            .any(|folder| same_path(&folder.path, path))
        {
            return false;
        }
        self.folders.push(GameFolder {
            path: path.to_path_buf(),
            seen: BTreeSet::new(),
        });
        true
    }

    /// Forgets the folder with this view id. Games it already added stay in
    /// the library. Returns whether a folder was removed.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.folders.len();
        self.folders.retain(|folder| folder_id(&folder.path) != id);
        self.folders.len() != before
    }

    pub fn views(&self) -> Vec<GameFolderView> {
        self.folders
            .iter()
            .map(|folder| GameFolderView {
                id: folder_id(&folder.path),
                label: folder_label(&folder.path),
                available: folder.path.is_dir(),
            })
            .collect()
    }

    pub fn folder_mut(&mut self, path: &Path) -> Option<&mut GameFolder> {
        self.folders
            .iter_mut()
            .find(|folder| same_path(&folder.path, path))
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || fs::canonicalize(left)
            .ok()
            .zip(fs::canonicalize(right).ok())
            .is_some_and(|(left, right)| left == right)
}

/// A stable id for a folder that does not carry its path into the interface.
pub fn folder_id(path: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orivo-game-folder-v1\0");
    digest.update(path.to_string_lossy().as_bytes());
    let hash = digest.finalize();
    format!(
        "folder:{}",
        hash.iter()
            .take(12)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

pub fn folder_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// One game found in a games folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundGame {
    /// The item directly inside the games folder this game came from. Its
    /// name is the game's key in the folder's `seen` list.
    pub entry: PathBuf,
    /// What is imported: an executable, an app bundle or a shortcut file.
    pub path: PathBuf,
    /// The card's name when it should not come from `path`: a game found in
    /// its own folder is called after that folder, a link after the link.
    pub title: Option<String>,
}

impl FoundGame {
    pub fn key(&self) -> String {
        entry_key(&self.entry)
    }
}

pub fn entry_key(entry: &Path) -> String {
    entry
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What one folder held.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FolderScan {
    pub games: Vec<FoundGame>,
    /// Items directly inside the folder, hidden and system files aside.
    pub entries: usize,
    /// Extensions of the files that are neither a game nor a shortcut to one
    /// (`"txt"`, or `""` for a file without one), so an empty result can say
    /// what was there instead.
    pub unrecognized: BTreeSet<String>,
    /// The folder could not be listed.
    pub unreadable: bool,
    /// Every name in the folder, games or not, for pruning the `seen` list.
    pub names: BTreeSet<String>,
}

/// The games directly inside `root`, one per item, sorted by item name.
/// `budget` counts directory entries across a whole scan.
pub fn scan_folder(root: &Path, budget: &mut usize) -> FolderScan {
    let mut scan = FolderScan::default();
    let Ok(entries) = sorted_entries(root, true) else {
        scan.unreadable = true;
        return scan;
    };
    for entry in entries {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let path = entry.path();
        let name = entry_key(&path);
        if is_system_file(&name) {
            continue;
        }
        scan.names.insert(name.clone());
        scan.entries += 1;
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        match classify(&path, file_type.is_symlink(), budget) {
            Classified::Game(found) => scan.games.push(found),
            Classified::Ignored => {}
            Classified::Unrecognized(extension) => {
                scan.unrecognized.insert(extension);
            }
        }
    }
    scan
}

enum Classified {
    Game(FoundGame),
    /// Recognised, and deliberately not a game: an installer, an empty folder.
    Ignored,
    Unrecognized(String),
}

fn classify(path: &Path, is_link: bool, budget: &mut usize) -> Classified {
    if is_link {
        // Followed once, for the item it stands for. `metadata` follows the
        // link; a broken one has no target to describe.
        let Ok(metadata) = fs::metadata(path) else {
            return Classified::Unrecognized(extension_of(path));
        };
        let title = Some(shortcut_title(&stem_of(path))).filter(|stem| !stem.is_empty());
        if metadata.is_dir() {
            let Ok(target) = canonical(path) else {
                return Classified::Ignored;
            };
            if is_app_bundle(&target) {
                return Classified::Game(FoundGame {
                    entry: path.to_path_buf(),
                    path: target,
                    title,
                });
            }
            return game_in_directory(path, budget)
                .map_or(Classified::Ignored, |program| folder_game(path, program));
        }
        return match classify_file(path) {
            // `game_for_path` names the card after the link itself.
            FileKind::Game => Classified::Game(FoundGame {
                entry: path.to_path_buf(),
                path: path.to_path_buf(),
                title: None,
            }),
            FileKind::Tool => Classified::Ignored,
            FileKind::Other => Classified::Unrecognized(extension_of(path)),
        };
    }

    if path.is_dir() {
        if is_app_bundle(path) {
            return Classified::Game(FoundGame {
                entry: path.to_path_buf(),
                path: path.to_path_buf(),
                title: None,
            });
        }
        if is_skipped_directory(path) {
            return Classified::Ignored;
        }
        return game_in_directory(path, budget)
            .map_or(Classified::Ignored, |program| folder_game(path, program));
    }

    match classify_file(path) {
        FileKind::Game => Classified::Game(FoundGame {
            entry: path.to_path_buf(),
            path: path.to_path_buf(),
            title: None,
        }),
        FileKind::Tool => Classified::Ignored,
        FileKind::Other => Classified::Unrecognized(extension_of(path)),
    }
}

/// A game found inside its own folder is called after the folder: a binary
/// is often `game.exe` or `Hades-Win64-Shipping.exe`. An app bundle already
/// carries its own display name.
fn folder_game(folder: &Path, program: PathBuf) -> Classified {
    let title = (!is_app_bundle(&program)).then(|| entry_key(folder));
    Classified::Game(FoundGame {
        entry: folder.to_path_buf(),
        path: program,
        title,
    })
}

enum FileKind {
    Game,
    Tool,
    Other,
}

/// A file directly in the games folder, or the file a link there points to.
fn classify_file(path: &Path) -> FileKind {
    let extension = extension_of(path);
    let launchable = match extension.as_str() {
        "exe" | "lnk" | "url" => true,
        "bat" | "cmd" => cfg!(windows),
        _ => shortcut::is_finder_alias(path),
    };
    if !launchable {
        return FileKind::Other;
    }
    if is_tool_name(&stem_of(path)) {
        FileKind::Tool
    } else {
        FileKind::Game
    }
}

/// The game inside a folder of its own: its `.app` on macOS, otherwise its
/// largest eligible executable.
fn game_in_directory(directory: &Path, budget: &mut usize) -> Option<PathBuf> {
    let mut best = Best::default();
    search_directory(directory, EXECUTABLE_LEVELS, false, budget, &mut best);
    if cfg!(target_os = "macos")
        && let Some(app) = best.app
    {
        return Some(app);
    }
    best.executable
        .or(best.second_choice)
        .map(|(_, path)| path)
}

#[derive(Default)]
struct Best {
    /// The first app bundle met, in name order, shallowest first.
    app: Option<PathBuf>,
    /// The largest eligible `.exe`. A game's own binary is almost always its
    /// largest, and the first one seen wins a tie, so the choice is the same on
    /// every scan.
    executable: Option<(u64, PathBuf)>,
    /// The largest second-choice program: a debug build, a bootstrapper, or
    /// a program in a crack or emulator folder.
    second_choice: Option<(u64, PathBuf)>,
}

fn search_directory(
    directory: &Path,
    levels: usize,
    second_choice: bool,
    budget: &mut usize,
    best: &mut Best,
) {
    let Ok(entries) = sorted_entries(directory, false) else {
        return;
    };
    let mut deeper = Vec::new();
    for entry in entries {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            if is_app_bundle(&path) {
                // A bundle is one program, and a Wine wrapper's bundle holds
                // a whole Windows drive: never searched for an `.exe`.
                if best.app.is_none() {
                    best.app = Some(path);
                }
            } else if levels > 1 && !is_skipped_directory(&path) {
                deeper.push(path);
            }
        } else if file_type.is_file() && is_game_executable(&path) {
            let size = entry.metadata().map_or(0, |metadata| metadata.len());
            let slot = if second_choice || is_second_choice_name(&stem_of(&path)) {
                &mut best.second_choice
            } else {
                &mut best.executable
            };
            if slot.as_ref().is_none_or(|(current, _)| size > *current) {
                *slot = Some((size, path));
            }
        }
    }
    for path in deeper {
        let second = second_choice || is_second_choice_directory(&path);
        search_directory(&path, levels - 1, second, budget, best);
    }
}

fn is_second_choice_name(stem: &str) -> bool {
    words_of(stem)
        .iter()
        .any(|word| SECOND_CHOICE_WORDS.contains(&word.as_str()))
}

fn is_second_choice_directory(path: &Path) -> bool {
    let name = entry_key(path).to_lowercase();
    SECOND_CHOICE_DIRECTORIES.contains(&name.as_str())
}

/// Directory entries sorted by name, hidden ones left out. Symbolic links are
/// kept only where `keep_links` says so: in the games folder itself, where a
/// link is an item the player put there, and not below, where it could point
/// anywhere.
fn sorted_entries(directory: &Path, keep_links: bool) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| {
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            let link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
            !hidden && (keep_links || !link)
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn is_game_executable(path: &Path) -> bool {
    extension_of(path) == "exe" && !is_tool_name(&stem_of(path))
}

fn is_app_bundle(path: &Path) -> bool {
    extension_of(path) == "app" && path.is_dir()
}

fn is_system_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SYSTEM_FILES.contains(&lower.as_str())
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Whether a program's name says it is an installer, an uninstaller, a
/// redistributable or a crash reporter rather than a game.
pub fn is_tool_name(stem: &str) -> bool {
    let lower = stem.to_ascii_lowercase();
    if TOOL_FRAGMENTS
        .iter()
        .any(|fragment| lower.contains(fragment))
    {
        return true;
    }
    let words = words_of(stem);
    words
        .iter()
        .any(|word| TOOL_WORDS.contains(&word.as_str()) || word.starts_with("unins"))
        || words
            .windows(2)
            .any(|pair| pair[0] == "crash" && CRASH_TOOL_WORDS.contains(&pair[1].as_str()))
}

/// The words of a file name, lowercased: split on anything that is not a
/// letter or a digit, between a lowercase letter and a capital, after an
/// acronym, and between letters and digits. `UnityCrashHandler64` is
/// `unity crash handler 64`.
fn words_of(stem: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;
    let mut characters = stem.chars().peekable();
    while let Some(character) = characters.next() {
        if !character.is_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            previous = None;
            continue;
        }
        if let Some(previous) = previous {
            // `EOSBootstrapper` is `eos bootstrapper`: an acronym ends before
            // the capital that starts the next word.
            let acronym_break = previous.is_uppercase()
                && character.is_uppercase()
                && characters.peek().is_some_and(|next| next.is_lowercase());
            let case_break =
                (previous.is_lowercase() && character.is_uppercase()) || acronym_break;
            let digit_break = previous.is_ascii_digit() != character.is_ascii_digit();
            if (case_break || digit_break) && !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        }
        current.extend(character.to_lowercase());
        previous = Some(character);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn is_skipped_directory(path: &Path) -> bool {
    let name = entry_key(path).to_lowercase();
    // A Unity game's `<Game>_Data` holds its assets and plugins, never the game.
    SKIPPED_DIRECTORIES.contains(&name.as_str()) || name.ends_with("_data")
}

// ---------------------------------------------------------------------------
// Repairing cards an older scan pointed at the wrong program
// ---------------------------------------------------------------------------

/// What `repair_tool_cards` changed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Repair {
    /// Cards now pointing at the game in their install folder.
    pub repointed: usize,
    /// Cards hidden: their program is a tool and the folder holds no game, or
    /// the game there already has a card of its own.
    pub hidden: usize,
}

impl Repair {
    pub fn changed(&self) -> bool {
        self.repointed + self.hidden > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    Game,
    SecondChoice,
    Tool,
}

/// How a program looks as a game: a tool (checksum checker, crash reporter,
/// a program in a redistributables or data folder), a second choice (debug
/// build, bootstrapper, crack copy), or a game.
fn pick_of(executable: &Path) -> Pick {
    let ancestors = executable
        .ancestors()
        .skip(1)
        .take(EXECUTABLE_LEVELS)
        .collect::<Vec<_>>();
    if is_tool_name(&stem_of(executable)) || ancestors.iter().any(|dir| is_skipped_directory(dir)) {
        return Pick::Tool;
    }
    if is_second_choice_name(&stem_of(executable))
        || ancestors.iter().any(|dir| is_second_choice_directory(dir))
    {
        return Pick::SecondChoice;
    }
    Pick::Game
}

/// The folder a game is installed in, seen from one of its programs: the
/// folder holding the outermost checksum, crack, emulator, data or plugin
/// folder above the program (`Game\_Emulators\Eden\eden.exe` is `Game`), or
/// the program's own folder when there is none.
fn install_root(executable: &Path) -> Option<PathBuf> {
    let parent = executable.parent()?;
    let outermost_inner = parent
        .ancestors()
        .take(EXECUTABLE_LEVELS)
        .filter(|directory| {
            let name = entry_key(directory).to_lowercase();
            is_skipped_directory(directory)
                || is_second_choice_directory(directory)
                || matches!(name.as_str(), "plugins" | "x86" | "x86_64")
        })
        .last();
    Some(
        outermost_inner
            .and_then(Path::parent)
            .unwrap_or(parent)
            .to_path_buf(),
    )
}

/// Cards an older games-folder scan (or a pick by hand) pointed at a program
/// that is not the game — a checksum checker, a Unity plugin, a debug build,
/// a crack copy — are pointed at the game in the same install folder. When
/// there is no game there (a repack's installer folder) or the game already
/// has its card, the card is hidden rather than removed, so nothing is lost.
/// A card keeps its id, play time and artwork; one whose title was only the
/// program's name is called after the install folder.
pub fn repair_tool_cards(catalog: &mut Catalog) -> Repair {
    let mut repair = Repair::default();
    let wine_origins = catalog
        .wine_inventory
        .iter()
        .filter_map(|entry| entry.origin_direct_game_id.clone())
        .collect::<BTreeSet<_>>();
    let mut taken = catalog
        .games
        .iter()
        .filter(|game| game.source == GameSource::Local)
        .filter_map(|game| game.executable_path.clone())
        .collect::<BTreeSet<_>>();
    for game in &mut catalog.games {
        if game.source != GameSource::Local
            || game.hidden
            || !matches!(game.launch_target, LaunchTarget::Direct)
            || wine_origins.contains(&game.id)
        {
            continue;
        }
        let Some(executable) = game.executable_path.clone() else {
            continue;
        };
        if extension_of(&executable) != "exe" {
            continue;
        }
        let pick = pick_of(&executable);
        if pick == Pick::Game {
            continue;
        }
        let Some(root) = install_root(&executable).filter(|root| root.is_dir()) else {
            // A repack's folder deleted after the install: a checksum tool
            // that is not even there any more is nobody's game.
            if pick == Pick::Tool && !executable.exists() {
                game.hidden = true;
                repair.hidden += 1;
            }
            continue;
        };
        let mut budget = MAX_ENTRIES;
        let better = game_in_directory(&root, &mut budget)
            .filter(|candidate| *candidate != executable && pick_of(candidate) == Pick::Game);
        match better {
            Some(candidate) if taken.contains(&candidate) => {
                game.hidden = true;
                repair.hidden += 1;
            }
            Some(candidate) => {
                if game.title == stem_of(&executable) {
                    let title = shortcut_title(&entry_key(&root));
                    if !title.trim().is_empty() {
                        game.title = title;
                    }
                }
                game.working_directory = candidate.parent().map(Path::to_path_buf);
                taken.insert(candidate.clone());
                game.executable_path = Some(candidate);
                repair.repointed += 1;
            }
            None if pick == Pick::Tool => {
                game.hidden = true;
                repair.hidden += 1;
            }
            None => {}
        }
    }
    repair
}

// ---------------------------------------------------------------------------
// From a file to a card
// ---------------------------------------------------------------------------

/// The card for one game file: an executable, an app bundle, or a shortcut
/// to one. Shared by the games folder scan and the manual import, so a
/// shortcut picked by hand behaves exactly like one found in a folder.
pub fn game_for_path(path: &Path) -> Result<Game, String> {
    game_for(path, true)
}

fn game_for(path: &Path, follow: bool) -> Result<Game, String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| not_found(path))?;
    if metadata.file_type().is_symlink() {
        if !follow {
            return Err(unsupported(path));
        }
        let target = canonical(path).map_err(|_| broken(path))?;
        return game_for(&target, false).map(|game| named_after(game, path));
    }

    match extension_of(path).as_str() {
        "url" => {
            shortcut::launcher_url(path).ok_or_else(|| unsupported(path))?;
            let mut game = Game::from_executable(path).map_err(|error| error.to_string())?;
            // The folder a shortcut sits in is not the game's: an image in it
            // belongs to some other card, or to none.
            game.artwork_path = None;
            game.artwork_source_path = None;
            game.working_directory = None;
            game.title = shortcut_title(&game.title);
            Ok(game)
        }
        "lnk" => shell_link_game(path),
        _ if metadata.is_file() && shortcut::is_finder_alias(path) => {
            if !follow {
                return Err(unsupported(path));
            }
            let target = shortcut::resolve_finder_alias(path).ok_or_else(|| broken(path))?;
            game_for(&target, false).map(|game| named_after(game, path))
        }
        _ => Game::from_executable(path).map_err(|error| error.to_string()),
    }
}

/// A `.lnk` whose program is on disk becomes a card for that program, with
/// the link's arguments and folder: Orivo starts it and can time the session.
/// A link Windows alone can follow — to a Store app, or one that asks to run
/// as administrator — is kept as the shortcut itself and opened by the shell.
fn shell_link_game(path: &Path) -> Result<Game, String> {
    let link = shortcut::read_shell_link(path).ok_or_else(|| unsupported(path))?;
    match &link.target {
        Some(target) if target.is_dir() => Err(not_a_game(path)),
        Some(target) if !is_program(target) => Err(not_a_game(path)),
        Some(target) if is_tool_name(&stem_of(target)) => Err(not_a_game(path)),
        Some(target) if !link.run_as_administrator => {
            let mut game = Game::from_executable(target).map_err(|error| error.to_string())?;
            game.arguments = link.arguments.clone();
            if let Some(directory) = &link.working_directory {
                game.working_directory = Some(directory.clone());
            }
            Ok(named_after(game, path))
        }
        Some(_) => shell_opened_game(path),
        None if !link.names_a_path => shell_opened_game(path),
        None => Err(broken(path)),
    }
}

/// A shortcut only the Windows shell can open.
fn shell_opened_game(path: &Path) -> Result<Game, String> {
    if !cfg!(windows) {
        return Err(unsupported(path));
    }
    let mut game = Game::from_executable(path).map_err(|error| error.to_string())?;
    game.artwork_path = None;
    game.artwork_source_path = None;
    game.working_directory = None;
    Ok(game)
}

/// `fs::canonicalize`, without the `\\?\` prefix Windows gives it when the
/// path does not need one: a program started from `\\?\C:\…`, or a batch
/// file run in such a folder, is refused by `cmd` and by many games.
fn canonical(path: &Path) -> io::Result<PathBuf> {
    let resolved = fs::canonicalize(path)?;
    if cfg!(windows)
        && let Some(plain) = resolved.to_str().and_then(without_verbatim_prefix)
    {
        return Ok(PathBuf::from(plain));
    }
    Ok(resolved)
}

/// `\\?\C:\Games` is `C:\Games`, and `\\?\UNC\server\share` is
/// `\\server\share`. Anything else keeps its prefix.
fn without_verbatim_prefix(path: &str) -> Option<String> {
    let rest = path.strip_prefix(r"\\?\")?;
    if let Some(share) = rest.strip_prefix(r"UNC\") {
        return Some(format!(r"\\{share}"));
    }
    let bytes = rest.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
        .then(|| rest.to_string())
}

fn is_program(path: &Path) -> bool {
    match extension_of(path).as_str() {
        "exe" | "bat" | "cmd" | "com" => path.is_file(),
        "app" => path.is_dir(),
        _ => false,
    }
}

/// The card keeps the shortcut's identity and name: the player named the
/// shortcut, and the program behind it may move or be renamed by an update.
fn named_after(mut game: Game, entry: &Path) -> Game {
    game.id = local_game_id(entry);
    let title = shortcut_title(&stem_of(entry));
    if !title.trim().is_empty() {
        game.title = title;
    }
    game
}

/// What Windows adds to a shortcut's name when it makes one: the localised
/// "Shortcut" suffix, after the program's own file name.
const SHORTCUT_SUFFIXES: &[&str] = &[
    " - shortcut",
    " - raccourci",
    " - verknüpfung",
    " - acceso directo",
    " - collegamento",
    " - atalho",
    " - snelkoppeling",
    " - skrót",
];

/// A shortcut's name as a game title: `Control.exe - Raccourci` and
/// `Control.exe` are both "Control".
pub fn shortcut_title(stem: &str) -> String {
    let mut title = stem.trim();
    for suffix in SHORTCUT_SUFFIXES {
        if title.len() > suffix.len()
            && let Some(head) = title.get(..title.len() - suffix.len())
            && title[head.len()..].to_lowercase() == *suffix
        {
            title = head.trim_end();
            break;
        }
    }
    for extension in [".exe", ".bat", ".cmd", ".app", ".lnk", ".url"] {
        if title.len() > extension.len()
            && let Some(head) = title.get(..title.len() - extension.len())
            && title[head.len()..].eq_ignore_ascii_case(extension)
        {
            title = head.trim_end();
            break;
        }
    }
    title.to_string()
}

fn not_found(path: &Path) -> String {
    format!("{} could not be found.", file_label(path))
}

fn broken(path: &Path) -> String {
    format!(
        "{} points to something that is no longer there.",
        file_label(path)
    )
}

fn unsupported(path: &Path) -> String {
    format!(
        "{} is not a game Orivo can start on this device.",
        file_label(path)
    )
}

fn not_a_game(path: &Path) -> String {
    format!("{} does not start a game.", file_label(path))
}

fn file_label(path: &Path) -> String {
    let name = entry_key(path);
    if name.is_empty() {
        "This file".into()
    } else {
        format!("“{name}”")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system temp dir. Every test gets its own,
    /// so they can run in parallel.
    fn scratch(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let root = std::env::temp_dir().join(format!(
            "orivo-game-folder-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn touch(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0_u8; bytes]).unwrap();
    }

    fn app_bundle(path: &Path, executable: &str) {
        let contents = path.join("Contents");
        touch(&contents.join("MacOS").join(executable), 10);
        fs::write(
            contents.join("Info.plist"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>\
                 <key>CFBundleExecutable</key><string>{executable}</string></dict></plist>"
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                contents.join("MacOS").join(executable),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
    }

    fn discover(root: &Path) -> Vec<PathBuf> {
        let mut budget = MAX_ENTRIES;
        scan_folder(root, &mut budget)
            .games
            .into_iter()
            .map(|game| game.path)
            .collect()
    }

    #[test]
    fn a_folder_yields_its_largest_game_executable_and_skips_setup_programs() {
        let root = scratch("largest");
        touch(&root.join("Hades/Hades.exe"), 10);
        touch(&root.join("Hades/bin/Hades64.exe"), 500);
        touch(&root.join("Hades/UnityCrashHandler64.exe"), 900);
        touch(&root.join("Hades/Unins000.exe"), 900);
        touch(
            &root.join("Hades/_CommonRedist/vcredist/vc_redist.exe"),
            900,
        );

        let mut budget = MAX_ENTRIES;
        let scan = scan_folder(&root, &mut budget);
        assert_eq!(scan.games.len(), 1);
        assert_eq!(scan.games[0].path, root.join("Hades/bin/Hades64.exe"));
        // Called after its folder, not after `Hades64`.
        assert_eq!(scan.games[0].title.as_deref(), Some("Hades"));
        assert_eq!(scan.games[0].key(), "Hades");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_executable_directly_in_the_folder_is_a_game_and_setup_is_not() {
        let root = scratch("loose");
        touch(&root.join("Celeste.exe"), 10);
        touch(&root.join("setup_celeste.exe"), 10);
        touch(&root.join("notes.txt"), 10);
        touch(&root.join("desktop.ini"), 10);

        let mut budget = MAX_ENTRIES;
        let scan = scan_folder(&root, &mut budget);
        assert_eq!(
            scan.games.iter().map(|game| &game.path).collect::<Vec<_>>(),
            vec![&root.join("Celeste.exe")]
        );
        assert_eq!(scan.unrecognized, BTreeSet::from(["txt".to_string()]));
        // `desktop.ini` is the system's, not an item the player put there.
        assert_eq!(scan.entries, 3);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn games_whose_names_contain_a_tool_word_stay_games() {
        for game in [
            "CrashBandicootNSaneTrilogy",
            "Reinstalled",
            "The Installation",
            "Hades",
            "Uninvited",
        ] {
            assert!(!is_tool_name(game), "{game} was taken for a tool");
        }
        for tool in [
            "unins000",
            "Uninstall Hades",
            "setup",
            "setup_celeste",
            "HadesSetup",
            "UnityCrashHandler64",
            "CrashReportClient",
            "crashpad_handler",
            "vc_redist.x64",
            "DXSETUP",
            "UE4PrereqSetup_x64",
            "dotNetFx40_Full_x86_x64",
        ] {
            assert!(is_tool_name(tool), "{tool} was taken for a game");
        }
    }

    #[test]
    fn a_resolved_windows_path_loses_its_verbatim_prefix() {
        assert_eq!(
            without_verbatim_prefix(r"\\?\C:\Games\Hades\Hades.exe").as_deref(),
            Some(r"C:\Games\Hades\Hades.exe")
        );
        assert_eq!(
            without_verbatim_prefix(r"\\?\UNC\nas\games\Hades.exe").as_deref(),
            Some(r"\\nas\games\Hades.exe")
        );
        assert_eq!(without_verbatim_prefix(r"C:\Games"), None);
        assert_eq!(without_verbatim_prefix(r"\\?\Volume{1234}\Games"), None);
    }

    #[test]
    fn a_shortcut_loses_what_windows_added_to_its_name() {
        assert_eq!(shortcut_title("ACBlackFlag.exe"), "ACBlackFlag");
        assert_eq!(shortcut_title("Control.exe - Raccourci"), "Control");
        assert_eq!(shortcut_title("Hades - Shortcut"), "Hades");
        assert_eq!(shortcut_title("Elden Ring"), "Elden Ring");
        assert_eq!(shortcut_title(".exe"), ".exe");
        assert_eq!(shortcut_title("Grand Theft Auto V"), "Grand Theft Auto V");
    }

    #[test]
    fn checksum_tools_plugins_and_engine_helpers_are_never_the_game() {
        let root = scratch("not-games");
        // The checker is the largest file a repack's folder holds.
        touch(&root.join("A Way Out/_Redist/QuickSFV.EXE"), 900);
        touch(&root.join("A Way Out/Haze1/Binaries/Win64/AWayOut.exe"), 500);
        touch(&root.join("Clair Obscur/RapidCRC.exe"), 900);
        touch(&root.join("Clair Obscur/Sandfall/Binaries/Win64/SandFall-Win64-Shipping.exe"), 800);
        touch(&root.join("Clair Obscur/Engine/Binaries/Win64/CrashReportClient.exe"), 950);
        touch(&root.join("Reentry/ReEntry_Data/Plugins/ZFGameBrowser.exe"), 900);
        touch(&root.join("Reentry/ReEntry.exe"), 100);
        // A repack's installer folder holds no game at all.
        touch(&root.join("Control [FitGirl Repack]/setup.exe"), 900);
        touch(&root.join("Control [FitGirl Repack]/MD5/QuickSFV.EXE"), 900);

        assert_eq!(
            discover(&root),
            vec![
                root.join("A Way Out/Haze1/Binaries/Win64/AWayOut.exe"),
                root.join("Clair Obscur/Sandfall/Binaries/Win64/SandFall-Win64-Shipping.exe"),
                root.join("Reentry/ReEntry.exe"),
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_debug_build_a_bootstrapper_or_a_crack_copy_is_only_a_last_resort() {
        let root = scratch("second-choice");
        touch(&root.join("Surviving Mars/MarsDebug.exe"), 900);
        touch(&root.join("Surviving Mars/MarsSteam.exe"), 500);
        touch(&root.join("Biped 2/EOSBootstrapper.exe"), 900);
        touch(&root.join("Biped 2/Biped2.exe"), 500);
        touch(&root.join("Ghost/NoDVD/Epic Files/GhostOfTsushima.exe"), 900);
        touch(&root.join("Ghost/GhostOfTsushima.exe"), 500);
        // Alone, a second choice is still found.
        touch(&root.join("Galaxy/emu/Ryujinx.exe"), 500);
        // A Switch repack: five emulators, and the repack's launcher.
        touch(&root.join("Mario/sudachi.exe"), 900);
        touch(&root.join("Mario/emu/Ryujinx.exe"), 900);
        touch(&root.join("Mario/_Emulators/Eden/eden.exe"), 950);
        touch(&root.join("Mario/Launcher.exe"), 100);

        assert_eq!(
            discover(&root),
            vec![
                root.join("Biped 2/Biped2.exe"),
                root.join("Galaxy/emu/Ryujinx.exe"),
                root.join("Ghost/GhostOfTsushima.exe"),
                root.join("Mario/Launcher.exe"),
                root.join("Surviving Mars/MarsSteam.exe"),
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cards_pointing_at_a_tool_are_pointed_at_the_game_or_hidden() {
        let root = scratch("repair");
        touch(&root.join("Clair Obscur/RapidCRC.exe"), 900);
        touch(&root.join("Clair Obscur/Sandfall/Binaries/Win64/SandFall-Win64-Shipping.exe"), 800);
        touch(&root.join("Reentry/ReEntry_Data/Plugins/ZFGameBrowser.exe"), 900);
        touch(&root.join("Reentry/ReEntry.exe"), 100);
        touch(&root.join("Control [FitGirl Repack]/setup.exe"), 900);
        touch(&root.join("Control [FitGirl Repack]/MD5/QuickSFV.EXE"), 900);
        touch(&root.join("Ghost/NoDVD/Epic Files/GhostOfTsushima.exe"), 900);
        touch(&root.join("Ghost/GhostOfTsushima.exe"), 500);
        touch(&root.join("Hades/Hades.exe"), 500);
        touch(&root.join("Galaxy/emu/Ryujinx.exe"), 500);
        touch(&root.join("Mario/_Emulators/Eden/eden.exe"), 950);
        touch(&root.join("Mario/Launcher.exe"), 100);
        touch(&root.join("Gone [FitGirl Repack]/MD5/QuickSFV.EXE"), 10);

        let card = |path: &str| {
            let mut game = Game::from_executable(root.join(path)).unwrap();
            game.id = format!("local:{path}");
            game
        };
        let mut catalog = Catalog::default();
        for path in [
            "Clair Obscur/RapidCRC.exe",
            "Reentry/ReEntry_Data/Plugins/ZFGameBrowser.exe",
            "Control [FitGirl Repack]/MD5/QuickSFV.EXE",
            "Ghost/GhostOfTsushima.exe",
            "Ghost/NoDVD/Epic Files/GhostOfTsushima.exe",
            "Hades/Hades.exe",
            "Galaxy/emu/Ryujinx.exe",
            "Mario/_Emulators/Eden/eden.exe",
            "Gone [FitGirl Repack]/MD5/QuickSFV.EXE",
        ] {
            catalog.games.push(card(path));
        }
        catalog.games[1].play_time_seconds = 42;
        // The repack folder is deleted once the game is installed.
        fs::remove_dir_all(root.join("Gone [FitGirl Repack]")).unwrap();

        let repair = repair_tool_cards(&mut catalog);
        assert_eq!(repair, Repair { repointed: 3, hidden: 3 });
        let state = catalog
            .games
            .iter()
            .map(|game| {
                (
                    game.title.as_str(),
                    game.executable_path
                        .as_deref()
                        .unwrap()
                        .strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    game.hidden,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            state,
            vec![
                ("Clair Obscur", "Clair Obscur/Sandfall/Binaries/Win64/SandFall-Win64-Shipping.exe".into(), false),
                ("Reentry", "Reentry/ReEntry.exe".into(), false),
                // An installer folder: no game to point at.
                ("QuickSFV", "Control [FitGirl Repack]/MD5/QuickSFV.EXE".into(), true),
                ("GhostOfTsushima", "Ghost/GhostOfTsushima.exe".into(), false),
                // The crack copy of a game that already has its card.
                ("GhostOfTsushima", "Ghost/NoDVD/Epic Files/GhostOfTsushima.exe".into(), true),
                ("Hades", "Hades/Hades.exe".into(), false),
                // Nothing better beside it: left alone.
                ("Ryujinx", "Galaxy/emu/Ryujinx.exe".into(), false),
                // An emulator of a repack that has its own launcher.
                ("Mario", "Mario/Launcher.exe".into(), false),
                ("QuickSFV", "Gone [FitGirl Repack]/MD5/QuickSFV.EXE".into(), true),
            ]
        );
        // The card keeps what it had.
        assert_eq!(catalog.games[1].id, "local:Reentry/ReEntry_Data/Plugins/ZFGameBrowser.exe");
        assert_eq!(catalog.games[1].play_time_seconds, 42);
        assert_eq!(catalog.games[1].working_directory, Some(root.join("Reentry")));
        // Idempotent.
        assert_eq!(repair_tool_cards(&mut catalog), Repair::default());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_game_named_crash_something_is_found() {
        let root = scratch("crash");
        touch(&root.join("CrashBandicootNSaneTrilogy.exe"), 10);
        touch(&root.join("Crash Team Racing/CTR.exe"), 10);
        assert_eq!(
            discover(&root),
            vec![
                root.join("Crash Team Racing/CTR.exe"),
                root.join("CrashBandicootNSaneTrilogy.exe"),
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_executable_deeper_than_the_levels_allowed_is_not_found() {
        let root = scratch("depth");
        touch(&root.join("Deep/a/b/c/d/Deep.exe"), 10);
        // An Unreal game: the binary is three folders below the game's own.
        touch(&root.join("Unreal/Game/Binaries/Win64/Game-Win64-Shipping.exe"), 10);

        assert_eq!(
            discover(&root),
            vec![root.join("Unreal/Game/Binaries/Win64/Game-Win64-Shipping.exe")]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn steam_libraries_and_hidden_folders_are_left_to_their_owners() {
        let root = scratch("skipped");
        touch(&root.join("steamapps/common/Game/Game.exe"), 10);
        touch(&root.join(".cache/Hidden.exe"), 10);

        assert!(discover(&root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shortcuts_in_the_folder_are_games() {
        let root = scratch("shortcuts");
        fs::write(
            root.join("Hades.lnk"),
            shortcut::tests::shell_link_bytes(r"C:\Games\Hades\Hades.exe", None, None, false),
        )
        .unwrap();
        fs::write(
            root.join("Elden Ring.url"),
            "[InternetShortcut]\r\nURL=steam://rungameid/1245620\r\n",
        )
        .unwrap();
        fs::write(root.join("Uninstall Hades.lnk"), b"").unwrap();

        assert_eq!(
            discover(&root),
            vec![root.join("Elden Ring.url"), root.join("Hades.lnk")]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_launcher_url_shortcut_becomes_a_card_named_after_it_and_a_web_link_does_not() {
        let root = scratch("url");
        let steam = root.join("Elden Ring.url");
        fs::write(
            &steam,
            "[InternetShortcut]\r\nURL=steam://rungameid/1245620\r\n",
        )
        .unwrap();
        touch(&root.join("cover.png"), 10);
        let game = game_for_path(&steam).unwrap();
        assert_eq!(game.title, "Elden Ring");
        assert_eq!(game.executable_path.as_deref(), Some(steam.as_path()));
        assert_eq!(game.id, local_game_id(&steam));
        // The folder's image is not this game's.
        assert_eq!(game.artwork_path, None);
        game.validate().unwrap();

        let web = root.join("Wiki.url");
        fs::write(&web, "[InternetShortcut]\nURL=https://example.com\n").unwrap();
        assert!(game_for_path(&web).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_shell_link_to_a_program_becomes_a_card_for_that_program_with_its_arguments() {
        let root = scratch("lnk");
        let program = root.join("Games/Hades/Hades.exe");
        touch(&program, 10);
        let link = root.join("Shortcuts/My Hades.lnk");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        fs::write(
            &link,
            shortcut::tests::shell_link_bytes(
                program.to_str().unwrap(),
                Some(root.join("Games").to_str().unwrap()),
                Some("-dx12 --skip-intro"),
                false,
            ),
        )
        .unwrap();

        let game = game_for_path(&link).unwrap();
        assert_eq!(game.title, "My Hades");
        assert_eq!(game.id, local_game_id(&link));
        assert_eq!(game.executable_path.as_deref(), Some(program.as_path()));
        assert_eq!(game.arguments, vec!["-dx12", "--skip-intro"]);
        assert_eq!(game.working_directory, Some(root.join("Games")));
        game.validate().unwrap();

        // A link to a folder or a document is not a game.
        let manual = root.join("Shortcuts/Manual.lnk");
        touch(&root.join("Games/Hades/Manual.pdf"), 10);
        fs::write(
            &manual,
            shortcut::tests::shell_link_bytes(
                root.join("Games/Hades/Manual.pdf").to_str().unwrap(),
                None,
                None,
                false,
            ),
        )
        .unwrap();
        assert!(game_for_path(&manual).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_app_bundle_is_a_native_game_directly_or_inside_its_folder() {
        let root = scratch("app");
        app_bundle(&root.join("Celeste.app"), "Celeste");
        app_bundle(
            &root.join("Hollow Knight/Hollow Knight.app"),
            "hollow_knight",
        );
        touch(&root.join("Hollow Knight/Windows/hollow_knight.exe"), 900);

        let mut budget = MAX_ENTRIES;
        let scan = scan_folder(&root, &mut budget);
        assert_eq!(scan.games[0].path, root.join("Celeste.app"));
        let hollow = &scan.games[1];
        if cfg!(target_os = "macos") {
            assert_eq!(hollow.path, root.join("Hollow Knight/Hollow Knight.app"));
        } else {
            assert_eq!(
                hollow.path,
                root.join("Hollow Knight/Windows/hollow_knight.exe")
            );
        }

        let game = game_for_path(&root.join("Celeste.app")).unwrap();
        assert_eq!(
            game.executable_path,
            Some(root.join("Celeste.app/Contents/MacOS/Celeste"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_in_the_folder_is_followed_once() {
        let root = scratch("links");
        let elsewhere = scratch("links-target");
        touch(&elsewhere.join("Celeste/Celeste.exe"), 10);
        touch(&elsewhere.join("Loose.exe"), 10);
        app_bundle(&elsewhere.join("Native.app"), "Native");
        touch(&elsewhere.join("Inner/Inner.exe"), 10);
        let folder = root.join("Games");
        fs::create_dir_all(&folder).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("Celeste"), folder.join("Celeste")).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("Loose.exe"), folder.join("My Game.exe"))
            .unwrap();
        std::os::unix::fs::symlink(elsewhere.join("Native.app"), folder.join("Native")).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("gone.exe"), folder.join("Broken.exe")).unwrap();
        // A link below a game's folder is never followed.
        fs::create_dir_all(folder.join("Linked")).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("Inner"), folder.join("Linked/Inner")).unwrap();

        let mut budget = MAX_ENTRIES;
        let scan = scan_folder(&folder, &mut budget);
        let found = scan
            .games
            .iter()
            .map(|game| (game.key(), game.path.clone(), game.title.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            found,
            vec![
                (
                    "Celeste".to_string(),
                    folder.join("Celeste/Celeste.exe"),
                    Some("Celeste".to_string())
                ),
                (
                    "My Game.exe".to_string(),
                    folder.join("My Game.exe"),
                    None
                ),
                (
                    "Native".to_string(),
                    fs::canonicalize(elsewhere.join("Native.app")).unwrap(),
                    Some("Native".to_string())
                ),
            ]
        );
        assert!(
            scan.unrecognized.contains("exe"),
            "the broken link is reported"
        );

        let game = game_for_path(&folder.join("My Game.exe")).unwrap();
        assert_eq!(game.title, "My Game");
        assert_eq!(game.id, local_game_id(&folder.join("My Game.exe")));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(elsewhere).unwrap();
    }

    /// Writes a `.lnk` the way Explorer does, through `WScript.Shell`, so the
    /// parser is checked against Windows' own output and not only against
    /// bytes this crate built.
    #[cfg(windows)]
    fn windows_shortcut(link: &Path, target: &Path, arguments: &str, directory: &Path) {
        let quote = |value: &str| value.replace('\'', "''");
        let script = format!(
            "$shell = New-Object -ComObject WScript.Shell; \
             $link = $shell.CreateShortcut('{}'); \
             $link.TargetPath = '{}'; $link.Arguments = '{}'; \
             $link.WorkingDirectory = '{}'; $link.Save()",
            quote(&link.to_string_lossy()),
            quote(&target.to_string_lossy()),
            quote(arguments),
            quote(&directory.to_string_lossy()),
        );
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .unwrap();
        assert!(status.success(), "PowerShell could not write {}", link.display());
    }

    #[cfg(windows)]
    #[test]
    fn shortcuts_windows_itself_wrote_are_imported_with_their_arguments() {
        // `%TEMP%` can be an 8.3 short path (`C:\Users\RUNNER~1\…` on CI),
        // while Windows saves a shortcut's target in its long form: compare
        // against the long one.
        let root = canonical(&scratch("windows-shortcuts")).unwrap();
        let game_directory = root.join("Jeux").join("Héros Ünïcode");
        let program = game_directory.join("Héros.exe");
        touch(&program, 10);
        let uninstaller = game_directory.join("unins000.exe");
        touch(&uninstaller, 10);
        let folder = root.join("Raccourcis");
        fs::create_dir_all(&folder).unwrap();

        windows_shortcut(
            &folder.join("Mon Héros.lnk"),
            &program,
            r#"-windowed "--name=Player One""#,
            &game_directory,
        );
        // A shortcut to a folder and one to an uninstaller are not games.
        windows_shortcut(&folder.join("Saves.lnk"), &game_directory, "", &game_directory);
        windows_shortcut(&folder.join("Remove it.lnk"), &uninstaller, "", &game_directory);
        fs::write(
            folder.join("Elden Ring.url"),
            "[InternetShortcut]\r\nURL=steam://rungameid/1245620\r\n",
        )
        .unwrap();
        fs::write(folder.join("desktop.ini"), "[.ShellClassInfo]\r\n").unwrap();

        let mut budget = MAX_ENTRIES;
        let scan = scan_folder(&folder, &mut budget);
        let mut games = Vec::new();
        let mut refused = Vec::new();
        for found in &scan.games {
            match game_for_path(&found.path) {
                Ok(game) => games.push(game),
                Err(_) => refused.push(found.key()),
            }
        }
        assert_eq!(
            games.iter().map(|game| game.title.as_str()).collect::<Vec<_>>(),
            vec!["Elden Ring", "Mon Héros"]
        );
        assert_eq!(refused, vec!["Remove it.lnk", "Saves.lnk"]);

        let hero = &games[1];
        assert_eq!(hero.executable_path.as_deref(), Some(program.as_path()));
        assert_eq!(hero.arguments, vec!["-windowed", "--name=Player One"]);
        assert_eq!(hero.working_directory.as_deref(), Some(game_directory.as_path()));
        assert_eq!(hero.id, local_game_id(&folder.join("Mon Héros.lnk")));
        hero.validate().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_record_keeps_several_folders_and_reads_the_old_single_folder_format() {
        let root = scratch("record");
        let record = root.join("game-folder.json");
        let first = root.join("Games");
        let second = root.join("Shortcuts");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();

        fs::write(
            &record,
            serde_json::to_string(&serde_json::json!({ "path": first })).unwrap(),
        )
        .unwrap();
        let mut folders = GameFolders::load(&record);
        assert_eq!(folders.folders.len(), 1);
        assert_eq!(folders.folders[0].path, first);

        assert!(folders.add(&second));
        assert!(!folders.add(&first), "the same folder is not added twice");
        folders.folders[0].seen.insert("Hades".into());
        folders.save(&record).unwrap();

        let reloaded = GameFolders::load(&record);
        assert_eq!(reloaded, folders);
        let views = reloaded.views();
        assert_eq!(views.len(), 2);
        assert_eq!(views[1].label, "Shortcuts");
        assert!(views.iter().all(|view| view.available));
        assert!(!views[0].id.contains("Games"), "the id carries no path");

        let mut removed = reloaded.clone();
        assert!(removed.remove(&views[0].id));
        assert_eq!(removed.folders.len(), 1);
        assert_eq!(removed.folders[0].path, second);

        fs::write(&record, "{ not json").unwrap();
        assert_eq!(GameFolders::load(&record), GameFolders::default());
        fs::remove_dir_all(root).unwrap();
    }
}
