//! The GameStream host settings and the streamable-game feed.
//!
//! Discovery of a remote machine's games has to happen on this side of the
//! plugin sandbox: a WASI component imports no network at all, and by design
//! never will for this host. So the host asks, and writes the answer into the
//! profile's granted folder as one `.stream` placeholder per application, the
//! document `04-specification-plugin.md` reserves for exactly this moment. The
//! plugin then discovers those files like any other, and the launch path reads
//! them back through [`crate::runner_host`]'s closed `stream` mode.
//!
//! ## Why the client answers, and not Sunshine's web API
//!
//! `GET /api/apps` would give the same list, and it is what this module did
//! first. It asks for the wrong thing: Sunshine's web API authenticates with
//! the **admin account of its web interface**, so Orivo would have to hold a
//! password for a capability it does not otherwise need, store it, and accept
//! a self-signed certificate to send it.
//!
//! `moonlight list <host>` returns the same names and authenticates with the
//! **client certificate established at pairing** — the same authorisation that
//! lets the machine be streamed from at all. So there is no password to keep,
//! nothing to store in clear, no certificate decision, and nothing specific to
//! one host implementation: it is Moonlight's own protocol, so Apollo and any
//! other Sunshine fork answer it the same way. A user who can stream can list;
//! a user who cannot list could not have streamed either.
//!
//! The cost is that the client binary has to be present — which it has to be
//! anyway, since it is also what plays the game — and that an unreachable host
//! makes `list` hang rather than fail, so every call is bounded by a deadline
//! and the child is killed when it passes.
//!
//! Two boundaries are worth stating, because both are the point:
//!
//! * **The placeholder is host-authored.** The plugin never sees a URL and
//!   never sees the client; it sees files Orivo wrote. The launch re-reads and
//!   re-validates them before any string can reach an argument list.
//! * **The user's own files are untouchable.** Only files listed in the feed
//!   manifest beside them are ever rewritten or removed. A hand-made
//!   `Some Game.stream` in the same folder survives every refresh, and an app
//!   whose name collides with one yields rather than overwriting it.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The settings document, beside the other host-private configuration.
pub const SETTINGS_FILE: &str = "gamestream.json";
/// The feed's own manifest inside a profile's granted folder. Not a
/// `.stream` file, so the plugin's listing walks past it.
const MANIFEST_FILE: &str = ".orivo-gamestream.json";
const MANIFEST_VERSION: u32 = 1;
/// A response bound: the client prints one short name per line, and anything
/// larger is not an application list.
const MAX_APPS_BYTES: usize = 256 * 1024;
/// And a count bound, for the same reason.
const MAX_APPS: usize = 2_000;
/// The stem a placeholder file may have. The suffix, a dedupe infix and the
/// plugin's own 127-byte reference limit all fit beside it.
const MAX_PLACEHOLDER_STEM: usize = 100;
/// `moonlight list` does not fail on a host that is not there — it waits. The
/// deadline is what turns "asleep" into an answer instead of a hang; it is
/// generous because a cold host on a slow link is not an error.
const LIST_TIMEOUT: Duration = Duration::from_secs(20);
/// How often the deadline is checked while the client works.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameStreamSettingsDto {
    /// `astra.local`, `astra.local:47990`, `192.168.1.40`, `[fd00::1]:47990`,
    /// or the same with an `https://` / `http://` scheme. Empty means "not
    /// configured", and an import of a stream profile then skips the refresh
    /// instead of failing, so manually maintained placeholders keep working.
    ///
    /// A scheme and a port are accepted and then discarded: what the client is
    /// given is the machine, since pairing and streaming are its own protocol
    /// on its own ports. They are tolerated because a user who has been in
    /// Sunshine's web interface has that URL in their clipboard.
    pub host: String,
}

/// What Settings is told. It is the same document — there is no credential in
/// this feature to withhold, which is the whole reason the listing goes
/// through the client (see the module docs).
pub type GameStreamSettingsView = GameStreamSettingsDto;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GameStreamSettingsUpdate {
    pub host: Option<String>,
}

/// Shared, mutable settings. The same instance backs the Settings command and
/// the import that refreshes placeholders, so a saved address is live without
/// a restart.
#[derive(Debug)]
pub struct GameStreamService {
    path: PathBuf,
    inner: Mutex<GameStreamSettingsDto>,
}

impl GameStreamService {
    pub fn load(path: PathBuf) -> Self {
        let stored = if path.is_file() {
            match fs::read_to_string(&path).and_then(|encoded| {
                serde_json::from_str::<GameStreamSettingsDto>(&encoded).map_err(io::Error::other)
            }) {
                Ok(dto) => trim_all(dto),
                Err(error) => {
                    eprintln!(
                        "orivo: gamestream settings are unreadable ({error}); starting empty"
                    );
                    GameStreamSettingsDto::default()
                }
            }
        } else {
            GameStreamSettingsDto::default()
        };
        Self {
            path,
            inner: Mutex::new(stored),
        }
    }

    pub fn settings(&self) -> GameStreamSettingsDto {
        self.inner
            .lock()
            .map(|stored| stored.clone())
            .unwrap_or_default()
    }

    /// The address Moonlight itself talks to: the configured host without the
    /// scheme and without Sunshine's web port.
    ///
    /// Pairing and streaming are the machine's own protocol on its own ports,
    /// not the web API — which is why this is not the `origin` the feed is
    /// fetched on, and why a host configured as `https://astra.local:47990`
    /// still pairs against `astra.local`. Validated by the same parser as the
    /// feed, so a string that could not be fetched from cannot be paired with
    /// either.
    pub fn stream_host(&self) -> Result<String, GameStreamFeedError> {
        parse_endpoint(&self.settings().host).map(|endpoint| endpoint.stream_host)
    }

    /// Whether this client is already paired with the configured machine.
    ///
    /// Asked by listing, because listing is exactly the thing pairing
    /// authorises: a host that answers with its games has accepted this client,
    /// and one that refuses has not. That is cheaper to be right about than
    /// reading the client's own refusal, which is a sentence in the user's
    /// language rather than a fact.
    pub fn is_paired(&self, client: &Path) -> Result<bool, GameStreamFeedError> {
        let endpoint = parse_endpoint(&self.settings().host)?;
        match list_apps(client, &endpoint.stream_host) {
            Ok(_) => Ok(true),
            Err(GameStreamFeedError::NotPaired(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// What Settings reads. Nothing is withheld, because nothing here is a
    /// credential.
    pub fn view(&self) -> GameStreamSettingsView {
        self.settings()
    }

    pub fn update(
        &self,
        update: GameStreamSettingsUpdate,
    ) -> Result<GameStreamSettingsDto, String> {
        let mut next = self.settings();
        if let Some(host) = update.host {
            let host = host.trim().to_owned();
            // Validated before it is stored, so a bad address is refused where
            // the user typed it rather than a minute later, inside an import.
            if !host.is_empty() {
                parse_endpoint(&host).map_err(|error| error.to_string())?;
            }
            next.host = host;
        }
        self.save(&next)?;
        // A successful save must also land in the live copy — `settings()` reads
        // straight from the mutex, and the import that drives a refresh later on
        // the same instance has to observe a host saved from this command.
        if let Ok(mut stored) = self.inner.lock() {
            *stored = next.clone();
        }
        Ok(next)
    }

    /// Ask the client which games this machine can stream, and rewrite this
    /// folder's managed placeholders to match.
    ///
    /// `client` is the profile's own application, resolved and re-checked by
    /// the caller exactly as a launch would: it is the Moonlight binary the
    /// user chose, and it is the only thing that holds the pairing this call
    /// is authorised by. One folder per refresh; the caller decides which.
    pub fn refresh_placeholders(
        &self,
        client: &Path,
        directory: &Path,
    ) -> Result<RefreshOutcome, GameStreamFeedError> {
        let endpoint = parse_endpoint(&self.settings().host)?;
        let apps = list_apps(client, &endpoint.stream_host)?;
        apply_placeholders(directory, &endpoint.stream_host, &apps)
    }

    fn save(&self, dto: &GameStreamSettingsDto) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "Orivo could not resolve its configuration directory.".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Orivo could not save its configuration: {error}"))?;
        let sequence = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = parent.join(format!(
            ".{SETTINGS_FILE}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        let result = (|| -> Result<(), io::Error> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            serde_json::to_writer_pretty(&mut file, dto).map_err(io::Error::other)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|error| format!("Orivo could not save its configuration: {error}"))
    }
}

fn trim_all(dto: GameStreamSettingsDto) -> GameStreamSettingsDto {
    GameStreamSettingsDto {
        host: dto.host.trim().to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Every way the feed can decline. The messages are shown to the user by way
/// of the import, so each one names the remedy; nothing here carries a raw
/// path or an upstream body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameStreamFeedError {
    /// No host configured. The import treats this as "refresh nothing" — the
    /// manual mode the plugin documents still works — while a direct call
    /// gets the refusal.
    NotConfigured,
    /// The address is not a machine Orivo can name.
    HostInvalid,
    /// The profile's application could not be started at all. It is the
    /// streaming client, and it is what holds the pairing, so without it there
    /// is nothing authorised to ask.
    ClientUnavailable,
    /// The client ran and did not come back inside the deadline. On this path
    /// that means the machine did not answer: `list` waits on an absent host
    /// rather than failing.
    Unreachable(String),
    /// The client came back refusing. Overwhelmingly this is "not paired yet",
    /// because pairing is what authorises the question.
    NotPaired(String),
    /// The client answered with something that is not a list of names.
    Unreadable,
    /// The profile's folder is not there right now.
    DirectoryUnavailable,
    /// The folder is there and Orivo cannot write into it.
    WriteFailed,
}

impl std::fmt::Display for GameStreamFeedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => write!(formatter, "Add your gaming PC first."),
            Self::HostInvalid => write!(
                formatter,
                "That doesn't look like a PC name or address. Try something like my-pc.local or 192.168.1.20."
            ),
            Self::ClientUnavailable => write!(
                formatter,
                "Moonlight isn't installed anymore. Reinstall it or choose it again in Settings › Plugins › Game streaming."
            ),
            Self::Unreachable(host) => write!(
                formatter,
                "Can't reach {host}. Check it's on, on the same network, and running Sunshine or Apollo."
            ),
            Self::NotPaired(host) => write!(
                formatter,
                "{host} hasn't accepted this computer yet. Pair with it in Settings › Plugins › Game streaming."
            ),
            Self::Unreadable => write!(
                formatter,
                "Your PC answered, but Orivo couldn't read its list of games."
            ),
            Self::DirectoryUnavailable => write!(
                formatter,
                "Orivo's folder for your streamed games isn't available right now. Open Settings › Plugins › Game streaming and refresh your games."
            ),
            Self::WriteFailed => write!(
                formatter,
                "Orivo couldn't save your PC's game list. Check there is free space on this computer, then refresh your games."
            ),
        }
    }
}

impl std::error::Error for GameStreamFeedError {}

// ---------------------------------------------------------------------------
// The endpoint
// ---------------------------------------------------------------------------

/// The machine, as the client is told about it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FeedEndpoint {
    /// The host without scheme or port: the address the client connects to.
    /// Pairing and streaming are its own protocol on its own ports, so a port
    /// typed here — Sunshine's web interface, most likely — is accepted and
    /// then dropped rather than passed on.
    stream_host: String,
}

fn parse_endpoint(input: &str) -> Result<FeedEndpoint, GameStreamFeedError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(GameStreamFeedError::NotConfigured);
    }
    let (scheme, rest) = if let Some(rest) = trimmed.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        ("http", rest)
    } else if trimmed.contains("://") {
        // A scheme Orivo does not drive — `file:`, `ftp:` — is a refusal, not
        // a fallback to https.
        return Err(GameStreamFeedError::HostInvalid);
    } else {
        ("https", trimmed)
    };
    // No path, no query, no fragment, no userinfo: this is an address, and
    // every part of one that is not host or port is refused here rather than
    // encoded into a URL later.
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', ' ', '\t']) {
        return Err(GameStreamFeedError::HostInvalid);
    }

    let (host, port) = if let Some(bracketed) = rest.strip_prefix('[') {
        let Some(end) = bracketed.find(']') else {
            return Err(GameStreamFeedError::HostInvalid);
        };
        let host = format!("[{}]", &bracketed[..end]);
        let remainder = &bracketed[end + 1..];
        if remainder.is_empty() {
            (host, None)
        } else {
            let Some(port) = remainder.strip_prefix(':') else {
                return Err(GameStreamFeedError::HostInvalid);
            };
            (host, Some(parse_port(port)?))
        }
    } else if rest.contains(':') && rest.matches(':').count() == 1 {
        let (host, port) = rest.split_once(':').expect("one colon was counted");
        (host.to_owned(), Some(parse_port(port)?))
    } else if rest.contains(':') {
        // An IPv6 literal without brackets; the URL would be ambiguous.
        return Err(GameStreamFeedError::HostInvalid);
    } else {
        (rest.to_owned(), None)
    };

    if !stream_host_label_valid(&host) {
        return Err(GameStreamFeedError::HostInvalid);
    }
    // The scheme and the port were only ever tolerated, never used: what the
    // client is given is the machine.
    let _ = (scheme, port);
    Ok(FeedEndpoint { stream_host: host })
}

fn parse_port(port: &str) -> Result<u16, GameStreamFeedError> {
    port.parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or(GameStreamFeedError::HostInvalid)
}

/// A host label, as both the URL and (later) the stream argument accept it:
/// name characters, brackets for IPv6, and never a leading `-`.
fn stream_host_label_valid(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && !host.starts_with('-')
        && !host.chars().any(char::is_control)
        && host.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '.' | '-' | '_' | ':' | '[' | ']')
        })
}

// ---------------------------------------------------------------------------
// Asking the client
// ---------------------------------------------------------------------------

/// Run `<client> list <host>` and return the names it printed.
///
/// The argument list is closed and every element of it is the host's: a
/// canonical path Orivo resolved, the literal `list`, and an address that
/// passed [`stream_host_label_valid`]. There is no shell anywhere, so the
/// address is one argument whatever is in it — but it still cannot start with
/// `-`, because an argument that looks like an option is worth refusing before
/// it is passed to someone else's command-line parser.
fn list_apps(client: &Path, host: &str) -> Result<Vec<String>, GameStreamFeedError> {
    list_apps_within(client, host, LIST_TIMEOUT)
}

/// The deadline is a parameter so a test can prove the hang is bounded without
/// waiting [`LIST_TIMEOUT`] to find out.
fn list_apps_within(
    client: &Path,
    host: &str,
    timeout: Duration,
) -> Result<Vec<String>, GameStreamFeedError> {
    let mut child = Command::new(client)
        .arg("list")
        .arg(host)
        .stdin(Stdio::null())
        // The client writes its own log location to stderr and its answer to
        // stdout, so only one of the two is of any interest.
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| GameStreamFeedError::ClientUnavailable)?;

    // Read on a thread so a client that writes more than a pipe buffer cannot
    // deadlock against the deadline below — it would fill the pipe, block, and
    // never exit, which is the one case the deadline exists for.
    let stdout = child
        .stdout
        .take()
        .ok_or(GameStreamFeedError::ClientUnavailable)?;
    let reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout
            .take(MAX_APPS_BYTES as u64 + 1)
            .read_to_string(&mut text);
        text
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // The reader is deliberately *not* joined here. Killing the
                    // client does not necessarily close the pipe — anything it
                    // spawned inherited the write end and can hold it open —
                    // so joining would wait for that grandchild and put the
                    // whole deadline back where it started. The thread ends on
                    // its own when the pipe finally closes, and its answer was
                    // never going to be read.
                    return Err(GameStreamFeedError::Unreachable(host.to_owned()));
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(_) => return Err(GameStreamFeedError::ClientUnavailable),
        }
    };
    let text = reader.join().unwrap_or_default();
    if !status.success() {
        // The client distinguishes its failures only in its own log, in its own
        // language. Pairing is overwhelmingly the reason a reachable host
        // refuses, and it is the one the user can act on, so that is what the
        // message names.
        return Err(GameStreamFeedError::NotPaired(host.to_owned()));
    }
    if text.len() > MAX_APPS_BYTES {
        return Err(GameStreamFeedError::Unreadable);
    }
    parse_app_names(&text)
}

/// One application name per line. Blank lines are dropped rather than refused:
/// what matters is the list, and a stray newline is not a malformed answer.
fn parse_app_names(text: &str) -> Result<Vec<String>, GameStreamFeedError> {
    let names: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .take(MAX_APPS + 1)
        .map(str::to_owned)
        .collect();
    if names.len() > MAX_APPS {
        return Err(GameStreamFeedError::Unreadable);
    }
    Ok(names)
}

// ---------------------------------------------------------------------------
// Finding the client
// ---------------------------------------------------------------------------

/// A streaming client Orivo found on this machine.
///
/// The `id` is what crosses the IPC boundary; the path never does. That is the
/// same rule the rest of the runner surface follows, and it is load-bearing
/// here: a WebView that could hand back a path would be choosing which binary
/// the host starts. An id is only ever honoured if it matches something this
/// function found on its own, so the set of programs it can name is the set
/// Orivo already decided to look for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedClient {
    pub id: String,
    pub label: String,
    pub path: PathBuf,
}

/// What Settings is told about one: a name and a handle, never a location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedClientView {
    pub id: String,
    pub label: String,
}

impl DetectedClient {
    pub fn view(&self) -> DetectedClientView {
        DetectedClientView {
            id: self.id.clone(),
            label: self.label.clone(),
        }
    }
}

/// Where a Moonlight install actually lands, per platform.
///
/// Each entry is a pair: the file whose presence proves the install is there,
/// and the path to *store* on the profile. On macOS those differ — the proof is
/// the binary inside the bundle, but what is stored is the bundle, because
/// `catalog::resolve_executable` re-reads `Info.plist` on every launch and so
/// survives an update that renames the binary. Everywhere else they are the
/// same file.
///
/// Deliberately a fixed list rather than a `PATH` walk or a search: this
/// function's answer becomes a program the host starts, so what it can find is
/// a decision made here and reviewable here, not whatever a user's environment
/// happens to point at. The flatpak and snap entries are the exported
/// launchers, which is what those packages expect to be run.
fn client_candidates() -> Vec<(PathBuf, PathBuf)> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = home.as_ref() {
            roots.push(home.join("Applications"));
        }
        for root in roots {
            for bundle in ["Moonlight.app", "Moonlight Qt.app"] {
                let bundle = root.join(bundle);
                candidates.push((bundle.join("Contents/MacOS/Moonlight"), bundle));
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        for root in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            let Some(root) = std::env::var_os(root).map(PathBuf::from) else {
                continue;
            };
            for path in [
                root.join("Moonlight Game Streaming Project")
                    .join("Moonlight")
                    .join("Moonlight.exe"),
                root.join("Moonlight Game Streaming").join("Moonlight.exe"),
                root.join("Programs")
                    .join("Moonlight")
                    .join("Moonlight.exe"),
            ] {
                candidates.push((path.clone(), path));
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut paths: Vec<PathBuf> = [
            "/usr/bin/moonlight",
            "/usr/local/bin/moonlight",
            "/var/lib/flatpak/exports/bin/com.moonlight_stream.Moonlight",
            "/snap/bin/moonlight",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        if let Some(home) = home.as_ref() {
            paths
                .push(home.join(".local/share/flatpak/exports/bin/com.moonlight_stream.Moonlight"));
        }
        for path in paths {
            candidates.push((path.clone(), path));
        }
    }

    let _ = home;
    candidates
}

/// A stable handle for one found client: the first 16 bytes of the digest of
/// its canonical path, hex. Stable across restarts because the path is, and
/// opaque because the path is what it must not carry.
fn client_id(path: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(path.as_os_str().as_encoded_bytes());
    format!("client-{}", &format!("{:x}", digest.finalize())[..32])
}

/// Every streaming client present on this machine, in the order above and with
/// duplicates removed — two of the candidate paths can canonicalise to one
/// file, and a user should be offered it once.
pub fn detect_clients() -> Vec<DetectedClient> {
    let mut seen = BTreeSet::new();
    let mut found = Vec::new();
    for (probe, application) in client_candidates() {
        // The proof has to be a real file; the thing stored may be a bundle.
        if !probe.is_file() {
            continue;
        }
        let Ok(path) = fs::canonicalize(&application) else {
            continue;
        };
        if !seen.insert(path.clone()) {
            continue;
        }
        let label = client_label(&application);
        found.push(DetectedClient {
            id: client_id(&path),
            label,
            path,
        });
    }
    found
}

/// A name a person recognises. On macOS the binary inside a bundle is called
/// `Moonlight` either way, so the bundle is the better name; elsewhere the file
/// already is one.
fn client_label(candidate: &Path) -> String {
    #[cfg(target_os = "macos")]
    {
        if let Some(bundle) = candidate
            .ancestors()
            .find(|ancestor| {
                ancestor
                    .extension()
                    .is_some_and(|extension| extension == "app")
            })
            .and_then(Path::file_stem)
            .and_then(|stem| stem.to_str())
        {
            return bundle.to_owned();
        }
    }
    candidate
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Moonlight")
        .to_owned()
}

/// Resolve a handle the WebView sent back to the client it names, or nothing.
///
/// Detection is re-run rather than cached: an id is honoured because the file
/// is there *now*, not because it was there when the list was drawn.
pub fn client_for_id(id: &str) -> Option<PathBuf> {
    detect_clients()
        .into_iter()
        .find(|client| client.id == id)
        .map(|client| client.path)
}

// ---------------------------------------------------------------------------
// Placeholders
// ---------------------------------------------------------------------------

/// What one refresh did, for tests and for the journal around it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshOutcome {
    /// Applications the host listed.
    pub found: usize,
    /// Placeholders written or rewritten.
    pub written: usize,
    /// Managed placeholders removed because their game is gone.
    pub removed: usize,
    /// Applications left to whatever already answers to their name — a
    /// hand-made file, or a name that sanitises to nothing.
    pub skipped: usize,
}

/// The document a placeholder holds, in the order `04-specification-plugin.md`
/// fixes. This struct is the only writer of those bytes.
#[derive(Debug, Serialize)]
struct Placeholder {
    host: String,
    client: String,
    app: String,
}

/// The feed's own bookkeeping beside the placeholders: which files in this
/// folder it wrote, so a later refresh knows what it may remove and what it
/// must leave alone. Written last, after the files themselves — a crash in
/// between leaves a stray file rather than a deletable one.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FeedManifest {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    files: Vec<String>,
}

fn read_manifest(path: &Path) -> FeedManifest {
    fs::read_to_string(path)
        .ok()
        .and_then(|encoded| serde_json::from_str::<FeedManifest>(&encoded).ok())
        .unwrap_or_default()
}

/// One application name, as a placeholder filename's stem.
///
/// Path separators, the Windows-reserved characters and control characters
/// become spaces and the run collapses, so two apps that differ only in
/// punctuation do not collide silently — the dedupe below gives them distinct
/// files instead. Nothing is left that a filesystem would reject or that
/// would hide the file as a dotfile.
fn placeholder_stem(app: &str) -> Option<String> {
    let cleaned: String = app
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                ' '
            } else {
                character
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let collapsed = collapsed.trim_matches('.').trim();
    if collapsed.is_empty() {
        return None;
    }
    let mut stem: String = collapsed.chars().take(MAX_PLACEHOLDER_STEM).collect();
    stem = stem.trim_end().to_owned();
    (!stem.is_empty()).then_some(stem)
}

/// `Name.stream` for a taken `Name` becomes `Name (2).stream`, `Name (3)…`.
fn dedupe_file_name(base: &str, attempt: usize) -> String {
    let stem = base.strip_suffix(".stream").unwrap_or(base);
    format!("{stem} ({attempt}).stream")
}

fn apply_placeholders(
    directory: &Path,
    stream_host: &str,
    apps: &[String],
) -> Result<RefreshOutcome, GameStreamFeedError> {
    if !directory.is_dir() {
        return Err(GameStreamFeedError::DirectoryUnavailable);
    }
    let manifest_path = directory.join(MANIFEST_FILE);
    let previous = read_manifest(&manifest_path);
    let previous_files: BTreeSet<String> = previous.files.iter().cloned().collect();

    let mut outcome = RefreshOutcome {
        found: apps.len(),
        ..RefreshOutcome::default()
    };
    // Lowercased names already spoken for in this pass, so two apps that
    // sanitise alike get two files rather than one shared card.
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut desired: Vec<(String, &String)> = Vec::new();

    for app in apps {
        let Some(stem) = placeholder_stem(app) else {
            outcome.skipped += 1;
            continue;
        };
        let base = format!("{stem}.stream");
        let mut candidate = base.clone();
        let mut attempt = 2;
        loop {
            let key = candidate.to_lowercase();
            if used.contains(&key) {
                candidate = dedupe_file_name(&base, attempt);
                attempt += 1;
                continue;
            }
            if directory.join(&candidate).exists() && !previous_files.contains(&candidate) {
                // An answer already there that the feed did not write: a
                // hand-made file, or another profile's. It keeps its name and
                // it keeps its bytes.
                used.insert(key);
                outcome.skipped += 1;
                break;
            }
            used.insert(key);
            desired.push((candidate, app));
            break;
        }
    }

    // Remove what the feed wrote and the host no longer lists. The manifest
    // is the whole authority: a file it does not name is never touched.
    for name in &previous.files {
        if desired
            .iter()
            .any(|(file, _)| file.eq_ignore_ascii_case(name))
        {
            continue;
        }
        let path = directory.join(name);
        if path.is_file() && fs::remove_file(&path).is_ok() {
            outcome.removed += 1;
        }
    }

    for (file, app) in &desired {
        let placeholder = Placeholder {
            host: stream_host.to_owned(),
            client: "moonlight".into(),
            app: (*app).clone(),
        };
        let encoded =
            serde_json::to_vec(&placeholder).map_err(|_| GameStreamFeedError::WriteFailed)?;
        let path = directory.join(file);
        // A file the host still lists with the bytes it already has is left in
        // place: rewriting an identical payload is not a write worth recording,
        // and it is the only case `written` stays silent in a second refresh.
        let existing = fs::read(&path).ok();
        if existing.as_deref() != Some(encoded.as_slice()) {
            write_atomic(&path, &encoded)?;
            outcome.written += 1;
        }
    }

    let mut manifest = FeedManifest {
        version: MANIFEST_VERSION,
        files: desired.into_iter().map(|(file, _)| file).collect(),
    };
    manifest.files.sort();
    let encoded = serde_json::to_vec(&manifest).map_err(|_| GameStreamFeedError::WriteFailed)?;
    write_atomic(&manifest_path, &encoded)?;
    Ok(outcome)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), GameStreamFeedError> {
    let parent = path.parent().ok_or(GameStreamFeedError::WriteFailed)?;
    let sequence = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(
        ".orivo-stream.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    let result = (|| -> Result<(), io::Error> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|_| GameStreamFeedError::WriteFailed)
}

// ---------------------------------------------------------------------------
// Is the machine there, and does it know this client?
// ---------------------------------------------------------------------------

/// The ports a Sunshine or Apollo host answers a client on, whatever its web
/// interface does: GameStream's HTTP and HTTPS endpoints. Pairing, listing and
/// streaming all start with a request to one of them, so a machine that
/// accepts a connection on neither cannot be paired with or streamed from.
const HOST_PROBE_PORTS: [u16; 2] = [47989, 47984];
/// How long one connection attempt may take. Short on purpose: a PC that is
/// switched off should read as "no answer" in a moment, not after the twenty
/// seconds `moonlight list` would wait for it.
const REACH_TIMEOUT: Duration = Duration::from_secs(3);
/// `list` once the machine is known to be there. A paired host answers in well
/// under a second; this only has to outlast a cold one.
pub(crate) const PROBE_LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// Reduce whatever a person pasted into the box to something that can be a
/// host: no scheme, no path, no query, no credentials. The port is left for
/// [`parse_endpoint`] to drop.
///
/// People copy this out of Sunshine's own web address — what is in their
/// browser's bar is `https://my-pc.local:47990/pin` — so explaining ports and
/// paths to them would be asking them to do this function's work.
pub fn tidy_host_input(input: &str) -> String {
    let mut text = input.trim();
    for scheme in ["https://", "http://"] {
        if text.len() >= scheme.len()
            && text.is_char_boundary(scheme.len())
            && text[..scheme.len()].eq_ignore_ascii_case(scheme)
        {
            text = &text[scheme.len()..];
            break;
        }
    }
    if let Some(end) = text.find(['/', '?', '#']) {
        text = &text[..end];
    }
    if let Some(at) = text.rfind('@') {
        text = &text[at + 1..];
    }
    text.trim().to_owned()
}

/// The machine a pasted address names — what the client is given, and what is
/// saved. The same grammar as the feed's, applied after [`tidy_host_input`].
pub fn normalise_host(input: &str) -> Result<String, GameStreamFeedError> {
    parse_endpoint(&tidy_host_input(input)).map(|endpoint| endpoint.stream_host)
}

/// What a machine said when asked, in the four words the setup needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HostState {
    /// It answered the client's `list`: this client is accepted.
    Paired,
    /// It is there and refused the question. Overwhelmingly: not paired yet.
    NotPaired,
    /// Nothing answered on the machine's ports, or `list` never came back.
    Unreachable,
    /// The client itself could not be started, so nothing was asked.
    ClientMissing,
}

/// What Settings is told about a probe: the state, the machine as it was
/// understood, and how many games it listed if it listed any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatusView {
    pub state: HostState,
    pub host: String,
    pub games: usize,
}

/// Decide the state from the two facts that were gathered. Pure, so that what
/// each outcome means is stated once and tested without a network.
///
/// Reachability is asked separately from `list` because `list` cannot tell the
/// two failures apart: a machine that is off makes it wait and a machine that
/// has not paired this client makes it exit non-zero, and the user has to be
/// told which, since one needs a PIN and the other a power button.
pub fn host_state(reachable: bool, listing: &Result<Vec<String>, GameStreamFeedError>) -> HostState {
    if !reachable {
        return HostState::Unreachable;
    }
    match listing {
        // An answer that could not be read was still an answer: the machine
        // accepted this client, and the import says what it could not parse.
        Ok(_) | Err(GameStreamFeedError::Unreadable) => HostState::Paired,
        Err(GameStreamFeedError::ClientUnavailable) => HostState::ClientMissing,
        // The machine accepted a connection and then `list` did not come back:
        // from the user's chair that is a machine that is not answering.
        Err(GameStreamFeedError::Unreachable(_)) => HostState::Unreachable,
        Err(_) => HostState::NotPaired,
    }
}

/// Whether anything on `host` accepts a connection on one of `ports`.
///
/// A refused connection counts as "no": a machine that is on but has nothing
/// listening is a machine whose Sunshine or Apollo is not running, which is
/// the thing the user is told to check.
fn host_reachable(host: &str, ports: &[u16], timeout: Duration) -> bool {
    let name = host.trim_start_matches('[').trim_end_matches(']');
    for port in ports {
        let Ok(addresses) = (name, *port).to_socket_addrs() else {
            continue;
        };
        for address in addresses {
            if TcpStream::connect_timeout(&address, timeout).is_ok() {
                return true;
            }
        }
    }
    false
}

/// [`host_reachable`] on the ports a real host listens on.
pub fn host_is_reachable(host: &str) -> bool {
    host_reachable(host, &HOST_PROBE_PORTS, REACH_TIMEOUT)
}

/// Ask a machine whether it is there and whether it knows this client.
///
/// `reachable` is a parameter so a test can stand a fake machine up without
/// opening the real ports, and so the pairing poll can reuse the reading.
pub(crate) fn probe_host_with(
    client: &Path,
    host: &str,
    reachable: impl FnOnce(&str) -> bool,
    list_timeout: Duration,
) -> HostStatusView {
    let reached = reachable(host);
    // Not asked at all when nothing answered: `list` would only wait.
    let listing = if reached {
        list_apps_within(client, host, list_timeout)
    } else {
        Err(GameStreamFeedError::Unreachable(host.to_owned()))
    };
    HostStatusView {
        state: host_state(reached, &listing),
        host: host.to_owned(),
        games: listing.as_ref().map(Vec::len).unwrap_or_default(),
    }
}

/// [`probe_host_with`] against the real network.
pub fn probe_host(client: &Path, host: &str) -> HostStatusView {
    probe_host_with(client, host, host_is_reachable, PROBE_LIST_TIMEOUT)
}

/// The page on a Sunshine or Apollo host where the PIN is typed. Both serve
/// their web interface on 47990, with a certificate they made themselves —
/// which is why the setup says the browser will warn.
pub fn pin_page_url(stream_host: &str) -> String {
    format!("https://{stream_host}:47990/pin")
}

/// Where Moonlight is downloaded from. A fixed address: the setup opens it for a
/// person who has no client, and nothing the WebView says becomes part of it.
pub const MOONLIGHT_DOWNLOAD_URL: &str = "https://moonlight-stream.org";

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// The address behind one of the two links the setup offers. A closed set of
/// names rather than a URL: the WebView says which page it wants and Orivo
/// builds the address, so nothing it sends can become somewhere to browse to.
pub fn link_for(target: &str, configured_host: &str) -> Result<String, String> {
    match target {
        "moonlight" => Ok(MOONLIGHT_DOWNLOAD_URL.to_owned()),
        "sunshine-pin" => normalise_host(configured_host)
            .map(|host| pin_page_url(&host))
            .map_err(|error| error.to_string()),
        _ => Err("Orivo doesn't know that link.".to_owned()),
    }
}

/// Open Moonlight's download page, or the PIN page of the saved PC, in the
/// user's browser.
#[tauri::command]
pub fn open_gamestream_link(
    target: String,
    service: tauri::State<'_, std::sync::Arc<GameStreamService>>,
) -> Result<(), String> {
    use crate::store::UrlOpener;
    let url = link_for(&target, &service.settings().host)?;
    crate::store::SystemUrlOpener.open(&url)
}

#[tauri::command]
pub fn get_gamestream_settings(
    service: tauri::State<'_, std::sync::Arc<GameStreamService>>,
) -> Result<GameStreamSettingsView, String> {
    Ok(service.view())
}

#[tauri::command]
pub fn update_gamestream_settings(
    update: GameStreamSettingsUpdate,
    service: tauri::State<'_, std::sync::Arc<GameStreamService>>,
) -> Result<GameStreamSettingsView, String> {
    service.update(update)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "orivo-gamestream-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn service(&self) -> GameStreamService {
            GameStreamService::load(self.path.join(SETTINGS_FILE))
        }

        fn folder(&self) -> PathBuf {
            let folder = self.path.join("streams");
            fs::create_dir_all(&folder).unwrap();
            folder
        }

        /// A stand-in for the streaming client: a script that writes `stdout`
        /// on its standard output, the way `moonlight list` does, and exits
        /// with `code`. It also records the arguments it was given, so a test
        /// can assert the closed argument list reached it.
        fn client(&self, name: &str, stdout: &str, code: i32) -> PathBuf {
            let path = self.path.join(name);
            let log = self.path.join(format!("{name}.args"));
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n# The real client writes its log location here, and it is not the answer.\necho 'Redirecting log output' >&2\nprintf '%s' '{}'\nexit {code}\n",
                log.display(),
                stdout.replace('\'', "'\\''"),
            );
            fs::write(&path, script).unwrap();
            make_executable(&path);
            path
        }

        fn client_args(&self, name: &str) -> Vec<String> {
            fs::read_to_string(self.path.join(format!("{name}.args")))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        /// A client that never answers, for the deadline.
        fn sleeping_client(&self, name: &str) -> PathBuf {
            let path = self.path.join(name);
            fs::write(&path, "#!/bin/sh\nsleep 30\n").unwrap();
            make_executable(&path);
            path
        }
    }

    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(not(unix))]
        let _ = path;
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn update_host(service: &GameStreamService, host: &str) {
        service
            .update(GameStreamSettingsUpdate {
                host: Some(host.to_owned()),
            })
            .unwrap();
    }

    // -- settings ----------------------------------------------------------

    #[test]
    fn settings_start_empty_and_survive_a_reload() {
        let root = TestRoot::new("settings");
        assert_eq!(root.service().settings(), GameStreamSettingsDto::default());
        let service = root.service();
        let saved = service
            .update(GameStreamSettingsUpdate {
                host: Some("  astra.local  ".into()),
            })
            .unwrap();
        assert_eq!(saved.host, "astra.local");
        assert_eq!(root.service().settings(), saved);
    }

    /// There is no credential in this feature to leak — the listing is
    /// authorised by the client's pairing, not by a password — so the whole
    /// document is one field. Asserted on the bytes, so growing it back into a
    /// credential store has to be a deliberate change to this test too.
    #[test]
    fn the_settings_document_holds_an_address_and_nothing_else() {
        let root = TestRoot::new("no-credential");
        let service = root.service();
        update_host(&service, "astra.local");
        assert_eq!(
            serde_json::to_string(&service.view()).unwrap(),
            r#"{"host":"astra.local"}"#
        );
    }

    #[test]
    fn an_unreadable_settings_file_degrades_to_empty() {
        let root = TestRoot::new("corrupt");
        fs::write(root.path.join(SETTINGS_FILE), b"not json").unwrap();
        assert_eq!(root.service().settings(), GameStreamSettingsDto::default());
    }

    /// A settings file written by the version that kept credentials still
    /// loads: the extra fields are dropped rather than failing the read, which
    /// is what keeps an upgrade from looking like a lost configuration.
    #[test]
    fn a_settings_file_from_the_credentialled_version_still_loads() {
        let root = TestRoot::new("legacy");
        fs::write(
            root.path.join(SETTINGS_FILE),
            br#"{"host":"astra.local","username":"admin","password":"pw"}"#,
        )
        .unwrap();
        assert_eq!(root.service().settings().host, "astra.local");
    }

    #[test]
    fn a_bad_address_is_refused_before_it_is_stored() {
        let root = TestRoot::new("bad-host");
        let service = root.service();
        let error = service
            .update(GameStreamSettingsUpdate {
                host: Some("a host with spaces".into()),
            })
            .expect_err("spaces are not a host");
        assert!(error.contains("my-pc.local"), "{error}");
        assert_eq!(service.settings().host, "");
    }

    // -- endpoint ----------------------------------------------------------

    /// A scheme and a port are tolerated and then dropped: what the client is
    /// given is the machine, because pairing and streaming are its own
    /// protocol on its own ports. Pasting Sunshine's web address works.
    #[test]
    fn an_address_reduces_to_the_machine() {
        for (input, expected) in [
            ("astra.local", "astra.local"),
            ("astra.local:47990", "astra.local"),
            ("https://astra.local:47990", "astra.local"),
            ("http://127.0.0.1:8080", "127.0.0.1"),
            ("[fd00::1]:47990", "[fd00::1]"),
        ] {
            assert_eq!(
                parse_endpoint(input).unwrap().stream_host,
                expected,
                "{input}"
            );
        }
    }

    #[test]
    fn addresses_that_are_not_hosts_are_refused() {
        for bad in [
            "",
            "file:///etc/passwd",
            "astra.local/api",
            "astra.local?x=1",
            "user@astra.local",
            "a b",
            "fd00::1",
            "-astra.local",
            "astra.local:",
            "[fd00::1",
        ] {
            let expected = if bad.is_empty() {
                GameStreamFeedError::NotConfigured
            } else {
                GameStreamFeedError::HostInvalid
            };
            assert_eq!(parse_endpoint(bad).unwrap_err(), expected, "{bad:?}");
        }
    }

    // -- parsing -----------------------------------------------------------

    #[test]
    fn the_clients_answer_is_one_name_per_line() {
        assert_eq!(
            parse_app_names("A Way Out\nDesktop\n\n  Nine Sols  \n").unwrap(),
            vec![
                "A Way Out".to_owned(),
                "Desktop".to_owned(),
                "Nine Sols".to_owned()
            ]
        );
        assert_eq!(parse_app_names("").unwrap(), Vec::<String>::new());
        let flood = "x\n".repeat(MAX_APPS + 1);
        assert_eq!(
            parse_app_names(&flood).unwrap_err(),
            GameStreamFeedError::Unreadable
        );
    }

    #[test]
    fn placeholder_stems_are_filename_shaped() {
        assert_eq!(placeholder_stem("Modulus"), Some("Modulus".into()));
        assert_eq!(placeholder_stem("A/B: C"), Some("A B C".into()));
        assert_eq!(placeholder_stem("..."), None);
        assert_eq!(placeholder_stem("   "), None);
        let long = "x".repeat(300);
        assert_eq!(
            placeholder_stem(&long).map(|stem| stem.len()),
            Some(MAX_PLACEHOLDER_STEM)
        );
    }

    // -- asking the client -------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn the_client_is_asked_with_a_closed_argument_list() {
        let root = TestRoot::new("args");
        let client = root.client("client", "Modulus\n", 0);
        assert_eq!(
            list_apps(&client, "astra.local").unwrap(),
            vec!["Modulus".to_owned()]
        );
        // `list <host>`, and nothing else. In particular no credential, because
        // the pairing the client already holds is the authorisation.
        assert_eq!(root.client_args("client"), vec!["list", "astra.local"]);
    }

    /// An unreachable machine makes the client wait rather than fail, so the
    /// deadline is the only thing that turns it into an answer.
    #[cfg(unix)]
    #[test]
    fn a_client_that_never_answers_is_cut_off_at_the_deadline() {
        let root = TestRoot::new("hang");
        let client = root.sleeping_client("client");
        let started = Instant::now();
        assert_eq!(
            list_apps_within(&client, "astra.local", Duration::from_millis(300)).unwrap_err(),
            GameStreamFeedError::Unreachable("astra.local".into())
        );
        assert!(started.elapsed() < Duration::from_secs(5), "it waited");
    }

    #[cfg(unix)]
    #[test]
    fn a_client_that_is_not_there_is_not_a_hang() {
        let root = TestRoot::new("missing");
        assert_eq!(
            list_apps(&root.path.join("nope"), "astra.local").unwrap_err(),
            GameStreamFeedError::ClientUnavailable
        );
    }

    /// A refusal is overwhelmingly "not paired", and that is the one thing the
    /// user can act on — so `is_paired` reads it as a state rather than an
    /// error, and leaves the real errors as errors.
    #[cfg(unix)]
    #[test]
    fn pairing_state_is_read_from_whether_the_machine_answers() {
        let root = TestRoot::new("paired");
        let service = root.service();
        update_host(&service, "astra.local");
        assert!(
            service
                .is_paired(&root.client("yes", "Modulus\n", 0))
                .unwrap()
        );
        assert!(!service.is_paired(&root.client("no", "", 1)).unwrap());
        assert_eq!(
            service.is_paired(&root.path.join("nope")).unwrap_err(),
            GameStreamFeedError::ClientUnavailable
        );
    }

    // -- the refresh -------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn the_feed_writes_one_placeholder_per_application() {
        let root = TestRoot::new("refresh");
        let folder = root.folder();
        let client = root.client("client", "Modulus\nDesktop\n", 0);
        let service = root.service();
        update_host(&service, "https://astra.local:47990");

        let outcome = service.refresh_placeholders(&client, &folder).unwrap();
        assert_eq!(
            outcome,
            RefreshOutcome {
                found: 2,
                written: 2,
                removed: 0,
                skipped: 0,
            }
        );

        let placeholder: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(folder.join("Modulus.stream")).unwrap())
                .unwrap();
        // The machine, not Sunshine's web port: this is what a launch will hand
        // the client, and the client talks the machine's own protocol.
        assert_eq!(placeholder["host"], "astra.local");
        assert_eq!(placeholder["client"], "moonlight");
        assert_eq!(placeholder["app"], "Modulus");

        let manifest = read_manifest(&folder.join(MANIFEST_FILE));
        assert_eq!(manifest.version, MANIFEST_VERSION);
        assert_eq!(manifest.files, vec!["Desktop.stream", "Modulus.stream"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_second_refresh_removes_dropped_games_and_keeps_handmade_files() {
        let root = TestRoot::new("stale");
        let folder = root.folder();
        let service = root.service();
        update_host(&service, "astra.local");
        service
            .refresh_placeholders(&root.client("first", "Alpha\nBravo\nCharlie\n", 0), &folder)
            .unwrap();

        let handmade = folder.join("Mine.stream");
        fs::write(
            &handmade,
            b"{\"host\":\"old\",\"client\":\"moonlight\",\"app\":\"Mine\"}",
        )
        .unwrap();

        let outcome = service
            .refresh_placeholders(&root.client("second", "Alpha\nDelta\n", 0), &folder)
            .unwrap();
        assert_eq!(
            outcome,
            RefreshOutcome {
                found: 2,
                written: 1,
                removed: 2,
                skipped: 0,
            }
        );

        assert!(folder.join("Alpha.stream").is_file());
        assert!(folder.join("Delta.stream").is_file());
        assert!(!folder.join("Bravo.stream").exists());
        assert!(!folder.join("Charlie.stream").exists());
        assert!(
            handmade.is_file(),
            "a hand-made file is not the feed's to remove"
        );
        let manifest = read_manifest(&folder.join(MANIFEST_FILE));
        assert_eq!(manifest.files, vec!["Alpha.stream", "Delta.stream"]);
    }

    #[cfg(unix)]
    #[test]
    fn an_application_whose_name_answers_to_a_handmade_file_yields() {
        let root = TestRoot::new("yield");
        let folder = root.folder();
        fs::write(
            folder.join("Modulus.stream"),
            b"{\"host\":\"astra.local\",\"client\":\"moonlight\",\"app\":\"Modulus\"}",
        )
        .unwrap();
        let service = root.service();
        update_host(&service, "astra.local");

        let outcome = service
            .refresh_placeholders(&root.client("client", "Modulus\nOther\n", 0), &folder)
            .unwrap();
        assert_eq!(
            outcome,
            RefreshOutcome {
                found: 2,
                written: 1,
                removed: 0,
                skipped: 1,
            }
        );
        let kept = fs::read_to_string(folder.join("Modulus.stream")).unwrap();
        assert!(kept.contains("astra.local"));
        let manifest = read_manifest(&folder.join(MANIFEST_FILE));
        assert_eq!(manifest.files, vec!["Other.stream"]);
    }

    #[cfg(unix)]
    #[test]
    fn two_applications_that_sanitize_alike_get_two_files() {
        let root = TestRoot::new("dedupe");
        let folder = root.folder();
        let service = root.service();
        update_host(&service, "astra.local");

        let outcome = service
            .refresh_placeholders(&root.client("client", "A/B\nA B\n", 0), &folder)
            .unwrap();
        assert_eq!(outcome.written, 2);
        assert!(folder.join("A B.stream").is_file());
        assert!(folder.join("A B (2).stream").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_machine_that_refuses_changes_nothing() {
        let root = TestRoot::new("refused");
        let folder = root.folder();
        let service = root.service();
        update_host(&service, "astra.local");

        assert_eq!(
            service
                .refresh_placeholders(&root.client("client", "", 1), &folder)
                .unwrap_err(),
            GameStreamFeedError::NotPaired("astra.local".into())
        );
        assert!(!folder.join(MANIFEST_FILE).exists());
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn an_unconfigured_host_is_the_manual_mode_refusal() {
        let root = TestRoot::new("unconfigured");
        let folder = root.folder();
        assert_eq!(
            root.service()
                .refresh_placeholders(&root.client("client", "Modulus\n", 0), &folder)
                .unwrap_err(),
            GameStreamFeedError::NotConfigured
        );
    }

    // -- finding the client ------------------------------------------------

    /// The handle is derived from the path, so it survives a restart, and it
    /// carries none of it, so a WebView holding one has learned nothing about
    /// the filesystem.
    #[test]
    fn a_client_handle_is_stable_and_says_nothing_about_where_it_points() {
        let path = Path::new("/Applications/Moonlight.app/Contents/MacOS/Moonlight");
        let id = client_id(path);
        assert_eq!(id, client_id(path));
        assert_ne!(id, client_id(Path::new("/Applications/Other")));
        assert!(!id.contains("Moonlight"));
        assert!(!id.contains('/'));
        // It has to pass the catalogue's opaque-id grammar to cross IPC.
        assert!(
            id.chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        );
    }

    /// Only what detection found can be resolved: an id for a real file that
    /// detection does not look at is still nothing.
    #[test]
    fn a_handle_for_something_detection_did_not_find_resolves_to_nothing() {
        let root = TestRoot::new("handles");
        let stranger = root.path.join("Moonlight");
        fs::write(&stranger, b"#!/bin/sh\n").unwrap();
        let canonical = fs::canonicalize(&stranger).unwrap();
        assert_eq!(client_for_id(&client_id(&canonical)), None);
        assert_eq!(client_for_id("client-not-a-real-handle"), None);
    }

    /// Whatever this machine has, every answer is something that exists, is
    /// named without a path, resolves back through its own handle, and is
    /// something a launch could actually start — on macOS that is a bundle, so
    /// the check is the one the launch itself makes rather than `is_file`.
    #[test]
    fn detection_answers_only_with_applications_that_are_there() {
        for client in detect_clients() {
            assert!(client.path.exists(), "{client:?}");
            assert!(!client.label.contains('/'), "{client:?}");
            assert_eq!(client_for_id(&client.id).as_deref(), Some(&*client.path));
            let resolved =
                crate::catalog::resolve_executable(&client.path).expect("a startable program");
            assert!(resolved.is_file(), "{client:?} -> {resolved:?}");
        }
    }

    // -- the guided setup ----------------------------------------------------

    /// People paste what is in their browser's bar. Explaining ports and paths
    /// to them would be asking them to do the host's work, so all of it goes.
    #[test]
    fn a_pasted_address_is_reduced_to_the_machine() {
        for (input, expected) in [
            ("my-pc.local", "my-pc.local"),
            ("  my-pc.local  ", "my-pc.local"),
            ("https://my-pc.local:47990/pin", "my-pc.local"),
            ("HTTPS://my-pc.local:47990", "my-pc.local"),
            ("http://192.168.1.20/", "192.168.1.20"),
            ("192.168.1.20:47990", "192.168.1.20"),
            ("https://[fd00::1]:47990/", "[fd00::1]"),
            (
                "https://admin:secret@my-pc.local:47990/pin?x=1#top",
                "my-pc.local",
            ),
        ] {
            assert_eq!(normalise_host(input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn what_is_not_an_address_stays_refused_after_tidying() {
        assert_eq!(
            normalise_host("   ").unwrap_err(),
            GameStreamFeedError::NotConfigured
        );
        for bad in [
            "file:///etc/passwd",
            "ftp://my-pc.local",
            "my pc",
            "-my-pc",
            "fd00::1",
        ] {
            assert_eq!(
                normalise_host(bad).unwrap_err(),
                GameStreamFeedError::HostInvalid,
                "{bad:?}"
            );
        }
        // Said in words a person can use, with an example to copy.
        let message = GameStreamFeedError::HostInvalid.to_string();
        assert!(message.contains("my-pc.local"), "{message}");
    }

    /// The four states the setup branches on, each from the two facts that
    /// decide it. Reachability comes first: `list` waits on a machine that is
    /// off and exits non-zero on one that has not paired, and only the second
    /// of those is a reason to show a PIN.
    #[test]
    fn what_a_machine_said_maps_to_one_state() {
        let listing = Ok(vec!["Desktop".to_owned()]);
        let refused: Result<Vec<String>, _> =
            Err(GameStreamFeedError::NotPaired("my-pc.local".into()));
        let waited: Result<Vec<String>, _> =
            Err(GameStreamFeedError::Unreachable("my-pc.local".into()));
        let no_client: Result<Vec<String>, _> = Err(GameStreamFeedError::ClientUnavailable);
        let garbled: Result<Vec<String>, _> = Err(GameStreamFeedError::Unreadable);

        assert_eq!(host_state(true, &listing), HostState::Paired);
        assert_eq!(host_state(true, &refused), HostState::NotPaired);
        assert_eq!(host_state(true, &waited), HostState::Unreachable);
        assert_eq!(host_state(true, &no_client), HostState::ClientMissing);
        // It answered; reading the answer is the import's problem.
        assert_eq!(host_state(true, &garbled), HostState::Paired);
        // Nothing listening: whatever `list` would have said is beside the point.
        assert_eq!(host_state(false, &listing), HostState::Unreachable);
        assert_eq!(host_state(false, &refused), HostState::Unreachable);
    }

    #[test]
    fn a_machine_accepting_connections_is_reachable_and_a_closed_port_is_not() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let open = listener.local_addr().unwrap().port();
        let closed = {
            let spare = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            spare.local_addr().unwrap().port()
        };
        let wait = Duration::from_secs(2);
        assert!(host_reachable("127.0.0.1", &[open], wait));
        // Either port being open is enough: a host may serve only one.
        assert!(host_reachable("127.0.0.1", &[closed, open], wait));
        // On, with nothing listening: Sunshine is not running.
        assert!(!host_reachable("127.0.0.1", &[closed], wait));
        assert!(!host_reachable("no-such-host.invalid", &[open], wait));
    }

    #[cfg(unix)]
    #[test]
    fn a_probe_asks_the_client_only_when_the_machine_answered() {
        let root = TestRoot::new("probe");
        let paired = root.client("paired", "Desktop\nCeleste\n", 0);
        let refusing = root.client("refusing", "", 1);
        let wait = Duration::from_secs(5);

        let view = probe_host_with(&paired, "my-pc.local", |_| true, wait);
        assert_eq!(view.state, HostState::Paired);
        assert_eq!(view.games, 2);
        assert_eq!(view.host, "my-pc.local");
        assert_eq!(root.client_args("paired"), vec!["list", "my-pc.local"]);

        let view = probe_host_with(&refusing, "my-pc.local", |_| true, wait);
        assert_eq!(view.state, HostState::NotPaired);

        // No answer from the machine: the client is not even started.
        let view = probe_host_with(&paired, "other.local", |_| false, wait);
        assert_eq!(view.state, HostState::Unreachable);
        assert_eq!(root.client_args("paired"), vec!["list", "my-pc.local"]);

        let missing = root.path.join("not-installed");
        let view = probe_host_with(&missing, "my-pc.local", |_| true, wait);
        assert_eq!(view.state, HostState::ClientMissing);
    }

    /// The links are a closed set the host builds: nothing the WebView sends
    /// becomes an address.
    #[test]
    fn only_the_two_known_links_can_be_opened() {
        assert_eq!(
            link_for("moonlight", "").unwrap(),
            "https://moonlight-stream.org"
        );
        assert_eq!(
            link_for("sunshine-pin", "https://my-pc.local:47990/").unwrap(),
            "https://my-pc.local:47990/pin"
        );
        assert_eq!(
            link_for("sunshine-pin", "").unwrap_err(),
            "Add your gaming PC first."
        );
        assert!(link_for("https://evil.example", "my-pc.local").is_err());
        assert!(link_for("", "my-pc.local").is_err());
    }
}
