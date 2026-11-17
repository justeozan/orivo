//! Game streaming built into Orivo: a gaming PC running Sunshine (or a fork
//! such as Apollo) streams its games here, with no Moonlight and no plugin.
//!
//! Settings shows this as a plugin, because that is where a person looks for
//! "play my PC's games", but it is compiled in. The protocol is Starfire's
//! (`starfire-core`, Apache-2.0, pinned to Orivo's fork), and Moonlight's own
//! core, GPLv3, could not be linked here anyway. That is also what lets it run
//! on iOS, where Orivo can start no process and load no plugin: everything below
//! is plain Rust over sockets.
//!
//! Three things live here, in the order the setup asks for them:
//!
//! 1. **The machine.** One address, saved once it has answered.
//! 2. **The pairing.** Orivo draws the PIN, the person types it into Sunshine,
//!    and Starfire runs the handshake. What comes out is a client certificate
//!    the host now trusts plus the host's own certificate, pinned. Both are
//!    kept, filed under the host's unique id rather than its address, so a PC
//!    that moves from the LAN to Tailscale is still the PC this client paired
//!    with.
//! 3. **The games.** The host's `/applist`, asked over mutual TLS with that
//!    pairing, becomes one card per game.
//!
//! Playing is the next step (a native player). Until it lands, a card says so
//! instead of pretending.

use std::{
    collections::BTreeMap,
    fs, io,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use starfire_core::{
    discovery::{self, HostAddress},
    launch::App,
    pairing::ClientIdentity,
    session::{self, SavedPairing},
};

use crate::catalog::{Catalog, Game, GameSource, LaunchTarget};
use crate::gamestream::{HostState, HostStatusView};

/// The provider token on every card this source adds, and its `GameSource`.
pub const PROVIDER: &str = "stream";
/// Where the address and the pairings are kept, inside the app's data folder.
pub const STATE_FILE: &str = "builtin-stream.json";

const ID_PREFIX: &str = "stream:";
const MAX_TITLE_CHARS: usize = 120;
const MAX_DEVICE_NAME_CHARS: usize = 48;
/// How long the host may hold the first pairing request open while someone
/// walks to it and types the PIN. The setup gives up on its side at three
/// minutes as well.
const PAIRING_WAIT: Duration = Duration::from_secs(180);

// ---------------------------------------------------------------------------
// What is kept on disk
// ---------------------------------------------------------------------------

/// One pairing, as written to disk. The key is a secret: the file is written
/// readable by this user only, and none of these fields ever reaches the
/// WebView.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredPairing {
    client_unique_id: String,
    device_name: String,
    client_cert_pem: String,
    client_key_pem: String,
    host_cert_pem: String,
}

impl std::fmt::Debug for StoredPairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredPairing")
            .field("client_unique_id", &self.client_unique_id)
            .field("device_name", &self.device_name)
            .finish_non_exhaustive()
    }
}

impl StoredPairing {
    fn from_saved(saved: &SavedPairing) -> Self {
        Self {
            client_unique_id: saved.identity.unique_id.clone(),
            device_name: saved.identity.device_name.clone(),
            client_cert_pem: saved.identity.cert_pem.clone(),
            client_key_pem: saved.identity.key_pem.clone(),
            host_cert_pem: String::from_utf8_lossy(&saved.host_cert_pem).into_owned(),
        }
    }

    fn to_saved(&self) -> SavedPairing {
        SavedPairing {
            identity: ClientIdentity {
                unique_id: self.client_unique_id.clone(),
                device_name: self.device_name.clone(),
                cert_pem: self.client_cert_pem.clone(),
                key_pem: self.client_key_pem.clone(),
            },
            host_cert_pem: self.host_cert_pem.clone().into_bytes(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredState {
    /// The address the person gave, normalised. Empty until one answered.
    #[serde(default)]
    host: String,
    /// Pairings by the host's `uniqueid` from `/serverinfo`.
    #[serde(default)]
    pairings: BTreeMap<String, StoredPairing>,
}

// ---------------------------------------------------------------------------
// What the setup is told
// ---------------------------------------------------------------------------

/// The saved address and whether this client is paired with the machine it
/// names, as far as the last look knows. The setup asks the machine itself
/// ([`BuiltinStreamService::probe`]) before trusting `paired`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinStreamSettings {
    pub host: String,
    pub paired: bool,
}

/// Where a pairing attempt stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "phase")]
pub enum PairingStatus {
    /// No attempt is open.
    Idle,
    /// Waiting for the PIN to be typed on the host.
    Waiting { pin: String },
    /// The host accepted this client.
    Paired,
    /// The attempt ended without pairing, with a sentence for the person.
    Failed { message: String },
}

/// The PIN to show while the host waits for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingStarted {
    pub pin: String,
    pub host: String,
}

/// What an import found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamGamesSync {
    pub games: usize,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinStreamError {
    NoHost,
    InvalidHost,
    Unreachable,
    NotPaired,
    Refused(String),
    Storage,
}

impl std::fmt::Display for BuiltinStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoHost => f.write_str("Add your gaming PC first."),
            Self::InvalidHost => f.write_str(
                "That address does not look like a computer. Use its name or IP address, like my-pc.local or 192.168.1.20.",
            ),
            Self::Unreachable => f.write_str(
                "Your gaming PC did not answer. Check that it is on, that Sunshine is running, and that both devices are on the same network.",
            ),
            Self::NotPaired => {
                f.write_str("Your gaming PC does not know Orivo yet. Pair them again.")
            }
            Self::Refused(message) => f.write_str(message),
            Self::Storage => f.write_str("Orivo could not save the pairing on this device."),
        }
    }
}

impl std::error::Error for BuiltinStreamError {}

// ---------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Attempt {
    Idle,
    Waiting { pin: String, generation: u64 },
    Paired,
    Failed(String),
}

pub struct BuiltinStreamService {
    path: PathBuf,
    state: Mutex<StoredState>,
    attempt: Mutex<Attempt>,
    generation: Mutex<u64>,
}

impl BuiltinStreamService {
    pub fn load(path: PathBuf) -> Self {
        let state = fs::read_to_string(&path)
            .ok()
            .and_then(|encoded| match serde_json::from_str::<StoredState>(&encoded) {
                Ok(state) => Some(state),
                Err(error) => {
                    eprintln!("orivo: built-in streaming state is unreadable ({error}); starting empty");
                    None
                }
            })
            .unwrap_or_default();
        Self {
            path,
            state: Mutex::new(state),
            attempt: Mutex::new(Attempt::Idle),
            generation: Mutex::new(0),
        }
    }

    fn snapshot(&self) -> StoredState {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default()
    }

    fn update(&self, change: impl FnOnce(&mut StoredState)) -> Result<(), BuiltinStreamError> {
        let mut state = self.state.lock().map_err(|_| BuiltinStreamError::Storage)?;
        let mut next = state.clone();
        change(&mut next);
        let encoded = serde_json::to_vec_pretty(&next).map_err(|_| BuiltinStreamError::Storage)?;
        write_private(&self.path, &encoded).map_err(|_| BuiltinStreamError::Storage)?;
        *state = next;
        Ok(())
    }

    pub fn settings(&self) -> BuiltinStreamSettings {
        let state = self.snapshot();
        BuiltinStreamSettings {
            paired: !state.host.is_empty() && !state.pairings.is_empty(),
            host: state.host,
        }
    }

    fn saved_host(&self) -> Result<String, BuiltinStreamError> {
        let host = self.snapshot().host;
        if host.is_empty() {
            Err(BuiltinStreamError::NoHost)
        } else {
            Ok(host)
        }
    }

    /// Ask a machine whether it is there and whether it knows this client. An
    /// empty `host` means the saved one. A machine that answered is saved as
    /// the one to stream from, which is what the setup's "Connect" means.
    pub fn probe(&self, host: &str) -> Result<HostStatusView, BuiltinStreamError> {
        let host = if host.trim().is_empty() {
            self.saved_host()?
        } else {
            normalise_host(host)?
        };
        let status = probe_host(&host, &self.snapshot().pairings);
        if status.state != HostState::Unreachable {
            self.update(|state| state.host = host.clone())?;
        }
        Ok(status)
    }

    /// Draw a PIN and start the handshake in the background. The host holds
    /// the first request open until the PIN is typed into it; the result is
    /// read with [`Self::pairing_status`]. A new attempt, or a cancel, makes the
    /// previous one's result land nowhere.
    pub fn begin_pairing(
        self: &Arc<Self>,
        device_name: &str,
    ) -> Result<PairingStarted, BuiltinStreamError> {
        let host = self.saved_host()?;
        let pin = pairing_pin().ok_or(BuiltinStreamError::Storage)?;
        let device_name = tidy_device_name(device_name);
        let generation = {
            let mut generation = self
                .generation
                .lock()
                .map_err(|_| BuiltinStreamError::Storage)?;
            *generation += 1;
            *generation
        };
        if let Ok(mut attempt) = self.attempt.lock() {
            *attempt = Attempt::Waiting {
                pin: pin.clone(),
                generation,
            };
        }

        let service = Arc::clone(self);
        let (thread_host, thread_pin) = (host.clone(), pin.clone());
        std::thread::spawn(move || {
            let outcome = pair_with(&thread_host, &device_name, &thread_pin);
            let current = service.generation.lock().map(|g| *g).unwrap_or(0);
            if current != generation {
                return;
            }
            let next = match outcome.and_then(|(host_id, saved)| {
                service.update(|state| {
                    state
                        .pairings
                        .insert(host_id, StoredPairing::from_saved(&saved));
                })
            }) {
                Ok(()) => Attempt::Paired,
                Err(error) => Attempt::Failed(error.to_string()),
            };
            if let Ok(mut attempt) = service.attempt.lock() {
                *attempt = next;
            }
        });
        Ok(PairingStarted { pin, host })
    }

    pub fn pairing_status(&self) -> PairingStatus {
        match self.attempt.lock().map(|attempt| attempt.clone()) {
            Ok(Attempt::Waiting { pin, .. }) => PairingStatus::Waiting { pin },
            Ok(Attempt::Paired) => PairingStatus::Paired,
            Ok(Attempt::Failed(message)) => PairingStatus::Failed { message },
            Ok(Attempt::Idle) | Err(_) => PairingStatus::Idle,
        }
    }

    /// Stop listening for the open attempt. The host may keep its side open
    /// until it times out; whatever it answers then is ignored.
    pub fn cancel_pairing(&self) {
        if let Ok(mut generation) = self.generation.lock() {
            *generation += 1;
        }
        if let Ok(mut attempt) = self.attempt.lock() {
            *attempt = Attempt::Idle;
        }
    }

    /// Forget every pairing and the address. The host keeps its own record of
    /// this client until it is removed in Sunshine.
    pub fn forget(&self) -> Result<(), BuiltinStreamError> {
        self.cancel_pairing();
        self.update(|state| *state = StoredState::default())
    }

    /// Everything the player needs to stream one game: where the PC is, the
    /// pairing kept for it, and the title the host knows the game by (the host
    /// starts a game by that title, not by the card's name, which the person
    /// may have edited).
    pub fn stream_target(&self, app_ref: &str) -> Result<StreamTarget, BuiltinStreamError> {
        let host = self.saved_host()?;
        let state = self.snapshot();
        let info = discovery::probe(&HostAddress {
            host: host.clone(),
            http_port: discovery::DEFAULT_HTTP_PORT,
        })
        .map_err(|_| BuiltinStreamError::Unreachable)?;
        let pairing = info
            .unique_id
            .as_ref()
            .and_then(|id| state.pairings.get(id))
            .ok_or(BuiltinStreamError::NotPaired)?
            .to_saved();
        let apps = session::resume(&host, &pairing)
            .and_then(|client| client.applist())
            .map_err(|_| BuiltinStreamError::NotPaired)?;
        let app = apps.into_iter().find(|app| app.id == app_ref).ok_or_else(|| {
            BuiltinStreamError::Refused("That game is no longer on your gaming PC.".into())
        })?;
        Ok(StreamTarget {
            host,
            pairing,
            title: app.title,
        })
    }

    /// The saved machine's games, asked with this client's pairing.
    pub fn list_games(&self) -> Result<Vec<App>, BuiltinStreamError> {
        let host = self.saved_host()?;
        let state = self.snapshot();
        let info = discovery::probe(&HostAddress {
            host: host.clone(),
            http_port: discovery::DEFAULT_HTTP_PORT,
        })
        .map_err(|_| BuiltinStreamError::Unreachable)?;
        let pairing = info
            .unique_id
            .as_ref()
            .and_then(|id| state.pairings.get(id))
            .ok_or(BuiltinStreamError::NotPaired)?;
        let client = session::resume(&host, &pairing.to_saved())
            .map_err(|_| BuiltinStreamError::NotPaired)?;
        client.applist().map_err(|_| BuiltinStreamError::NotPaired)
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

type Service<'state> = tauri::State<'state, Arc<BuiltinStreamService>>;

#[tauri::command]
pub fn get_builtin_stream_settings(service: Service<'_>) -> BuiltinStreamSettings {
    service.settings()
}

/// Ask a machine whether it is there and knows this client, and keep its
/// address if it answered. Network work, so it runs off the async runtime.
#[tauri::command]
pub async fn probe_builtin_stream_host(
    host: String,
    service: Service<'_>,
) -> Result<HostStatusView, String> {
    let service = Arc::clone(&service);
    tauri::async_runtime::spawn_blocking(move || service.probe(&host))
        .await
        .map_err(|_| BuiltinStreamError::Unreachable.to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn begin_builtin_stream_pairing(
    device_name: String,
    service: Service<'_>,
) -> Result<PairingStarted, String> {
    service
        .inner()
        .begin_pairing(&device_name)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn builtin_stream_pairing_status(service: Service<'_>) -> PairingStatus {
    service.pairing_status()
}

#[tauri::command]
pub fn cancel_builtin_stream_pairing(service: Service<'_>) {
    service.cancel_pairing();
}

#[tauri::command]
pub fn forget_builtin_stream(service: Service<'_>) -> Result<(), String> {
    service.forget().map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// The network, kept out of the service so the service's rules can be tested
// ---------------------------------------------------------------------------

/// Is the machine there, and does it accept this client? Being there is
/// `/serverinfo` over plain HTTP, which any client may ask. Being accepted is
/// `/applist` over mutual TLS with the pairing kept for that machine, the one
/// question only a paired client gets an answer to.
fn probe_host(host: &str, pairings: &BTreeMap<String, StoredPairing>) -> HostStatusView {
    let view = |state, games| HostStatusView {
        state,
        host: host.to_owned(),
        games,
    };
    let Ok(info) = discovery::probe(&HostAddress {
        host: host.to_owned(),
        http_port: discovery::DEFAULT_HTTP_PORT,
    }) else {
        return view(HostState::Unreachable, 0);
    };
    let Some(pairing) = info.unique_id.as_ref().and_then(|id| pairings.get(id)) else {
        return view(HostState::NotPaired, 0);
    };
    match session::resume(host, &pairing.to_saved()).and_then(|client| client.applist()) {
        Ok(apps) => view(HostState::Paired, apps.len()),
        Err(_) => view(HostState::NotPaired, 0),
    }
}

/// Run the handshake and say which machine accepted it.
fn pair_with(
    host: &str,
    device_name: &str,
    pin: &str,
) -> Result<(String, SavedPairing), BuiltinStreamError> {
    let address = HostAddress {
        host: host.to_owned(),
        http_port: discovery::DEFAULT_HTTP_PORT,
    };
    let info = discovery::probe(&address).map_err(|_| BuiltinStreamError::Unreachable)?;
    let host_id = info
        .unique_id
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            BuiltinStreamError::Refused(
                "Your gaming PC did not say who it is. Update Sunshine and try again.".into(),
            )
        })?;
    let saved = pair_and_save(host, device_name, pin, PAIRING_WAIT).map_err(|error| {
        // The detail is for the log; the person gets a sentence.
        eprintln!("orivo: built-in streaming pairing with {host} failed: {error}");
        BuiltinStreamError::Refused(pairing_failure(&error.to_string()))
    })?;
    Ok((host_id, saved))
}

/// Starfire's `session::pair_and_save`, with one tolerance: Apollo registers
/// the client and then drops the connection on the fourth step
/// (`clientpairingsecret`), so the answer never arrives. That reset is not a
/// verdict. The fifth step, over the certificate the host now trusts, is: it
/// only succeeds if the host really took the client.
fn pair_and_save(
    host: &str,
    device_name: &str,
    pin: &str,
    pin_wait: Duration,
) -> starfire_core::Result<SavedPairing> {
    use starfire_core::{
        https::{cert_pem_to_der, HttpsClient},
        pairing::{
            crypto::{aes_ecb_decrypt, pin_key, random_bytes},
            PairingClient,
        },
        Error,
    };

    let identity = ClientIdentity::generate(device_name)?;
    let (cert, key) = (identity.cert_pem.clone(), identity.key_pem.clone());
    let mut client = PairingClient::new(host, discovery::DEFAULT_HTTP_PORT, identity);
    client.timeout = pin_wait;

    let salt = random_bytes::<16>()?;
    // Step 1 blocks until the PIN is typed on the host.
    let host_cert = client.get_server_cert(&salt)?;
    let key_from_pin = pin_key(&salt, pin);
    let answer = client.client_challenge(&key_from_pin, &random_bytes::<16>()?)?;
    let decrypted = aes_ecb_decrypt(&key_from_pin, &answer);
    if decrypted.len() < 48 {
        return Err(Error::Protocol(format!(
            "challengeresponse too short: {} bytes (want 48)",
            decrypted.len()
        )));
    }
    let mut server_challenge = [0u8; 16];
    server_challenge.copy_from_slice(&decrypted[32..48]);
    let secret = random_bytes::<16>()?;
    client.server_challenge_resp(&key_from_pin, &server_challenge, &secret)?;
    match client.client_pairing_secret(&secret) {
        Ok(()) => {}
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::ConnectionReset => {
            eprintln!("orivo: the host dropped the connection after the pairing secret; checking with the trusted certificate");
        }
        Err(error) => return Err(error),
    }

    let https = HttpsClient::new(&cert, &key, Some(cert_pem_to_der(&host_cert)?))?;
    client.pair_challenge(&https, 47984)?;
    Ok(SavedPairing {
        identity: client.identity,
        host_cert_pem: host_cert,
    })
}

/// Starfire's errors are for logs. Say what happened in the person's terms.
fn pairing_failure(detail: &str) -> String {
    let detail = detail.to_ascii_lowercase();
    if detail.contains("timed out")
        || detail.contains("temporarily unavailable")
        || detail.contains("would block")
    {
        "The PIN was not entered in time. Try again, and type the PIN in Sunshine's PIN page."
            .into()
    } else if detail.contains("paired=0") {
        "Your gaming PC refused the PIN. Try again with the new one.".into()
    } else {
        "Pairing did not finish. Try again.".into()
    }
}

/// Four digits, rejection-sampled so every PIN is as likely as any other
/// (65536 is not a multiple of 10000).
fn pairing_pin() -> Option<String> {
    loop {
        let mut bytes = [0u8; 2];
        getrandom::fill(&mut bytes).ok()?;
        let sample = u16::from_le_bytes(bytes);
        if sample < 60_000 {
            return Some(format!("{:04}", sample % 10_000));
        }
    }
}

/// The address grammar the Moonlight setup already uses, so the same text pastes
/// the same way into either.
fn normalise_host(input: &str) -> Result<String, BuiltinStreamError> {
    crate::gamestream::normalise_host(input).map_err(|_| BuiltinStreamError::InvalidHost)
}

/// The name Sunshine shows in its client list. Printable, bounded, never empty.
fn tidy_device_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_DEVICE_NAME_CHARS)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "Orivo".into()
    } else {
        name.to_owned()
    }
}

/// Write a file only this user can read, atomically.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("no parent directory"))?;
    fs::create_dir_all(parent)?;
    let sequence = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(
        ".builtin-stream.{}.{sequence}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

// ---------------------------------------------------------------------------
// The library
// ---------------------------------------------------------------------------

/// What the player is handed to start one stream.
pub struct StreamTarget {
    pub host: String,
    pub pairing: SavedPairing,
    pub title: String,
}

/// A card's id, from the game's title so that it survives the host renumbering
/// its apps. A title with nothing to keep falls back to the host's own id.
pub fn game_id(app: &App) -> String {
    let mut slug = String::new();
    for character in app.title.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        format!("{ID_PREFIX}app-{}", app.id)
    } else {
        format!("{ID_PREFIX}{slug}")
    }
}

/// One card per game the host lists. "Desktop" and Steam Big Picture are how
/// Sunshine shows the whole PC, not games, so they are left out. Anything the
/// catalog would refuse is dropped rather than failing the whole import.
pub fn games_from(apps: &[App]) -> Vec<Game> {
    let mut games = BTreeMap::new();
    for app in apps {
        let title: String = app
            .title
            .chars()
            .filter(|character| !character.is_control())
            .take(MAX_TITLE_CHARS)
            .collect();
        let title = title.trim();
        if title.is_empty() || is_not_a_game(title) {
            continue;
        }
        let game = Game {
            id: game_id(app),
            title: title.to_owned(),
            executable_path: None,
            source: GameSource::Stream,
            source_id: Some(app.id.clone()),
            launch_target: LaunchTarget::Provider {
                provider: PROVIDER.to_owned(),
                app_ref: app.id.clone(),
            },
            alternate_launch_targets: Vec::new(),
            installation_path: None,
            working_directory: None,
            arguments: Vec::new(),
            description: None,
            metadata: None,
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
        };
        if game.validate().is_ok() {
            games.entry(game.id.clone()).or_insert(game);
        }
    }
    games.into_values().collect()
}

fn is_not_a_game(title: &str) -> bool {
    matches!(
        title.to_ascii_lowercase().as_str(),
        "desktop" | "steam big picture" | "virtual desktop"
    )
}

/// Bring the library's streamed cards in line with the host's list. What the
/// person made of a card (play time, hidden, the artwork they found or chose)
/// stays; a game the host no longer lists leaves. Returns whether anything
/// changed.
pub fn apply(catalog: &mut Catalog, games: Vec<Game>) -> bool {
    let mut incoming: BTreeMap<String, Game> = games
        .into_iter()
        .map(|game| (game.id.clone(), game))
        .collect();
    let mut changed = false;

    catalog.games.retain_mut(|existing| {
        if existing.source != GameSource::Stream {
            return true;
        }
        let Some(fresh) = incoming.remove(&existing.id) else {
            changed = true;
            return false;
        };
        // Only what the host says about the game is refreshed: its title and
        // the id it launches under.
        if existing.title != fresh.title
            || existing.source_id != fresh.source_id
            || existing.launch_target != fresh.launch_target
        {
            existing.title = fresh.title;
            existing.source_id = fresh.source_id;
            existing.launch_target = fresh.launch_target;
            changed = true;
        }
        true
    });

    for (_, game) in incoming {
        catalog.games.push(game);
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, title: &str) -> App {
        App {
            id: id.into(),
            title: title.into(),
            hdr_supported: false,
        }
    }

    #[test]
    fn card_ids_come_from_titles_and_survive_renumbering() {
        assert_eq!(game_id(&app("1", "Celeste")), "stream:celeste");
        assert_eq!(
            game_id(&app("2", "Kena: Bridge of Spirits")),
            "stream:kena-bridge-of-spirits"
        );
        assert_eq!(
            game_id(&app("99", "Celeste")),
            game_id(&app("1", "Celeste"))
        );
        assert_eq!(game_id(&app("7", "  ")), "stream:app-7");
        assert_eq!(game_id(&app("8", "東方")), "stream:app-8");
    }

    #[test]
    fn the_whole_pc_is_not_a_game_and_titles_are_tidied() {
        let games = games_from(&[
            app("1", "Desktop"),
            app("2", "Steam Big Picture"),
            app("3", "Hades\u{7}"),
            app("4", "Celeste"),
            app("5", ""),
        ]);
        let titles: Vec<_> = games.iter().map(|game| game.title.as_str()).collect();
        assert_eq!(titles, vec!["Celeste", "Hades"]);
        let hades = games.iter().find(|game| game.title == "Hades").unwrap();
        assert_eq!(hades.source, GameSource::Stream);
        assert_eq!(
            hades.launch_target,
            LaunchTarget::Provider {
                provider: "stream".into(),
                app_ref: "3".into()
            }
        );
        assert!(hades.validate().is_ok());
    }

    #[test]
    fn an_id_the_catalog_would_refuse_is_dropped_not_fatal() {
        let games = games_from(&[app("not an id", "Celeste"), app("12", "Hades")]);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].title, "Hades");
    }

    #[test]
    fn applying_keeps_what_the_person_made_of_a_card() {
        let mut catalog = Catalog::default();
        assert!(apply(
            &mut catalog,
            games_from(&[app("1", "Celeste"), app("2", "Hades")])
        ));
        let celeste = catalog
            .games
            .iter_mut()
            .find(|g| g.title == "Celeste")
            .unwrap();
        celeste.play_time_seconds = 3600;
        celeste.hidden = true;
        celeste.cover_path = Some(PathBuf::from("/covers/celeste.jpg"));

        // The host renumbered Celeste and dropped Hades.
        assert!(apply(&mut catalog, games_from(&[app("41", "Celeste")])));
        assert_eq!(catalog.games.len(), 1);
        let celeste = &catalog.games[0];
        assert_eq!(celeste.play_time_seconds, 3600);
        assert!(celeste.hidden);
        assert_eq!(
            celeste.cover_path,
            Some(PathBuf::from("/covers/celeste.jpg"))
        );
        assert_eq!(celeste.source_id.as_deref(), Some("41"));

        assert!(!apply(&mut catalog, games_from(&[app("41", "Celeste")])));
    }

    /// A build without the "stream" source sets these entries aside (the
    /// tolerant catalog); this build must read them as games, not set them
    /// aside, and write them back as it found them.
    #[test]
    fn stream_games_are_read_not_set_aside() {
        let dir = std::env::temp_dir().join(format!(
            "orivo-builtin-stream-catalog-{}",
            std::process::id()
        ));
        let path = dir.join("catalog.json");
        let mut catalog = Catalog::default();
        apply(
            &mut catalog,
            games_from(&[app("1", "Celeste"), app("2", "Hades")]),
        );
        catalog.save_atomically(&path).unwrap();

        let loaded = Catalog::load(&path).unwrap();
        assert!(loaded.unreadable_games.is_empty());
        assert_eq!(loaded.games.len(), 2);
        assert!(loaded.games.iter().all(|g| g.source == GameSource::Stream));
        assert_eq!(loaded, catalog);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn applying_leaves_other_sources_alone() {
        let mut catalog = Catalog::default();
        let mut local = games_from(&[app("1", "Celeste")]).remove(0);
        local.id = "local-celeste".into();
        local.source = GameSource::Local;
        local.source_id = None;
        local.launch_target = LaunchTarget::Direct;
        local.executable_path = Some(PathBuf::from("/games/celeste"));
        catalog.games.push(local);
        apply(&mut catalog, Vec::new());
        assert_eq!(catalog.games.len(), 1);
    }

    #[test]
    fn device_names_are_printable_bounded_and_never_empty() {
        assert_eq!(tidy_device_name("Orivo (Mac)"), "Orivo (Mac)");
        assert_eq!(tidy_device_name("  \u{0}\n "), "Orivo");
        assert_eq!(
            tidy_device_name(&"x".repeat(200)).chars().count(),
            MAX_DEVICE_NAME_CHARS
        );
    }

    #[test]
    fn pins_are_four_digits() {
        for _ in 0..200 {
            let pin = pairing_pin().unwrap();
            assert_eq!(pin.len(), 4);
            assert!(pin.bytes().all(|byte| byte.is_ascii_digit()));
        }
    }

    #[test]
    fn pairing_failures_read_as_sentences() {
        assert!(
            pairing_failure("i/o error: Resource temporarily unavailable (os error 35)")
                .contains("in time")
        );
        assert!(pairing_failure("protocol error: clientchallenge: paired=0").contains("refused"));
        assert_eq!(
            pairing_failure("anything else"),
            "Pairing did not finish. Try again."
        );
    }

    #[test]
    fn the_state_file_round_trips_and_is_private() {
        let dir = std::env::temp_dir().join(format!("orivo-builtin-stream-{}", std::process::id()));
        let path = dir.join(STATE_FILE);
        let service = BuiltinStreamService::load(path.clone());
        assert_eq!(
            service.settings(),
            BuiltinStreamSettings {
                host: String::new(),
                paired: false
            }
        );
        service
            .update(|state| {
                state.host = "astra.local".into();
                state.pairings.insert(
                    "HOST-ID".into(),
                    StoredPairing {
                        client_unique_id: "ABCD".into(),
                        device_name: "Orivo".into(),
                        client_cert_pem: "cert".into(),
                        client_key_pem: "key".into(),
                        host_cert_pem: "host".into(),
                    },
                );
            })
            .unwrap();
        let reloaded = BuiltinStreamService::load(path.clone());
        assert_eq!(
            reloaded.settings(),
            BuiltinStreamSettings {
                host: "astra.local".into(),
                paired: true
            }
        );
        assert!(!format!("{:?}", reloaded.snapshot()).contains("key\""));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        reloaded.forget().unwrap();
        assert_eq!(BuiltinStreamService::load(path).settings().host, "");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_attempt_or_a_cancel_silences_the_old_one() {
        let dir = std::env::temp_dir().join(format!(
            "orivo-builtin-stream-cancel-{}",
            std::process::id()
        ));
        let service = BuiltinStreamService::load(dir.join(STATE_FILE));
        assert_eq!(service.pairing_status(), PairingStatus::Idle);
        assert_eq!(
            Arc::new(BuiltinStreamService::load(dir.join(STATE_FILE)))
                .begin_pairing("Orivo")
                .unwrap_err(),
            BuiltinStreamError::NoHost
        );
        service.cancel_pairing();
        assert_eq!(service.pairing_status(), PairingStatus::Idle);
        let _ = fs::remove_dir_all(dir);
    }
}
