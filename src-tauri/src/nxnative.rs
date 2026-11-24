//! nxnative: Switch games rebuilt as native Apple-silicon apps.
//!
//! [nxnative](https://github.com/seedds/nxnative) (MIT) packs a game the
//! player owns into a Mac app or an unsigned IPA. It is not an emulator and
//! Orivo ships none of it: the player's checkout does the work, and this module
//! only speaks the contract nxnative documents for front ends
//! (`docs/frontends.md` in that repository):
//!
//! * **A built Mac app says what it is.** `DMTitleID` in its `Info.plist` is the
//!   base game's title id, and `Contents/Resources/dm/launcher.json` lists the
//!   settings it supports. That is how a card is recognised as a Switch game.
//! * **A build reports itself.** `scripts/build_ipa_from_nsp.py --progress json`
//!   writes one JSON object per line and exits with a documented code, so
//!   progress is parsed rather than scraped from text.
//! * **The prerequisites are checkable.** Apple silicon, macOS 26.6, Xcode
//!   27.2 with its Metal toolchain, Homebrew's `xcodegen`, `zstd` and
//!   `glslang`, and an arm64 Python 3.12 with five modules. Each one is a check
//!   with the command that fixes it, so the player is told what is missing
//!   instead of meeting it forty minutes into a build.
//!
//! What it never does: read `prod.keys`. The build is handed the path the
//! player picked, and the file stays the script's business.
//!
//! This is the proof of concept described in `docs/nxnative.md`; the pure
//! parts are here and tested, the process and window plumbing is in
//! `nxnative_commands.rs`.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// The `Info.plist` key nxnative writes into every app it packs.
pub const TITLE_ID_KEY: &str = "DMTitleID";
/// The packing script, relative to a checkout.
pub const BUILD_SCRIPT: &str = "scripts/build_ipa_from_nsp.py";
/// Where the player's clone goes when Orivo makes it.
pub const REPOSITORY_URL: &str = "https://github.com/seedds/nxnative.git";
/// The settings list a Mac app carries.
const LAUNCHER_JSON: &str = "Contents/Resources/dm/launcher.json";
/// A settings list is a few kilobytes; anything far larger is not one.
const MAX_LAUNCHER_JSON_BYTES: u64 = 1024 * 1024;
/// The extensions a game folder holds when it needs `prod.keys`. A dump folder
/// or a `.nup` does not.
const KEYED_PACKAGE_EXTENSIONS: &[&str] = &["nsp", "xci", "nsz", "xcz"];
/// Homebrew's prefix on Apple silicon. An app started from Finder does not
/// inherit a shell's `PATH`, so the tools are looked for here explicitly.
const HOMEBREW_BIN: &str = "/opt/homebrew/bin";
/// The Python modules the packing script imports, by import name.
const PYTHON_MODULES: &[&str] = &["lz4", "capstone", "fontTools", "nsz", "zbic"];

// ---------------------------------------------------------------------------
// Recognising a built app
// ---------------------------------------------------------------------------

/// What a built Mac app says about the game in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NxnativeApp {
    /// Sixteen upper-case hexadecimal digits.
    pub title_id: String,
    pub bundle_id: Option<String>,
    pub version: Option<String>,
}

impl NxnativeApp {
    /// The line under a card's title. The same shape the Ryujinx runner uses,
    /// so a Switch game reads the same whichever way it is played.
    pub fn metadata_line(&self) -> String {
        format!("Nintendo Switch · {}", self.title_id)
    }
}

/// The nxnative identity of a `.app`, or `None` for any other app.
pub fn inspect_bundle(bundle: &Path) -> Option<NxnativeApp> {
    let info = plist::Value::from_file(bundle.join("Contents/Info.plist")).ok()?;
    let info = info.as_dictionary()?;
    let title_id = info
        .get(TITLE_ID_KEY)
        .and_then(plist::Value::as_string)
        .and_then(normalised_title_id)?;
    let text = |key: &str| {
        info.get(key)
            .and_then(plist::Value::as_string)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    Some(NxnativeApp {
        title_id,
        bundle_id: text("CFBundleIdentifier"),
        version: text("CFBundleShortVersionString"),
    })
}

/// A title id is exactly sixteen hexadecimal digits; two spellings of one id
/// would be two games, so it is kept in upper case.
pub fn normalised_title_id(value: &str) -> Option<String> {
    let value = value.trim();
    (value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_uppercase())
}

// ---------------------------------------------------------------------------
// Starting a built app, the way a console starts a game
// ---------------------------------------------------------------------------

/// The environment Orivo starts a built Mac app with, as a console starts a
/// game. No settings window: the panel draws the settings and writes the same
/// saved choices, so Play goes straight to the game. With `fill`, Orivo's
/// library (`nxnative_fill.m`), the game covers the screen on the desktop
/// Orivo is on, so going from one to the other switches no Space. `french`
/// carries the game's title when the player reads Orivo in French, for the
/// Home button's quit prompt.
pub fn console_env(fill: Option<&Path>, french: Option<&str>) -> Vec<(&'static str, String)> {
    let mut env = vec![("DM_LAUNCHER", "0".to_owned())];
    if let Some(library) = fill {
        env.push(("DM_FULLSCREEN", "1".to_owned()));
        env.push((
            "DYLD_INSERT_LIBRARIES",
            library.to_string_lossy().into_owned(),
        ));
    }
    if let Some(title) = french {
        // One line per text, in the host's order: hold, title, note, quit, cancel.
        let title = title.replace(['\n', '\r'], " ");
        env.push((
            "DM_HOME_QUIT_TEXT",
            format!(
                "Maintenez pour quitter\nQuitter {title} ?\nLa progression non sauvegardée sera perdue.\nQuitter\nAnnuler"
            ),
        ));
    }
    env
}

/// One row of `launcher.json`. Schema 1 grows only by optional fields, so
/// every field past `env` and `kind` is optional here and unknown ones are
/// ignored, as the contract asks of a reader.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherSetting {
    pub env: String,
    pub kind: String,
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub options: Vec<(String, String)>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub on: Option<String>,
    #[serde(default)]
    pub off: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub needs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct LauncherList {
    /// A list written before the field existed has none and is schema 1.
    #[serde(default = "schema_one")]
    pub schema: u32,
    #[serde(default)]
    pub game: Option<String>,
    #[serde(default)]
    pub settings: Vec<LauncherSetting>,
}

fn schema_one() -> u32 {
    1
}

/// The settings a built app offers. `None` when it carries no list (start it
/// as it is) or a list of a schema this reader cannot draw.
pub fn launcher_settings(bundle: &Path) -> Option<LauncherList> {
    let file = fs::File::open(bundle.join(LAUNCHER_JSON)).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_LAUNCHER_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_LAUNCHER_JSON_BYTES {
        return None;
    }
    parse_launcher_list(&bytes)
}

pub fn parse_launcher_list(bytes: &[u8]) -> Option<LauncherList> {
    let list: LauncherList = serde_json::from_slice(bytes).ok()?;
    // A new `kind` is schema 2: a reader cannot draw what it does not know.
    (list.schema == 1).then_some(list)
}

/// The `.app` a library card's executable lives in.
pub fn bundle_of(executable: &Path) -> Option<PathBuf> {
    executable
        .ancestors()
        .find(|ancestor| {
            ancestor
                .extension()
                .is_some_and(|extension| extension == "app")
        })
        .map(Path::to_path_buf)
}

/// A bundle id `defaults` may be handed: reverse-DNS characters only, never
/// something that reads as an option.
pub fn valid_bundle_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with(['-', '.'])
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

/// The preferences key a setting's saved choice lives under.
pub fn saved_choice_key(env: &str) -> String {
    format!("launcher.{env}")
}

/// Whether this build offers `value` for `setting`: the same check the app's
/// own launcher makes before it uses a saved choice.
pub fn accepted_value(setting: &LauncherSetting, value: &str) -> bool {
    match setting.kind.as_str() {
        "choice" => setting.options.iter().any(|(option, _)| option == value),
        "toggle" => setting.on.as_deref() == Some(value) || setting.off.as_deref() == Some(value),
        "range" => value.parse::<f64>().is_ok_and(|number| {
            number.is_finite()
                && setting.min.is_none_or(|min| number >= min - 1e-9)
                && setting.max.is_none_or(|max| number <= max + 1e-9)
        }),
        _ => false,
    }
}

/// The saved choices out of `defaults export <bundle id> -`: every
/// `launcher.<env>` string, by env.
pub fn saved_choices(preferences: &[u8]) -> std::collections::BTreeMap<String, String> {
    plist::Value::from_reader(std::io::Cursor::new(preferences))
        .ok()
        .and_then(plist::Value::into_dictionary)
        .map(|dictionary| {
            dictionary
                .into_iter()
                .filter_map(|(key, value)| {
                    let env = key.strip_prefix("launcher.")?.to_owned();
                    Some((env, value.into_string()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Prerequisites
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckState {
    Ok,
    Missing,
    /// Present, but not the version the build needs.
    Outdated,
}

/// One prerequisite, as the checklist shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: &'static str,
    pub state: CheckState,
    /// What was found, in a form a person can read: a version, a list of
    /// missing modules. Never a path under the player's home.
    pub found: Option<String>,
    /// The command that fixes it, to copy into Terminal; `None` when Orivo
    /// does it itself or when only a download page can.
    pub fix: Option<String>,
}

impl Check {
    fn new(id: &'static str, state: CheckState, found: Option<String>, fix: Option<&str>) -> Self {
        Self {
            id,
            state,
            found,
            fix: fix.map(str::to_owned),
        }
    }

    #[cfg(test)]
    pub fn is_ok(&self) -> bool {
        self.state == CheckState::Ok
    }
}

/// The machine, as far as the checklist asks about it. A trait so the checks
/// can be tested without the machine.
pub trait Probe {
    /// A program's standard output when it exits successfully.
    fn run(&self, program: &Path, args: &[&str]) -> Option<String>;
    fn is_file(&self, path: &Path) -> bool;
    fn apple_silicon(&self) -> bool;
}

/// The real machine. Every command is one process with tokenised arguments
/// and no shell, given five seconds at most by its own nature (these are
/// version queries), with Homebrew's prefix on `PATH`.
pub struct SystemProbe;

impl Probe for SystemProbe {
    fn run(&self, program: &Path, args: &[&str]) -> Option<String> {
        let output = Command::new(program)
            .args(args)
            .env("PATH", tool_path())
            .stdin(Stdio::null())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn apple_silicon(&self) -> bool {
        cfg!(all(target_os = "macos", target_arch = "aarch64"))
    }
}

/// `PATH` for the tools a build runs: Homebrew first, then the system's.
pub fn tool_path() -> String {
    format!("{HOMEBREW_BIN}:/usr/bin:/bin:/usr/sbin:/sbin")
}

/// The environment a fix runs in, inside Orivo, where no terminal can answer
/// a question: Homebrew and pip are told not to ask.
pub const FIX_ENV: &[(&str, &str)] = &[
    ("NONINTERACTIVE", "1"),
    ("HOMEBREW_NO_ENV_HINTS", "1"),
    ("HOMEBREW_NO_COLOR", "1"),
    ("PIP_NO_INPUT", "1"),
    ("PIP_DISABLE_PIP_VERSION_CHECK", "1"),
];

/// The order "Install all" walks the checklist in: each item can need the
/// one before it (the modules need Python, Python needs Homebrew).
pub const FIX_ORDER: &[&str] = &[
    "xcode",
    "metal-toolchain",
    "build-tools",
    "python",
    "python-modules",
    "checkout",
];

/// A fix as Orivo runs it: every question it might still ask is answered
/// yes, the way `-y` would, by `yes` on its standard input.
pub fn unattended(command: &str) -> String {
    format!("yes 2>/dev/null | {{ {command}\n}}")
}

/// A fix that needs an administrator, through macOS's own password sheet
/// (`osascript`), never a terminal's `sudo`.
pub fn admin_script(command: &str) -> String {
    format!(
        "do shell script \"{}\" with administrator privileges",
        command.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

/// A process's output cut into lines as it arrives. `\r` ends a line too, so
/// a progress bar redrawn in place becomes successive lines; colour codes and
/// blank lines are dropped.
#[derive(Default)]
pub struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in bytes {
            if byte == b'\n' || byte == b'\r' {
                if let Some(line) = clean_line(&std::mem::take(&mut self.pending)) {
                    lines.push(line);
                }
            } else {
                self.pending.push(byte);
            }
        }
        lines
    }

    pub fn finish(self) -> Option<String> {
        clean_line(&self.pending)
    }
}

fn clean_line(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ … final byte in @..~
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    let out = out.trim_end().to_owned();
    (!out.trim().is_empty()).then_some(out)
}

/// The Python modules fix, as run: a private environment made from the
/// system's Python, then nxnative's modules installed into it. Homebrew's
/// Python refuses a plain `pip install`, and a private one never touches it.
pub fn python_env_command(base: &Path, env: &Path) -> String {
    let python = env.join("bin/python3");
    format!(
        "{base} -m venv {env} && {python} -m pip install --upgrade pip && {python} -m pip install lz4 'capstone>=5,<6' fonttools nsz zbic 'backports.zstd; python_version < \"3.14\"'",
        base = shell_single_quote(&base.to_string_lossy()),
        env = shell_single_quote(&env.to_string_lossy()),
        python = shell_single_quote(&python.to_string_lossy()),
    )
}

fn shell_single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Where a tool is, looking in Homebrew's prefix and then the system's.
fn find_tool(probe: &dyn Probe, names: &[&str]) -> Option<PathBuf> {
    names.iter().find_map(|name| {
        [HOMEBREW_BIN, "/usr/local/bin", "/usr/bin"]
            .iter()
            .map(|directory| Path::new(directory).join(name))
            .find(|path| probe.is_file(path))
    })
}

/// `(major, minor)` out of the first `a.b` in a version string.
pub fn major_minor(text: &str) -> Option<(u32, u32)> {
    text.split(|character: char| !character.is_ascii_digit() && character != '.')
        .filter(|word| word.contains('.'))
        .find_map(|word| {
            let mut parts = word.split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next()?.parse().ok()?;
            Some((major, minor))
        })
}

/// The Python a build runs with: arm64, 3.12 or newer. Orivo's own
/// environment first (`env`, made by the Python modules fix), because
/// Homebrew's Python refuses `pip install` (PEP 668); then the newest of
/// Homebrew's and the system's, whose `/usr/bin/python3` is Xcode's 3.9.
pub fn find_python(probe: &dyn Probe, env: Option<&Path>) -> Option<(PathBuf, (u32, u32))> {
    if let Some(found) = env
        .map(|env| env.join("bin/python3"))
        .and_then(|path| python_version(probe, path))
        .filter(|(_, version)| *version >= (3, 12))
    {
        return Some(found);
    }
    [
        Path::new(HOMEBREW_BIN).join("python3"),
        PathBuf::from("/usr/local/bin/python3"),
        PathBuf::from("/usr/bin/python3"),
    ]
    .into_iter()
    .filter_map(|path| python_version(probe, path))
    .max_by_key(|(_, version)| *version)
}

/// An arm64 Python's version, or `None` when the file is missing or not one.
fn python_version(probe: &dyn Probe, path: PathBuf) -> Option<(PathBuf, (u32, u32))> {
    Some(path)
        .filter(|path| probe.is_file(path))
        .and_then(|path| {
            let answer = probe.run(
                &path,
                &[
                    "-I",
                    "-c",
                    "import sys,platform;print(sys.version_info[0],sys.version_info[1],platform.machine())",
                ],
            )?;
            let mut words = answer.split_whitespace();
            let major: u32 = words.next()?.parse().ok()?;
            let minor: u32 = words.next()?.parse().ok()?;
            let machine = words.next()?;
            (machine == "arm64").then_some((path, (major, minor)))
        })
}

/// Whether a folder is an nxnative checkout Orivo can build with.
pub fn is_checkout(probe: &dyn Probe, checkout: &Path) -> bool {
    probe.is_file(&checkout.join(BUILD_SCRIPT))
}

/// The whole checklist, in the order a player fixes it.
///
/// `python_env` is Orivo's own Python environment, which may not exist yet.
pub fn doctor(
    probe: &dyn Probe,
    os_version: Option<&str>,
    checkout: Option<&Path>,
    python_env: Option<&Path>,
) -> Vec<Check> {
    use CheckState::*;
    let mut checks = Vec::new();

    checks.push(Check::new(
        "apple-silicon",
        if probe.apple_silicon() { Ok } else { Missing },
        None,
        None,
    ));

    let macos = os_version.and_then(major_minor);
    checks.push(Check::new(
        "macos",
        match macos {
            Some(version) if version >= (26, 6) => Ok,
            Some(_) => Outdated,
            None => Missing,
        },
        os_version.map(str::to_owned),
        None,
    ));

    let xcodebuild = PathBuf::from("/usr/bin/xcodebuild");
    let xcode = probe
        .run(&xcodebuild, &["-version"])
        .and_then(|text| text.lines().next().map(str::to_owned));
    checks.push(Check::new(
        "xcode",
        match xcode.as_deref().and_then(major_minor) {
            Some((27, 2)) => Ok,
            Some(_) => Outdated,
            None => Missing,
        },
        xcode,
        Some("xcode-select -s /Applications/Xcode.app"),
    ));

    let metal = probe.run(Path::new("/usr/bin/xcrun"), &["metal", "--version"]);
    checks.push(Check::new(
        "metal-toolchain",
        if metal.is_some() { Ok } else { Missing },
        None,
        Some("xcodebuild -downloadComponent MetalToolchain"),
    ));

    let brew = find_tool(probe, &["brew"]);
    checks.push(Check::new(
        "homebrew",
        if brew.is_some() { Ok } else { Missing },
        None,
        None,
    ));

    let missing_tools: Vec<&str> = [
        ("xcodegen", &["xcodegen"][..]),
        ("zstd", &["zstd"][..]),
        ("glslang", &["glslang", "glslangValidator"][..]),
    ]
    .iter()
    .filter(|(_, names)| find_tool(probe, names).is_none())
    .map(|(formula, _)| *formula)
    .collect();
    checks.push(Check::new(
        "build-tools",
        if missing_tools.is_empty() {
            Ok
        } else {
            Missing
        },
        (!missing_tools.is_empty()).then(|| missing_tools.join(", ")),
        Some("brew install xcodegen zstd glslang"),
    ));

    let python = find_python(probe, python_env);
    checks.push(Check::new(
        "python",
        match python.as_ref().map(|(_, version)| *version) {
            Some(version) if version >= (3, 12) => Ok,
            Some(_) => Outdated,
            None => Missing,
        },
        python
            .as_ref()
            .map(|(_, (major, minor))| format!("Python {major}.{minor}")),
        Some("brew install python@3.13"),
    ));

    let missing_modules = python.as_ref().and_then(|(path, _)| {
        let probe_script = format!(
            "import importlib.util as u;print(','.join(m for m in {:?} if u.find_spec(m) is None))",
            PYTHON_MODULES
        );
        probe.run(path, &["-I", "-c", &probe_script])
    });
    checks.push(Check::new(
        "python-modules",
        match missing_modules.as_deref() {
            Some("") => Ok,
            _ => Missing,
        },
        missing_modules.filter(|missing| !missing.is_empty()),
        // What the tooltip shows; `python_env_command` is what runs.
        Some("python3 -m venv nxnative-python && pip install lz4 capstone fonttools nsz zbic"),
    ));

    let commit = checkout
        .filter(|checkout| is_checkout(probe, checkout))
        .map(|checkout| {
            probe
                .run(
                    Path::new("/usr/bin/git"),
                    &[
                        "-C",
                        &checkout.to_string_lossy(),
                        "rev-parse",
                        "--short",
                        "HEAD",
                    ],
                )
                .unwrap_or_default()
        });
    checks.push(Check::new(
        "checkout",
        if commit.is_some() { Ok } else { Missing },
        commit.filter(|commit| !commit.is_empty()),
        None,
    ));

    checks
}

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildPlatform {
    /// A Mac app, which Orivo adds to the library when it is built.
    MacApp,
    /// An unsigned IPA, left beside the game for the player to sideload.
    Ipa,
}

/// Everything a build needs, resolved by the host. Every path came from a
/// native picker or from the checklist; none came from the WebView.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    pub python: PathBuf,
    pub checkout: PathBuf,
    /// A folder holding one game's packages, a dump folder, or a `.nup`.
    pub source: PathBuf,
    pub keys: Option<PathBuf>,
    pub platform: BuildPlatform,
}

/// The packing script, as one process with one argument per value.
pub fn build_command(request: &BuildRequest) -> Command {
    let mut command = Command::new(&request.python);
    command
        .arg("-u")
        .arg(request.checkout.join(BUILD_SCRIPT))
        .arg(&request.source);
    if request.platform == BuildPlatform::MacApp {
        command.args(["--platform", "mac-app"]);
    }
    if let Some(keys) = &request.keys {
        command.arg("-k").arg(keys);
    }
    command
        .args(["--progress", "json"])
        .current_dir(&request.checkout)
        .env("PATH", tool_path())
        .env("PYTHONUNBUFFERED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Whether a game source needs `prod.keys`: a folder of NSP/XCI packages does,
/// a dump folder and a `.nup` do not.
pub fn source_needs_keys(source: &Path) -> bool {
    if source.is_file() {
        return has_extension(source, KEYED_PACKAGE_EXTENSIONS);
    }
    fs::read_dir(source).is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .take(4096)
            .any(|entry| has_extension(&entry.path(), KEYED_PACKAGE_EXTENSIONS))
    })
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// One line of `--progress json`, reduced to what the panel shows. The `game`
/// event's `out` is kept host-side: it is a path, and it is how the built app
/// reaches the library.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum BuildEvent {
    #[serde(rename_all = "camelCase")]
    Stage {
        index: u32,
        count: u32,
        name: String,
    },
    Step {
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        name: String,
        /// 0 to 0.99, or `None` when the work cannot be counted.
        fraction: Option<f64>,
        eta_seconds: Option<f64>,
        note: Option<String>,
    },
    StageEnd {
        name: String,
        ok: bool,
        summary: Option<String>,
        warn: bool,
    },
    ShadersStale {
        count: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Game {
        title_id: String,
        name: String,
        version: Option<String>,
        #[serde(skip)]
        out: Option<PathBuf>,
    },
    Say {
        message: String,
        error: bool,
    },
}

#[derive(Deserialize)]
struct RawEvent {
    ev: String,
    #[serde(default)]
    k: Option<u32>,
    #[serde(default)]
    n: Option<u32>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    step: Option<String>,
    #[serde(default)]
    frac: Option<f64>,
    #[serde(default)]
    eta: Option<f64>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    warn: Option<bool>,
    #[serde(default)]
    count: Option<u64>,
    #[serde(default)]
    title_id: Option<String>,
    #[serde(default)]
    version: Option<serde_json::Value>,
    #[serde(default)]
    out: Option<String>,
    #[serde(default)]
    msg: Option<String>,
}

/// One stdout line as an event. A line that is not one of the documented
/// events is `None`: the stream is the script's, and a reader skips what it
/// does not know rather than stopping the build over it.
pub fn parse_event(line: &str) -> Option<BuildEvent> {
    let raw: RawEvent = serde_json::from_str(line.trim()).ok()?;
    let name = || raw.name.clone().unwrap_or_default();
    // A progress line names its step, or `""` while the stage has none yet.
    let step_or_stage = || {
        raw.step
            .clone()
            .filter(|step| !step.is_empty())
            .unwrap_or_else(name)
    };
    Some(match raw.ev.as_str() {
        "stage" => BuildEvent::Stage {
            index: raw.k?,
            count: raw.n?,
            name: name(),
        },
        "step" => BuildEvent::Step {
            name: step_or_stage(),
        },
        "progress" => BuildEvent::Progress {
            name: step_or_stage(),
            fraction: raw.frac.map(|fraction| fraction.clamp(0.0, 1.0)),
            eta_seconds: raw.eta.filter(|eta| eta.is_finite() && *eta >= 0.0),
            note: raw.note.clone().filter(|note| !note.is_empty()),
        },
        "stage_end" => BuildEvent::StageEnd {
            name: name(),
            ok: raw.ok.unwrap_or(false),
            summary: raw.summary.clone().filter(|summary| !summary.is_empty()),
            warn: raw.warn.unwrap_or(false),
        },
        "shaders_stale" => BuildEvent::ShadersStale { count: raw.count },
        "game" => BuildEvent::Game {
            title_id: raw.title_id.as_deref().and_then(normalised_title_id)?,
            name: name(),
            version: raw.version.as_ref().and_then(|version| match version {
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Number(number) => Some(number.to_string()),
                _ => None,
            }),
            out: raw.out.as_deref().map(PathBuf::from),
        },
        "say" => {
            let message = raw.msg.clone().unwrap_or_default();
            match message.strip_prefix("[RED] ") {
                Some(error) => BuildEvent::Say {
                    message: error.to_owned(),
                    error: true,
                },
                None => BuildEvent::Say {
                    message,
                    error: false,
                },
            }
        }
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildOutcome {
    Finished,
    /// Stopped on an error; the last `say` says why.
    Failed,
    /// The arguments were refused before JSON mode started; stderr says why.
    InvalidArguments,
    Cancelled,
    Crashed,
}

/// The documented exit codes. `None` is a process ended by a signal, which is
/// what a cancel that had to kill looks like.
pub fn outcome(code: Option<i32>) -> BuildOutcome {
    match code {
        Some(0) => BuildOutcome::Finished,
        Some(1) => BuildOutcome::Failed,
        Some(2) => BuildOutcome::InvalidArguments,
        Some(129 | 130 | 143) | None => BuildOutcome::Cancelled,
        Some(_) => BuildOutcome::Crashed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn a_title_id_is_sixteen_hex_digits_in_upper_case() {
        assert_eq!(
            normalised_title_id("01007ef00011e000").as_deref(),
            Some("01007EF00011E000")
        );
        assert_eq!(normalised_title_id("01007EF00011E00"), None);
        assert_eq!(normalised_title_id("01007EF00011E00G"), None);
        assert_eq!(normalised_title_id(""), None);
    }

    #[test]
    fn a_console_start_skips_the_window_and_covers_the_screen_where_it_is() {
        assert_eq!(console_env(None, None), [("DM_LAUNCHER", "0".to_owned())]);
        let env = console_env(Some(Path::new("/x/fill.dylib")), Some("Zelda\nX"));
        assert!(env.contains(&("DM_FULLSCREEN", "1".to_owned())));
        assert!(env.contains(&("DYLD_INSERT_LIBRARIES", "/x/fill.dylib".to_owned())));
        let text = &env
            .iter()
            .find(|(key, _)| *key == "DM_HOME_QUIT_TEXT")
            .unwrap()
            .1;
        assert_eq!(text.lines().count(), 5);
        assert_eq!(text.lines().nth(1), Some("Quitter Zelda X ?"));
    }

    #[test]
    fn an_nxnative_app_is_recognised_by_its_title_id_and_another_app_is_not() {
        let root = scratch("inspect");
        let wonder = bundle(
            &root,
            "Super Mario Bros. Wonder",
            &[
                ("CFBundleExecutable", "nxnative"),
                ("CFBundleIdentifier", "org.nxnative.010015100b514000"),
                ("CFBundleShortVersionString", "2.0.0"),
                ("DMTitleID", "010015100b514000"),
            ],
        );
        let app = inspect_bundle(&wonder).unwrap();
        assert_eq!(app.title_id, "010015100B514000");
        assert_eq!(
            app.bundle_id.as_deref(),
            Some("org.nxnative.010015100b514000")
        );
        assert_eq!(app.version.as_deref(), Some("2.0.0"));
        assert_eq!(app.metadata_line(), "Nintendo Switch · 010015100B514000");

        let other = bundle(&root, "Celeste", &[("CFBundleExecutable", "Celeste")]);
        assert_eq!(inspect_bundle(&other), None);
        let broken = bundle(&root, "Broken", &[("DMTitleID", "not-an-id")]);
        assert_eq!(inspect_bundle(&broken), None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_documented_settings_list_reads_and_unknown_fields_are_ignored() {
        let json = br#"{"schema": 1, "game": "01007EF00011E000", "settings": [
 {"env": "DM_FPS_TARGET", "kind": "choice", "section": "Game", "label": "Frame rate", "note": "n", "default": "60",
  "options": [["30", "30 fps"], ["60", "60 fps"]], "level": "basic", "future": true},
 {"env": "DM_RENDER_SCALE", "kind": "range", "section": "Game", "label": "Render scale", "default": "1.0", "min": 0.5, "max": 1.0, "step": 0.05,
  "base": {"docked": [1600, 900], "handheld": [1280, 720]}},
 {"env": "DM_FPS_HOLD", "kind": "toggle", "section": "Performance", "label": "Hold frames when late", "default": "on", "on": "on", "off": "off"}
]}"#;
        let list = parse_launcher_list(json).unwrap();
        assert_eq!(list.schema, 1);
        assert_eq!(list.settings.len(), 3);
        assert_eq!(list.settings[0].options[1], ("60".into(), "60 fps".into()));
        assert_eq!(list.settings[1].step, Some(0.05));
        assert_eq!(list.settings[2].on.as_deref(), Some("on"));

        // Written before the field existed: schema 1.
        assert_eq!(
            parse_launcher_list(br#"{"settings": []}"#).unwrap().schema,
            1
        );
        // A new kind is schema 2, which this reader cannot draw.
        assert_eq!(
            parse_launcher_list(br#"{"schema": 2, "settings": []}"#),
            None
        );
    }

    #[test]
    fn every_documented_event_parses_and_an_unknown_line_is_skipped() {
        assert_eq!(
            parse_event(r#"{"ev":"stage","t":1,"k":2,"n":7,"name":"Shaders","step":""}"#),
            Some(BuildEvent::Stage {
                index: 2,
                count: 7,
                name: "Shaders".into()
            })
        );
        assert_eq!(
            parse_event(
                r#"{"ev":"progress","t":1,"k":2,"name":"Shaders","step":"Translating","done":10,"total":40,"unit":"files","est":false,"note":"","frac":0.25,"eta":30.5,"elapsed":10}"#
            ),
            Some(BuildEvent::Progress {
                name: "Translating".into(),
                fraction: Some(0.25),
                eta_seconds: Some(30.5),
                note: None,
            })
        );
        assert_eq!(
            parse_event(
                r#"{"ev":"progress","t":1,"k":1,"name":"Extract","step":"","frac":null,"eta":null}"#
            ),
            Some(BuildEvent::Progress {
                name: "Extract".into(),
                fraction: None,
                eta_seconds: None,
                note: None,
            })
        );
        assert_eq!(
            parse_event(
                r#"{"ev":"stage_end","t":1,"k":3,"name":"Pipelines","ok":true,"summary":"","elapsed":4,"warn":true}"#
            ),
            Some(BuildEvent::StageEnd {
                name: "Pipelines".into(),
                ok: true,
                summary: None,
                warn: true,
            })
        );
        let game = parse_event(
            r#"{"ev":"game","t":1,"title_id":"010015100b514000","name":"Super Mario Bros. Wonder","version":327680,"bundle_id":"org.nxnative.010015100b514000","out":"/Games/Wonder/Super Mario Bros. Wonder.app","icon":null}"#,
        )
        .unwrap();
        match game {
            BuildEvent::Game {
                title_id,
                version,
                out,
                ..
            } => {
                assert_eq!(title_id, "010015100B514000");
                assert_eq!(version.as_deref(), Some("327680"));
                assert_eq!(
                    out.as_deref(),
                    Some(Path::new("/Games/Wonder/Super Mario Bros. Wonder.app"))
                );
            }
            other => panic!("not a game event: {other:?}"),
        }
        assert_eq!(
            parse_event(r#"{"ev":"say","t":1,"msg":"[RED] prod.keys cannot open this game"}"#),
            Some(BuildEvent::Say {
                message: "prod.keys cannot open this game".into(),
                error: true
            })
        );
        assert_eq!(parse_event(r#"{"ev":"future","t":1}"#), None);
        assert_eq!(parse_event("Traceback (most recent call last):"), None);
    }

    #[test]
    fn the_game_events_path_never_reaches_the_page() {
        let event = parse_event(
            r#"{"ev":"game","t":1,"title_id":"010015100B514000","name":"Wonder","version":"1.0","out":"/Users/someone/Wonder.app"}"#,
        )
        .unwrap();
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("/Users/"), "{json}");
    }

    #[test]
    fn exit_codes_are_the_documented_ones() {
        assert_eq!(outcome(Some(0)), BuildOutcome::Finished);
        assert_eq!(outcome(Some(1)), BuildOutcome::Failed);
        assert_eq!(outcome(Some(2)), BuildOutcome::InvalidArguments);
        assert_eq!(outcome(Some(143)), BuildOutcome::Cancelled);
        assert_eq!(outcome(Some(130)), BuildOutcome::Cancelled);
        assert_eq!(outcome(None), BuildOutcome::Cancelled);
        assert_eq!(outcome(Some(134)), BuildOutcome::Crashed);
    }

    #[test]
    fn the_build_is_one_process_with_one_argument_per_value() {
        let request = BuildRequest {
            python: "/opt/homebrew/bin/python3".into(),
            checkout: "/nx".into(),
            source: "/Games/Wonder; rm -rf ~".into(),
            keys: Some("/keys/prod.keys".into()),
            platform: BuildPlatform::MacApp,
        };
        let command = build_command(&request);
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-u",
                "/nx/scripts/build_ipa_from_nsp.py",
                "/Games/Wonder; rm -rf ~",
                "--platform",
                "mac-app",
                "-k",
                "/keys/prod.keys",
                "--progress",
                "json"
            ]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/nx")));
    }

    #[test]
    fn a_folder_of_packages_needs_keys_and_a_dump_does_not() {
        let root = scratch("keys");
        let packages = root.join("Wonder");
        fs::create_dir_all(&packages).unwrap();
        fs::write(packages.join("Wonder [010015100B514000][v0].NSP"), b"").unwrap();
        assert!(source_needs_keys(&packages));
        let dump = root.join("Bananza");
        fs::create_dir_all(dump.join("base/exefs")).unwrap();
        assert!(!source_needs_keys(&dump));
        let nup = root.join("Bananza.nup");
        fs::write(&nup, b"").unwrap();
        assert!(!source_needs_keys(&nup));
        let _ = fs::remove_dir_all(root);
    }

    /// A machine described by a table: which files exist, and what each
    /// command prints.
    struct FakeProbe {
        files: Vec<PathBuf>,
        outputs: BTreeMap<String, String>,
        apple_silicon: bool,
    }

    impl FakeProbe {
        fn key(program: &Path, args: &[&str]) -> String {
            let first = args
                .iter()
                .find(|arg| !arg.starts_with('-') && !arg.starts_with('/'))
                .copied();
            format!("{} {}", program.display(), first.unwrap_or(""))
        }
    }

    impl Probe for FakeProbe {
        fn run(&self, program: &Path, args: &[&str]) -> Option<String> {
            if args.iter().any(|arg| arg.contains("find_spec")) {
                return self
                    .outputs
                    .get(&format!("{} modules", program.display()))
                    .cloned();
            }
            if args.iter().any(|arg| arg.contains("version_info")) {
                return self
                    .outputs
                    .get(&format!("{} version", program.display()))
                    .cloned();
            }
            self.outputs.get(&Self::key(program, args)).cloned()
        }
        fn is_file(&self, path: &Path) -> bool {
            self.files.iter().any(|file| file == path)
        }
        fn apple_silicon(&self) -> bool {
            self.apple_silicon
        }
    }

    fn state_of(checks: &[Check], id: &str) -> CheckState {
        checks.iter().find(|check| check.id == id).unwrap().state
    }

    #[test]
    fn a_ready_mac_passes_every_check() {
        let probe = FakeProbe {
            files: [
                "/opt/homebrew/bin/brew",
                "/opt/homebrew/bin/xcodegen",
                "/opt/homebrew/bin/zstd",
                "/opt/homebrew/bin/glslangValidator",
                "/opt/homebrew/bin/python3",
                "/usr/bin/python3",
                "/nx/scripts/build_ipa_from_nsp.py",
            ]
            .map(PathBuf::from)
            .to_vec(),
            outputs: [
                ("/usr/bin/xcodebuild ", "Xcode 27.2\nBuild version 27C45"),
                ("/usr/bin/xcrun metal", "Apple metal version 40000.1"),
                ("/opt/homebrew/bin/python3 version", "3 13 arm64"),
                ("/usr/bin/python3 version", "3 9 arm64"),
                ("/opt/homebrew/bin/python3 modules", ""),
                ("/usr/bin/git rev-parse", "a1b2c3d"),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
            apple_silicon: true,
        };
        let checks = doctor(&probe, Some("26.6.1"), Some(Path::new("/nx")), None);
        let failing: Vec<_> = checks.iter().filter(|check| !check.is_ok()).collect();
        assert!(failing.is_empty(), "{failing:?}");
        let python = checks.iter().find(|check| check.id == "python").unwrap();
        assert_eq!(python.found.as_deref(), Some("Python 3.13"));
        let checkout = checks.iter().find(|check| check.id == "checkout").unwrap();
        assert_eq!(checkout.found.as_deref(), Some("a1b2c3d"));
    }

    #[test]
    fn a_fresh_mac_is_told_what_is_missing() {
        let probe = FakeProbe {
            files: ["/usr/bin/python3"].map(PathBuf::from).to_vec(),
            outputs: [
                ("/usr/bin/xcodebuild ", "Xcode 26.4\nBuild version 26E1"),
                ("/usr/bin/python3 version", "3 9 arm64"),
                (
                    "/usr/bin/python3 modules",
                    "lz4,capstone,fontTools,nsz,zbic",
                ),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
            apple_silicon: true,
        };
        let checks = doctor(&probe, Some("26.4"), None, None);
        assert_eq!(state_of(&checks, "macos"), CheckState::Outdated);
        assert_eq!(state_of(&checks, "xcode"), CheckState::Outdated);
        assert_eq!(state_of(&checks, "metal-toolchain"), CheckState::Missing);
        assert_eq!(state_of(&checks, "homebrew"), CheckState::Missing);
        assert_eq!(state_of(&checks, "build-tools"), CheckState::Missing);
        assert_eq!(state_of(&checks, "python"), CheckState::Outdated);
        assert_eq!(state_of(&checks, "python-modules"), CheckState::Missing);
        assert_eq!(state_of(&checks, "checkout"), CheckState::Missing);
        let tools = checks
            .iter()
            .find(|check| check.id == "build-tools")
            .unwrap();
        assert_eq!(tools.found.as_deref(), Some("xcodegen, zstd, glslang"));
    }

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "orivo-nxnative-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn bundle(root: &Path, name: &str, info: &[(&str, &str)]) -> PathBuf {
        let bundle = root.join(format!("{name}.app"));
        fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        let mut dictionary = plist::Dictionary::new();
        for (key, value) in info {
            dictionary.insert((*key).into(), plist::Value::String((*value).into()));
        }
        plist::Value::Dictionary(dictionary)
            .to_file_xml(bundle.join("Contents/Info.plist"))
            .unwrap();
        bundle
    }

    #[test]
    fn output_is_cut_into_lines_at_newlines_and_carriage_returns() {
        let mut splitter = LineSplitter::default();
        assert_eq!(
            splitter.push(b"Downloading 10%\rDownloading 50%\r"),
            ["Downloading 10%", "Downloading 50%"]
        );
        assert_eq!(
            splitter.push(b"\x1b[34m==>\x1b[0m Pouring glslang\n\n  \nhalf"),
            ["==> Pouring glslang"]
        );
        assert_eq!(splitter.push(b" a line\n"), ["half a line"]);
        assert_eq!(splitter.push(b"tail"), Vec::<String>::new());
        assert_eq!(splitter.finish().as_deref(), Some("tail"));
    }

    #[test]
    fn an_unattended_fix_answers_yes_and_keeps_its_exit_code() {
        let run = |command: &str| {
            Command::new("/bin/zsh")
                .arg("-c")
                .arg(unattended(command))
                .stdin(Stdio::null())
                .output()
                .unwrap()
        };
        let asked = run("read answer; echo \"got $answer\"");
        assert_eq!(String::from_utf8_lossy(&asked.stdout), "got y\n");
        assert!(run("true && echo ok").status.success());
        assert_eq!(run("echo no; exit 3").status.code(), Some(3));
    }

    #[test]
    fn an_admin_fix_is_one_quoted_applescript_string() {
        assert_eq!(
            admin_script(r#"xcode-select -s "/Applications/Xcode.app""#),
            r#"do shell script "xcode-select -s \"/Applications/Xcode.app\"" with administrator privileges"#
        );
    }

    #[test]
    fn orivos_own_python_environment_is_used_once_it_exists() {
        let probe = FakeProbe {
            files: ["/opt/homebrew/bin/python3", "/env/bin/python3"]
                .map(PathBuf::from)
                .to_vec(),
            outputs: [
                ("/opt/homebrew/bin/python3 version", "3 14 arm64"),
                (
                    "/opt/homebrew/bin/python3 modules",
                    "lz4,capstone,fontTools,nsz,zbic",
                ),
                ("/env/bin/python3 version", "3 14 arm64"),
                ("/env/bin/python3 modules", ""),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
            apple_silicon: true,
        };
        let env = Path::new("/env");
        assert_eq!(
            find_python(&probe, Some(env)).map(|(path, _)| path),
            Some(PathBuf::from("/env/bin/python3"))
        );
        // Not made yet: Homebrew's, whose modules are missing.
        assert_eq!(
            find_python(&probe, Some(Path::new("/nowhere"))).map(|(path, _)| path),
            Some(PathBuf::from("/opt/homebrew/bin/python3"))
        );
        let checks = doctor(&probe, Some("26.6"), None, Some(env));
        assert_eq!(state_of(&checks, "python-modules"), CheckState::Ok);
        let checks = doctor(&probe, Some("26.6"), None, Some(Path::new("/nowhere")));
        assert_eq!(state_of(&checks, "python-modules"), CheckState::Missing);
    }

    #[test]
    fn the_python_modules_fix_makes_a_private_environment() {
        let command = python_env_command(
            Path::new("/opt/homebrew/bin/python3"),
            Path::new("/Users/a/Library/Application Support/run.evey.orivo/nxnative-python"),
        );
        assert!(command.starts_with("'/opt/homebrew/bin/python3' -m venv '/Users/a/Library/Application Support/run.evey.orivo/nxnative-python' && "));
        assert!(command.contains("'/Users/a/Library/Application Support/run.evey.orivo/nxnative-python/bin/python3' -m pip install lz4"));
    }

    #[test]
    fn a_saved_choice_has_to_be_one_this_build_offers() {
        let list = parse_launcher_list(
            br#"{"settings": [
 {"env": "DM_FPS_TARGET", "kind": "choice", "default": "60", "options": [["30", "30 fps"], ["60", "60 fps"]]},
 {"env": "DM_RENDER_SCALE", "kind": "range", "default": "1.0", "min": 0.5, "max": 1.0, "step": 0.05},
 {"env": "DM_FPS_HOLD", "kind": "toggle", "default": "on", "on": "on", "off": "off"},
 {"env": "DM_FUTURE", "kind": "colour", "default": "red"}
]}"#,
        )
        .unwrap();
        let [fps, scale, hold, future] = &list.settings[..] else {
            panic!()
        };
        assert!(accepted_value(fps, "30"));
        assert!(!accepted_value(fps, "45"));
        assert!(accepted_value(scale, "0.75"));
        assert!(!accepted_value(scale, "1.5"));
        assert!(!accepted_value(scale, "NaN"));
        assert!(accepted_value(hold, "off"));
        assert!(!accepted_value(hold, "maybe"));
        assert!(!accepted_value(future, "red"));
    }

    #[test]
    fn saved_choices_are_the_launcher_keys_of_the_apps_domain() {
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert("launcher.DM_FPS_TARGET".into(), "30".into());
        dictionary.insert("cloudSaves.folder".into(), "/somewhere".into());
        dictionary.insert("launcher.DM_BROKEN".into(), plist::Value::Integer(3.into()));
        let mut bytes = Vec::new();
        plist::Value::Dictionary(dictionary)
            .to_writer_xml(&mut bytes)
            .unwrap();
        let saved = saved_choices(&bytes);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved["DM_FPS_TARGET"], "30");
        assert!(saved_choices(b"").is_empty());
    }

    #[test]
    fn a_bundle_id_is_never_an_option() {
        assert!(valid_bundle_id("org.nxnative.010015100b514000"));
        assert!(!valid_bundle_id("-delete"));
        assert!(!valid_bundle_id("org.nxnative id"));
        assert!(!valid_bundle_id(""));
        assert_eq!(
            bundle_of(Path::new("/Games/Wonder.app/Contents/MacOS/nxnative")).as_deref(),
            Some(Path::new("/Games/Wonder.app"))
        );
        assert_eq!(bundle_of(Path::new("/Games/game.exe")), None);
    }

    #[test]
    fn an_imported_nxnative_app_becomes_a_switch_card() {
        let root = scratch("import");
        let wonder = bundle(
            &root,
            "Super Mario Bros. Wonder",
            &[
                ("CFBundleExecutable", "nxnative"),
                ("CFBundleName", "Super Mario Bros. Wonder"),
                ("DMTitleID", "010015100B514000"),
            ],
        );
        fs::write(wonder.join("Contents/MacOS/nxnative"), b"#!/bin/sh\n").unwrap();
        let game = crate::catalog::Game::from_executable(&wonder).unwrap();
        assert_eq!(game.title, "Super Mario Bros. Wonder");
        assert_eq!(
            game.metadata.as_deref(),
            Some("Nintendo Switch · 010015100B514000")
        );

        let other = bundle(&root, "Celeste", &[("CFBundleExecutable", "Celeste")]);
        fs::write(other.join("Contents/MacOS/Celeste"), b"#!/bin/sh\n").unwrap();
        assert_eq!(
            crate::catalog::Game::from_executable(&other)
                .unwrap()
                .metadata,
            None
        );
        let _ = fs::remove_dir_all(root);
    }

    /// The real pipe: a process that writes the documented stream the way the
    /// packing script does, read line by line and parsed, with its exit code.
    #[test]
    fn a_build_process_stream_is_followed_to_its_exit_code() {
        let Some(python) = [
            "/opt/homebrew/bin/python3",
            "/usr/bin/python3",
            "/usr/local/bin/python3",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file()) else {
            return;
        };
        let checkout = scratch("stream");
        fs::create_dir_all(checkout.join("scripts")).unwrap();
        fs::write(
            checkout.join(BUILD_SCRIPT),
            r#"import json, sys, time
assert sys.argv[1:] == [sys.argv[1], "--platform", "mac-app", "--progress", "json"], sys.argv
def ev(**fields):
    print(json.dumps({"t": time.time(), **fields}))
ev(ev="stage", k=1, n=2, name="Extract", step="")
ev(ev="progress", k=1, name="Extract", step="Unpacking", done=1, total=2, unit="files", est=False, note="", frac=0.5, eta=1.0, elapsed=0.1)
ev(ev="game", title_id="0100000000010000", name="Odyssey", version="1.0", bundle_id="org.nxnative.0100000000010000", out=sys.argv[1] + "/Odyssey.app", icon=None)
ev(ev="say", msg="[RED] the stand-in stops here")
sys.exit(1)
"#,
        )
        .unwrap();
        let source = checkout.join("game");
        fs::create_dir_all(&source).unwrap();
        let output = build_command(&BuildRequest {
            python,
            checkout: checkout.clone(),
            source: source.clone(),
            keys: None,
            platform: BuildPlatform::MacApp,
        })
        .output()
        .unwrap();
        let events: Vec<BuildEvent> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(parse_event)
            .collect();
        assert_eq!(
            events.len(),
            4,
            "{events:?}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(matches!(
            &events[2],
            BuildEvent::Game { out: Some(out), .. } if out == &source.join("Odyssey.app")
        ));
        assert_eq!(
            events[3],
            BuildEvent::Say {
                message: "the stand-in stops here".into(),
                error: true
            }
        );
        assert_eq!(outcome(output.status.code()), BuildOutcome::Failed);
        let _ = fs::remove_dir_all(checkout);
    }

    #[test]
    fn versions_are_read_out_of_the_tools_own_words() {
        assert_eq!(major_minor("Xcode 27.2"), Some((27, 2)));
        assert_eq!(major_minor("26.6.1"), Some((26, 6)));
        assert_eq!(major_minor("Xcode"), None);
    }
}
