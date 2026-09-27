//! Trusted native host adapters for Orivo's console-emulator runners on Android.
//!
//! These are Winlator's design applied to the emulators a player already has
//! installed: the WIT runner contract carries only opaque profile and game
//! identifiers, this module resolves them through private catalog data, rechecks
//! every filesystem boundary, and then builds one closed, explicit Android
//! intent. There is no shell, no `Runtime.exec`, and no command string anywhere.
//! What Orivo owns is the granted folder and the decision to send an intent;
//! everything else — the cores, the BIOS files, the save states — lives in the
//! emulator's own storage, which Orivo can neither read nor validate.
//!
//! Two things are genuinely not Winlator's.
//!
//! **A ROM is data, and it is large.** A `.desktop` file is read whole and
//! hashed at every launch; a 1.5 GB disc image cannot be. So the identity of a
//! ROM is its byte length together with a digest — of the whole file when it is
//! small enough to be one, of a bounded head when it is not. What that proves,
//! and what it does not, is written down at [`rom_fingerprint`].
//!
//! **One of these emulators opens a content URI.** PPSSPP declares `content` as
//! a data scheme and reads `intent.getData()`, so Orivo hands it the very
//! document it read, with a read grant attached, and never a path. RetroArch
//! takes a path in a string extra, which is the same narrow `primary:` mapping
//! Winlator needs and the same refusals.
//!
//! Which emulators these are, what each one exposes, and what was read to
//! establish it, is recorded in `docs/console-emulators.md`.

use crate::catalog::{
    ConsoleEmulator, ConsoleEmulatorProfile, ConsoleRomInventoryEntry, ConsoleSystem,
};
pub use crate::catalog::{PPSSPP_RUNNER_ID, RETROARCH_RUNNER_ID};
use crate::console_saf::{RomBytes, RomDocumentTree, rom_document_tree};
use crate::winlator_runner::{AndroidIntentExtra, WinlatorRunnerError};
use crate::winlator_saf::DocumentTreeGrant;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// A ROM folder holds one file per game. These bounds are an order of magnitude
/// above any real collection and still refuse a folder that was pointed at the
/// whole of shared storage by mistake.
pub const DEFAULT_MAX_SCAN_FILES: usize = 4_000;
pub const DEFAULT_MAX_SCAN_DEPTH: usize = 4;
const MAX_ROM_TITLE_CHARS: usize = 160;
/// What a granted folder is called when its own name cannot be shown.
const DEFAULT_ROOT_LABEL: &str = "Authorized ROM folder";

/// A ROM small enough to be hashed whole.
///
/// Every cartridge-era console is inside this by a wide margin: the largest
/// commercial SNES cartridge is 6 MB, the largest Mega Drive one 8 MB, the
/// largest Game Boy Advance one 32 MB. A disc image is not, and never will be.
pub const MAX_FULLY_HASHED_ROM_BYTES: u64 = 32 * 1024 * 1024;
/// How much of a larger image is hashed instead. Every disc format keeps its
/// header and its table of contents at the front, so this is the part that
/// changes when the image becomes a different game.
pub const ROM_HEAD_DIGEST_BYTES: u64 = 1024 * 1024;

/// `FLAG_ACTIVITY_NEW_TASK`.
///
/// Deliberately *not* Winlator's `CLEAR_TASK | CLEAR_TOP`: that set is what
/// Winlator Cmod documents for a frontend, and an emulator is not Winlator.
/// RetroArch handles a second launch itself — `RetroActivityFuture.onNewIntent`
/// compares the new `ROM` and `LIBRETRO` against the current ones and restarts
/// only when they differ — and clearing the task under it would take that
/// decision away from the app that owns the session.
const INTENT_FLAGS: i32 = 0x1000_0000;
/// `FLAG_GRANT_READ_URI_PERMISSION`. The whole of what an emulator receiving a
/// document is given: read access to one file, for one launch.
const FLAG_GRANT_READ_URI_PERMISSION: i32 = 0x0000_0001;
/// `Intent.ACTION_VIEW`.
const ACTION_VIEW: &str = "android.intent.action.VIEW";

/// The host-only equivalent of WIT's `launch-intent`. Its closed mode enum means
/// the WIT `mode` string can never become an Android component, an extra key, or
/// a process argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleLaunchIntent {
    runner_id: String,
    profile_id: String,
    game_ref: String,
    mode: ConsoleLaunchMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleLaunchMode {
    /// Start one ROM in the emulator the profile names. It is the only thing any
    /// of these emulators exposes to another app.
    Rom,
}

impl ConsoleLaunchIntent {
    pub fn new(
        runner_id: &str,
        profile_id: &str,
        game_ref: &str,
    ) -> Result<Self, ConsoleRunnerError> {
        if !crate::catalog::is_console_runner_id(runner_id)
            || !valid_opaque_id(profile_id)
            || !valid_opaque_id(game_ref)
        {
            return Err(ConsoleRunnerError::InvalidIntent);
        }
        Ok(Self {
            runner_id: runner_id.into(),
            profile_id: profile_id.into(),
            game_ref: game_ref.into(),
            mode: ConsoleLaunchMode::Rom,
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

    pub fn mode(&self) -> ConsoleLaunchMode {
        self.mode
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleRunnerError {
    Cancelled,
    PlatformUnsupported,
    ProfileDisabled,
    InvalidProfile,
    RomMissing,
    RomOutsideScope,
    /// The ROM is still where it was and is no longer the file that was
    /// confirmed, so nothing is handed over until a deliberate reimport.
    RomNotLaunchable,
    /// The folder answered, the file did not — including a provider that will not
    /// say how large a document is, which is half of what identifies it.
    RomUnreadable,
    /// The path names something *inside* an archive, so the bytes the host hashed
    /// are not the bytes the emulator would load.
    RomInsideArchive,
    /// A patch file sits beside the ROM, and the emulator would apply it without
    /// being asked.
    RomHasSidecarPatch,
    AccessDenied,
    TooManyFiles,
    InvalidIntent,
    InvalidPage,
    /// The three below are only ever produced by the Android intent layer. They
    /// still carry their own sentence on every platform, so a desktop build
    /// cannot drift out of sync with the message a device shows.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    EmulatorMissing(ConsoleEmulator),
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    EmulatorRefusedLaunch(ConsoleEmulator),
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    LaunchFailed(ConsoleEmulator),
    /// The four below belong to the storage access grant, and are the same four
    /// Winlator has: the rules they come from are shared, the sentences are not.
    RomFolderUnsupported,
    RomFolderTooBroad,
    RomFolderNotConnected,
    RomFolderAccessLost,
}

/// The grant rules live in `winlator_saf` because they are the platform's and not
/// Winlator's — which provider may become a path, and which folders any app can
/// drop a file into. Their *sentences* are not shared: a player who pointed Orivo
/// at a ROM folder should not be told about Winlator's export folder.
impl From<WinlatorRunnerError> for ConsoleRunnerError {
    fn from(error: WinlatorRunnerError) -> Self {
        match error {
            WinlatorRunnerError::ExportFolderUnsupported => Self::RomFolderUnsupported,
            WinlatorRunnerError::ExportFolderTooBroad => Self::RomFolderTooBroad,
            WinlatorRunnerError::ExportFolderNotConnected => Self::RomFolderNotConnected,
            WinlatorRunnerError::ExportFolderAccessLost => Self::RomFolderAccessLost,
            WinlatorRunnerError::ShortcutOutsideScope => Self::RomOutsideScope,
            WinlatorRunnerError::ShortcutMissing => Self::RomMissing,
            WinlatorRunnerError::Cancelled => Self::Cancelled,
            WinlatorRunnerError::TooManyFiles => Self::TooManyFiles,
            // Everything left is a Winlator-shaped failure this side cannot
            // produce — a `.desktop` file that is too large, a distribution with
            // no exported activity — so it becomes the one honest answer rather
            // than a sentence about a shortcut.
            _ => Self::AccessDenied,
        }
    }
}

impl std::fmt::Display for ConsoleRunnerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Cancelled => "The ROM import was cancelled.",
            Self::PlatformUnsupported => {
                "Console games can only be started on Android, where the emulator is."
            }
            Self::ProfileDisabled => {
                "This emulator is switched off in Orivo. Turn it on and try again."
            }
            Self::InvalidProfile => {
                "This emulator's setup is no longer valid. Review it and try again."
            }
            Self::RomMissing => "This ROM is no longer in the folder you granted.",
            Self::RomOutsideScope => "This ROM is outside the folders allowed for this emulator.",
            Self::RomNotLaunchable => {
                "This file changed since you added it. Add it again so Orivo knows what it is."
            }
            Self::RomUnreadable => "Orivo could not read this ROM. Check the folder and try again.",
            Self::RomInsideArchive => {
                "This game is inside an archive. Unpack it into the folder so Orivo can see what it is adding."
            }
            Self::RomHasSidecarPatch => {
                "A patch file sits next to this game, and the emulator would apply it without asking. Move it out of the folder, or add the patched game itself."
            }
            Self::AccessDenied => {
                "Orivo could not read one of the folders allowed for this emulator."
            }
            Self::TooManyFiles => {
                "This folder contains too many files to scan at once. Choose the folder your ROMs are in."
            }
            Self::InvalidIntent => "This launch request is invalid.",
            Self::InvalidPage => "This ROM list is no longer available.",
            Self::EmulatorMissing(emulator) => {
                return write!(
                    formatter,
                    "{} is not installed on this device. Install it and try again.",
                    emulator.label()
                );
            }
            Self::EmulatorRefusedLaunch(emulator) => {
                return write!(
                    formatter,
                    "{} refused the launch request. This build does not let another app start a game.",
                    emulator.label()
                );
            }
            Self::LaunchFailed(emulator) => {
                return write!(
                    formatter,
                    "{} could not start this game. Try again.",
                    emulator.label()
                );
            }
            Self::RomFolderUnsupported => {
                "Orivo can only read a folder in this device's own storage. Choose the folder your ROMs are in."
            }
            Self::RomFolderTooBroad => {
                "That folder is one every app can drop files into. Make a folder for your ROMs inside it and choose that."
            }
            Self::RomFolderNotConnected => "Connect the folder your ROMs are in, then try again.",
            Self::RomFolderAccessLost => {
                "Orivo no longer has access to that ROM folder. Connect it again."
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ConsoleRunnerError {}

/// How a ROM reaches the emulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RomDelivery {
    /// A filesystem path in a string extra. The emulator opens it with its own
    /// permissions, which means the path has to be a real one — the same narrow
    /// `primary:` mapping Winlator needs, and the same refusals.
    Path,
    /// The document Orivo read, handed over with `FLAG_GRANT_READ_URI_PERMISSION`
    /// on it. Nothing about where shared storage is mounted is involved, and the
    /// emulator gets read access to exactly one file.
    Document,
}

/// One emulator Orivo is willing to address, and everything about it that is a
/// compile-time constant. Every string here was read from that project's own
/// manifest and source and then from the APK that was installed to verify it: no
/// value on this table can come from the WebView, the catalog, or a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConsoleLaunchSurface {
    emulator: ConsoleEmulator,
    runner_id: &'static str,
    /// Every published package that ships this activity, in the order Orivo tries
    /// them. One app has several published builds — RetroArch's ABI flavours,
    /// PPSSPP's Gold — the activity class name is the same in all of them, and
    /// Orivo may not ask the platform which one is installed: package visibility
    /// on API 30+ needs a `<queries>` manifest entry, and `src-tauri/gen/` is not
    /// tracked. So the hand-off tries each and reads the platform's answer.
    packages: &'static [&'static str],
    activity: &'static str,
    delivery: RomDelivery,
    /// `Intent.setAction`, where the emulator's entry point is an action rather
    /// than bare extras.
    action: Option<&'static str>,
    /// Does this emulator's loader read files *beside* the one it was given?
    ///
    /// RetroArch does: `runloop_path_fill_names` in `runloop.c` truncates the
    /// content path at its last dot and looks for `<that>.ips`, `.bps`, `.ups`
    /// and `.xdelta`, then `task_content.c` applies whichever it finds unless
    /// `--no-patch` was passed — which an intent cannot pass. So a file the host
    /// never hashed decides what runs, and the host has to look for it.
    applies_soft_patches: bool,
}

/// The extra keys RetroArch's own frontend interface documents.
///
/// `ROM` becomes `args->content_path` and `LIBRETRO` becomes
/// `args->libretro_path` in `frontend/drivers/platform_unix.c`. Both are paths,
/// and `LIBRETRO` is a library RetroArch will `dlopen`, which is exactly why it
/// is composed here from two compile-time constants and a closed enum.
const RETROARCH_ROM_EXTRA: &str = "ROM";
const RETROARCH_CORE_EXTRA: &str = "LIBRETRO";
/// Where RetroArch keeps the cores it downloaded: `ApplicationInfo.dataDir` plus
/// `cores` (`platform_unix.c`, `DEFAULT_DIR_CORE`).
///
/// `dataDir` is *asked for*, through a `<queries>` entry the tracked Android
/// library project of `tauri-plugin-orivo-saf` merges into the app manifest —
/// which is the only manifest a commit in this repository can reach, since
/// `src-tauri/gen/android` is regenerated. The constant below is the fallback for
/// a device that answers nothing: it is what a primary-user install has, and a
/// secondary user's `/data/user/<id>/…` is exactly the case the query covers.
const ANDROID_PRIMARY_USER_DATA_ROOT: &str = "/data/user/0";
const RETROARCH_CORE_DIRECTORY: &str = "cores";

fn launch_surface(emulator: ConsoleEmulator) -> ConsoleLaunchSurface {
    match emulator {
        ConsoleEmulator::RetroArch => ConsoleLaunchSurface {
            emulator,
            runner_id: RETROARCH_RUNNER_ID,
            // The ABI flavours, most specific first. `com.retroarch.aarch64` is
            // both the 64-bit buildbot build and Play's "RetroArch Plus".
            packages: &[
                "com.retroarch.aarch64",
                "com.retroarch",
                "com.retroarch.ra32",
            ],
            activity: "com.retroarch.browser.retroactivity.RetroActivityFuture",
            delivery: RomDelivery::Path,
            action: None,
            applies_soft_patches: true,
        },
        ConsoleEmulator::Ppsspp => ConsoleLaunchSurface {
            emulator,
            runner_id: PPSSPP_RUNNER_ID,
            packages: &["org.ppsspp.ppsspp", "org.ppsspp.ppssppgold"],
            activity: "org.ppsspp.ppsspp.PpssppActivity",
            delivery: RomDelivery::Document,
            action: Some(ACTION_VIEW),
            // PPSSPP is handed one document and has read access to nothing else,
            // so there is no "beside" for it to read.
            applies_soft_patches: false,
        },
    }
}

/// The libretro core RetroArch is asked to load for a console.
///
/// A core is a shared library RetroArch will `dlopen`, so this is a closed table
/// and not a name from anywhere else. Each entry is the upstream core's own
/// Android library name, which is what RetroArch's own core downloader writes
/// into its core directory.
fn libretro_core(system: ConsoleSystem) -> Option<&'static str> {
    match system {
        ConsoleSystem::Nes => Some("fceumm_libretro_android.so"),
        ConsoleSystem::Snes => Some("snes9x_libretro_android.so"),
        ConsoleSystem::GameBoy => Some("gambatte_libretro_android.so"),
        ConsoleSystem::GameBoyAdvance => Some("mgba_libretro_android.so"),
        ConsoleSystem::MegaDrive => Some("genesis_plus_gx_libretro_android.so"),
        // Orivo names no libretro PSP core: PPSSPP itself is the profile for that
        // console, and the catalog refuses the pairing anyway.
        ConsoleSystem::PlayStationPortable => None,
    }
}

/// What the platform says about one emulator package.
///
/// Package visibility is not a permission and grants nothing — an explicit
/// component already bypasses intent filters, so the launch never needed it. What
/// it buys is the ability to *ask*: where this emulator actually keeps its data,
/// and who installed it. Both are facts about the app a game is about to be handed
/// to, and both used to be composed or unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    /// `ApplicationInfo.dataDir`, when the platform answered.
    pub data_directory: Option<PathBuf>,
    /// The package that installed it — Play, F-Droid, a file manager, or nothing
    /// at all for a sideload.
    pub installer: Option<String>,
}

/// The platform's answer about the packages Orivo names, or silence.
///
/// A trait because everything above it is pure and has to stay testable on a
/// host: a device answers through JNI, a desktop answers nothing, and a test
/// answers whatever the case under test needs.
pub trait InstalledPackages {
    fn lookup(&self, package: &str) -> Option<InstalledPackage>;
}

/// What a desktop build, and any device that will not answer, knows.
///
/// On a device this is only the fallback inside [`installed_packages`], so a
/// release build for Android never names it — and every test does.
#[cfg_attr(target_os = "android", allow(dead_code))]
pub struct NoInstalledPackages;

impl InstalledPackages for NoInstalledPackages {
    fn lookup(&self, _package: &str) -> Option<InstalledPackage> {
        None
    }
}

/// The platform on this build.
pub fn installed_packages() -> Box<dyn InstalledPackages> {
    #[cfg(target_os = "android")]
    {
        Box::new(android::AndroidPackages)
    }
    #[cfg(not(target_os = "android"))]
    {
        Box::new(NoInstalledPackages)
    }
}

/// Which of an emulator's published packages are installed, most specific first,
/// and what the platform said about each.
///
/// A package the platform does not answer about is still tried: the query can
/// fail for reasons that have nothing to do with the app being absent, and a
/// launch that refused on silence would be worse than one that finds out from
/// `ActivityNotFoundException`. Installed ones simply go first.
pub fn emulator_packages(
    emulator: ConsoleEmulator,
    packages: &dyn InstalledPackages,
) -> Vec<(&'static str, Option<InstalledPackage>)> {
    let surface = launch_surface(emulator);
    let mut known = Vec::new();
    let mut unknown = Vec::new();
    for package in surface.packages {
        match packages.lookup(package) {
            Some(facts) => known.push((*package, Some(facts))),
            None => unknown.push((*package, None)),
        }
    }
    known.extend(unknown);
    known
}

/// One document, named the way the platform names it.
///
/// The URI is deliberately *not* assembled here.
/// `DocumentsContract.buildDocumentUriUsingTree` is the platform's own builder
/// and the JNI layer calls it, so nothing in Orivo depends on the shape of a
/// `content://` string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomDocument {
    pub tree_uri: String,
    pub document_id: String,
}

/// A fully resolved Android intent, with no room for a free-form command.
///
/// Keys are `&'static str` from the table above; values are either a path the
/// host produced from a scope-checked file or a document it holds a read grant
/// on. Building one is pure, which is what lets `cargo test` assert the exact
/// intent on macOS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleIntent {
    package: &'static str,
    activity: &'static str,
    action: Option<&'static str>,
    flags: i32,
    data: Option<RomDocument>,
    extras: Vec<AndroidIntentExtra>,
}

/// These accessors are read by the Android intent layer and by the host tests
/// that assert the exact component and extras. A desktop build links neither,
/// which is the only reason this needs an allowance.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
impl ConsoleIntent {
    pub fn package(&self) -> &str {
        self.package
    }

    pub fn activity(&self) -> &str {
        self.activity
    }

    pub fn action(&self) -> Option<&str> {
        self.action
    }

    pub fn flags(&self) -> i32 {
        self.flags
    }

    pub fn data(&self) -> Option<&RomDocument> {
        self.data.as_ref()
    }

    pub fn extras(&self) -> &[AndroidIntentExtra] {
        &self.extras
    }
}

/// Where a profile's ROMs are read from.
///
/// Two grants, one pipeline, exactly as Winlator has: a readable directory
/// canonicalises and opens a pathname, a storage access grant resolves a document
/// identifier and opens a stream. Everything above this — the bounded walk, the
/// extensions, the fingerprint, the intent — cannot tell them apart.
pub trait RomSource {
    fn roots(&self) -> Result<Vec<RomRoot>, ConsoleRunnerError>;

    fn entries(&self, directory: &Path) -> Result<Vec<RomEntry>, ConsoleRunnerError>;

    /// Turn a candidate into the identity the host will store, refusing anything
    /// the profile did not grant and anything this console does not play.
    fn resolve(&self, rom: &Path) -> Result<PathBuf, ConsoleRunnerError>;

    /// The ROM's byte length, and up to `max_bytes` of its head.
    fn read_head(&self, rom: &Path, max_bytes: u64) -> Result<RomBytes, ConsoleRunnerError>;

    /// The document this ROM is, for an emulator that takes a content URI. A
    /// plain readable directory has none, and says so rather than handing over a
    /// `file://` URI — which the platform refuses to let one app give another.
    fn document(&self, rom: &Path) -> Result<RomDocument, ConsoleRunnerError>;

    /// The names that exist beside this file, in the same folder.
    ///
    /// Asked for rather than probed one pathname at a time, because the storage
    /// access grant answers a *listing* and cannot answer "does this path exist".
    fn sibling_names(&self, rom: &Path) -> Result<Vec<String>, ConsoleRunnerError>;
}

/// Refuse a ROM an emulator would load with something the host never saw.
///
/// Only for the emulators whose loader does that, and only immediately before the
/// hand-off: a patch dropped into the folder after the import is exactly the case
/// this exists for, and the import cannot see the future.
fn refuse_sidecar_patch(
    surface: &ConsoleLaunchSurface,
    source: &dyn RomSource,
    rom: &Path,
) -> Result<(), ConsoleRunnerError> {
    if !surface.applies_soft_patches {
        return Ok(());
    }
    let beside = source.sibling_names(rom)?;
    let patches = crate::source_review::soft_patch_siblings(rom);
    let named = patches
        .iter()
        .filter_map(|patch| patch.file_name().and_then(|name| name.to_str()))
        .collect::<Vec<_>>();
    // The comparison is case-insensitive because the loader lowercases nothing
    // and the filesystem under shared storage does not either: `.IPS` is the
    // same file to a user and a different string to a `==`.
    if beside.iter().any(|existing| {
        named
            .iter()
            .any(|patch| existing.eq_ignore_ascii_case(patch))
    }) {
        return Err(ConsoleRunnerError::RomHasSidecarPatch);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomRoot {
    pub directory: PathBuf,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomEntry {
    pub path: PathBuf,
    pub kind: RomEntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RomEntryKind {
    Directory,
    File,
    /// Counted against the scan's budget and never opened: a symlink, a device
    /// node, or a provider row that did not belong to the folder being listed.
    Ignored,
}

impl RomEntry {
    fn ignored() -> Self {
        // The walker matches on the kind before it ever looks at the path, so an
        // ignored entry deliberately carries none.
        Self {
            path: PathBuf::new(),
            kind: RomEntryKind::Ignored,
        }
    }
}

/// A grant that is an ordinary readable directory.
pub struct FilesystemRoms<'a> {
    directories: &'a [PathBuf],
    /// The *emulator*, not one of its consoles: a folder holds whatever the user
    /// put there, and one connect covers every console that emulator runs. Which
    /// console a file is for is the file's extension, and the catalog then pins it
    /// to the profile for that console.
    emulator: ConsoleEmulator,
}

impl<'a> FilesystemRoms<'a> {
    pub fn for_profile(profile: &'a ConsoleEmulatorProfile) -> Self {
        Self {
            directories: &profile.rom_directories,
            emulator: profile.emulator,
        }
    }
}

impl RomSource for FilesystemRoms<'_> {
    fn roots(&self) -> Result<Vec<RomRoot>, ConsoleRunnerError> {
        let mut roots = Vec::new();
        for directory in self.directories {
            let directory =
                fs::canonicalize(directory).map_err(|_| ConsoleRunnerError::AccessDenied)?;
            if !directory.is_dir() {
                return Err(ConsoleRunnerError::AccessDenied);
            }
            roots.push(RomRoot {
                label: safe_label(&directory, DEFAULT_ROOT_LABEL),
                directory,
            });
        }
        Ok(roots)
    }

    fn entries(&self, directory: &Path) -> Result<Vec<RomEntry>, ConsoleRunnerError> {
        let listing = fs::read_dir(directory).map_err(|_| ConsoleRunnerError::AccessDenied)?;
        let mut entries = Vec::new();
        for entry in listing {
            let entry = entry.map_err(|_| ConsoleRunnerError::AccessDenied)?;
            let file_type = entry
                .file_type()
                .map_err(|_| ConsoleRunnerError::AccessDenied)?;
            // Never traverse a symlink: the canonicalisation in `resolve` is a
            // second line of defence for files and overlapping granted roots.
            let kind = if file_type.is_symlink() {
                RomEntryKind::Ignored
            } else if file_type.is_dir() {
                RomEntryKind::Directory
            } else if file_type.is_file() {
                RomEntryKind::File
            } else {
                RomEntryKind::Ignored
            };
            entries.push(RomEntry {
                path: entry.path(),
                kind,
            });
        }
        Ok(entries)
    }

    fn resolve(&self, rom: &Path) -> Result<PathBuf, ConsoleRunnerError> {
        // Before the canonicalisation, because a file literally named
        // `pack.zip#Alter Ego.nes` exists and canonicalises perfectly well — and
        // is the one an emulator would read as an entry inside `pack.zip`.
        if !crate::source_review::is_plain_file_path(rom) {
            return Err(ConsoleRunnerError::RomInsideArchive);
        }
        let rom = fs::canonicalize(rom).map_err(|_| ConsoleRunnerError::RomMissing)?;
        if !crate::source_review::is_plain_file_path(&rom) {
            return Err(ConsoleRunnerError::RomInsideArchive);
        }
        if !rom.is_file() || self.emulator.system_for(&rom).is_none() {
            return Err(ConsoleRunnerError::RomMissing);
        }
        if !belongs_to_grant(&rom, self.directories)? {
            return Err(ConsoleRunnerError::RomOutsideScope);
        }
        Ok(rom)
    }

    fn read_head(&self, rom: &Path, max_bytes: u64) -> Result<RomBytes, ConsoleRunnerError> {
        read_rom_head(rom, max_bytes)
    }

    fn document(&self, _rom: &Path) -> Result<RomDocument, ConsoleRunnerError> {
        Err(ConsoleRunnerError::RomFolderNotConnected)
    }

    fn sibling_names(&self, rom: &Path) -> Result<Vec<String>, ConsoleRunnerError> {
        let directory = rom.parent().ok_or(ConsoleRunnerError::RomMissing)?;
        Ok(self
            .entries(directory)?
            .into_iter()
            .filter_map(|entry| {
                entry
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .collect())
    }
}

/// A grant the user handed over with `ACTION_OPEN_DOCUMENT_TREE`.
///
/// Every ROM is checked twice, because the two halves of a SAF grant can
/// disagree: once as a document identifier that has to sit under the granted
/// tree, and once as a path that has to sit under the directory that tree stands
/// for. The first stops a provider from answering with somebody else's row; the
/// second stops an identifier that resolves outside the folder — a `..` in the
/// middle of it — from becoming the path an emulator is handed.
pub struct DocumentTreeRoms {
    trees: Vec<GrantedTree>,
    emulator: ConsoleEmulator,
}

struct GrantedTree {
    grant: DocumentTreeGrant,
    tree: Box<dyn RomDocumentTree>,
}

impl DocumentTreeRoms {
    pub fn new(emulator: ConsoleEmulator) -> Self {
        Self {
            trees: Vec::new(),
            emulator,
        }
    }

    pub fn with_tree(mut self, grant: DocumentTreeGrant, tree: Box<dyn RomDocumentTree>) -> Self {
        self.trees.push(GrantedTree { grant, tree });
        self
    }

    /// The granted tree a path belongs to, and the identifier that names it
    /// there. A path no grant covers never becomes a document.
    fn locate(&self, path: &Path) -> Result<(&GrantedTree, String), ConsoleRunnerError> {
        if self.trees.is_empty() {
            return Err(ConsoleRunnerError::RomFolderNotConnected);
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
            .ok_or(ConsoleRunnerError::RomOutsideScope)
    }
}

impl RomSource for DocumentTreeRoms {
    fn roots(&self) -> Result<Vec<RomRoot>, ConsoleRunnerError> {
        if self.trees.is_empty() {
            return Err(ConsoleRunnerError::RomFolderNotConnected);
        }
        Ok(self
            .trees
            .iter()
            .map(|granted| RomRoot {
                directory: granted.grant.directory().to_path_buf(),
                label: safe_label(granted.grant.directory(), DEFAULT_ROOT_LABEL),
            })
            .collect())
    }

    fn entries(&self, directory: &Path) -> Result<Vec<RomEntry>, ConsoleRunnerError> {
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
                    // A provider is free to answer a listing with any row at all,
                    // so a row that does not resolve to a child of the folder
                    // being listed is dropped rather than followed.
                    Ok(path) if path.parent() == Some(directory) => RomEntry {
                        path,
                        kind: if row.is_directory() {
                            RomEntryKind::Directory
                        } else {
                            RomEntryKind::File
                        },
                    },
                    _ => RomEntry::ignored(),
                }
            })
            .collect())
    }

    fn resolve(&self, rom: &Path) -> Result<PathBuf, ConsoleRunnerError> {
        self.locate(rom)?;
        // A provider may hand back a name with an archive delimiter in it, and
        // `primary:…/pack.zip#Alter Ego.nes` is a perfectly ordinary document
        // identifier that resolves to a perfectly ordinary path.
        if !crate::source_review::is_plain_file_path(rom) {
            return Err(ConsoleRunnerError::RomInsideArchive);
        }
        if self.emulator.system_for(rom).is_none() {
            return Err(ConsoleRunnerError::RomMissing);
        }
        // There is no canonicalisation and no existence probe here on purpose: a
        // path Orivo reaches only through SAF cannot be `stat`ed, and the read
        // that follows is the only honest answer about whether it is still there.
        Ok(rom.to_path_buf())
    }

    fn read_head(&self, rom: &Path, max_bytes: u64) -> Result<RomBytes, ConsoleRunnerError> {
        let (granted, document_id) = self.locate(rom)?;
        granted.tree.read_head(&document_id, max_bytes)
    }

    fn document(&self, rom: &Path) -> Result<RomDocument, ConsoleRunnerError> {
        let (granted, document_id) = self.locate(rom)?;
        Ok(RomDocument {
            tree_uri: granted.grant.tree_uri().to_string(),
            document_id,
        })
    }

    fn sibling_names(&self, rom: &Path) -> Result<Vec<String>, ConsoleRunnerError> {
        let directory = rom.parent().ok_or(ConsoleRunnerError::RomMissing)?;
        Ok(self
            .entries(directory)?
            .into_iter()
            .filter_map(|entry| {
                entry
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .collect())
    }
}

/// The source a profile is read through on this platform.
///
/// On a device a profile that carries a storage access grant is read through it,
/// and the grant has to still be persisted: a permission the user revoked in the
/// system settings is a sentence asking them to reconnect, never a silent empty
/// library. Everything else — every desktop build, and a device profile granted a
/// plainly readable directory — reads the filesystem.
pub fn rom_source_for_profile(
    profile: &ConsoleEmulatorProfile,
) -> Result<Box<dyn RomSource + '_>, ConsoleRunnerError> {
    if profile.rom_trees.is_empty() {
        return Ok(Box::new(FilesystemRoms::for_profile(profile)));
    }
    Ok(Box::new(document_tree_source(profile)?))
}

fn document_tree_source(
    profile: &ConsoleEmulatorProfile,
) -> Result<DocumentTreeRoms, ConsoleRunnerError> {
    document_tree_source_with(
        profile,
        &crate::winlator_saf::external_storage_root()?,
        &crate::winlator_saf::persisted_read_tree_uris()?,
        rom_document_tree,
    )
}

/// The decisions above, with the device's three answers passed in: where the
/// shared volume is mounted, which grants survived, and what reads a tree. Only
/// this shape can be exercised without a device, and the checks are the point.
fn document_tree_source_with(
    profile: &ConsoleEmulatorProfile,
    external_storage_root: &Path,
    persisted: &[String],
    reader: impl Fn(&str) -> Box<dyn RomDocumentTree>,
) -> Result<DocumentTreeRoms, ConsoleRunnerError> {
    let mut source = DocumentTreeRoms::new(profile.emulator);
    for tree_uri in &profile.rom_trees {
        // A permission the user revoked from the system settings simply stops
        // being listed. Asking for it back is the only honest answer; reading the
        // folder by pathname instead would be reaching around the grant.
        if !persisted.iter().any(|granted| granted == tree_uri) {
            return Err(ConsoleRunnerError::RomFolderAccessLost);
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
/// whose documents are rows in a cloud index. RetroArch is handed a *file path*,
/// so a folder Orivo cannot name as a path is refused here with a sentence rather
/// than stored and discovered to be useless at launch — and the refusal is the
/// same for PPSSPP, which does not need the path, because one rule the user can
/// understand beats two they cannot predict.
pub fn grant_for_picked_rom_folder(
    tree_uri: &str,
) -> Result<DocumentTreeGrant, ConsoleRunnerError> {
    let external_storage_root = crate::winlator_saf::external_storage_root()?;
    let grant = DocumentTreeGrant::parse(tree_uri, &external_storage_root)?;
    grant.refuse_if_too_broad()?;
    if !crate::winlator_saf::persisted_read_tree_uris()?
        .iter()
        .any(|granted| granted == grant.tree_uri())
    {
        return Err(ConsoleRunnerError::RomFolderAccessLost);
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
pub struct ScannedRom {
    pub game_ref: String,
    pub title: String,
    /// Which console this file is for, read from its extension. One connect
    /// covers every console the emulator runs, so this is what files a ROM under
    /// the right profile instead of asking the user to sort their folder first.
    pub system: ConsoleSystem,
    pub directory_label: String,
    pub rom_path: PathBuf,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomScanResult {
    pub scanned_files: usize,
    pub roms: Vec<ScannedRom>,
}

/// Enumerate the ROMs a profile's grants currently expose.
///
/// Breadth-bounded, depth-bounded, cancellable, symlinks never followed, every
/// candidate re-resolved before it receives an opaque reference — and written once
/// over a [`RomSource`], so a folder read through SAF is walked by exactly the
/// code a readable directory is.
pub fn scan_roms(
    profile: &ConsoleEmulatorProfile,
    source: &dyn RomSource,
    cancelled: &AtomicBool,
    limits: ScanLimits,
    mut progress: impl FnMut(usize),
) -> Result<RomScanResult, ConsoleRunnerError> {
    if !profile.enabled {
        return Err(ConsoleRunnerError::ProfileDisabled);
    }
    profile
        .validate()
        .map_err(|_| ConsoleRunnerError::InvalidProfile)?;
    if limits.max_files == 0 || limits.max_depth == 0 {
        return Err(ConsoleRunnerError::InvalidPage);
    }

    let mut candidates = BTreeMap::new();
    let mut scanned_files = 0;
    for root in source.roots()? {
        cancelled_or(cancelled)?;
        scan_directory(
            profile,
            source,
            &root,
            &root.directory,
            0,
            limits,
            cancelled,
            &mut scanned_files,
            &mut candidates,
            &mut progress,
        )?;
    }

    Ok(RomScanResult {
        scanned_files,
        roms: candidates.into_values().collect(),
    })
}

#[allow(clippy::too_many_arguments)]
fn scan_directory(
    profile: &ConsoleEmulatorProfile,
    source: &dyn RomSource,
    root: &RomRoot,
    directory: &Path,
    depth: usize,
    limits: ScanLimits,
    cancelled: &AtomicBool,
    scanned_files: &mut usize,
    candidates: &mut BTreeMap<String, ScannedRom>,
    progress: &mut impl FnMut(usize),
) -> Result<(), ConsoleRunnerError> {
    cancelled_or(cancelled)?;
    let mut entries = source.entries(directory)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));

    for entry in entries {
        cancelled_or(cancelled)?;
        *scanned_files = scanned_files.saturating_add(1);
        if *scanned_files > limits.max_files {
            return Err(ConsoleRunnerError::TooManyFiles);
        }
        if *scanned_files % 32 == 0 {
            progress(*scanned_files);
        }
        match entry.kind {
            RomEntryKind::Ignored => {}
            RomEntryKind::Directory => {
                if depth < limits.max_depth {
                    scan_directory(
                        profile,
                        source,
                        root,
                        &entry.path,
                        depth + 1,
                        limits,
                        cancelled,
                        scanned_files,
                        candidates,
                        progress,
                    )?;
                }
            }
            RomEntryKind::File => {
                let Some(system) = profile.emulator.system_for(&entry.path) else {
                    continue;
                };
                let Ok(rom) = source.resolve(&entry.path) else {
                    continue;
                };
                if rom == root.directory || !rom.starts_with(&root.directory) {
                    continue;
                }
                // A ROM that cannot be read is skipped rather than failing the
                // whole scan: one unreadable file must not hide a whole shelf.
                let Ok(candidate) =
                    read_rom_candidate(source, &rom, system, &root.label, cancelled)
                else {
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

/// Revalidate one host-owned ROM against a profile without accepting a scanner
/// reference from the caller. The WebView can name only a catalog id; the native
/// host resolves, scope-checks and hashes the stored path itself.
pub fn validate_rom_for_profile(
    profile: &ConsoleEmulatorProfile,
    source: &dyn RomSource,
    rom: &Path,
    cancelled: &AtomicBool,
) -> Result<ScannedRom, ConsoleRunnerError> {
    if !profile.enabled {
        return Err(ConsoleRunnerError::ProfileDisabled);
    }
    profile
        .validate()
        .map_err(|_| ConsoleRunnerError::InvalidProfile)?;
    cancelled_or(cancelled)?;
    let rom = source.resolve(rom)?;
    // The source answers for the whole emulator; a profile is one console of it,
    // and a `.smc` reaching an NES profile would be a card that launches with the
    // wrong core. The catalog holds the same invariant; this is the runner's own.
    if !profile.system.recognises_rom(&rom) {
        return Err(ConsoleRunnerError::RomNotLaunchable);
    }
    let label = rom
        .parent()
        .map(|directory| safe_label(directory, DEFAULT_ROOT_LABEL))
        .unwrap_or_else(|| DEFAULT_ROOT_LABEL.into());
    read_rom_candidate(source, &rom, profile.system, &label, cancelled)
}

/// Recheck a scan snapshot at the exact moment it crosses into persistence.
///
/// The same rule as Winlator's, for the same reason: the game reference is a hash
/// of the *path*, so a file replaced between the preview and the confirmation
/// looks identical to it, and any app can create a file in a shared folder with
/// no permission at all. The content digest is part of what is being confirmed.
pub fn revalidate_rom_import_candidate(
    profile: &ConsoleEmulatorProfile,
    source: &dyn RomSource,
    candidate: &ScannedRom,
    cancelled: &AtomicBool,
) -> Result<ScannedRom, ConsoleRunnerError> {
    let current = validate_rom_for_profile(profile, source, &candidate.rom_path, cancelled)?;
    if candidate.game_ref != current.game_ref
        || candidate.fingerprint != current.fingerprint
        || candidate.system != current.system
    {
        return Err(ConsoleRunnerError::RomNotLaunchable);
    }
    Ok(ScannedRom {
        directory_label: candidate.directory_label.clone(),
        ..current
    })
}

/// Read, bound and fingerprint one ROM the source has already resolved.
fn read_rom_candidate(
    source: &dyn RomSource,
    rom: &Path,
    system: ConsoleSystem,
    directory_label: &str,
    cancelled: &AtomicBool,
) -> Result<ScannedRom, ConsoleRunnerError> {
    cancelled_or(cancelled)?;
    Ok(ScannedRom {
        game_ref: game_reference_for(rom),
        system,
        // Orivo does not parse a ROM. A cartridge header holds a 12-character
        // shouted title at best and a disc image holds an internal id, and
        // neither is what the person who put the file there called it. The file
        // name is the name they chose.
        title: rom_title_from_filename(rom),
        directory_label: directory_label.into(),
        rom_path: rom.to_path_buf(),
        fingerprint: fingerprint_rom(source, rom, cancelled)?,
    })
}

/// The digest that decides whether this is still the file the user confirmed.
///
/// Two reads at most, and the second only for a file between the two bounds:
/// asking for [`ROM_HEAD_DIGEST_BYTES`] first is what reveals the true length
/// without reading 32 MiB of a 1.5 GB image to discover it is one.
fn fingerprint_rom(
    source: &dyn RomSource,
    rom: &Path,
    cancelled: &AtomicBool,
) -> Result<String, ConsoleRunnerError> {
    cancelled_or(cancelled)?;
    let head = source.read_head(rom, ROM_HEAD_DIGEST_BYTES)?;
    if head.length <= ROM_HEAD_DIGEST_BYTES {
        return Ok(rom_fingerprint(&head));
    }
    if head.length <= MAX_FULLY_HASHED_ROM_BYTES {
        cancelled_or(cancelled)?;
        let whole = source.read_head(rom, MAX_FULLY_HASHED_ROM_BYTES)?;
        return Ok(rom_fingerprint(&whole));
    }
    Ok(rom_fingerprint(&head))
}

/// What a ROM's fingerprint covers.
///
/// For anything up to [`MAX_FULLY_HASHED_ROM_BYTES`] — which is every cartridge
/// ever pressed — it is the whole file, and the answer is exact. For a disc image
/// it is the byte length and the first [`ROM_HEAD_DIGEST_BYTES`], and the honest
/// statement of what that catches is: a file replaced by a different game, a file
/// truncated, a file appended to. What it does not catch is an image of the same
/// length whose bytes were changed only past the head — a patch applied in place
/// to a game the user already approved. Reading 1.5 GB over a `ContentResolver`
/// before every launch is the price of catching that, and it is not one a player
/// pressing Play should pay.
///
/// The two are never comparable: the prefix says which one this is, and the
/// length is inside the digest either way.
fn rom_fingerprint(bytes: &RomBytes) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orivo-console-rom-v1\0");
    digest.update(bytes.length.to_le_bytes());
    digest.update(&bytes.head);
    let digest = digest.finalize();
    if bytes.length <= MAX_FULLY_HASHED_ROM_BYTES {
        format!("sha256:{digest:x}")
    } else {
        format!("sha256-head:{digest:x}")
    }
}

#[cfg(unix)]
fn read_rom_head(rom: &Path, max_bytes: u64) -> Result<RomBytes, ConsoleRunnerError> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};

    // `O_NOFOLLOW` rejects a leaf symlink introduced after the canonical scope
    // check, and the metadata is read from the open descriptor rather than the
    // pathname, so the length belongs to the file actually being read.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(rom)
        .map_err(rom_file_error)?;
    let metadata = file.metadata().map_err(rom_file_error)?;
    if !metadata.is_file() {
        return Err(ConsoleRunnerError::RomMissing);
    }
    let mut head = Vec::new();
    file.take(max_bytes).read_to_end(&mut head)?;
    Ok(RomBytes {
        length: metadata.len(),
        head,
    })
}

#[cfg(not(unix))]
fn read_rom_head(rom: &Path, max_bytes: u64) -> Result<RomBytes, ConsoleRunnerError> {
    use std::io::Read;

    let metadata = fs::symlink_metadata(rom).map_err(rom_file_error)?;
    if !metadata.is_file() {
        return Err(ConsoleRunnerError::RomMissing);
    }
    let file = fs::File::open(rom).map_err(rom_file_error)?;
    let mut head = Vec::new();
    file.take(max_bytes).read_to_end(&mut head)?;
    Ok(RomBytes {
        length: metadata.len(),
        head,
    })
}

impl From<io::Error> for ConsoleRunnerError {
    fn from(error: io::Error) -> Self {
        rom_file_error(error)
    }
}

fn rom_file_error(error: io::Error) -> ConsoleRunnerError {
    match error.kind() {
        io::ErrorKind::NotFound => ConsoleRunnerError::RomMissing,
        io::ErrorKind::PermissionDenied => ConsoleRunnerError::AccessDenied,
        _ => ConsoleRunnerError::RomUnreadable,
    }
}

/// Resolve a typed intent into the explicit Android intents Orivo is willing to
/// send for it, most specific package first.
///
/// This is pure: every filesystem check happens here, and the result carries no
/// capability beyond "hand this document, or this path, to this component". The
/// final content check is deliberately repeated in [`PreparedConsoleLaunch::launch`],
/// immediately before the intent leaves the process.
pub fn prepare_console_launch(
    profile: &ConsoleEmulatorProfile,
    source: &dyn RomSource,
    game: &ConsoleRomInventoryEntry,
    intent: &ConsoleLaunchIntent,
    packages: &dyn InstalledPackages,
) -> Result<PreparedConsoleLaunch, ConsoleRunnerError> {
    let surface = launch_surface(profile.emulator);
    if intent.runner_id() != surface.runner_id
        || intent.profile_id() != profile.id
        || intent.game_ref() != game.game_ref
        || intent.mode() != ConsoleLaunchMode::Rom
    {
        return Err(ConsoleRunnerError::InvalidIntent);
    }
    if !profile.enabled {
        return Err(ConsoleRunnerError::ProfileDisabled);
    }
    profile
        .validate()
        .map_err(|_| ConsoleRunnerError::InvalidProfile)?;
    if game.profile_id != profile.id {
        return Err(ConsoleRunnerError::InvalidIntent);
    }
    game.validate()
        .map_err(|_| ConsoleRunnerError::RomNotLaunchable)?;

    let current =
        validate_rom_for_profile(profile, source, &game.rom_path, &AtomicBool::new(false))?;
    if game.fingerprint != current.fingerprint || game.game_ref != current.game_ref {
        return Err(ConsoleRunnerError::RomNotLaunchable);
    }
    // The fingerprint answers "are these the same bytes?", and for an emulator
    // that reads files beside the one it is given, that is not the whole question.
    refuse_sidecar_patch(&surface, source, &current.rom_path)?;

    let handoff = match surface.delivery {
        RomDelivery::Document => RomHandoff::Document(source.document(&current.rom_path)?),
        RomDelivery::Path => {
            // The last place this can be asked, on the exact string that will
            // cross: the checks above were about a `Path`, and this is what the
            // emulator parses.
            if !crate::source_review::is_plain_file_path(&current.rom_path) {
                return Err(ConsoleRunnerError::RomInsideArchive);
            }
            RomHandoff::Path(
                // Android extras are Java strings. A pathname that is not valid
                // UTF-8 could not survive the crossing intact, so it is refused
                // here rather than silently replaced.
                current
                    .rom_path
                    .to_str()
                    .ok_or(ConsoleRunnerError::RomNotLaunchable)?
                    .to_string(),
            )
        }
    };

    let candidates = emulator_packages(profile.emulator, packages)
        .into_iter()
        .map(|(package, facts)| {
            console_intent(&surface, package, profile.system, &handoff, facts.as_ref())
        })
        .collect::<Vec<_>>();

    Ok(PreparedConsoleLaunch {
        emulator: profile.emulator,
        candidates,
        rom_path: current.rom_path,
        fingerprint: current.fingerprint,
        title: current.title,
    })
}

/// What a ROM is to the emulator that will open it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RomHandoff {
    Path(String),
    Document(RomDocument),
}

fn console_intent(
    surface: &ConsoleLaunchSurface,
    package: &'static str,
    system: ConsoleSystem,
    handoff: &RomHandoff,
    facts: Option<&InstalledPackage>,
) -> ConsoleIntent {
    let mut extras = Vec::new();
    let mut data = None;
    let mut flags = INTENT_FLAGS;
    match handoff {
        RomHandoff::Path(path) => {
            extras.push(AndroidIntentExtra::Text {
                key: RETROARCH_ROM_EXTRA,
                value: path.clone(),
            });
            if let Some(core) = libretro_core(system) {
                // The directory the platform reported, or the one a primary-user
                // install has. Either way the *file name* comes from the closed
                // table above: this is a library the emulator will `dlopen`.
                let directory = facts
                    .and_then(|facts| facts.data_directory.clone())
                    .unwrap_or_else(|| {
                        PathBuf::from(format!("{ANDROID_PRIMARY_USER_DATA_ROOT}/{package}"))
                    })
                    .join(RETROARCH_CORE_DIRECTORY);
                extras.push(AndroidIntentExtra::Text {
                    key: RETROARCH_CORE_EXTRA,
                    value: directory.join(core).to_string_lossy().into_owned(),
                });
            }
        }
        RomHandoff::Document(document) => {
            data = Some(document.clone());
            flags |= FLAG_GRANT_READ_URI_PERMISSION;
        }
    }
    ConsoleIntent {
        package,
        activity: surface.activity,
        action: surface.action,
        flags,
        data,
        extras,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedConsoleLaunch {
    emulator: ConsoleEmulator,
    /// One intent per published package of this emulator, in the order to try
    /// them. A package that is not installed answers `ActivityNotFoundException`
    /// and nothing else happens, which is the only way Orivo can find out.
    candidates: Vec<ConsoleIntent>,
    rom_path: PathBuf,
    fingerprint: String,
    title: String,
}

impl PreparedConsoleLaunch {
    /// Exposed only so a host test can assert the exact intents without an
    /// emulator; the hand-off below reads the field directly.
    #[allow(dead_code)]
    pub fn candidates(&self) -> &[ConsoleIntent] {
        &self.candidates
    }

    /// The name the player sees on the other side of the hand-off.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Send the prepared intent. Nothing is spawned and no process is owned: the
    /// emulator starts, and Orivo's only feedback is whether Android accepted the
    /// hand-off.
    pub fn launch(&self, source: &dyn RomSource) -> Result<(), ConsoleRunnerError> {
        // This is deliberately immediately before the hand-off. The earlier
        // checks resolved the ROM against its grant; this content-addressed
        // recheck rejects one replaced while the launch was being prepared
        // without changing its pathname.
        if fingerprint_rom(source, &self.rom_path, &AtomicBool::new(false))? != self.fingerprint {
            return Err(ConsoleRunnerError::RomNotLaunchable);
        }
        self.send()
    }

    #[cfg(target_os = "android")]
    fn send(&self) -> Result<(), ConsoleRunnerError> {
        android::start_activity(self.emulator, self.candidates.clone())
    }

    #[cfg(not(target_os = "android"))]
    fn send(&self) -> Result<(), ConsoleRunnerError> {
        Err(ConsoleRunnerError::PlatformUnsupported)
    }
}

/// The only part of this module that is not testable on the host. It builds each
/// intent through JNI on Android's main thread and reports back whether the
/// platform accepted one, so "RetroArch is not installed" reaches the user as a
/// sentence instead of a silently dropped tap.
#[cfg(target_os = "android")]
mod android {
    use super::{ConsoleEmulator, ConsoleIntent, ConsoleRunnerError, InstalledPackage};
    use crate::winlator_runner::AndroidIntentExtra;
    use crate::winlator_saf::android::{java_string, with_env};
    use jni::{JNIEnv, objects::JObject};
    use std::{path::PathBuf, sync::mpsc, time::Duration};

    /// `Build.VERSION_CODES.R`, where `getInstallSourceInfo` arrived.
    const ANDROID_R: i32 = 30;

    /// The platform's answers about the packages the `<queries>` entry in
    /// `tauri-plugin-orivo-saf`'s library manifest makes visible.
    ///
    /// Silence is an ordinary answer: a package that is not installed, a device
    /// that filtered the query anyway, or a build whose manifest predates the
    /// entry. The caller falls back rather than refusing.
    pub struct AndroidPackages;

    impl super::InstalledPackages for AndroidPackages {
        fn lookup(&self, package: &str) -> Option<InstalledPackage> {
            let package = package.to_string();
            with_env(move |env, activity| {
                let manager = env
                    .call_method(
                        activity,
                        "getPackageManager",
                        "()Landroid/content/pm/PackageManager;",
                        &[],
                    )?
                    .l()?;
                let name = env.new_string(&package)?;
                let information = env
                    .call_method(
                        &manager,
                        "getApplicationInfo",
                        "(Ljava/lang/String;I)Landroid/content/pm/ApplicationInfo;",
                        &[(&name).into(), 0i32.into()],
                    )?
                    .l()?;
                let data_directory = env
                    .get_field(&information, "dataDir", "Ljava/lang/String;")?
                    .l()?;
                Ok(Some(InstalledPackage {
                    data_directory: java_string(env, &data_directory).map(PathBuf::from),
                    installer: installer_of(env, &manager, &package),
                }))
            })
            .ok()
            .flatten()
        }
    }

    /// Who installed this package, when the platform will say.
    ///
    /// `getInstallSourceInfo` is API 30; below that only the deprecated
    /// `getInstallerPackageName` exists, and a sideload answers `null` on both.
    /// None of the three outcomes is an error: the user is being *shown* this,
    /// not gated on it.
    fn installer_of(env: &mut JNIEnv<'_>, manager: &JObject<'_>, package: &str) -> Option<String> {
        let sdk = env
            .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")
            .and_then(|version| version.i())
            .unwrap_or(0);
        let name = env.new_string(package).ok()?;
        let installer = if sdk >= ANDROID_R {
            let source = env
                .call_method(
                    manager,
                    "getInstallSourceInfo",
                    "(Ljava/lang/String;)Landroid/content/pm/InstallSourceInfo;",
                    &[(&name).into()],
                )
                .and_then(|source| source.l())
                .ok()?;
            env.call_method(
                &source,
                "getInstallingPackageName",
                "()Ljava/lang/String;",
                &[],
            )
            .and_then(|installer| installer.l())
            .ok()?
        } else {
            env.call_method(
                manager,
                "getInstallerPackageName",
                "(Ljava/lang/String;)Ljava/lang/String;",
                &[(&name).into()],
            )
            .and_then(|installer| installer.l())
            .ok()?
        };
        java_string(env, &installer)
    }

    /// Starting an activity is a handful of JNI calls on an already-running main
    /// thread. A wait this long only ever expires when that thread is wedged, in
    /// which case reporting a failure beats blocking the launch.
    const HAND_OFF_TIMEOUT: Duration = Duration::from_secs(5);

    /// Try each published package of this emulator in order.
    ///
    /// Only `ActivityNotFoundException` moves on to the next one: it means the
    /// package is not installed, which is the one thing Orivo cannot ask about.
    /// A `SecurityException` is a build that unexported the activity and a
    /// different sentence, and anything else is a failure worth reporting rather
    /// than retrying against another package.
    pub(super) fn start_activity(
        emulator: ConsoleEmulator,
        candidates: Vec<ConsoleIntent>,
    ) -> Result<(), ConsoleRunnerError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        // The closure runs on the main thread whenever that thread gets round to
        // it, which may be after this call has given up waiting. Reporting a
        // failure and then starting the emulator anyway is the one outcome worth
        // preventing: the user would be reading "could not start" while the game
        // came up behind it. So the wait and the work agree through this flag.
        let abandoned = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let queued = std::sync::Arc::clone(&abandoned);
        tauri::wry::prelude::dispatch(move |env, activity, _webview| {
            if queued.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            let mut outcome = Err(ConsoleRunnerError::EmulatorMissing(emulator));
            for intent in &candidates {
                outcome = match build_and_start(env, activity, intent) {
                    Ok(()) => Ok(()),
                    // A Java exception is still pending on this thread, so it must
                    // be taken before any further JNI call is made.
                    Err(_) => Err(classify_pending_exception(env, emulator)),
                };
                match outcome {
                    Err(ConsoleRunnerError::EmulatorMissing(_)) => continue,
                    _ => break,
                }
            }
            let _ = sender.send(outcome);
        });
        receiver.recv_timeout(HAND_OFF_TIMEOUT).unwrap_or_else(|_| {
            abandoned.store(true, std::sync::atomic::Ordering::Release);
            Err(ConsoleRunnerError::LaunchFailed(emulator))
        })
    }

    fn build_and_start(
        env: &mut JNIEnv<'_>,
        activity: &JObject<'_>,
        intent: &ConsoleIntent,
    ) -> jni::errors::Result<()> {
        let intent_class = env.find_class("android/content/Intent")?;
        let target = env.new_object(&intent_class, "()V", &[])?;

        // An explicit component is what makes this safe *and* what makes a
        // missing emulator detectable: intent filters are bypassed entirely, so
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
        if let Some(action) = intent.action() {
            let action = env.new_string(action)?;
            env.call_method(
                &target,
                "setAction",
                "(Ljava/lang/String;)Landroid/content/Intent;",
                &[(&action).into()],
            )?;
        }
        if let Some(document) = intent.data() {
            // The platform's own builder, never a `content://` string Orivo
            // assembled: the shape of a tree document URI is Android's business.
            let tree = crate::winlator_saf::android::parse_uri(env, &document.tree_uri)?;
            let identifier = env.new_string(&document.document_id)?;
            let data = env
                .call_static_method(
                    "android/provider/DocumentsContract",
                    "buildDocumentUriUsingTree",
                    "(Landroid/net/Uri;Ljava/lang/String;)Landroid/net/Uri;",
                    &[(&tree).into(), (&identifier).into()],
                )?
                .l()?;
            env.call_method(
                &target,
                "setData",
                "(Landroid/net/Uri;)Landroid/content/Intent;",
                &[(&data).into()],
            )?;
        }
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

    fn classify_pending_exception(
        env: &mut JNIEnv<'_>,
        emulator: ConsoleEmulator,
    ) -> ConsoleRunnerError {
        let Ok(throwable) = env.exception_occurred() else {
            return ConsoleRunnerError::LaunchFailed(emulator);
        };
        // `exception_describe` puts the Java stack trace in logcat, which is the
        // only place a device-side launch failure can be diagnosed from.
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        if throwable.is_null() {
            return ConsoleRunnerError::LaunchFailed(emulator);
        }
        match java_class_name(env, &throwable).as_deref() {
            Some("android.content.ActivityNotFoundException") => {
                ConsoleRunnerError::EmulatorMissing(emulator)
            }
            Some("java.lang.SecurityException") => {
                ConsoleRunnerError::EmulatorRefusedLaunch(emulator)
            }
            _ => ConsoleRunnerError::LaunchFailed(emulator),
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

/// How a ROM is named on the confirmation: the file itself, and the folders
/// between the connected one and it.
///
/// The same reason Winlator's shortcuts need it, for a weaker but real version of
/// the same problem: a ROM's title *is* its file name, two files can claim the
/// same game, and one of them may have arrived without the user knowing.
pub fn rom_origin(root: &Path, rom: &Path) -> crate::source_review::FileOrigin {
    crate::source_review::file_origin(root, rom, MAX_ROM_TITLE_CHARS)
}

fn belongs_to_grant(rom: &Path, directories: &[PathBuf]) -> Result<bool, ConsoleRunnerError> {
    let mut readable_grant = false;
    for directory in directories {
        let root = match fs::canonicalize(directory) {
            Ok(root) if root.is_dir() => root,
            Ok(_) => continue,
            Err(_) => continue,
        };
        readable_grant = true;
        if rom != root && rom.starts_with(&root) {
            return Ok(true);
        }
    }
    if readable_grant {
        Ok(false)
    } else {
        Err(ConsoleRunnerError::AccessDenied)
    }
}

fn cancelled_or(cancelled: &AtomicBool) -> Result<(), ConsoleRunnerError> {
    if cancelled.load(Ordering::Acquire) {
        Err(ConsoleRunnerError::Cancelled)
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

fn rom_title_from_filename(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .and_then(|name| display_text(name))
        .unwrap_or_else(|| "Console game".into())
}

fn display_text(value: &str) -> Option<String> {
    crate::source_review::display_text(value, MAX_ROM_TITLE_CHARS)
}

fn safe_label(path: &Path, fallback: &str) -> String {
    crate::source_review::folder_label(path, fallback, MAX_ROM_TITLE_CHARS)
}

/// Hash the ROM's path independently from its contents. The reference is
/// path-stable so a re-imported ROM refreshes one library card, while the content
/// digest makes the host refuse a replaced file until a deliberate reimport has
/// updated its private inventory.
fn game_reference_for(rom_path: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orivo-console-rom-reference-v1\0");
    digest.update(rom_path.as_os_str().as_encoded_bytes());
    format!("rom:{:x}", digest.finalize())
}

/// A directory row a provider can be made to return, so the walk meeting one is a
/// test rather than a hope.
#[cfg(test)]
pub(crate) fn directory_row(
    document_id: &str,
    display_name: &str,
) -> crate::winlator_saf::TreeDocument {
    crate::winlator_saf::TreeDocument {
        document_id: document_id.into(),
        display_name: display_name.into(),
        mime_type: crate::winlator_saf::DIRECTORY_MIME_TYPE.into(),
        size: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console_saf::fake::FakeRomTree;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A recognisable head, long enough that truncating it changes the digest.
    const NES_HEADER: &[u8] = b"NES\x1a\x02\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";

    fn temporary_directory(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "orivo-console-runner-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::canonicalize(directory).unwrap()
    }

    fn write_rom(directory: &Path, file: &str, contents: &[u8]) -> PathBuf {
        let path = directory.join(file);
        fs::write(&path, contents).unwrap();
        fs::canonicalize(path).unwrap()
    }

    /// Every test below reads a real directory unless it needs a provider, which
    /// is what a desktop build and a device with a plainly readable folder both
    /// do. The storage access grant has its own tests, against a fake provider.
    fn filesystem(profile: &ConsoleEmulatorProfile) -> FilesystemRoms<'_> {
        FilesystemRoms::for_profile(profile)
    }

    fn profile(
        granted: &Path,
        emulator: ConsoleEmulator,
        system: ConsoleSystem,
    ) -> ConsoleEmulatorProfile {
        ConsoleEmulatorProfile {
            id: "console-test".into(),
            display_name: system.label().into(),
            emulator,
            system,
            rom_directories: vec![granted.to_path_buf()],
            rom_trees: Vec::new(),
            enabled: true,
            last_imported_at: None,
        }
    }

    fn nes_profile(granted: &Path) -> ConsoleEmulatorProfile {
        profile(granted, ConsoleEmulator::RetroArch, ConsoleSystem::Nes)
    }

    fn profile_for_snes(granted: &Path) -> ConsoleEmulatorProfile {
        ConsoleEmulatorProfile {
            id: "console-test-snes".into(),
            ..profile(granted, ConsoleEmulator::RetroArch, ConsoleSystem::Snes)
        }
    }

    fn inventory(
        profile: &ConsoleEmulatorProfile,
        candidate: &ScannedRom,
    ) -> ConsoleRomInventoryEntry {
        ConsoleRomInventoryEntry {
            profile_id: profile.id.clone(),
            game_ref: candidate.game_ref.clone(),
            title: candidate.title.clone(),
            rom_path: candidate.rom_path.clone(),
            fingerprint: candidate.fingerprint.clone(),
            imported_at: None,
        }
    }

    fn scan(profile: &ConsoleEmulatorProfile, source: &dyn RomSource) -> Vec<ScannedRom> {
        scan_roms(
            profile,
            source,
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap()
        .roms
    }

    /// A ROM folder is shared storage, so everything in it is a candidate and
    /// almost none of it is a game. The console decides what is.
    #[test]
    fn scans_only_the_files_this_console_plays() {
        let granted = temporary_directory("extensions");
        for file in [
            "Alter Ego.nes",
            "Lan Master.NES",
            "notes.txt",
            "Wipeout.iso",
        ] {
            write_rom(&granted, file, NES_HEADER);
        }
        let profile = nes_profile(&granted);
        let titles = scan(&profile, &filesystem(&profile))
            .into_iter()
            .map(|rom| rom.title)
            .collect::<Vec<_>>();
        assert_eq!(titles.len(), 2);
        assert!(titles.contains(&"Alter Ego".to_string()));
        assert!(titles.contains(&"Lan Master".to_string()));
    }

    /// The name is the file's, never a header's: a cartridge header holds a
    /// shouted twelve-character title at best, and the person who put the file
    /// there already named it.
    #[test]
    fn names_a_rom_after_its_file_and_not_after_its_header() {
        let granted = temporary_directory("title");
        write_rom(&granted, "Battle Kid.nes", b"NES\x1a SUPER MARIO BROS.");
        let profile = nes_profile(&granted);
        assert_eq!(scan(&profile, &filesystem(&profile))[0].title, "Battle Kid");
    }

    /// One connect covers every console the emulator runs, and the extension is
    /// what says which. That only works because no two of these consoles claim
    /// the same extension, so it is held to here rather than assumed.
    #[test]
    fn no_console_claims_another_consoles_extension() {
        let mut seen = std::collections::BTreeMap::new();
        for emulator in [ConsoleEmulator::RetroArch, ConsoleEmulator::Ppsspp] {
            for system in emulator.systems() {
                for extension in system.rom_extensions() {
                    assert!(
                        seen.insert(*extension, *system).is_none(),
                        "{extension} is claimed by two consoles"
                    );
                }
            }
        }
    }

    /// A folder holds whatever the user put there. Each file lands under the
    /// console it is for, so one connect does not become "sort your ROMs first".
    #[test]
    fn files_each_rom_under_the_console_its_extension_names() {
        let granted = temporary_directory("mixed-folder");
        write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        write_rom(&granted, "Uwol.smc", NES_HEADER);
        write_rom(&granted, "Tobu Tobu Girl.gb", NES_HEADER);
        let profile = nes_profile(&granted);
        let found = scan(&profile, &filesystem(&profile))
            .into_iter()
            .map(|rom| (rom.title, rom.system))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(found.get("Alter Ego"), Some(&ConsoleSystem::Nes));
        assert_eq!(found.get("Uwol"), Some(&ConsoleSystem::Snes));
        assert_eq!(found.get("Tobu Tobu Girl"), Some(&ConsoleSystem::GameBoy));
    }

    /// A card in the NES profile pointing at an SNES file would launch with the
    /// wrong core. The runner refuses it on its own, not only because the catalog
    /// would have.
    #[test]
    fn refuses_to_launch_a_rom_for_another_console_than_its_profile() {
        let granted = temporary_directory("wrong-console");
        write_rom(&granted, "Uwol.smc", NES_HEADER);
        let profile = nes_profile(&granted);
        let snes = profile_for_snes(&granted);
        let candidate = scan(&snes, &filesystem(&snes))
            .into_iter()
            .find(|rom| rom.system == ConsoleSystem::Snes)
            .unwrap();
        let entry = ConsoleRomInventoryEntry {
            profile_id: profile.id.clone(),
            ..inventory(&snes, &candidate)
        };
        let intent =
            ConsoleLaunchIntent::new(RETROARCH_RUNNER_ID, &profile.id, &candidate.game_ref)
                .unwrap();
        assert_eq!(
            prepare_console_launch(
                &profile,
                &filesystem(&profile),
                &entry,
                &intent,
                &NoInstalledPackages,
            ),
            Err(ConsoleRunnerError::RomNotLaunchable)
        );
    }

    /// A cartridge is hashed whole, so the answer about "is this the same file?"
    /// is exact. The two kinds of digest are never comparable, which is why the
    /// prefix says which one it is.
    #[test]
    fn hashes_a_cartridge_whole() {
        let granted = temporary_directory("cartridge");
        write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        let profile = nes_profile(&granted);
        let fingerprint = &scan(&profile, &filesystem(&profile))[0].fingerprint;
        assert!(
            fingerprint.starts_with("sha256:"),
            "a cartridge was not hashed whole: {fingerprint}"
        );
    }

    /// The same bytes at a different length are a different file. Without the
    /// length in the digest, a head-hashed image and that image with anything
    /// appended would be indistinguishable.
    #[test]
    fn the_length_is_part_of_the_fingerprint() {
        let head = RomBytes {
            length: 64,
            head: NES_HEADER.to_vec(),
        };
        let longer = RomBytes {
            length: 65,
            head: NES_HEADER.to_vec(),
        };
        assert_ne!(rom_fingerprint(&head), rom_fingerprint(&longer));
    }

    #[test]
    fn a_whole_file_digest_and_a_head_digest_are_told_apart() {
        let whole = RomBytes {
            length: 32,
            head: NES_HEADER.to_vec(),
        };
        let image = RomBytes {
            length: MAX_FULLY_HASHED_ROM_BYTES + 1,
            head: NES_HEADER.to_vec(),
        };
        assert!(rom_fingerprint(&whole).starts_with("sha256:"));
        assert!(rom_fingerprint(&image).starts_with("sha256-head:"));
    }

    #[test]
    fn refuses_a_rom_outside_the_profile_grant() {
        let granted = temporary_directory("scope");
        let elsewhere = temporary_directory("scope-elsewhere");
        let outside = write_rom(&elsewhere, "Alter Ego.nes", NES_HEADER);
        let profile = nes_profile(&granted);
        assert_eq!(
            filesystem(&profile).resolve(&outside),
            Err(ConsoleRunnerError::RomOutsideScope)
        );
    }

    #[test]
    fn a_symlink_into_the_grant_does_not_smuggle_a_rom_in() {
        let granted = temporary_directory("symlink");
        let elsewhere = temporary_directory("symlink-elsewhere");
        let outside = write_rom(&elsewhere, "Alter Ego.nes", NES_HEADER);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, granted.join("Alter Ego.nes")).unwrap();
        let profile = nes_profile(&granted);
        assert!(scan(&profile, &filesystem(&profile)).is_empty());
    }

    /// RetroArch reads `ROM` and `LIBRETRO` and nothing else matters here. The
    /// core is a library it will `dlopen`, so every part of that string is either
    /// a compile-time constant or a closed enum — one intent per published
    /// package, because Orivo may not ask the platform which one is installed.
    #[test]
    fn prepares_one_explicit_intent_per_published_retroarch_package() {
        let granted = temporary_directory("retroarch-intent");
        let rom = write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        let profile = nes_profile(&granted);
        let candidate = scan(&profile, &filesystem(&profile))[0].clone();
        let entry = inventory(&profile, &candidate);
        let intent =
            ConsoleLaunchIntent::new(RETROARCH_RUNNER_ID, &profile.id, &candidate.game_ref)
                .unwrap();

        let prepared = prepare_console_launch(
            &profile,
            &filesystem(&profile),
            &entry,
            &intent,
            &NoInstalledPackages,
        )
        .unwrap();
        let candidates = prepared.candidates();
        assert_eq!(
            candidates
                .iter()
                .map(ConsoleIntent::package)
                .collect::<Vec<_>>(),
            [
                "com.retroarch.aarch64",
                "com.retroarch",
                "com.retroarch.ra32"
            ]
        );
        for intent in candidates {
            assert_eq!(
                intent.activity(),
                "com.retroarch.browser.retroactivity.RetroActivityFuture"
            );
            assert_eq!(intent.action(), None);
            // FLAG_ACTIVITY_NEW_TASK, and nothing that would clear a session the
            // player is in the middle of.
            assert_eq!(intent.flags(), 0x1000_0000);
            assert_eq!(intent.data(), None);
            assert_eq!(
                intent.extras(),
                [
                    AndroidIntentExtra::Text {
                        key: "ROM",
                        value: rom.to_str().unwrap().to_string(),
                    },
                    AndroidIntentExtra::Text {
                        key: "LIBRETRO",
                        value: format!(
                            "/data/user/0/{}/cores/fceumm_libretro_android.so",
                            intent.package()
                        ),
                    },
                ]
            );
        }
    }

    /// PPSSPP declares `content` as a data scheme and reads `intent.getData()`,
    /// so it is handed the very document Orivo read, with a read grant on it —
    /// and no path, no folder, and nothing about where storage is mounted.
    #[test]
    fn hands_ppsspp_the_document_it_read_and_never_a_path() {
        let granted = through_a_storage_access_grant::granted_profile(
            ConsoleEmulator::Ppsspp,
            ConsoleSystem::PlayStationPortable,
        );
        let tree = through_a_storage_access_grant::folder(&[(
            "primary:Download/Roms/PSP/Wagic.pbp",
            NES_HEADER.to_vec(),
        )]);
        let source = through_a_storage_access_grant::source(&granted, tree);
        let candidate = scan(&granted, &source)[0].clone();
        let entry = inventory(&granted, &candidate);
        let intent =
            ConsoleLaunchIntent::new(PPSSPP_RUNNER_ID, &granted.id, &candidate.game_ref).unwrap();

        let prepared =
            prepare_console_launch(&granted, &source, &entry, &intent, &NoInstalledPackages)
                .unwrap();
        let candidates = prepared.candidates();
        assert_eq!(
            candidates
                .iter()
                .map(ConsoleIntent::package)
                .collect::<Vec<_>>(),
            ["org.ppsspp.ppsspp", "org.ppsspp.ppssppgold"]
        );
        for intent in candidates {
            assert_eq!(intent.activity(), "org.ppsspp.ppsspp.PpssppActivity");
            assert_eq!(intent.action(), Some("android.intent.action.VIEW"));
            // FLAG_ACTIVITY_NEW_TASK | FLAG_GRANT_READ_URI_PERMISSION.
            assert_eq!(intent.flags(), 0x1000_0001);
            assert!(intent.extras().is_empty());
            assert_eq!(
                intent.data(),
                Some(&RomDocument {
                    tree_uri: through_a_storage_access_grant::ROM_TREE.into(),
                    document_id: "primary:Download/Roms/PSP/Wagic.pbp".into(),
                })
            );
        }
    }

    /// A `file://` URI is not something one app may hand another — the platform
    /// throws rather than delivering it — so a folder Orivo can only read by
    /// pathname cannot feed PPSSPP, and it says so instead of building an intent
    /// that would crash on arrival.
    #[test]
    fn refuses_a_ppsspp_launch_from_a_folder_orivo_only_has_a_path_to() {
        let granted = temporary_directory("ppsspp-path-only");
        write_rom(&granted, "Wagic.pbp", NES_HEADER);
        let profile = profile(
            &granted,
            ConsoleEmulator::Ppsspp,
            ConsoleSystem::PlayStationPortable,
        );
        let candidate = scan(&profile, &filesystem(&profile))[0].clone();
        let entry = inventory(&profile, &candidate);
        let intent =
            ConsoleLaunchIntent::new(PPSSPP_RUNNER_ID, &profile.id, &candidate.game_ref).unwrap();
        assert_eq!(
            prepare_console_launch(
                &profile,
                &filesystem(&profile),
                &entry,
                &intent,
                &NoInstalledPackages,
            ),
            Err(ConsoleRunnerError::RomFolderNotConnected)
        );
    }

    /// RetroArch reads `pack.zip#Alter Ego.nes` as an entry inside `pack.zip`, so
    /// a path like that ends in `.nes`, passes every check about what kind of file
    /// it is, and makes the host hash the decoy while the core loads the archive.
    #[test]
    fn refuses_a_rom_path_that_names_a_file_inside_an_archive() {
        let granted = temporary_directory("archive-member");
        // The decoy is a real file; what makes the path dangerous is the `#`.
        write_rom(&granted, "pack.zip", NES_HEADER);
        let profile = nes_profile(&granted);
        let member = granted.join("pack.zip#Alter Ego.nes");
        assert_eq!(
            filesystem(&profile).resolve(&member),
            Err(ConsoleRunnerError::RomInsideArchive)
        );
    }

    /// A patch dropped beside a ROM is applied by RetroArch without being asked,
    /// so the file the host hashed stops deciding what runs. The launch says so
    /// rather than handing over a game it cannot describe.
    #[test]
    fn refuses_to_launch_a_rom_with_a_patch_file_beside_it() {
        let granted = temporary_directory("sidecar-patch");
        write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        let profile = nes_profile(&granted);
        let candidate = scan(&profile, &filesystem(&profile))[0].clone();
        let entry = inventory(&profile, &candidate);
        let intent =
            ConsoleLaunchIntent::new(RETROARCH_RUNNER_ID, &profile.id, &candidate.game_ref)
                .unwrap();
        // It launches while the folder holds only the ROM.
        prepare_console_launch(
            &profile,
            &filesystem(&profile),
            &entry,
            &intent,
            &NoInstalledPackages,
        )
        .unwrap();

        for patch in [
            "Alter Ego.ips",
            "Alter Ego.BPS",
            "Alter Ego.ups",
            "Alter Ego.xdelta",
        ] {
            let path = granted.join(patch);
            fs::write(&path, b"PATCH").unwrap();
            assert_eq!(
                prepare_console_launch(
                    &profile,
                    &filesystem(&profile),
                    &entry,
                    &intent,
                    &NoInstalledPackages,
                ),
                Err(ConsoleRunnerError::RomHasSidecarPatch),
                "a {patch} beside the ROM was ignored"
            );
            fs::remove_file(&path).unwrap();
        }
    }

    /// PPSSPP is handed one document and has read access to nothing else, so
    /// there is no file beside it to find — and refusing there would cost a user
    /// a game for a file the emulator can never open.
    #[test]
    fn a_patch_beside_a_psp_image_does_not_block_it() {
        let granted = through_a_storage_access_grant::granted_profile(
            ConsoleEmulator::Ppsspp,
            ConsoleSystem::PlayStationPortable,
        );
        let tree = through_a_storage_access_grant::folder(&[
            ("primary:Download/Roms/Wagic.pbp", NES_HEADER.to_vec()),
            ("primary:Download/Roms/Wagic.ips", b"PATCH".to_vec()),
        ]);
        let source = through_a_storage_access_grant::source(&granted, tree);
        let candidate = scan(&granted, &source)[0].clone();
        let entry = inventory(&granted, &candidate);
        let intent =
            ConsoleLaunchIntent::new(PPSSPP_RUNNER_ID, &granted.id, &candidate.game_ref).unwrap();
        assert!(
            prepare_console_launch(&granted, &source, &entry, &intent, &NoInstalledPackages)
                .is_ok()
        );
    }

    /// The core's *directory* is the platform's answer when there is one, and the
    /// primary-user default when there is not. Its file name is never either.
    #[test]
    fn asks_the_platform_where_the_cores_are_before_composing_a_path() {
        struct Elsewhere;
        impl InstalledPackages for Elsewhere {
            fn lookup(&self, package: &str) -> Option<InstalledPackage> {
                (package == "com.retroarch").then(|| InstalledPackage {
                    data_directory: Some(PathBuf::from("/data/user/11/com.retroarch")),
                    installer: Some("com.android.vending".into()),
                })
            }
        }

        let granted = temporary_directory("core-directory");
        write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        let profile = nes_profile(&granted);
        let candidate = scan(&profile, &filesystem(&profile))[0].clone();
        let entry = inventory(&profile, &candidate);
        let intent =
            ConsoleLaunchIntent::new(RETROARCH_RUNNER_ID, &profile.id, &candidate.game_ref)
                .unwrap();
        let prepared =
            prepare_console_launch(&profile, &filesystem(&profile), &entry, &intent, &Elsewhere)
                .unwrap();

        // The installed package is tried first, and its own data directory is
        // where its cores are — not the primary user's.
        let candidates = prepared.candidates();
        assert_eq!(candidates[0].package(), "com.retroarch");
        assert!(candidates[0].extras().contains(&AndroidIntentExtra::Text {
            key: "LIBRETRO",
            value: "/data/user/11/com.retroarch/cores/fceumm_libretro_android.so".into(),
        }));
        // The ones the platform said nothing about still get an intent, with the
        // default a primary-user install has.
        assert!(candidates[1].extras().contains(&AndroidIntentExtra::Text {
            key: "LIBRETRO",
            value: format!(
                "/data/user/0/{}/cores/fceumm_libretro_android.so",
                candidates[1].package()
            ),
        }));
    }

    #[test]
    fn refuses_a_rom_replaced_after_it_was_imported() {
        let granted = temporary_directory("replaced");
        write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        let profile = nes_profile(&granted);
        let candidate = scan(&profile, &filesystem(&profile))[0].clone();
        let entry = inventory(&profile, &candidate);
        let intent =
            ConsoleLaunchIntent::new(RETROARCH_RUNNER_ID, &profile.id, &candidate.game_ref)
                .unwrap();

        write_rom(
            &granted,
            "Alter Ego.nes",
            b"NES\x1a something else entirely",
        );
        assert_eq!(
            prepare_console_launch(
                &profile,
                &filesystem(&profile),
                &entry,
                &intent,
                &NoInstalledPackages,
            ),
            Err(ConsoleRunnerError::RomNotLaunchable)
        );
    }

    /// The same check has to hold between preparing the intent and sending it,
    /// because that window is the one an attacker controls.
    #[test]
    fn refuses_to_send_an_intent_for_a_rom_replaced_mid_launch() {
        let granted = temporary_directory("replaced-mid-launch");
        write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        let profile = nes_profile(&granted);
        let candidate = scan(&profile, &filesystem(&profile))[0].clone();
        let entry = inventory(&profile, &candidate);
        let intent =
            ConsoleLaunchIntent::new(RETROARCH_RUNNER_ID, &profile.id, &candidate.game_ref)
                .unwrap();
        let prepared = prepare_console_launch(
            &profile,
            &filesystem(&profile),
            &entry,
            &intent,
            &NoInstalledPackages,
        )
        .unwrap();

        write_rom(
            &granted,
            "Alter Ego.nes",
            b"NES\x1a something else entirely",
        );
        assert_eq!(
            prepared.launch(&filesystem(&profile)),
            Err(ConsoleRunnerError::RomNotLaunchable)
        );
    }

    /// The file the user vouched for is the file that gets imported. Every
    /// reference Orivo holds is derived from the path, so the content digest is
    /// the only thing that can tell two files at that path apart.
    #[test]
    fn refuses_to_import_a_rom_replaced_after_the_user_was_shown_it() {
        let granted = temporary_directory("replaced-before-import");
        write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        let profile = nes_profile(&granted);
        let previewed = scan(&profile, &filesystem(&profile))[0].clone();

        write_rom(&granted, "Alter Ego.nes", b"NES\x1a not that game");
        assert_eq!(
            revalidate_rom_import_candidate(
                &profile,
                &filesystem(&profile),
                &previewed,
                &AtomicBool::new(false),
            ),
            Err(ConsoleRunnerError::RomNotLaunchable)
        );
    }

    #[test]
    fn imports_the_rom_the_preview_actually_showed() {
        let granted = temporary_directory("unchanged-before-import");
        write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        let profile = nes_profile(&granted);
        let previewed = scan(&profile, &filesystem(&profile))[0].clone();
        let imported = revalidate_rom_import_candidate(
            &profile,
            &filesystem(&profile),
            &previewed,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(imported, previewed);
    }

    #[test]
    fn refuses_an_intent_that_names_another_profile_another_game_or_another_emulator() {
        let granted = temporary_directory("wrong-intent");
        write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        let profile = nes_profile(&granted);
        let candidate = scan(&profile, &filesystem(&profile))[0].clone();
        let entry = inventory(&profile, &candidate);

        for intent in [
            ConsoleLaunchIntent::new(RETROARCH_RUNNER_ID, "another-profile", &candidate.game_ref)
                .unwrap(),
            ConsoleLaunchIntent::new(RETROARCH_RUNNER_ID, &profile.id, "rom:deadbeef").unwrap(),
            // A PSP runner asking a RetroArch profile for a launch.
            ConsoleLaunchIntent::new(PPSSPP_RUNNER_ID, &profile.id, &candidate.game_ref).unwrap(),
        ] {
            assert_eq!(
                prepare_console_launch(
                    &profile,
                    &filesystem(&profile),
                    &entry,
                    &intent,
                    &NoInstalledPackages,
                ),
                Err(ConsoleRunnerError::InvalidIntent),
                "accepted {intent:?}"
            );
        }
    }

    #[test]
    fn refuses_a_reference_that_is_not_an_opaque_identifier() {
        for (profile_id, game_ref) in [
            ("../escape", "rom:aa"),
            ("profile", "rom:aa\u{0}"),
            ("", "rom:aa"),
            ("profile", "/absolute"),
        ] {
            assert_eq!(
                ConsoleLaunchIntent::new(RETROARCH_RUNNER_ID, profile_id, game_ref),
                Err(ConsoleRunnerError::InvalidIntent)
            );
        }
        assert_eq!(
            ConsoleLaunchIntent::new("com.someone.else", "profile", "rom:aa"),
            Err(ConsoleRunnerError::InvalidIntent)
        );
    }

    #[test]
    fn refuses_a_disabled_profile_before_touching_the_filesystem() {
        let granted = temporary_directory("disabled");
        write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        let mut profile = nes_profile(&granted);
        let candidate = scan(&profile, &filesystem(&profile))[0].clone();
        let entry = inventory(&profile, &candidate);
        let intent =
            ConsoleLaunchIntent::new(RETROARCH_RUNNER_ID, &profile.id, &candidate.game_ref)
                .unwrap();
        profile.enabled = false;

        assert_eq!(
            prepare_console_launch(
                &profile,
                &filesystem(&profile),
                &entry,
                &intent,
                &NoInstalledPackages,
            ),
            Err(ConsoleRunnerError::ProfileDisabled)
        );
        assert_eq!(
            scan_roms(
                &profile,
                &filesystem(&profile),
                &AtomicBool::new(false),
                ScanLimits::default(),
                |_| {},
            ),
            Err(ConsoleRunnerError::ProfileDisabled)
        );
    }

    #[test]
    fn a_cancelled_scan_stops_instead_of_walking_the_grant() {
        let granted = temporary_directory("cancelled");
        write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        let profile = nes_profile(&granted);
        assert_eq!(
            scan_roms(
                &profile,
                &filesystem(&profile),
                &AtomicBool::new(true),
                ScanLimits::default(),
                |_| {},
            ),
            Err(ConsoleRunnerError::Cancelled)
        );
    }

    #[test]
    fn a_scan_beyond_its_file_budget_is_refused_rather_than_truncated() {
        let granted = temporary_directory("budget");
        for index in 0..6 {
            write_rom(&granted, &format!("Game {index}.nes"), NES_HEADER);
        }
        let profile = nes_profile(&granted);
        assert_eq!(
            scan_roms(
                &profile,
                &filesystem(&profile),
                &AtomicBool::new(false),
                ScanLimits {
                    max_files: 3,
                    max_depth: 2
                },
                |_| {},
            ),
            Err(ConsoleRunnerError::TooManyFiles)
        );
    }

    #[test]
    fn a_launch_on_a_desktop_host_is_refused_rather_than_shelled_out() {
        let granted = temporary_directory("desktop-launch");
        write_rom(&granted, "Alter Ego.nes", NES_HEADER);
        let profile = nes_profile(&granted);
        let candidate = scan(&profile, &filesystem(&profile))[0].clone();
        let entry = inventory(&profile, &candidate);
        let intent =
            ConsoleLaunchIntent::new(RETROARCH_RUNNER_ID, &profile.id, &candidate.game_ref)
                .unwrap();
        let prepared = prepare_console_launch(
            &profile,
            &filesystem(&profile),
            &entry,
            &intent,
            &NoInstalledPackages,
        )
        .unwrap();

        // Everything up to the hand-off works everywhere; only the hand-off is
        // Android's, and off it that is a sentence rather than a process.
        assert_eq!(
            prepared.launch(&filesystem(&profile)),
            Err(ConsoleRunnerError::PlatformUnsupported)
        );
    }

    /// Every refusal the user can reach says something different, because a
    /// shared sentence is a refusal nobody can act on.
    #[test]
    fn every_refusal_has_its_own_sentence() {
        let refusals = [
            ConsoleRunnerError::Cancelled,
            ConsoleRunnerError::PlatformUnsupported,
            ConsoleRunnerError::ProfileDisabled,
            ConsoleRunnerError::InvalidProfile,
            ConsoleRunnerError::RomMissing,
            ConsoleRunnerError::RomOutsideScope,
            ConsoleRunnerError::RomNotLaunchable,
            ConsoleRunnerError::RomUnreadable,
            ConsoleRunnerError::AccessDenied,
            ConsoleRunnerError::TooManyFiles,
            ConsoleRunnerError::InvalidIntent,
            ConsoleRunnerError::InvalidPage,
            ConsoleRunnerError::EmulatorMissing(ConsoleEmulator::RetroArch),
            ConsoleRunnerError::EmulatorMissing(ConsoleEmulator::Ppsspp),
            ConsoleRunnerError::EmulatorRefusedLaunch(ConsoleEmulator::RetroArch),
            ConsoleRunnerError::LaunchFailed(ConsoleEmulator::Ppsspp),
            ConsoleRunnerError::RomFolderUnsupported,
            ConsoleRunnerError::RomFolderTooBroad,
            ConsoleRunnerError::RomFolderNotConnected,
            ConsoleRunnerError::RomFolderAccessLost,
        ];
        let sentences = refusals
            .iter()
            .map(ToString::to_string)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(sentences.len(), refusals.len());
        for sentence in &sentences {
            assert!(sentence.ends_with('.'), "not a sentence: {sentence}");
        }
        // The sentence names the emulator, so "not installed" is actionable.
        assert!(
            ConsoleRunnerError::EmulatorMissing(ConsoleEmulator::RetroArch)
                .to_string()
                .starts_with("RetroArch is not installed")
        );
    }

    /// The rules about which provider, which volume and which document may become
    /// a path are Winlator's, and reused. Their sentences are not: a player who
    /// pointed Orivo at a ROM folder is not told about an export folder.
    #[test]
    fn a_grant_refusal_keeps_its_own_words() {
        assert_eq!(
            ConsoleRunnerError::from(WinlatorRunnerError::ExportFolderTooBroad),
            ConsoleRunnerError::RomFolderTooBroad
        );
        let sentence = ConsoleRunnerError::RomFolderTooBroad.to_string();
        assert!(!sentence.contains("Winlator"), "{sentence}");
    }

    mod through_a_storage_access_grant {
        use super::*;
        use crate::console_saf::RomDocumentTree;

        pub(super) const ROM_TREE: &str =
            "content://com.android.externalstorage.documents/tree/primary%3ADownload%2FRoms";
        pub(super) const ROM_DOCUMENT: &str = "primary:Download/Roms";
        const EXTERNAL_STORAGE: &str = "/storage/emulated/0";

        pub(super) fn granted_profile(
            emulator: ConsoleEmulator,
            system: ConsoleSystem,
        ) -> ConsoleEmulatorProfile {
            ConsoleEmulatorProfile {
                id: "console-granted".into(),
                display_name: system.label().into(),
                emulator,
                system,
                rom_directories: vec![PathBuf::from("/storage/emulated/0/Download/Roms")],
                rom_trees: vec![ROM_TREE.to_string()],
                enabled: true,
                last_imported_at: None,
            }
        }

        pub(super) fn folder(documents: &[(&str, Vec<u8>)]) -> FakeRomTree {
            let mut tree = FakeRomTree::new(ROM_DOCUMENT);
            for (document_id, contents) in documents {
                // Any folder between the grant and the file has to exist as a row,
                // the way a provider would report it.
                let mut prefix = ROM_DOCUMENT.to_string();
                let relative = document_id.strip_prefix("primary:Download/Roms/").unwrap();
                let mut parts = relative.split('/').peekable();
                while let Some(part) = parts.next() {
                    if parts.peek().is_none() {
                        break;
                    }
                    prefix = format!("{prefix}/{part}");
                    tree = tree.with_directory(&prefix);
                }
                tree = tree.with_document(document_id, contents);
            }
            tree
        }

        pub(super) fn source(
            profile: &ConsoleEmulatorProfile,
            tree: FakeRomTree,
        ) -> DocumentTreeRoms {
            document_tree_source_with(
                profile,
                Path::new(EXTERNAL_STORAGE),
                &[ROM_TREE.to_string()],
                move |_| Box::new(tree.clone()) as Box<dyn RomDocumentTree>,
            )
            .unwrap()
        }

        fn psp_profile() -> ConsoleEmulatorProfile {
            granted_profile(ConsoleEmulator::Ppsspp, ConsoleSystem::PlayStationPortable)
        }

        #[test]
        fn reads_a_rom_it_can_only_reach_through_the_grant() {
            let profile = psp_profile();
            let tree = folder(&[("primary:Download/Roms/Wagic.pbp", NES_HEADER.to_vec())]);
            let roms = scan(&profile, &source(&profile, tree));
            assert_eq!(roms.len(), 1);
            assert_eq!(roms[0].title, "Wagic");
            assert_eq!(
                roms[0].rom_path,
                Path::new("/storage/emulated/0/Download/Roms/Wagic.pbp")
            );
        }

        /// A provider is free to answer a listing with any row it likes, so a row
        /// that does not resolve to a child of the folder being listed is dropped
        /// rather than followed.
        #[test]
        fn drops_a_row_the_provider_claims_as_a_child_of_the_wrong_folder() {
            let profile = psp_profile();
            let tree = folder(&[("primary:Download/Roms/Wagic.pbp", NES_HEADER.to_vec())])
                .with_document("primary:Download/Elsewhere/Trojan.pbp", NES_HEADER)
                .with_stray_row(
                    ROM_DOCUMENT,
                    crate::winlator_saf::TreeDocument {
                        document_id: "primary:Download/Elsewhere/Trojan.pbp".into(),
                        display_name: "Trojan.pbp".into(),
                        mime_type: "application/octet-stream".into(),
                        size: Some(16),
                    },
                );
            let titles = scan(&profile, &source(&profile, tree))
                .into_iter()
                .map(|rom| rom.title)
                .collect::<Vec<_>>();
            assert_eq!(titles, ["Wagic"]);
        }

        /// An identifier carrying `..` looks perfectly inside the tree to the
        /// provider and would leave the folder on the filesystem.
        #[test]
        fn drops_a_row_whose_identifier_walks_out_of_the_folder() {
            let profile = psp_profile();
            let tree = folder(&[("primary:Download/Roms/Wagic.pbp", NES_HEADER.to_vec())])
                .with_stray_row(
                    ROM_DOCUMENT,
                    crate::winlator_saf::TreeDocument {
                        document_id: "primary:Download/Roms/../Elsewhere/Trojan.pbp".into(),
                        display_name: "Trojan.pbp".into(),
                        mime_type: "application/octet-stream".into(),
                        size: Some(16),
                    },
                );
            let titles = scan(&profile, &source(&profile, tree))
                .into_iter()
                .map(|rom| rom.title)
                .collect::<Vec<_>>();
            assert_eq!(titles, ["Wagic"]);
        }

        #[test]
        fn walks_a_subfolder_the_provider_reports() {
            let profile = psp_profile();
            let tree = folder(&[("primary:Download/Roms/PSP/Wagic.pbp", NES_HEADER.to_vec())]);
            let roms = scan(&profile, &source(&profile, tree));
            assert_eq!(roms.len(), 1);
            assert_eq!(
                roms[0].rom_path,
                Path::new("/storage/emulated/0/Download/Roms/PSP/Wagic.pbp")
            );
        }

        /// A directory row that names the folder being listed would make the walk
        /// loop, and the scan's budget would be the only thing stopping it.
        #[test]
        fn does_not_follow_a_directory_row_that_names_its_own_parent() {
            let profile = psp_profile();
            let tree = folder(&[("primary:Download/Roms/Wagic.pbp", NES_HEADER.to_vec())])
                .with_stray_row(ROM_DOCUMENT, directory_row(ROM_DOCUMENT, "Roms"));
            let roms = scan_roms(
                &profile,
                &source(&profile, tree),
                &AtomicBool::new(false),
                ScanLimits::default(),
                |_| {},
            )
            .unwrap();
            assert_eq!(roms.roms.len(), 1);
        }

        #[test]
        fn skips_a_document_the_provider_refuses_and_keeps_the_others() {
            let profile = psp_profile();
            let tree = folder(&[
                ("primary:Download/Roms/Wagic.pbp", NES_HEADER.to_vec()),
                ("primary:Download/Roms/Broken.pbp", NES_HEADER.to_vec()),
            ])
            .with_unreadable("primary:Download/Roms/Broken.pbp");
            let titles = scan(&profile, &source(&profile, tree))
                .into_iter()
                .map(|rom| rom.title)
                .collect::<Vec<_>>();
            assert_eq!(titles, ["Wagic"]);
        }

        /// Half of what identifies a large image is its length, so a provider that
        /// will not say is a refusal rather than a digest of a prefix.
        #[test]
        fn refuses_a_document_whose_length_the_provider_will_not_say() {
            let profile = psp_profile();
            let tree = folder(&[("primary:Download/Roms/Wagic.pbp", NES_HEADER.to_vec())])
                .with_no_size_for("primary:Download/Roms/Wagic.pbp");
            assert!(scan(&profile, &source(&profile, tree)).is_empty());
        }

        /// A disc image is read once, for its head, and never in full: a launch
        /// that pulled 1.5 GB through a `ContentResolver` would not be a launch.
        #[test]
        fn reads_a_disc_image_once_and_hashes_its_head() {
            let profile = psp_profile();
            let tree = folder(&[("primary:Download/Roms/Wipeout.iso", NES_HEADER.to_vec())])
                .with_claimed_length(
                    "primary:Download/Roms/Wipeout.iso",
                    MAX_FULLY_HASHED_ROM_BYTES * 40,
                );
            let roms = scan(&profile, &source(&profile, tree.clone()));
            assert_eq!(roms.len(), 1);
            assert!(roms[0].fingerprint.starts_with("sha256-head:"));
            assert_eq!(tree.reads(), 1);
        }

        /// A file between the two bounds costs one extra read and is then hashed
        /// whole, because the first read is what reveals how large it is.
        #[test]
        fn reads_a_middling_file_twice_and_then_hashes_it_whole() {
            let profile = psp_profile();
            let bytes = vec![0xAB; (ROM_HEAD_DIGEST_BYTES as usize) + 32];
            let tree = folder(&[("primary:Download/Roms/Wagic.pbp", bytes)]);
            let roms = scan(&profile, &source(&profile, tree.clone()));
            assert_eq!(roms.len(), 1);
            assert!(roms[0].fingerprint.starts_with("sha256:"));
            assert_eq!(tree.reads(), 2);
        }

        #[test]
        fn refuses_a_rom_outside_the_granted_folder() {
            let profile = psp_profile();
            let tree = folder(&[("primary:Download/Roms/Wagic.pbp", NES_HEADER.to_vec())]);
            let source = source(&profile, tree);
            assert_eq!(
                source.resolve(Path::new(
                    "/storage/emulated/0/Download/Elsewhere/Wagic.pbp"
                )),
                Err(ConsoleRunnerError::RomOutsideScope)
            );
        }

        /// A permission the user revoked from the system settings simply stops
        /// being listed. Asking for it back is the only honest answer; reading the
        /// folder by pathname instead would be reaching around the grant.
        #[test]
        fn a_revoked_grant_asks_to_be_connected_again() {
            let profile = psp_profile();
            assert_eq!(
                document_tree_source_with(&profile, Path::new(EXTERNAL_STORAGE), &[], |_| {
                    Box::new(FakeRomTree::default())
                })
                .err(),
                Some(ConsoleRunnerError::RomFolderAccessLost)
            );
        }

        /// Any app can leave a file in `Download` with no permission at all, so a
        /// grant on one of those folders is refused on *every* use, not only when
        /// it was picked.
        #[test]
        fn refuses_to_read_a_grant_on_a_folder_anything_can_write_into() {
            for tree_uri in [
                "content://com.android.externalstorage.documents/tree/primary%3A",
                "content://com.android.externalstorage.documents/tree/primary%3ADownload",
                "content://com.android.externalstorage.documents/tree/primary%3AAndroid%2Fdata",
            ] {
                let profile = ConsoleEmulatorProfile {
                    rom_trees: vec![tree_uri.to_string()],
                    ..psp_profile()
                };
                assert_eq!(
                    document_tree_source_with(
                        &profile,
                        Path::new(EXTERNAL_STORAGE),
                        &[tree_uri.to_string()],
                        |_| Box::new(FakeRomTree::default()),
                    )
                    .err(),
                    Some(ConsoleRunnerError::RomFolderTooBroad),
                    "read a grant on {tree_uri}"
                );
            }
        }

        /// A folder Orivo cannot name as a path is refused with a sentence rather
        /// than stored and discovered to be useless at launch — including for
        /// PPSSPP, which would not need the path, because one rule the user can
        /// understand beats two they cannot predict.
        #[test]
        fn refuses_a_grant_on_a_provider_that_has_no_file_path() {
            for tree_uri in [
                "content://com.android.providers.downloads.documents/tree/msf%3A42",
                "content://com.android.externalstorage.documents/tree/1234-ABCD%3ARoms",
                "content://com.example.cloud/tree/opaque",
            ] {
                let profile = ConsoleEmulatorProfile {
                    rom_trees: vec![tree_uri.to_string()],
                    ..psp_profile()
                };
                assert_eq!(
                    document_tree_source_with(
                        &profile,
                        Path::new(EXTERNAL_STORAGE),
                        &[tree_uri.to_string()],
                        |_| Box::new(FakeRomTree::default()),
                    )
                    .err(),
                    Some(ConsoleRunnerError::RomFolderUnsupported),
                    "accepted {tree_uri}"
                );
            }
        }

        /// A desktop build that met a profile carrying a tree URI has to refuse it
        /// rather than quietly read the pathname behind it instead.
        #[test]
        fn a_desktop_host_refuses_a_grant_instead_of_reading_the_path_behind_it() {
            let profile = psp_profile();
            assert!(matches!(
                rom_source_for_profile(&profile).err(),
                Some(ConsoleRunnerError::RomFolderUnsupported)
            ));
        }
    }
}
