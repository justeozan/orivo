//! Trusted native host adapter for Orivo's Winlator runner on Android.
//!
//! Winlator is a separate Android application (Box64 + Wine + DXVK/Turnip), so
//! unlike the Wine-Staging adapter this host owns no engine, no prefix and no
//! process. What it owns is the same boundary: the WIT runner contract carries
//! only opaque profile and game identifiers, and this module resolves them
//! through private catalog data, rechecks every filesystem boundary, then
//! builds one closed, explicit Android intent. There is no shell, no
//! `Runtime.exec`, and no command string anywhere in the path.
//!
//! Finding the shortcut is a second boundary, and it has two shapes. On a
//! device Orivo reads Winlator's export folder through a storage access
//! grant — no manifest permission, a `ContentResolver` instead of a path —
//! while a readable directory is what every desktop build and every test here
//! uses. Both arrive as a [`WinlatorShortcutSource`], so the bounded walk, the
//! scope checks, the size cap and the fingerprint are written once and the
//! platform only decides where the bytes come from.
//!
//! What Winlator's side of the contract actually is, and why the shapes below
//! look the way they do, is recorded in `docs/winlator-runner.md`.

pub use crate::catalog::WINLATOR_RUNNER_ID;
use crate::catalog::{WinlatorDistribution, WinlatorProfile, WinlatorShortcutInventoryEntry};
use crate::winlator_saf::{DocumentTree, DocumentTreeGrant};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// A frontend shortcut directory holds one small text file per game plus the
/// two notes Winlator writes beside them. These bounds are an order of
/// magnitude above any real library and still refuse a directory that was
/// pointed at the whole of shared storage by mistake.
pub const DEFAULT_MAX_SCAN_FILES: usize = 4_000;
pub const DEFAULT_MAX_SCAN_DEPTH: usize = 4;
pub const MAX_PAGE_SIZE: usize = 100;
/// Winlator's exported `.desktop` files are a handful of `key=value` lines. A
/// file larger than this is not one, and is refused rather than parsed.
const MAX_SHORTCUT_BYTES: u64 = 64 * 1024;
const MAX_SHORTCUT_TITLE_CHARS: usize = 160;
/// What a granted folder is called when its own name cannot be shown.
const DEFAULT_ROOT_LABEL: &str = "Authorized shortcut folder";

/// The default directory Winlator Cmod writes an exported frontend shortcut
/// into when the user has not chosen another one. It is shared storage, which
/// is precisely why Orivo can read it and the container behind it cannot be
/// read.
pub const DEFAULT_FRONTEND_SHORTCUT_DIRECTORY: &str =
    "/storage/emulated/0/Download/Winlator/Frontend";

/// `FLAG_ACTIVITY_NEW_TASK | FLAG_ACTIVITY_CLEAR_TASK | FLAG_ACTIVITY_CLEAR_TOP`.
///
/// This mirrors the flag set Winlator Cmod itself prints for frontends, minus
/// `FLAG_ACTIVITY_NO_HISTORY`: that flag finishes the activity as soon as it
/// stops, which would end a running game the moment the player switched away
/// from it.
const INTENT_FLAGS: i32 = 0x1000_0000 | 0x0000_8000 | 0x0400_0000;

/// The host-only equivalent of WIT's `launch-intent`. Its closed mode enum
/// means the WIT `mode` string can never become an Android component, an extra
/// key, or a process argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinlatorLaunchIntent {
    runner_id: String,
    profile_id: String,
    game_ref: String,
    mode: WinlatorLaunchMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinlatorLaunchMode {
    /// Start one shortcut Winlator exported for a frontend. This is the only
    /// launch mode any published Winlator build exposes to another app.
    ExportedShortcut,
}

impl WinlatorLaunchIntent {
    pub fn new(profile_id: &str, game_ref: &str) -> Result<Self, WinlatorRunnerError> {
        if !valid_opaque_id(profile_id) || !valid_opaque_id(game_ref) {
            return Err(WinlatorRunnerError::InvalidIntent);
        }
        Ok(Self {
            runner_id: WINLATOR_RUNNER_ID.into(),
            profile_id: profile_id.into(),
            game_ref: game_ref.into(),
            mode: WinlatorLaunchMode::ExportedShortcut,
        })
    }

    pub fn runner_id(&self) -> &str {
        &self.runner_id
    }

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn game_ref(&self) -> &str {
        &self.game_ref
    }

    pub fn mode(&self) -> WinlatorLaunchMode {
        self.mode
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinlatorRunnerError {
    Cancelled,
    PlatformUnsupported,
    DistributionNotLaunchable,
    ProfileDisabled,
    InvalidProfile,
    ShortcutMissing,
    ShortcutOutsideScope,
    ShortcutNotLaunchable,
    AccessDenied,
    TooManyFiles,
    ShortcutTooLarge,
    InvalidIntent,
    InvalidPage,
    /// The three outcomes below are only ever produced by the Android intent
    /// layer. They still carry their own sentence on every platform, so a
    /// desktop build cannot drift out of sync with the message a device shows.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    WinlatorMissing,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    WinlatorRefusedLaunch,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    LaunchFailed,
    /// The three below belong to the storage access grant. Only the first is
    /// reachable off a device — it is the answer to a folder Orivo cannot turn
    /// into a path — but all three carry their sentence everywhere, so a desktop
    /// build cannot drift out of sync with what a device says.
    ExportFolderUnsupported,
    ExportFolderTooBroad,
    ExportFolderNotConnected,
    ExportFolderAccessLost,
}

impl std::fmt::Display for WinlatorRunnerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Cancelled => "Winlator import was cancelled.",
            Self::PlatformUnsupported => "Winlator games can only be started on Android.",
            Self::DistributionNotLaunchable => {
                "This Winlator build does not let another app start a game. Use a Winlator Cmod build, or start the game from Winlator itself."
            }
            Self::ProfileDisabled => "This Winlator profile is disabled. Enable it and try again.",
            Self::InvalidProfile => {
                "This Winlator profile is no longer valid. Review its setup and try again."
            }
            Self::ShortcutMissing => "This Winlator shortcut is no longer available.",
            Self::ShortcutOutsideScope => {
                "This shortcut is outside the folders allowed for its Winlator profile."
            }
            Self::ShortcutNotLaunchable => {
                "This Winlator shortcut changed. Export it again from Winlator so Orivo can pick it up."
            }
            Self::AccessDenied => {
                "Orivo could not read one of the folders allowed for this Winlator profile."
            }
            Self::TooManyFiles => {
                "This folder contains too many files to scan at once. Choose the folder Winlator exports its shortcuts into."
            }
            Self::ShortcutTooLarge => {
                "This is too large to be a Winlator shortcut. Choose the folder Winlator exports its shortcuts into."
            }
            Self::InvalidIntent => "This Winlator launch request is invalid.",
            Self::InvalidPage => "This Winlator import page is no longer available.",
            Self::WinlatorMissing => {
                "Winlator is not installed on this device. Install it, export a shortcut from it, and try again."
            }
            Self::WinlatorRefusedLaunch => {
                "Winlator refused the launch request. This build does not allow another app to start a game."
            }
            Self::LaunchFailed => "Winlator could not start this game. Try again.",
            Self::ExportFolderUnsupported => {
                "Orivo can only read a folder in this device's own storage. Choose the folder Winlator exports its shortcuts into."
            }
            // The one sentence that names a path, because it is the one the
            // user has to go and find. It is the constant, not a copy of it.
            Self::ExportFolderTooBroad => {
                return write!(
                    formatter,
                    "That folder is one every app can drop files into. Choose the folder Winlator exports its shortcuts into — {DEFAULT_FRONTEND_SHORTCUT_DIRECTORY} by default."
                );
            }
            Self::ExportFolderNotConnected => {
                "Connect the folder Winlator exports its shortcuts into, then try again."
            }
            Self::ExportFolderAccessLost => {
                "Orivo no longer has access to the folder Winlator exports into. Connect it again."
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for WinlatorRunnerError {}

/// One Android component Orivo is willing to address, and the extra keys that
/// component documents. Every string here is a compile-time constant read from
/// the distribution's own manifest and source: no value on this table can ever
/// come from the WebView, the catalog, or a shortcut file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WinlatorLaunchSurface {
    package: &'static str,
    activity: &'static str,
    shortcut_path_extra: &'static str,
    shortcut_name_extra: &'static str,
    container_id_extra: &'static str,
}

/// Resolve the exported launch surface for a distribution, or refuse.
///
/// `Official` resolves to nothing on purpose. In brunodev85's build only
/// `com.winlator.MainActivity` is exported, and it has no game-launch entry
/// point; its `XServerDisplayActivity` is `exported="false"`, so an intent
/// aimed at it would be rejected by the platform rather than start a game.
fn launch_surface(
    distribution: WinlatorDistribution,
) -> Result<WinlatorLaunchSurface, WinlatorRunnerError> {
    match distribution {
        WinlatorDistribution::Cmod => Ok(WinlatorLaunchSurface {
            package: "com.winlator.cmod",
            activity: "com.winlator.cmod.XServerDisplayActivity",
            shortcut_path_extra: "shortcut_path",
            shortcut_name_extra: "shortcut_name",
            container_id_extra: "container_id",
        }),
        WinlatorDistribution::Official => Err(WinlatorRunnerError::DistributionNotLaunchable),
    }
}

/// A fully resolved Android intent, with no room for a free-form command.
///
/// Keys are `&'static str` taken from [`WinlatorLaunchSurface`]; values are
/// either a bounded integer or a string the host itself produced from a
/// canonicalised, scope-checked, content-verified file. Building one is pure,
/// which is what lets `cargo test` assert the exact intent on macOS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidIntent {
    package: &'static str,
    activity: &'static str,
    flags: i32,
    extras: Vec<AndroidIntentExtra>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AndroidIntentExtra {
    Int { key: &'static str, value: i32 },
    Text { key: &'static str, value: String },
}

/// These accessors are read by the Android intent layer and by the host tests
/// that assert the exact component and extras. A desktop build links neither,
/// which is the only reason this needs an allowance.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
impl AndroidIntent {
    pub fn package(&self) -> &str {
        self.package
    }

    pub fn activity(&self) -> &str {
        self.activity
    }

    pub fn flags(&self) -> i32 {
        self.flags
    }

    pub fn extras(&self) -> &[AndroidIntentExtra] {
        &self.extras
    }
}

/// Where a profile's exported shortcuts are read from.
///
/// Two grants, one pipeline. A readable directory canonicalises and opens a
/// pathname; a storage access grant resolves a document identifier and opens a
/// stream. Everything above this trait — the bounded walk, the size cap, the
/// title, the fingerprint, the intent — cannot tell them apart, which is why the
/// SAF path is exercised by the same tests and not by a parallel one.
pub trait WinlatorShortcutSource {
    /// The granted folders, labelled for display. Each is also the boundary a
    /// resolved shortcut is checked against.
    fn roots(&self) -> Result<Vec<ShortcutRoot>, WinlatorRunnerError>;

    /// One directory's immediate children, in no particular order.
    fn entries(&self, directory: &Path) -> Result<Vec<ShortcutEntry>, WinlatorRunnerError>;

    /// Turn a candidate into the identity the host will store and hand to
    /// Winlator, refusing anything the profile did not grant.
    fn resolve(&self, shortcut: &Path) -> Result<PathBuf, WinlatorRunnerError>;

    /// Read one shortcut, bounded by [`MAX_SHORTCUT_BYTES`].
    fn read(&self, shortcut: &Path) -> Result<Vec<u8>, WinlatorRunnerError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutRoot {
    pub directory: PathBuf,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutEntry {
    pub path: PathBuf,
    pub kind: ShortcutEntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutEntryKind {
    Directory,
    File,
    /// Counted against the scan's budget and never opened: a symlink, a device
    /// node, or a provider row that did not belong to the folder being listed.
    Ignored,
}

impl ShortcutEntry {
    fn ignored() -> Self {
        // The walker matches on the kind before it ever looks at the path, so an
        // ignored entry deliberately carries none.
        Self {
            path: PathBuf::new(),
            kind: ShortcutEntryKind::Ignored,
        }
    }
}

/// A grant that is an ordinary readable directory.
pub struct FilesystemShortcuts<'a> {
    directories: &'a [PathBuf],
}

impl<'a> FilesystemShortcuts<'a> {
    pub fn for_profile(profile: &'a WinlatorProfile) -> Self {
        Self {
            directories: &profile.shortcut_directories,
        }
    }
}

impl WinlatorShortcutSource for FilesystemShortcuts<'_> {
    fn roots(&self) -> Result<Vec<ShortcutRoot>, WinlatorRunnerError> {
        let mut roots = Vec::new();
        for directory in self.directories {
            let directory =
                fs::canonicalize(directory).map_err(|_| WinlatorRunnerError::AccessDenied)?;
            if !directory.is_dir() {
                return Err(WinlatorRunnerError::AccessDenied);
            }
            roots.push(ShortcutRoot {
                label: safe_label(&directory, DEFAULT_ROOT_LABEL),
                directory,
            });
        }
        Ok(roots)
    }

    fn entries(&self, directory: &Path) -> Result<Vec<ShortcutEntry>, WinlatorRunnerError> {
        let listing = fs::read_dir(directory).map_err(|_| WinlatorRunnerError::AccessDenied)?;
        let mut entries = Vec::new();
        for entry in listing {
            let entry = entry.map_err(|_| WinlatorRunnerError::AccessDenied)?;
            let file_type = entry
                .file_type()
                .map_err(|_| WinlatorRunnerError::AccessDenied)?;
            // Never traverse a symlink: the canonicalisation in `resolve` is a
            // second line of defence for files and overlapping granted roots.
            let kind = if file_type.is_symlink() {
                ShortcutEntryKind::Ignored
            } else if file_type.is_dir() {
                ShortcutEntryKind::Directory
            } else if file_type.is_file() {
                ShortcutEntryKind::File
            } else {
                ShortcutEntryKind::Ignored
            };
            entries.push(ShortcutEntry {
                path: entry.path(),
                kind,
            });
        }
        Ok(entries)
    }

    fn resolve(&self, shortcut: &Path) -> Result<PathBuf, WinlatorRunnerError> {
        let shortcut =
            fs::canonicalize(shortcut).map_err(|_| WinlatorRunnerError::ShortcutMissing)?;
        if !shortcut.is_file() || !is_winlator_shortcut(&shortcut) {
            return Err(WinlatorRunnerError::ShortcutMissing);
        }
        if !belongs_to_grant(&shortcut, self.directories)? {
            return Err(WinlatorRunnerError::ShortcutOutsideScope);
        }
        Ok(shortcut)
    }

    fn read(&self, shortcut: &Path) -> Result<Vec<u8>, WinlatorRunnerError> {
        read_shortcut_bytes(shortcut)
    }
}

/// A grant the user handed over with `ACTION_OPEN_DOCUMENT_TREE`.
///
/// Every shortcut is checked twice here, because the two halves of a SAF grant
/// can disagree: once as a document identifier that has to sit under the granted
/// tree, and once as a path that has to sit under the directory that tree stands
/// for. The first stops a provider from answering with somebody else's row; the
/// second stops an identifier that resolves outside the folder — a `..` in the
/// middle of it — from becoming the path Orivo hands to Winlator.
pub struct DocumentTreeShortcuts {
    trees: Vec<GrantedTree>,
}

struct GrantedTree {
    grant: DocumentTreeGrant,
    tree: Box<dyn DocumentTree>,
}

impl DocumentTreeShortcuts {
    pub fn new() -> Self {
        Self { trees: Vec::new() }
    }

    pub fn with_tree(mut self, grant: DocumentTreeGrant, tree: Box<dyn DocumentTree>) -> Self {
        self.trees.push(GrantedTree { grant, tree });
        self
    }

    /// The granted tree a path belongs to, and the identifier that names it
    /// there. A path no grant covers never becomes a document.
    fn locate(&self, path: &Path) -> Result<(&GrantedTree, String), WinlatorRunnerError> {
        if self.trees.is_empty() {
            return Err(WinlatorRunnerError::ExportFolderNotConnected);
        }
        self.trees
            .iter()
            .find_map(|granted| {
                granted
                    .grant
                    .document_id_for(path)
                    .ok()
                    .map(|document_id| (granted, document_id))
            })
            .ok_or(WinlatorRunnerError::ShortcutOutsideScope)
    }
}

impl WinlatorShortcutSource for DocumentTreeShortcuts {
    fn roots(&self) -> Result<Vec<ShortcutRoot>, WinlatorRunnerError> {
        if self.trees.is_empty() {
            return Err(WinlatorRunnerError::ExportFolderNotConnected);
        }
        Ok(self
            .trees
            .iter()
            .map(|granted| ShortcutRoot {
                directory: granted.grant.directory().to_path_buf(),
                label: safe_label(granted.grant.directory(), DEFAULT_ROOT_LABEL),
            })
            .collect())
    }

    fn entries(&self, directory: &Path) -> Result<Vec<ShortcutEntry>, WinlatorRunnerError> {
        // The granted folder is the one path with no identifier *inside* the
        // tree, because it is the tree.
        let (granted, document_id) = match self
            .trees
            .iter()
            .find(|granted| granted.grant.directory() == directory)
        {
            Some(granted) => (granted, granted.grant.tree_document_id().to_string()),
            None => self.locate(directory)?,
        };
        Ok(granted
            .tree
            .children(&document_id)?
            .into_iter()
            .map(|row| {
                match granted.grant.path_for(&row.document_id) {
                    // A provider is free to answer a listing with any row at
                    // all, so a row that does not resolve to a child of the
                    // folder being listed is dropped rather than followed.
                    Ok(path) if path.parent() == Some(directory) => ShortcutEntry {
                        path,
                        kind: if row.is_directory() {
                            ShortcutEntryKind::Directory
                        } else {
                            ShortcutEntryKind::File
                        },
                    },
                    _ => ShortcutEntry::ignored(),
                }
            })
            .collect())
    }

    fn resolve(&self, shortcut: &Path) -> Result<PathBuf, WinlatorRunnerError> {
        self.locate(shortcut)?;
        if !is_winlator_shortcut(shortcut) {
            return Err(WinlatorRunnerError::ShortcutMissing);
        }
        // There is no canonicalisation and no existence probe here on purpose: a
        // path Orivo reaches only through SAF cannot be `stat`ed, and the read
        // that follows is the only honest answer about whether it is still there.
        Ok(shortcut.to_path_buf())
    }

    fn read(&self, shortcut: &Path) -> Result<Vec<u8>, WinlatorRunnerError> {
        let (granted, document_id) = self.locate(shortcut)?;
        granted.tree.read(&document_id, MAX_SHORTCUT_BYTES)
    }
}

/// The source a profile is read through on this platform.
///
/// On a device a profile that carries a storage access grant is read through it,
/// and the grant has to still be persisted: a permission the user revoked in the
/// system settings is a sentence asking them to reconnect, never a silent empty
/// library. Everything else — every desktop build, and a device profile granted
/// a plainly readable directory — reads the filesystem.
pub fn shortcut_source_for_profile(
    profile: &WinlatorProfile,
) -> Result<Box<dyn WinlatorShortcutSource + '_>, WinlatorRunnerError> {
    if profile.shortcut_trees.is_empty() {
        return Ok(Box::new(FilesystemShortcuts::for_profile(profile)));
    }
    Ok(Box::new(document_tree_source(profile)?))
}

fn document_tree_source(
    profile: &WinlatorProfile,
) -> Result<DocumentTreeShortcuts, WinlatorRunnerError> {
    document_tree_source_with(
        profile,
        &crate::winlator_saf::external_storage_root()?,
        &crate::winlator_saf::persisted_read_tree_uris()?,
        crate::winlator_saf::document_tree,
    )
}

/// The decisions above, with the device's three answers passed in: where the
/// shared volume is mounted, which grants survived, and what reads a tree. Only
/// this shape can be exercised without a device, and the checks are the point.
fn document_tree_source_with(
    profile: &WinlatorProfile,
    external_storage_root: &Path,
    persisted: &[String],
    reader: impl Fn(&str) -> Box<dyn DocumentTree>,
) -> Result<DocumentTreeShortcuts, WinlatorRunnerError> {
    let mut source = DocumentTreeShortcuts::new();
    for tree_uri in &profile.shortcut_trees {
        // A permission the user revoked from the system settings simply stops
        // being listed. Asking for it back is the only honest answer; reading the
        // folder by pathname instead would be reaching around the grant.
        if !persisted.iter().any(|granted| granted == tree_uri) {
            return Err(WinlatorRunnerError::ExportFolderAccessLost);
        }
        let grant = DocumentTreeGrant::parse(tree_uri, external_storage_root)?;
        // Re-checked on every use, not only when the folder was picked: a grant
        // persisted by an older build, or one whose folder turned out to be a
        // drop folder, must stop being read rather than be trusted because it is
        // already in the catalog.
        grant.refuse_if_too_broad()?;
        source = source.with_tree(grant, reader(tree_uri));
    }
    Ok(source)
}

/// Validate a folder the user just picked, before anything is persisted.
///
/// The picker hands back whatever provider the user browsed to, including ones
/// whose documents are rows in a cloud index. Orivo has to hand Winlator a *file
/// path*, so a folder it cannot name as a path is refused here with a sentence
/// rather than stored and discovered to be useless at launch.
pub fn grant_for_picked_folder(tree_uri: &str) -> Result<DocumentTreeGrant, WinlatorRunnerError> {
    let external_storage_root = crate::winlator_saf::external_storage_root()?;
    let grant = DocumentTreeGrant::parse(tree_uri, &external_storage_root)?;
    grant.refuse_if_too_broad()?;
    if !crate::winlator_saf::persisted_read_tree_uris()?
        .iter()
        .any(|granted| granted == grant.tree_uri())
    {
        return Err(WinlatorRunnerError::ExportFolderAccessLost);
    }
    Ok(grant)
}

#[derive(Debug, Clone, Copy)]
pub struct ScanLimits {
    pub max_files: usize,
    pub max_depth: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_files: DEFAULT_MAX_SCAN_FILES,
            max_depth: DEFAULT_MAX_SCAN_DEPTH,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedWinlatorShortcut {
    pub game_ref: String,
    pub title: String,
    pub directory_label: String,
    pub shortcut_path: PathBuf,
    pub fingerprint: String,
    pub container_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinlatorScanResult {
    pub scanned_files: usize,
    pub shortcuts: Vec<ScannedWinlatorShortcut>,
}

/// What Orivo is willing to read out of a shortcut Winlator wrote. Everything
/// else in the file — the Wine command line, the prefix path, the mapped drive
/// letter — is Winlator's business and is deliberately ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WinlatorShortcutEntry {
    pub name: Option<String>,
    pub container_id: Option<u32>,
}

/// Parse an exported Winlator shortcut.
///
/// Winlator writes `container_id:<id>` under `[Extra Data]` inside a container,
/// then rewrites it as `container_id=<id>` when exporting the file for a
/// frontend — and its own reader looks for the `=` form. Both are accepted here
/// so a shortcut copied by hand still resolves. A value that is not a small
/// integer, or a name carrying control characters, is dropped rather than
/// repaired: the caller then falls back to host-owned defaults.
pub fn parse_winlator_shortcut(contents: &str) -> WinlatorShortcutEntry {
    let mut entry = WinlatorShortcutEntry::default();
    let mut section = String::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            section = rest.split(']').next().unwrap_or_default().to_string();
            continue;
        }
        if let Some(value) = line.strip_prefix("container_id=") {
            entry.container_id = entry.container_id.or_else(|| parse_container_id(value));
            continue;
        }
        if section == "Extra Data"
            && let Some(value) = line.strip_prefix("container_id:")
        {
            entry.container_id = entry.container_id.or_else(|| parse_container_id(value));
            continue;
        }
        if section == "Desktop Entry"
            && let Some(value) = line.strip_prefix("Name=")
            && entry.name.is_none()
        {
            entry.name = display_text(value);
        }
    }
    entry
}

fn parse_container_id(value: &str) -> Option<u32> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|id| *id >= 1 && *id <= 9_999)
}

/// Enumerate the exported shortcuts a profile's grants currently expose.
///
/// The shape mirrors the Wine scanner on purpose — breadth-bounded,
/// depth-bounded, cancellable, symlinks never followed, every candidate
/// re-resolved before it receives an opaque reference — and it is written once
/// over a [`WinlatorShortcutSource`], so a folder read through SAF is walked by
/// exactly the code a readable directory is.
pub fn scan_winlator_shortcuts(
    profile: &WinlatorProfile,
    source: &dyn WinlatorShortcutSource,
    cancelled: &AtomicBool,
    limits: ScanLimits,
    mut progress: impl FnMut(usize),
) -> Result<WinlatorScanResult, WinlatorRunnerError> {
    if !profile.enabled {
        return Err(WinlatorRunnerError::ProfileDisabled);
    }
    profile
        .validate()
        .map_err(|_| WinlatorRunnerError::InvalidProfile)?;
    if limits.max_files == 0 || limits.max_depth == 0 {
        return Err(WinlatorRunnerError::InvalidPage);
    }

    let mut candidates = BTreeMap::new();
    let mut scanned_files = 0;
    for root in source.roots()? {
        cancelled_or(cancelled)?;
        scan_directory(
            source,
            &root,
            &root.directory,
            0,
            limits,
            profile.container_id,
            cancelled,
            &mut scanned_files,
            &mut candidates,
            &mut progress,
        )?;
    }

    Ok(WinlatorScanResult {
        scanned_files,
        shortcuts: candidates.into_values().collect(),
    })
}

#[allow(clippy::too_many_arguments)]
fn scan_directory(
    source: &dyn WinlatorShortcutSource,
    root: &ShortcutRoot,
    directory: &Path,
    depth: usize,
    limits: ScanLimits,
    container_fallback: Option<u32>,
    cancelled: &AtomicBool,
    scanned_files: &mut usize,
    candidates: &mut BTreeMap<String, ScannedWinlatorShortcut>,
    progress: &mut impl FnMut(usize),
) -> Result<(), WinlatorRunnerError> {
    cancelled_or(cancelled)?;
    let mut entries = source.entries(directory)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));

    for entry in entries {
        cancelled_or(cancelled)?;
        *scanned_files = scanned_files.saturating_add(1);
        if *scanned_files > limits.max_files {
            return Err(WinlatorRunnerError::TooManyFiles);
        }
        if *scanned_files % 32 == 0 {
            progress(*scanned_files);
        }
        match entry.kind {
            ShortcutEntryKind::Ignored => {}
            ShortcutEntryKind::Directory => {
                if depth < limits.max_depth {
                    scan_directory(
                        source,
                        root,
                        &entry.path,
                        depth + 1,
                        limits,
                        container_fallback,
                        cancelled,
                        scanned_files,
                        candidates,
                        progress,
                    )?;
                }
            }
            ShortcutEntryKind::File => {
                if !is_winlator_shortcut(&entry.path) {
                    continue;
                }
                let Ok(shortcut) = source.resolve(&entry.path) else {
                    continue;
                };
                if shortcut == root.directory || !shortcut.starts_with(&root.directory) {
                    continue;
                }
                // A shortcut that cannot be read is skipped rather than failing
                // the whole scan: one unreadable file must not hide a whole
                // library.
                let Ok(candidate) = read_shortcut_candidate(
                    source,
                    &shortcut,
                    &root.label,
                    container_fallback,
                    cancelled,
                ) else {
                    continue;
                };
                candidates
                    .entry(candidate.game_ref.clone())
                    .or_insert(candidate);
            }
        }
    }
    progress(*scanned_files);
    Ok(())
}

/// Return a stable bounded page from an already-completed scan snapshot.
pub fn page_winlator_inventory(
    shortcuts: &[ScannedWinlatorShortcut],
    offset: usize,
    limit: usize,
) -> Result<(Vec<ScannedWinlatorShortcut>, Option<usize>), WinlatorRunnerError> {
    if limit == 0 || limit > MAX_PAGE_SIZE || offset > shortcuts.len() {
        return Err(WinlatorRunnerError::InvalidPage);
    }
    let end = offset.saturating_add(limit).min(shortcuts.len());
    let next = (end < shortcuts.len()).then_some(end);
    Ok((shortcuts[offset..end].to_vec(), next))
}

/// Revalidate one host-owned shortcut against a profile without accepting a
/// scanner reference from the caller. The WebView can name only a catalog id;
/// the native host resolves, scope-checks and hashes the stored path itself.
pub fn validate_winlator_shortcut_for_profile(
    profile: &WinlatorProfile,
    source: &dyn WinlatorShortcutSource,
    shortcut: &Path,
    cancelled: &AtomicBool,
) -> Result<ScannedWinlatorShortcut, WinlatorRunnerError> {
    if !profile.enabled {
        return Err(WinlatorRunnerError::ProfileDisabled);
    }
    profile
        .validate()
        .map_err(|_| WinlatorRunnerError::InvalidProfile)?;
    cancelled_or(cancelled)?;
    let shortcut = source.resolve(shortcut)?;
    let label = shortcut
        .parent()
        .map(|directory| safe_label(directory, DEFAULT_ROOT_LABEL))
        .unwrap_or_else(|| DEFAULT_ROOT_LABEL.into());
    read_shortcut_candidate(source, &shortcut, &label, profile.container_id, cancelled)
}

/// Recheck a scan snapshot at the exact moment it crosses into persistence. A
/// scan is only a preview: Winlator may have re-exported the shortcut, or a
/// symlink may have been inserted, before the user pressed Import.
pub fn revalidate_winlator_import_candidate(
    profile: &WinlatorProfile,
    source: &dyn WinlatorShortcutSource,
    candidate: &ScannedWinlatorShortcut,
    cancelled: &AtomicBool,
) -> Result<ScannedWinlatorShortcut, WinlatorRunnerError> {
    let current = validate_winlator_shortcut_for_profile(
        profile,
        source,
        &candidate.shortcut_path,
        cancelled,
    )?;
    if candidate.game_ref != current.game_ref {
        return Err(WinlatorRunnerError::ShortcutNotLaunchable);
    }
    Ok(ScannedWinlatorShortcut {
        directory_label: candidate.directory_label.clone(),
        ..current
    })
}

/// Read, bound, hash and parse one shortcut the source has already resolved.
/// The digest is taken from the same bytes that were parsed, so the fingerprint
/// can never describe a different revision of the file than the title and the
/// container id do.
fn read_shortcut_candidate(
    source: &dyn WinlatorShortcutSource,
    shortcut: &Path,
    directory_label: &str,
    container_fallback: Option<u32>,
    cancelled: &AtomicBool,
) -> Result<ScannedWinlatorShortcut, WinlatorRunnerError> {
    cancelled_or(cancelled)?;
    let bytes = source.read(shortcut)?;
    let fingerprint = format!("sha256:{:x}", Sha256::digest(&bytes));
    // A shortcut Winlator wrote is ASCII `key=value` text. Anything that is not
    // valid UTF-8 is not one, and is refused rather than lossily decoded.
    let contents = String::from_utf8(bytes).map_err(|_| WinlatorRunnerError::ShortcutMissing)?;
    let entry = parse_winlator_shortcut(&contents);
    Ok(ScannedWinlatorShortcut {
        game_ref: game_reference_for(shortcut),
        title: entry
            .name
            .unwrap_or_else(|| shortcut_title_from_filename(shortcut)),
        directory_label: directory_label.into(),
        shortcut_path: shortcut.to_path_buf(),
        // The profile-level container is a fallback, never an override: the
        // shortcut Winlator exported knows which container it belongs to.
        container_id: entry.container_id.or(container_fallback),
        fingerprint,
    })
}

#[cfg(unix)]
fn read_shortcut_bytes(shortcut: &Path) -> Result<Vec<u8>, WinlatorRunnerError> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};

    // `O_NOFOLLOW` rejects a leaf symlink introduced after the canonical scope
    // check, and the metadata is read from the open descriptor rather than the
    // pathname so the size bound applies to the file actually being read.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(shortcut)
        .map_err(shortcut_file_error)?;
    let metadata = file.metadata().map_err(shortcut_file_error)?;
    if !metadata.is_file() {
        return Err(WinlatorRunnerError::ShortcutMissing);
    }
    if metadata.len() > MAX_SHORTCUT_BYTES {
        return Err(WinlatorRunnerError::ShortcutTooLarge);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_SHORTCUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(shortcut_file_error)?;
    if bytes.len() as u64 > MAX_SHORTCUT_BYTES {
        return Err(WinlatorRunnerError::ShortcutTooLarge);
    }
    Ok(bytes)
}

#[cfg(not(unix))]
fn read_shortcut_bytes(shortcut: &Path) -> Result<Vec<u8>, WinlatorRunnerError> {
    let metadata = fs::symlink_metadata(shortcut).map_err(shortcut_file_error)?;
    if !metadata.is_file() {
        return Err(WinlatorRunnerError::ShortcutMissing);
    }
    if metadata.len() > MAX_SHORTCUT_BYTES {
        return Err(WinlatorRunnerError::ShortcutTooLarge);
    }
    fs::read(shortcut).map_err(shortcut_file_error)
}

fn shortcut_file_error(error: io::Error) -> WinlatorRunnerError {
    match error.kind() {
        io::ErrorKind::NotFound => WinlatorRunnerError::ShortcutMissing,
        io::ErrorKind::PermissionDenied => WinlatorRunnerError::AccessDenied,
        _ => WinlatorRunnerError::ShortcutMissing,
    }
}

/// Resolve a typed intent into one explicit Android intent.
///
/// This is the Winlator equivalent of `prepare_wine_launch`, and it is pure:
/// every filesystem check happens here, and the result carries no capability
/// beyond "send these extras to this component". The final content check is
/// deliberately repeated in [`PreparedWinlatorLaunch::launch`], immediately
/// before the intent leaves the process.
pub fn prepare_winlator_launch(
    profile: &WinlatorProfile,
    source: &dyn WinlatorShortcutSource,
    game: &WinlatorShortcutInventoryEntry,
    intent: &WinlatorLaunchIntent,
) -> Result<PreparedWinlatorLaunch, WinlatorRunnerError> {
    if intent.runner_id() != WINLATOR_RUNNER_ID
        || intent.profile_id() != profile.id
        || intent.game_ref() != game.game_ref
        || intent.mode() != WinlatorLaunchMode::ExportedShortcut
    {
        return Err(WinlatorRunnerError::InvalidIntent);
    }
    if !profile.enabled {
        return Err(WinlatorRunnerError::ProfileDisabled);
    }
    profile
        .validate()
        .map_err(|_| WinlatorRunnerError::InvalidProfile)?;
    if game.profile_id != profile.id {
        return Err(WinlatorRunnerError::InvalidIntent);
    }
    game.validate()
        .map_err(|_| WinlatorRunnerError::ShortcutNotLaunchable)?;
    let surface = launch_surface(profile.distribution)?;

    let current = validate_winlator_shortcut_for_profile(
        profile,
        source,
        &game.shortcut_path,
        &AtomicBool::new(false),
    )?;
    if game.fingerprint != current.fingerprint || game.game_ref != current.game_ref {
        return Err(WinlatorRunnerError::ShortcutNotLaunchable);
    }
    // Android extras are Java strings. A pathname that is not valid UTF-8 could
    // not survive the crossing intact, so it is refused here rather than being
    // silently replaced.
    let shortcut_path = current
        .shortcut_path
        .to_str()
        .ok_or(WinlatorRunnerError::ShortcutNotLaunchable)?
        .to_string();

    let mut extras = Vec::new();
    // Winlator reads the container from the shortcut when this extra is absent,
    // so an unknown container is an omission rather than a guessed zero.
    if let Some(container_id) = current.container_id {
        extras.push(AndroidIntentExtra::Int {
            key: surface.container_id_extra,
            value: i32::try_from(container_id).map_err(|_| WinlatorRunnerError::InvalidProfile)?,
        });
    }
    extras.push(AndroidIntentExtra::Text {
        key: surface.shortcut_name_extra,
        value: current.title.clone(),
    });
    extras.push(AndroidIntentExtra::Text {
        key: surface.shortcut_path_extra,
        value: shortcut_path,
    });

    Ok(PreparedWinlatorLaunch {
        intent: AndroidIntent {
            package: surface.package,
            activity: surface.activity,
            flags: INTENT_FLAGS,
            extras,
        },
        shortcut_path: current.shortcut_path,
        fingerprint: current.fingerprint,
        title: current.title,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedWinlatorLaunch {
    intent: AndroidIntent,
    shortcut_path: PathBuf,
    fingerprint: String,
    title: String,
}

impl PreparedWinlatorLaunch {
    /// Exposed only so a host test can assert the exact intent without an
    /// emulator; the hand-off below reads the field directly.
    #[allow(dead_code)]
    pub fn intent(&self) -> &AndroidIntent {
        &self.intent
    }

    /// The name Winlator itself gave this shortcut, resolved from the file the
    /// host just verified rather than from the card the WebView asked about.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Send the prepared intent. Nothing is spawned and no process is owned:
    /// Winlator starts, and Orivo's only feedback is whether Android accepted
    /// the hand-off.
    pub fn launch(&self, source: &dyn WinlatorShortcutSource) -> Result<(), WinlatorRunnerError> {
        // This is deliberately immediately before the hand-off. The earlier
        // checks resolved the shortcut against its grant; this content-addressed
        // recheck rejects one rewritten while the launch was being prepared
        // without changing its pathname.
        let bytes = source.read(&self.shortcut_path)?;
        if format!("sha256:{:x}", Sha256::digest(&bytes)) != self.fingerprint {
            return Err(WinlatorRunnerError::ShortcutNotLaunchable);
        }
        self.send()
    }

    #[cfg(target_os = "android")]
    fn send(&self) -> Result<(), WinlatorRunnerError> {
        android::start_activity(self.intent.clone())
    }

    #[cfg(not(target_os = "android"))]
    fn send(&self) -> Result<(), WinlatorRunnerError> {
        Err(WinlatorRunnerError::PlatformUnsupported)
    }
}

/// The only part of this module that is not testable on the host. It builds the
/// intent through JNI on Android's main thread and reports back whether the
/// platform accepted it, so "Winlator is not installed" reaches the user as a
/// sentence instead of a silently dropped tap.
#[cfg(target_os = "android")]
mod android {
    use super::{AndroidIntent, AndroidIntentExtra, WinlatorRunnerError};
    use jni::{JNIEnv, objects::JObject};
    use std::{sync::mpsc, time::Duration};

    /// Starting an activity is a handful of JNI calls on an already-running
    /// main thread. A wait this long only ever expires when that thread is
    /// wedged, in which case reporting a failure beats blocking the launch.
    const HAND_OFF_TIMEOUT: Duration = Duration::from_secs(5);

    pub(super) fn start_activity(intent: AndroidIntent) -> Result<(), WinlatorRunnerError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        tauri::wry::prelude::dispatch(move |env, activity, _webview| {
            let outcome = match build_and_start(env, activity, &intent) {
                Ok(()) => Ok(()),
                // A Java exception is still pending on this thread, so it must
                // be taken before any further JNI call is made.
                Err(_) => Err(classify_pending_exception(env)),
            };
            let _ = sender.send(outcome);
        });
        receiver
            .recv_timeout(HAND_OFF_TIMEOUT)
            .unwrap_or(Err(WinlatorRunnerError::LaunchFailed))
    }

    fn build_and_start(
        env: &mut JNIEnv<'_>,
        activity: &JObject<'_>,
        intent: &AndroidIntent,
    ) -> jni::errors::Result<()> {
        let intent_class = env.find_class("android/content/Intent")?;
        let target = env.new_object(&intent_class, "()V", &[])?;

        // An explicit component is what makes this safe *and* what makes a
        // missing Winlator detectable: intent filters are bypassed entirely, so
        // no `<queries>` manifest entry is needed to see the package, and an
        // absent one surfaces as ActivityNotFoundException.
        let component_class = env.find_class("android/content/ComponentName")?;
        let package = env.new_string(intent.package())?;
        let class_name = env.new_string(intent.activity())?;
        let component = env.new_object(
            &component_class,
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[(&package).into(), (&class_name).into()],
        )?;
        env.call_method(
            &target,
            "setComponent",
            "(Landroid/content/ComponentName;)Landroid/content/Intent;",
            &[(&component).into()],
        )?;
        env.call_method(
            &target,
            "addFlags",
            "(I)Landroid/content/Intent;",
            &[intent.flags().into()],
        )?;

        for extra in intent.extras() {
            match extra {
                AndroidIntentExtra::Int { key, value } => {
                    let key = env.new_string(key)?;
                    env.call_method(
                        &target,
                        "putExtra",
                        "(Ljava/lang/String;I)Landroid/content/Intent;",
                        &[(&key).into(), (*value).into()],
                    )?;
                }
                AndroidIntentExtra::Text { key, value } => {
                    let key = env.new_string(key)?;
                    let value = env.new_string(value)?;
                    env.call_method(
                        &target,
                        "putExtra",
                        "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
                        &[(&key).into(), (&value).into()],
                    )?;
                }
            }
        }

        env.call_method(
            activity,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[(&target).into()],
        )?;
        Ok(())
    }

    fn classify_pending_exception(env: &mut JNIEnv<'_>) -> WinlatorRunnerError {
        let Ok(throwable) = env.exception_occurred() else {
            return WinlatorRunnerError::LaunchFailed;
        };
        // `exception_describe` puts the Java stack trace in logcat, which is
        // the only place a device-side launch failure can be diagnosed from.
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        if throwable.is_null() {
            return WinlatorRunnerError::LaunchFailed;
        }
        match java_class_name(env, &throwable).as_deref() {
            Some("android.content.ActivityNotFoundException") => {
                WinlatorRunnerError::WinlatorMissing
            }
            Some("java.lang.SecurityException") => WinlatorRunnerError::WinlatorRefusedLaunch,
            _ => WinlatorRunnerError::LaunchFailed,
        }
    }

    fn java_class_name(env: &mut JNIEnv<'_>, throwable: &JObject<'_>) -> Option<String> {
        let class = env
            .call_method(throwable, "getClass", "()Ljava/lang/Class;", &[])
            .and_then(|class| class.l())
            .ok()?;
        let name = env
            .call_method(&class, "getName", "()Ljava/lang/String;", &[])
            .and_then(|name| name.l())
            .ok()?;
        env.get_string(&name.into())
            .ok()
            .map(|name| name.to_string_lossy().into_owned())
    }
}

fn belongs_to_grant(shortcut: &Path, directories: &[PathBuf]) -> Result<bool, WinlatorRunnerError> {
    let mut readable_grant = false;
    for directory in directories {
        let root = match fs::canonicalize(directory) {
            Ok(root) if root.is_dir() => root,
            Ok(_) => continue,
            Err(_) => continue,
        };
        readable_grant = true;
        if shortcut != root && shortcut.starts_with(&root) {
            return Ok(true);
        }
    }
    if readable_grant {
        Ok(false)
    } else {
        Err(WinlatorRunnerError::AccessDenied)
    }
}

fn cancelled_or(cancelled: &AtomicBool) -> Result<(), WinlatorRunnerError> {
    if cancelled.load(Ordering::Acquire) {
        Err(WinlatorRunnerError::Cancelled)
    } else {
        Ok(())
    }
}

fn valid_opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

fn is_winlator_shortcut(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("desktop"))
}

fn shortcut_title_from_filename(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .and_then(display_text)
        .unwrap_or_else(|| "Winlator game".into())
}

fn display_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && !trimmed.chars().any(char::is_control))
        .then(|| trimmed.chars().take(MAX_SHORTCUT_TITLE_CHARS).collect())
}

fn safe_label(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(display_text)
        .unwrap_or_else(|| fallback.into())
}

/// Hash the shortcut bytes independently from the persistent game reference.
/// The reference is path-stable so a re-exported shortcut refreshes one library
/// card, while the content digest makes the host refuse a changed shortcut until
/// a deliberate reimport has updated its private inventory.
fn game_reference_for(canonical_path: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orivo-winlator-shortcut-reference-v1\0");
    digest.update(canonical_path.as_os_str().as_encoded_bytes());
    format!("shortcut:{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "orivo-winlator-runner-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::canonicalize(directory).unwrap()
    }

    /// The exact shape Winlator Cmod writes when the user exports a shortcut to
    /// a frontend directory: the in-container `[Extra Data]` block, plus the
    /// `container_id=` line the exporter appends and its own reader looks for.
    fn exported_shortcut(name: &str, container_id: u32) -> String {
        format!(
            "[Desktop Entry]\nName={name}\nExec=env WINEPREFIX=\"/data/data/com.winlator.cmod/files/imagefs/home/xuser/.wine\" wine D:\\\\\\\\Games\\\\\\\\{name}\\\\\\\\{name}.exe\nType=Application\nStartupNotify=true\nPath=/data/data/com.winlator.cmod/files/imagefs/home/xuser/.wine/dosdevices/d:/Games/{name}\nIcon=\nStartupWMClass={name}.exe\n\n[Extra Data]\ncontainer_id:{container_id}\ncontainer_id={container_id}\n"
        )
    }

    fn write_shortcut(directory: &Path, file: &str, contents: &str) -> PathBuf {
        let path = directory.join(file);
        fs::write(&path, contents).unwrap();
        fs::canonicalize(path).unwrap()
    }

    /// Every test below reads a real directory, which is what a desktop build
    /// and a device with a plainly readable folder both do. The storage access
    /// grant has its own tests, against a fake provider.
    fn filesystem(profile: &WinlatorProfile) -> FilesystemShortcuts<'_> {
        FilesystemShortcuts::for_profile(profile)
    }

    fn profile(granted: &Path) -> WinlatorProfile {
        WinlatorProfile {
            id: "winlator-test".into(),
            display_name: "Winlator".into(),
            distribution: WinlatorDistribution::Cmod,
            container_id: None,
            shortcut_directories: vec![granted.to_path_buf()],
            shortcut_trees: Vec::new(),
            enabled: true,
            last_imported_at: None,
        }
    }

    fn inventory(
        profile: &WinlatorProfile,
        candidate: &ScannedWinlatorShortcut,
    ) -> WinlatorShortcutInventoryEntry {
        WinlatorShortcutInventoryEntry {
            profile_id: profile.id.clone(),
            game_ref: candidate.game_ref.clone(),
            title: candidate.title.clone(),
            shortcut_path: candidate.shortcut_path.clone(),
            fingerprint: candidate.fingerprint.clone(),
            container_id: candidate.container_id,
            imported_at: None,
        }
    }

    #[test]
    fn reads_the_name_and_container_winlator_wrote_into_an_exported_shortcut() {
        let entry = parse_winlator_shortcut(&exported_shortcut("Hollow Knight", 3));
        assert_eq!(entry.name.as_deref(), Some("Hollow Knight"));
        assert_eq!(entry.container_id, Some(3));
    }

    /// A shortcut copied straight out of a container carries only the
    /// `[Extra Data]` colon form, so that one has to resolve too.
    #[test]
    fn reads_the_in_container_extra_data_form() {
        let entry = parse_winlator_shortcut(
            "[Desktop Entry]\nName=Celeste\n\n[Extra Data]\ncontainer_id:7\n",
        );
        assert_eq!(entry.name.as_deref(), Some("Celeste"));
        assert_eq!(entry.container_id, Some(7));
    }

    /// `container_id:` is only meaningful inside `[Extra Data]`. A line that
    /// shape elsewhere is somebody else's key, not a container.
    #[test]
    fn ignores_a_colon_form_container_outside_its_own_section() {
        let entry = parse_winlator_shortcut("[Desktop Entry]\nName=Braid\ncontainer_id:7\n");
        assert_eq!(entry.container_id, None);
    }

    #[test]
    fn drops_a_container_id_that_is_not_a_container_number() {
        for value in ["0", "abc", "100000", "-2", "2 3", ""] {
            let entry = parse_winlator_shortcut(&format!(
                "[Desktop Entry]\nName=X\ncontainer_id={value}\n"
            ));
            assert_eq!(entry.container_id, None, "accepted container id {value:?}");
        }
    }

    #[test]
    fn a_name_carrying_control_characters_falls_back_to_the_filename() {
        let granted = temporary_directory("control-name");
        write_shortcut(
            &granted,
            "Quake.desktop",
            "[Desktop Entry]\nName=Qu\u{7}ake\ncontainer_id=1\n",
        );
        let profile = profile(&granted);
        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(scan.shortcuts.len(), 1);
        assert_eq!(scan.shortcuts[0].title, "Quake");
    }

    #[test]
    fn scans_only_the_desktop_files_inside_the_grant() {
        let granted = temporary_directory("scan");
        write_shortcut(
            &granted,
            "Celeste.desktop",
            &exported_shortcut("Celeste", 1),
        );
        write_shortcut(&granted, "Braid.desktop", &exported_shortcut("Braid", 2));
        // The two notes Winlator writes beside its exported shortcuts, and a
        // Windows executable that belongs to Winlator's drive, not to Orivo.
        write_shortcut(&granted, "FRONTEND_INSTRUCTIONS.txt", "am start -n ...");
        write_shortcut(&granted, "metadata.pegasus.txt", "collection: Windows");
        write_shortcut(&granted, "Celeste.exe", "MZ");

        let profile = profile(&granted);
        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        // A scan snapshot is keyed by opaque reference, like the Wine scanner's,
        // so it carries no display order of its own.
        let mut titles = scan
            .shortcuts
            .iter()
            .map(|shortcut| shortcut.title.as_str())
            .collect::<Vec<_>>();
        titles.sort_unstable();
        assert_eq!(titles, ["Braid", "Celeste"]);
        assert!(
            scan.shortcuts
                .iter()
                .all(|shortcut| shortcut.fingerprint.starts_with("sha256:")
                    && shortcut.game_ref.starts_with("shortcut:"))
        );
    }

    #[test]
    fn refuses_a_shortcut_outside_the_profile_grant() {
        let granted = temporary_directory("scope-granted");
        let elsewhere = temporary_directory("scope-elsewhere");
        let outside = write_shortcut(&elsewhere, "Doom.desktop", &exported_shortcut("Doom", 1));
        let profile = profile(&granted);
        assert_eq!(
            validate_winlator_shortcut_for_profile(
                &profile,
                &filesystem(&profile),
                &outside,
                &AtomicBool::new(false)
            ),
            Err(WinlatorRunnerError::ShortcutOutsideScope)
        );
    }

    #[test]
    fn a_symlink_into_the_grant_does_not_smuggle_a_shortcut_in() {
        let granted = temporary_directory("symlink-granted");
        let elsewhere = temporary_directory("symlink-elsewhere");
        let outside = write_shortcut(&elsewhere, "Doom.desktop", &exported_shortcut("Doom", 1));
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, granted.join("Doom.desktop")).unwrap();

        let profile = profile(&granted);
        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        assert!(scan.shortcuts.is_empty());
    }

    #[test]
    fn refuses_a_file_too_large_to_be_a_shortcut() {
        let granted = temporary_directory("oversized");
        let oversized = write_shortcut(
            &granted,
            "Huge.desktop",
            &"x".repeat(MAX_SHORTCUT_BYTES as usize + 1),
        );
        let profile = profile(&granted);
        assert_eq!(
            validate_winlator_shortcut_for_profile(
                &profile,
                &filesystem(&profile),
                &oversized,
                &AtomicBool::new(false)
            ),
            Err(WinlatorRunnerError::ShortcutTooLarge)
        );
    }

    #[test]
    fn prepares_one_explicit_intent_with_only_host_owned_extra_keys() {
        let granted = temporary_directory("intent");
        write_shortcut(
            &granted,
            "Celeste.desktop",
            &exported_shortcut("Celeste", 4),
        );
        let profile = profile(&granted);
        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        let candidate = &scan.shortcuts[0];
        let entry = inventory(&profile, candidate);
        let intent = WinlatorLaunchIntent::new(&profile.id, &candidate.game_ref).unwrap();
        let prepared =
            prepare_winlator_launch(&profile, &filesystem(&profile), &entry, &intent).unwrap();

        assert_eq!(prepared.title(), "Celeste");
        let intent = prepared.intent();
        assert_eq!(intent.package(), "com.winlator.cmod");
        assert_eq!(
            intent.activity(),
            "com.winlator.cmod.XServerDisplayActivity"
        );
        // NEW_TASK | CLEAR_TASK | CLEAR_TOP, and deliberately not NO_HISTORY.
        assert_eq!(intent.flags(), 0x1000_0000 | 0x0000_8000 | 0x0400_0000);
        assert_eq!(
            intent.extras(),
            [
                AndroidIntentExtra::Int {
                    key: "container_id",
                    value: 4
                },
                AndroidIntentExtra::Text {
                    key: "shortcut_name",
                    value: "Celeste".into()
                },
                AndroidIntentExtra::Text {
                    key: "shortcut_path",
                    value: candidate.shortcut_path.to_str().unwrap().into()
                },
            ]
        );
    }

    /// The container written into the shortcut is the one Winlator itself put
    /// there, so it must win over the profile-level fallback.
    #[test]
    fn the_container_in_the_shortcut_outranks_the_profile_fallback() {
        let granted = temporary_directory("container-precedence");
        write_shortcut(
            &granted,
            "Celeste.desktop",
            &exported_shortcut("Celeste", 4),
        );
        write_shortcut(
            &granted,
            "Braid.desktop",
            "[Desktop Entry]\nName=Braid\nType=Application\n",
        );
        let mut profile = profile(&granted);
        profile.container_id = Some(9);

        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        let mut containers = scan
            .shortcuts
            .iter()
            .map(|shortcut| (shortcut.title.as_str(), shortcut.container_id))
            .collect::<Vec<_>>();
        containers.sort_unstable();
        assert_eq!(containers, [("Braid", Some(9)), ("Celeste", Some(4))]);
    }

    /// A shortcut with no container anywhere leaves the extra out entirely.
    /// Winlator then resolves the container from the file itself, which is
    /// better than Orivo guessing a number.
    #[test]
    fn an_unknown_container_is_omitted_rather_than_guessed() {
        let granted = temporary_directory("no-container");
        write_shortcut(
            &granted,
            "Braid.desktop",
            "[Desktop Entry]\nName=Braid\nType=Application\n",
        );
        let profile = profile(&granted);
        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        let candidate = &scan.shortcuts[0];
        let entry = inventory(&profile, candidate);
        let intent = WinlatorLaunchIntent::new(&profile.id, &candidate.game_ref).unwrap();
        let prepared =
            prepare_winlator_launch(&profile, &filesystem(&profile), &entry, &intent).unwrap();
        assert!(
            prepared
                .intent()
                .extras()
                .iter()
                .all(|extra| !matches!(extra, AndroidIntentExtra::Int { .. }))
        );
    }

    /// Winlator rewrites an exported shortcut whenever the user re-exports it,
    /// and the new file can point at a different executable or container. The
    /// stored fingerprint is what makes that a refusal instead of a surprise.
    #[test]
    fn refuses_a_shortcut_rewritten_after_it_was_imported() {
        let granted = temporary_directory("rewritten");
        write_shortcut(
            &granted,
            "Celeste.desktop",
            &exported_shortcut("Celeste", 4),
        );
        let profile = profile(&granted);
        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        let candidate = &scan.shortcuts[0];
        let entry = inventory(&profile, candidate);
        let intent = WinlatorLaunchIntent::new(&profile.id, &candidate.game_ref).unwrap();

        write_shortcut(
            &granted,
            "Celeste.desktop",
            &exported_shortcut("Celeste", 5),
        );
        assert_eq!(
            prepare_winlator_launch(&profile, &filesystem(&profile), &entry, &intent),
            Err(WinlatorRunnerError::ShortcutNotLaunchable)
        );
    }

    /// The same check has to hold between preparing the intent and sending it,
    /// because that window is the one an attacker controls.
    #[test]
    fn refuses_to_send_an_intent_for_a_shortcut_rewritten_mid_launch() {
        let granted = temporary_directory("rewritten-mid-launch");
        write_shortcut(
            &granted,
            "Celeste.desktop",
            &exported_shortcut("Celeste", 4),
        );
        let profile = profile(&granted);
        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        let candidate = &scan.shortcuts[0];
        let entry = inventory(&profile, candidate);
        let intent = WinlatorLaunchIntent::new(&profile.id, &candidate.game_ref).unwrap();
        let prepared =
            prepare_winlator_launch(&profile, &filesystem(&profile), &entry, &intent).unwrap();

        write_shortcut(
            &granted,
            "Celeste.desktop",
            &exported_shortcut("Celeste", 5),
        );
        assert_eq!(
            prepared.launch(&filesystem(&profile)),
            Err(WinlatorRunnerError::ShortcutNotLaunchable)
        );
    }

    /// brunodev85's official build declares `XServerDisplayActivity` as
    /// `exported="false"`, so there is no intent Orivo can send that would start
    /// a game there. Saying so beats a silently dropped launch.
    #[test]
    fn refuses_the_official_distribution_whose_activity_is_not_exported() {
        let granted = temporary_directory("official");
        write_shortcut(
            &granted,
            "Celeste.desktop",
            &exported_shortcut("Celeste", 1),
        );
        let mut profile = profile(&granted);
        profile.distribution = WinlatorDistribution::Official;
        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        let candidate = &scan.shortcuts[0];
        let entry = inventory(&profile, candidate);
        let intent = WinlatorLaunchIntent::new(&profile.id, &candidate.game_ref).unwrap();
        assert_eq!(
            prepare_winlator_launch(&profile, &filesystem(&profile), &entry, &intent),
            Err(WinlatorRunnerError::DistributionNotLaunchable)
        );
    }

    #[test]
    fn refuses_an_intent_that_names_another_profile_or_another_game() {
        let granted = temporary_directory("mismatched-intent");
        write_shortcut(
            &granted,
            "Celeste.desktop",
            &exported_shortcut("Celeste", 1),
        );
        let profile = profile(&granted);
        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        let candidate = &scan.shortcuts[0];
        let entry = inventory(&profile, candidate);

        let other_profile =
            WinlatorLaunchIntent::new("winlator-other", &candidate.game_ref).unwrap();
        assert_eq!(
            prepare_winlator_launch(&profile, &filesystem(&profile), &entry, &other_profile),
            Err(WinlatorRunnerError::InvalidIntent)
        );
        let other_game = WinlatorLaunchIntent::new(&profile.id, "shortcut:deadbeef").unwrap();
        assert_eq!(
            prepare_winlator_launch(&profile, &filesystem(&profile), &entry, &other_game),
            Err(WinlatorRunnerError::InvalidIntent)
        );
    }

    #[test]
    fn refuses_a_reference_that_is_not_an_opaque_identifier() {
        for value in ["", "../etc/passwd", "a b", "/absolute/path", "-dashed"] {
            assert_eq!(
                WinlatorLaunchIntent::new("winlator-test", value),
                Err(WinlatorRunnerError::InvalidIntent),
                "accepted game reference {value:?}"
            );
        }
    }

    #[test]
    fn refuses_a_disabled_profile_before_touching_the_filesystem() {
        let granted = temporary_directory("disabled");
        write_shortcut(
            &granted,
            "Celeste.desktop",
            &exported_shortcut("Celeste", 1),
        );
        let mut profile = profile(&granted);
        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        let candidate = &scan.shortcuts[0];
        let entry = inventory(&profile, candidate);
        let intent = WinlatorLaunchIntent::new(&profile.id, &candidate.game_ref).unwrap();

        profile.enabled = false;
        assert_eq!(
            prepare_winlator_launch(&profile, &filesystem(&profile), &entry, &intent),
            Err(WinlatorRunnerError::ProfileDisabled)
        );
        assert_eq!(
            scan_winlator_shortcuts(
                &profile,
                &filesystem(&profile),
                &AtomicBool::new(false),
                ScanLimits::default(),
                |_| {}
            ),
            Err(WinlatorRunnerError::ProfileDisabled)
        );
    }

    #[test]
    fn a_cancelled_scan_stops_instead_of_walking_the_grant() {
        let granted = temporary_directory("cancelled");
        write_shortcut(
            &granted,
            "Celeste.desktop",
            &exported_shortcut("Celeste", 1),
        );
        let profile = profile(&granted);
        assert_eq!(
            scan_winlator_shortcuts(
                &profile,
                &filesystem(&profile),
                &AtomicBool::new(true),
                ScanLimits::default(),
                |_| {}
            ),
            Err(WinlatorRunnerError::Cancelled)
        );
    }

    #[test]
    fn a_scan_beyond_its_file_budget_is_refused_rather_than_truncated() {
        let granted = temporary_directory("budget");
        for index in 0..4 {
            write_shortcut(
                &granted,
                &format!("Game{index}.desktop"),
                &exported_shortcut(&format!("Game{index}"), 1),
            );
        }
        let profile = profile(&granted);
        assert_eq!(
            scan_winlator_shortcuts(
                &profile,
                &filesystem(&profile),
                &AtomicBool::new(false),
                ScanLimits {
                    max_files: 2,
                    max_depth: 2
                },
                |_| {}
            ),
            Err(WinlatorRunnerError::TooManyFiles)
        );
    }

    #[test]
    fn pages_a_scan_snapshot_within_its_bounds() {
        let granted = temporary_directory("paging");
        for index in 0..3 {
            write_shortcut(
                &granted,
                &format!("Game{index}.desktop"),
                &exported_shortcut(&format!("Game{index}"), 1),
            );
        }
        let profile = profile(&granted);
        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        let (page, next) = page_winlator_inventory(&scan.shortcuts, 0, 2).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(next, Some(2));
        let (page, next) = page_winlator_inventory(&scan.shortcuts, 2, 2).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(next, None);
        assert_eq!(
            page_winlator_inventory(&scan.shortcuts, 0, 0),
            Err(WinlatorRunnerError::InvalidPage)
        );
        assert_eq!(
            page_winlator_inventory(&scan.shortcuts, 4, 2),
            Err(WinlatorRunnerError::InvalidPage)
        );
    }

    /// Nothing about this adapter may reach a process. On a host that has no
    /// Android intent service the hand-off is a refusal, never a fallback.
    #[cfg(not(target_os = "android"))]
    #[test]
    fn a_launch_on_a_desktop_host_is_refused_rather_than_shelled_out() {
        let granted = temporary_directory("desktop-host");
        write_shortcut(
            &granted,
            "Celeste.desktop",
            &exported_shortcut("Celeste", 1),
        );
        let profile = profile(&granted);
        let scan = scan_winlator_shortcuts(
            &profile,
            &filesystem(&profile),
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        let candidate = &scan.shortcuts[0];
        let entry = inventory(&profile, candidate);
        let intent = WinlatorLaunchIntent::new(&profile.id, &candidate.game_ref).unwrap();
        let prepared =
            prepare_winlator_launch(&profile, &filesystem(&profile), &entry, &intent).unwrap();
        assert_eq!(
            prepared.launch(&filesystem(&profile)),
            Err(WinlatorRunnerError::PlatformUnsupported)
        );
    }
    /// Everything below reads the folder the way a device does: through a storage
    /// access grant, with a provider that answers from memory and sometimes lies.
    mod through_a_storage_access_grant {
        use super::*;
        use crate::winlator_saf::{DIRECTORY_MIME_TYPE, TreeDocument, fake::FakeDocumentTree};

        const FRONTEND_TREE: &str = "content://com.android.externalstorage.documents/tree/primary%3ADownload%2FWinlator%2FFrontend";
        const FRONTEND_DOCUMENT: &str = "primary:Download/Winlator/Frontend";
        const EXTERNAL_ROOT: &str = "/storage/emulated/0";
        const FRONTEND_PATH: &str = "/storage/emulated/0/Download/Winlator/Frontend";

        fn granted_profile() -> WinlatorProfile {
            WinlatorProfile {
                id: "winlator-saf".into(),
                display_name: "Winlator".into(),
                distribution: WinlatorDistribution::Cmod,
                container_id: None,
                shortcut_directories: vec![PathBuf::from(FRONTEND_PATH)],
                shortcut_trees: vec![FRONTEND_TREE.into()],
                enabled: true,
                last_imported_at: None,
            }
        }

        fn source(tree: FakeDocumentTree) -> DocumentTreeShortcuts {
            let grant = DocumentTreeGrant::parse(FRONTEND_TREE, Path::new(EXTERNAL_ROOT)).unwrap();
            DocumentTreeShortcuts::new().with_tree(grant, Box::new(tree))
        }

        fn frontend(documents: &[(&str, String)]) -> FakeDocumentTree {
            let mut tree = FakeDocumentTree::new(FRONTEND_DOCUMENT);
            for (name, contents) in documents {
                tree = tree.with_document(&format!("{FRONTEND_DOCUMENT}/{name}"), contents);
            }
            tree
        }

        fn scan(
            profile: &WinlatorProfile,
            source: &DocumentTreeShortcuts,
        ) -> Result<WinlatorScanResult, WinlatorRunnerError> {
            scan_winlator_shortcuts(
                profile,
                source,
                &AtomicBool::new(false),
                ScanLimits::default(),
                |_| {},
            )
        }

        /// The point of the whole grant: a shortcut Orivo can only *read* through
        /// a `ContentResolver` still resolves to the file path Winlator opens.
        #[test]
        fn adopts_a_shortcut_it_can_only_read_through_the_grant() {
            let profile = granted_profile();
            let source = source(frontend(&[
                ("Celeste.desktop", exported_shortcut("Celeste", 2)),
                ("FRONTEND_INSTRUCTIONS.txt", "am start -n ...".into()),
                ("metadata.pegasus.txt", "collection: Windows".into()),
            ]));
            let scan = scan(&profile, &source).unwrap();

            assert_eq!(scan.shortcuts.len(), 1);
            let shortcut = &scan.shortcuts[0];
            assert_eq!(shortcut.title, "Celeste");
            assert_eq!(shortcut.container_id, Some(2));
            assert_eq!(
                shortcut.shortcut_path,
                Path::new("/storage/emulated/0/Download/Winlator/Frontend/Celeste.desktop")
            );
            assert!(shortcut.fingerprint.starts_with("sha256:"));
        }

        /// Winlator's `shortcut_path` extra is a file path. A content URI there
        /// would be a file Winlator cannot open, so this is the assertion that
        /// says the SAF grant did not leak into the hand-off.
        #[test]
        fn hands_winlator_a_file_path_and_never_a_content_uri() {
            let profile = granted_profile();
            let source = source(frontend(&[(
                "Celeste.desktop",
                exported_shortcut("Celeste", 4),
            )]));
            let scan = scan(&profile, &source).unwrap();
            let candidate = &scan.shortcuts[0];
            let entry = inventory(&profile, candidate);
            let intent = WinlatorLaunchIntent::new(&profile.id, &candidate.game_ref).unwrap();
            let prepared = prepare_winlator_launch(&profile, &source, &entry, &intent).unwrap();

            assert_eq!(
                prepared.intent().extras(),
                [
                    AndroidIntentExtra::Int {
                        key: "container_id",
                        value: 4
                    },
                    AndroidIntentExtra::Text {
                        key: "shortcut_name",
                        value: "Celeste".into()
                    },
                    AndroidIntentExtra::Text {
                        key: "shortcut_path",
                        value: "/storage/emulated/0/Download/Winlator/Frontend/Celeste.desktop"
                            .into()
                    },
                ]
            );
        }

        /// A provider answers a listing with whatever rows it likes. Orivo checks
        /// every one of them against the grant instead of following it.
        #[test]
        fn drops_a_provider_row_that_does_not_belong_to_the_folder() {
            let profile = granted_profile();
            let tree = frontend(&[("Celeste.desktop", exported_shortcut("Celeste", 1))])
                // Somebody else's document, inside no grant of ours.
                .with_stray_row(
                    FRONTEND_DOCUMENT,
                    TreeDocument {
                        document_id: "primary:Download/Secrets/Keys.desktop".into(),
                        display_name: "Keys.desktop".into(),
                        mime_type: "application/octet-stream".into(),
                        size: Some(64),
                    },
                )
                // An identifier that looks inside the tree and resolves outside it.
                .with_stray_row(
                    FRONTEND_DOCUMENT,
                    TreeDocument {
                        document_id: format!("{FRONTEND_DOCUMENT}/../../../etc/hosts.desktop"),
                        display_name: "hosts.desktop".into(),
                        mime_type: "application/octet-stream".into(),
                        size: Some(64),
                    },
                )
                // A directory row pointing back at the folder being listed, which
                // is how a provider would make the walk loop.
                .with_stray_row(
                    FRONTEND_DOCUMENT,
                    TreeDocument {
                        document_id: FRONTEND_DOCUMENT.into(),
                        display_name: "Frontend".into(),
                        mime_type: DIRECTORY_MIME_TYPE.into(),
                        size: None,
                    },
                );
            let source = source(tree);
            let scan = scan(&profile, &source).unwrap();

            let titles = scan
                .shortcuts
                .iter()
                .map(|shortcut| shortcut.title.as_str())
                .collect::<Vec<_>>();
            assert_eq!(titles, ["Celeste"]);
            // Every row still counted against the budget: a provider cannot buy
            // unbounded work by answering with rows that are then discarded.
            assert_eq!(scan.scanned_files, 4);
        }

        /// The row is inside the grant, so every scope check passes — and it is
        /// still not a child of the folder being listed. A provider that can
        /// answer one listing with another folder's documents decides what the
        /// walk sees, and the depth limit stops meaning anything: here the
        /// subfolder is never listed as a child, so its document can only arrive
        /// by way of the stray row.
        #[test]
        fn drops_a_row_the_provider_claims_as_a_child_of_the_wrong_folder() {
            let profile = granted_profile();
            let tree = frontend(&[("Celeste.desktop", exported_shortcut("Celeste", 1))])
                .with_document(
                    &format!("{FRONTEND_DOCUMENT}/RPG/Ys.desktop"),
                    &exported_shortcut("Ys", 3),
                )
                .with_stray_row(
                    FRONTEND_DOCUMENT,
                    TreeDocument {
                        document_id: format!("{FRONTEND_DOCUMENT}/RPG/Ys.desktop"),
                        display_name: "Ys.desktop".into(),
                        mime_type: "application/octet-stream".into(),
                        size: Some(64),
                    },
                );
            let scan = scan(&profile, &source(tree)).unwrap();
            let titles = scan
                .shortcuts
                .iter()
                .map(|shortcut| shortcut.title.as_str())
                .collect::<Vec<_>>();
            assert_eq!(titles, ["Celeste"]);
        }

        #[test]
        fn walks_a_subfolder_the_provider_reports() {
            let profile = granted_profile();
            let tree = frontend(&[("Celeste.desktop", exported_shortcut("Celeste", 1))])
                .with_directory(&format!("{FRONTEND_DOCUMENT}/RPG"))
                .with_document(
                    &format!("{FRONTEND_DOCUMENT}/RPG/Ys.desktop"),
                    &exported_shortcut("Ys", 3),
                );
            let source = source(tree);
            let scan = scan(&profile, &source).unwrap();

            let mut paths = scan
                .shortcuts
                .iter()
                .map(|shortcut| shortcut.shortcut_path.clone())
                .collect::<Vec<_>>();
            paths.sort();
            assert_eq!(
                paths,
                [
                    PathBuf::from("/storage/emulated/0/Download/Winlator/Frontend/Celeste.desktop"),
                    PathBuf::from("/storage/emulated/0/Download/Winlator/Frontend/RPG/Ys.desktop"),
                ]
            );
        }

        #[test]
        fn refuses_a_document_too_large_to_be_a_shortcut() {
            let profile = granted_profile();
            let oversized = "x".repeat(MAX_SHORTCUT_BYTES as usize + 1);
            let source = source(frontend(&[("Huge.desktop", oversized)]));
            assert_eq!(scan(&profile, &source).unwrap().shortcuts.len(), 0);
            assert_eq!(
                validate_winlator_shortcut_for_profile(
                    &profile,
                    &source,
                    Path::new("/storage/emulated/0/Download/Winlator/Frontend/Huge.desktop"),
                    &AtomicBool::new(false)
                ),
                Err(WinlatorRunnerError::ShortcutTooLarge)
            );
        }

        #[test]
        fn refuses_a_shortcut_outside_the_granted_folder() {
            let profile = granted_profile();
            let source = source(frontend(&[(
                "Celeste.desktop",
                exported_shortcut("Celeste", 1),
            )]));
            for outside in [
                "/storage/emulated/0/Download/Celeste.desktop",
                "/storage/emulated/0/Download/Winlator/FrontendEvil/Celeste.desktop",
                "/etc/hosts.desktop",
            ] {
                assert_eq!(
                    validate_winlator_shortcut_for_profile(
                        &profile,
                        &source,
                        Path::new(outside),
                        &AtomicBool::new(false)
                    ),
                    Err(WinlatorRunnerError::ShortcutOutsideScope),
                    "accepted {outside}"
                );
            }
        }

        /// The window between preparing the intent and sending it is the one an
        /// attacker controls, and re-exporting a shortcut is how Winlator itself
        /// walks into it.
        #[test]
        fn refuses_to_send_an_intent_for_a_document_rewritten_mid_launch() {
            let profile = granted_profile();
            let tree = frontend(&[("Celeste.desktop", exported_shortcut("Celeste", 4))]);
            let source = source(tree.clone());
            let scan = scan(&profile, &source).unwrap();
            let candidate = &scan.shortcuts[0];
            let entry = inventory(&profile, candidate);
            let intent = WinlatorLaunchIntent::new(&profile.id, &candidate.game_ref).unwrap();
            let prepared = prepare_winlator_launch(&profile, &source, &entry, &intent).unwrap();

            tree.write(
                &format!("{FRONTEND_DOCUMENT}/Celeste.desktop"),
                &exported_shortcut("Celeste", 5),
            );
            assert_eq!(
                prepared.launch(&source),
                Err(WinlatorRunnerError::ShortcutNotLaunchable)
            );
        }

        #[test]
        fn a_cancelled_grant_scan_reads_nothing() {
            let profile = granted_profile();
            let tree = frontend(&[("Celeste.desktop", exported_shortcut("Celeste", 1))]);
            let source = source(tree.clone());
            assert_eq!(
                scan_winlator_shortcuts(
                    &profile,
                    &source,
                    &AtomicBool::new(true),
                    ScanLimits::default(),
                    |_| {}
                ),
                Err(WinlatorRunnerError::Cancelled)
            );
            assert_eq!(tree.reads(), 0);
        }

        #[test]
        fn a_grant_scan_beyond_its_file_budget_is_refused_rather_than_truncated() {
            let profile = granted_profile();
            let documents = (0..4)
                .map(|index| (format!("Game{index}.desktop"), exported_shortcut("Game", 1)))
                .collect::<Vec<_>>();
            let mut tree = FakeDocumentTree::new(FRONTEND_DOCUMENT);
            for (name, contents) in &documents {
                tree = tree.with_document(&format!("{FRONTEND_DOCUMENT}/{name}"), contents);
            }
            assert_eq!(
                scan_winlator_shortcuts(
                    &profile,
                    &source(tree),
                    &AtomicBool::new(false),
                    ScanLimits {
                        max_files: 2,
                        max_depth: 2
                    },
                    |_| {}
                ),
                Err(WinlatorRunnerError::TooManyFiles)
            );
        }

        /// One unreadable document must not hide the rest of a library.
        #[test]
        fn skips_a_document_the_provider_refuses_and_keeps_the_others() {
            let profile = granted_profile();
            let tree = frontend(&[
                ("Celeste.desktop", exported_shortcut("Celeste", 1)),
                ("Braid.desktop", exported_shortcut("Braid", 2)),
            ])
            .with_unreadable(&format!("{FRONTEND_DOCUMENT}/Braid.desktop"));
            let scan = scan(&profile, &source(tree)).unwrap();
            let titles = scan
                .shortcuts
                .iter()
                .map(|shortcut| shortcut.title.as_str())
                .collect::<Vec<_>>();
            assert_eq!(titles, ["Celeste"]);
        }

        /// A permission the user revoked in the system settings stops being
        /// persisted. Reading the folder by pathname instead would be reaching
        /// around the grant, so the answer is a sentence asking for it back.
        #[test]
        fn a_revoked_grant_asks_to_be_connected_again() {
            let profile = granted_profile();
            assert_eq!(
                document_tree_source_with(
                    &profile,
                    Path::new(EXTERNAL_ROOT),
                    &[
                        "content://com.android.externalstorage.documents/tree/primary%3AOther"
                            .to_string()
                    ],
                    |_| Box::new(FakeDocumentTree::default()),
                )
                .err(),
                Some(WinlatorRunnerError::ExportFolderAccessLost)
            );
            // Still granted: the same profile resolves to a readable source.
            assert!(
                document_tree_source_with(
                    &profile,
                    Path::new(EXTERNAL_ROOT),
                    &[FRONTEND_TREE.to_string()],
                    |_| Box::new(FakeDocumentTree::new(FRONTEND_DOCUMENT)),
                )
                .is_ok()
            );
        }

        /// Every use of a grant re-checks how broad it is, not only the moment
        /// it was picked: a grant persisted by an older build, or one whose
        /// folder is a drop folder, must stop being read rather than be trusted
        /// because it is already in the catalog.
        #[test]
        fn refuses_to_read_a_grant_on_a_folder_anything_can_write_into() {
            let mut profile = granted_profile();
            for tree_uri in [
                "content://com.android.externalstorage.documents/tree/primary%3A",
                "content://com.android.externalstorage.documents/tree/primary%3ADownload",
                "content://com.android.externalstorage.documents/tree/primary%3ADCIM",
            ] {
                profile.shortcut_trees = vec![tree_uri.into()];
                assert_eq!(
                    document_tree_source_with(
                        &profile,
                        Path::new(EXTERNAL_ROOT),
                        &profile.shortcut_trees,
                        |_| Box::new(FakeDocumentTree::default()),
                    )
                    .err(),
                    Some(WinlatorRunnerError::ExportFolderTooBroad),
                    "read {tree_uri}"
                );
            }
        }

        /// The sentence has to name the folder to look for, and name it once:
        /// the constant is the only place that path is written.
        #[test]
        fn the_refusal_names_winlators_own_export_folder() {
            assert!(
                WinlatorRunnerError::ExportFolderTooBroad
                    .to_string()
                    .contains(DEFAULT_FRONTEND_SHORTCUT_DIRECTORY)
            );
        }

        /// A folder whose documents are rows in a cloud index can never become a
        /// path Winlator opens, so it is refused before anything is persisted.
        #[test]
        fn refuses_a_grant_on_a_provider_that_has_no_file_path() {
            let mut profile = granted_profile();
            profile.shortcut_trees =
                vec!["content://com.android.providers.downloads.documents/tree/downloads".into()];
            assert_eq!(
                document_tree_source_with(
                    &profile,
                    Path::new(EXTERNAL_ROOT),
                    &profile.shortcut_trees,
                    |_| Box::new(FakeDocumentTree::default()),
                )
                .err(),
                Some(WinlatorRunnerError::ExportFolderUnsupported)
            );
        }

        /// There is no document provider on a desktop, and no honest fallback: a
        /// profile that carries a grant must not quietly read the pathname.
        #[cfg(not(target_os = "android"))]
        #[test]
        fn a_desktop_host_refuses_a_grant_instead_of_reading_the_path_behind_it() {
            let profile = granted_profile();
            assert_eq!(
                shortcut_source_for_profile(&profile).err(),
                Some(WinlatorRunnerError::ExportFolderUnsupported)
            );
        }
    }

    /// Each error carries its own sentence on every platform, including the three
    /// only a device produces. A duplicate would mean one of them is unsayable.
    #[test]
    fn every_refusal_has_its_own_sentence() {
        let messages = [
            WinlatorRunnerError::Cancelled,
            WinlatorRunnerError::PlatformUnsupported,
            WinlatorRunnerError::DistributionNotLaunchable,
            WinlatorRunnerError::ProfileDisabled,
            WinlatorRunnerError::InvalidProfile,
            WinlatorRunnerError::ShortcutMissing,
            WinlatorRunnerError::ShortcutOutsideScope,
            WinlatorRunnerError::ShortcutNotLaunchable,
            WinlatorRunnerError::AccessDenied,
            WinlatorRunnerError::TooManyFiles,
            WinlatorRunnerError::ShortcutTooLarge,
            WinlatorRunnerError::InvalidIntent,
            WinlatorRunnerError::InvalidPage,
            WinlatorRunnerError::WinlatorMissing,
            WinlatorRunnerError::WinlatorRefusedLaunch,
            WinlatorRunnerError::LaunchFailed,
            WinlatorRunnerError::ExportFolderUnsupported,
            WinlatorRunnerError::ExportFolderNotConnected,
            WinlatorRunnerError::ExportFolderAccessLost,
        ]
        .map(|error| error.to_string());
        let distinct = messages.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(distinct.len(), messages.len());
        assert!(messages.iter().all(|message| message.ends_with('.')));
    }

    /// A source with no grant at all is the state between installing Orivo and
    /// connecting a folder, and it has to say so rather than read nothing.
    #[test]
    fn a_grant_that_was_never_connected_says_so() {
        let source = DocumentTreeShortcuts::new();
        assert_eq!(
            source.roots().err(),
            Some(WinlatorRunnerError::ExportFolderNotConnected)
        );
    }
}
