//! The games folder: one directory the player chooses once, and every game in
//! it joins the library without another trip through the file dialog.
//!
//! A game is either an `.exe` directly inside the folder, or a folder directly
//! inside it. For a folder, the game is the largest eligible `.exe` found up to
//! three directory levels down: setup programs, redistributables and crash
//! handlers are never the game, and the folders installers and Steam keep
//! their own files in are skipped. The scan reads names, sizes and listings
//! only. It never opens or runs an executable.

use std::{
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Directory listings walked for one scan, across every game folder.
const MAX_ENTRIES: usize = 20_000;
/// How many directory levels below a game folder can hold its executable.
const EXECUTABLE_LEVELS: usize = 3;

/// Fragments of an executable's name that mark it as something other than the
/// game: installers, uninstallers, redistributable prerequisites, crash reporters.
const SKIPPED_EXECUTABLE_FRAGMENTS: &[&str] = &[
    "unins",
    "setup",
    "install",
    "redist",
    "dxsetup",
    "dotnet",
    "crash",
    "prereq",
    "vcredist",
    "vc_redist",
];

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
];

#[derive(Serialize, Deserialize)]
struct StoredGameFolder {
    path: PathBuf,
}

/// The folder the player chose, or `None` when there is no record of one or the
/// record cannot be read. A damaged record must not stop the app from starting.
pub fn stored_folder(record: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(record).ok()?;
    let stored: StoredGameFolder = serde_json::from_str(&text).ok()?;
    stored.path.is_dir().then_some(stored.path)
}

pub fn store_folder(record: &Path, folder: &Path) -> io::Result<()> {
    let text = serde_json::to_string(&StoredGameFolder {
        path: folder.to_path_buf(),
    })
    .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
    fs::write(record, text)
}

/// The game executables under `root`, one per game, sorted by path.
pub fn discover_games(root: &Path) -> Vec<PathBuf> {
    let mut budget = MAX_ENTRIES;
    let Ok(entries) = sorted_entries(root) else {
        return Vec::new();
    };
    let mut games = Vec::new();
    for entry in entries {
        if budget == 0 {
            break;
        }
        budget -= 1;
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_file() {
            if is_game_executable(&path) {
                games.push(path);
            }
        } else if file_type.is_dir() && !is_skipped_directory(&path) {
            let mut best = None;
            largest_executable(&path, EXECUTABLE_LEVELS, &mut budget, &mut best);
            if let Some((_, executable)) = best {
                games.push(executable);
            }
        }
    }
    games.sort();
    games
}

/// Keeps the largest eligible executable under `directory`. A game's own
/// binary is almost always its largest `.exe`, and the first one seen wins a
/// tie, so the choice is the same on every scan.
fn largest_executable(
    directory: &Path,
    levels: usize,
    budget: &mut usize,
    best: &mut Option<(u64, PathBuf)>,
) {
    let Ok(entries) = sorted_entries(directory) else {
        return;
    };
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
            if levels > 1 && !is_skipped_directory(&path) {
                largest_executable(&path, levels - 1, budget, best);
            }
        } else if file_type.is_file() && is_game_executable(&path) {
            let size = entry.metadata().map_or(0, |metadata| metadata.len());
            if best.as_ref().is_none_or(|(current, _)| size > *current) {
                *best = Some((size, path));
            }
        }
    }
}

/// Directory entries sorted by name, with hidden entries and symbolic links
/// left out. A link can point anywhere, so it is never followed into a game.
fn sorted_entries(directory: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| {
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            let link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
            !hidden && !link
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn is_game_executable(path: &Path) -> bool {
    let is_exe = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
    if !is_exe {
        return false;
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    !SKIPPED_EXECUTABLE_FRAGMENTS
        .iter()
        .any(|fragment| stem.contains(fragment))
}

fn is_skipped_directory(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    SKIPPED_DIRECTORIES.contains(&name.as_str())
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

        assert_eq!(
            discover_games(&root),
            vec![root.join("Hades/bin/Hades64.exe")]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_executable_directly_in_the_folder_is_a_game_and_setup_is_not() {
        let root = scratch("loose");
        touch(&root.join("Celeste.exe"), 10);
        touch(&root.join("setup_celeste.exe"), 10);
        touch(&root.join("notes.txt"), 10);

        assert_eq!(discover_games(&root), vec![root.join("Celeste.exe")]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_executable_deeper_than_the_levels_allowed_is_not_found() {
        let root = scratch("depth");
        touch(&root.join("Deep/a/b/c/Deep.exe"), 10);
        touch(&root.join("Shallow/a/b/Shallow.exe"), 10);

        assert_eq!(
            discover_games(&root),
            vec![root.join("Shallow/a/b/Shallow.exe")]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn steam_libraries_and_hidden_folders_are_left_to_their_owners() {
        let root = scratch("skipped");
        touch(&root.join("steamapps/common/Game/Game.exe"), 10);
        touch(&root.join(".cache/Hidden.exe"), 10);

        assert!(discover_games(&root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_stored_folder_is_read_back_only_while_it_still_exists() {
        let root = scratch("record");
        let record = root.join("game-folder.json");
        let games = root.join("Games");
        fs::create_dir_all(&games).unwrap();

        store_folder(&record, &games).unwrap();
        assert_eq!(stored_folder(&record), Some(games.clone()));

        fs::remove_dir_all(&games).unwrap();
        assert_eq!(stored_folder(&record), None);
        fs::remove_dir_all(root).unwrap();
    }
}
