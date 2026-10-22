//! Native FitGirl repack pipeline.
//!
//! FitGirl repacks are FreeArc archives (`fg-*.bin`) plus a tiny Inno Setup
//! wrapper.  The `.bin` files extract with the installer's own `unarc.dll`
//! under Wine.  Audio is stored as OGG chunks that first have to be trimmed
//! using their `.x2` sidecars, converted to FSB5 banks, concatenated and
//! patched with the HDiffPatch stream in `inner.fgpack.x5`.
//!
//! Everything privileged stays here on the host; the WASM plugin only sees
//! catalogue data.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::extract;

const HDIFF_MAGIC: &[u8] = b"HDIFF13&";
const FSB5_MAGIC: &[u8] = b"FSB5";
const FREEARC_MAGIC: &[u8] = b"ArC";
const SREP_ENTRY: &str = "\n[External compressor:srep]\nheader = 0\nunpackcmd = srep.exe -d $$arcpackedfile$$.tmp $$arcdatafile$$.tmp\n";

#[derive(Debug, thiserror::Error)]
pub enum FitGirlError {
    #[error("not a FitGirl repack: {0}")]
    NotFitGirl(String),
    #[error("missing tool {0}: {1}")]
    MissingTool(String, String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("command failed: {0}")]
    Command(String),
    #[error("timed out: {0}")]
    Timeout(String),
    #[error("cancelled")]
    Cancelled,
    #[error("invalid repack: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct FitGirlOptions {
    /// Optional directory containing `unarc.dll`, `fsb.exe`, `x5.exe`, …
    /// When `None`, the tools are extracted from `setup.exe` with innoextract.
    pub tools_dir: Option<PathBuf>,
    /// Scratch directory for tools, conversions and the disposable Wine prefix.
    pub work_dir: PathBuf,
    pub wine: PathBuf,
    pub wine_prefix: PathBuf,
    pub srep: Option<PathBuf>,
    pub helper: Option<PathBuf>,
    pub native_unarc: Option<PathBuf>,
    pub innoextract: Option<PathBuf>,
}

impl FitGirlOptions {
    /// Build options from the environment, with sensible local defaults.
    pub fn from_env(work_dir: impl Into<PathBuf>) -> Self {
        let work_dir = work_dir.into();
        // `wine_binary` only exists on macOS/Linux; Windows runs the setup
        // natively through `extract::run_setup`, so the field is unused there.
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let wine = std::env::var_os("QUIKY_WINE")
            .map(PathBuf::from)
            .unwrap_or_else(extract::wine_binary);
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let wine = std::env::var_os("QUIKY_WINE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("wine"));
        let wine_prefix = std::env::var_os("QUIKY_WINEPREFIX")
            .or_else(|| std::env::var_os("WINEPREFIX"))
            .map(PathBuf::from)
            .unwrap_or_else(|| work_dir.join("wineprefix"));
        let tools_dir = std::env::var_os("QUIKY_FITGIRL_TOOLS").map(PathBuf::from);
        let srep = std::env::var_os("QUIKY_SREP").map(PathBuf::from);
        let helper = std::env::var_os("QUIKY_UNARC_HELPER").map(PathBuf::from);
        let native_unarc = std::env::var_os("QUIKY_NATIVE_UNARC").map(PathBuf::from);
        let innoextract = std::env::var_os("QUIKY_INNOEXTRACT").map(PathBuf::from);
        Self {
            tools_dir,
            work_dir,
            wine,
            wine_prefix,
            srep,
            helper,
            native_unarc,
            innoextract,
        }
    }
}

/// Host-side tools that cannot live inside a `.orivo-plugin` package (which
/// refuses native payloads). They are compiled into the host crate instead and
/// materialised into a private scratch directory for each install.
const SREP_BYTES: &[u8] = include_bytes!("../../tools/bin/srep.exe");
const UNARC_HELPER_BYTES: &[u8] = include_bytes!("../../tools/bin/unarc_extract.exe");
#[cfg(target_os = "macos")]
const NATIVE_UNARC_BYTES: &[u8] = include_bytes!("../../tools/bin/unarc");

#[derive(Debug, Clone)]
pub struct BundledTools {
    pub srep: PathBuf,
    pub helper: PathBuf,
    pub native_unarc: Option<PathBuf>,
}

/// Write the embedded tools into `directory`, replacing stale copies.
///
/// The helper must sit next to the `unarc.dll` that `install_fitgirl` extracts
/// from `setup.exe`; passing these explicit paths into [`FitGirlOptions`] is
/// what makes a shipped Orivo build independent of the build machine's
/// `host/tools/bin` checkout.
pub fn ensure_bundled_tools(directory: &Path) -> Result<BundledTools, FitGirlError> {
    fs::create_dir_all(directory)?;
    let srep = directory.join("srep.exe");
    let helper = directory.join("unarc_extract.exe");
    fs::write(&srep, SREP_BYTES)?;
    fs::write(&helper, UNARC_HELPER_BYTES)?;

    #[cfg(target_os = "macos")]
    let native_unarc = {
        let path = directory.join("unarc");
        fs::write(&path, NATIVE_UNARC_BYTES)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        }
        Some(path)
    };
    #[cfg(not(target_os = "macos"))]
    let native_unarc = None;

    Ok(BundledTools {
        srep,
        helper,
        native_unarc,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitGirlReport {
    pub archives: usize,
    pub fgpacks: usize,
    pub resource_files: usize,
    pub installed_bytes: u64,
}

/// Return every `fg-*.bin` in a repack directory, sorted by name (`fg-01`,
/// `fg-02`, …).
pub fn find_bins(dir: &Path) -> Vec<PathBuf> {
    let mut bins = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| {
                        let lower = name.to_ascii_lowercase();
                        lower.starts_with("fg-") && lower.ends_with(".bin")
                    })
                    .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    bins.sort();
    bins
}

/// Verify the `fg-*.bin` archives against the MD5 list shipped inside the
/// repack. A missing list is not fatal (selective downloads may omit it), but
/// a listed archive that does not match is: extracting it would fail later,
/// after minutes of Wine work, with a much less useful error.
pub fn verify_bins_md5(dir: &Path) -> Result<usize, FitGirlError> {
    use md5::{Digest as _, Md5};

    let list = dir.join("MD5").join("fitgirl-bins.md5");
    let Ok(text) = fs::read_to_string(&list) else {
        return Ok(0);
    };
    let mut checked = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        let Some((digest, name)) = line.split_once(' ') else {
            continue;
        };
        if digest.len() != 32 {
            continue;
        }
        let name = name.trim().trim_start_matches('*').replace('\\', "/");
        let name = name.rsplit('/').next().unwrap_or_default();
        let path = dir.join(name);
        if !path.is_file() {
            continue;
        }
        let mut file = File::open(&path)?;
        let mut hasher = Md5::new();
        let mut buffer = vec![0_u8; 128 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(digest) {
            return Err(FitGirlError::Invalid(format!(
                "{} failed its MD5 check",
                path.display()
            )));
        }
        checked += 1;
    }
    Ok(checked)
}

/// True when `dir` contains an Inno `setup.exe` and at least one `fg-*.bin`.
pub fn is_fitgirl_repack(dir: &Path) -> bool {
    dir.is_dir() && extract::find_setup_exe(dir).is_some() && !find_bins(dir).is_empty()
}

/// Locate a FitGirl repack directly in `root`, or one directory below it.
/// Torrent clients often leave the repack inside `"Game [FitGirl Repack]"`.
pub fn find_repack_dir(root: &Path) -> Option<PathBuf> {
    if is_fitgirl_repack(root) {
        return Some(root.to_path_buf());
    }
    let mut children = fs::read_dir(root)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    children.sort();
    children.into_iter().find(|path| is_fitgirl_repack(path))
}

/// Parse the 8-byte little-endian length stored in a `.x2` sidecar.
///
/// `02.fgpack.x2` looks like `FILE 02.fgpack.x2\r\n<u64 le>`, while
/// `01.fgpack.x2` has no filename prefix.
pub fn parse_x2_len(data: &[u8]) -> Result<u64, FitGirlError> {
    let body = match data.windows(2).position(|window| window == b"\r\n") {
        Some(pos) => &data[pos + 2..],
        None => data,
    };
    if body.len() < 8 {
        return Err(FitGirlError::Invalid(
            "x2 sidecar is shorter than 8 bytes".into(),
        ));
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&body[..8]);
    let len = u64::from_le_bytes(bytes);
    if len == 0 {
        return Err(FitGirlError::Invalid("x2 sidecar has zero length".into()));
    }
    Ok(len)
}

pub fn parse_x2_file(path: &Path) -> Result<u64, FitGirlError> {
    parse_x2_len(&fs::read(path)?)
}

/// Drop the `FILE inner.fgpack.x5\r\n` container header and return the raw
/// HDiffPatch stream.
pub fn strip_hdiff_header(data: &[u8]) -> Result<&[u8], FitGirlError> {
    data.windows(HDIFF_MAGIC.len())
        .position(|window| window == HDIFF_MAGIC)
        .map(|pos| &data[pos..])
        .ok_or_else(|| FitGirlError::Invalid("no HDIFF13 stream found".into()))
}

/// Some FitGirl repacks ship a single preassembled `inner.fgpack` (Ogg data)
/// instead of trimmed chunks with `.x2` sidecars. In that case the `.x5`
/// HDiffPatch applies directly to the extracted file; there is nothing to
/// convert to FSB5 or concatenate.
fn direct_inner_fgpack(fgpacks: &[PathBuf], install_dir: &Path) -> Option<PathBuf> {
    if fgpacks.len() != 1 {
        return None;
    }
    let fgpack = &fgpacks[0];
    if !fgpack
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("inner.fgpack"))
    {
        return None;
    }
    if install_dir.join("inner.fgpack.x2").is_file() {
        return None;
    }
    if !install_dir.join("inner.fgpack.x5").is_file() {
        return None;
    }
    Some(fgpack.clone())
}

/// Ensure `arc.ini` has the external SREP definition used by FitGirl v3
/// streams (the CLS SREP plugin does not work reliably under Wine).
pub fn ensure_srep_entry(text: &str) -> String {
    if text
        .to_ascii_lowercase()
        .contains("[external compressor:srep]")
    {
        text.to_string()
    } else {
        let mut out = text.to_string();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(SREP_ENTRY);
        out
    }
}

fn to_windows_path(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    format!("Z:{}", absolute.to_string_lossy().replace('/', "\\"))
}

fn command_name(command: &Command) -> String {
    command.get_program().to_string_lossy().to_string()
}

fn run_command(
    command: &mut Command,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    log: Option<&Path>,
) -> Result<(), FitGirlError> {
    if let Some(path) = log {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = File::create(path)?;
        command.stdout(Stdio::from(file.try_clone()?));
        command.stderr(Stdio::from(file));
    } else {
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
    }

    let name = command_name(command);
    let mut child = command
        .spawn()
        .map_err(|error| FitGirlError::Command(format!("{name}: {error}")))?;
    let started = Instant::now();
    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(FitGirlError::Cancelled);
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| FitGirlError::Command(format!("{name}: {error}")))?
        {
            if status.success() {
                return Ok(());
            }
            return Err(FitGirlError::Command(format!(
                "{name} exited with {status}"
            )));
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(FitGirlError::Timeout(name));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), FitGirlError> {
    if cancel.load(Ordering::Relaxed) {
        Err(FitGirlError::Cancelled)
    } else {
        Ok(())
    }
}

fn copy_dir(source: &Path, destination: &Path) -> Result<(), FitGirlError> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn find_dir_with(root: &Path, depth: usize, wanted: &[&str]) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if wanted.contains(&name) && path.join("unarc.dll").is_file() {
            return Some(path);
        }
        if let Some(found) = find_dir_with(&path, depth - 1, wanted) {
            return Some(found);
        }
    }
    None
}

fn project_tool(name: &str) -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tools/bin")
        .join(name);
    path.is_file().then_some(path)
}

fn resolve_tool(explicit: Option<&Path>, names: &[&str]) -> Option<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
    }
    for name in names {
        if let Some(path) = project_tool(name) {
            return Some(path);
        }
        if let Some(path) = extract::which(name) {
            return Some(path);
        }
    }
    None
}

fn extract_setup_tools(
    setup: &Path,
    destination: &Path,
    innoextract: Option<&Path>,
) -> Result<PathBuf, FitGirlError> {
    let innoextract = innoextract
        .map(Path::to_path_buf)
        .or_else(|| extract::which("innoextract"))
        .ok_or_else(|| {
            FitGirlError::MissingTool(
                "innoextract".into(),
                "install innoextract or set QUIKY_INNOEXTRACT".into(),
            )
        })?;
    fs::create_dir_all(destination)?;
    let mut command = Command::new(innoextract);
    command
        .arg("-e")
        .arg("-s")
        .arg("-d")
        .arg(destination)
        .arg(setup);
    run_command(&mut command, Duration::from_secs(600), None, None)?;
    find_dir_with(destination, 4, &["tmp", "{tmp}"]).ok_or_else(|| {
        FitGirlError::Invalid(format!(
            "innoextract did not produce a tmp/ tools directory in {}",
            destination.display()
        ))
    })
}

fn prepare_tools(source: &Path, work: &Path, srep: Option<&Path>) -> Result<PathBuf, FitGirlError> {
    let tools = work.join("tools");
    if tools.exists() {
        fs::remove_dir_all(&tools)?;
    }
    copy_dir(source, &tools)?;

    // The CLS SREP plugin hangs under Wine ("Can't open read file mapping!").
    // Disable it and use the standalone SREP 3.x decoder through arc.ini.
    for name in [
        "CLS-srep.dll",
        "CLS-srep_x64.exe",
        "CLS-srep_x86.exe",
        "cls-srep.dll",
        "cls-srep_x64.exe",
        "cls-srep_x86.exe",
    ] {
        let path = tools.join(name);
        if path.exists() {
            let mut disabled = path.as_os_str().to_owned();
            disabled.push(".disabled");
            fs::rename(&path, PathBuf::from(disabled))?;
        }
    }

    let srep = srep
        .map(Path::to_path_buf)
        .filter(|path| path.is_file())
        .or_else(|| {
            let local = tools.join("srep.exe");
            local.is_file().then_some(local)
        })
        .or_else(|| project_tool("srep.exe"))
        .ok_or_else(|| {
            FitGirlError::MissingTool(
                "srep.exe".into(),
                "set QUIKY_SREP or put srep.exe in host/tools/bin".into(),
            )
        })?;
    let srep_dest = tools.join("srep.exe");
    if srep != srep_dest {
        fs::copy(&srep, &srep_dest)?;
    }

    let arc_ini = tools.join("arc.ini");
    let text = fs::read_to_string(&arc_ini).unwrap_or_default();
    fs::write(&arc_ini, ensure_srep_entry(&text))?;
    Ok(tools)
}

struct ArcIniGuard {
    path: PathBuf,
    backup: Option<Vec<u8>>,
}

impl Drop for ArcIniGuard {
    fn drop(&mut self) {
        match &self.backup {
            Some(bytes) => {
                let _ = fs::write(&self.path, bytes);
            }
            None => {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

fn prepare_prefix(options: &FitGirlOptions, tools: &Path) -> Result<ArcIniGuard, FitGirlError> {
    let drive_c = options.wine_prefix.join("drive_c");
    fs::create_dir_all(&drive_c)?;
    let arc_ini = drive_c.join("arc.ini");
    let backup = fs::read(&arc_ini).ok();
    // The setup's arc.ini is CRLF.  FreeArc under Wine can leave a trailing
    // CR on external command lines, which breaks the srep/rzs temp-file close
    // path.  The working native/host setup uses LF, so normalise it.
    let text = fs::read_to_string(tools.join("arc.ini"))?.replace("\r\n", "\n");
    fs::write(&arc_ini, text)?;

    if !options.wine_prefix.join("system.reg").exists() {
        let wineboot = options
            .wine
            .parent()
            .map(|parent| parent.join("wineboot"))
            .unwrap_or_else(|| PathBuf::from("wineboot"));
        if wineboot.is_file() {
            let mut command = Command::new(wineboot);
            command
                .env("WINEPREFIX", &options.wine_prefix)
                .env("WINEDEBUG", "-all")
                .env("WINEDLLOVERRIDES", "mscoree,mshtml=")
                .arg("-u");
            // A failed wineboot is not fatal: the first real Wine invocation
            // initialises the rest of the prefix.
            let _ = run_command(&mut command, Duration::from_secs(180), None, None);
        }
    }

    Ok(ArcIniGuard {
        path: arc_ini,
        backup,
    })
}

fn run_unarc(
    options: &FitGirlOptions,
    tools: &Path,
    helper: &Path,
    archive: &Path,
    destination: &Path,
    cancel: &AtomicBool,
    log_name: &str,
) -> Result<(), FitGirlError> {
    fs::create_dir_all(destination)?;
    let dll = tools.join("unarc.dll");
    if !dll.is_file() {
        return Err(FitGirlError::MissingTool(
            "unarc.dll".into(),
            format!("missing from {}", tools.display()),
        ));
    }
    let mut command = Command::new(&options.wine);
    command
        .env("WINEPREFIX", &options.wine_prefix)
        .env("WINEDEBUG", "-all")
        .current_dir(tools)
        .arg(to_windows_path(helper))
        .arg(to_windows_path(&dll))
        .arg(to_windows_path(archive))
        .arg(to_windows_path(destination));
    let log = options.work_dir.join(format!("{log_name}.log"));
    run_command(
        &mut command,
        Duration::from_secs(3600),
        Some(cancel),
        Some(&log),
    )
}

struct NativeUnarc {
    binary: PathBuf,
    cwd: PathBuf,
}

fn shell_quote(value: &Path) -> String {
    format!("'{}'", value.to_string_lossy().replace('\'', "'\\''"))
}

/// Prepare the optional native FreeArc/UnArc fallback.
///
/// `unarc.dll` rejects some FitGirl archives because their stored CRCs are
/// stale.  The patched native `unarc` only warns about those CRCs, so it is
/// used as a retry path.  It cannot load CLS plugins, so the DLL remains the
/// primary extractor.
fn prepare_native_unarc(
    options: &FitGirlOptions,
    tools: &Path,
) -> Result<Option<NativeUnarc>, FitGirlError> {
    #[cfg(not(unix))]
    {
        let _ = (options, tools);
        return Ok(None);
    }

    #[cfg(unix)]
    {
        let binary = options
            .native_unarc
            .clone()
            .filter(|path| path.is_file())
            .or_else(|| project_tool("unarc"));
        let Some(binary) = binary else {
            return Ok(None);
        };
        let srep = tools.join("srep.exe");
        let oggre = tools.join("oggre_dec.exe");
        if !srep.is_file() || !oggre.is_file() {
            return Ok(None);
        }

        let cwd = options.work_dir.join("native-tools");
        if cwd.exists() {
            fs::remove_dir_all(&cwd)?;
        }
        fs::create_dir_all(&cwd)?;
        let target = cwd.join("unarc");
        fs::copy(&binary, &target)?;

        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;

        let srep_wrapper = cwd.join("wine-srep");
        let oggre_wrapper = cwd.join("wine-oggre");
        let wine = shell_quote(&options.wine);
        let prefix = shell_quote(&options.wine_prefix);
        fs::write(
            &srep_wrapper,
            format!(
                "#!/bin/sh\nexport WINEPREFIX={prefix} WINEDEBUG=-all MVK_CONFIG_LOG_LEVEL=0\nexec {wine} {} \"$@\"\n",
                shell_quote(&srep)
            ),
        )?;
        fs::write(
            &oggre_wrapper,
            format!(
                "#!/bin/sh\nexport WINEPREFIX={prefix} WINEDEBUG=-all MVK_CONFIG_LOG_LEVEL=0\nexec {wine} {} \"$@\"\n",
                shell_quote(&oggre)
            ),
        )?;
        fs::set_permissions(&srep_wrapper, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&oggre_wrapper, fs::Permissions::from_mode(0o755))?;

        // Native unarc only recognises an absolute command when it is written
        // without shell quotes; it otherwise prepends "./" to the command.
        let arc_ini = format!(
            "[External compressor:srep]\nheader = 0\nunpackcmd = {} -d arcpackedfile.tmp arcdatafile.tmp\n\n[External compressor:mpzz]\nheader = 0\nunpackcmd = {} arcpackedfile.tmp arcdatafile.tmp\n",
            srep_wrapper.display(),
            oggre_wrapper.display()
        );
        fs::write(cwd.join("arc.ini"), arc_ini)?;
        Ok(Some(NativeUnarc {
            binary: target,
            cwd,
        }))
    }
}

fn run_native_unarc(
    options: &FitGirlOptions,
    native: &NativeUnarc,
    archive: &Path,
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<(), FitGirlError> {
    fs::create_dir_all(destination)?;
    let mut command = Command::new(&native.binary);
    command
        .current_dir(&native.cwd)
        .arg("x")
        .arg(format!("-dp{}", destination.display()))
        .arg("-o+")
        .arg(archive);
    run_command(
        &mut command,
        Duration::from_secs(3600),
        Some(cancel),
        Some(&options.work_dir.join("native-unarc.log")),
    )?;
    if fs::read_dir(destination)?.next().is_none() {
        return Err(FitGirlError::Invalid(format!(
            "native unarc extracted nothing from {}",
            archive.display()
        )));
    }
    Ok(())
}

fn truncate_copy(source: &Path, destination: &Path, length: u64) -> Result<(), FitGirlError> {
    let mut input = File::open(source)?;
    let mut output = File::create(destination)?;
    let mut limited = (&mut input).take(length);
    let copied = std::io::copy(&mut limited, &mut output)?;
    output.flush()?;
    if copied != length {
        return Err(FitGirlError::Invalid(format!(
            "{} is shorter than its x2 length ({} < {})",
            source.display(),
            copied,
            length
        )));
    }
    Ok(())
}

fn starts_with(path: &Path, magic: &[u8]) -> Result<bool, FitGirlError> {
    let mut file = File::open(path)?;
    let mut buffer = [0u8; 16];
    let read = file.read(&mut buffer)?;
    Ok(buffer[..read].starts_with(magic))
}

fn convert_fgpack(
    options: &FitGirlOptions,
    tools: &Path,
    fgpack: &Path,
    x2: &Path,
    output: &Path,
    cancel: &AtomicBool,
) -> Result<u64, FitGirlError> {
    let length = parse_x2_file(x2)?;
    let stem = fgpack
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("audio");
    let ogg = options.work_dir.join(format!("{stem}.ogg"));
    truncate_copy(fgpack, &ogg, length)?;

    let fsb = tools.join("fsb.exe");
    if !fsb.is_file() {
        return Err(FitGirlError::MissingTool(
            "fsb.exe".into(),
            format!("missing from {}", tools.display()),
        ));
    }
    let mut command = Command::new(&options.wine);
    command
        .env("WINEPREFIX", &options.wine_prefix)
        .env("WINEDEBUG", "-all")
        .current_dir(tools)
        .arg(to_windows_path(&fsb))
        .arg(to_windows_path(&ogg))
        .arg(to_windows_path(output));
    run_command(
        &mut command,
        Duration::from_secs(900),
        Some(cancel),
        Some(&options.work_dir.join("fsb.log")),
    )?;
    let _ = fs::remove_file(&ogg);

    if !output.is_file() || fs::metadata(output)?.len() == 0 {
        return Err(FitGirlError::Invalid(format!(
            "fsb.exe produced no data for {}",
            fgpack.display()
        )));
    }
    if !starts_with(output, FSB5_MAGIC)? {
        return Err(FitGirlError::Invalid(format!(
            "{} is not an FSB5 bank",
            output.display()
        )));
    }
    Ok(fs::metadata(output)?.len())
}

fn concat_files(inputs: &[PathBuf], output: &Path) -> Result<u64, FitGirlError> {
    let mut destination = File::create(output)?;
    let mut total = 0u64;
    for input in inputs {
        let mut source = File::open(input)?;
        total += std::io::copy(&mut source, &mut destination)?;
    }
    destination.flush()?;
    Ok(total)
}

fn apply_x5(
    options: &FitGirlOptions,
    tools: &Path,
    old: &Path,
    x5: &Path,
    output: &Path,
    cancel: &AtomicBool,
) -> Result<(), FitGirlError> {
    let data = fs::read(x5)?;
    let raw = strip_hdiff_header(&data)?;
    let diff = options.work_dir.join("inner.fgpack.hdiff");
    fs::write(&diff, raw)?;

    let x5_exe = tools.join("x5.exe");
    if !x5_exe.is_file() {
        return Err(FitGirlError::MissingTool(
            "x5.exe".into(),
            format!("missing from {}", tools.display()),
        ));
    }
    let mut command = Command::new(&options.wine);
    command
        .env("WINEPREFIX", &options.wine_prefix)
        .env("WINEDEBUG", "-all")
        .current_dir(tools)
        .arg(to_windows_path(&x5_exe))
        .arg(to_windows_path(old))
        .arg(to_windows_path(&diff))
        .arg(to_windows_path(output));
    run_command(
        &mut command,
        Duration::from_secs(1800),
        Some(cancel),
        Some(&options.work_dir.join("x5.log")),
    )?;
    if !starts_with(output, FREEARC_MAGIC)? {
        return Err(FitGirlError::Invalid(format!(
            "{} is not a FreeArc archive",
            output.display()
        )));
    }
    Ok(())
}

fn count_resources(dir: &Path) -> usize {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("resource"))
            {
                count += 1;
            }
        }
    }
    count
}

fn has_exe(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
            {
                return true;
            }
        }
    }
    false
}

fn remove_partial_audio(dir: &Path) -> Result<(), FitGirlError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let audio = name.ends_with(".fgpack") || name.ends_with(".x2") || name.ends_with(".x5");
        if audio && entry.metadata().map(|m| m.len() == 0).unwrap_or(false) {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn dir_size(path: &Path) -> Result<u64, FitGirlError> {
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total += dir_size(&entry.path())?;
        } else if metadata.is_file() {
            total += metadata.len();
        }
    }
    Ok(total)
}

fn remove_staged_audio(dir: &Path) -> Result<(), FitGirlError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with(".fgpack") || name.contains(".fgpack.x2") || name == "inner.fgpack.x5" {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// Install a FitGirl repack into `install_dir`.
///
/// The source `.bin` files stay untouched.  `progress` is called with
/// `(step, percent, message)` so the caller can surface progress without
/// coupling this module to the install state machine.
pub fn install_fitgirl(
    repack_dir: &Path,
    install_dir: &Path,
    options: &FitGirlOptions,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str, u8, String),
) -> Result<FitGirlReport, FitGirlError> {
    if !is_fitgirl_repack(repack_dir) {
        return Err(FitGirlError::NotFitGirl(repack_dir.display().to_string()));
    }
    fs::create_dir_all(&options.work_dir)?;
    fs::create_dir_all(install_dir)?;
    check_cancel(cancel)?;

    let bins = find_bins(repack_dir);
    progress("prepare", 2, format!("{} archives", bins.len()));

    let source_tools = match &options.tools_dir {
        Some(dir) => dir.clone(),
        None => {
            let setup = extract::find_setup_exe(repack_dir).ok_or_else(|| {
                FitGirlError::NotFitGirl(format!("no setup*.exe in {}", repack_dir.display()))
            })?;
            extract_setup_tools(
                &setup,
                &options.work_dir.join("setup-tools"),
                options.innoextract.as_deref(),
            )?
        }
    };
    let tools = prepare_tools(&source_tools, &options.work_dir, options.srep.as_deref())?;
    let helper = resolve_tool(
        options.helper.as_deref(),
        &["unarc_extract.exe", "unarc_extract"],
    )
    .ok_or_else(|| {
        FitGirlError::MissingTool(
            "unarc_extract.exe".into(),
            "run `make build-wine-helper` or set QUIKY_UNARC_HELPER".into(),
        )
    })?;
    // The helper must live in the tools directory, next to unarc.dll and the
    // setup's DLLs.  Wine resolves unarc.dll's dependent DLLs/plugins relative
    // to the host executable's directory; running the helper from host/tools/bin
    // makes srep fail with "can't close file" on some archives.
    let helper = {
        let destination = tools.join("unarc_extract.exe");
        if helper != destination {
            fs::copy(&helper, &destination)?;
        }
        destination
    };
    let _guard = prepare_prefix(options, &tools)?;
    let native = prepare_native_unarc(options, &tools)?;
    progress("tools", 5, tools.display().to_string());

    for (index, archive) in bins.iter().enumerate() {
        check_cancel(cancel)?;
        let name = archive
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("archive")
            .to_string();
        let percent = 5 + ((index as u32 * 45) / bins.len().max(1) as u32) as u8;
        progress("extract", percent, name.clone());
        let mut last_error = None;
        for attempt in 0..3 {
            match run_unarc(
                options,
                &tools,
                &helper,
                archive,
                install_dir,
                cancel,
                "unarc",
            ) {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(FitGirlError::Cancelled) => return Err(FitGirlError::Cancelled),
                Err(error) => {
                    last_error = Some(error);
                    if attempt < 2 {
                        // Wine sometimes reports "can't close file" when an
                        // external codec exits just after unarc closes its
                        // temporary file. A clean retry is enough.
                        remove_partial_audio(install_dir)?;
                        progress(
                            "extract",
                            percent,
                            format!("retrying {name} with unarc.dll (attempt {})", attempt + 2),
                        );
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        }
        if let Some(error) = last_error {
            let Some(native) = &native else {
                return Err(error);
            };
            progress(
                "extract",
                percent,
                format!("retrying {name} with native unarc"),
            );
            let native_out = options.work_dir.join("native-out");
            if native_out.exists() {
                fs::remove_dir_all(&native_out)?;
            }
            run_native_unarc(options, native, archive, &native_out, cancel)?;
            for entry in fs::read_dir(&native_out)? {
                let entry = entry?;
                let source = entry.path();
                let destination = install_dir.join(entry.file_name());
                if source.is_dir() {
                    copy_dir(&source, &destination)?;
                } else {
                    fs::copy(&source, &destination)?;
                }
            }
        }
    }

    let mut fgpacks = fs::read_dir(install_dir)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("fgpack"))
        })
        .collect::<Vec<_>>();
    fgpacks.sort();
    if fgpacks.is_empty() {
        return Err(FitGirlError::Invalid(
            "no .fgpack audio chunks were extracted".into(),
        ));
    }

    let inner = if let Some(direct) = direct_inner_fgpack(&fgpacks, install_dir) {
        check_cancel(cancel)?;
        progress("audio", 55, "using preassembled inner.fgpack".into());
        direct
    } else {
        let mut fsb_files = Vec::new();
        for (index, fgpack) in fgpacks.iter().enumerate() {
            check_cancel(cancel)?;
            let percent = 50 + ((index as u32 * 20) / fgpacks.len().max(1) as u32) as u8;
            progress(
                "audio",
                percent,
                fgpack
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("fgpack")
                    .to_string(),
            );
            let name = fgpack
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| FitGirlError::Invalid("non-UTF-8 fgpack name".into()))?;
            let x2 = install_dir.join(format!("{name}.x2"));
            if !x2.is_file() {
                return Err(FitGirlError::Invalid(format!(
                    "missing x2 sidecar for {}",
                    fgpack.display()
                )));
            }
            let fsb = options.work_dir.join(format!(
                "{}.fsb",
                Path::new(name)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("audio")
            ));
            convert_fgpack(options, &tools, fgpack, &x2, &fsb, cancel)?;
            fsb_files.push(fsb);
        }

        check_cancel(cancel)?;
        progress("patch", 72, "concatenating FSB5 banks".into());
        let inner = options.work_dir.join("inner.fgpack");
        let inner_size = concat_files(&fsb_files, &inner)?;
        if inner_size == 0 {
            return Err(FitGirlError::Invalid("empty inner.fgpack".into()));
        }
        for fsb in &fsb_files {
            let _ = fs::remove_file(fsb);
        }
        inner
    };

    let x5 = install_dir.join("inner.fgpack.x5");
    if !x5.is_file() {
        return Err(FitGirlError::Invalid(format!("missing {}", x5.display())));
    }
    let inner_arc = options.work_dir.join("inner.fgpack.arc");
    progress("patch", 78, "applying HDiffPatch".into());
    apply_x5(options, &tools, &inner, &x5, &inner_arc, cancel)?;

    progress("resources", 88, "extracting FSB5 resource files".into());
    let inner_out = options.work_dir.join("inner-out");
    if inner_out.exists() {
        fs::remove_dir_all(&inner_out)?;
    }
    run_unarc(
        options,
        &tools,
        &helper,
        &inner_arc,
        &inner_out,
        cancel,
        "unarc-inner",
    )?;
    let resource_files = count_resources(&inner_out);
    if resource_files == 0 {
        return Err(FitGirlError::Invalid(
            "inner archive contains no .resource files".into(),
        ));
    }
    // The inner archive stores paths relative to the game root.
    for entry in fs::read_dir(&inner_out)? {
        let entry = entry?;
        let source = entry.path();
        let destination = install_dir.join(entry.file_name());
        if source.is_dir() {
            copy_dir(&source, &destination)?;
        } else {
            fs::copy(&source, &destination)?;
        }
    }

    remove_staged_audio(install_dir)?;
    if !has_exe(install_dir) {
        return Err(FitGirlError::Invalid(
            "no game executable was extracted".into(),
        ));
    }
    if count_resources(install_dir) == 0 {
        return Err(FitGirlError::Invalid(
            "no .resource files were installed".into(),
        ));
    }
    let installed_bytes = dir_size(install_dir)?;
    progress("done", 100, "FitGirl repack installed".into());
    Ok(FitGirlReport {
        archives: bins.len(),
        fgpacks: fgpacks.len(),
        resource_files,
        installed_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_x2(dir: &Path, name: &str, header: Option<&str>, len: u64) -> PathBuf {
        let path = dir.join(name);
        let mut bytes = Vec::new();
        if let Some(header) = header {
            bytes.extend_from_slice(header.as_bytes());
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(&len.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn detects_fitgirl_layout() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("setup.exe"), b"x").unwrap();
        fs::write(dir.path().join("fg-02.bin"), b"x").unwrap();
        fs::write(dir.path().join("fg-01.bin"), b"x").unwrap();
        fs::write(dir.path().join("readme.txt"), b"x").unwrap();
        assert!(is_fitgirl_repack(dir.path()));
        let bins = find_bins(dir.path());
        assert_eq!(bins.len(), 2);
        assert!(bins[0].ends_with("fg-01.bin"));
        assert!(bins[1].ends_with("fg-02.bin"));
    }

    #[test]
    fn counts_installed_bytes_recursively() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("top.bin"), b"1234").unwrap();
        fs::create_dir_all(dir.path().join("nested")).unwrap();
        fs::write(dir.path().join("nested").join("data.bin"), b"123456").unwrap();
        assert_eq!(dir_size(dir.path()).unwrap(), 10);
    }

    #[test]
    fn rejects_non_fitgirl_layout() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("setup.exe"), b"x").unwrap();
        assert!(!is_fitgirl_repack(dir.path()));
    }

    #[test]
    fn parses_both_x2_shapes() {
        let dir = tempfile::tempdir().unwrap();
        let short = write_x2(dir.path(), "01.fgpack.x2", None, 133_853_155);
        let long = write_x2(
            dir.path(),
            "02.fgpack.x2",
            Some("FILE 02.fgpack.x2"),
            131_117_899,
        );
        assert_eq!(parse_x2_file(&short).unwrap(), 133_853_155);
        assert_eq!(parse_x2_file(&long).unwrap(), 131_117_899);
    }

    #[test]
    fn rejects_short_x2() {
        assert!(parse_x2_len(b"abc").is_err());
        assert!(parse_x2_len(&[0u8; 8]).is_err());
    }

    #[test]
    fn direct_inner_requires_x5_without_x2() {
        let dir = tempfile::tempdir().unwrap();
        let fgpack = dir.path().join("inner.fgpack");
        std::fs::write(&fgpack, b"ogg").unwrap();
        assert_eq!(direct_inner_fgpack(&[fgpack.clone()], dir.path()), None);

        std::fs::write(dir.path().join("inner.fgpack.x5"), b"HDIFF13&").unwrap();
        assert_eq!(
            direct_inner_fgpack(&[fgpack.clone()], dir.path()),
            Some(fgpack.clone())
        );

        std::fs::write(dir.path().join("inner.fgpack.x2"), b"x2").unwrap();
        assert_eq!(direct_inner_fgpack(&[fgpack], dir.path()), None);
    }

    #[test]
    fn direct_inner_ignores_other_names_and_multiple_chunks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("inner.fgpack.x5"), b"HDIFF13&").unwrap();

        let other = dir.path().join("01.fgpack");
        std::fs::write(&other, b"ogg").unwrap();
        assert_eq!(direct_inner_fgpack(&[other.clone()], dir.path()), None);

        let inner = dir.path().join("inner.fgpack");
        std::fs::write(&inner, b"ogg").unwrap();
        assert_eq!(
            direct_inner_fgpack(&[inner.clone(), other], dir.path()),
            None
        );
    }

    #[test]
    fn strips_file_header_from_hdiff() {
        let data = b"FILE inner.fgpack.x5\r\nHDIFF13&payload";
        assert_eq!(strip_hdiff_header(data).unwrap(), b"HDIFF13&payload");
        assert!(strip_hdiff_header(b"nope").is_err());
    }

    #[test]
    fn adds_srep_entry_once() {
        let once = ensure_srep_entry("[External compressor:mpzz]\n");
        assert!(once.contains("[External compressor:srep]"));
        assert_eq!(ensure_srep_entry(&once), once);
        let normalized = ensure_srep_entry("[external compressor:SREP]\n");
        assert_eq!(normalized, "[external compressor:SREP]\n");
    }

    #[test]
    fn finds_repack_one_level_down() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("Game [FitGirl Repack]");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("setup.exe"), b"x").unwrap();
        fs::write(nested.join("fg-01.bin"), b"x").unwrap();
        assert_eq!(find_repack_dir(root.path()).unwrap(), nested);
    }

    // `to_windows_path` maps a host path to Wine's `Z:` drive and is only ever
    // reached when spawning Wine — which never happens on Windows, where the
    // setup runs natively. On Windows `/tmp/a b` is not absolute, so the
    // assertion cannot hold there; gate it to the platforms that use Wine.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn windows_path_is_z_drive() {
        assert_eq!(to_windows_path(Path::new("/tmp/a b")), "Z:\\tmp\\a b");
    }

    #[test]
    fn guards_restore_arc_ini() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arc.ini");
        fs::write(&path, b"original").unwrap();
        {
            let _guard = ArcIniGuard {
                path: path.clone(),
                backup: Some(b"original".to_vec()),
            };
            fs::write(&path, b"temporary").unwrap();
        }
        assert_eq!(fs::read(&path).unwrap(), b"original");
    }

    #[test]
    fn component_cleanup_skips_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("01.fgpack"), b"x").unwrap();
        fs::write(dir.path().join("01.fgpack.x2"), b"x").unwrap();
        fs::write(dir.path().join("inner.fgpack.x5"), b"x").unwrap();
        fs::write(dir.path().join("keep.resource"), b"x").unwrap();
        remove_staged_audio(dir.path()).unwrap();
        assert!(!dir.path().join("01.fgpack").exists());
        assert!(!dir.path().join("01.fgpack.x2").exists());
        assert!(!dir.path().join("inner.fgpack.x5").exists());
        assert!(dir.path().join("keep.resource").exists());
    }

    #[test]
    fn invalid_hdiff_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x5");
        fs::write(&path, b"FILE nope\r\nnot a patch").unwrap();
        let options = FitGirlOptions::from_env(dir.path());
        let result = apply_x5(
            &options,
            dir.path(),
            &path,
            &path,
            &dir.path().join("out"),
            &AtomicBool::new(false),
        );
        assert!(matches!(result, Err(FitGirlError::Invalid(_))));
    }
}
