//! The console-emulator source flow: connecting a ROM folder, asking about what
//! is in it, and turning the answers into cards.
//!
//! This is the half of the feature that owns catalog state. What an emulator
//! exposes, what a ROM has to be, and how an intent is built are
//! [`crate::console_runner`]'s; what the user is asked, and what may be written
//! because of their answer, are here — the same split `runner_commands.rs` makes
//! for third-party runners, and for the same reason: `lib.rs` is shared by every
//! lot working on this repository at once.
//!
//! Nothing here adds a game by itself. Any app can create a file in a shared
//! folder with no permission at all, so a file that was found is a question, and
//! only the references that come back from that question are written.

use crate::catalog::{
    ConsoleEmulator, ConsoleEmulatorProfile, ConsoleRomInventoryEntry, ConsoleSystem,
    is_console_runner_id,
};
use crate::{
    AppState, Catalog, Game, GameSource, LaunchTarget, console_runner, persist_catalog,
    source_review::{self, SourceTitleCollision},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, State};

/// The Orivo-managed console emulator profiles, one per (emulator, console).
///
/// One connect covers a whole emulator, and the folder it grants is shared by
/// every console that emulator runs — but a *card* has to know which console it
/// is for, because that is what picks the core. So the managed profiles are
/// provisioned together, keyed by a fixed opaque token so they are reused across
/// restarts, and each ROM lands in the one its extension names. The full "Add an
/// emulator" flow, which will let a user point several folders at hand-made
/// profiles, is a separate change.
const AUTO_CONSOLE_PROFILE_PREFIX: &str = "orivo-auto";
const CONSOLE_IMPORT_PAGE_SIZE: usize = 50;
/// The bound the Wine and Winlator imports already use, for the same reason: a
/// selection larger than this is not a person choosing.
const MAX_CONSOLE_IMPORT_SELECTION: usize = 2_000;

/// Console emulators are Android applications, so these runners exist nowhere
/// else. The refusal is the same shape as Winlator's and Wine's.
pub(crate) fn require_console_runner_platform() -> Result<(), String> {
    if cfg!(target_os = "android") {
        Ok(())
    } else {
        Err(console_runner::ConsoleRunnerError::PlatformUnsupported.to_string())
    }
}

/// What the WebView learns about a ROM folder. Never a URI, never an absolute
/// path — and never a card, because the answer to this is the user choosing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConsoleRomFolderView {
    connected: bool,
    /// Which scan this list came from. The WebView hands it back with the
    /// references it chose, so an answer can never land on another snapshot.
    token: u64,
    emulator: String,
    emulator_label: String,
    /// Is the app this list would hand a game to actually here, and who put it
    /// there? Package visibility is the only reason Orivo can say either, and
    /// saying it is the point: a game is about to be given to *that* app.
    emulator_installed: bool,
    emulator_installer: Option<String>,
    folder_label: Option<String>,
    found: Vec<ConsoleRomView>,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConsoleRomView {
    game_ref: String,
    title: String,
    /// The console the file's extension names, for the row the user reads.
    system_label: String,
    /// The file itself, and the folders between the connected one and it. A ROM's
    /// title is its file name, so two files can claim the same game and one of
    /// them may have arrived without the user knowing.
    file_name: String,
    folder_path: String,
    duplicate_title: SourceTitleCollision,
    already_imported: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConsoleImportResponse {
    imported_ids: Vec<String>,
    skipped_refs: Vec<String>,
    message: String,
}

/// The managed profile id for one console of one emulator.
pub(crate) fn auto_console_profile_id(emulator: ConsoleEmulator, system: ConsoleSystem) -> String {
    format!(
        "{AUTO_CONSOLE_PROFILE_PREFIX}-{}-{}",
        emulator.slug(),
        system.slug()
    )
}

pub(crate) fn auto_console_profile_name(
    emulator: ConsoleEmulator,
    system: ConsoleSystem,
) -> String {
    format!("{} ({})", system.label(), emulator.label())
}

pub(crate) fn console_game_id(profile_id: &str, game_ref: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(profile_id.as_bytes());
    digest.update(game_ref.as_bytes());
    format!("runner:{profile_id}:{:x}", digest.finalize())
}

pub(crate) fn console_catalog_game(
    profile: &ConsoleEmulatorProfile,
    candidate: &console_runner::ScannedRom,
) -> Game {
    Game {
        id: console_game_id(&profile.id, &candidate.game_ref),
        title: candidate.title.clone(),
        executable_path: None,
        source: GameSource::Local,
        source_id: None,
        launch_target: LaunchTarget::Runner {
            runner_id: profile.runner_id().into(),
            game_ref: candidate.game_ref.clone(),
            profile_id: profile.id.clone(),
        },
        installation_path: None,
        working_directory: None,
        arguments: Vec::new(),
        description: Some(format!(
            "{} game started through {} on this device.",
            candidate.system.label(),
            profile.emulator.label()
        )),
        metadata: Some(candidate.system.label().into()),
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

/// A ROM scan the user has not acted on yet.
///
/// The WebView only ever sees opaque references, titles and console names out of
/// this; the paths and the fingerprints stay here, and the import re-derives them
/// from the folder rather than from anything it was sent.
#[derive(Debug)]
pub(crate) struct ConsoleImportPreview {
    /// Which scan this is. Two connects can overlap — the second one's chooser
    /// opens while the first is still walking a folder — and the answer the user
    /// gives belongs to the list they were shown, not to whichever snapshot
    /// happened to land last.
    token: u64,
    emulator: ConsoleEmulator,
    roms: Vec<console_runner::ScannedRom>,
}

/// The one scan the user is waiting on, and the flag that stops it.
#[derive(Debug, Default)]
pub(crate) struct ConsoleImportState {
    preview: Mutex<Option<ConsoleImportPreview>>,
    /// The flag the scan in flight is watching. A new connect raises the old one
    /// before installing its own: hashing a folder is bounded but not fast, and
    /// the user who just picked another folder is not waiting for the last one.
    scan: Mutex<Arc<AtomicBool>>,
    next_token: AtomicU64,
}

impl ConsoleImportState {
    /// Stop whatever scan is running and take the flag the next one watches.
    fn begin_scan(&self) -> Result<(Arc<AtomicBool>, u64), String> {
        let mut scan = self
            .scan
            .lock()
            .map_err(|_| "This emulator's import is temporarily unavailable.".to_string())?;
        scan.store(true, Ordering::Release);
        let next = Arc::new(AtomicBool::new(false));
        *scan = Arc::clone(&next);
        Ok((next, self.next_token.fetch_add(1, Ordering::Relaxed) + 1))
    }
}

/// How a pass reaches a ROM folder. The directory is what an emulator taking a
/// path opens and what every scope check compares against; the tree is how Orivo
/// reads it, which on API 30+ is the only way it can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsoleRomFolder {
    directory: PathBuf,
    tree_uri: Option<String>,
}

impl ConsoleRomFolder {
    /// A folder read by pathname is stored canonical, because every later scope
    /// check compares it against a canonicalised ROM path. A folder behind a
    /// storage access grant is already canonical — it was derived from the
    /// identifier the provider gave — and a path Orivo cannot read cannot be
    /// canonicalised anyway.
    pub(crate) fn granted_directory(&self) -> Option<PathBuf> {
        match self.tree_uri {
            Some(_) => Some(self.directory.clone()),
            None => fs::canonicalize(&self.directory).ok(),
        }
    }
}

/// The managed profiles for one emulator, and their inventories, cloned out from
/// under a read lock that is released immediately: everything after this reads a
/// folder, and reading a folder is not something to do while the catalog is
/// locked.
#[allow(clippy::type_complexity)]
pub(crate) fn console_managed_profiles(
    state: &AppState,
    emulator: ConsoleEmulator,
) -> Result<(Vec<ConsoleEmulatorProfile>, Vec<ConsoleRomInventoryEntry>), String> {
    let catalog = state
        .catalog
        .read()
        .map_err(|_| "the game catalog is temporarily unavailable".to_string())?;
    let profiles = emulator
        .systems()
        .iter()
        .filter_map(|system| catalog.console_profile(&auto_console_profile_id(emulator, *system)))
        .filter(|profile| profile.enabled)
        .cloned()
        .collect::<Vec<_>>();
    let wanted = profiles
        .iter()
        .map(|profile| profile.id.clone())
        .collect::<BTreeSet<_>>();
    let inventory = catalog
        .console_inventory
        .iter()
        .filter(|entry| wanted.contains(&entry.profile_id))
        .cloned()
        .collect();
    Ok((profiles, inventory))
}

/// Point every managed profile of one emulator at a folder, creating them the
/// first time.
pub(crate) fn point_managed_console_profiles(
    state: &AppState,
    emulator: ConsoleEmulator,
    folder: &ConsoleRomFolder,
) -> Result<(), String> {
    let _mutation = state
        .catalog_mutation
        .lock()
        .map_err(|_| "Emulator profiles are temporarily unavailable.".to_string())?;
    let mut next = state
        .catalog
        .read()
        .map_err(|_| "Emulator profiles are temporarily unavailable.".to_string())?
        .clone();
    point_managed_console_profiles_at(&mut next, emulator, folder)?;
    persist_catalog(&next, &state.catalog_path).map_err(|_| {
        "Orivo could not save that folder. Your library was left unchanged.".to_string()
    })?;
    let mut catalog = state
        .catalog
        .write()
        .map_err(|_| "Emulator profiles are temporarily unavailable.".to_string())?;
    *catalog = next;
    Ok(())
}

/// Move an emulator's managed profiles to a folder without losing what still
/// belongs to them.
///
/// A profile keeps its identity across a change of folder, so its cards keep
/// theirs. What cannot survive is an inventory entry outside the new grant: it
/// could never be launched again, and the catalog's own scope check would refuse
/// the whole write. Those entries, and the cards that stand on them, go.
pub(crate) fn point_managed_console_profiles_at(
    catalog: &mut Catalog,
    emulator: ConsoleEmulator,
    folder: &ConsoleRomFolder,
) -> Result<(), String> {
    let directory = folder
        .granted_directory()
        .ok_or_else(|| "Orivo could not read that folder.".to_string())?;
    let managed = emulator
        .systems()
        .iter()
        .map(|system| auto_console_profile_id(emulator, *system))
        .collect::<BTreeSet<_>>();

    catalog.console_inventory.retain(|entry| {
        !managed.contains(&entry.profile_id) || entry.rom_path.starts_with(&directory)
    });
    let live = catalog
        .console_inventory
        .iter()
        .map(|entry| (entry.profile_id.clone(), entry.game_ref.clone()))
        .collect::<BTreeSet<_>>();
    catalog.games.retain(|game| match &game.launch_target {
        LaunchTarget::Runner {
            runner_id,
            profile_id,
            game_ref,
        } if is_console_runner_id(runner_id) && managed.contains(profile_id) => {
            live.contains(&(profile_id.clone(), game_ref.clone()))
        }
        _ => true,
    });

    for system in emulator.systems() {
        let id = auto_console_profile_id(emulator, *system);
        let mut profile =
            catalog
                .console_profile(&id)
                .cloned()
                .unwrap_or_else(|| ConsoleEmulatorProfile {
                    id: id.clone(),
                    display_name: auto_console_profile_name(emulator, *system),
                    emulator,
                    system: *system,
                    rom_directories: Vec::new(),
                    rom_trees: Vec::new(),
                    enabled: true,
                    last_imported_at: None,
                });
        profile.rom_directories = vec![directory.clone()];
        profile.rom_trees = folder.tree_uri.iter().cloned().collect();
        // Choosing the folder by hand is explicit enough to undo an earlier
        // disable: the alternative is a connect that appears to do nothing.
        profile.enabled = true;
        catalog
            .upsert_console_profile(profile)
            .map_err(|_| "Orivo could not use that folder for this emulator.".to_string())?;
    }
    catalog
        .validate()
        .map_err(|_| "Orivo could not use that folder for this emulator.".to_string())?;
    Ok(())
}

/// Scan a connected ROM folder and remember what was found, so the user can be
/// shown it and asked.
///
/// The scan runs with no catalog lock held. It reads a folder over a
/// `ContentResolver` and hashes what it finds, and is bounded but not fast;
/// holding the lock across it would make every other catalog write wait on shared
/// storage.
pub(crate) fn preview_console_roms(
    state: &AppState,
    emulator: ConsoleEmulator,
    folder: Option<&ConsoleRomFolder>,
) -> Result<ConsoleRomFolderView, String> {
    if let Some(folder) = folder {
        point_managed_console_profiles(state, emulator, folder)?;
        crate::release_stale_document_grants(state);
    }
    let (profiles, inventory) = console_managed_profiles(state, emulator)?;
    // Every managed profile of one emulator shares the grant, and the scan is
    // emulator-wide: which console a file is for is the file's extension. So any
    // one of them carries the folder and the bounds for the whole scan.
    let Some(scanning_profile) = profiles.first() else {
        return Err(console_runner::ConsoleRunnerError::RomFolderNotConnected.to_string());
    };
    let source = console_runner::rom_source_for_profile(scanning_profile)
        .map_err(|error| error.to_string())?;
    let (cancelled, token) = state.console_preview.begin_scan()?;
    let scan = console_runner::scan_roms(
        scanning_profile,
        source.as_ref(),
        &cancelled,
        console_runner::ScanLimits::default(),
        |_| {},
    )
    .map_err(|error| error.to_string())?;

    let taken_titles = state
        .catalog
        .read()
        .map(|catalog| {
            source_review::taken_titles(catalog.games.iter().map(|game| game.title.as_str()))
        })
        .unwrap_or_default();
    let connected_root = scanning_profile
        .rom_directories
        .first()
        .cloned()
        .unwrap_or_default();
    let found = scan
        .roms
        .iter()
        .map(|rom| {
            let origin = console_runner::rom_origin(&connected_root, &rom.rom_path);
            ConsoleRomView {
                game_ref: rom.game_ref.clone(),
                title: rom.title.clone(),
                system_label: rom.system.label().into(),
                file_name: origin.file_name,
                folder_path: origin.folder_path,
                duplicate_title: source_review::title_collision(
                    &rom.title,
                    &taken_titles,
                    scan.roms.iter().map(|other| other.title.as_str()),
                ),
                already_imported: inventory.iter().any(|entry| {
                    entry.game_ref == rom.game_ref && entry.fingerprint == rom.fingerprint
                }),
            }
        })
        .collect::<Vec<_>>();
    let label = connected_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string);

    *state
        .console_preview
        .preview
        .lock()
        .map_err(|_| "This emulator's import is temporarily unavailable.".to_string())? =
        Some(ConsoleImportPreview {
            token,
            emulator,
            roms: scan.roms,
        });

    let waiting = found.iter().filter(|found| !found.already_imported).count();
    let folder_name = label.clone().unwrap_or_else(|| "that folder".into());
    let emulator_label = emulator.label();
    let installed =
        console_runner::emulator_packages(emulator, console_runner::installed_packages().as_ref())
            .into_iter()
            .find_map(|(_, facts)| facts);
    Ok(ConsoleRomFolderView {
        connected: true,
        token,
        emulator: emulator.slug().into(),
        emulator_label: emulator_label.into(),
        emulator_installed: installed.is_some(),
        emulator_installer: installed.and_then(|facts| facts.installer),
        folder_label: label,
        message: match (found.len(), waiting) {
            (0, _) => format!(
                "Connected {folder_name}. Orivo found nothing in it that {emulator_label} plays."
            ),
            (_, 0) => {
                format!("Connected {folder_name}. Every game in it is already in your library.")
            }
            (_, 1) => format!("Connected {folder_name}. One game is waiting to be added."),
            (_, waiting) => {
                format!("Connected {folder_name}. {waiting} games are waiting to be added.")
            }
        },
        found,
    })
}

/// Turn the ROMs the user picked into library cards.
///
/// The preview is a preview, never an authorization: every reference is looked up
/// in the host's own snapshot, re-resolved against the profile's grant and re-read
/// from the folder before it becomes a card, so a reference the WebView invented
/// resolves to nothing.
pub(crate) fn import_console_roms_now(
    state: &AppState,
    token: u64,
    game_refs: &[String],
) -> Result<ConsoleImportResponse, String> {
    if game_refs.is_empty() || game_refs.len() > MAX_CONSOLE_IMPORT_SELECTION {
        return Err("Choose the games to add, then try again.".into());
    }
    let requested = game_refs.iter().cloned().collect::<BTreeSet<_>>();
    let (emulator, candidates) = {
        let preview = state
            .console_preview
            .preview
            .lock()
            .map_err(|_| "This emulator's import is temporarily unavailable.".to_string())?;
        let preview = preview
            .as_ref()
            // The answer belongs to the list it was given for. A second connect
            // replaces the snapshot, and an answer to the first one would then be
            // references chosen from a list nobody is looking at.
            .filter(|preview| preview.token == token)
            .ok_or_else(|| {
                "This list is no longer available. Connect the folder again.".to_string()
            })?;
        (
            preview.emulator,
            preview
                .roms
                .iter()
                .filter(|rom| requested.contains(&rom.game_ref))
                .cloned()
                .collect::<Vec<_>>(),
        )
    };
    let mut skipped_refs = requested
        .iter()
        .filter(|game_ref| {
            !candidates
                .iter()
                .any(|candidate| &&candidate.game_ref == game_ref)
        })
        .cloned()
        .collect::<Vec<_>>();

    let (profiles, _) = console_managed_profiles(state, emulator)?;
    if profiles.is_empty() {
        return Err("This folder is no longer connected.".into());
    }

    // Re-read and re-hash before the lock is taken, not under it: a preview is
    // only a preview, and the folder is the slow part.
    let cancelled = AtomicBool::new(false);
    let mut validated = Vec::new();
    for candidate in &candidates {
        // The profile for the console this file is for: the grant is the same
        // across an emulator's profiles, but the card has to belong to the one
        // whose core will open it.
        let Some(profile) = profiles
            .iter()
            .find(|profile| profile.system == candidate.system)
        else {
            skipped_refs.push(candidate.game_ref.clone());
            continue;
        };
        let source = match console_runner::rom_source_for_profile(profile) {
            Ok(source) => source,
            Err(error) => return Err(error.to_string()),
        };
        match console_runner::revalidate_rom_import_candidate(
            profile,
            source.as_ref(),
            candidate,
            &cancelled,
        ) {
            Ok(current) => validated.push((profile.clone(), current)),
            Err(_) => skipped_refs.push(candidate.game_ref.clone()),
        }
    }

    let _mutation = state
        .catalog_mutation
        .lock()
        .map_err(|_| "the game catalog is temporarily unavailable".to_string())?;
    let mut next = state
        .catalog
        .read()
        .map_err(|_| "the game catalog is temporarily unavailable".to_string())?
        .clone();
    let imported_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default();
    let (imported_ids, rejected) = apply_console_import(&mut next, &validated, imported_at)?;
    skipped_refs.extend(rejected);

    if imported_ids.is_empty() {
        return Ok(ConsoleImportResponse {
            message: "None of those games could be added. They may have changed since Orivo listed them — review the folder again."
                .into(),
            imported_ids,
            skipped_refs,
        });
    }
    for profile in &profiles {
        if let Some(persisted) = next
            .console_profiles
            .iter_mut()
            .find(|persisted| persisted.id == profile.id)
        {
            persisted.last_imported_at = Some(imported_at);
        }
    }
    persist_catalog(&next, &state.catalog_path)
        .map_err(|_| "Orivo could not save these games.".to_string())?;
    let mut catalog = state
        .catalog
        .write()
        .map_err(|_| "the game catalog is temporarily unavailable".to_string())?;
    *catalog = next;

    let message = match imported_ids.len() {
        1 => "One game was added to your library.".to_string(),
        added => format!("{added} games were added to your library."),
    };
    // A ROM the user chose and did not get is worth a sentence: the usual reason
    // is that its file changed between the list and the confirmation.
    let message = match skipped_refs.len() {
        0 => message,
        1 => format!("{message} One changed since Orivo listed it and was left out."),
        skipped => {
            format!("{message} {skipped} changed since Orivo listed them and were left out.")
        }
    };
    Ok(ConsoleImportResponse {
        imported_ids,
        skipped_refs,
        message,
    })
}

/// Write the chosen ROMs into a catalog, in bounded pages.
///
/// One page at a time, like the runner contract's `discover-page`: every upsert
/// validates before it mutates, so one rejected candidate is skipped rather than
/// rolled back, and the whole catalog is validated once a page rather than once a
/// candidate.
pub(crate) fn apply_console_import(
    catalog: &mut Catalog,
    validated: &[(ConsoleEmulatorProfile, console_runner::ScannedRom)],
    imported_at: u64,
) -> Result<(Vec<String>, Vec<String>), String> {
    let mut imported_ids = Vec::new();
    let mut rejected = Vec::new();
    for page in validated.chunks(CONSOLE_IMPORT_PAGE_SIZE) {
        let mut applied = catalog.clone();
        for (profile, current) in page {
            let entry = ConsoleRomInventoryEntry {
                profile_id: profile.id.clone(),
                game_ref: current.game_ref.clone(),
                title: current.title.clone(),
                rom_path: current.rom_path.clone(),
                fingerprint: current.fingerprint.clone(),
                imported_at: Some(imported_at),
            };
            let game = console_catalog_game(profile, current);
            let game_id = game.id.clone();
            if applied.upsert_console_inventory(entry).is_err()
                || applied.upsert_runner(game).is_err()
            {
                rejected.push(current.game_ref.clone());
                continue;
            }
            imported_ids.push(game_id);
        }
        if applied.validate().is_err() {
            return Err("Orivo could not add these games safely.".into());
        }
        *catalog = applied;
    }
    Ok((imported_ids, rejected))
}

#[tauri::command]
pub(crate) async fn connect_console_rom_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    emulator: String,
) -> Result<ConsoleRomFolderView, String> {
    use tauri_plugin_orivo_saf::OrivoSafExt;

    require_console_runner_platform()?;
    // The WebView names an emulator because it is choosing a menu row, and this
    // is the only door that token comes through: anything outside the closed set
    // is refused here rather than becoming a package name later.
    let emulator = ConsoleEmulator::from_slug(&emulator)
        .ok_or_else(|| "Orivo does not know that emulator.".to_string())?;
    // A folder already connected is reviewed without a chooser: the same menu
    // entry is what the user reaches for after adding ROMs to it, and re-picking
    // the same folder to see them would be a chore.
    let connected = {
        let catalog = state
            .catalog
            .read()
            .map_err(|_| "Emulator profiles are temporarily unavailable.".to_string())?;
        emulator.systems().iter().any(|system| {
            catalog
                .console_profile(&auto_console_profile_id(emulator, *system))
                .is_some_and(|profile| profile.enabled && !profile.rom_trees.is_empty())
        })
    };
    if connected {
        let review = console_blocking(&app, move |state| {
            preview_console_roms(state, emulator, None)
        })
        .await;
        match review {
            Ok(view) => return Ok(view),
            // The grant was revoked, or the folder stopped being one Orivo may
            // read. Asking for it again is the answer, so fall through.
            Err(error) => eprintln!("orivo: ROM folder review needs a new grant: {error}"),
        }
    }

    let picked = app
        .orivo_saf()
        .pick_document_tree()
        .await
        .map_err(|error| {
            // The detail names a Java exception; the player gets a sentence.
            eprintln!("orivo: ROM folder picker: {error}");
            "Orivo could not open the folder chooser. Try again.".to_string()
        })?;
    let Some(tree_uri) = picked else {
        return Ok(ConsoleRomFolderView {
            connected: false,
            token: 0,
            emulator: emulator.slug().into(),
            emulator_label: emulator.label().into(),
            emulator_installed: false,
            emulator_installer: None,
            folder_label: None,
            found: Vec::new(),
            message: "No folder was connected.".into(),
        });
    };

    console_blocking(&app, move |state| {
        // The chooser returns whichever provider the user browsed to, and
        // whichever folder. An emulator taking a path opens a *file path*, and a
        // folder every app can drop a file into is not a ROM folder, so both are
        // refused here — before anything is persisted.
        let grant = console_runner::grant_for_picked_rom_folder(&tree_uri)
            .map_err(|error| error.to_string())?;
        let folder = ConsoleRomFolder {
            directory: grant.directory().to_path_buf(),
            tree_uri: Some(grant.tree_uri().to_string()),
        };
        preview_console_roms(state, emulator, Some(&folder))
    })
    .await
}

#[tauri::command]
pub(crate) async fn import_console_roms(
    app: AppHandle,
    token: u64,
    game_refs: Vec<String>,
) -> Result<ConsoleImportResponse, String> {
    require_console_runner_platform()?;
    console_blocking(&app, move |state| {
        import_console_roms_now(state, token, &game_refs)
    })
    .await
}

/// Run one piece of console-emulator work off the command executor, for the same
/// reason the Winlator flow does: all of it reads shared storage over a
/// `ContentResolver` and hashes what it finds.
async fn console_blocking<T, F>(app: &AppHandle, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&AppState) -> Result<T, String> + Send + 'static,
{
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || work(&app.state::<AppState>()))
        .await
        .map_err(|_| "That did not finish. Try again.".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{state_for, temporary_directory};
    use crate::{game_detail, presentation_catalog, winlator_saf};
    use std::path::Path;

    pub(super) fn nes_rom(name: &str) -> Vec<u8> {
        let mut bytes = b"NES\x1a\x02\x01".to_vec();
        bytes.extend_from_slice(name.as_bytes());
        bytes
    }

    fn console_folder(directory: &Path) -> ConsoleRomFolder {
        ConsoleRomFolder {
            directory: directory.to_path_buf(),
            tree_uri: None,
        }
    }

    /// The whole flow, with nothing between the folder and the card but the
    /// user's answer: a ROM is offered, chosen, and only then written.
    #[test]
    fn imports_only_the_roms_that_were_chosen() {
        let home = temporary_directory("console-chosen");
        let granted = temporary_directory("console-chosen-folder");
        for (file, name) in [
            ("Alter Ego.nes", "Alter Ego"),
            ("Uwol.smc", "Uwol"),
            ("notes.txt", "notes"),
        ] {
            fs::write(granted.join(file), nes_rom(name)).unwrap();
        }
        let state = state_for(&home);
        let view = preview_console_roms(
            &state,
            ConsoleEmulator::RetroArch,
            Some(&console_folder(&granted)),
        )
        .unwrap();

        // The text file is not a game, and nothing is in the library yet.
        assert_eq!(view.found.len(), 2);
        assert!(state.catalog.read().unwrap().games.is_empty());

        let chosen = view
            .found
            .iter()
            .filter(|found| found.title == "Alter Ego")
            .map(|found| found.game_ref.clone())
            .collect::<Vec<_>>();
        let imported = import_console_roms_now(&state, view.token, &chosen).unwrap();
        assert_eq!(imported.imported_ids.len(), 1);
        assert!(imported.skipped_refs.is_empty());

        let catalog = state.catalog.read().unwrap();
        assert_eq!(catalog.games.len(), 1);
        assert_eq!(catalog.console_inventory.len(), 1);
        // And it landed in the profile for the console its extension names, not
        // in whichever one the scan happened to run through.
        assert_eq!(
            catalog.console_inventory[0].profile_id,
            auto_console_profile_id(ConsoleEmulator::RetroArch, ConsoleSystem::Nes)
        );
        catalog.validate().unwrap();
    }

    /// One connect covers a whole emulator, so a folder holding two consoles
    /// produces cards in two profiles — and each card knows which core will open
    /// it.
    #[test]
    fn a_mixed_folder_files_each_game_under_its_own_console() {
        let home = temporary_directory("console-mixed");
        let granted = temporary_directory("console-mixed-folder");
        fs::write(granted.join("Alter Ego.nes"), nes_rom("Alter Ego")).unwrap();
        fs::write(granted.join("Uwol.smc"), nes_rom("Uwol")).unwrap();
        let state = state_for(&home);
        let view = preview_console_roms(
            &state,
            ConsoleEmulator::RetroArch,
            Some(&console_folder(&granted)),
        )
        .unwrap();
        let chosen = view
            .found
            .iter()
            .map(|found| found.game_ref.clone())
            .collect::<Vec<_>>();
        import_console_roms_now(&state, view.token, &chosen).unwrap();

        let catalog = state.catalog.read().unwrap();
        let profiles = catalog
            .console_inventory
            .iter()
            .map(|entry| entry.profile_id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            profiles,
            BTreeSet::from([
                auto_console_profile_id(ConsoleEmulator::RetroArch, ConsoleSystem::Nes),
                auto_console_profile_id(ConsoleEmulator::RetroArch, ConsoleSystem::Snes),
            ])
        );
        catalog.validate().unwrap();
    }

    /// A reference the WebView made up is not a game. The preview is the host's
    /// own snapshot, and anything outside it resolves to nothing.
    #[test]
    fn an_invented_rom_reference_imports_nothing() {
        let home = temporary_directory("console-invented");
        let granted = temporary_directory("console-invented-folder");
        fs::write(granted.join("Alter Ego.nes"), nes_rom("Alter Ego")).unwrap();
        let state = state_for(&home);
        let view = preview_console_roms(
            &state,
            ConsoleEmulator::RetroArch,
            Some(&console_folder(&granted)),
        )
        .unwrap();

        let imported =
            import_console_roms_now(&state, view.token, &["rom:deadbeef".to_string()]).unwrap();
        assert!(imported.imported_ids.is_empty());
        assert_eq!(imported.skipped_refs, ["rom:deadbeef"]);
        assert!(state.catalog.read().unwrap().games.is_empty());
    }

    /// Two connects can overlap — the second chooser opens while the first is
    /// still walking a folder — and the answer the user gives belongs to the list
    /// they were shown. An answer carrying the older list's token is refused
    /// rather than applied to whichever snapshot landed last.
    #[test]
    fn an_answer_to_a_list_that_was_replaced_imports_nothing() {
        let home = temporary_directory("console-token");
        let first = temporary_directory("console-token-first");
        fs::write(first.join("Alter Ego.nes"), nes_rom("Alter Ego")).unwrap();
        let second = temporary_directory("console-token-second");
        fs::write(second.join("Uwol.smc"), nes_rom("Uwol")).unwrap();

        let state = state_for(&home);
        let stale = preview_console_roms(
            &state,
            ConsoleEmulator::RetroArch,
            Some(&console_folder(&first)),
        )
        .unwrap();
        let fresh = preview_console_roms(
            &state,
            ConsoleEmulator::RetroArch,
            Some(&console_folder(&second)),
        )
        .unwrap();
        assert_ne!(stale.token, fresh.token);

        let answer = vec![stale.found[0].game_ref.clone()];
        assert!(import_console_roms_now(&state, stale.token, &answer).is_err());
        assert!(state.catalog.read().unwrap().games.is_empty());
    }

    /// The slug names a menu row, and the host's set of them is closed. Anything
    /// else is refused here rather than reaching a package name.
    #[test]
    fn an_emulator_orivo_does_not_know_is_refused() {
        for slug in ["retroarch", "ppsspp"] {
            assert!(ConsoleEmulator::from_slug(slug).is_some(), "{slug}");
        }
        for slug in [
            "",
            "RetroArch",
            "dolphin",
            "com.retroarch",
            "retroarch\u{0}",
        ] {
            assert!(
                ConsoleEmulator::from_slug(slug).is_none(),
                "accepted {slug:?}"
            );
        }
    }

    /// Any app can drop a file into a shared folder, so the window between the
    /// question and the answer is one somebody else can write in.
    #[test]
    fn a_rom_replaced_between_the_question_and_the_answer_is_not_imported() {
        let home = temporary_directory("console-swapped");
        let granted = temporary_directory("console-swapped-folder");
        let rom = granted.join("Alter Ego.nes");
        fs::write(&rom, nes_rom("Alter Ego")).unwrap();
        let state = state_for(&home);
        let view = preview_console_roms(
            &state,
            ConsoleEmulator::RetroArch,
            Some(&console_folder(&granted)),
        )
        .unwrap();

        fs::write(&rom, nes_rom("something else entirely")).unwrap();
        let chosen = vec![view.found[0].game_ref.clone()];
        let imported = import_console_roms_now(&state, view.token, &chosen).unwrap();

        assert!(imported.imported_ids.is_empty());
        assert_eq!(imported.skipped_refs, chosen);
        assert!(state.catalog.read().unwrap().games.is_empty());
        assert!(state.catalog.read().unwrap().console_inventory.is_empty());
    }

    /// A ROM's title is its file name, so the confirmation has to name the file
    /// and its folder — and say when a name is already somebody else's.
    #[test]
    fn the_rom_confirmation_names_the_file_its_folder_and_its_console() {
        let home = temporary_directory("console-spoof");
        let granted = temporary_directory("console-spoof-folder");
        let nested = granted.join("new");
        fs::create_dir_all(&nested).unwrap();
        fs::write(granted.join("Alter Ego.nes"), nes_rom("one")).unwrap();
        fs::write(nested.join("Alter Ego.nes"), nes_rom("two")).unwrap();
        let state = state_for(&home);
        let view = preview_console_roms(
            &state,
            ConsoleEmulator::RetroArch,
            Some(&console_folder(&granted)),
        )
        .unwrap();

        assert_eq!(view.found.len(), 2);
        for found in &view.found {
            assert_eq!(found.file_name, "Alter Ego.nes");
            assert_eq!(found.system_label, "NES");
            assert_eq!(found.duplicate_title, SourceTitleCollision::Folder);
        }
        let folders = view
            .found
            .iter()
            .map(|found| found.folder_path.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(folders, BTreeSet::from([String::new(), "new".to_string()]));
    }

    /// Connecting another folder keeps the profiles — and so the card ids — while
    /// dropping the inventory entries that fell outside the new grant: those
    /// could never be launched again.
    #[test]
    fn connecting_another_rom_folder_drops_the_cards_from_the_old_one() {
        let home = temporary_directory("console-repoint");
        let granted = temporary_directory("console-repoint-folder");
        fs::write(granted.join("Alter Ego.nes"), nes_rom("Alter Ego")).unwrap();
        let state = state_for(&home);
        let view = preview_console_roms(
            &state,
            ConsoleEmulator::RetroArch,
            Some(&console_folder(&granted)),
        )
        .unwrap();
        import_console_roms_now(&state, view.token, &[view.found[0].game_ref.clone()]).unwrap();
        assert_eq!(state.catalog.read().unwrap().console_inventory.len(), 1);

        let elsewhere = temporary_directory("console-repoint-elsewhere");
        preview_console_roms(
            &state,
            ConsoleEmulator::RetroArch,
            Some(&console_folder(&elsewhere)),
        )
        .unwrap();

        let catalog = state.catalog.read().unwrap();
        assert!(catalog.console_inventory.is_empty());
        assert!(!catalog.games.iter().any(|game| matches!(
            &game.launch_target,
            LaunchTarget::Runner { runner_id, .. } if is_console_runner_id(runner_id)
        )));
        // The profiles themselves survive, so every card imported into them next
        // keeps a stable id.
        assert_eq!(
            catalog.console_profiles.len(),
            ConsoleEmulator::RetroArch.systems().len()
        );
        catalog.validate().unwrap();
    }

    /// The library projection derives "can this be played?" from the profile and
    /// the private inventory behind a card, and it reads them out of the
    /// presentation catalog rather than the stored one. A kind of profile that
    /// projection forgets to carry makes every card of that kind say
    /// *Unavailable* on a device where it works — which is exactly what the
    /// first end-to-end run on the emulator showed.
    #[test]
    fn a_console_card_keeps_its_profile_through_the_library_projection() {
        let home = temporary_directory("console-projection");
        let granted = temporary_directory("console-projection-folder");
        fs::write(granted.join("Alter Ego.nes"), nes_rom("Alter Ego")).unwrap();
        let state = state_for(&home);
        let view = preview_console_roms(
            &state,
            ConsoleEmulator::RetroArch,
            Some(&console_folder(&granted)),
        )
        .unwrap();
        import_console_roms_now(&state, view.token, &[view.found[0].game_ref.clone()]).unwrap();

        let stored = state.catalog.read().unwrap();
        let presentation = presentation_catalog(&stored, false);
        let game = presentation
            .games
            .iter()
            .find(|game| game.title == "Alter Ego")
            .expect("the imported card");
        let LaunchTarget::Runner {
            profile_id,
            game_ref,
            ..
        } = &game.launch_target
        else {
            panic!("a console card must be a runner target");
        };
        assert!(
            game_detail::console_game_launchable(&presentation, profile_id, game_ref),
            "the card lost its profile on the way to the library"
        );
    }

    /// A persisted grant is handed back when nothing points at it any more, and a
    /// sweep that only knew about Winlator would hand back a ROM folder and
    /// silently break every card behind it.
    #[test]
    fn a_rom_folder_grant_is_not_swept_away_as_stale() {
        const ROM_TREE: &str =
            "content://com.android.externalstorage.documents/tree/primary%3ADownload%2FRoms";
        let mut catalog = Catalog::default();
        catalog.console_profiles.push(ConsoleEmulatorProfile {
            id: auto_console_profile_id(ConsoleEmulator::RetroArch, ConsoleSystem::Nes),
            display_name: "NES (RetroArch)".into(),
            emulator: ConsoleEmulator::RetroArch,
            system: ConsoleSystem::Nes,
            rom_directories: vec![PathBuf::from("/storage/emulated/0/Download/Roms")],
            rom_trees: vec![ROM_TREE.to_string()],
            enabled: true,
            last_imported_at: None,
        });
        catalog.validate().unwrap();

        // The sweep's own rule, not a copy of it: the sweep itself has to ask the
        // platform which grants exist, and no host can answer that.
        let in_use = crate::document_grants_in_use(&catalog);
        assert!(
            winlator_saf::stale_persisted_trees(&[ROM_TREE.to_string()], &in_use).is_empty(),
            "a connected ROM folder was treated as a grant nobody wanted"
        );
    }
}
