//! Trusted native host adapter for Orivo's bundled Wine-Staging runner.
//!
//! The WIT runner contract carries only opaque profile and game identifiers.
//! This module is the corresponding host-side implementation: it resolves
//! those identifiers through private catalog data, rechecks every filesystem
//! boundary, then builds a tokenised `Command` without involving a shell.

pub use crate::catalog::WINE_STAGING_RUNNER_ID;
use crate::catalog::{
    WineGameInventoryEntry, WineGraphicsBackend, WineGraphicsOptions, WinePrefixLayout,
    WineProfile, WineVirtualDesktop,
};
use flate2::read::GzDecoder;
use object::{Object, ObjectSymbol};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tar::Archive;

#[cfg(target_os = "macos")]
use core_graphics::display::CGDisplay;

#[cfg(unix)]
use std::{
    ffi::CString,
    os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd},
        unix::fs::OpenOptionsExt,
    },
};

pub const DEFAULT_MAX_SCAN_FILES: usize = 20_000;
pub const DEFAULT_MAX_SCAN_DEPTH: usize = 16;
pub const MAX_PAGE_SIZE: usize = 100;
const WINE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_DXMT_ENGINE_COMPONENT_BYTES: u64 = 256 * 1024 * 1024;
const WINE_PREFIX_INITIALIZATION_TIMEOUT: Duration = Duration::from_secs(45);
const WINE_PREFIX_CONFIGURATION_TIMEOUT: Duration = Duration::from_secs(10);
const FINGERPRINT_BUFFER_BYTES: usize = 64 * 1024;
const MAX_FINGERPRINT_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_DXVK_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DXVK_ENTRY_BYTES: usize = 64 * 1024 * 1024;
const MAX_DXVK_CONFIG_BYTES: usize = 1 * 1024 * 1024;
const DXVK_MACOS_ARCHIVE_SHA256: &str =
    "acd1520ad105d8ef124a09c8e11a259a5dc8bdc565ad18e0e52693f9807b2477";
pub const DXVK_MACOS_DOWNLOAD_URL: &str = "https://github.com/Gcenx/DXVK-macOS/releases/download/v1.10.3-20230507-repack/dxvk-macOS-async-v1.10.3-20230507-repack.tar.gz";
const DXVK_MACOS_OVERRIDE: &str = "d3d10core,d3d11=n,b";
const WINE_MAC_DRIVER_REGISTRY_KEY: &str = r"HKEY_CURRENT_USER\Software\Wine\Mac Driver";
const WINE_MAC_DRIVER_RETINA_MODE_VALUE: &str = "RetinaMode";
const DXVK_MACOS_FILES: [&str; 4] = [
    "x64/d3d10core.dll",
    "x64/d3d11.dll",
    "x32/d3d10core.dll",
    "x32/d3d11.dll",
];
static WINE_PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static DXVK_INSTALL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A closed capability result for DXMT's presentation ABI. `Unsupported` is
/// not an error on its own: automatic launch selection can use DXVK or Wine
/// 3D without exposing a binary path or low-level symbol to the WebView.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DxmtWineEngineSupport {
    Supported,
    Unsupported,
}

/// The host-only equivalent of WIT's `launch-intent`. Its closed enum means
/// the WIT `mode` string can never become a process argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineLaunchIntent {
    runner_id: String,
    profile_id: String,
    game_ref: String,
    mode: WineLaunchMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WineLaunchMode {
    WineStaging,
}

impl WineLaunchIntent {
    pub fn new(profile_id: &str, game_ref: &str) -> Result<Self, WineRunnerError> {
        if !valid_opaque_id(profile_id) || !valid_opaque_id(game_ref) {
            return Err(WineRunnerError::InvalidIntent);
        }
        Ok(Self {
            runner_id: WINE_STAGING_RUNNER_ID.into(),
            profile_id: profile_id.into(),
            game_ref: game_ref.into(),
            mode: WineLaunchMode::WineStaging,
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

    pub fn mode(&self) -> WineLaunchMode {
        self.mode
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WineRunnerError {
    Cancelled,
    WineMissing,
    WineAccessDenied,
    InvalidWine,
    WineNotStaging,
    ProfileDisabled,
    InvalidProfile,
    GameMissing,
    GameOutsideScope,
    GameNotLaunchable,
    AccessDenied,
    TooManyFiles,
    FingerprintTooLarge,
    InvalidIntent,
    InvalidPage,
    InvalidDxvkPackage,
    DxvkDownload,
    DxvkRuntimeUnavailable,
    DxmtRuntimeUnavailable,
    PrefixInitialization,
    DxvkInstallation,
    ProcessStart,
}

impl std::fmt::Display for WineRunnerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Cancelled => "Wine import was cancelled.",
            Self::WineMissing => {
                "Wine-Staging could not be found. Select its Wine binary and try again."
            }
            Self::WineAccessDenied => {
                "Orivo could not access the selected Wine-Staging binary. Check its permissions and try again."
            }
            Self::InvalidWine => "The selected file is not a usable Wine binary.",
            Self::WineNotStaging => "The selected Wine installation is not Wine-Staging.",
            Self::ProfileDisabled => "This Wine profile is disabled. Enable it and try again.",
            Self::InvalidProfile => {
                "This Wine profile is no longer valid. Review its setup and try again."
            }
            Self::GameMissing => "This Windows game is no longer available.",
            Self::GameOutsideScope => {
                "This game is outside the folders allowed for its Wine profile."
            }
            Self::GameNotLaunchable => {
                "This Windows game needs to be reimported before it can launch."
            }
            Self::AccessDenied => {
                "Orivo could not read one of the folders allowed for this Wine profile."
            }
            Self::TooManyFiles => {
                "This folder contains too many files to scan at once. Choose a narrower games folder."
            }
            Self::FingerprintTooLarge => {
                "This Windows executable is too large to validate safely. Choose a narrower games folder."
            }
            Self::InvalidIntent => "This Wine launch request is invalid.",
            Self::InvalidPage => "This Wine import page is no longer available.",
            Self::InvalidDxvkPackage => {
                "The selected DXVK-macOS package is incomplete or not the supported official release."
            }
            Self::DxvkDownload => {
                "Orivo could not download the verified DXVK-macOS compatibility runtime. Check your connection and retry."
            }
            Self::DxvkRuntimeUnavailable => {
                "DXVK-macOS needs to be installed again for this Wine profile before it can launch."
            }
            Self::DxmtRuntimeUnavailable => {
                "This Wine engine does not yet have Orivo's verified DXMT Metal runtime. Choose a compatible Wine engine or retry in compatibility mode."
            }
            Self::PrefixInitialization => {
                "Orivo could not configure this profile's private Wine environment."
            }
            Self::DxvkInstallation => {
                "Orivo could not install DXVK-macOS into this profile's private Wine prefix."
            }
            Self::ProcessStart => {
                "Wine could not start this game. Check the Wine-Staging installation and try again."
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for WineRunnerError {}

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
pub struct ScannedWineGame {
    pub game_ref: String,
    pub title: String,
    pub directory_label: String,
    pub executable_path: PathBuf,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineScanResult {
    pub scanned_files: usize,
    pub games: Vec<ScannedWineGame>,
}

/// Validate a binary selected by the native picker. `--version` is executed
/// with a fixed argument and no shell; output is never returned to the UI.
pub fn probe_wine_staging(
    selected: &Path,
    cancelled: &AtomicBool,
) -> Result<PathBuf, WineRunnerError> {
    cancelled_or(cancelled)?;
    let wine_binary = fs::canonicalize(selected).map_err(wine_probe_path_error)?;
    let metadata = wine_binary.metadata().map_err(wine_probe_path_error)?;
    if !metadata.is_file() || !is_executable(&wine_binary) {
        return Err(WineRunnerError::InvalidWine);
    }
    #[cfg(all(target_os = "macos", not(test)))]
    if !is_macos_mach_o_binary(&wine_binary) {
        return Err(WineRunnerError::InvalidWine);
    }

    // A probe must never fall back to `~/.wine` or a WINEPREFIX inherited
    // from another app. The temporary prefix is unique and removed when the
    // validation process returns (best effort if Wine left helper files).
    let probe_prefix = ProbePrefix::create()?;
    let mut probe = Command::new(&wine_binary);
    clear_untrusted_wine_environment(&mut probe);
    let mut child = probe
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("WINEPREFIX", probe_prefix.path())
        .spawn()
        .map_err(wine_probe_path_error)?;
    let deadline = Instant::now() + WINE_PROBE_TIMEOUT;
    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WineRunnerError::Cancelled);
        }
        match child.try_wait().map_err(|_| WineRunnerError::InvalidWine)? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WineRunnerError::InvalidWine);
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    };
    let output = child
        .wait_with_output()
        .map_err(|_| WineRunnerError::InvalidWine)?;
    cancelled_or(cancelled)?;
    if !status.success() || !output.status.success() {
        return Err(WineRunnerError::InvalidWine);
    }
    let version = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    if !is_wine_staging_version(&version) {
        return if version.contains("wine") {
            Err(WineRunnerError::WineNotStaging)
        } else {
            Err(WineRunnerError::InvalidWine)
        };
    }
    Ok(wine_binary)
}

/// Check whether a Wine-Staging engine exposes the macOS driver ABI required
/// by DXMT to attach a Metal view. A successful `dlopen` of DXMT is
/// deliberately insufficient: without these exports DXMT can load but cannot
/// present a game window.
///
/// It reads only the component derived from an already-validated Wine binary;
/// it does not run a shell, load a dylib, search `PATH`, or inspect a
/// user-provided component path. Callers reach it after their own
/// `probe_wine_staging`, which is what keeps a profile from paying for a
/// second `wine --version` process just to ask this question.
pub(crate) fn probe_dxmt_wine_engine_for_validated_binary(
    wine_binary: &Path,
    cancelled: &AtomicBool,
) -> Result<DxmtWineEngineSupport, WineRunnerError> {
    cancelled_or(cancelled)?;
    let Some(binary_directory) = wine_binary.parent() else {
        return Ok(DxmtWineEngineSupport::Unsupported);
    };
    let Some(runtime_root) = binary_directory.parent() else {
        return Ok(DxmtWineEngineSupport::Unsupported);
    };
    let runtime_root = fs::canonicalize(runtime_root).map_err(wine_probe_path_error)?;
    let expected_binary = runtime_root.join("bin").join(
        wine_binary
            .file_name()
            .ok_or(WineRunnerError::InvalidWine)?,
    );
    if expected_binary != wine_binary {
        return Ok(DxmtWineEngineSupport::Unsupported);
    }
    let component = runtime_root.join("lib/wine/x86_64-unix/winemac.so");
    let metadata = match fs::symlink_metadata(&component) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => metadata,
        Ok(_) | Err(_) => return Ok(DxmtWineEngineSupport::Unsupported),
    };
    if metadata.len() == 0 || metadata.len() > MAX_DXMT_ENGINE_COMPONENT_BYTES {
        return Ok(DxmtWineEngineSupport::Unsupported);
    }
    let component = match fs::canonicalize(component) {
        Ok(component) if component.starts_with(&runtime_root) => component,
        Ok(_) | Err(_) => return Ok(DxmtWineEngineSupport::Unsupported),
    };
    cancelled_or(cancelled)?;
    let bytes = match fs::read(component) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(DxmtWineEngineSupport::Unsupported),
    };
    cancelled_or(cancelled)?;
    let object = match object::File::parse(&*bytes) {
        Ok(object) => object,
        Err(_) => return Ok(DxmtWineEngineSupport::Unsupported),
    };
    let symbols = object
        .dynamic_symbols()
        .filter_map(|symbol| symbol.name().ok())
        .map(normalise_mach_o_export_name);
    Ok(if dxmt_support_from_exported_symbols(symbols) {
        DxmtWineEngineSupport::Supported
    } else {
        DxmtWineEngineSupport::Unsupported
    })
}

fn normalise_mach_o_export_name(name: &str) -> &str {
    name.strip_prefix('_').unwrap_or(name)
}

fn dxmt_support_from_exported_symbols<'a>(symbols: impl IntoIterator<Item = &'a str>) -> bool {
    let exports = symbols.into_iter().collect::<BTreeSet<_>>();
    if exports.contains("macdrv_functions") {
        return true;
    }
    [
        "get_win_data",
        "release_win_data",
        "macdrv_view_create_metal_view",
        "macdrv_view_get_metal_layer",
        "macdrv_view_release_metal_view",
    ]
    .into_iter()
    .all(|symbol| exports.contains(symbol))
}

/// Probe a deliberately small allowlist of conventional macOS Wine-Staging
/// locations without executing any candidate. This is convenience discovery
/// only: no PATH lookup, shell, recursive application scan, or automatic
/// binary execution can turn the environment into an implicit grant. The user
/// must explicitly confirm a detected candidate before `probe_wine_staging`
/// starts it with the fixed `--version` argument.
pub fn detect_wine_staging(cancelled: &AtomicBool) -> Result<Option<PathBuf>, WineRunnerError> {
    for candidate in default_wine_staging_locations() {
        cancelled_or(cancelled)?;
        let Ok(candidate) = fs::canonicalize(candidate) else {
            continue;
        };
        let Ok(metadata) = candidate.metadata() else {
            continue;
        };
        if !metadata.is_file() || !is_executable(&candidate) {
            continue;
        }
        #[cfg(all(target_os = "macos", not(test)))]
        if !is_macos_mach_o_binary(&candidate) {
            continue;
        }
        return Ok(Some(candidate));
    }
    Ok(None)
}

fn default_wine_staging_locations() -> Vec<PathBuf> {
    [
        "/Applications/Wine Staging.app/Contents/Resources/wine/bin/wine",
        "/Applications/Wine Staging.app/Contents/Resources/wine/bin/wine64",
        "/Applications/Wine-Staging.app/Contents/Resources/wine/bin/wine",
        "/Applications/Wine-Staging.app/Contents/Resources/wine/bin/wine64",
        "/opt/homebrew/bin/wine-staging",
        "/usr/local/bin/wine-staging",
        "/opt/homebrew/bin/wine",
        "/usr/local/bin/wine",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

/// The scan intentionally knows only profile-scoped folders. Directory
/// symlinks are never followed, and every candidate is canonicalised again
/// before it receives an opaque reference.
pub fn scan_wine_games(
    profile: &WineProfile,
    cancelled: &AtomicBool,
    limits: ScanLimits,
    mut progress: impl FnMut(usize),
) -> Result<WineScanResult, WineRunnerError> {
    if !profile.enabled {
        return Err(WineRunnerError::ProfileDisabled);
    }
    profile
        .validate()
        .map_err(|_| WineRunnerError::InvalidProfile)?;
    if limits.max_files == 0 || limits.max_depth == 0 {
        return Err(WineRunnerError::InvalidPage);
    }

    let mut candidates = BTreeMap::new();
    let mut scanned_files = 0;
    for directory in &profile.game_directories {
        cancelled_or(cancelled)?;
        let root = fs::canonicalize(directory).map_err(|_| WineRunnerError::AccessDenied)?;
        if !root.is_dir() {
            return Err(WineRunnerError::AccessDenied);
        }
        let label = safe_label(&root, "Authorized game folder");
        scan_directory(
            &root,
            &root,
            &label,
            0,
            limits,
            cancelled,
            &mut scanned_files,
            &mut candidates,
            &mut progress,
        )?;
    }

    Ok(WineScanResult {
        scanned_files,
        games: candidates.into_values().collect(),
    })
}

/// Return a stable bounded page from an already-completed scan snapshot.
pub fn page_wine_inventory(
    games: &[ScannedWineGame],
    offset: usize,
    limit: usize,
) -> Result<(Vec<ScannedWineGame>, Option<usize>), WineRunnerError> {
    if limit == 0 || limit > MAX_PAGE_SIZE || offset > games.len() {
        return Err(WineRunnerError::InvalidPage);
    }
    let end = offset.saturating_add(limit).min(games.len());
    let next = (end < games.len()).then_some(end);
    Ok((games[offset..end].to_vec(), next))
}

/// Resolve a typed intent into a private process specification. The profile
/// prefix is accepted only when it is precisely the generated child of
/// Orivo's managed prefix root; this prevents any other application's prefix
/// from being touched.
pub fn prepare_wine_launch(
    profile: &WineProfile,
    game: &WineGameInventoryEntry,
    intent: &WineLaunchIntent,
    prefix_root: &Path,
) -> Result<PreparedWineLaunch, WineRunnerError> {
    if intent.runner_id() != WINE_STAGING_RUNNER_ID
        || intent.profile_id() != profile.id
        || intent.game_ref() != game.game_ref
        || intent.mode() != WineLaunchMode::WineStaging
    {
        return Err(WineRunnerError::InvalidIntent);
    }
    if !profile.enabled {
        return Err(WineRunnerError::ProfileDisabled);
    }
    profile
        .validate()
        .map_err(|_| WineRunnerError::InvalidProfile)?;
    if game.profile_id != profile.id {
        return Err(WineRunnerError::InvalidIntent);
    }
    game.validate()
        .map_err(|_| WineRunnerError::GameNotLaunchable)?;

    let wine_binary = probe_wine_staging(&profile.wine_binary, &AtomicBool::new(false))?;
    let current = revalidate_wine_import_candidate(
        profile,
        &ScannedWineGame {
            game_ref: game.game_ref.clone(),
            title: game.title.clone(),
            directory_label: String::new(),
            executable_path: game.executable_path.clone(),
            fingerprint: game.fingerprint.clone(),
        },
        &AtomicBool::new(false),
    )
    .map_err(|error| match error {
        WineRunnerError::GameMissing | WineRunnerError::GameOutsideScope => error,
        _ => WineRunnerError::GameNotLaunchable,
    })?;
    if game.fingerprint != current.fingerprint || game.game_ref != current.game_ref {
        return Err(WineRunnerError::GameNotLaunchable);
    }
    let prefix = match game.compatibility.prefix_layout {
        WinePrefixLayout::LegacySharedProfile => {
            ensure_managed_prefix(&profile.prefix, prefix_root, &profile.id)?
        }
        WinePrefixLayout::Isolated => {
            ensure_managed_game_prefix(prefix_root, &profile.id, &game.game_ref)?
        }
    };
    let graphics = resolve_wine_game_graphics(profile, game, &wine_binary, prefix_root, &prefix)?;
    let macos_retina_mode_enabled =
        synchronize_macos_retina_mode(&wine_binary, &prefix, profile.macos_retina_mode_enabled)?;

    let working_directory = current
        .executable_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or(WineRunnerError::GameNotLaunchable)?;

    Ok(PreparedWineLaunch {
        wine_binary,
        prefix,
        executable: current.executable_path,
        working_directory,
        executable_fingerprint: game.fingerprint.clone(),
        graphics,
        macos_retina_mode_enabled,
    })
}

/// Resolve only the finite set of host-owned graphics paths. Automatic mode
/// never interprets a plugin/WebView argument and never relaunches a game on
/// an uncertain early exit; an explicit retry updates the inventory before a
/// subsequent typed launch intent reaches this function.
fn resolve_wine_game_graphics(
    profile: &WineProfile,
    game: &WineGameInventoryEntry,
    wine_binary: &Path,
    prefix_root: &Path,
    prefix: &Path,
) -> Result<WineGraphicsOptions, WineRunnerError> {
    let requested = &game.compatibility.graphics;
    let backend = match requested.backend {
        WineGraphicsBackend::Auto => {
            match game
                .compatibility
                .last_backend
                .filter(|backend| !game.compatibility.rejected_backends.contains(backend))
            {
                // Wine 3D is an automatic fallback, not a permanent choice:
                // a verified DXVK seed may be prepared after an earlier safe
                // fallback. Preserve Wine 3D only when DXVK was explicitly
                // rejected for this game through the closed Retry action.
                Some(WineGraphicsBackend::WineD3d)
                    if !game
                        .compatibility
                        .rejected_backends
                        .contains(&WineGraphicsBackend::DxvkMacos) =>
                {
                    resolve_automatic_graphics_backend(
                        profile,
                        game,
                        wine_binary,
                        prefix_root,
                        prefix,
                    )?
                }
                Some(WineGraphicsBackend::WineD3d) => WineGraphicsBackend::WineD3d,
                Some(WineGraphicsBackend::DxvkMacos) => {
                    ensure_dxvk_macos_game_runtime(
                        profile,
                        game,
                        wine_binary,
                        prefix_root,
                        prefix,
                    )?;
                    WineGraphicsBackend::DxvkMacos
                }
                Some(WineGraphicsBackend::Dxmt) => {
                    return Err(WineRunnerError::DxmtRuntimeUnavailable);
                }
                Some(WineGraphicsBackend::Auto) => return Err(WineRunnerError::InvalidProfile),
                None => resolve_automatic_graphics_backend(
                    profile,
                    game,
                    wine_binary,
                    prefix_root,
                    prefix,
                )?,
            }
        }
        WineGraphicsBackend::Dxmt => return Err(WineRunnerError::DxmtRuntimeUnavailable),
        WineGraphicsBackend::DxvkMacos => {
            ensure_dxvk_macos_game_runtime(profile, game, wine_binary, prefix_root, prefix)?;
            WineGraphicsBackend::DxvkMacos
        }
        WineGraphicsBackend::WineD3d => WineGraphicsBackend::WineD3d,
    };
    Ok(WineGraphicsOptions {
        backend,
        virtual_desktop: requested.virtual_desktop.clone(),
    })
}

/// Auto intentionally treats a missing optional runtime as an eligibility
/// failure, not as a launch failure. The safe Wine 3D path is always last so
/// a game remains launchable while Orivo waits for a verified DXMT engine.
fn resolve_automatic_graphics_backend(
    profile: &WineProfile,
    game: &WineGameInventoryEntry,
    wine_binary: &Path,
    prefix_root: &Path,
    prefix: &Path,
) -> Result<WineGraphicsBackend, WineRunnerError> {
    // DXMT has no safe fallback from arbitrary Wine bundles: its builtin
    // components must be installed into an Orivo-owned, engine-verified
    // runtime. Until that runtime exists, skip it rather than modifying the
    // selected third-party Wine installation.
    if !game
        .compatibility
        .rejected_backends
        .contains(&WineGraphicsBackend::DxvkMacos)
    {
        match ensure_dxvk_macos_game_runtime(profile, game, wine_binary, prefix_root, prefix) {
            Ok(()) => return Ok(WineGraphicsBackend::DxvkMacos),
            // A missing profile seed simply means the optional DXVK backend
            // has not been installed yet. It is safe to continue to Wine 3D.
            Err(WineRunnerError::DxvkRuntimeUnavailable) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(WineGraphicsBackend::WineD3d)
}

#[derive(Debug)]
pub struct PreparedWineLaunch {
    wine_binary: PathBuf,
    prefix: PathBuf,
    executable: PathBuf,
    working_directory: PathBuf,
    executable_fingerprint: String,
    graphics: WineGraphicsOptions,
    macos_retina_mode_enabled: Option<bool>,
}

impl PreparedWineLaunch {
    pub fn graphics_backend(&self) -> WineGraphicsBackend {
        self.graphics.backend
    }

    /// The resolved high-density display policy is host-probed and written
    /// only to Orivo's private Wine prefix. It is returned to the catalog so
    /// subsequent launches do not mutate a prefix unless the active display
    /// configuration has changed.
    pub fn macos_retina_mode_enabled(&self) -> Option<bool> {
        self.macos_retina_mode_enabled
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.wine_binary);
        clear_untrusted_wine_environment(&mut command);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env("WINEPREFIX", &self.prefix);
        if self.graphics.backend == WineGraphicsBackend::DxvkMacos {
            // This is a fixed host policy, not a WebView/plugin-provided
            // environment value. Only the two DLLs copied into the private
            // prefix are selected; Wine's DXGI and all other components stay
            // on their bundled implementation for macOS compatibility.
            command.env("WINEDLLOVERRIDES", DXVK_MACOS_OVERRIDE);
        }
        // Wine must receive the authorised canonical pathname, not an open
        // descriptor such as `/dev/fd/9`: Unity and similar engines derive
        // their sibling `*_Data` directory from the module pathname rather
        // than the current working directory.
        append_fixed_graphics_arguments(&mut command, &self.graphics, &self.executable);
        command.current_dir(&self.working_directory);
        command
    }

    pub fn spawn(&self) -> Result<Child, WineRunnerError> {
        // This is deliberately immediately before `Command::spawn`. The
        // earlier profile/scope validation resolves only host-owned paths;
        // this last no-follow, content-addressed check rejects a game changed
        // while the launch was being prepared without changing its pathname.
        validate_wine_launch_executable(
            &self.executable,
            &self.executable_fingerprint,
            &AtomicBool::new(false),
        )?;
        self.command()
            .spawn()
            .map_err(|_| WineRunnerError::ProcessStart)
    }

    #[cfg(test)]
    fn executable(&self) -> &Path {
        &self.executable
    }
}

fn append_fixed_graphics_arguments(
    command: &mut Command,
    graphics: &WineGraphicsOptions,
    executable: &Path,
) {
    match graphics.virtual_desktop.as_ref() {
        Some(WineVirtualDesktop { width, height }) => {
            command
                .arg("explorer")
                .arg(format!("/desktop=Orivo,{width}x{height}"))
                .arg(executable);
        }
        None => {
            command.arg(executable);
        }
    }
}

fn validate_wine_launch_executable(
    executable: &Path,
    expected_fingerprint: &str,
    cancelled: &AtomicBool,
) -> Result<(), WineRunnerError> {
    // `prepare_wine_launch` receives a canonical path. Recheck that this is
    // still the same path before opening it so a directory swap cannot route
    // the final check outside the granted game tree.
    let canonical = fs::canonicalize(executable).map_err(wine_launch_file_error)?;
    if canonical != executable || !is_windows_executable(&canonical) {
        return Err(WineRunnerError::GameNotLaunchable);
    }

    #[cfg(unix)]
    {
        // Hash the opened descriptor rather than reopening the pathname while
        // reading it. `O_NOFOLLOW` also rejects a leaf symlink introduced
        // after the canonical scope check.
        let mut executable_handle = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(executable)
            .map_err(wine_launch_file_error)?;
        let metadata = executable_handle
            .metadata()
            .map_err(|_| WineRunnerError::GameNotLaunchable)?;
        if !metadata.is_file() {
            return Err(WineRunnerError::GameNotLaunchable);
        }
        let fingerprint = content_fingerprint_for_open_file(&mut executable_handle, cancelled)?;
        if fingerprint != expected_fingerprint {
            return Err(WineRunnerError::GameNotLaunchable);
        }
        return Ok(());
    }

    #[cfg(not(unix))]
    {
        let fingerprint = content_fingerprint_for(executable, cancelled)?;
        if fingerprint != expected_fingerprint {
            return Err(WineRunnerError::GameNotLaunchable);
        }
        Ok(())
    }
}

#[cfg(unix)]
fn open_directory_without_following_links(directory: &Path) -> Result<fs::File, WineRunnerError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(directory)
        .map_err(wine_launch_file_error)?;
    if !file
        .metadata()
        .map_err(|_| WineRunnerError::GameNotLaunchable)?
        .is_dir()
    {
        return Err(WineRunnerError::GameNotLaunchable);
    }
    Ok(file)
}

fn wine_launch_file_error(error: io::Error) -> WineRunnerError {
    match error.kind() {
        io::ErrorKind::NotFound => WineRunnerError::GameMissing,
        io::ErrorKind::PermissionDenied => WineRunnerError::AccessDenied,
        _ => WineRunnerError::GameNotLaunchable,
    }
}

/// The only DXVK package currently accepted by Orivo. The archive itself and
/// every DLL are checked against a shipped allowlist before the bytes ever
/// reach a Wine prefix. It is intentionally not serialisable: a package path
/// never crosses the WebView boundary or survives a restart.
#[derive(Debug, Clone)]
pub struct DxvkMacosPackage {
    files: BTreeMap<String, Vec<u8>>,
}

/// Read a DXVK-macOS package that was picked out of the filesystem, checking
/// the archive on disk before any of it is decompressed.
///
/// Test-only. Every shipping path downloads the one fixed release into memory
/// and goes through `load_dxvk_macos_package_bytes` below, so nothing in a
/// release build can be pointed at a path at all. It stays because it is what
/// proves the on-disk checks — symlink, size, digest — reject an archive that
/// is not the allowlisted release, before `load_dxvk_macos_package_reader`
/// ever sees a byte of it.
#[cfg(test)]
pub fn load_dxvk_macos_package(selected: &Path) -> Result<DxvkMacosPackage, WineRunnerError> {
    let selected_metadata =
        fs::symlink_metadata(selected).map_err(|_| WineRunnerError::InvalidDxvkPackage)?;
    if selected_metadata.file_type().is_symlink() {
        return Err(WineRunnerError::InvalidDxvkPackage);
    }
    let archive_path =
        fs::canonicalize(selected).map_err(|_| WineRunnerError::InvalidDxvkPackage)?;
    let metadata =
        fs::symlink_metadata(&archive_path).map_err(|_| WineRunnerError::InvalidDxvkPackage)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_DXVK_ARCHIVE_BYTES
    {
        return Err(WineRunnerError::InvalidDxvkPackage);
    }
    if sha256_file(&archive_path, MAX_DXVK_ARCHIVE_BYTES)? != DXVK_MACOS_ARCHIVE_SHA256 {
        return Err(WineRunnerError::InvalidDxvkPackage);
    }

    let source = fs::File::open(&archive_path).map_err(|_| WineRunnerError::InvalidDxvkPackage)?;
    load_dxvk_macos_package_reader(source)
}

/// Validate the exact fixed DXVK-macOS release after an internal download.
/// The bytes are never named by, exposed to, or supplied by the WebView.
pub fn load_dxvk_macos_package_bytes(bytes: &[u8]) -> Result<DxvkMacosPackage, WineRunnerError> {
    if bytes.is_empty() || bytes.len() > MAX_DXVK_ARCHIVE_BYTES as usize {
        return Err(WineRunnerError::InvalidDxvkPackage);
    }
    let mut digest = Sha256::new();
    digest.update(bytes);
    if format!("{:x}", digest.finalize()) != DXVK_MACOS_ARCHIVE_SHA256 {
        return Err(WineRunnerError::InvalidDxvkPackage);
    }
    load_dxvk_macos_package_reader(Cursor::new(bytes))
}

fn load_dxvk_macos_package_reader(source: impl Read) -> Result<DxvkMacosPackage, WineRunnerError> {
    let decoder = GzDecoder::new(source);
    let mut archive = Archive::new(decoder);
    let mut root = None;
    let mut seen = BTreeSet::new();
    let mut files = BTreeMap::new();
    let entries = archive
        .entries()
        .map_err(|_| WineRunnerError::InvalidDxvkPackage)?;
    for entry in entries {
        let mut entry = entry.map_err(|_| WineRunnerError::InvalidDxvkPackage)?;
        let entry_path = entry
            .path()
            .map_err(|_| WineRunnerError::InvalidDxvkPackage)?
            .into_owned();
        let relative = dxvk_archive_relative_path(&entry_path, &mut root)?;
        let entry_type = entry.header().entry_type();
        match (
            relative.as_deref(),
            entry_type.is_dir(),
            entry_type.is_file(),
        ) {
            (None, true, _) | (Some("x64" | "x32"), true, _) => {}
            (Some("dxvk.conf"), _, true) => {
                if !seen.insert("dxvk.conf".to_string()) {
                    return Err(WineRunnerError::InvalidDxvkPackage);
                }
                let _ = read_limited(&mut entry, MAX_DXVK_CONFIG_BYTES)?;
            }
            (Some(relative), _, true) if expected_dxvk_hash(relative).is_some() => {
                if !seen.insert(relative.to_string()) {
                    return Err(WineRunnerError::InvalidDxvkPackage);
                }
                let bytes = read_limited(&mut entry, MAX_DXVK_ENTRY_BYTES)?;
                validate_dxvk_dll(relative, &bytes)?;
                files.insert(relative.to_string(), bytes);
            }
            _ => return Err(WineRunnerError::InvalidDxvkPackage),
        }
    }

    let package = DxvkMacosPackage { files };
    package.validate()?;
    Ok(package)
}

impl DxvkMacosPackage {
    fn validate(&self) -> Result<(), WineRunnerError> {
        if self.files.len() != DXVK_MACOS_FILES.len()
            || self
                .files
                .keys()
                .any(|relative| !DXVK_MACOS_FILES.contains(&relative.as_str()))
        {
            return Err(WineRunnerError::InvalidDxvkPackage);
        }
        for relative in DXVK_MACOS_FILES {
            let bytes = self
                .files
                .get(relative)
                .ok_or(WineRunnerError::InvalidDxvkPackage)?;
            validate_dxvk_dll(relative, bytes)?;
        }
        Ok(())
    }
}

/// Initialise the generated prefix through Wine's own fixed `wineboot -u`
/// command, then copy only the allowlisted DXVK D3D10/11 DLLs into it. No
/// installation file is copied into the shared Wine bundle or another app's
/// prefix.
pub fn install_dxvk_macos(
    profile: &WineProfile,
    prefix_root: &Path,
    package: &DxvkMacosPackage,
) -> Result<(), WineRunnerError> {
    profile
        .validate()
        .map_err(|_| WineRunnerError::InvalidProfile)?;
    package.validate()?;
    let prefix = ensure_managed_prefix(&profile.prefix, prefix_root, &profile.id)?;
    let wine_binary = probe_wine_staging(&profile.wine_binary, &AtomicBool::new(false))?;
    initialize_managed_wine_prefix(&wine_binary, &prefix)?;
    install_dxvk_macos_files(&prefix, package)?;
    // Do not persist `DxvkMacos` until the exact four files can be read back
    // and match their shipped hashes. A broken or externally altered private
    // prefix therefore stays on the safe Wine 3D backend instead of claiming
    // that a graphics runtime is ready.
    ensure_dxvk_macos_runtime(&prefix).map_err(|_| WineRunnerError::DxvkInstallation)
}

/// Check whether the private profile prefix already contains the exact
/// allowlisted DXVK-macOS seed. This is host-only preflight state: callers
/// receive no DLL names or paths and cannot nominate another prefix. Unlike
/// the launch/install helpers, the probe never creates a profile directory:
/// a concurrent profile deletion therefore cannot leave an orphan behind.
pub fn profile_has_dxvk_macos_runtime(profile: &WineProfile, prefix_root: &Path) -> bool {
    if profile.validate().is_err() || !valid_opaque_id(&profile.id) {
        return false;
    }
    let Ok(root) = managed_prefix_root(prefix_root) else {
        return false;
    };
    let expected = root.join(&profile.id);
    if profile.prefix != expected {
        return false;
    }
    let Ok(metadata) = fs::symlink_metadata(&expected) else {
        return false;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return false;
    }
    let Ok(prefix) = fs::canonicalize(&expected) else {
        return false;
    };
    if prefix != expected {
        return false;
    }
    ensure_dxvk_macos_runtime(&prefix).is_ok()
}

fn ensure_dxvk_macos_runtime(prefix: &Path) -> Result<(), WineRunnerError> {
    for relative in DXVK_MACOS_FILES {
        let target = dxvk_target_path(prefix, relative)?;
        let metadata =
            fs::symlink_metadata(&target).map_err(|_| WineRunnerError::DxvkRuntimeUnavailable)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > u64::try_from(MAX_DXVK_ENTRY_BYTES).unwrap_or(u64::MAX)
        {
            return Err(WineRunnerError::DxvkRuntimeUnavailable);
        }
        let bytes = fs::read(&target).map_err(|_| WineRunnerError::DxvkRuntimeUnavailable)?;
        validate_dxvk_dll(relative, &bytes).map_err(|_| WineRunnerError::DxvkRuntimeUnavailable)?;
    }
    Ok(())
}

/// Reuse only the exact, host-validated DXVK files previously installed into
/// the profile's legacy prefix as an immutable seed for a new isolated game
/// prefix. This lets Auto select DXVK-macOS for subsequent games without
/// sharing registry/DLL state, without reopening a user archive, and without
/// letting a path from the WebView choose either source or destination.
fn ensure_dxvk_macos_game_runtime(
    profile: &WineProfile,
    game: &WineGameInventoryEntry,
    wine_binary: &Path,
    prefix_root: &Path,
    game_prefix: &Path,
) -> Result<(), WineRunnerError> {
    if ensure_dxvk_macos_runtime(game_prefix).is_ok() {
        return Ok(());
    }
    if game.compatibility.prefix_layout != WinePrefixLayout::Isolated {
        return Err(WineRunnerError::DxvkRuntimeUnavailable);
    }
    let source_prefix = ensure_managed_prefix(&profile.prefix, prefix_root, &profile.id)?;
    let package = dxvk_macos_package_from_prefix(&source_prefix)?;
    initialize_managed_wine_prefix(wine_binary, game_prefix)?;
    install_dxvk_macos_files(game_prefix, &package)?;
    ensure_dxvk_macos_runtime(game_prefix).map_err(|_| WineRunnerError::DxvkInstallation)
}

fn dxvk_macos_package_from_prefix(prefix: &Path) -> Result<DxvkMacosPackage, WineRunnerError> {
    ensure_dxvk_macos_runtime(prefix)?;
    let mut files = BTreeMap::new();
    for relative in DXVK_MACOS_FILES {
        let target = dxvk_target_path(prefix, relative)?;
        let bytes = fs::read(target).map_err(|_| WineRunnerError::DxvkRuntimeUnavailable)?;
        validate_dxvk_dll(relative, &bytes).map_err(|_| WineRunnerError::DxvkRuntimeUnavailable)?;
        files.insert(relative.to_string(), bytes);
    }
    let package = DxvkMacosPackage { files };
    package
        .validate()
        .map_err(|_| WineRunnerError::DxvkRuntimeUnavailable)?;
    Ok(package)
}

fn dxvk_archive_relative_path(
    path: &Path,
    root: &mut Option<String>,
) -> Result<Option<String>, WineRunnerError> {
    let mut components = path.components();
    let Some(std::path::Component::Normal(root_component)) = components.next() else {
        return Err(WineRunnerError::InvalidDxvkPackage);
    };
    let root_component = root_component
        .to_str()
        .filter(|component| {
            !component.is_empty()
                && component.len() <= 160
                && !component.chars().any(char::is_control)
        })
        .ok_or(WineRunnerError::InvalidDxvkPackage)?;
    match root {
        Some(existing) if existing != root_component => {
            return Err(WineRunnerError::InvalidDxvkPackage);
        }
        Some(_) => {}
        None => *root = Some(root_component.into()),
    }
    let mut relative = Vec::new();
    for component in components {
        let std::path::Component::Normal(component) = component else {
            return Err(WineRunnerError::InvalidDxvkPackage);
        };
        let component = component
            .to_str()
            .filter(|component| !component.is_empty() && !component.chars().any(char::is_control))
            .ok_or(WineRunnerError::InvalidDxvkPackage)?;
        relative.push(component);
    }
    if relative.is_empty() {
        Ok(None)
    } else {
        Ok(Some(relative.join("/")))
    }
}

fn read_limited(reader: &mut impl Read, maximum: usize) -> Result<Vec<u8>, WineRunnerError> {
    let mut bytes = Vec::new();
    reader
        .take(u64::try_from(maximum.saturating_add(1)).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|_| WineRunnerError::InvalidDxvkPackage)?;
    if bytes.len() > maximum {
        return Err(WineRunnerError::InvalidDxvkPackage);
    }
    Ok(bytes)
}

/// Digest a bounded file without reading it all into memory. Test-only, along
/// with its one caller: a shipping build hashes the DXVK release from the
/// bytes it just downloaded, never from a path.
#[cfg(test)]
fn sha256_file(path: &Path, maximum: u64) -> Result<String, WineRunnerError> {
    let metadata = fs::metadata(path).map_err(|_| WineRunnerError::InvalidDxvkPackage)?;
    if metadata.len() > maximum {
        return Err(WineRunnerError::InvalidDxvkPackage);
    }
    let mut file = fs::File::open(path).map_err(|_| WineRunnerError::InvalidDxvkPackage)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; FINGERPRINT_BUFFER_BYTES];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| WineRunnerError::InvalidDxvkPackage)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn expected_dxvk_hash(relative: &str) -> Option<&'static str> {
    match relative {
        "x64/d3d10core.dll" => {
            Some("0fa08bba860c63e3abeeabfc96d0e7aa327411a975f8f23d2dc63594ef5f796e")
        }
        "x64/d3d11.dll" => Some("0ff0b0835dde29556bd01dfce7b1ae348d7f229cb1e1a37cc71ea1a028beeca4"),
        "x32/d3d10core.dll" => {
            Some("2277187a4e13fc2049fe28cb8dd05b34b48c68031c386c3597ba0e18b0c3d327")
        }
        "x32/d3d11.dll" => Some("f79d417f675d00008375eb4ea2318af6fcd0b27971d604d3cf345a0dcfa998a0"),
        _ => None,
    }
}

fn validate_dxvk_dll(relative: &str, bytes: &[u8]) -> Result<(), WineRunnerError> {
    let expected_hash = expected_dxvk_hash(relative).ok_or(WineRunnerError::InvalidDxvkPackage)?;
    let expected_machine = if relative.starts_with("x64/") {
        0x8664
    } else {
        0x014c
    };
    validate_pe_machine(bytes, expected_machine)?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    if format!("{:x}", digest.finalize()) != expected_hash {
        return Err(WineRunnerError::InvalidDxvkPackage);
    }
    Ok(())
}

fn validate_pe_machine(bytes: &[u8], expected_machine: u16) -> Result<(), WineRunnerError> {
    if bytes.len() < 0x40 || &bytes[..2] != b"MZ" {
        return Err(WineRunnerError::InvalidDxvkPackage);
    }
    let offset = u32::from_le_bytes(
        bytes[0x3c..0x40]
            .try_into()
            .map_err(|_| WineRunnerError::InvalidDxvkPackage)?,
    ) as usize;
    if offset > bytes.len().saturating_sub(6) || &bytes[offset..offset + 4] != b"PE\0\0" {
        return Err(WineRunnerError::InvalidDxvkPackage);
    }
    let machine = u16::from_le_bytes(
        bytes[offset + 4..offset + 6]
            .try_into()
            .map_err(|_| WineRunnerError::InvalidDxvkPackage)?,
    );
    if machine != expected_machine {
        return Err(WineRunnerError::InvalidDxvkPackage);
    }
    Ok(())
}

fn dxvk_target_path(prefix: &Path, relative: &str) -> Result<PathBuf, WineRunnerError> {
    let (directory, file_name) = match relative {
        "x64/d3d10core.dll" | "x64/d3d11.dll" => (
            prefix.join("drive_c/windows/system32"),
            relative.rsplit('/').next().unwrap_or_default(),
        ),
        "x32/d3d10core.dll" | "x32/d3d11.dll" => (
            prefix.join("drive_c/windows/syswow64"),
            relative.rsplit('/').next().unwrap_or_default(),
        ),
        _ => return Err(WineRunnerError::InvalidDxvkPackage),
    };
    Ok(directory.join(file_name))
}

fn initialize_managed_wine_prefix(
    wine_binary: &Path,
    prefix: &Path,
) -> Result<(), WineRunnerError> {
    let binary_directory = wine_binary
        .parent()
        .ok_or(WineRunnerError::PrefixInitialization)?;
    let wineboot = binary_directory.join("wineboot");
    let metadata =
        fs::symlink_metadata(&wineboot).map_err(|_| WineRunnerError::PrefixInitialization)?;
    let resolved_wineboot =
        fs::canonicalize(&wineboot).map_err(|_| WineRunnerError::PrefixInitialization)?;
    let resolved_directory =
        fs::canonicalize(binary_directory).map_err(|_| WineRunnerError::PrefixInitialization)?;
    let is_expected_wineboot_symlink =
        metadata.file_type().is_symlink() && resolved_wineboot == wine_binary;
    if (!metadata.is_file() && !is_expected_wineboot_symlink)
        || !is_executable(&resolved_wineboot)
        || (resolved_wineboot != wine_binary && !resolved_wineboot.starts_with(&resolved_directory))
    {
        return Err(WineRunnerError::PrefixInitialization);
    }

    // Preserve the `wineboot` basename when it is a normal Wine symlink. Wine
    // uses it to select its boot path, so invoking the canonical `wine` file
    // instead would be subtly different even though both paths point to the
    // same executable in common macOS bundles.
    let mut command = Command::new(&wineboot);
    clear_untrusted_wine_environment(&mut command);
    let mut child = command
        .arg("-u")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("WINEPREFIX", prefix)
        .spawn()
        .map_err(|_| WineRunnerError::PrefixInitialization)?;
    let deadline = Instant::now() + WINE_PREFIX_INITIALIZATION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                // Wine-Staging may return from wineboot before its detached
                // wineserver has finished materialising the registry. Do not
                // run a fixed registry mutation into that gap: Wine reports
                // success but drops it for an as-yet-empty prefix.
                while !managed_wine_prefix_has_registry(prefix) {
                    if Instant::now() >= deadline {
                        return Err(WineRunnerError::PrefixInitialization);
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                return Ok(());
            }
            Ok(Some(_)) | Err(_) => return Err(WineRunnerError::PrefixInitialization),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WineRunnerError::PrefixInitialization);
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
        }
    }
}

/// Only use this as a readiness check for a prefix we have already validated
/// as a direct child of Orivo's private root. `symlink_metadata` keeps an
/// unexpected redirected registry from being considered ready.
fn managed_wine_prefix_has_registry(prefix: &Path) -> bool {
    matches!(
        fs::symlink_metadata(prefix.join("user.reg")),
        Ok(metadata) if metadata.is_file()
    )
}

/// Keep Wine's macOS driver aligned with the active primary display without
/// exposing a registry key, display size or Wine argument to the WebView.
/// `RetinaMode` is a prefix-wide Wine setting, so this runs only for an
/// Orivo-owned prefix and only when the host observes a changed display mode.
fn synchronize_macos_retina_mode(
    wine_binary: &Path,
    prefix: &Path,
    previously_enabled: Option<bool>,
) -> Result<Option<bool>, WineRunnerError> {
    #[cfg(target_os = "macos")]
    {
        let Some(enabled) = macos_primary_display_retina_mode() else {
            // Do not override a working profile when CoreGraphics cannot
            // report a display mode (for example while the display is
            // reconnecting). The next launch can retry safely.
            return Ok(previously_enabled);
        };
        if previously_enabled == Some(enabled) {
            return Ok(previously_enabled);
        }

        // `wine reg add` exits successfully without creating a new prefix on
        // some Wine-Staging builds. Bootstrap only an empty private prefix
        // first; existing profiles keep their Wine state untouched.
        if !managed_wine_prefix_has_registry(prefix) {
            initialize_managed_wine_prefix(wine_binary, prefix)?;
        }

        let value = if enabled { "y" } else { "n" };
        let mut command = Command::new(wine_binary);
        clear_untrusted_wine_environment(&mut command);
        let mut child = command
            .arg("reg")
            .arg("add")
            .arg(WINE_MAC_DRIVER_REGISTRY_KEY)
            .arg("/v")
            .arg(WINE_MAC_DRIVER_RETINA_MODE_VALUE)
            .arg("/t")
            .arg("REG_SZ")
            .arg("/d")
            .arg(value)
            .arg("/f")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env("WINEPREFIX", prefix)
            .spawn()
            .map_err(|_| WineRunnerError::PrefixInitialization)?;
        let deadline = Instant::now() + WINE_PREFIX_CONFIGURATION_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(Some(enabled)),
                Ok(Some(_)) | Err(_) => return Err(WineRunnerError::PrefixInitialization),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(WineRunnerError::PrefixInitialization);
                }
                Ok(None) => thread::sleep(Duration::from_millis(25)),
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (wine_binary, prefix, previously_enabled);
        Ok(None)
    }
}

/// CoreGraphics exposes the current logical mode and backing-pixel mode
/// separately. Wine's macOS driver needs RetinaMode only when those differ;
/// enabling it on a conventional display would make the output blurry.
#[cfg(target_os = "macos")]
fn macos_primary_display_retina_mode() -> Option<bool> {
    let mode = CGDisplay::main().display_mode()?;
    let logical_width = mode.width();
    let logical_height = mode.height();
    let pixel_width = mode.pixel_width();
    let pixel_height = mode.pixel_height();
    if logical_width == 0 || logical_height == 0 || pixel_width == 0 || pixel_height == 0 {
        return None;
    }
    Some(pixel_width > logical_width || pixel_height > logical_height)
}

#[cfg(unix)]
#[derive(Debug, Clone)]
struct PrivateDxvkDirectory {
    descriptor: Arc<OwnedFd>,
}

#[cfg(unix)]
impl PrivateDxvkDirectory {
    fn raw_descriptor(&self) -> libc::c_int {
        self.descriptor.as_raw_fd()
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct StagedDxvkFile {
    directory: PrivateDxvkDirectory,
    target_name: String,
    temporary_name: String,
    backup_name: String,
    had_previous: bool,
    committed: bool,
}

/// Install into descriptor-pinned directories only. Every `openat`, rename
/// and unlink is relative to a file descriptor opened with `O_NOFOLLOW`, so a
/// later swap of `drive_c`, `windows` or `system32` cannot redirect a write
/// outside Orivo's private prefix.
fn install_dxvk_macos_files(
    prefix: &Path,
    package: &DxvkMacosPackage,
) -> Result<(), WineRunnerError> {
    #[cfg(unix)]
    {
        return install_dxvk_macos_files_unix(prefix, package);
    }

    #[cfg(not(unix))]
    {
        let _ = (prefix, package);
        Err(WineRunnerError::DxvkInstallation)
    }
}

#[cfg(unix)]
fn install_dxvk_macos_files_unix(
    prefix: &Path,
    package: &DxvkMacosPackage,
) -> Result<(), WineRunnerError> {
    let mut staged = Vec::with_capacity(DXVK_MACOS_FILES.len());
    for relative in DXVK_MACOS_FILES {
        let bytes = package
            .files
            .get(relative)
            .ok_or(WineRunnerError::InvalidDxvkPackage)?;
        let (directory_components, file_name) = dxvk_target_components(relative)?;
        let directory = open_private_dxvk_target_directory(prefix, directory_components)?;
        if private_dxvk_entry_exists(&directory, file_name).is_err() {
            rollback_dxvk_files(&mut staged);
            return Err(WineRunnerError::DxvkInstallation);
        }
        let sequence = DXVK_INSTALL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary_name = format!(
            ".orivo-dxvk-{}-{sequence}-{file_name}.new",
            std::process::id()
        );
        let backup_name = format!(
            ".orivo-dxvk-{}-{sequence}-{file_name}.bak",
            std::process::id()
        );
        let mut temporary = match create_private_dxvk_file(&directory, &temporary_name) {
            Ok(file) => file,
            Err(_) => {
                rollback_dxvk_files(&mut staged);
                return Err(WineRunnerError::DxvkInstallation);
            }
        };
        if temporary
            .write_all(bytes)
            .and_then(|_| temporary.sync_all())
            .is_err()
        {
            let _ = unlink_private_dxvk_entry(&directory, &temporary_name);
            rollback_dxvk_files(&mut staged);
            return Err(WineRunnerError::DxvkInstallation);
        }
        staged.push(StagedDxvkFile {
            directory,
            target_name: file_name.into(),
            temporary_name,
            backup_name,
            had_previous: false,
            committed: false,
        });
    }

    for index in 0..staged.len() {
        let stage = &mut staged[index];
        if private_dxvk_entry_exists(&stage.directory, &stage.target_name)? {
            if rename_private_dxvk_entry(&stage.directory, &stage.target_name, &stage.backup_name)
                .is_err()
            {
                rollback_dxvk_files(&mut staged);
                return Err(WineRunnerError::DxvkInstallation);
            }
            stage.had_previous = true;
        }
        if rename_private_dxvk_entry(&stage.directory, &stage.temporary_name, &stage.target_name)
            .is_err()
        {
            rollback_dxvk_files(&mut staged);
            return Err(WineRunnerError::DxvkInstallation);
        }
        stage.committed = true;
    }
    for stage in &staged {
        if stage.had_previous {
            let _ = unlink_private_dxvk_entry(&stage.directory, &stage.backup_name);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn dxvk_target_components(relative: &str) -> Result<(&'static str, &'static str), WineRunnerError> {
    match relative {
        "x64/d3d10core.dll" => Ok(("drive_c/windows/system32", "d3d10core.dll")),
        "x64/d3d11.dll" => Ok(("drive_c/windows/system32", "d3d11.dll")),
        "x32/d3d10core.dll" => Ok(("drive_c/windows/syswow64", "d3d10core.dll")),
        "x32/d3d11.dll" => Ok(("drive_c/windows/syswow64", "d3d11.dll")),
        _ => Err(WineRunnerError::InvalidDxvkPackage),
    }
}

#[cfg(unix)]
fn open_private_dxvk_target_directory(
    prefix: &Path,
    components: &str,
) -> Result<PrivateDxvkDirectory, WineRunnerError> {
    let mut directory = open_directory_without_following_links(prefix)
        .map_err(|_| WineRunnerError::DxvkInstallation)?;
    for component in components.split('/') {
        directory = open_or_create_private_dxvk_directory(&directory, component)?;
    }
    Ok(PrivateDxvkDirectory {
        descriptor: Arc::new(directory.into()),
    })
}

#[cfg(unix)]
fn open_or_create_private_dxvk_directory(
    parent: &fs::File,
    name: &str,
) -> Result<fs::File, WineRunnerError> {
    match open_directory_at(parent.as_raw_fd(), name) {
        Ok(descriptor) => owned_descriptor_to_file(descriptor),
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
            let name = CString::new(name).map_err(|_| WineRunnerError::DxvkInstallation)?;
            let created = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
            if created != 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
                return Err(WineRunnerError::DxvkInstallation);
            }
            open_directory_at(parent.as_raw_fd(), name.to_str().unwrap_or_default())
                .map_err(|_| WineRunnerError::DxvkInstallation)
                .and_then(owned_descriptor_to_file)
        }
        Err(_) => Err(WineRunnerError::DxvkInstallation),
    }
}

#[cfg(unix)]
fn open_directory_at(parent: libc::c_int, name: &str) -> io::Result<OwnedFd> {
    let name = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let descriptor = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

#[cfg(unix)]
fn owned_descriptor_to_file(descriptor: OwnedFd) -> Result<fs::File, WineRunnerError> {
    Ok(unsafe { fs::File::from_raw_fd(descriptor.into_raw_fd()) })
}

#[cfg(unix)]
fn create_private_dxvk_file(
    directory: &PrivateDxvkDirectory,
    name: &str,
) -> Result<fs::File, WineRunnerError> {
    let name = CString::new(name).map_err(|_| WineRunnerError::DxvkInstallation)?;
    let descriptor = unsafe {
        libc::openat(
            directory.raw_descriptor(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(WineRunnerError::DxvkInstallation);
    }
    Ok(unsafe { fs::File::from_raw_fd(descriptor) })
}

#[cfg(unix)]
fn private_dxvk_entry_exists(
    directory: &PrivateDxvkDirectory,
    name: &str,
) -> Result<bool, WineRunnerError> {
    let name = CString::new(name).map_err(|_| WineRunnerError::DxvkInstallation)?;
    let mut status = unsafe { std::mem::zeroed::<libc::stat>() };
    let result = unsafe {
        libc::fstatat(
            directory.raw_descriptor(),
            name.as_ptr(),
            &mut status,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        if (status.st_mode & libc::S_IFMT) != libc::S_IFREG {
            return Err(WineRunnerError::DxvkInstallation);
        }
        return Ok(true);
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(libc::ENOENT) => Ok(false),
        _ => Err(WineRunnerError::DxvkInstallation),
    }
}

#[cfg(unix)]
fn rename_private_dxvk_entry(
    directory: &PrivateDxvkDirectory,
    from: &str,
    to: &str,
) -> Result<(), WineRunnerError> {
    let from = CString::new(from).map_err(|_| WineRunnerError::DxvkInstallation)?;
    let to = CString::new(to).map_err(|_| WineRunnerError::DxvkInstallation)?;
    let result = unsafe {
        libc::renameat(
            directory.raw_descriptor(),
            from.as_ptr(),
            directory.raw_descriptor(),
            to.as_ptr(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(WineRunnerError::DxvkInstallation)
    }
}

#[cfg(unix)]
fn unlink_private_dxvk_entry(
    directory: &PrivateDxvkDirectory,
    name: &str,
) -> Result<(), WineRunnerError> {
    let name = CString::new(name).map_err(|_| WineRunnerError::DxvkInstallation)?;
    let result = unsafe { libc::unlinkat(directory.raw_descriptor(), name.as_ptr(), 0) };
    if result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
        Ok(())
    } else {
        Err(WineRunnerError::DxvkInstallation)
    }
}

#[cfg(unix)]
fn rollback_dxvk_files(staged: &mut [StagedDxvkFile]) {
    for stage in staged.iter_mut().rev() {
        if stage.committed {
            let _ = unlink_private_dxvk_entry(&stage.directory, &stage.target_name);
            stage.committed = false;
        }
        if stage.had_previous
            && matches!(
                private_dxvk_entry_exists(&stage.directory, &stage.target_name),
                Ok(false)
            )
        {
            let _ =
                rename_private_dxvk_entry(&stage.directory, &stage.backup_name, &stage.target_name);
            stage.had_previous = false;
        }
        let _ = unlink_private_dxvk_entry(&stage.directory, &stage.temporary_name);
    }
}

fn scan_directory(
    root: &Path,
    directory: &Path,
    label: &str,
    depth: usize,
    limits: ScanLimits,
    cancelled: &AtomicBool,
    scanned_files: &mut usize,
    candidates: &mut BTreeMap<String, ScannedWineGame>,
    progress: &mut impl FnMut(usize),
) -> Result<(), WineRunnerError> {
    cancelled_or(cancelled)?;
    let entries = fs::read_dir(directory).map_err(|_| WineRunnerError::AccessDenied)?;
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| WineRunnerError::AccessDenied)?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        cancelled_or(cancelled)?;
        *scanned_files = scanned_files.saturating_add(1);
        if *scanned_files > limits.max_files {
            return Err(WineRunnerError::TooManyFiles);
        }
        if *scanned_files % 32 == 0 {
            progress(*scanned_files);
        }
        let file_type = entry
            .file_type()
            .map_err(|_| WineRunnerError::AccessDenied)?;
        // Never traverse a symlink: canonicalisation below is a second line
        // of defence for files and overlapping granted roots.
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            if depth < limits.max_depth {
                scan_directory(
                    root,
                    &path,
                    label,
                    depth + 1,
                    limits,
                    cancelled,
                    scanned_files,
                    candidates,
                    progress,
                )?;
            }
            continue;
        }
        if !file_type.is_file() || !is_windows_executable(&path) {
            continue;
        }
        let executable = fs::canonicalize(path).map_err(|_| WineRunnerError::AccessDenied)?;
        if executable == root || !executable.starts_with(root) || !executable.is_file() {
            continue;
        }
        let fingerprint = content_fingerprint_for(&executable, cancelled)?;
        let game_ref = game_reference_for(&executable);
        candidates
            .entry(game_ref.clone())
            .or_insert_with(|| ScannedWineGame {
                game_ref,
                title: game_title(&executable),
                directory_label: label.into(),
                executable_path: executable,
                fingerprint,
            });
    }
    progress(*scanned_files);
    Ok(())
}

/// Recheck a scan snapshot at the exact moment it crosses into persistence.
/// A scan is only a preview: the game may have moved, been replaced, or had a
/// symlink inserted before the user presses Import.
pub fn revalidate_wine_import_candidate(
    profile: &WineProfile,
    candidate: &ScannedWineGame,
    cancelled: &AtomicBool,
) -> Result<ScannedWineGame, WineRunnerError> {
    if !profile.enabled {
        return Err(WineRunnerError::ProfileDisabled);
    }
    profile
        .validate()
        .map_err(|_| WineRunnerError::InvalidProfile)?;
    cancelled_or(cancelled)?;
    let current = validate_wine_game_for_profile(profile, &candidate.executable_path, cancelled)?;
    if candidate.game_ref != current.game_ref {
        return Err(WineRunnerError::GameNotLaunchable);
    }
    Ok(ScannedWineGame {
        game_ref: current.game_ref,
        title: current.title,
        directory_label: candidate.directory_label.clone(),
        executable_path: current.executable_path,
        fingerprint: current.fingerprint,
    })
}

/// Revalidate one host-owned executable against a profile without accepting a
/// scanner reference from the caller. This powers the explicit conversion of
/// an already-imported Direct `.exe` into a Wine runner card: the WebView can
/// name only its catalog id, while the native host resolves and hashes the
/// stored path itself.
pub fn validate_wine_game_for_profile(
    profile: &WineProfile,
    executable: &Path,
    cancelled: &AtomicBool,
) -> Result<ScannedWineGame, WineRunnerError> {
    if !profile.enabled {
        return Err(WineRunnerError::ProfileDisabled);
    }
    profile
        .validate()
        .map_err(|_| WineRunnerError::InvalidProfile)?;
    cancelled_or(cancelled)?;
    let executable = fs::canonicalize(executable).map_err(|_| WineRunnerError::GameMissing)?;
    if !executable.is_file() || !is_windows_executable(&executable) {
        return Err(WineRunnerError::GameMissing);
    }
    if !belongs_to_grant(&executable, &profile.game_directories)? {
        return Err(WineRunnerError::GameOutsideScope);
    }
    let fingerprint = content_fingerprint_for(&executable, cancelled)?;
    Ok(ScannedWineGame {
        game_ref: game_reference_for(&executable),
        title: game_title(&executable),
        directory_label: executable
            .parent()
            .map(|directory| safe_label(directory, "Authorized game folder"))
            .unwrap_or_else(|| "Authorized game folder".into()),
        executable_path: executable,
        fingerprint,
    })
}

/// Create a one-profile-only prefix beneath the host-managed root. This is
/// deliberately a host helper: no plugin or WebView value ever contributes a
/// prefix component.
pub fn create_managed_prefix(
    prefix_root: &Path,
    profile_id: &str,
) -> Result<PathBuf, WineRunnerError> {
    if !valid_opaque_id(profile_id) {
        return Err(WineRunnerError::InvalidProfile);
    }
    let root = managed_prefix_root(prefix_root)?;
    let prefix = root.join(profile_id);
    match fs::symlink_metadata(&prefix) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(WineRunnerError::InvalidProfile);
        }
        // A new profile id must never reuse an existing directory. This
        // avoids accidentally adopting an old or externally-created prefix.
        Ok(_) => return Err(WineRunnerError::InvalidProfile),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(WineRunnerError::InvalidProfile),
    }
    fs::create_dir(&prefix).map_err(|_| WineRunnerError::InvalidProfile)?;
    let canonical = fs::canonicalize(&prefix).map_err(|_| WineRunnerError::InvalidProfile)?;
    if canonical != prefix {
        return Err(WineRunnerError::InvalidProfile);
    }
    Ok(canonical)
}

/// Create the one-profile prefix beneath the host-managed root, reusing it if
/// Orivo already created it. Unlike `create_managed_prefix`, this is safe to
/// call repeatedly for a persistent managed profile (such as the automatic
/// Windows-games default that Orivo provisions without a setup wizard): a
/// concurrent symlink swap still fails closed. No plugin or WebView value ever
/// contributes a prefix component.
pub fn ensure_managed_profile_prefix(
    prefix_root: &Path,
    profile_id: &str,
) -> Result<PathBuf, WineRunnerError> {
    if !valid_opaque_id(profile_id) {
        return Err(WineRunnerError::InvalidProfile);
    }
    let expected = managed_prefix_root(prefix_root)?.join(profile_id);
    ensure_managed_prefix(&expected, prefix_root, profile_id)
}

fn ensure_managed_prefix(
    prefix: &Path,
    prefix_root: &Path,
    profile_id: &str,
) -> Result<PathBuf, WineRunnerError> {
    if !valid_opaque_id(profile_id) {
        return Err(WineRunnerError::InvalidProfile);
    }
    let root = managed_prefix_root(prefix_root)?;
    let expected = root.join(profile_id);
    if prefix != expected {
        return Err(WineRunnerError::InvalidProfile);
    }
    match fs::symlink_metadata(&expected) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(WineRunnerError::InvalidProfile);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&expected).map_err(|_| WineRunnerError::InvalidProfile)?;
        }
        Err(_) => return Err(WineRunnerError::InvalidProfile),
    }
    let canonical = fs::canonicalize(&expected).map_err(|_| WineRunnerError::InvalidProfile)?;
    if canonical != expected {
        return Err(WineRunnerError::InvalidProfile);
    }
    Ok(canonical)
}

/// Return the opaque filesystem component for a private game prefix. The
/// source identifiers are validated before this point and are hashed rather
/// than written into a path component, so a game reference can never become a
/// traversal segment or disclose a source path through the filesystem.
fn game_prefix_component(profile_id: &str, game_ref: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orivo-wine-game-prefix-v1\0");
    digest.update(profile_id.as_bytes());
    digest.update(b"\0");
    digest.update(game_ref.as_bytes());
    format!("{:x}", digest.finalize())
}

/// Ensure a game-specific prefix beneath the host-owned root. The legacy
/// `<root>/<profile-id>` prefix is deliberately not reused: new games receive
/// their own mutable registry/DLL state, while existing v5 games keep their
/// original profile prefix through `WinePrefixLayout::LegacySharedProfile`.
pub fn ensure_managed_game_prefix(
    prefix_root: &Path,
    profile_id: &str,
    game_ref: &str,
) -> Result<PathBuf, WineRunnerError> {
    if !valid_opaque_id(profile_id) || !valid_opaque_id(game_ref) {
        return Err(WineRunnerError::InvalidProfile);
    }
    let root = managed_prefix_root(prefix_root)?;
    let games = ensure_private_directory_child(&root, "games")?;
    let profile = ensure_private_directory_child(&games, profile_id)?;
    ensure_private_directory_child(&profile, &game_prefix_component(profile_id, game_ref))
}

/// Create exactly one path component under an already canonicalised private
/// directory. A post-create canonical check makes a concurrent symlink swap
/// fail closed rather than redirecting a future Wine prefix outside Orivo.
fn ensure_private_directory_child(
    parent: &Path,
    component: &str,
) -> Result<PathBuf, WineRunnerError> {
    if component.is_empty()
        || component.len() > 160
        || component.chars().any(char::is_control)
        || component.contains('/')
        || component.contains('\\')
        || component == "."
        || component == ".."
    {
        return Err(WineRunnerError::InvalidProfile);
    }
    let expected = parent.join(component);
    match fs::symlink_metadata(&expected) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(WineRunnerError::InvalidProfile);
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(&expected).map_err(|_| WineRunnerError::InvalidProfile)?;
        }
        Err(_) => return Err(WineRunnerError::InvalidProfile),
    }
    let canonical = fs::canonicalize(&expected).map_err(|_| WineRunnerError::InvalidProfile)?;
    if canonical != expected || !canonical.starts_with(parent) {
        return Err(WineRunnerError::InvalidProfile);
    }
    Ok(canonical)
}

fn managed_prefix_root(prefix_root: &Path) -> Result<PathBuf, WineRunnerError> {
    if !prefix_root.is_absolute()
        || prefix_root.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(WineRunnerError::InvalidProfile);
    }
    fs::create_dir_all(prefix_root).map_err(|_| WineRunnerError::InvalidProfile)?;
    let metadata =
        fs::symlink_metadata(prefix_root).map_err(|_| WineRunnerError::InvalidProfile)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(WineRunnerError::InvalidProfile);
    }
    fs::canonicalize(prefix_root).map_err(|_| WineRunnerError::InvalidProfile)
}

fn belongs_to_grant(executable: &Path, directories: &[PathBuf]) -> Result<bool, WineRunnerError> {
    let mut readable_grant = false;
    for directory in directories {
        let root = match fs::canonicalize(directory) {
            Ok(root) if root.is_dir() => root,
            Ok(_) => continue,
            Err(_) => continue,
        };
        readable_grant = true;
        if executable != root && executable.starts_with(&root) {
            return Ok(true);
        }
    }
    if readable_grant {
        Ok(false)
    } else {
        Err(WineRunnerError::AccessDenied)
    }
}

fn cancelled_or(cancelled: &AtomicBool) -> Result<(), WineRunnerError> {
    if cancelled.load(Ordering::Acquire) {
        Err(WineRunnerError::Cancelled)
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

fn is_windows_executable(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

fn game_title(path: &Path) -> String {
    let title = path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
        .map(|name| name.chars().take(160).collect());
    title.unwrap_or_else(|| "Windows game".into())
}

fn safe_label(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
        .map(|name| name.chars().take(96).collect())
        .unwrap_or_else(|| fallback.into())
}

/// Hash executable bytes independently from the persistent game reference.
/// The reference is path-stable so a patched executable refreshes one library
/// card, while the content digest makes the host reject a changed file until a
/// deliberate reimport has updated its private inventory.
fn content_fingerprint_for(path: &Path, cancelled: &AtomicBool) -> Result<String, WineRunnerError> {
    let mut file = fs::File::open(path).map_err(|_| WineRunnerError::AccessDenied)?;
    content_fingerprint_for_open_file(&mut file, cancelled)
}

fn content_fingerprint_for_open_file(
    file: &mut fs::File,
    cancelled: &AtomicBool,
) -> Result<String, WineRunnerError> {
    let metadata = file.metadata().map_err(|_| WineRunnerError::AccessDenied)?;
    if metadata.len() > MAX_FINGERPRINT_BYTES {
        return Err(WineRunnerError::FingerprintTooLarge);
    }
    if !metadata.is_file() {
        return Err(WineRunnerError::GameNotLaunchable);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| WineRunnerError::AccessDenied)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; FINGERPRINT_BUFFER_BYTES];
    loop {
        cancelled_or(cancelled)?;
        let read = file
            .read(&mut buffer)
            .map_err(|_| WineRunnerError::AccessDenied)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn game_reference_for(canonical_path: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(b"orivo-wine-game-reference-v1\0");
    digest.update(canonical_path.as_os_str().as_encoded_bytes());
    format!("exe:{:x}", digest.finalize())
}

fn wine_probe_path_error(error: io::Error) -> WineRunnerError {
    match error.kind() {
        io::ErrorKind::PermissionDenied => WineRunnerError::WineAccessDenied,
        io::ErrorKind::NotFound => WineRunnerError::WineMissing,
        _ => WineRunnerError::InvalidWine,
    }
}

struct ProbePrefix {
    path: PathBuf,
}

impl ProbePrefix {
    fn create() -> Result<Self, WineRunnerError> {
        let root = std::env::temp_dir().join("orivo-wine-probe");
        fs::create_dir_all(&root).map_err(|_| WineRunnerError::WineAccessDenied)?;
        let root_metadata =
            fs::symlink_metadata(&root).map_err(|_| WineRunnerError::WineAccessDenied)?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(WineRunnerError::WineAccessDenied);
        }
        let root = fs::canonicalize(&root).map_err(|_| WineRunnerError::WineAccessDenied)?;
        for _ in 0..32 {
            let sequence = WINE_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!("{}-{sequence}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    let canonical =
                        fs::canonicalize(&path).map_err(|_| WineRunnerError::WineAccessDenied)?;
                    if canonical != path {
                        let _ = fs::remove_dir_all(&path);
                        return Err(WineRunnerError::WineAccessDenied);
                    }
                    return Ok(Self { path: canonical });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(WineRunnerError::WineAccessDenied),
            }
        }
        Err(WineRunnerError::WineAccessDenied)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ProbePrefix {
    fn drop(&mut self) {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let _ = fs::remove_file(&self.path);
            }
            Ok(metadata) if metadata.is_dir() => {
                let _ = fs::remove_dir_all(&self.path);
            }
            _ => {}
        }
    }
}

fn clear_untrusted_wine_environment(command: &mut Command) {
    // Wine reads a broad family of WINE* switches. Carrying one from the
    // parent process into either the probe or launch could select another
    // application's prefix or loader. macOS's DYLD injection family is also
    // removed before a user-selected binary is started.
    for key in [
        "WINEPREFIX",
        "WINEARCH",
        "WINEDLLOVERRIDES",
        "WINEDEBUG",
        "WINESERVER",
        "WINELOADER",
        "WINELOADERNOEXEC",
        "DYLD_INSERT_LIBRARIES",
        "DYLD_LIBRARY_PATH",
        "DYLD_FRAMEWORK_PATH",
    ] {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        let Some(key_name) = key.to_str() else {
            continue;
        };
        let upper = key_name.to_ascii_uppercase();
        if upper.starts_with("WINE") || upper.starts_with("DYLD_") {
            command.env_remove(key);
        }
    }
}

fn is_wine_staging_version(version: &str) -> bool {
    version.lines().any(|line| {
        let line = line.trim_start();
        (line.starts_with("wine-") || line.starts_with("wine64-")) && line.contains("staging")
    })
}

#[cfg(all(target_os = "macos", not(test)))]
fn is_macos_mach_o_binary(path: &Path) -> bool {
    let mut magic = [0_u8; 4];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && matches!(
            magic,
            // 32-bit and 64-bit Mach-O, both endiannesses.
            [0xfe, 0xed, 0xfa, 0xce]
                | [0xce, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xcf, 0xfa, 0xed, 0xfe]
                // Universal/fat Mach-O, both 32-bit and 64-bit forms.
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
                | [0xca, 0xfe, 0xba, 0xbf]
                | [0xbf, 0xba, 0xfe, 0xca]
        )
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.metadata()
        .map(|metadata| !metadata.permissions().readonly())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{
        WineGameCompatibility, WineGraphicsBackend, WineGraphicsOptions, WineProfile,
    };
    use std::{
        ffi::OsStr,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temporary_directory(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "orivo-wine-runner-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::canonicalize(directory).unwrap()
    }

    fn write_staging_binary(directory: &Path) -> PathBuf {
        let wine = directory.join("wine");
        fs::write(
            &wine,
            "#!/bin/sh\nif [ \"${1:-}\" = \"-u\" ]; then\n  mkdir -p \"$WINEPREFIX\"\n  : > \"$WINEPREFIX/user.reg\"\n  exit 0\nfi\nif [ \"${1:-}\" = \"reg\" ]; then\n  exit 0\nfi\necho 'wine-10.0 (Staging)'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            use std::os::unix::fs::symlink;
            let mut permissions = fs::metadata(&wine).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&wine, permissions).unwrap();
            symlink(&wine, directory.join("wineboot")).unwrap();
        }
        wine
    }

    fn profile(root: &Path, wine: PathBuf, games: PathBuf) -> WineProfile {
        WineProfile {
            id: "wine-profile-test".into(),
            display_name: "Windows games".into(),
            wine_binary: wine,
            prefix: root.join("prefixes/wine-profile-test"),
            game_directories: vec![games],
            graphics: WineGraphicsOptions::default(),
            dxmt_engine_supported: None,
            macos_retina_mode_enabled: None,
            enabled: true,
            last_imported_at: None,
        }
    }

    #[test]
    fn requires_an_executable_staging_binary() {
        let root = temporary_directory("probe");
        let wine = write_staging_binary(&root);
        assert!(probe_wine_staging(&wine, &AtomicBool::new(false)).is_ok());
        fs::write(root.join("not-wine"), "text").unwrap();
        assert_eq!(
            probe_wine_staging(&root.join("not-wine"), &AtomicBool::new(false)),
            Err(WineRunnerError::InvalidWine)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scans_only_executables_in_granted_directories_and_pages_them() {
        let root = temporary_directory("scan");
        let games = root.join("Games");
        fs::create_dir_all(games.join("Nested")).unwrap();
        fs::write(games.join("Alpha.exe"), "binary").unwrap();
        fs::write(games.join("Nested/Beta.EXE"), "binary").unwrap();
        fs::write(games.join("readme.txt"), "text").unwrap();
        let profile = profile(&root, write_staging_binary(&root), games);

        let scanned = scan_wine_games(
            &profile,
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(scanned.games.len(), 2);
        assert!(
            scanned
                .games
                .iter()
                .all(|game| game.game_ref.starts_with("exe:"))
        );
        assert!(
            scanned
                .games
                .iter()
                .all(|game| game.fingerprint.starts_with("sha256:"))
        );
        let (first, next) = page_wine_inventory(&scanned.games, 0, 1).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(next, Some(1));
        let (second, next) = page_wine_inventory(&scanned.games, 1, 1).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(next, None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn keeps_a_path_stable_game_reference_when_executable_content_changes() {
        let root = temporary_directory("fingerprint");
        let games = root.join("Games");
        fs::create_dir_all(&games).unwrap();
        let executable = games.join("Game.exe");
        fs::write(&executable, "first build").unwrap();
        let profile = profile(&root, write_staging_binary(&root), games);
        let initial = scan_wine_games(
            &profile,
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap()
        .games
        .pop()
        .unwrap();

        fs::write(&executable, "patched build").unwrap();
        let refreshed =
            revalidate_wine_import_candidate(&profile, &initial, &AtomicBool::new(false)).unwrap();

        assert_eq!(refreshed.game_ref, initial.game_ref);
        assert_ne!(refreshed.fingerprint, initial.fingerprint);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_preview_file_swapped_for_a_symlink_outside_its_grant() {
        use std::os::unix::fs::symlink;

        let root = temporary_directory("swap");
        let games = root.join("Games");
        let executable = games.join("Game.exe");
        let outside = root.join("Outside.exe");
        fs::create_dir_all(&games).unwrap();
        fs::write(&executable, "inside").unwrap();
        fs::write(&outside, "outside").unwrap();
        let profile = profile(&root, write_staging_binary(&root), games);
        let preview = scan_wine_games(
            &profile,
            &AtomicBool::new(false),
            ScanLimits::default(),
            |_| {},
        )
        .unwrap()
        .games
        .pop()
        .unwrap();
        fs::remove_file(&executable).unwrap();
        symlink(&outside, &executable).unwrap();

        assert_eq!(
            revalidate_wine_import_candidate(&profile, &preview, &AtomicBool::new(false)),
            Err(WineRunnerError::GameOutsideScope)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_canonical_executable_path_for_unity_data_directories() {
        let root = temporary_directory("unity-application-path");
        let game_directory = root.join("Games/Blue Prince");
        let executable = game_directory.join("BLUE PRINCE.exe");
        let data_marker = game_directory.join("BLUE PRINCE_Data/marker");
        fs::create_dir_all(data_marker.parent().unwrap()).unwrap();
        fs::write(&executable, "validated game").unwrap();
        fs::write(&data_marker, "game data").unwrap();
        let wine = write_staging_binary(&root);
        let profile = profile(&root, wine, root.join("Games"));
        let canonical_executable = fs::canonicalize(&executable).unwrap();
        let game = WineGameInventoryEntry {
            profile_id: profile.id.clone(),
            game_ref: game_reference_for(&canonical_executable),
            title: "Blue Prince".into(),
            executable_path: canonical_executable.clone(),
            fingerprint: content_fingerprint_for(&canonical_executable, &AtomicBool::new(false))
                .unwrap(),
            imported_at: None,
            compatibility: WineGameCompatibility::automatic(),
            origin_direct_game_id: None,
        };
        let intent = WineLaunchIntent::new(&profile.id, &game.game_ref).unwrap();
        let prepared =
            prepare_wine_launch(&profile, &game, &intent, &root.join("prefixes")).unwrap();

        let command = prepared.command();
        let arguments = command.get_args().collect::<Vec<_>>();
        let launch_path = PathBuf::from(arguments.last().unwrap());
        assert_eq!(launch_path, canonical_executable);
        assert_eq!(
            fs::read(
                launch_path
                    .parent()
                    .unwrap()
                    .join("BLUE PRINCE_Data/marker")
            )
            .unwrap(),
            b"game data"
        );
        assert_eq!(
            command.get_current_dir(),
            Some(launch_path.parent().unwrap())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_an_executable_swapped_after_launch_preparation() {
        use std::os::unix::fs::symlink;

        let root = temporary_directory("launch-final-validation");
        let games = root.join("Games");
        let executable = games.join("Game.exe");
        let outside = root.join("Outside.exe");
        fs::create_dir_all(&games).unwrap();
        fs::write(&executable, "validated game").unwrap();
        fs::write(&outside, "outside game").unwrap();
        let wine = write_staging_binary(&root);
        let profile = profile(&root, wine, games);
        let canonical_executable = fs::canonicalize(&executable).unwrap();
        let game = WineGameInventoryEntry {
            profile_id: profile.id.clone(),
            game_ref: game_reference_for(&canonical_executable),
            title: "Game".into(),
            executable_path: canonical_executable.clone(),
            fingerprint: content_fingerprint_for(&canonical_executable, &AtomicBool::new(false))
                .unwrap(),
            imported_at: None,
            compatibility: WineGameCompatibility::automatic(),
            origin_direct_game_id: None,
        };
        let intent = WineLaunchIntent::new(&profile.id, &game.game_ref).unwrap();
        let prepared =
            prepare_wine_launch(&profile, &game, &intent, &root.join("prefixes")).unwrap();

        fs::remove_file(&canonical_executable).unwrap();
        symlink(&outside, &canonical_executable).unwrap();

        assert!(matches!(
            prepared.spawn(),
            Err(WineRunnerError::GameNotLaunchable)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_a_game_outside_its_granted_scope() {
        let root = temporary_directory("scope");
        let games = root.join("Games");
        let outside = root.join("Outside.exe");
        fs::create_dir_all(&games).unwrap();
        fs::write(&outside, "binary").unwrap();
        let wine = write_staging_binary(&root);
        let profile = profile(&root, wine, games);
        let fingerprint = content_fingerprint_for(&outside, &AtomicBool::new(false)).unwrap();
        let game = WineGameInventoryEntry {
            profile_id: profile.id.clone(),
            game_ref: game_reference_for(&outside),
            title: "Outside".into(),
            executable_path: outside,
            fingerprint,
            imported_at: None,
            compatibility: WineGameCompatibility::automatic(),
            origin_direct_game_id: None,
        };
        let intent = WineLaunchIntent::new(&profile.id, &game.game_ref).unwrap();
        assert!(matches!(
            prepare_wine_launch(&profile, &game, &intent, &root.join("prefixes")),
            Err(WineRunnerError::GameOutsideScope)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn builds_a_tokenised_wine_command_with_only_controlled_arguments() {
        let root = temporary_directory("command");
        let games = root.join("Games");
        fs::create_dir_all(&games).unwrap();
        let executable = games.join("Game.exe");
        fs::write(&executable, "binary").unwrap();
        let wine = write_staging_binary(&root);
        let profile = profile(&root, wine.clone(), games);
        let canonical_executable = fs::canonicalize(&executable).unwrap();
        let fingerprint =
            content_fingerprint_for(&canonical_executable, &AtomicBool::new(false)).unwrap();
        let mut game = WineGameInventoryEntry {
            profile_id: profile.id.clone(),
            game_ref: game_reference_for(&canonical_executable),
            title: "Game".into(),
            executable_path: executable,
            fingerprint,
            imported_at: None,
            compatibility: WineGameCompatibility::automatic(),
            origin_direct_game_id: None,
        };
        game.compatibility.graphics.virtual_desktop = Some(WineVirtualDesktop {
            width: 1280,
            height: 720,
        });
        let intent = WineLaunchIntent::new(&profile.id, &game.game_ref).unwrap();
        let prepared =
            prepare_wine_launch(&profile, &game, &intent, &root.join("prefixes")).unwrap();
        assert_eq!(
            prepared.executable().file_name(),
            Some(OsStr::new("Game.exe"))
        );
        let command = prepared.command();
        assert_eq!(
            command.get_program(),
            fs::canonicalize(&wine).unwrap().as_os_str()
        );
        let arguments = command.get_args().collect::<Vec<_>>();
        assert_eq!(
            arguments[..2],
            [
                OsStr::new("explorer"),
                OsStr::new("/desktop=Orivo,1280x720")
            ]
        );
        assert_eq!(arguments[2], canonical_executable.as_os_str());
        assert_eq!(command.get_current_dir(), canonical_executable.parent());
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == OsStr::new("WINEPREFIX") && value.is_some())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dxvk_command_uses_only_the_fixed_host_override() {
        let executable = PathBuf::from("/Games/Windows/Blue Prince/BLUE PRINCE.exe");
        let prepared = PreparedWineLaunch {
            wine_binary: PathBuf::from(
                "/Applications/Wine Staging.app/Contents/Resources/wine/bin/wine",
            ),
            prefix: PathBuf::from(
                "/Users/orivo/Library/Application Support/Orivo/wine-prefixes/test",
            ),
            executable: executable.clone(),
            working_directory: PathBuf::from("/Games/Windows/Blue Prince"),
            executable_fingerprint: "sha256:fixture".into(),
            graphics: WineGraphicsOptions {
                backend: WineGraphicsBackend::DxvkMacos,
                virtual_desktop: None,
            },
            macos_retina_mode_enabled: None,
        };

        let command = prepared.command();
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new("/Games/Windows/Blue Prince/BLUE PRINCE.exe")]
        );
        let environments = command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            environments.get(OsStr::new("WINEDLLOVERRIDES")),
            Some(&Some(OsStr::new(DXVK_MACOS_OVERRIDE).to_owned()))
        );
        assert_eq!(
            environments.get(OsStr::new("WINEPREFIX")),
            Some(&Some(
                OsStr::new("/Users/orivo/Library/Application Support/Orivo/wine-prefixes/test")
                    .to_owned()
            ))
        );
        assert_eq!(environments.get(OsStr::new("WINEDEBUG")), Some(&None));
    }

    #[test]
    fn refuses_dxvk_launch_when_the_private_runtime_is_absent() {
        let root = temporary_directory("dxvk-missing");
        let games = root.join("Games");
        fs::create_dir_all(&games).unwrap();
        let executable = games.join("Blue Prince.exe");
        fs::write(&executable, "binary").unwrap();
        let wine = write_staging_binary(&root);
        let profile = profile(&root, wine, games);
        let canonical_executable = fs::canonicalize(&executable).unwrap();
        let mut game = WineGameInventoryEntry {
            profile_id: profile.id.clone(),
            game_ref: game_reference_for(&canonical_executable),
            title: "Blue Prince".into(),
            executable_path: canonical_executable.clone(),
            fingerprint: content_fingerprint_for(&canonical_executable, &AtomicBool::new(false))
                .unwrap(),
            imported_at: None,
            compatibility: WineGameCompatibility::automatic(),
            origin_direct_game_id: None,
        };
        game.compatibility.graphics.backend = WineGraphicsBackend::DxvkMacos;
        let intent = WineLaunchIntent::new(&profile.id, &game.game_ref).unwrap();

        assert!(matches!(
            prepare_wine_launch(&profile, &game, &intent, &root.join("prefixes")),
            Err(WineRunnerError::DxvkRuntimeUnavailable)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_dxvk_archive_that_is_not_the_allowlisted_release() {
        let root = temporary_directory("dxvk-archive");
        let archive = root.join("unexpected.tar.gz");
        fs::write(&archive, b"not a DXVK package").unwrap();

        assert!(matches!(
            load_dxvk_macos_package(&archive),
            Err(WineRunnerError::InvalidDxvkPackage)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_inside_the_private_prefix_before_dxvk_writes() {
        use std::os::unix::fs::symlink;

        let root = temporary_directory("dxvk-prefix-symlink");
        let prefix = root.join("prefix");
        let outside = root.join("outside");
        fs::create_dir(&prefix).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, prefix.join("drive_c")).unwrap();

        assert!(matches!(
            open_private_dxvk_target_directory(&prefix, "drive_c/windows/system32"),
            Err(WineRunnerError::DxvkInstallation)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pins_dxvk_writes_to_a_directory_descriptor_after_a_late_swap() {
        use std::os::unix::fs::symlink;

        let root = temporary_directory("dxvk-directory-pin");
        let prefix = root.join("prefix");
        let system32 = prefix.join("drive_c/windows/system32");
        let outside = root.join("outside");
        fs::create_dir_all(&system32).unwrap();
        fs::create_dir(&outside).unwrap();
        let directory =
            open_private_dxvk_target_directory(&prefix, "drive_c/windows/system32").unwrap();

        let retained_drive = root.join("retained-drive-c");
        fs::rename(prefix.join("drive_c"), &retained_drive).unwrap();
        symlink(&outside, prefix.join("drive_c")).unwrap();

        let mut file = create_private_dxvk_file(&directory, "pinned.dll").unwrap();
        file.write_all(b"pinned").unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert_eq!(
            fs::read(retained_drive.join("windows/system32/pinned.dll")).unwrap(),
            b"pinned"
        );
        assert!(!outside.join("windows/system32/pinned.dll").exists());
        drop(directory);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn strips_inherited_wine_and_dyld_overrides_before_launching() {
        let mut command = Command::new("wine");
        command
            .env("WINEPREFIX", "/other-app-prefix")
            .env("WINEDEBUG", "+all")
            .env("DYLD_INSERT_LIBRARIES", "/tmp/injected.dylib");
        clear_untrusted_wine_environment(&mut command);
        let overrides = command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(overrides.get(OsStr::new("WINEPREFIX")), Some(&None));
        assert_eq!(overrides.get(OsStr::new("WINEDEBUG")), Some(&None));
        assert_eq!(
            overrides.get(OsStr::new("DYLD_INSERT_LIBRARIES")),
            Some(&None)
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_managed_prefix_root() {
        use std::os::unix::fs::symlink;

        let root = temporary_directory("prefix-root");
        let target = root.join("other-prefix-root");
        let symlinked_root = root.join("prefixes");
        fs::create_dir(&target).unwrap();
        symlink(&target, &symlinked_root).unwrap();

        assert_eq!(
            create_managed_prefix(&symlinked_root, "wine-profile-test"),
            Err(WineRunnerError::InvalidProfile)
        );
        fs::remove_dir_all(root).unwrap();
    }
}
