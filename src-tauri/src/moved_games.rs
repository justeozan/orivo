//! Games that moved.
//!
//! A local card remembers the program it starts by its path. When the player
//! moves the game — into another games folder, a sub-folder, another disk —
//! that path stops answering, and Play used to end on "executable does not
//! exist". A card whose program is gone is looked for before that can happen:
//!
//! - in the games folders, where a game is usually moved to;
//! - around where it was: the nearest folder above it that is still there;
//! - on a Mac, through Spotlight, which knows where any indexed file went.
//!
//! A candidate carries the program's own name (or its app bundle's), and it is
//! taken only when it is the one best match. A name that says nothing about
//! the game (`Game.exe`, `Launcher.exe`) must also sit in a folder that kept
//! the old folder's name or the game's title, so one game's card is never
//! pointed at another game. The card keeps its id, its play time and its
//! artwork. A card that cannot be found stays in the library, shown as not
//! installed, until it comes back or the player locates it by hand.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
};

use crate::catalog::{Catalog, Game, GameSource, LaunchTarget, WINE_STAGING_RUNNER_ID};

/// The mark a card carries while its game is nowhere to be found. Set and
/// cleared by the background pass, read by the library: projecting a card
/// never touches the disk, where one offline network drive can hang for
/// seconds.
pub const MISSING_KEY: &str = "programMissing";

/// Directory entries one relocation pass may read, across every place it looks.
pub const MAX_ENTRIES: usize = 20_000;
/// How deep below a place to look: a category folder, the game's folder, and
/// the four levels a game's program can sit below it.
const SEARCH_DEPTH: usize = 6;
/// Spotlight questions one pass may ask. A disk that is not connected can
/// leave dozens of cards missing at once; each keeps its mark until the next pass.
const MAX_TRACES: usize = 8;

/// Folders a moved game is never in: the system's own, and the trash. A game
/// in the trash was deleted, not moved.
const SKIPPED_DIRECTORIES: &[&str] = &["$recycle.bin", "system volume information", "node_modules"];

/// Whether this local card's program is no longer where the card says. Only
/// a card Orivo starts itself has a program to lose: a store's game is the
/// store's to find. This reads the disk; the library reads `marked_missing`.
pub fn is_missing(game: &Game) -> bool {
    program_of(game).is_some_and(|program| !program.exists())
}

/// Whether the last pass found this card's game gone.
pub fn marked_missing(game: &Game) -> bool {
    game.extra
        .get(MISSING_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn mark(game: &mut Game, missing: bool) {
    if missing {
        game.extra
            .insert(MISSING_KEY.into(), serde_json::Value::Bool(true));
    } else {
        game.extra.remove(MISSING_KEY);
    }
}

/// The program a local card starts, when it has one Orivo looks after.
fn program_of(game: &Game) -> Option<&Path> {
    if game.source != GameSource::Local
        || !matches!(game.launch_target, LaunchTarget::Direct)
        || game.id.starts_with("showcase-")
    {
        return None;
    }
    game.executable_path
        .as_deref()
        .filter(|path| !path.as_os_str().is_empty())
}

/// What one pass changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Relocation {
    /// Cards found where their game went, now pointing there: id and title.
    pub relocated: Vec<(String, String)>,
    /// Cards whose game is still nowhere to be found.
    pub missing: usize,
}

/// Point every card whose game moved at where it went, and mark the ones
/// that cannot be found. `places` are the games folders; `trace` names other
/// paths the system knows by the given file name (Spotlight on a Mac). `only`
/// limits the pass to one card.
pub fn relocate(
    catalog: &mut Catalog,
    places: &[PathBuf],
    only: Option<&str>,
    trace: &dyn Fn(&str) -> Vec<PathBuf>,
) -> Relocation {
    let mut relocation = Relocation::default();
    mark_wine_games(catalog, only);
    // A Windows game a Mac runs through Wine keeps its path in the Wine
    // record too, under an id derived from it; moving that one is the Wine
    // setup's business, not a path to rewrite here.
    let wine_origins = catalog
        .wine_inventory
        .iter()
        .filter_map(|entry| entry.origin_direct_game_id.clone())
        .collect::<BTreeSet<_>>();
    let mut lost = Vec::new();
    for (index, game) in catalog.games.iter_mut().enumerate() {
        if only.is_some_and(|id| game.id != id) || program_of(game).is_none() {
            continue;
        }
        if !is_missing(game) {
            mark(game, false);
        } else if game.hidden || wine_origins.contains(&game.id) {
            mark(game, true);
        } else {
            lost.push(index);
        }
    }
    if lost.is_empty() {
        return relocation;
    }

    let mut taken = catalog
        .games
        .iter()
        .filter_map(program_of)
        .map(Path::to_path_buf)
        .collect::<BTreeSet<_>>();
    let wanted = lost
        .iter()
        .filter_map(|&index| program_of(&catalog.games[index]))
        .filter_map(|program| anchor_name(&anchor_of(program).0))
        .collect::<BTreeSet<_>>();

    let mut roots = places
        .iter()
        .filter(|place| place.is_dir())
        .cloned()
        .collect::<Vec<_>>();
    for &index in &lost {
        if let Some(near) = program_of(&catalog.games[index]).and_then(nearest_remaining_folder)
            && !roots.contains(&near)
        {
            roots.push(near);
        }
    }
    let mut budget = MAX_ENTRIES;
    // Places can overlap (a games folder inside the folder a game left), so
    // one item can be met twice: it is still one candidate.
    let mut index = BTreeMap::<String, BTreeSet<PathBuf>>::new();
    for root in &roots {
        walk(root, SEARCH_DEPTH, &wanted, &mut budget, &mut index);
    }

    let mut traces = 0;
    for game_index in lost {
        let game = &catalog.games[game_index];
        let Some(program) = program_of(game).map(Path::to_path_buf) else {
            continue;
        };
        let (anchor, inside) = anchor_of(&program);
        let Some(name) = anchor_name(&anchor) else {
            mark(&mut catalog.games[game_index], true);
            relocation.missing += 1;
            continue;
        };
        let mut candidates = index
            .get(&name)
            .map(|found| found.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut found = best_match(&anchor, &inside, &game.title, &candidates, &taken);
        if found.is_none()
            && traces < MAX_TRACES
            && let Some(file_name) = anchor.file_name().and_then(OsStr::to_str)
        {
            traces += 1;
            for path in trace(file_name) {
                if !candidates.contains(&path) {
                    candidates.push(path);
                }
            }
            found = best_match(&anchor, &inside, &game.title, &candidates, &taken);
        }
        let game = &mut catalog.games[game_index];
        let Some((new_anchor, matched)) = found else {
            mark(game, true);
            relocation.missing += 1;
            continue;
        };
        let new_program = join_inside(&new_anchor, &inside);
        repoint(game, &program, &new_program, &anchor, &new_anchor, matched);
        mark(game, false);
        taken.insert(new_program);
        relocation
            .relocated
            .push((game.id.clone(), game.title.clone()));
    }
    relocation
}

/// A Wine card's program lives in its Wine record. It is not moved here (its
/// reference is derived from the path), but the card says when it is gone.
fn mark_wine_games(catalog: &mut Catalog, only: Option<&str>) {
    let gone = catalog
        .wine_inventory
        .iter()
        .filter(|entry| !entry.executable_path.exists())
        .map(|entry| (entry.profile_id.clone(), entry.game_ref.clone()))
        .collect::<BTreeSet<_>>();
    for game in &mut catalog.games {
        if only.is_some_and(|id| game.id != id) {
            continue;
        }
        if let LaunchTarget::Runner {
            runner_id,
            profile_id,
            game_ref,
        } = &game.launch_target
            && runner_id == WINE_STAGING_RUNNER_ID
        {
            let missing = gone.contains(&(profile_id.clone(), game_ref.clone()));
            mark(game, missing);
        }
    }
}

/// Carry what a pass decided on a snapshot over to the catalog as it is now.
/// A card changed in between (imported again, located by hand) keeps its
/// newer state. Returns whether anything was carried.
pub fn carry_over(current: &mut Catalog, before: &Catalog, after: &Catalog) -> bool {
    let mut changed = false;
    for updated in &after.games {
        let Some(old) = before.games.iter().find(|game| game.id == updated.id) else {
            continue;
        };
        if old.executable_path == updated.executable_path
            && marked_missing(old) == marked_missing(updated)
        {
            continue;
        }
        let Some(game) = current.games.iter_mut().find(|game| game.id == updated.id) else {
            continue;
        };
        if game.executable_path != old.executable_path {
            continue;
        }
        game.executable_path = updated.executable_path.clone();
        game.working_directory = updated.working_directory.clone();
        game.installation_path = updated.installation_path.clone();
        game.artwork_source_path = updated.artwork_source_path.clone();
        game.cover_source_path = updated.cover_source_path.clone();
        mark(game, marked_missing(updated));
        changed = true;
    }
    changed
}

/// Point a card at a program the player chose by hand, or that a shortcut
/// now names. The card keeps its id, title, play time and artwork.
pub fn point_at(game: &mut Game, found: &Game) {
    game.executable_path = found.executable_path.clone();
    game.working_directory = found.working_directory.clone();
    game.arguments = found.arguments.clone();
    game.installation_path = found.installation_path.clone();
    mark(game, false);
}

/// Rewrite a card for its game's new place. Paths inside the folder that
/// moved follow it; the working folder, when it was the program's own, does too.
fn repoint(
    game: &mut Game,
    old_program: &Path,
    new_program: &Path,
    old_anchor: &Path,
    new_anchor: &Path,
    matched: usize,
) {
    let old_root = moved_root(old_anchor, matched);
    let new_root = moved_root(new_anchor, matched);
    let rebase = |path: &Path| -> Option<PathBuf> {
        if path == old_program.parent()? {
            return new_program.parent().map(Path::to_path_buf);
        }
        let rest = path.strip_prefix(&old_root).ok()?;
        Some(new_root.join(rest))
    };
    game.working_directory = match game.working_directory.as_deref() {
        Some(directory) => rebase(directory).or_else(|| Some(directory.to_path_buf())),
        None => None,
    };
    if let Some(installation) = game.installation_path.as_deref().and_then(rebase) {
        game.installation_path = Some(installation);
    }
    for source in [&mut game.artwork_source_path, &mut game.cover_source_path] {
        if let Some(rebased) = source.as_deref().and_then(rebase) {
            *source = Some(rebased);
        }
    }
    game.executable_path = Some(new_program.to_path_buf());
}

/// The item that moves with the game: its app bundle when the program is
/// inside one, the program itself otherwise. The second half is the path
/// inside the bundle.
fn anchor_of(program: &Path) -> (PathBuf, PathBuf) {
    let bundle = program
        .ancestors()
        .filter(|ancestor| has_extension(ancestor, "app"))
        .last();
    match bundle {
        Some(bundle) => (
            bundle.to_path_buf(),
            program
                .strip_prefix(bundle)
                .map(Path::to_path_buf)
                .unwrap_or_default(),
        ),
        None => (program.to_path_buf(), PathBuf::new()),
    }
}

fn join_inside(anchor: &Path, inside: &Path) -> PathBuf {
    if inside.as_os_str().is_empty() {
        anchor.to_path_buf()
    } else {
        anchor.join(inside)
    }
}

/// The folder that moved as one piece: the anchor with as many folders above
/// it as were found unchanged at the new place (`matched` counts the anchor).
fn moved_root(anchor: &Path, matched: usize) -> PathBuf {
    let mut root = anchor.to_path_buf();
    for _ in 1..matched {
        match root.parent() {
            Some(parent) => root = parent.to_path_buf(),
            None => break,
        }
    }
    root
}

fn anchor_name(anchor: &Path) -> Option<String> {
    anchor
        .file_name()
        .and_then(OsStr::to_str)
        .map(str::to_lowercase)
        .filter(|name| !name.is_empty())
}

/// The closest folder above the program that is still there, unless it is a
/// disk's root or the home folder: looking through all of those is a search
/// of the whole machine, and Spotlight already does that better.
fn nearest_remaining_folder(program: &Path) -> Option<PathBuf> {
    let near = program
        .ancestors()
        .skip(1)
        .find(|ancestor| ancestor.is_dir())?;
    let is_root = near.parent().is_none()
        || near
            .components()
            .filter(|component| matches!(component, Component::Normal(_)))
            .count()
            == 0;
    let is_home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .is_some_and(|home| Path::new(&home) == near);
    let is_volumes = near == Path::new("/Volumes") || near == Path::new("/Users");
    (!is_root && !is_home && !is_volumes).then(|| near.to_path_buf())
}

/// Every item named like one of `wanted`, below `directory`. Links are not
/// followed and app bundles are not entered: a game is never inside another.
fn walk(
    directory: &Path,
    depth: usize,
    wanted: &BTreeSet<String>,
    budget: &mut usize,
    found: &mut BTreeMap<String, BTreeSet<PathBuf>>,
) {
    if depth == 0 || *budget == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut folders = Vec::new();
    for entry in entries.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        let lower = name.to_lowercase();
        if name.starts_with('.') {
            continue;
        }
        if wanted.contains(&lower) {
            found.entry(lower.clone()).or_default().insert(path.clone());
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir()
            && !has_extension(&path, "app")
            && !SKIPPED_DIRECTORIES.contains(&lower.as_str())
        {
            folders.push(path);
        }
    }
    folders.sort();
    for folder in folders {
        walk(&folder, depth - 1, wanted, budget, found);
    }
}

/// The one candidate that is this card's game, with how many trailing path
/// components it shares with the old place (the name itself counts as one).
fn best_match(
    old_anchor: &Path,
    inside: &Path,
    title: &str,
    candidates: &[PathBuf],
    taken: &BTreeSet<PathBuf>,
) -> Option<(PathBuf, usize)> {
    let mut best: Option<(PathBuf, usize)> = None;
    let mut tied = false;
    for candidate in candidates {
        if candidate == old_anchor || is_in_trash(candidate) {
            continue;
        }
        let program = join_inside(candidate, inside);
        if !program.is_file() || taken.contains(&program) {
            continue;
        }
        let matched = shared_tail(old_anchor, candidate);
        if matched == 0 || (matched == 1 && !names_the_game(candidate, title)) {
            continue;
        }
        match &best {
            Some((_, score)) if matched < *score => {}
            Some((_, score)) if matched == *score => tied = true,
            _ => {
                best = Some((candidate.clone(), matched));
                tied = false;
            }
        }
    }
    if tied { None } else { best }
}

/// How many components, from the end, two paths share. Compared without
/// case: Windows and macOS both treat `Hades` and `hades` as one folder.
fn shared_tail(left: &Path, right: &Path) -> usize {
    left.components()
        .rev()
        .zip(right.components().rev())
        .take_while(|(left, right)| match (left, right) {
            (Component::Normal(left), Component::Normal(right)) => {
                left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
            }
            _ => false,
        })
        .count()
}

/// Whether the candidate's own name, or its folder's, is the game's title:
/// what makes a renamed folder's `Hades.exe` Hades, and a stray `Game.exe`
/// nobody's.
fn names_the_game(candidate: &Path, title: &str) -> bool {
    let title = Catalog::normalised_title(title);
    if title.is_empty() {
        return false;
    }
    let stem = candidate
        .file_stem()
        .map(|stem| Catalog::normalised_title(&stem.to_string_lossy()));
    let folder = candidate
        .parent()
        .and_then(Path::file_name)
        .map(|name| Catalog::normalised_title(&name.to_string_lossy()));
    stem.as_deref() == Some(title.as_str()) || folder.as_deref() == Some(title.as_str())
}

fn is_in_trash(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy().to_lowercase();
        matches!(name.as_str(), ".trash" | ".trashes" | "$recycle.bin")
    })
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .is_some_and(|found| found.eq_ignore_ascii_case(extension))
}

/// Where Spotlight says items with this exact name are, on a Mac. Nothing
/// elsewhere, and nothing when Spotlight is slow or off: the pass then relies
/// on the folders it walked.
pub fn spotlight(name: &str) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        spotlight_query(name)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = name;
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
fn spotlight_query(name: &str) -> Vec<PathBuf> {
    use std::{
        io::Read,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    const TIMEOUT: Duration = Duration::from_secs(3);
    const MAX_RESULTS: usize = 64;

    if name.is_empty() || name.chars().any(char::is_control) {
        return Vec::new();
    }
    // The name goes inside a quoted Spotlight string, where `*` is a
    // wildcard: every special character is escaped so it means only itself.
    let mut escaped = String::with_capacity(name.len());
    for character in name.chars() {
        if matches!(character, '\\' | '"' | '\'' | '*') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    let query = format!("kMDItemFSName == \"{escaped}\"c");
    let Ok(mut child) = Command::new("/usr/bin/mdfind")
        .arg("-0")
        .arg(query)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < TIMEOUT => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    }
    let mut output = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut output);
    }
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .filter_map(|path| std::str::from_utf8(path).ok())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .take(MAX_RESULTS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::local_game_id;

    /// A fresh directory of its own, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn scratch() -> Scratch {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "orivo-moved-games-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Scratch(root)
    }

    fn nothing(_: &str) -> Vec<PathBuf> {
        Vec::new()
    }

    fn write_program(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"MZ").unwrap();
    }

    fn card(title: &str, program: &Path) -> Game {
        let mut game = Game::from_executable(program).unwrap();
        game.title = title.into();
        game
    }

    fn catalog_with(games: Vec<Game>) -> Catalog {
        let mut catalog = Catalog::default();
        for game in games {
            catalog.add(game).unwrap();
        }
        catalog
    }

    #[test]
    fn a_game_moved_into_another_games_folder_is_found_there() {
        let root = scratch();
        let old = root.path().join("Games/Hades/Hades.exe");
        write_program(&old);
        let mut game = card("Hades", &old);
        game.play_time_seconds = 3_600;
        let id = game.id.clone();
        let mut catalog = catalog_with(vec![game]);

        let other = root.path().join("More Games");
        let new = other.join("Hades/Hades.exe");
        write_program(&new);
        fs::remove_dir_all(root.path().join("Games/Hades")).unwrap();
        assert!(is_missing(&catalog.games[0]));

        let relocation = relocate(&mut catalog, &[other], None, &nothing);

        assert_eq!(
            relocation.relocated,
            vec![(id.clone(), "Hades".to_string())]
        );
        let game = &catalog.games[0];
        assert_eq!(game.id, id, "the card keeps its id");
        assert_eq!(game.play_time_seconds, 3_600);
        assert_eq!(game.executable_path.as_deref(), Some(new.as_path()));
        assert_eq!(game.working_directory.as_deref(), new.parent());
        assert!(!is_missing(game));
    }

    #[test]
    fn a_game_moved_into_a_sub_folder_is_found_from_where_it_was() {
        let root = scratch();
        let old = root.path().join("Games/Celeste/bin/Celeste.exe");
        write_program(&old);
        let mut catalog = catalog_with(vec![card("Celeste", &old)]);
        let new = root
            .path()
            .join("Games/Platformers/Celeste/bin/Celeste.exe");
        write_program(&new);
        fs::remove_dir_all(root.path().join("Games/Celeste")).unwrap();

        let relocation = relocate(&mut catalog, &[], None, &nothing);

        assert_eq!(relocation.relocated.len(), 1);
        assert_eq!(
            catalog.games[0].executable_path.as_deref(),
            Some(new.as_path())
        );
    }

    #[test]
    fn a_renamed_folder_is_followed_when_the_program_names_the_game() {
        let root = scratch();
        let old = root.path().join("Games/Hades v1.38/Hades.exe");
        write_program(&old);
        let mut catalog = catalog_with(vec![card("Hades", &old)]);
        let new = root.path().join("Games/Hades/Hades.exe");
        write_program(&new);
        fs::remove_dir_all(root.path().join("Games/Hades v1.38")).unwrap();

        relocate(&mut catalog, &[], None, &nothing);

        assert_eq!(
            catalog.games[0].executable_path.as_deref(),
            Some(new.as_path())
        );
    }

    #[test]
    fn a_generic_program_in_a_renamed_folder_is_not_taken_for_the_game() {
        let root = scratch();
        let old = root.path().join("Games/Inscryption/Game.exe");
        write_program(&old);
        let mut catalog = catalog_with(vec![card("Inscryption", &old)]);
        // Another game's own `Game.exe`, in a folder that is not this one.
        write_program(&root.path().join("Games/Pentiment/Game.exe"));
        fs::remove_dir_all(root.path().join("Games/Inscryption")).unwrap();

        let relocation = relocate(&mut catalog, &[], None, &nothing);

        assert!(relocation.relocated.is_empty());
        assert_eq!(relocation.missing, 1);
        assert_eq!(
            catalog.games[0].executable_path.as_deref(),
            Some(old.as_path())
        );
    }

    #[test]
    fn two_equally_good_places_leave_the_card_where_it_was() {
        let root = scratch();
        let old = root.path().join("Games/Hades/Hades.exe");
        write_program(&old);
        let mut catalog = catalog_with(vec![card("Hades", &old)]);
        let first = root.path().join("A");
        let second = root.path().join("B");
        write_program(&first.join("Hades/Hades.exe"));
        write_program(&second.join("Hades/Hades.exe"));
        fs::remove_dir_all(root.path().join("Games/Hades")).unwrap();

        let relocation = relocate(&mut catalog, &[first, second], None, &nothing);

        assert!(relocation.relocated.is_empty());
        assert_eq!(relocation.missing, 1);
    }

    #[test]
    fn a_program_another_card_starts_is_never_taken() {
        let root = scratch();
        let old = root.path().join("Old/Hades/Hades.exe");
        write_program(&old);
        let kept = root.path().join("Games/Hades/Hades.exe");
        write_program(&kept);
        let mut catalog = catalog_with(vec![card("Hades", &old), card("Hades", &kept)]);
        fs::remove_dir_all(root.path().join("Old/Hades")).unwrap();

        let relocation = relocate(&mut catalog, &[root.path().join("Games")], None, &nothing);

        assert!(relocation.relocated.is_empty());
    }

    #[test]
    fn a_game_moved_to_the_trash_is_not_found_there() {
        let root = scratch();
        let old = root.path().join("Games/Hades/Hades.exe");
        write_program(&old);
        let mut catalog = catalog_with(vec![card("Hades", &old)]);
        let trashed = root.path().join(".Trash/Hades/Hades.exe");
        write_program(&trashed);
        fs::remove_dir_all(root.path().join("Games/Hades")).unwrap();

        let trace = |_: &str| vec![trashed.clone()];
        let relocation = relocate(&mut catalog, &[], None, &trace);

        assert!(relocation.relocated.is_empty());
        assert_eq!(relocation.missing, 1);
    }

    #[test]
    fn the_system_trace_finds_a_game_moved_anywhere() {
        let root = scratch();
        let old = root.path().join("Downloads/Balatro/Balatro.exe");
        write_program(&old);
        let mut catalog = catalog_with(vec![card("Balatro", &old)]);
        let new = root.path().join("Elsewhere/Deep/Balatro/Balatro.exe");
        write_program(&new);
        fs::remove_dir_all(root.path().join("Downloads")).unwrap();

        let found = new.clone();
        let trace = move |name: &str| {
            assert_eq!(name, "Balatro.exe");
            vec![found.clone()]
        };
        let relocation = relocate(&mut catalog, &[], None, &trace);

        assert_eq!(relocation.relocated.len(), 1);
        assert_eq!(
            catalog.games[0].executable_path.as_deref(),
            Some(new.as_path())
        );
    }

    #[test]
    fn an_app_bundle_is_followed_as_one_item() {
        let root = scratch();
        let old_bundle = root.path().join("Games/Celeste.app");
        let old = old_bundle.join("Contents/MacOS/Celeste");
        write_program(&old);
        let mut game = Game::from_executable(&old).unwrap();
        game.title = "Celeste".into();
        let mut catalog = catalog_with(vec![game]);
        let new_bundle = root.path().join("Applications/Celeste.app");
        write_program(&new_bundle.join("Contents/MacOS/Celeste"));
        fs::remove_dir_all(&old_bundle).unwrap();

        let trace = |_: &str| vec![new_bundle.clone()];
        relocate(&mut catalog, &[], None, &trace);

        assert_eq!(
            catalog.games[0].executable_path.as_deref(),
            Some(new_bundle.join("Contents/MacOS/Celeste").as_path())
        );
    }

    #[test]
    fn only_the_named_card_is_looked_for() {
        let root = scratch();
        let first = root.path().join("Games/Hades/Hades.exe");
        let second = root.path().join("Games/Celeste/Celeste.exe");
        write_program(&first);
        write_program(&second);
        let mut catalog = catalog_with(vec![card("Hades", &first), card("Celeste", &second)]);
        let place = root.path().join("Moved");
        write_program(&place.join("Hades/Hades.exe"));
        write_program(&place.join("Celeste/Celeste.exe"));
        fs::remove_dir_all(root.path().join("Games")).unwrap();

        let only = local_game_id(&first);
        let relocation = relocate(&mut catalog, &[place], Some(&only), &nothing);

        assert_eq!(relocation.relocated, vec![(only, "Hades".to_string())]);
        assert!(is_missing(&catalog.games[1]));
    }

    #[test]
    fn a_game_that_cannot_be_found_is_marked_until_it_comes_back() {
        let root = scratch();
        let program = root.path().join("Games/Hades/Hades.exe");
        write_program(&program);
        let mut catalog = catalog_with(vec![card("Hades", &program)]);
        // The disk it was on is not connected.
        fs::remove_file(&program).unwrap();

        let relocation = relocate(&mut catalog, &[], None, &nothing);
        assert_eq!(relocation.missing, 1);
        assert!(marked_missing(&catalog.games[0]));
        assert_eq!(
            catalog.games[0].executable_path.as_deref(),
            Some(program.as_path()),
            "an unfound card keeps its path, for when the disk comes back"
        );

        write_program(&program);
        relocate(&mut catalog, &[], None, &nothing);
        assert!(!marked_missing(&catalog.games[0]));
    }

    #[test]
    fn a_pass_is_carried_over_only_to_cards_that_did_not_change_meanwhile() {
        let root = scratch();
        let hades = root.path().join("Games/Hades/Hades.exe");
        let celeste = root.path().join("Games/Celeste/Celeste.exe");
        write_program(&hades);
        write_program(&celeste);
        let before = catalog_with(vec![card("Hades", &hades), card("Celeste", &celeste)]);
        let moved = root.path().join("Moved");
        write_program(&moved.join("Hades/Hades.exe"));
        write_program(&moved.join("Celeste/Celeste.exe"));
        fs::remove_dir_all(root.path().join("Games")).unwrap();
        let mut after = before.clone();
        relocate(&mut after, &[moved.clone()], None, &nothing);

        // While the pass ran, Celeste was located by hand somewhere else.
        let mut current = before.clone();
        current.games[0].play_time_seconds = 60;
        let by_hand = root.path().join("Hand/Celeste.exe");
        current.games[1].executable_path = Some(by_hand.clone());

        assert!(carry_over(&mut current, &before, &after));
        assert_eq!(
            current.games[0].executable_path.as_deref(),
            Some(moved.join("Hades/Hades.exe").as_path())
        );
        assert_eq!(
            current.games[0].play_time_seconds, 60,
            "the pass never touches play time"
        );
        assert_eq!(
            current.games[1].executable_path.as_deref(),
            Some(by_hand.as_path())
        );
    }

    #[test]
    fn a_store_game_is_never_missing() {
        let mut game = card("Hades", Path::new("/nowhere/Hades.exe"));
        game.id = "showcase-hades".into();
        assert!(!is_missing(&game));
    }

    #[test]
    fn the_shared_tail_ignores_case() {
        assert_eq!(
            shared_tail(
                Path::new("/a/Games/Hades/Hades.exe"),
                Path::new("/b/games/HADES/hades.exe")
            ),
            3
        );
        assert_eq!(shared_tail(Path::new("/a/x.exe"), Path::new("/b/y.exe")), 0);
    }
}
