//! Installing, updating and rolling back a plugin without ever leaving half of
//! one on disk.
//!
//! The installer used to unpack into a staging directory and rename it over the
//! live one. That is atomic for the *reader* — discovery sees the old tree or
//! the new one — but it is not recoverable for the *writer*: the previous
//! version was deleted before the rename, so a process killed in between left
//! nothing behind, and a component that turned out to be unusable could not be
//! undone.
//!
//! This module turns the swap into a small transaction with a written intent.
//! Every operation is a sequence of renames — the only filesystem mutation that
//! is atomic across a crash — and a journal file says which sequence was in
//! progress. At the next start, [`PluginStore::recover`] reads that file,
//! observes which renames happened, and finishes or undoes the operation. The
//! invariant it maintains is the one the plan asks for: a cut at any step leaves
//! either the previous version intact, or the new one complete and smoke-tested.
//!
//! ```text
//! <root>/<id>/                     the live plugin; the only tree discovery reads
//! <root>/.versions/<id>/previous/  the rollback target
//! <root>/.versions/<id>/previous.json   ⟺ previous/ is a complete version
//! <root>/.journal/<id>.json        ⟺ an operation is in flight or was interrupted
//! <root>/.journal/<id>.smoke       ⟺ the promoted tree has not passed its smoke test
//! <root>/.staging/<id>~new/        the unpacked candidate
//! <root>/.staging/<id>~discard/    a tree on its way out
//! <root>/.staging/trusted/<id>     one byte: this version arrived release-signed
//! ```
//!
//! The commit point is a single rename of the journal file onto
//! `previous.json`. Before it, the operation is undone; after it, the operation
//! happened and the displaced tree is a rollback target. There is no moment
//! where both are true and none where neither is.
//!
//! Two names carry the whole scheme. `~` cannot appear in a plugin id — the
//! grammar is lowercase, digits, hyphen and dot — so `<id>~new` can never
//! collide with a real plugin called `<id>.new`. And `previous.json` is written
//! *only* by the commit rename, so its presence is proof rather than a hint.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

const STAGING_DIRECTORY: &str = ".staging";
const TRUST_DIRECTORY: &str = "trusted";
const VERSIONS_DIRECTORY: &str = ".versions";
const JOURNAL_DIRECTORY: &str = ".journal";
const PREVIOUS_DIRECTORY: &str = "previous";
const PREVIOUS_RECORD: &str = "previous.json";
const SIGNATURE_FILE: &str = "signature.ed25519";
/// Suffixes on a staging directory. Deliberately not valid inside a plugin id,
/// so a package can never name itself into another package's scratch space.
const STAGED_SUFFIX: &str = "~new";
const DISCARD_SUFFIX: &str = "~discard";
const JOURNAL_FORMAT_VERSION: u32 = 1;
const MAX_JOURNAL_BYTES: u64 = 8 * 1024;

/// Every entry of a package, already bounded and read into memory by the
/// installer. Nothing reaches this module that has not been hashed against its
/// manifest first.
pub type PackageFiles = BTreeMap<String, Vec<u8>>;

// ---------------------------------------------------------------------------
// The written intent
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    /// A package becomes the live version. The tree it displaces, if any,
    /// becomes the rollback target.
    Install,
    /// The rollback target becomes the live version again.
    Rollback,
}

/// The journal entry, and — after the commit rename — the description of the
/// rollback target. One type for both because they are the same fact read from
/// two moments: *this version is replacing that one*.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub format_version: u32,
    pub kind: OperationKind,
    pub plugin_id: String,
    /// The version that is live once the operation completes.
    pub incoming_version: String,
    pub incoming_trusted: bool,
    /// The version being displaced — the one that lands in `previous/`. `None`
    /// on a first install, where there is nothing to keep.
    pub displaced_version: Option<String>,
    pub displaced_trusted: bool,
}

impl OperationRecord {
    fn valid_for(&self, plugin_id: &str) -> bool {
        self.format_version == JOURNAL_FORMAT_VERSION && self.plugin_id == plugin_id
    }
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub plugin_id: String,
    pub version: String,
    /// What a user can go back to after this install, if anything.
    pub rollback_to: Option<String>,
}

/// What a rollback target looks like from outside. The version is the only
/// thing a caller needs; the path stays inside this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackTarget {
    pub version: String,
    pub trusted: bool,
}

/// What [`PluginStore::recover`] did to one interrupted operation. Reported so
/// a crash is visible in the journal rather than inferred from a plugin that
/// quietly changed version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryOutcome {
    pub plugin_id: String,
    pub resolution: Resolution,
}

/// When the host is asked to grade the candidate. The same check runs twice on
/// purpose, and the two moments are not interchangeable: in staging a refusal
/// costs nothing and the directory is not named after the plugin, while at the
/// final path the identity check discovery runs becomes possible and a refusal
/// has to be undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checkpoint {
    Staged,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// The operation had not reached its point of no return. The version that
    /// was live before it started is live now.
    RolledBack,
    /// The operation had swapped and passed its smoke test. Only bookkeeping
    /// was missing, and it has been written.
    Committed,
    /// A journal for a plugin that no longer has a tree anywhere. Nothing to
    /// restore; the entry is removed so it cannot be replayed.
    Abandoned,
}

/// The step an interrupted operation is stopped before. Production never sets
/// one: it exists so the crash-safety tests can cut the sequence at each rename
/// rather than assert that they could have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum Step {
    /// Unpack the package into `<id>~new`.
    Stage,
    /// Grade the staged tree, before anything is displaced.
    Preflight,
    /// Drop the rollback target this operation is about to replace.
    ClearPrevious,
    /// Write the journal file.
    WriteJournal,
    /// Rename the live tree to `previous/`.
    ArchiveLive,
    /// Rename the staged tree to live.
    PromoteStaged,
    /// Call the component at its final path.
    SmokeTest,
    /// Write the trust marker for the incoming version.
    WriteTrust,
    /// Rename the journal onto `previous.json`.
    Commit,
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PluginStore {
    root: PathBuf,
    /// One transaction at a time, per store.
    ///
    /// The recovery table is written against a *crash* — a sequence that stops
    /// — and two transactions interleaving their renames is not that: both
    /// would read the same directory layout and draw different conclusions from
    /// it. Serialising them is what keeps "which renames happened" an answer
    /// rather than a race. It costs nothing in practice: an install is already
    /// behind a download.
    gate: Arc<Mutex<()>>,
}

impl PluginStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            gate: Arc::new(Mutex::new(())),
        }
    }

    /// A panic inside a transaction poisons the gate, and refusing every later
    /// install for it would be the wrong trade: the invariants live on disk and
    /// recovery re-derives them from what is actually there.
    fn enter(&self) -> MutexGuard<'_, ()> {
        self.gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn live_directory(&self, plugin_id: &str) -> PathBuf {
        self.root.join(plugin_id)
    }

    /// A one-byte marker recording that a version arrived release-signed. It is
    /// host-owned state about *how* a plugin arrived, so it deliberately lives
    /// outside the plugin's own directory, where a package could otherwise
    /// declare itself trusted.
    pub fn trust_marker(&self, plugin_id: &str) -> PathBuf {
        self.root
            .join(STAGING_DIRECTORY)
            .join(TRUST_DIRECTORY)
            .join(plugin_id)
    }

    pub fn is_trusted(&self, plugin_id: &str) -> bool {
        self.trust_marker(plugin_id).is_file()
    }

    fn staged_directory(&self, plugin_id: &str) -> PathBuf {
        self.root
            .join(STAGING_DIRECTORY)
            .join(format!("{plugin_id}{STAGED_SUFFIX}"))
    }

    fn discard_directory(&self, plugin_id: &str) -> PathBuf {
        self.root
            .join(STAGING_DIRECTORY)
            .join(format!("{plugin_id}{DISCARD_SUFFIX}"))
    }

    fn previous_directory(&self, plugin_id: &str) -> PathBuf {
        self.root
            .join(VERSIONS_DIRECTORY)
            .join(plugin_id)
            .join(PREVIOUS_DIRECTORY)
    }

    fn previous_record(&self, plugin_id: &str) -> PathBuf {
        self.root
            .join(VERSIONS_DIRECTORY)
            .join(plugin_id)
            .join(PREVIOUS_RECORD)
    }

    fn journal_path(&self, plugin_id: &str) -> PathBuf {
        self.root
            .join(JOURNAL_DIRECTORY)
            .join(format!("{plugin_id}.json"))
    }

    /// Present from just before the tree is promoted until its smoke test
    /// passes. It is what stops recovery from committing a version that was
    /// never proved to work — the difference between "the new one complete" and
    /// "the new one present".
    fn smoke_marker(&self, plugin_id: &str) -> PathBuf {
        self.root
            .join(JOURNAL_DIRECTORY)
            .join(format!("{plugin_id}.smoke"))
    }

    /// The version a user can return to, or `None`. An operation in flight
    /// makes the answer unknowable rather than empty, so it is reported as
    /// absent until recovery has settled it.
    pub fn rollback_target(&self, plugin_id: &str) -> Option<RollbackTarget> {
        if self.journal_path(plugin_id).exists() {
            return None;
        }
        if !is_directory(&self.previous_directory(plugin_id)) {
            return None;
        }
        let record = read_record(&self.previous_record(plugin_id), plugin_id)?;
        Some(RollbackTarget {
            version: record.displaced_version?,
            trusted: record.displaced_trusted,
        })
    }

    // -----------------------------------------------------------------------
    // Install
    // -----------------------------------------------------------------------

    /// Make `files` the live version of `plugin_id`, keeping whatever was live
    /// as the rollback target.
    ///
    /// `verify` is the host's verdict on the tree, asked twice: once in staging
    /// and once at the final path. A refusal in staging is returned as it
    /// stands, because nothing has moved. A refusal at the final path is not an
    /// error the caller has to clean up after — the previous version is put
    /// back before this function returns.
    pub fn install(
        &self,
        plugin_id: &str,
        version: &str,
        trusted: bool,
        files: &PackageFiles,
        verify: &dyn Fn(&Path, Checkpoint) -> Result<(), String>,
    ) -> Result<InstallOutcome, String> {
        self.install_stopping_before(None, plugin_id, version, trusted, files, verify)
    }

    fn install_stopping_before(
        &self,
        stop: Option<Step>,
        plugin_id: &str,
        version: &str,
        trusted: bool,
        files: &PackageFiles,
        verify: &dyn Fn(&Path, Checkpoint) -> Result<(), String>,
    ) -> Result<InstallOutcome, String> {
        let _gate = self.enter();
        let cut = |step: Step| stop == Some(step);
        // An interrupted operation on this plugin is settled first. Starting a
        // second transaction over an unfinished one would overwrite the journal
        // that says how to undo it.
        self.recover_plugin(plugin_id);

        let live = self.live_directory(plugin_id);
        let displaced_version = installed_version(&live);
        let displaced_trusted = self.is_trusted(plugin_id);

        if cut(Step::Stage) {
            return Err(interrupted());
        }
        let staged = self.staged_directory(plugin_id);
        self.write_tree(&staged, files)?;

        if cut(Step::Preflight) {
            return Err(interrupted());
        }
        // Graded before anything is displaced. It cannot ask the component who
        // it is — that answer is only meaningful once the directory is named
        // after the plugin — but everything it *can* check is free here and
        // expensive after a swap.
        if let Err(refusal) = verify(&staged, Checkpoint::Staged) {
            let _ = fs::remove_dir_all(&staged);
            return Err(refusal);
        }

        if cut(Step::ClearPrevious) {
            return Err(interrupted());
        }
        // The record goes first: an unmarked directory is an invalid rollback
        // target, a marked half-deleted one would be a corrupt restore.
        let _ = fs::remove_file(self.previous_record(plugin_id));
        let previous = self.previous_directory(plugin_id);
        let _ = fs::remove_dir_all(&previous);

        if cut(Step::WriteJournal) {
            return Err(interrupted());
        }
        let record = OperationRecord {
            format_version: JOURNAL_FORMAT_VERSION,
            kind: OperationKind::Install,
            plugin_id: plugin_id.to_owned(),
            incoming_version: version.to_owned(),
            incoming_trusted: trusted,
            displaced_version: displaced_version.clone(),
            displaced_trusted,
        };
        self.write_journal(&record)?;

        if cut(Step::ArchiveLive) {
            return Err(interrupted());
        }
        if is_directory(&live) {
            create_parent(&previous)?;
            rename(&live, &previous)?;
        }

        if cut(Step::PromoteStaged) {
            return Err(interrupted());
        }
        // From here the live tree is unproven, and recovery must undo rather
        // than finish. The marker is what tells it which of the two to do.
        touch(&self.smoke_marker(plugin_id))?;
        rename(&staged, &live)?;

        if cut(Step::SmokeTest) {
            return Err(interrupted());
        }
        if let Err(refusal) = verify(&live, Checkpoint::Live) {
            self.undo_install(&record);
            return Err(refusal);
        }
        let _ = fs::remove_file(self.smoke_marker(plugin_id));

        if cut(Step::WriteTrust) {
            return Err(interrupted());
        }
        self.write_trust(plugin_id, trusted)?;

        if cut(Step::Commit) {
            return Err(interrupted());
        }
        self.commit_install(&record)?;
        Ok(InstallOutcome {
            plugin_id: plugin_id.to_owned(),
            version: version.to_owned(),
            rollback_to: displaced_version,
        })
    }

    /// The point of no return, in one rename. With something displaced, the
    /// journal *becomes* the rollback target's description; with nothing
    /// displaced there is no target, so the journal is simply dropped.
    fn commit_install(&self, record: &OperationRecord) -> Result<(), String> {
        let journal = self.journal_path(&record.plugin_id);
        if record.displaced_version.is_some() {
            let destination = self.previous_record(&record.plugin_id);
            create_parent(&destination)?;
            rename(&journal, &destination)?;
        } else {
            let _ = fs::remove_file(&journal);
        }
        sync_directory(journal.parent());
        Ok(())
    }

    /// Put back what an install displaced.
    ///
    /// Re-entrant on purpose: a crash inside this sequence is recovered by
    /// running it again, so it branches on what is on disk rather than on how
    /// far it got. The one state that has to be read correctly is "the kept
    /// tree is gone and something was displaced" — that is this function having
    /// already restored it, not a plugin to delete.
    fn undo_install(&self, record: &OperationRecord) {
        let plugin_id = &record.plugin_id;
        let live = self.live_directory(plugin_id);
        let previous = self.previous_directory(plugin_id);

        if is_directory(&previous) {
            if is_directory(&live) {
                let discard = self.discard_directory(plugin_id);
                let _ = fs::remove_dir_all(&discard);
                let _ = create_parent(&discard);
                let _ = rename(&live, &discard);
            }
            let _ = rename(&previous, &live);
            let _ = self.write_trust(plugin_id, record.displaced_trusted);
        } else if record.displaced_version.is_none() {
            // A first install that failed leaves no plugin, and no claim that
            // one was ever signed.
            let _ = fs::remove_dir_all(&live);
            let _ = fs::remove_file(self.trust_marker(plugin_id));
        } else {
            let _ = self.write_trust(plugin_id, record.displaced_trusted);
        }
        self.clear_operation(plugin_id);
    }

    // -----------------------------------------------------------------------
    // Rollback
    // -----------------------------------------------------------------------

    /// Return to the kept version. The tree being left is discarded rather than
    /// kept as a second target: a rollback is a way back to a version that
    /// worked, not a two-way switch, and one slot is the only one whose
    /// contents are always known to have passed a smoke test.
    pub fn rollback(&self, plugin_id: &str) -> Result<RollbackTarget, String> {
        self.rollback_stopping_before(None, plugin_id)
    }

    fn rollback_stopping_before(
        &self,
        stop: Option<Step>,
        plugin_id: &str,
    ) -> Result<RollbackTarget, String> {
        let _gate = self.enter();
        let cut = |step: Step| stop == Some(step);
        self.recover_plugin(plugin_id);

        let target = self.rollback_target(plugin_id).ok_or_else(|| {
            "Orivo has no earlier version of this plugin to go back to.".to_string()
        })?;
        let live = self.live_directory(plugin_id);
        let record = OperationRecord {
            format_version: JOURNAL_FORMAT_VERSION,
            kind: OperationKind::Rollback,
            plugin_id: plugin_id.to_owned(),
            incoming_version: target.version.clone(),
            incoming_trusted: target.trusted,
            displaced_version: installed_version(&live),
            displaced_trusted: self.is_trusted(plugin_id),
        };

        if cut(Step::WriteJournal) {
            return Err(interrupted());
        }
        self.write_journal(&record)?;

        if cut(Step::ArchiveLive) {
            return Err(interrupted());
        }
        let discard = self.discard_directory(plugin_id);
        let _ = fs::remove_dir_all(&discard);
        if is_directory(&live) {
            create_parent(&discard)?;
            rename(&live, &discard)?;
        }

        if cut(Step::PromoteStaged) {
            return Err(interrupted());
        }
        rename(&self.previous_directory(plugin_id), &live)?;

        if cut(Step::WriteTrust) {
            return Err(interrupted());
        }
        self.write_trust(plugin_id, target.trusted)?;

        if cut(Step::Commit) {
            return Err(interrupted());
        }
        self.finish_rollback(plugin_id);
        Ok(target)
    }

    fn finish_rollback(&self, plugin_id: &str) {
        let _ = fs::remove_file(self.previous_record(plugin_id));
        self.clear_operation(plugin_id);
    }

    // -----------------------------------------------------------------------
    // Removal
    // -----------------------------------------------------------------------

    /// Remove a plugin and everything the host kept about it, including the
    /// version it could have gone back to. Uninstalling is the destructive
    /// door; keeping a rollback target for a plugin the user removed would be
    /// disk the Plugins panel cannot show and cannot offer.
    pub fn remove(&self, plugin_id: &str) -> Result<(), String> {
        let _gate = self.enter();
        self.recover_plugin(plugin_id);
        let directory = self.live_directory(plugin_id);
        // `symlink_metadata` never follows: a symlink planted in the plugin
        // root must not turn a removal into a delete somewhere else.
        let metadata = fs::symlink_metadata(&directory)
            .map_err(|_| "That plugin is not installed.".to_string())?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err("That is not a plugin Orivo installed.".into());
        }
        fs::remove_dir_all(&directory)
            .map_err(|_| "The plugin could not be removed.".to_string())?;
        let _ = fs::remove_file(self.trust_marker(plugin_id));
        let _ = fs::remove_dir_all(self.root.join(VERSIONS_DIRECTORY).join(plugin_id));
        self.clear_operation(plugin_id);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Recovery
    // -----------------------------------------------------------------------

    /// Settle every interrupted operation, then sweep the scratch space. Runs
    /// once at start and again before each operation, because a journal left by
    /// a crash must never be overwritten by the next transaction.
    pub fn recover(&self) -> Vec<RecoveryOutcome> {
        let _gate = self.enter();
        let mut outcomes = Vec::new();
        let Ok(entries) = fs::read_dir(self.root.join(JOURNAL_DIRECTORY)) else {
            self.sweep_staging();
            return outcomes;
        };
        let mut plugin_ids = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".json").map(str::to_owned)
            })
            .collect::<Vec<_>>();
        plugin_ids.sort();
        for plugin_id in plugin_ids {
            if let Some(resolution) = self.recover_plugin(&plugin_id) {
                outcomes.push(RecoveryOutcome {
                    plugin_id,
                    resolution,
                });
            }
        }
        self.sweep_staging();
        outcomes
    }

    /// Finish or undo one plugin's interrupted operation.
    ///
    /// The decision is made from what is on disk, not from a progress field: a
    /// field would have to be updated between renames, and an update that is
    /// not itself atomic reintroduces the problem it claims to solve. Which
    /// directories exist is the record, and each rename moves exactly one.
    fn recover_plugin(&self, plugin_id: &str) -> Option<Resolution> {
        let journal = self.journal_path(plugin_id);
        let Some(record) = read_record(&journal, plugin_id) else {
            if journal.exists() {
                // A journal Orivo cannot read is a journal it cannot act on.
                // Leaving the live tree alone is the safe half of the invariant.
                let _ = fs::remove_file(&journal);
                let _ = fs::remove_file(self.smoke_marker(plugin_id));
                return Some(Resolution::Abandoned);
            }
            return None;
        };
        let staged = is_directory(&self.staged_directory(plugin_id));
        let live = is_directory(&self.live_directory(plugin_id));
        let previous = is_directory(&self.previous_directory(plugin_id));
        let unproven = self.smoke_marker(plugin_id).exists();

        let resolution = match record.kind {
            OperationKind::Install => match (staged, live, previous) {
                // The candidate is still in staging and the live tree is where
                // it was: nothing was swapped. The rollback target was already
                // cleared, which costs the *previous* update's undo and never
                // the running version.
                (true, true, _) => {
                    let _ = fs::remove_dir_all(self.staged_directory(plugin_id));
                    let _ = fs::remove_dir_all(self.previous_directory(plugin_id));
                    Resolution::RolledBack
                }
                // Archived but not promoted. The tree in `previous/` is the
                // live one, moved by a rename that either happened or did not.
                (true, false, true) => {
                    let _ = fs::remove_dir_all(self.staged_directory(plugin_id));
                    self.restore_previous(&record);
                    Resolution::RolledBack
                }
                // A first install that never promoted. There is nothing to put
                // back, and the candidate is unproven.
                (true, false, false) => {
                    let _ = fs::remove_dir_all(self.staged_directory(plugin_id));
                    Resolution::RolledBack
                }
                // Promoted but never proved. Undo, exactly as a failed smoke
                // test would have.
                (false, true, _) if unproven => {
                    self.undo_install(&record);
                    return Some(Resolution::RolledBack);
                }
                // Promoted and proved, with a target kept: only the commit
                // rename was missing.
                (false, true, true) => {
                    let _ = self.write_trust(plugin_id, record.incoming_trusted);
                    let _ = self.commit_install(&record);
                    return Some(Resolution::Committed);
                }
                // Same, with nothing displaced — a first install. Or an undo
                // that had already put the old tree back and lost only its
                // bookkeeping; `displaced_version` separates the two.
                (false, true, false) => {
                    let trusted = if record.displaced_version.is_none() {
                        record.incoming_trusted
                    } else {
                        record.displaced_trusted
                    };
                    let _ = self.write_trust(plugin_id, trusted);
                    if record.displaced_version.is_none() {
                        Resolution::Committed
                    } else {
                        Resolution::RolledBack
                    }
                }
                // Archived, and then the promotion or an undo was cut between
                // its two renames. The kept tree is the only complete one.
                (false, false, true) => {
                    self.restore_previous(&record);
                    Resolution::RolledBack
                }
                (false, false, false) => Resolution::Abandoned,
            },
            OperationKind::Rollback => match (live, previous) {
                // The swap had not begun.
                (true, true) => Resolution::RolledBack,
                // Between the two renames: finishing gives the user the version
                // they asked to return to, which is also the one known to work.
                (false, true) => {
                    let _ = rename(
                        &self.previous_directory(plugin_id),
                        &self.live_directory(plugin_id),
                    );
                    let _ = self.write_trust(plugin_id, record.incoming_trusted);
                    self.finish_rollback(plugin_id);
                    return Some(Resolution::Committed);
                }
                // Promoted; only the marker and the trust flag were missing.
                (true, false) => {
                    let _ = self.write_trust(plugin_id, record.incoming_trusted);
                    self.finish_rollback(plugin_id);
                    return Some(Resolution::Committed);
                }
                // Neither exists. The tree on its way out is all there is.
                (false, false) => {
                    let discard = self.discard_directory(plugin_id);
                    if is_directory(&discard) {
                        let _ = rename(&discard, &self.live_directory(plugin_id));
                        let _ = self.write_trust(plugin_id, record.displaced_trusted);
                        Resolution::RolledBack
                    } else {
                        Resolution::Abandoned
                    }
                }
            },
        };
        self.clear_operation(plugin_id);
        Some(resolution)
    }

    fn restore_previous(&self, record: &OperationRecord) {
        let plugin_id = &record.plugin_id;
        let _ = rename(
            &self.previous_directory(plugin_id),
            &self.live_directory(plugin_id),
        );
        let _ = self.write_trust(plugin_id, record.displaced_trusted);
    }

    fn clear_operation(&self, plugin_id: &str) {
        let journal = self.journal_path(plugin_id);
        let _ = fs::remove_file(&journal);
        let _ = fs::remove_file(self.smoke_marker(plugin_id));
        let _ = fs::remove_dir_all(self.discard_directory(plugin_id));
        let _ = fs::remove_dir_all(self.staged_directory(plugin_id));
        sync_directory(journal.parent());
    }

    /// Anything left in staging by a process that stopped before it wrote a
    /// journal. Only the two operation suffixes are swept, so the trust
    /// markers that live alongside them are untouched.
    fn sweep_staging(&self) {
        let Ok(entries) = fs::read_dir(self.root.join(STAGING_DIRECTORY)) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(STAGED_SUFFIX) || name.ends_with(DISCARD_SUFFIX) {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }

    // -----------------------------------------------------------------------
    // Filesystem primitives
    // -----------------------------------------------------------------------

    /// Unpack a package into a fresh directory, durably. Every file is flushed
    /// before the directory is handed to a rename, because a rename that
    /// publishes unwritten bytes is exactly the half-written plugin this module
    /// exists to prevent.
    fn write_tree(&self, directory: &Path, files: &PackageFiles) -> Result<(), String> {
        let _ = fs::remove_dir_all(directory);
        fs::create_dir_all(directory)
            .map_err(|_| "The plugin folder is unavailable.".to_string())?;
        for (path, contents) in files {
            // The detached signature has done its work by now: it authenticated
            // the manifest, which pins every other file by digest. Keeping it
            // in the tree would only invite a reader to re-verify it there.
            if path == SIGNATURE_FILE {
                continue;
            }
            let target = directory.join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|_| "The plugin folder is unavailable.".to_string())?;
            }
            write_durably(&target, contents)
                .map_err(|_| "The plugin could not be written to disk.".to_string())?;
        }
        sync_directory(Some(directory));
        Ok(())
    }

    fn write_journal(&self, record: &OperationRecord) -> Result<(), String> {
        let path = self.journal_path(&record.plugin_id);
        create_parent(&path)?;
        let encoded = serde_json::to_vec(record)
            .map_err(|_| "The update could not be recorded.".to_string())?;
        write_durably(&path, &encoded)
            .map_err(|_| "The update could not be recorded.".to_string())?;
        sync_directory(path.parent());
        Ok(())
    }

    fn write_trust(&self, plugin_id: &str, trusted: bool) -> Result<(), String> {
        let marker = self.trust_marker(plugin_id);
        create_parent(&marker)?;
        if trusted {
            write_durably(&marker, b"1")
                .map_err(|_| "The plugin folder is unavailable.".to_string())
        } else {
            let _ = fs::remove_file(&marker);
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The version recorded in a tree's own manifest. Read as untrusted text and
/// only used for display and for the "is this an upgrade" comparison — never to
/// decide where anything is written.
fn installed_version(directory: &Path) -> Option<String> {
    let bytes = fs::read(directory.join("manifest.json")).ok()?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return None;
    }
    let manifest = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    let version = manifest.get("version")?.as_str()?;
    (!version.is_empty() && version.len() <= 32).then(|| version.to_owned())
}

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

fn read_record(path: &Path, plugin_id: &str) -> Option<OperationRecord> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_JOURNAL_BYTES {
        return None;
    }
    let record = serde_json::from_slice::<OperationRecord>(&fs::read(path).ok()?).ok()?;
    record.valid_for(plugin_id).then_some(record)
}

fn is_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_dir() && !metadata.file_type().is_symlink())
}

fn create_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(parent) => {
            fs::create_dir_all(parent).map_err(|_| "The plugin folder is unavailable.".to_string())
        }
        None => Ok(()),
    }
}

fn rename(from: &Path, to: &Path) -> Result<(), String> {
    fs::rename(from, to).map_err(|_| "The plugin could not be installed.".to_string())?;
    sync_directory(to.parent());
    Ok(())
}

fn touch(path: &Path) -> Result<(), String> {
    create_parent(path)?;
    write_durably(path, b"").map_err(|_| "The update could not be recorded.".to_string())
}

fn write_durably(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Flush a directory entry so a rename survives a power cut, not merely a
/// killed process. Windows offers no equivalent and does not need one for the
/// kill case, which is the one a test can produce.
#[cfg(unix)]
fn sync_directory(path: Option<&Path>) {
    if let Some(path) = path
        && let Ok(handle) = fs::File::open(path)
    {
        let _ = handle.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_directory(_path: Option<&Path>) {}

fn interrupted() -> String {
    "The update was interrupted.".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A root no other test can land in. These tests run in parallel and write
    /// the same plugin directory name, so the clock alone is not enough.
    fn temporary_root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "orivo-plugin-update-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    const PLUGIN: &str = "com.orivo.fixture";

    fn files(version: &str, payload: &str) -> PackageFiles {
        PackageFiles::from([
            (
                "manifest.json".to_string(),
                format!(r#"{{"id":"{PLUGIN}","version":"{version}"}}"#).into_bytes(),
            ),
            ("component.wasm".to_string(), payload.as_bytes().to_vec()),
            (
                "assets/catalog.json".to_string(),
                format!(r#"{{"version":"{version}"}}"#).into_bytes(),
            ),
            // Never written into the tree, whatever the archive carried.
            (SIGNATURE_FILE.to_string(), vec![0_u8; 64]),
        ])
    }

    fn passes(_directory: &Path, _checkpoint: Checkpoint) -> Result<(), String> {
        Ok(())
    }

    /// Refuses only at the final path, which is where the identity check that
    /// catches a component disagreeing with its manifest actually lives.
    fn refuses(_directory: &Path, checkpoint: Checkpoint) -> Result<(), String> {
        match checkpoint {
            Checkpoint::Staged => Ok(()),
            Checkpoint::Live => {
                Err("This plugin does not match the package it was installed from.".into())
            }
        }
    }

    fn live_version(store: &PluginStore) -> Option<String> {
        installed_version(&store.live_directory(PLUGIN))
    }

    fn live_payload(store: &PluginStore) -> Option<String> {
        fs::read_to_string(store.live_directory(PLUGIN).join("component.wasm")).ok()
    }

    /// Every step of an install, in the order the transaction runs them. A test
    /// that walks this list proves the recovery table has no gap rather than
    /// asserting that it does not.
    const INSTALL_STEPS: [Step; 9] = [
        Step::Stage,
        Step::Preflight,
        Step::ClearPrevious,
        Step::WriteJournal,
        Step::ArchiveLive,
        Step::PromoteStaged,
        Step::SmokeTest,
        Step::WriteTrust,
        Step::Commit,
    ];

    #[test]
    fn a_first_install_leaves_a_complete_tree_and_no_way_back() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());

        let outcome = store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .expect("installs");
        assert_eq!(outcome.rollback_to, None);
        assert_eq!(live_version(&store).as_deref(), Some("1.0.0"));
        assert!(
            store
                .live_directory(PLUGIN)
                .join("assets/catalog.json")
                .is_file()
        );
        // The detached signature is never materialised beside the component.
        assert!(!store.live_directory(PLUGIN).join(SIGNATURE_FILE).exists());
        assert!(store.is_trusted(PLUGIN));
        assert_eq!(store.rollback_target(PLUGIN), None);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_update_keeps_the_version_it_replaced() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .unwrap();

        let outcome = store
            .install(PLUGIN, "2.0.0", true, &files("2.0.0", "two"), &passes)
            .expect("updates");
        assert_eq!(outcome.rollback_to.as_deref(), Some("1.0.0"));
        assert_eq!(live_payload(&store).as_deref(), Some("two"));
        assert_eq!(
            store.rollback_target(PLUGIN),
            Some(RollbackTarget {
                version: "1.0.0".into(),
                trusted: true
            })
        );

        let target = store.rollback(PLUGIN).expect("rolls back");
        assert_eq!(target.version, "1.0.0");
        assert_eq!(live_payload(&store).as_deref(), Some("one"));
        // A rollback is a way back, not a switch: there is nothing further to
        // return to once it has run.
        assert_eq!(store.rollback_target(PLUGIN), None);
        fs::remove_dir_all(root).ok();
    }

    /// The plan's promise, as a test: the previous version stays until the new
    /// component has passed its smoke test, and the host puts it back itself.
    #[test]
    fn a_component_that_fails_its_smoke_test_is_rolled_back_automatically() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .unwrap();

        let refusal = store
            .install(PLUGIN, "2.0.0", true, &files("2.0.0", "two"), &refuses)
            .expect_err("the smoke test refuses it");
        assert!(refusal.contains("does not match the package"));

        assert_eq!(live_version(&store).as_deref(), Some("1.0.0"));
        assert_eq!(live_payload(&store).as_deref(), Some("one"));
        assert!(store.is_trusted(PLUGIN));
        assert!(!store.journal_path(PLUGIN).exists());
        // Nothing is left half-installed for discovery to trip over.
        assert!(!is_directory(&store.staged_directory(PLUGIN)));
        assert!(!is_directory(&store.discard_directory(PLUGIN)));
        fs::remove_dir_all(root).ok();
    }

    /// A candidate refused in staging costs nothing at all: no journal, no
    /// displaced tree, no rollback slot spent. That is the difference between
    /// the two checkpoints, and the reason the cheap one runs first.
    #[test]
    fn a_candidate_refused_in_staging_never_reaches_the_transaction() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install(PLUGIN, "2.0.0", true, &files("2.0.0", "two"), &passes)
            .unwrap();
        assert!(store.rollback_target(PLUGIN).is_some());

        let refusal = store.install(
            PLUGIN,
            "3.0.0",
            true,
            &files("3.0.0", "three"),
            &|_directory, _checkpoint| Err("This plugin is built for a newer Orivo.".into()),
        );
        assert_eq!(
            refusal,
            Err("This plugin is built for a newer Orivo.".into())
        );
        assert_eq!(live_payload(&store).as_deref(), Some("two"));
        assert!(!store.journal_path(PLUGIN).exists());
        assert!(!is_directory(&store.staged_directory(PLUGIN)));
        // And the way back is still there, which is what distinguishes this
        // from a candidate that got as far as being swapped in.
        assert_eq!(
            store.rollback_target(PLUGIN).map(|target| target.version),
            Some("1.0.0".into())
        );
        fs::remove_dir_all(root).ok();
    }

    /// The cost of one slot, stated as a test rather than discovered later: an
    /// update that reaches the swap and is undone leaves the working version
    /// live and no way further back. The slot is cleared before the archive
    /// rename because a rename needs its destination free, and one slot whose
    /// contents are always known-good is worth more than two whose provenance
    /// has to be reasoned about.
    #[test]
    fn an_update_that_is_undone_spends_the_earlier_rollback_slot() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install(PLUGIN, "2.0.0", true, &files("2.0.0", "two"), &passes)
            .unwrap();

        store
            .install(PLUGIN, "3.0.0", true, &files("3.0.0", "three"), &refuses)
            .expect_err("the smoke test refuses it");

        assert_eq!(live_payload(&store).as_deref(), Some("two"));
        assert_eq!(store.rollback_target(PLUGIN), None);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_first_install_that_fails_its_smoke_test_leaves_no_plugin() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());

        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &refuses)
            .expect_err("the smoke test refuses it");

        assert!(!is_directory(&store.live_directory(PLUGIN)));
        assert!(!store.is_trusted(PLUGIN));
        assert!(!store.journal_path(PLUGIN).exists());
        fs::remove_dir_all(root).ok();
    }

    /// A signed registry package upgrading a sideloaded one takes the trust
    /// marker with it; a sideload replacing an official build gives it up. The
    /// developer channel can never inherit the official channel's badge.
    #[test]
    fn the_trust_marker_follows_the_version_that_is_live() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());

        store
            .install(PLUGIN, "1.0.0", false, &files("1.0.0", "one"), &passes)
            .unwrap();
        assert!(!store.is_trusted(PLUGIN));

        store
            .install(PLUGIN, "2.0.0", true, &files("2.0.0", "two"), &passes)
            .unwrap();
        assert!(store.is_trusted(PLUGIN));

        store
            .install(PLUGIN, "3.0.0", false, &files("3.0.0", "three"), &passes)
            .unwrap();
        assert!(!store.is_trusted(PLUGIN));

        // And a rollback restores the badge the kept version arrived with.
        store.rollback(PLUGIN).unwrap();
        assert_eq!(live_version(&store).as_deref(), Some("2.0.0"));
        assert!(store.is_trusted(PLUGIN));
        fs::remove_dir_all(root).ok();
    }

    /// The crash-safety proof. For each step of the transaction: stop just
    /// before it, then "restart" by constructing a fresh store over the same
    /// root and recovering. The result must be one of exactly two trees, never
    /// a mixture and never an absence.
    #[test]
    fn an_update_interrupted_at_any_step_leaves_one_whole_version() {
        for step in INSTALL_STEPS {
            let root = temporary_root();
            let store = PluginStore::new(root.clone());
            store
                .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
                .unwrap();

            let cut = store.install_stopping_before(
                Some(step),
                PLUGIN,
                "2.0.0",
                true,
                &files("2.0.0", "two"),
                &passes,
            );
            assert!(cut.is_err(), "{step:?} should have been interrupted");

            let restarted = PluginStore::new(root.clone());
            let recovered = restarted.recover();

            let version = live_version(&restarted);
            let payload = live_payload(&restarted);
            assert!(
                (version.as_deref(), payload.as_deref()) == (Some("1.0.0"), Some("one"))
                    || (version.as_deref(), payload.as_deref()) == (Some("2.0.0"), Some("two")),
                "after {step:?}: version {version:?} with payload {payload:?}"
            );
            // Whatever survived, the tree is whole and the scratch space is
            // empty — a reader after recovery sees a plugin, not a transaction.
            assert!(
                restarted
                    .live_directory(PLUGIN)
                    .join("assets/catalog.json")
                    .is_file(),
                "after {step:?}: the surviving tree is missing an asset"
            );
            assert_eq!(
                fs::read_to_string(restarted.live_directory(PLUGIN).join("assets/catalog.json"))
                    .unwrap(),
                format!(r#"{{"version":"{}"}}"#, version.clone().unwrap()),
                "after {step:?}: the tree mixes two versions"
            );
            assert!(!restarted.journal_path(PLUGIN).exists(), "after {step:?}");
            assert!(!restarted.smoke_marker(PLUGIN).exists(), "after {step:?}");
            assert!(
                !is_directory(&restarted.staged_directory(PLUGIN)),
                "after {step:?}: staging was not swept"
            );
            assert!(
                !is_directory(&restarted.discard_directory(PLUGIN)),
                "after {step:?}: a discarded tree was left behind"
            );
            // Recovery either had work or the step was before the journal.
            assert_eq!(
                recovered.is_empty(),
                step <= Step::WriteJournal,
                "after {step:?}: recovery reported {recovered:?}"
            );
            fs::remove_dir_all(root).ok();
        }
    }

    /// The same walk for a first install, where the "previous version" is the
    /// absence of one. Recovery must not invent a plugin out of a staged tree
    /// that was never promoted, nor lose one that was.
    #[test]
    fn a_first_install_interrupted_at_any_step_is_all_or_nothing() {
        for step in INSTALL_STEPS {
            let root = temporary_root();
            let store = PluginStore::new(root.clone());

            store
                .install_stopping_before(
                    Some(step),
                    PLUGIN,
                    "1.0.0",
                    true,
                    &files("1.0.0", "one"),
                    &passes,
                )
                .expect_err("interrupted");

            let restarted = PluginStore::new(root.clone());
            restarted.recover();

            match live_version(&restarted).as_deref() {
                None => assert!(
                    !restarted.live_directory(PLUGIN).exists(),
                    "after {step:?}: a directory without a manifest"
                ),
                Some("1.0.0") => assert_eq!(live_payload(&restarted).as_deref(), Some("one")),
                other => panic!("after {step:?}: unexpected version {other:?}"),
            }
            assert!(!restarted.journal_path(PLUGIN).exists(), "after {step:?}");
            assert!(
                !is_directory(&restarted.staged_directory(PLUGIN)),
                "after {step:?}"
            );
            fs::remove_dir_all(root).ok();
        }
    }

    /// A rollback is a transaction too. Cutting it must never leave the plugin
    /// missing — the one outcome a user cannot recover from on their own.
    #[test]
    fn a_rollback_interrupted_at_any_step_leaves_one_whole_version() {
        for step in [
            Step::WriteJournal,
            Step::ArchiveLive,
            Step::PromoteStaged,
            Step::WriteTrust,
            Step::Commit,
        ] {
            let root = temporary_root();
            let store = PluginStore::new(root.clone());
            store
                .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
                .unwrap();
            store
                .install(PLUGIN, "2.0.0", false, &files("2.0.0", "two"), &passes)
                .unwrap();

            store
                .rollback_stopping_before(Some(step), PLUGIN)
                .expect_err("interrupted");

            let restarted = PluginStore::new(root.clone());
            restarted.recover();

            let payload = live_payload(&restarted);
            assert!(
                payload.as_deref() == Some("one") || payload.as_deref() == Some("two"),
                "after {step:?}: payload {payload:?}"
            );
            assert_eq!(
                live_version(&restarted).as_deref(),
                if payload.as_deref() == Some("one") {
                    Some("1.0.0")
                } else {
                    Some("2.0.0")
                },
                "after {step:?}: the tree mixes two versions"
            );
            // The trust marker tracks whichever version won, never the other.
            assert_eq!(
                restarted.is_trusted(PLUGIN),
                payload.as_deref() == Some("one"),
                "after {step:?}: the trust marker belongs to the other version"
            );
            assert!(!restarted.journal_path(PLUGIN).exists(), "after {step:?}");
            assert!(
                !is_directory(&restarted.discard_directory(PLUGIN)),
                "after {step:?}"
            );
            fs::remove_dir_all(root).ok();
        }
    }

    /// The rename between "archive the live tree" and "promote the candidate"
    /// is the one moment no plugin directory exists at all. Recovery has to see
    /// the archived tree for what it is — the live version, moved — rather than
    /// a rollback target from an older update.
    #[test]
    fn a_cut_between_the_two_renames_restores_the_live_tree() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install_stopping_before(
                Some(Step::PromoteStaged),
                PLUGIN,
                "2.0.0",
                true,
                &files("2.0.0", "two"),
                &passes,
            )
            .expect_err("interrupted");

        // The state a reader would find mid-crash: no plugin directory at all.
        assert!(!is_directory(&store.live_directory(PLUGIN)));
        assert!(is_directory(&store.previous_directory(PLUGIN)));

        let restarted = PluginStore::new(root.clone());
        assert_eq!(
            restarted.recover(),
            vec![RecoveryOutcome {
                plugin_id: PLUGIN.into(),
                resolution: Resolution::RolledBack,
            }]
        );
        assert_eq!(live_payload(&restarted).as_deref(), Some("one"));
        fs::remove_dir_all(root).ok();
    }

    /// A tree that was promoted and then never proved must not be committed by
    /// recovery. Without the smoke marker this is indistinguishable from a
    /// successful update whose commit rename was lost.
    #[test]
    fn a_promoted_tree_that_never_passed_its_smoke_test_is_not_committed() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install_stopping_before(
                Some(Step::SmokeTest),
                PLUGIN,
                "2.0.0",
                true,
                &files("2.0.0", "two"),
                &passes,
            )
            .expect_err("interrupted");

        assert_eq!(live_payload(&store).as_deref(), Some("two"));
        assert!(store.smoke_marker(PLUGIN).exists());

        let restarted = PluginStore::new(root.clone());
        restarted.recover();
        assert_eq!(live_payload(&restarted).as_deref(), Some("one"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_second_operation_settles_the_first_before_it_starts() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install_stopping_before(
                Some(Step::PromoteStaged),
                PLUGIN,
                "2.0.0",
                true,
                &files("2.0.0", "two"),
                &passes,
            )
            .expect_err("interrupted");

        // No `recover()` call: the next install has to do it, or it would
        // overwrite the journal that says how to undo the last one.
        store
            .install(PLUGIN, "3.0.0", true, &files("3.0.0", "three"), &passes)
            .expect("installs over an interrupted update");
        assert_eq!(live_payload(&store).as_deref(), Some("three"));
        assert_eq!(
            store.rollback_target(PLUGIN).map(|target| target.version),
            Some("1.0.0".into()),
            "the interrupted update never became a rollback target"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_journal_orivo_cannot_read_never_touches_the_live_tree() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .unwrap();
        let journal = store.journal_path(PLUGIN);
        create_parent(&journal).unwrap();
        fs::write(&journal, b"{ not json").unwrap();

        let restarted = PluginStore::new(root.clone());
        assert_eq!(
            restarted.recover(),
            vec![RecoveryOutcome {
                plugin_id: PLUGIN.into(),
                resolution: Resolution::Abandoned,
            }]
        );
        assert_eq!(live_payload(&restarted).as_deref(), Some("one"));
        assert!(!journal.exists());
        fs::remove_dir_all(root).ok();
    }

    /// A journal naming a different plugin is a file that was moved, copied or
    /// planted. Acting on it would let one plugin's record decide another
    /// plugin's directory.
    #[test]
    fn a_journal_that_names_another_plugin_is_refused() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        let record = OperationRecord {
            format_version: JOURNAL_FORMAT_VERSION,
            kind: OperationKind::Install,
            plugin_id: "com.orivo.elsewhere".into(),
            incoming_version: "9.0.0".into(),
            incoming_trusted: true,
            displaced_version: None,
            displaced_trusted: false,
        };
        let journal = store.journal_path(PLUGIN);
        create_parent(&journal).unwrap();
        fs::write(&journal, serde_json::to_vec(&record).unwrap()).unwrap();

        assert_eq!(read_record(&journal, PLUGIN), None);
        assert_eq!(
            store.recover(),
            vec![RecoveryOutcome {
                plugin_id: PLUGIN.into(),
                resolution: Resolution::Abandoned,
            }]
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn removing_a_plugin_takes_its_rollback_target_with_it() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install(PLUGIN, "2.0.0", true, &files("2.0.0", "two"), &passes)
            .unwrap();
        assert!(store.rollback_target(PLUGIN).is_some());

        store.remove(PLUGIN).expect("removes");
        assert!(!store.live_directory(PLUGIN).exists());
        assert!(!store.is_trusted(PLUGIN));
        assert_eq!(store.rollback_target(PLUGIN), None);
        assert!(!is_directory(&store.previous_directory(PLUGIN)));
        fs::remove_dir_all(root).ok();
    }

    /// The recovery table is written against a crash — a sequence that stops —
    /// and two transactions interleaving their renames is a different problem
    /// it cannot answer. The gate is what keeps it out; this is what notices if
    /// the gate ever goes away.
    #[test]
    fn concurrent_installs_of_one_plugin_still_leave_one_whole_version() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .unwrap();

        let payloads = ["two", "three", "four", "five"];
        std::thread::scope(|scope| {
            for (index, payload) in payloads.iter().enumerate() {
                let store = store.clone();
                scope.spawn(move || {
                    let version = format!("2.0.{index}");
                    let _ =
                        store.install(PLUGIN, &version, true, &files(&version, payload), &passes);
                });
            }
        });

        let version = live_version(&store).expect("a plugin is live");
        assert_eq!(
            fs::read_to_string(store.live_directory(PLUGIN).join("assets/catalog.json")).unwrap(),
            format!(r#"{{"version":"{version}"}}"#),
            "the surviving tree mixes two versions"
        );
        assert!(!store.journal_path(PLUGIN).exists());
        assert!(!is_directory(&store.staged_directory(PLUGIN)));
        assert!(!is_directory(&store.discard_directory(PLUGIN)));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rolling_back_without_a_kept_version_is_refused() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .unwrap();

        assert!(
            store
                .rollback(PLUGIN)
                .is_err_and(|error| error.contains("no earlier version"))
        );
        assert_eq!(live_version(&store).as_deref(), Some("1.0.0"));
        fs::remove_dir_all(root).ok();
    }

    /// Staging scratch left by a process that died before writing a journal is
    /// swept, and the trust markers that share the directory are not.
    #[test]
    fn sweeping_staging_spares_the_trust_markers() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", true, &files("1.0.0", "one"), &passes)
            .unwrap();
        let orphan = store.staged_directory("com.orivo.other");
        fs::create_dir_all(&orphan).unwrap();

        store.recover();
        assert!(!orphan.exists());
        assert!(store.is_trusted(PLUGIN));
        fs::remove_dir_all(root).ok();
    }
}
