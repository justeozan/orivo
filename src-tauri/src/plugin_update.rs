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
//! <root>/.staging/trusted/<id>     which key signed the live tree, and its digest
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
//!
//! **Every path in this module is built from a plugin id, so no id that did not
//! pass [`valid_plugin_id`] reaches one.** A journal is a file in a directory
//! any local process can write to, and its name is where the id used to come
//! from — an id of `..` would have made `remove_dir_all` climb out of the plugin
//! root and take the whole application data directory with it.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt, fs,
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
const COMPONENT_FILE: &str = "component.wasm";
const JOURNAL_FORMAT_VERSION: u32 = 1;
const MAX_JOURNAL_BYTES: u64 = 8 * 1024;
const MAX_COMPONENT_BYTES: u64 = 64 * 1024 * 1024;

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

/// Which channel a version arrived through. Not a label: it decides whether
/// the version is updated automatically, and whether the next package is
/// allowed to replace it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PackageChannel {
    /// Signed by a key this build compiles in. The signer is named rather than
    /// implied so a key rotation is visible in the record instead of silent.
    Official { signer: String },
    /// Sideloaded. Never updated automatically, and it cannot inherit the
    /// official badge from the version it replaces.
    Development,
}

impl PackageChannel {
    pub fn is_official(&self) -> bool {
        matches!(self, Self::Official { .. })
    }

    pub fn signer(&self) -> Option<&str> {
        match self {
            Self::Official { signer } => Some(signer),
            Self::Development => None,
        }
    }
}

impl fmt::Display for PackageChannel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Official { signer } => write!(formatter, "signed by {signer}"),
            Self::Development => formatter.write_str("unsigned"),
        }
    }
}

/// What the live version of a plugin *is*, as opposed to what it calls itself.
///
/// The component digest is re-derived from the tree on every read rather than
/// remembered, so it cannot disagree with the bytes the host will actually run.
/// It exists for the consumer the plan names next: a grant, or a runner profile
/// marked valid, is only meaningful against the package it was granted to, so
/// whoever holds one needs to be told when that package stops being the same
/// package. See [`PluginStore::observe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageIdentity {
    pub plugin_id: String,
    pub version: String,
    /// SHA-256 of `component.wasm` as it sits in the live tree.
    pub component_sha256: String,
    pub channel: PackageChannel,
}

/// What an observer is told. Two cases, because "this plugin is now something
/// else" and "this plugin is gone" call for different answers from anyone
/// holding a grant against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityChange {
    /// The live version of this plugin is now `current`. Sent for an install,
    /// an update, a rollback, and a crash recovery that moved the plugin —
    /// anything that makes the running package a different package.
    ///
    /// `previous` is what it was, and it is carried rather than left for the
    /// observer to have remembered: the interesting question is not *what is
    /// installed* but *what changed*, and a rollback and an update are the same
    /// event without it.
    Activated {
        previous: Option<PackageIdentity>,
        current: PackageIdentity,
    },
    /// The plugin is no longer installed.
    Removed { plugin_id: String },
}

impl IdentityChange {
    pub fn plugin_id(&self) -> &str {
        match self {
            Self::Activated { current, .. } => &current.plugin_id,
            Self::Removed { plugin_id } => plugin_id,
        }
    }
}

/// What a change of package means for permissions already granted to the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantVerdict {
    /// The new package is the old one's successor under the same signature, so
    /// the consent chain is unbroken.
    Keep,
    /// Whatever was agreed was agreed with something else. The holder should
    /// revoke and ask again.
    Revalidate,
}

/// Whether permissions given to one package carry over to the one that
/// replaced it.
///
/// A release signature is the consent chain: the user agreed to a package the
/// key vouches for, and the key vouches for its successors too. Everything that
/// breaks that chain breaks the inheritance — and a *downgrade* breaks it as
/// surely as a different signer does, because an older build is still signed
/// and may be the one whose `validate-profile` or discovery was weaker. That is
/// the case a signature check alone cannot see, so the version is compared
/// here rather than trusted.
pub fn grant_verdict(
    previous: Option<&PackageIdentity>,
    current: &PackageIdentity,
) -> GrantVerdict {
    let Some(previous) = previous else {
        // Nothing was installed under this id, so there is nothing to inherit.
        return GrantVerdict::Keep;
    };
    if previous.component_sha256 == current.component_sha256 && previous.channel == current.channel
    {
        // The same bytes, arrived the same way. A version string that moved
        // without the component moving is not a new package.
        //
        // The channel has to match too. The same component can be live as a
        // signed package and then, after a rollback, as the sideloaded build it
        // replaced — identical code, and a permission recorded against "this is
        // signed" that no longer describes it.
        return GrantVerdict::Keep;
    }
    match (&previous.channel, &current.channel) {
        // Same signer, and forward. The only way through.
        (PackageChannel::Official { signer: was }, PackageChannel::Official { signer: now })
            if was == now && is_newer(&current.version, &previous.version) =>
        {
            GrantVerdict::Keep
        }
        // A sideloaded build has no signer to vouch for a successor, a
        // different signer is a different authority, and an older or equal
        // version under the same one is not a successor at all.
        _ => GrantVerdict::Revalidate,
    }
}

/// Strictly newer, by `(major, minor, patch)`, with a prerelease below its
/// release. An unparseable version on either side is not newer: refusing to
/// compare is the safe answer when the alternative is inheriting a permission
/// on a guess.
fn is_newer(candidate: &str, installed: &str) -> bool {
    fn key(value: &str) -> Option<(u32, u32, u32, bool, String)> {
        let (core, prerelease) = value.split_once('-').unwrap_or((value, ""));
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some((
            major,
            minor,
            patch,
            prerelease.is_empty(),
            prerelease.to_owned(),
        ))
    }
    match (key(candidate), key(installed)) {
        (Some(candidate), Some(installed)) => candidate > installed,
        _ => false,
    }
}

/// Registered from `lib.rs`, so the installer never has to know who is
/// listening. Observers are called **after** the store's lock is released, and
/// must not call back into the store.
pub type IdentityObserver = Arc<dyn Fn(&IdentityChange) + Send + Sync>;

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
    pub incoming_channel: PackageChannel,
    /// The version being displaced — the one that lands in `previous/`. `None`
    /// on a first install, where there is nothing to keep.
    pub displaced_version: Option<String>,
    pub displaced_channel: PackageChannel,
}

impl OperationRecord {
    /// A journal is a file in a directory any local process can write to, and
    /// it names the directory the recovery will move. Both halves are checked:
    /// the grammar, so no path escapes the plugin root, and the identity, so a
    /// record that was copied or planted cannot decide another plugin's fate.
    fn valid_for(&self, plugin_id: &str) -> bool {
        self.format_version == JOURNAL_FORMAT_VERSION
            && self.plugin_id == plugin_id
            && valid_plugin_id(plugin_id)
    }
}

/// The one grammar every path in this module is built from.
///
/// It is the installer's directory-name rule, and it lives here because this is
/// the module that turns an id into a `remove_dir_all`. `..`, an empty segment,
/// a separator or a `~` never gets that far.
pub fn valid_plugin_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.split('.').count() >= 3
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
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
    pub channel: PackageChannel,
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

#[derive(Clone)]
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
    observers: Arc<Mutex<Vec<IdentityObserver>>>,
}

impl fmt::Debug for PluginStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginStore")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl PluginStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            gate: Arc::new(Mutex::new(())),
            observers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Be told when a plugin becomes a different package, or stops being
    /// installed at all.
    ///
    /// The seam exists for grants and runner profiles: both are agreements with
    /// a *package*, not with an id, so the holder has to learn when the package
    /// behind the id changes. Registering from `lib.rs` keeps that dependency
    /// pointing one way — the installer never learns who is listening.
    pub fn observe(&self, observer: IdentityObserver) {
        self.observers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(observer);
    }

    /// Run `work`, then tell the observers if the live package changed.
    ///
    /// Comparing before and after is what makes this exact rather than eager: a
    /// refused install, a no-op recovery and a rollback that could not start all
    /// leave the identity alone and say nothing. The comparison and the
    /// notification both happen outside the gate, so an observer is free to be
    /// slow and cannot deadlock the store.
    fn announcing<T>(&self, plugin_id: &str, work: impl FnOnce() -> T) -> T {
        let before = self.identity(plugin_id);
        let result = work();
        let after = self.identity(plugin_id);
        if before != after {
            let change = match after {
                Some(current) => IdentityChange::Activated {
                    previous: before,
                    current,
                },
                None => IdentityChange::Removed {
                    plugin_id: plugin_id.to_owned(),
                },
            };
            let observers = self
                .observers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            for observer in observers {
                observer(&change);
            }
        }
        result
    }

    /// What the live version of this plugin is. `None` when nothing is
    /// installed, or when the tree is too broken to describe — a directory
    /// without a readable manifest and component is not a package.
    pub fn identity(&self, plugin_id: &str) -> Option<PackageIdentity> {
        if !valid_plugin_id(plugin_id) {
            return None;
        }
        let live = self.live_directory(plugin_id);
        let component_sha256 = component_digest(&live)?;
        Some(PackageIdentity {
            plugin_id: plugin_id.to_owned(),
            version: installed_version(&live)?,
            channel: self.recorded_channel(plugin_id, &component_sha256),
            component_sha256,
        })
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

    /// Host-owned state about *how* a plugin arrived, deliberately outside the
    /// plugin's own directory, where a package could otherwise declare itself
    /// trusted.
    fn trust_marker(&self, plugin_id: &str) -> PathBuf {
        self.root
            .join(STAGING_DIRECTORY)
            .join(TRUST_DIRECTORY)
            .join(plugin_id)
    }

    /// The channel of the live tree, as recorded — but only if the record still
    /// describes *these* bytes.
    ///
    /// The marker names the component digest it was written for, so it does not
    /// survive the component being swapped underneath it: dropping another
    /// `.wasm` into an installed plugin now costs the official badge instead of
    /// inheriting it. This is tamper-*evident*, not tamper-proof — a local
    /// process that can write the marker can also write the right digest into
    /// it. Making it unforgeable needs a key the user's account cannot read,
    /// which is a keychain change and not this module's.
    fn recorded_channel(&self, plugin_id: &str, component_sha256: &str) -> PackageChannel {
        let Some(bytes) = read_small(&self.trust_marker(plugin_id), MAX_JOURNAL_BYTES) else {
            return PackageChannel::Development;
        };
        let Ok(marker) = serde_json::from_slice::<TrustMarker>(&bytes) else {
            return PackageChannel::Development;
        };
        if marker.component_sha256 != component_sha256 || marker.signer.trim().is_empty() {
            return PackageChannel::Development;
        }
        PackageChannel::Official {
            signer: marker.signer,
        }
    }

    pub fn is_trusted(&self, plugin_id: &str) -> bool {
        self.identity(plugin_id)
            .is_some_and(|identity| identity.channel.is_official())
    }

    /// The channel recorded for a component the caller has *already* hashed.
    ///
    /// The runner host re-reads and re-hashes a package before it will invoke
    /// it, and then has to know whether that exact component is the one the
    /// install transaction accepted a signature for. Asking with the digest in
    /// hand is both cheaper than re-deriving it and stricter than asking about
    /// the id: a component swapped under an installed plugin answers
    /// `Development`, whatever file sits beside it.
    pub fn channel_for_component(&self, plugin_id: &str, component_sha256: &str) -> PackageChannel {
        if !valid_plugin_id(plugin_id) {
            return PackageChannel::Development;
        }
        self.recorded_channel(plugin_id, component_sha256)
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
            channel: record.displaced_channel,
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
        channel: PackageChannel,
        files: &PackageFiles,
        verify: &dyn Fn(&Path, Checkpoint) -> Result<(), String>,
    ) -> Result<InstallOutcome, String> {
        self.announcing(plugin_id, || {
            self.install_stopping_before(None, plugin_id, version, channel.clone(), files, verify)
        })
    }

    fn install_stopping_before(
        &self,
        stop: Option<Step>,
        plugin_id: &str,
        version: &str,
        channel: PackageChannel,
        files: &PackageFiles,
        verify: &dyn Fn(&Path, Checkpoint) -> Result<(), String>,
    ) -> Result<InstallOutcome, String> {
        if !valid_plugin_id(plugin_id) {
            return Err("The plugin identity is not usable as a directory.".into());
        }
        let _gate = self.enter();
        let cut = |step: Step| stop == Some(step);
        // An interrupted operation on this plugin is settled first. Starting a
        // second transaction over an unfinished one would overwrite the journal
        // that says how to undo it.
        self.recover_plugin(plugin_id);

        let live = self.live_directory(plugin_id);
        let displaced_version = installed_version(&live);
        let displaced_channel = self
            .identity(plugin_id)
            .map_or(PackageChannel::Development, |identity| identity.channel);
        // The developer channel can never take over from the official one
        // silently. A sideloaded build replacing a signed package would keep the
        // id, and with it every grant and every runner profile the user agreed
        // to for the *signed* package — so the package that arrives with less
        // provenance has to arrive through a removal the user performed.
        if !channel.is_official() && displaced_channel.is_official() {
            return Err(
                "This plugin is installed from Orivo's registry. Remove it first to replace it with an unsigned build."
                    .into(),
            );
        }

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
            incoming_channel: channel,
            displaced_version: displaced_version.clone(),
            displaced_channel,
        };
        self.write_journal(&record)?;

        // Past the journal, `?` is the wrong exit. A disk that filled up, or a
        // Windows scanner still holding a file the host has just written, would
        // leave the plugin archived and unpromoted — that is, absent — until the
        // next start, and the first read of the catalogue happens before
        // recovery does. Every failure from here restores the tree itself.
        match self.swap_and_prove(stop, &record, &staged, &live, verify) {
            Ok(()) => {}
            // A cut is not an error, it is the process ceasing to exist. It
            // leaves the disk exactly as a crash would and hands the state to
            // recovery — which is the whole thing the interruption tests check.
            Err(SwapError::Interrupted) => return Err(interrupted()),
            Err(SwapError::Failed(error)) => {
                self.undo_install(&record);
                return Err(error);
            }
        }

        if cut(Step::WriteTrust) {
            return Err(interrupted());
        }
        // Only bookkeeping is left, and the version it describes is live and
        // proved. Undoing here would throw away a good update because a marker
        // could not be written, so the failure is handed to the one code path
        // that knows how to finish an operation from what is on disk.
        if self
            .write_trust(plugin_id, &record.incoming_channel)
            .is_err()
        {
            self.recover_plugin(plugin_id);
            return Ok(self.outcome(&record));
        }

        if cut(Step::Commit) {
            return Err(interrupted());
        }
        if self.commit_install(&record).is_err() {
            self.recover_plugin(plugin_id);
        }
        Ok(self.outcome(&record))
    }

    fn outcome(&self, record: &OperationRecord) -> InstallOutcome {
        InstallOutcome {
            plugin_id: record.plugin_id.clone(),
            version: record.incoming_version.clone(),
            rollback_to: record.displaced_version.clone(),
        }
    }

    /// Archive, promote, prove. The three steps that leave the plugin root
    /// mid-transaction, kept together so there is exactly one place that
    /// decides what an error between them means.
    fn swap_and_prove(
        &self,
        stop: Option<Step>,
        record: &OperationRecord,
        staged: &Path,
        live: &Path,
        verify: &dyn Fn(&Path, Checkpoint) -> Result<(), String>,
    ) -> Result<(), SwapError> {
        let cut = |step: Step| stop == Some(step);
        let plugin_id = &record.plugin_id;
        let previous = self.previous_directory(plugin_id);
        let failed = |error: String| SwapError::Failed(error);

        if cut(Step::ArchiveLive) {
            return Err(SwapError::Interrupted);
        }
        if is_directory(live) {
            create_parent(&previous).map_err(failed)?;
            rename(live, &previous).map_err(failed)?;
        }

        if cut(Step::PromoteStaged) {
            return Err(SwapError::Interrupted);
        }
        // From here the live tree is unproven, and recovery must undo rather
        // than finish. The marker is what tells it which of the two to do.
        touch(&self.smoke_marker(plugin_id)).map_err(failed)?;
        rename(staged, live).map_err(failed)?;

        if cut(Step::SmokeTest) {
            return Err(SwapError::Interrupted);
        }
        verify(live, Checkpoint::Live).map_err(failed)?;
        let _ = fs::remove_file(self.smoke_marker(plugin_id));
        Ok(())
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
            let _ = self.write_trust(plugin_id, &record.displaced_channel);
        } else if record.displaced_version.is_none() {
            // A first install that failed leaves no plugin, and no claim that
            // one was ever signed.
            let _ = fs::remove_dir_all(&live);
            let _ = fs::remove_file(self.trust_marker(plugin_id));
        } else {
            let _ = self.write_trust(plugin_id, &record.displaced_channel);
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
        self.announcing(plugin_id, || self.rollback_stopping_before(None, plugin_id))
    }

    fn rollback_stopping_before(
        &self,
        stop: Option<Step>,
        plugin_id: &str,
    ) -> Result<RollbackTarget, String> {
        if !valid_plugin_id(plugin_id) {
            return Err("That is not a plugin Orivo installed.".into());
        }
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
            incoming_channel: target.channel.clone(),
            displaced_version: installed_version(&live),
            displaced_channel: self
                .identity(plugin_id)
                .map_or(PackageChannel::Development, |identity| identity.channel),
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
        let _ = self.write_trust(plugin_id, &target.channel);

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
        self.announcing(plugin_id, || self.remove_locked(plugin_id))
    }

    fn remove_locked(&self, plugin_id: &str) -> Result<(), String> {
        if !valid_plugin_id(plugin_id) {
            return Err("That is not a plugin Orivo installed.".into());
        }
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
        let mut outcomes = Vec::new();
        for plugin_id in self.journalled_plugin_ids() {
            let settled = self.announcing(&plugin_id, || {
                let _gate = self.enter();
                self.recover_plugin(&plugin_id)
            });
            if let Some(resolution) = settled {
                outcomes.push(RecoveryOutcome {
                    plugin_id,
                    resolution,
                });
            }
        }
        let _gate = self.enter();
        self.sweep_staging();
        outcomes
    }

    /// The plugins a journal file names — and *only* those whose name is a
    /// plugin id.
    ///
    /// The journal directory is ordinary state on disk, so its file names are
    /// as untrusted as anything else a local process can write. `...json`
    /// names the plugin `..`, which every path helper below would happily join
    /// onto the plugin root; `undo_install` would then `remove_dir_all` the
    /// whole application data directory. A name that is not an inverse-DNS id
    /// is swept, never acted on.
    fn journalled_plugin_ids(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.root.join(JOURNAL_DIRECTORY)) else {
            return Vec::new();
        };
        let mut plugin_ids = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stem = name
                .strip_suffix(".json")
                .or_else(|| name.strip_suffix(".smoke"));
            match stem {
                // Nothing here can be recovered — the name is not a plugin
                // Orivo could have installed — and leaving it would invite the
                // next reader to try again. The smoke markers go with it: one
                // on its own would send a later journal down the undo path.
                Some(stem) if !valid_plugin_id(stem) => {
                    let _ = fs::remove_file(entry.path());
                }
                Some(stem) if name.ends_with(".json") => plugin_ids.push(stem.to_owned()),
                _ => {}
            }
        }
        plugin_ids.sort();
        plugin_ids
    }

    /// Finish or undo one plugin's interrupted operation.
    ///
    /// The decision is made from what is on disk, not from a progress field: a
    /// field would have to be updated between renames, and an update that is
    /// not itself atomic reintroduces the problem it claims to solve. Which
    /// directories exist is the record, and each rename moves exactly one.
    fn recover_plugin(&self, plugin_id: &str) -> Option<Resolution> {
        if !valid_plugin_id(plugin_id) {
            return None;
        }
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
                    let _ = self.write_trust(plugin_id, &record.incoming_channel);
                    let _ = self.commit_install(&record);
                    return Some(Resolution::Committed);
                }
                // Same, with nothing displaced — a first install. Or an undo
                // that had already put the old tree back and lost only its
                // bookkeeping; `displaced_version` separates the two.
                (false, true, false) => {
                    let channel = if record.displaced_version.is_none() {
                        &record.incoming_channel
                    } else {
                        &record.displaced_channel
                    };
                    let _ = self.write_trust(plugin_id, channel);
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
                    let _ = self.write_trust(plugin_id, &record.incoming_channel);
                    self.finish_rollback(plugin_id);
                    return Some(Resolution::Committed);
                }
                // Promoted; only the marker and the trust flag were missing.
                (true, false) => {
                    let _ = self.write_trust(plugin_id, &record.incoming_channel);
                    self.finish_rollback(plugin_id);
                    return Some(Resolution::Committed);
                }
                // Neither exists. The tree on its way out is all there is.
                (false, false) => {
                    let discard = self.discard_directory(plugin_id);
                    if is_directory(&discard) {
                        let _ = rename(&discard, &self.live_directory(plugin_id));
                        let _ = self.write_trust(plugin_id, &record.displaced_channel);
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
        let _ = self.write_trust(plugin_id, &record.displaced_channel);
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

    /// Record which key signed the tree that is live *now*.
    ///
    /// The digest goes in with the signer, so the marker only speaks for the
    /// bytes it was written against; see [`PluginStore::recorded_channel`].
    fn write_trust(&self, plugin_id: &str, channel: &PackageChannel) -> Result<(), String> {
        let marker = self.trust_marker(plugin_id);
        let Some(signer) = channel.signer() else {
            let _ = fs::remove_file(&marker);
            return Ok(());
        };
        let component_sha256 = component_digest(&self.live_directory(plugin_id))
            .ok_or_else(|| "The plugin component is unavailable.".to_string())?;
        create_parent(&marker)?;
        let encoded = serde_json::to_vec(&TrustMarker {
            signer: signer.to_owned(),
            component_sha256,
        })
        .map_err(|_| "The plugin folder is unavailable.".to_string())?;
        write_durably(&marker, &encoded)
            .map_err(|_| "The plugin folder is unavailable.".to_string())
    }
}

/// What the trust marker holds. Bound to a digest so it cannot outlive the
/// component it describes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustMarker {
    signer: String,
    component_sha256: String,
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

/// SHA-256 of the component in a package tree. `None` when there is no tree, or
/// no component in it — which is also how [`PluginStore::identity`] says "this
/// is not a package".
fn component_digest(directory: &Path) -> Option<String> {
    let bytes = read_small(&directory.join(COMPONENT_FILE), MAX_COMPONENT_BYTES)?;
    Some(format!("{:x}", Sha256::digest(&bytes)))
}

/// Read a bounded regular file, following nothing. Every caller here is reading
/// host state a local process could have replaced with a link or a device.
fn read_small(path: &Path, max_bytes: u64) -> Option<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > max_bytes {
        return None;
    }
    fs::read(path).ok()
}

fn read_record(path: &Path, plugin_id: &str) -> Option<OperationRecord> {
    let record =
        serde_json::from_slice::<OperationRecord>(&read_small(path, MAX_JOURNAL_BYTES)?).ok()?;
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

/// A cut and a failure look the same to a caller and are opposites to the
/// store: one is the process ceasing to exist, which recovery is built for, and
/// the other is a process still running and able to put the tree back itself.
enum SwapError {
    Interrupted,
    Failed(String),
}

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

    fn official() -> PackageChannel {
        PackageChannel::Official {
            signer: "orivo-release-v1".into(),
        }
    }

    fn development() -> PackageChannel {
        PackageChannel::Development
    }

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
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
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
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();

        let outcome = store
            .install(PLUGIN, "2.0.0", official(), &files("2.0.0", "two"), &passes)
            .expect("updates");
        assert_eq!(outcome.rollback_to.as_deref(), Some("1.0.0"));
        assert_eq!(live_payload(&store).as_deref(), Some("two"));
        assert_eq!(
            store.rollback_target(PLUGIN),
            Some(RollbackTarget {
                version: "1.0.0".into(),
                channel: official(),
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
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();

        let refusal = store
            .install(
                PLUGIN,
                "2.0.0",
                official(),
                &files("2.0.0", "two"),
                &refuses,
            )
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
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install(PLUGIN, "2.0.0", official(), &files("2.0.0", "two"), &passes)
            .unwrap();
        assert!(store.rollback_target(PLUGIN).is_some());

        let refusal = store.install(
            PLUGIN,
            "3.0.0",
            official(),
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
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install(PLUGIN, "2.0.0", official(), &files("2.0.0", "two"), &passes)
            .unwrap();

        store
            .install(
                PLUGIN,
                "3.0.0",
                official(),
                &files("3.0.0", "three"),
                &refuses,
            )
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
            .install(
                PLUGIN,
                "1.0.0",
                official(),
                &files("1.0.0", "one"),
                &refuses,
            )
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
            .install(
                PLUGIN,
                "1.0.0",
                development(),
                &files("1.0.0", "one"),
                &passes,
            )
            .unwrap();
        assert!(!store.is_trusted(PLUGIN));

        store
            .install(PLUGIN, "2.0.0", official(), &files("2.0.0", "two"), &passes)
            .unwrap();
        assert!(store.is_trusted(PLUGIN));

        // The other direction is refused outright; see
        // `an_unsigned_build_cannot_replace_a_signed_one`.
        assert!(
            store
                .install(
                    PLUGIN,
                    "3.0.0",
                    development(),
                    &files("3.0.0", "three"),
                    &passes
                )
                .is_err()
        );
        assert!(store.is_trusted(PLUGIN));

        // A rollback restores the badge the kept version arrived with — which
        // here means going *back* to an unsigned build. That is deliberate: it
        // is a version the user had, and the identity change is announced, so
        // anything holding a grant against the signed package is told.
        store.rollback(PLUGIN).unwrap();
        assert_eq!(live_version(&store).as_deref(), Some("1.0.0"));
        assert!(!store.is_trusted(PLUGIN));
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
                .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
                .unwrap();

            let cut = store.install_stopping_before(
                Some(step),
                PLUGIN,
                "2.0.0",
                official(),
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
                    official(),
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
                .install(
                    PLUGIN,
                    "1.0.0",
                    development(),
                    &files("1.0.0", "one"),
                    &passes,
                )
                .unwrap();
            store
                .install(PLUGIN, "2.0.0", official(), &files("2.0.0", "two"), &passes)
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
                payload.as_deref() == Some("two"),
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
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install_stopping_before(
                Some(Step::PromoteStaged),
                PLUGIN,
                "2.0.0",
                official(),
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
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install_stopping_before(
                Some(Step::SmokeTest),
                PLUGIN,
                "2.0.0",
                official(),
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
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install_stopping_before(
                Some(Step::PromoteStaged),
                PLUGIN,
                "2.0.0",
                official(),
                &files("2.0.0", "two"),
                &passes,
            )
            .expect_err("interrupted");

        // No `recover()` call: the next install has to do it, or it would
        // overwrite the journal that says how to undo the last one.
        store
            .install(
                PLUGIN,
                "3.0.0",
                official(),
                &files("3.0.0", "three"),
                &passes,
            )
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
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
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
            incoming_channel: official(),
            displaced_version: None,
            displaced_channel: development(),
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
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install(PLUGIN, "2.0.0", official(), &files("2.0.0", "two"), &passes)
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
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();

        let payloads = ["two", "three", "four", "five"];
        std::thread::scope(|scope| {
            for (index, payload) in payloads.iter().enumerate() {
                let store = store.clone();
                scope.spawn(move || {
                    let version = format!("2.0.{index}");
                    let _ = store.install(
                        PLUGIN,
                        &version,
                        official(),
                        &files(&version, payload),
                        &passes,
                    );
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

    /// The grammar every path in this module is built from. It moved here from
    /// the installer because this is the module that turns an id into a
    /// `remove_dir_all`, and a rule enforced somewhere else is a rule the
    /// dangerous caller does not have.
    #[test]
    fn an_id_that_is_not_a_plugin_id_never_becomes_a_path() {
        assert!(valid_plugin_id("com.orivo.quiky"));
        assert!(!valid_plugin_id(""));
        assert!(!valid_plugin_id(".."));
        assert!(!valid_plugin_id("../../etc"));
        assert!(!valid_plugin_id("quiky"));
        assert!(!valid_plugin_id("com.Orivo.quiky"));
        assert!(!valid_plugin_id("com.orivo.quiky~new"));
        assert!(!valid_plugin_id("com/orivo/quiky"));
        assert!(!valid_plugin_id("com.orivo."));
    }

    /// A journal file is a file in a directory any local process can write to,
    /// and its *name* is where recovery used to learn which plugin it was
    /// about. `...json` names the plugin `..`, whose live directory is the
    /// application data directory — and `undo_install` removes that directory.
    ///
    /// The test plants exactly that: a journal whose name and whose `pluginId`
    /// both say `..`, and the smoke marker that sends recovery down the
    /// undo path. Then it asserts the user's catalogue, their preferences and
    /// the plugin root are all still there.
    #[test]
    fn a_planted_journal_cannot_name_a_directory_outside_the_plugin_root() {
        let app_data = temporary_root();
        let plugin_root = app_data.join("plugins");
        fs::create_dir_all(&plugin_root).unwrap();
        let store = PluginStore::new(plugin_root.clone());
        store
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();

        // The user's own data, sitting where the plugin root's parent is.
        fs::write(app_data.join("catalog.json"), b"{}").unwrap();
        fs::write(app_data.join("preferences.json"), b"{}").unwrap();

        for escape in ["..", "", "../..", "/"] {
            let record = OperationRecord {
                format_version: JOURNAL_FORMAT_VERSION,
                kind: OperationKind::Install,
                plugin_id: escape.to_string(),
                incoming_version: "9.9.9".into(),
                incoming_channel: development(),
                displaced_version: Some("9.9.8".into()),
                displaced_channel: development(),
            };
            let journal_directory = plugin_root.join(JOURNAL_DIRECTORY);
            fs::create_dir_all(&journal_directory).unwrap();
            // `/` and `../..` do not even land in the journal directory; the
            // names that matter are the ones that do and still escape.
            let _ = fs::write(
                journal_directory.join(format!("{escape}.json")),
                serde_json::to_vec(&record).unwrap(),
            );
            let _ = fs::write(journal_directory.join(format!("{escape}.smoke")), b"");

            store.recover();

            assert!(
                app_data.join("catalog.json").is_file(),
                "{escape:?} took the library with it"
            );
            assert!(
                app_data.join("preferences.json").is_file(),
                "{escape:?} took the preferences with it"
            );
            assert!(plugin_root.is_dir(), "{escape:?} took the plugin root");
            assert_eq!(live_payload(&store).as_deref(), Some("one"), "{escape:?}");
            // And the inner guard, reached directly: a caller that already
            // holds an id is held to the same grammar as one that read it off a
            // file name.
            assert_eq!(store.recover_plugin(escape), None, "{escape:?}");
            assert!(app_data.join("catalog.json").is_file(), "{escape:?}");

            let leftovers = fs::read_dir(plugin_root.join(JOURNAL_DIRECTORY))
                .map(|entries| entries.filter_map(Result::ok).count())
                .unwrap_or(0);
            assert_eq!(
                leftovers, 0,
                "{escape:?}: a journal was left in place to be replayed"
            );
        }
        fs::remove_dir_all(app_data).ok();
    }

    /// The developer channel can never take over from the official one without
    /// the user removing the plugin first. A sideloaded build keeping the id
    /// would keep every grant and every runner profile agreed to for the
    /// *signed* package.
    #[test]
    fn an_unsigned_build_cannot_replace_a_signed_one() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();

        let refusal = store
            .install(
                PLUGIN,
                "2.0.0",
                development(),
                &files("2.0.0", "two"),
                &passes,
            )
            .expect_err("an unsigned build cannot take over");
        assert!(refusal.contains("Remove it first"), "{refusal}");
        assert_eq!(live_payload(&store).as_deref(), Some("one"));
        assert!(store.is_trusted(PLUGIN));

        // Removing it is the door, and it works.
        store.remove(PLUGIN).unwrap();
        store
            .install(
                PLUGIN,
                "2.0.0",
                development(),
                &files("2.0.0", "two"),
                &passes,
            )
            .expect("installs once the signed one is gone");
        assert!(!store.is_trusted(PLUGIN));
        fs::remove_dir_all(root).ok();
    }

    /// The marker names the digest it was written for, so swapping the
    /// component underneath an installed plugin costs the official badge
    /// instead of inheriting it.
    #[test]
    fn the_trust_marker_does_not_survive_the_component_it_describes() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();
        assert_eq!(
            store.identity(PLUGIN).unwrap().channel.signer(),
            Some("orivo-release-v1")
        );

        fs::write(
            store.live_directory(PLUGIN).join("component.wasm"),
            b"something else",
        )
        .unwrap();

        assert!(!store.is_trusted(PLUGIN));
        assert_eq!(store.identity(PLUGIN).unwrap().channel, development());
        fs::remove_dir_all(root).ok();
    }

    /// The rule the junction turns on, branch by branch.
    ///
    /// A release signature is the consent chain: the user agreed to a package
    /// the key vouches for, and the key vouches for its successors. Everything
    /// that breaks the chain breaks the inheritance — and a *downgrade* breaks
    /// it as surely as a different signer does, which is the case a signature
    /// check alone cannot see. That branch has no end-to-end test because the
    /// reference fixture reports exactly one version and a package claiming
    /// another fails the identity probe, so it is pinned here.
    #[test]
    fn permissions_follow_a_package_only_forward_and_only_under_one_signer() {
        fn identity(version: &str, digest: &str, channel: PackageChannel) -> PackageIdentity {
            PackageIdentity {
                plugin_id: PLUGIN.into(),
                version: version.into(),
                component_sha256: digest.into(),
                channel,
            }
        }
        let signed = |signer: &str| PackageChannel::Official {
            signer: signer.into(),
        };
        let one = identity("1.0.0", "aaaa", signed("orivo-release-v1"));

        // Nothing was installed under this id, so there is nothing to inherit.
        assert_eq!(grant_verdict(None, &one), GrantVerdict::Keep);
        // The same package, unchanged.
        assert_eq!(grant_verdict(Some(&one), &one), GrantVerdict::Keep);
        // Forward, under the same signature: the chain the signature exists for.
        assert_eq!(
            grant_verdict(
                Some(&one),
                &identity("1.0.1", "bbbb", signed("orivo-release-v1"))
            ),
            GrantVerdict::Keep
        );

        for (label, current) in [
            // A rollback. Still signed, still the same signer, and not a
            // successor — the build whose discovery or `validate-profile` was
            // weaker is exactly the one a rollback reaches.
            (
                "older",
                identity("0.9.0", "bbbb", signed("orivo-release-v1")),
            ),
            // A re-release under a version already consented to.
            (
                "same version, other bytes",
                identity("1.0.0", "bbbb", signed("orivo-release-v1")),
            ),
            // Another authority entirely.
            (
                "another signer",
                identity("1.0.1", "bbbb", signed("orivo-release-v2")),
            ),
            // Nobody vouches for a sideloaded build's successor.
            (
                "unsigned",
                identity("1.0.1", "bbbb", PackageChannel::Development),
            ),
            // The same bytes, arrived a different way: a permission recorded
            // against "this is signed" no longer describes it.
            (
                "same bytes, unsigned",
                identity("1.0.0", "aaaa", PackageChannel::Development),
            ),
        ] {
            assert_eq!(
                grant_verdict(Some(&one), &current),
                GrantVerdict::Revalidate,
                "{label}"
            );
        }

        // And nothing an unsigned build was allowed transfers, in any
        // direction: it has no signer to answer for a new version.
        let unsigned = identity("1.0.0", "aaaa", PackageChannel::Development);
        assert_eq!(
            grant_verdict(
                Some(&unsigned),
                &identity("1.0.1", "bbbb", PackageChannel::Development)
            ),
            GrantVerdict::Revalidate
        );
        assert_eq!(
            grant_verdict(
                Some(&unsigned),
                &identity("1.0.1", "bbbb", signed("orivo-release-v1"))
            ),
            GrantVerdict::Revalidate
        );
    }

    /// The seam E2 plugs into. A grant is an agreement with a package, so the
    /// holder has to be told every time the package behind an id changes — and
    /// told *nothing* when it does not, or the notification means nothing.
    #[test]
    fn every_change_of_package_is_announced_once_and_no_non_change_is() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        let seen: Arc<Mutex<Vec<IdentityChange>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        store.observe(Arc::new(move |change: &IdentityChange| {
            recorder.lock().unwrap().push(change.clone());
        }));

        store
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();
        store
            .install(PLUGIN, "2.0.0", official(), &files("2.0.0", "two"), &passes)
            .unwrap();
        // Refused in staging: nothing changed, so nothing is announced.
        store
            .install(
                PLUGIN,
                "3.0.0",
                official(),
                &files("3.0.0", "three"),
                &|_directory, _checkpoint| Err("no".into()),
            )
            .unwrap_err();
        store.rollback(PLUGIN).unwrap();
        store.remove(PLUGIN).unwrap();

        let seen = seen.lock().unwrap().clone();
        let versions = seen
            .iter()
            .map(|change| match change {
                IdentityChange::Activated { current, .. } => current.version.clone(),
                IdentityChange::Removed { .. } => "removed".to_string(),
            })
            .collect::<Vec<_>>();
        assert_eq!(versions, vec!["1.0.0", "2.0.0", "1.0.0", "removed"]);
        // The identity carries what a grant is actually held against.
        let IdentityChange::Activated {
            previous,
            current: first,
        } = &seen[0]
        else {
            panic!("the first change is an activation");
        };
        assert_eq!(previous, &None, "a first install displaces nothing");
        assert_eq!(first.plugin_id, PLUGIN);
        assert_eq!(first.component_sha256.len(), 64);
        assert!(first.channel.is_official());
        fs::remove_dir_all(root).ok();
    }

    /// After the live tree has been archived, an ordinary I/O failure is not a
    /// crash — the process is still running and can put it back. Leaving it for
    /// the next start would mean the plugin is *absent* in the meantime, and the
    /// catalogue is read before recovery runs.
    ///
    /// A directory where the smoke marker belongs makes the write fail at
    /// exactly that point: after the archive rename, before the promotion.
    #[test]
    fn an_io_failure_after_the_archive_rename_puts_the_tree_back_at_once() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();

        // `write_durably` opens this path with `File::create`, which cannot
        // truncate a directory.
        fs::create_dir_all(store.smoke_marker(PLUGIN)).unwrap();
        store
            .install(PLUGIN, "2.0.0", official(), &files("2.0.0", "two"), &passes)
            .expect_err("the marker cannot be written");

        // No recovery pass: the version that was working is back already.
        assert_eq!(live_payload(&store).as_deref(), Some("one"));
        assert!(store.is_trusted(PLUGIN));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rolling_back_without_a_kept_version_is_refused() {
        let root = temporary_root();
        let store = PluginStore::new(root.clone());
        store
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
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
            .install(PLUGIN, "1.0.0", official(), &files("1.0.0", "one"), &passes)
            .unwrap();
        let orphan = store.staged_directory("com.orivo.other");
        fs::create_dir_all(&orphan).unwrap();

        store.recover();
        assert!(!orphan.exists());
        assert!(store.is_trusted(PLUGIN));
        fs::remove_dir_all(root).ok();
    }
}
