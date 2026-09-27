//! Compiled plugin components, kept between sessions.
//!
//! Wasmtime recompiles a component every time `Component::new` is called, and
//! O1 measured what that costs on the two surfaces that discover plugins:
//! ~31 ms per installed component, 615 ms for twenty, paid again on every open
//! (`docs/performance.md`, section 3). Wasmtime can serialise a compiled
//! component and load it back, which is what this module does — section 3 bis of
//! the same document is the before and after: ~32 ms per component becomes
//! ~1.8 ms, and twenty components go from 646 ms to 36 ms.
//!
//! It is also the one place in this repository where a mistake is native code
//! execution rather than a refused plugin. `Component::deserialize` is `unsafe`
//! because it trusts its input completely: the bytes *are* machine code, and it
//! maps them executable without validating anything a compiler would have
//! validated. A cache file that any other process of this user could write
//! would therefore be a way to run arbitrary native code inside Orivo, with all
//! of Orivo's authority — worse than anything a plugin can do, because a plugin
//! is behind a sandbox and this would not be.
//!
//! So the rule here is narrow and absolute: **an artifact is deserialised only
//! when a secret only Rust holds proves Orivo wrote it.** The proof is an
//! HMAC-SHA256 tag over the artifact, the digest of the source component and a
//! fingerprint of the `Engine` that compiled it, keyed by a 256-bit key created
//! on first use and kept in the system keychain — never in a file, never in the
//! WebView, never in a Tauri command's return type.
//!
//! What that buys, precisely. A process that can *write* to this directory but
//! cannot read that keychain item — a downloaded archive unpacked into the
//! wrong place, a restored backup, a synchronised folder, a helper of some other
//! application, a plugin itself, which has `files.read` on folders the user
//! picked and no write anywhere — cannot produce bytes this module will load.
//! Neither can a copy of *another* installation's cache directory, because its
//! artifacts are tagged with another key. What it does not buy: a process that
//! can already read the keychain item without a prompt, attach to Orivo, or
//! replace Orivo's binary is not stopped by anything here, and does not need to
//! be — it already has everything the cache could give it. The asymmetry this
//! removes is the one that matters: being able to write one file should not be
//! enough.
//!
//! Everything else follows from "an artifact is regenerable". A missing,
//! corrupted, truncated, foreign, or stale artifact is a silent recompilation
//! from the source bytes the registry already verified — never an error the user
//! sees, and never a doubtful load. The cache is bounded, written atomically,
//! and can be purged whole at any time, which is the plugin plan's sixth
//! promise: a plugin's caches are the only thing Orivo may delete.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use wasmtime::{Engine, component::Component};

/// Under the app *cache* directory, not the app data directory: everything here
/// is derived from bytes Orivo still has, so deleting it costs a recompilation
/// and nothing else.
pub const CACHE_DIRECTORY: &str = "plugin-components";

/// File name suffix for an artifact, and the only thing [`purge`] removes
/// besides its own abandoned temporary files.
const ARTIFACT_SUFFIX: &str = ".cwasm";
const TEMPORARY_PREFIX: &str = ".writing-";

/// Eight bytes that say which format follows, so a future change to the header is
/// a refused artifact rather than a misread one. A format *gate*, not a security
/// one: the tag is what refuses a file Orivo did not write, and the format
/// version it is bound to travels in [`TAG_DOMAIN`] rather than here.
const ARTIFACT_MAGIC: &[u8; 8] = b"ORIVOCC1";
const TAG_BYTES: usize = 32;
const HEADER_BYTES: usize = ARTIFACT_MAGIC.len() + TAG_BYTES;

/// Domain separation. The install key authenticates one kind of thing and must
/// keep doing so if a second ever appears: #42 had to add exactly this to the
/// registry index after discovering that a package signature was also a valid
/// index signature under the same key.
const TAG_DOMAIN: &[u8] = b"orivo:plugin-compile-cache:artifact:v1";
const SLOT_DOMAIN: &[u8] = b"orivo:plugin-compile-cache:slot:v1";

/// Versioned, so a change to how the key is stored cannot make an older entry
/// decode into something it never meant.
const KEYRING_SERVICE: &str = "io.orivo.desktop.plugin-compile-cache.v1";
const KEYRING_ACCOUNT: &str = "artifact-key";
const KEY_BYTES: usize = 32;

/// How much of the engine fingerprint a slot name carries. Not a security
/// boundary — the tag is — only enough to recognise this build's artifacts among
/// a previous build's.
const GENERATION_BYTES: usize = 8;

/// An abandoned temporary file means a write that was interrupted. One older
/// than this cannot belong to a write still in progress, so it is swept.
const TEMPORARY_LIFETIME: Duration = Duration::from_secs(60 * 60);

type ArtifactTag = Hmac<Sha256>;

/// What the cache will hold. Both ceilings exist because the answer to "how big
/// can a compiled component get" is "as big as the component someone installs",
/// and a cache that grows with what a user installs is not a cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheLimits {
    pub max_artifact_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for CacheLimits {
    fn default() -> Self {
        // Measured, not guessed: the reference runner fixture is 45,359 bytes of
        // wasm and serialises to 246,648 — roughly 5.4× its source. 64 MiB is
        // therefore room for a component an order of magnitude larger than
        // anything plausible, and 256 MiB is room for a thousand of them.
        Self {
            max_artifact_bytes: 64 * 1024 * 1024,
            max_total_bytes: 256 * 1024 * 1024,
        }
    }
}

/// Hits, misses and refusals since this cache was opened.
///
/// A cache is invisible when it works, which makes it impossible to test and
/// impossible to measure. These counters are how "the warm cache was reused"
/// and "the altered artifact was refused" become assertions rather than
/// impressions. Nothing in the app reads them — a counter the user can see is a
/// counter that has to mean something to the user — so they carry the same
/// `allow(dead_code)` as the other test seams in this part of the codebase.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheCounts {
    pub hits: u64,
    pub misses: u64,
    pub stores: u64,
    /// Refused before Wasmtime was asked: wrong format, or a tag this
    /// installation did not write for this component and this engine.
    pub unauthenticated: u64,
    /// Authenticated, and Wasmtime still would not load it. Should be
    /// unreachable — the tag binds the engine fingerprint — so it is counted
    /// apart rather than folded in: the two say very different things about
    /// whether the key or the engine check is doing the work.
    pub unloadable: u64,
}

#[derive(Debug, Default)]
struct Counters {
    hits: AtomicU64,
    misses: AtomicU64,
    stores: AtomicU64,
    unauthenticated: AtomicU64,
    unloadable: AtomicU64,
}

/// A directory of compiled components, authenticated for one installation and
/// one `Engine` configuration.
#[derive(Debug)]
pub struct ComponentCache {
    engine: Engine,
    directory: PathBuf,
    key: [u8; KEY_BYTES],
    /// Binds an artifact to the exact compiler that produced it — see
    /// [`engine_fingerprint`].
    engine_fingerprint: [u8; 32],
    limits: CacheLimits,
    counters: Counters,
}

impl ComponentCache {
    pub fn open(
        engine: Engine,
        directory: PathBuf,
        key: [u8; KEY_BYTES],
        limits: CacheLimits,
    ) -> Self {
        let engine_fingerprint = engine_fingerprint(&engine);
        Self {
            engine,
            directory,
            key,
            engine_fingerprint,
            limits,
            counters: Counters::default(),
        }
    }

    #[allow(dead_code)]
    pub fn counts(&self) -> CacheCounts {
        CacheCounts {
            hits: self.counters.hits.load(Ordering::Relaxed),
            misses: self.counters.misses.load(Ordering::Relaxed),
            stores: self.counters.stores.load(Ordering::Relaxed),
            unauthenticated: self.counters.unauthenticated.load(Ordering::Relaxed),
            unloadable: self.counters.unloadable.load(Ordering::Relaxed),
        }
    }

    /// A component for `source`: from an artifact that proved itself, or from
    /// `compile`, whose result is kept for next time.
    ///
    /// Generic over the compiler's error on purpose. The cache has no failure of
    /// its own to report: every outcome is either a component or exactly the
    /// error compiling these bytes would have produced without it.
    ///
    /// The source digest is computed here rather than taken from the caller.
    /// Both call sites do re-hash the component against its manifest first, but
    /// "the artifact is bound to the SHA-256 of the bytes that were compiled" is
    /// a property of this module, and a property that depends on a caller
    /// remembering something is not a property.
    pub fn component<E>(
        &self,
        source: &[u8],
        compile: impl FnOnce() -> Result<Component, E>,
    ) -> Result<Component, E> {
        let digest = digest_of(source);
        if let Some(component) = self.load(&digest) {
            self.counters.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(component);
        }
        self.counters.misses.fetch_add(1, Ordering::Relaxed);
        let component = compile()?;
        self.store(&digest, &component);
        Ok(component)
    }

    /// Where the artifact for one source, under one engine generation, lives.
    ///
    /// Two parts, each with one job. The prefix is a readable slice of the engine
    /// fingerprint, so a changed `Config` or a new Wasmtime release looks in
    /// different names *and* this module can recognise the previous build's
    /// artifacts as dead weight rather than leaving them on the user's disk until
    /// the whole-cache ceiling happens to reclaim them. The suffix is a digest of
    /// the source, so one component is one slot.
    ///
    /// Neither half is trusted. The name says where to look; the tag says whether
    /// what was found may be run, and it binds the *whole* fingerprint rather
    /// than the prefix.
    fn slot_name(&self, digest: &[u8; 32]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(SLOT_DOMAIN);
        hasher.update(digest);
        format!(
            "{}-{:x}{ARTIFACT_SUFFIX}",
            self.generation(),
            hasher.finalize()
        )
    }

    /// The readable half of a slot name: enough of the engine fingerprint to
    /// separate one Orivo build's artifacts from another's.
    fn generation(&self) -> String {
        self.engine_fingerprint[..GENERATION_BYTES]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// The tag covers the artifact *and* what the artifact is for.
    ///
    /// A tag over the artifact alone would authenticate the bytes while saying
    /// nothing about which component they belong to, so a valid artifact could
    /// be copied onto another component's slot and Orivo would run the wrong
    /// code for a package the user did approve. Both bound fields are
    /// fixed-width and precede the only variable-length one, so no two inputs
    /// share a tag input.
    fn tag(&self, digest: &[u8; 32], artifact: &[u8]) -> [u8; TAG_BYTES] {
        self.mac(digest, artifact).finalize().into_bytes().into()
    }

    fn mac(&self, digest: &[u8; 32], artifact: &[u8]) -> ArtifactTag {
        let mut mac = <ArtifactTag as Mac>::new_from_slice(&self.key)
            .expect("HMAC-SHA256 accepts a key of any length");
        mac.update(TAG_DOMAIN);
        mac.update(&self.engine_fingerprint);
        mac.update(digest);
        mac.update(artifact);
        mac
    }

    fn load(&self, digest: &[u8; 32]) -> Option<Component> {
        let path = self.directory.join(self.slot_name(digest));
        let stored =
            read_regular_file(&path, self.limits.max_artifact_bytes + HEADER_BYTES as u64)?;
        if stored.len() < HEADER_BYTES || !stored.starts_with(ARTIFACT_MAGIC) {
            return self.refuse(&path, &self.counters.unauthenticated);
        }
        let (header, artifact) = stored.split_at(HEADER_BYTES);
        // `verify_slice` compares in constant time, through `subtle`. Timing is
        // not the threat here — whoever can write this file can read it back —
        // but a hand-rolled `==` on a tag is the kind of thing that gets copied
        // into a place where it is.
        if self
            .mac(digest, artifact)
            .verify_slice(&header[ARTIFACT_MAGIC.len()..])
            .is_err()
        {
            return self.refuse(&path, &self.counters.unauthenticated);
        }

        // SAFETY: `Component::deserialize` maps these bytes executable and runs
        // them. Three things must hold, and all three are established above.
        //
        // * They came out of one regular file and nothing else.
        //   `read_regular_file` opens with `O_NOFOLLOW` and asks the descriptor
        //   what it is, so the artifact cannot have been swapped for a link, a
        //   device or a directory between choosing the name and reading it.
        // * Orivo wrote them, in this installation. The tag is HMAC-SHA256 under
        //   a key that exists only in the system keychain, over the artifact,
        //   the source digest and the engine fingerprint — so a process that can
        //   write here but not read that key cannot produce bytes that reach
        //   this line, and another installation's artifact does not verify.
        // * This engine can load them. The tag binds the engine fingerprint,
        //   which includes the Wasmtime crate version and every `Config` field
        //   Wasmtime itself considers compilation-relevant; `deserialize` then
        //   re-checks the artifact's own ELF header against this engine before
        //   publishing any code, and that check is stricter than nothing but
        //   looser than this one — it compares only Wasmtime's *major* version.
        //
        // The panic guard is not part of the argument. The bytes are already
        // proven ours, so a panic in the loader is a Wasmtime bug rather than an
        // attack; it is caught for the same reason `PluginRuntime::compile` is
        // caught — a bug that quietly recompiles a component is invisible, and one
        // that unwinds out of a blocking worker is a plugin surface that stops
        // answering.
        let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            Component::deserialize(&self.engine, artifact)
        }));
        match loaded {
            Ok(Ok(component)) => Some(component),
            // Wasmtime refused an artifact this module vouched for: the file is
            // ours but no longer loadable here. Regenerating is the answer, and
            // keeping it would mean paying for the refusal again every time.
            Ok(Err(_)) | Err(_) => self.refuse(&path, &self.counters.unloadable),
        }
    }

    /// Forget one artifact and say why it will be recompiled. Removing it is the
    /// point: an artifact that cannot be used must not be re-read and re-refused
    /// on every open, and it costs a compile to replace either way.
    fn refuse(&self, path: &Path, counter: &AtomicU64) -> Option<Component> {
        counter.fetch_add(1, Ordering::Relaxed);
        let _ = fs::remove_file(path);
        None
    }

    /// Best effort throughout. A cache that cannot be written is a cache that is
    /// not used, which is the state Orivo was in before this module existed —
    /// so a full disk, a read-only directory or a refused `rename` costs a
    /// recompilation and is never surfaced to the user.
    fn store(&self, digest: &[u8; 32], component: &Component) {
        let Ok(artifact) = component.serialize() else {
            return;
        };
        if artifact.len() as u64 > self.limits.max_artifact_bytes {
            return;
        }
        if fs::create_dir_all(&self.directory).is_err() {
            return;
        }
        restrict_to_owner(&self.directory);

        let temporary = self.directory.join(format!(
            "{TEMPORARY_PREFIX}{}.{}",
            std::process::id(),
            unique_suffix()
        ));
        let written = (|| -> io::Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(ARTIFACT_MAGIC)?;
            file.write_all(&self.tag(digest, &artifact))?;
            file.write_all(&artifact)?;
            file.sync_all()?;
            // Two workers can prepare the same component at once. Each writes
            // its own temporary file and renames it over the slot, and `rename`
            // is atomic: a reader sees one whole artifact or the other, never a
            // file that is being appended to.
            fs::rename(&temporary, self.directory.join(self.slot_name(digest)))
        })();
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
            return;
        }
        self.counters.stores.fetch_add(1, Ordering::Relaxed);
        self.enforce_ceiling();
    }

    /// Bring the directory back under its ceiling, oldest artifact first — and
    /// drop anything a previous engine generation left behind on the way.
    ///
    /// Oldest-written rather than least-recently-used, because a hit would
    /// otherwise have to write to the file it just read — a syscall on the fast
    /// path to reorder a list whose members are "the plugins this user has
    /// installed". When that set outgrows the ceiling, which is what eviction is
    /// for, the honest answer is that every entry is about to be recompiled
    /// anyway.
    fn enforce_ceiling(&self) {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return;
        };
        let now = SystemTime::now();
        let generation = format!("{}-", self.generation());
        let mut artifacts: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        let mut total = 0u64;
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            if name.starts_with(TEMPORARY_PREFIX) {
                // A write interrupted by a crash or a kill leaves one behind,
                // and nothing else ever will.
                if now
                    .duration_since(modified)
                    .is_ok_and(|age| age > TEMPORARY_LIFETIME)
                {
                    let _ = fs::remove_file(&path);
                }
                continue;
            }
            if !name.ends_with(ARTIFACT_SUFFIX) {
                continue;
            }
            // An artifact from another engine generation cannot be loaded by this
            // one, ever. It is not a cache entry, it is a leftover, and a
            // Wasmtime upgrade would otherwise leave a whole cache of them behind
            // until the ceiling below happened to reclaim them one at a time.
            if !name.starts_with(&generation) {
                let _ = fs::remove_file(&path);
                continue;
            }
            total = total.saturating_add(metadata.len());
            artifacts.push((modified, metadata.len(), path));
        }
        if total <= self.limits.max_total_bytes {
            return;
        }
        artifacts.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.2.cmp(&right.2)));
        for (_, size, path) in artifacts {
            if total <= self.limits.max_total_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }
}

/// A fingerprint of everything that decides whether an artifact is loadable.
///
/// `Engine::precompile_compatibility_hash` is Wasmtime's own answer to "would a
/// binary from that engine deserialise in this one": the target triple, the
/// Cranelift flags, the tunables, the enabled wasm features, and — through
/// `Config::module_version` — the full version of the Wasmtime crate. Feeding it
/// through SHA-256 rather than `DefaultHasher` is deliberate: this digest is
/// part of what decides whether machine code is reloaded, and 64 bits chosen by
/// a hasher whose algorithm the standard library explicitly declines to promise
/// is the wrong width and the wrong guarantee.
fn engine_fingerprint(engine: &Engine) -> [u8; 32] {
    let mut hasher = DigestHasher::default();
    std::hash::Hash::hash(&engine.precompile_compatibility_hash(), &mut hasher);
    hasher.finish_digest()
}

/// A `std::hash::Hasher` that is really SHA-256. Every write is length-prefixed
/// so two adjacent fields cannot produce the same stream as one longer field.
#[derive(Default)]
struct DigestHasher(Sha256);

impl DigestHasher {
    fn finish_digest(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

impl std::hash::Hasher for DigestHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0.update((bytes.len() as u64).to_le_bytes());
        self.0.update(bytes);
    }

    /// Never used: `Hash::hash` only writes, and the digest is taken with
    /// [`DigestHasher::finish_digest`]. Truncating to the trait's `u64` here
    /// would throw away the width this hasher exists to keep.
    fn finish(&self) -> u64 {
        let digest = self.0.clone().finalize();
        u64::from_le_bytes(digest[..8].try_into().unwrap_or_default())
    }
}

fn digest_of(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Read a path that must be a regular file, without following a link.
///
/// `symlink_metadata` followed by a read would check one object and read
/// another; the descriptor is asked instead, the same way `plugin_runtime.rs`
/// reads a file inside a granted folder. `take(ceiling + 1)` is what makes an
/// oversized file visible rather than silently truncated.
fn read_regular_file(path: &Path, ceiling: u64) -> Option<Vec<u8>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Opens a junction or a symbolic link as itself, so the check below sees
        // the link rather than whatever it points at.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(ceiling.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= ceiling).then_some(bytes)
}

/// Nobody but this user needs to read compiled components, and the tag is not an
/// excuse to publish them. Windows inherits the app data ACL, which is already
/// per-user.
fn restrict_to_owner(directory: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(directory, fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = directory;
}

fn unique_suffix() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}.{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

// ---------------------------------------------------------------------------
// The install key
// ---------------------------------------------------------------------------

fn encode_key(key: &[u8; KEY_BYTES]) -> String {
    key.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_key(stored: &str) -> Option<[u8; KEY_BYTES]> {
    let stored = stored.trim();
    if stored.len() != KEY_BYTES * 2 {
        return None;
    }
    let mut key = [0u8; KEY_BYTES];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(stored.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(key)
}

/// The per-installation key, read once and memoised.
///
/// Lazily, and this matters: macOS asks for the keychain password every time an
/// application whose code signature it does not recognise reads an item, which
/// is why `sources.rs` keeps a secret-free directory of connected stores. The
/// cache is opened by the first component compilation, and nothing on the
/// startup, navigation or search path compiles a component — so a session that
/// never opens a plugin surface never touches the keychain for this.
fn install_key() -> Option<[u8; KEY_BYTES]> {
    static KEY: OnceLock<Option<[u8; KEY_BYTES]>> = OnceLock::new();
    *KEY.get_or_init(read_or_create_install_key)
}

fn read_or_create_install_key() -> Option<[u8; KEY_BYTES]> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).ok()?;
    match entry.get_password() {
        Ok(stored) => match decode_key(&stored) {
            Some(key) => Some(key),
            // A damaged entry is replaced rather than refused. Everything it
            // authenticated is regenerable, and refusing would leave the cache
            // off until someone deleted a keychain item by hand.
            None => create_install_key(&entry),
        },
        Err(keyring::Error::NoEntry) => create_install_key(&entry),
        Err(error) => {
            // The value is never logged. That it could not be read is enough to
            // explain a locked keychain or a denied ACL.
            eprintln!(
                "orivo: the plugin compile cache key is unavailable ({error}); components will be compiled on every open"
            );
            None
        }
    }
}

fn create_install_key(entry: &keyring::Entry) -> Option<[u8; KEY_BYTES]> {
    let mut key = [0u8; KEY_BYTES];
    if getrandom::fill(&mut key).is_err() {
        return None;
    }
    // A key that does not survive the session is worse than none: every artifact
    // written under it would be refused and deleted on the next run, so the cache
    // would never pay for itself and the directory would churn.
    if let Err(error) = entry.set_password(&encode_key(&key)) {
        eprintln!(
            "orivo: the plugin compile cache key could not be stored ({error}); components will be compiled on every open"
        );
        return None;
    }
    Some(key)
}

// ---------------------------------------------------------------------------
// The process-wide cache
// ---------------------------------------------------------------------------

static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// Names the cache directory, once, from `lib.rs` during setup.
///
/// Nothing is read, created or unlocked here. Naming a directory is all this
/// does, so setup stays free and the plan's first promise — the shell appears
/// without waiting for a plugin — is not spent on a cache.
pub fn configure(directory: PathBuf) {
    let _ = DIRECTORY.set(directory);
}

/// The cache for `engine`, or `None` when no directory was configured or the
/// install key is unavailable. `None` is the whole feature off: every caller
/// behaves exactly as it did before this module existed.
pub fn shared(engine: &Engine) -> Option<ComponentCache> {
    let directory = DIRECTORY.get()?.clone();
    Some(ComponentCache::open(
        engine.clone(),
        directory,
        install_key()?,
        CacheLimits::default(),
    ))
}

/// Delete every artifact in the configured directory, and report how many.
///
/// The plugin plan's sixth promise: disabling or removing a plugin keeps the
/// games, the preferences and the sessions, and only its regenerable caches may
/// be deleted. This is that door, and it is always safe to open — a component
/// whose artifact is gone is compiled again.
pub fn purge() -> Result<usize, String> {
    let Some(directory) = DIRECTORY.get() else {
        return Ok(0);
    };
    purge_directory(directory)
}

/// Only this module's own files, by name. The directory belongs to Orivo, but a
/// purge that removed whatever it found there would be a purge that could be
/// pointed at something else by a future mistake in [`configure`].
fn purge_directory(directory: &Path) -> Result<usize, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("Orivo could not open its plugin cache: {error}")),
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(ARTIFACT_SUFFIX) && !name.starts_with(TEMPORARY_PREFIX) {
            continue;
        }
        if !entry.metadata().is_ok_and(|metadata| metadata.is_file()) {
            continue;
        }
        if fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_runtime::{EpochMode, PluginLimits, PluginRuntime};
    use std::sync::{Arc, atomic::AtomicUsize};

    /// The real reference runner from P1/P2. An artifact measured against eight
    /// zero bytes would prove nothing about a component someone might install.
    const FIXTURE: &[u8] = include_bytes!("../fixtures/orivo-runner-fixture.wasm");
    /// A second, unrelated component, so "another component's artifact" is a real
    /// artifact and not a mutation of the first one.
    const SECOND: &[u8] = include_bytes!("../fixtures/composed-memories.wasm");

    const KEY_A: [u8; KEY_BYTES] = [0x11; KEY_BYTES];
    const KEY_B: [u8; KEY_BYTES] = [0x22; KEY_BYTES];

    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "orivo-compile-cache-{label}-{}-{}",
                std::process::id(),
                unique_suffix()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn cache_dir(&self) -> PathBuf {
            self.path.join("artifacts")
        }

        fn artifacts(&self) -> Vec<PathBuf> {
            let Ok(entries) = fs::read_dir(self.cache_dir()) else {
                return Vec::new();
            };
            let mut paths: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "cwasm")
                })
                .collect();
            paths.sort();
            paths
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Manual epochs: none of these tests runs guest code, so a ticking thread
    /// per runtime would only be seven more threads on a shared machine.
    fn runtime() -> PluginRuntime {
        PluginRuntime::with_limits(PluginLimits::default(), EpochMode::Manual)
            .expect("an engine is available")
    }

    /// Counts what the cache could not answer. A cache is invisible when it
    /// works, so every assertion below is really an assertion about this number.
    struct Compiler {
        engine: Engine,
        calls: AtomicUsize,
    }

    impl Compiler {
        fn new(runtime: &PluginRuntime) -> Self {
            Self {
                engine: runtime.engine().clone(),
                calls: AtomicUsize::new(0),
            }
        }

        fn compile(&self, bytes: &[u8]) -> Result<Component, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Component::new(&self.engine, bytes).map_err(|error| error.to_string())
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    fn open_cache(
        runtime: &PluginRuntime,
        directory: PathBuf,
        key: [u8; KEY_BYTES],
    ) -> ComponentCache {
        ComponentCache::open(
            runtime.engine().clone(),
            directory,
            key,
            CacheLimits::default(),
        )
    }

    // -----------------------------------------------------------------------
    // The reason the module exists
    // -----------------------------------------------------------------------

    #[test]
    fn a_warm_artifact_is_reused_instead_of_recompiled() {
        let scratch = Scratch::new("warm");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);

        let cold = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        cold.component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(compiler.calls(), 1);
        assert_eq!(cold.counts().stores, 1);

        // A second process, not a second call: the point is that the artifact
        // survives the runtime that wrote it.
        let warm = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        warm.component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(compiler.calls(), 1, "the warm artifact was recompiled");
        assert_eq!(warm.counts().hits, 1);
        assert_eq!(scratch.artifacts().len(), 1);
    }

    #[test]
    fn prepare_component_goes_through_the_cache() {
        let scratch = Scratch::new("hook");
        let digest = format!("{:x}", Sha256::digest(FIXTURE));

        let cold = runtime();
        cold.use_compile_cache(open_cache(&cold, scratch.cache_dir(), KEY_A));
        cold.prepare_component(FIXTURE, &digest).unwrap();

        let warm = runtime();
        warm.use_compile_cache(open_cache(&warm, scratch.cache_dir(), KEY_A));
        let prepared = warm.prepare_component(FIXTURE, &digest).unwrap();
        assert_eq!(
            warm.compile_cache_counts().map(|counts| counts.hits),
            Some(1),
            "prepare_component did not reuse the artifact"
        );

        // The artifact is not merely bytes that verified: it has to still be a
        // component the host can read a contract out of, which is the next thing
        // discovery does with it. Compared against a freshly compiled one rather
        // than against a hand-written expectation, so the assertion is "the same
        // component" and not "some component".
        let contract = warm
            .inspect_contract(&prepared)
            .expect("a reloaded artifact is still a component");
        let fresh = runtime();
        let compiled = fresh.prepare_component(FIXTURE, &digest).unwrap();
        assert_eq!(contract, fresh.inspect_contract(&compiled).unwrap());
        assert!(!contract.required_capabilities.is_empty());
        assert_eq!(
            scratch.artifacts().len(),
            1,
            "the hook did not leave exactly one artifact"
        );
    }

    #[test]
    fn opening_a_cache_touches_nothing_on_disk() {
        let scratch = Scratch::new("lazy");
        let runtime = runtime();
        let opened = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        assert!(
            !scratch.cache_dir().exists(),
            "opening the cache created its directory"
        );
        assert_eq!(opened.counts().stores, 0);
    }

    // -----------------------------------------------------------------------
    // Invalidation
    // -----------------------------------------------------------------------

    #[test]
    fn changed_source_bytes_are_compiled_again() {
        let scratch = Scratch::new("source");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        let cache = open_cache(&runtime, scratch.cache_dir(), KEY_A);

        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        cache
            .component(SECOND, || compiler.compile(SECOND))
            .unwrap();
        assert_eq!(compiler.calls(), 2);
        assert_eq!(scratch.artifacts().len(), 2, "two sources, two slots");

        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(compiler.calls(), 2, "the first source lost its artifact");
        assert_eq!(cache.counts().hits, 1);
    }

    #[test]
    fn a_different_engine_configuration_does_not_reuse_an_artifact() {
        let scratch = Scratch::new("engine");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        open_cache(&runtime, scratch.cache_dir(), KEY_A)
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(compiler.calls(), 1);

        // Fuel off is a tunable, so Wasmtime's own compatibility hash moves and
        // the artifact is not even looked for under the same name.
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true).consume_fuel(false);
        let other = Engine::new(&config).unwrap();
        let other_cache = ComponentCache::open(
            other.clone(),
            scratch.cache_dir(),
            KEY_A,
            CacheLimits::default(),
        );
        other_cache
            .component(FIXTURE, || {
                Component::new(&other, FIXTURE).map_err(|error| error.to_string())
            })
            .unwrap();
        assert_eq!(
            other_cache.counts().hits,
            0,
            "a foreign artifact was loaded"
        );
        assert_eq!(other_cache.counts().misses, 1);
        assert!(
            scratch
                .artifacts()
                .iter()
                .any(|path| path.ends_with(other_cache.slot_name(&digest_of(FIXTURE)))),
            "the second engine kept no artifact of its own"
        );
    }

    #[test]
    fn artifacts_from_another_engine_generation_are_reclaimed() {
        let scratch = Scratch::new("generation");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        open_cache(&runtime, scratch.cache_dir(), KEY_A)
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(scratch.artifacts().len(), 1);

        // A Wasmtime upgrade or a `Config` change is this, on a user's disk: the
        // old artifact can never be loaded again, so the next write reclaims it
        // rather than waiting for the whole-cache ceiling.
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true).consume_fuel(false);
        let next = Engine::new(&config).unwrap();
        let next_cache = ComponentCache::open(
            next.clone(),
            scratch.cache_dir(),
            KEY_A,
            CacheLimits::default(),
        );
        next_cache
            .component(FIXTURE, || {
                Component::new(&next, FIXTURE).map_err(|error| error.to_string())
            })
            .unwrap();

        let remaining = scratch.artifacts();
        assert_eq!(remaining.len(), 1, "a dead generation was left on disk");
        assert!(
            remaining[0].ends_with(next_cache.slot_name(&digest_of(FIXTURE))),
            "the live generation was reclaimed instead of the dead one"
        );
    }

    #[test]
    fn the_engine_fingerprint_follows_the_configuration() {
        let runtime = runtime();
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true).consume_fuel(false);
        let other = Engine::new(&config).unwrap();
        assert_ne!(
            engine_fingerprint(runtime.engine()),
            engine_fingerprint(&other)
        );
        assert_eq!(
            engine_fingerprint(runtime.engine()),
            engine_fingerprint(runtime.engine()),
            "the fingerprint is not stable within one engine"
        );
    }

    // -----------------------------------------------------------------------
    // Integrity — the part that is not about speed
    // -----------------------------------------------------------------------

    #[test]
    fn one_flipped_byte_in_the_artifact_is_refused_and_recompiled() {
        let scratch = Scratch::new("flip");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        open_cache(&runtime, scratch.cache_dir(), KEY_A)
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();

        let slot = scratch.artifacts().remove(0);
        let mut stored = fs::read(&slot).unwrap();
        // Well inside the machine code, not in a field this module reads.
        let target = HEADER_BYTES + stored[HEADER_BYTES..].len() / 2;
        stored[target] ^= 0x01;
        fs::write(&slot, &stored).unwrap();

        let after = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        after
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(after.counts().hits, 0, "an altered artifact was loaded");
        assert_eq!(after.counts().unauthenticated, 1);
        assert_eq!(compiler.calls(), 2);
        // Replaced, not merely refused: a file that cannot be used must not be
        // read and refused again on every open.
        assert_eq!(scratch.artifacts().len(), 1);
        assert_ne!(fs::read(&slot).unwrap(), stored);
    }

    #[test]
    fn one_flipped_byte_in_the_tag_is_refused() {
        let scratch = Scratch::new("tag");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        open_cache(&runtime, scratch.cache_dir(), KEY_A)
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();

        let slot = scratch.artifacts().remove(0);
        let mut stored = fs::read(&slot).unwrap();
        stored[ARTIFACT_MAGIC.len()] ^= 0x80;
        fs::write(&slot, &stored).unwrap();

        let after = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        after
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(
            after.counts().unauthenticated,
            1,
            "an altered tag was accepted"
        );
        assert_eq!(compiler.calls(), 2);
    }

    #[test]
    fn an_artifact_from_another_installation_is_refused() {
        let scratch = Scratch::new("foreign-key");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        open_cache(&runtime, scratch.cache_dir(), KEY_A)
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(scratch.artifacts().len(), 1);

        // Same directory, same engine, same component — another installation's
        // key. Copying a cache directory between machines must buy nothing.
        let elsewhere = open_cache(&runtime, scratch.cache_dir(), KEY_B);
        elsewhere
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(
            elsewhere.counts().hits,
            0,
            "another installation's artifact was deserialised"
        );
        assert_eq!(elsewhere.counts().unauthenticated, 1);
        assert_eq!(compiler.calls(), 2);
    }

    #[test]
    fn an_artifact_moved_onto_another_components_slot_is_refused() {
        let scratch = Scratch::new("substitute");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        let cache = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        cache
            .component(SECOND, || compiler.compile(SECOND))
            .unwrap();

        let fixture_slot = scratch
            .cache_dir()
            .join(cache.slot_name(&digest_of(FIXTURE)));
        let second_slot = scratch
            .cache_dir()
            .join(cache.slot_name(&digest_of(SECOND)));
        fs::copy(&fixture_slot, &second_slot).unwrap();

        // Every byte of this file was written by Orivo under the live key. Only
        // the tag's binding to the source digest tells the two slots apart.
        let after = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        after
            .component(SECOND, || compiler.compile(SECOND))
            .unwrap();
        assert_eq!(after.counts().hits, 0, "one component ran another's code");
        assert_eq!(after.counts().unauthenticated, 1);
        assert_eq!(compiler.calls(), 3);
    }

    #[test]
    fn an_artifact_from_another_engine_is_refused_before_wasmtime_sees_it() {
        let scratch = Scratch::new("cross-engine");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        let live = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        live.component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();

        // Another engine's artifact, authentically tagged by this installation,
        // renamed into the live engine's slot. Wasmtime would refuse it too — but
        // its own header check compares only the *major* Wasmtime version, so
        // "Wasmtime will notice" is not a guarantee this module gets to lean on.
        // The tag binds the whole engine fingerprint, which is why the refusal
        // below is `unauthenticated` and not `unloadable`.
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true).consume_fuel(false);
        let other = Engine::new(&config).unwrap();
        let other_scratch = Scratch::new("cross-engine-other");
        let foreign = ComponentCache::open(
            other.clone(),
            other_scratch.cache_dir(),
            KEY_A,
            CacheLimits::default(),
        );
        foreign
            .component(FIXTURE, || {
                Component::new(&other, FIXTURE).map_err(|error| error.to_string())
            })
            .unwrap();
        fs::rename(
            other_scratch
                .cache_dir()
                .join(foreign.slot_name(&digest_of(FIXTURE))),
            scratch
                .cache_dir()
                .join(live.slot_name(&digest_of(FIXTURE))),
        )
        .unwrap();

        let after = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        after
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(after.counts().hits, 0);
        assert_eq!(
            after.counts().unauthenticated,
            1,
            "the tag did not refuse a foreign engine's artifact"
        );
        assert_eq!(after.counts().unloadable, 0, "Wasmtime was asked to decide");
    }

    #[test]
    fn a_file_that_is_not_an_artifact_is_refused_rather_than_read() {
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        for (label, body) in [
            ("empty", Vec::new()),
            ("short", b"ORIVO".to_vec()),
            ("header-only", ARTIFACT_MAGIC.to_vec()),
            ("garbage", vec![0x7f; HEADER_BYTES + 4096]),
            ("wrong-magic", {
                let mut bytes = vec![0u8; HEADER_BYTES + 4096];
                bytes[..ARTIFACT_MAGIC.len()].copy_from_slice(b"ORIVOCC0");
                bytes
            }),
        ] {
            let scratch = Scratch::new(&format!("junk-{label}"));
            let cache = open_cache(&runtime, scratch.cache_dir(), KEY_A);
            fs::create_dir_all(scratch.cache_dir()).unwrap();
            fs::write(
                scratch
                    .cache_dir()
                    .join(cache.slot_name(&digest_of(FIXTURE))),
                &body,
            )
            .unwrap();
            let before = compiler.calls();
            cache
                .component(FIXTURE, || compiler.compile(FIXTURE))
                .unwrap_or_else(|error| panic!("{label} became a user-visible failure: {error}"));
            assert_eq!(cache.counts().hits, 0, "{label} was loaded");
            assert_eq!(compiler.calls(), before + 1, "{label} did not recompile");
        }
    }

    #[test]
    fn one_flipped_byte_in_the_magic_is_refused() {
        let scratch = Scratch::new("magic");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        open_cache(&runtime, scratch.cache_dir(), KEY_A)
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();

        // Authentically Orivo's file, in the right slot, with the right tag —
        // only the format marker is wrong. The tag does not cover it, so this is
        // the one thing the magic check alone refuses, and the reason a v2 layout
        // will be a refusal rather than a misread.
        let slot = scratch.artifacts().remove(0);
        let mut stored = fs::read(&slot).unwrap();
        stored[ARTIFACT_MAGIC.len() - 1] ^= 0x01;
        fs::write(&slot, &stored).unwrap();

        let after = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        after
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(after.counts().hits, 0, "a foreign format was read anyway");
        assert_eq!(after.counts().unauthenticated, 1);
        assert_eq!(compiler.calls(), 2);
    }

    #[test]
    fn a_reader_never_sees_a_half_written_artifact() {
        let scratch = Scratch::new("torn");
        let runtime = runtime();
        let cache = Arc::new(open_cache(&runtime, scratch.cache_dir(), KEY_A));
        let digest = digest_of(FIXTURE);
        let component = Component::new(runtime.engine(), FIXTURE).unwrap();
        cache.store(&digest, &component);

        // One worker rewriting the slot while another reads it. `rename` replaces
        // the name rather than the file's contents, so the reader sees the old
        // artifact or the new one and never a prefix of either — which is why
        // every read below must succeed and not one may be refused.
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = {
            let cache = Arc::clone(&cache);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    cache.store(&digest, &component);
                }
            })
        };
        let mut read = 0usize;
        for _ in 0..400 {
            if cache.load(&digest).is_some() {
                read += 1;
            }
        }
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();

        assert_eq!(
            cache.counts().unauthenticated,
            0,
            "a reader caught a torn write"
        );
        assert_eq!(read, 400, "the slot was momentarily unreadable");
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_in_a_slot_is_never_followed() {
        let scratch = Scratch::new("symlink");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        let cache = open_cache(&runtime, scratch.cache_dir(), KEY_A);

        // A valid artifact, out of reach, with a link to it planted in the slot:
        // the name is refused whatever it points at, so a cache directory cannot
        // be made to read a file elsewhere on the disk.
        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        let slot = scratch
            .cache_dir()
            .join(cache.slot_name(&digest_of(FIXTURE)));
        let hidden = scratch.path.join("elsewhere.cwasm");
        fs::rename(&slot, &hidden).unwrap();
        std::os::unix::fs::symlink(&hidden, &slot).unwrap();

        let after = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        after
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(
            after.counts().hits,
            0,
            "a link was followed out of the slot"
        );
        assert_eq!(compiler.calls(), 2);
    }

    // -----------------------------------------------------------------------
    // Bounds, concurrency and a disk that says no
    // -----------------------------------------------------------------------

    #[test]
    fn concurrent_preparation_leaves_one_valid_artifact() {
        let scratch = Scratch::new("concurrent");
        let runtime = runtime();
        let compiler = Arc::new(Compiler::new(&runtime));
        let cache = Arc::new(open_cache(&runtime, scratch.cache_dir(), KEY_A));

        let workers: Vec<_> = (0..6)
            .map(|_| {
                let cache = Arc::clone(&cache);
                let compiler = Arc::clone(&compiler);
                std::thread::spawn(move || {
                    cache
                        .component(FIXTURE, || compiler.compile(FIXTURE))
                        .map(|_| ())
                })
            })
            .collect();
        for worker in workers {
            worker
                .join()
                .unwrap()
                .expect("every caller got a component");
        }

        // One slot, whoever won the rename — and it verifies, which is the half
        // that a partially written file would fail.
        assert_eq!(scratch.artifacts().len(), 1);
        let after = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        after
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(after.counts().hits, 1, "the artifact left behind was torn");
    }

    #[test]
    fn a_directory_that_cannot_be_written_still_returns_a_component() {
        let scratch = Scratch::new("no-space");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        // A regular file where the cache directory should be. `create_dir_all`
        // fails the way it fails on a full or read-only disk, and the caller must
        // not be able to tell.
        let blocked = scratch.path.join("blocked");
        fs::write(&blocked, b"not a directory").unwrap();

        let cache = open_cache(&runtime, blocked.join("artifacts"), KEY_A);
        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .expect("a cache that cannot be written is not a failure");
        assert_eq!(cache.counts().stores, 0);
        assert_eq!(compiler.calls(), 1);

        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(compiler.calls(), 2, "nothing could have been cached");
    }

    #[test]
    fn an_artifact_larger_than_its_ceiling_is_never_written() {
        let scratch = Scratch::new("too-big");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        let cache = ComponentCache::open(
            runtime.engine().clone(),
            scratch.cache_dir(),
            KEY_A,
            CacheLimits {
                max_artifact_bytes: 1,
                max_total_bytes: 1024 * 1024,
            },
        );
        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(cache.counts().stores, 0);
        assert!(scratch.artifacts().is_empty());
    }

    #[test]
    fn the_cache_stays_under_its_total_ceiling() {
        let scratch = Scratch::new("ceiling");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);

        // Sized on the *larger* of the two artifacts, so the survivor is a
        // survivor rather than the only one that ever fit: the pair is over the
        // ceiling, and evicting the older one brings the total back under it in
        // exactly one step.
        let measured = Scratch::new("ceiling-probe");
        let probe = open_cache(&runtime, measured.cache_dir(), KEY_A);
        probe
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        let one = fs::metadata(&measured.artifacts()[0]).unwrap().len();

        let cache = ComponentCache::open(
            runtime.engine().clone(),
            scratch.cache_dir(),
            KEY_A,
            CacheLimits {
                max_artifact_bytes: 64 * 1024 * 1024,
                max_total_bytes: one,
            },
        );
        cache
            .component(SECOND, || compiler.compile(SECOND))
            .unwrap();
        // Modification times are the eviction order, so the two writes must be
        // distinguishable by the filesystem's clock.
        std::thread::sleep(Duration::from_millis(20));
        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();

        let remaining = scratch.artifacts();
        assert_eq!(remaining.len(), 1, "the ceiling did not hold");
        assert_eq!(
            remaining[0],
            scratch
                .cache_dir()
                .join(cache.slot_name(&digest_of(FIXTURE))),
            "eviction took the newest artifact instead of the oldest"
        );
    }

    #[test]
    fn purging_removes_every_artifact_and_nothing_else() {
        let scratch = Scratch::new("purge");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        let cache = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        cache
            .component(SECOND, || compiler.compile(SECOND))
            .unwrap();
        assert_eq!(scratch.artifacts().len(), 2);

        let bystander = scratch.cache_dir().join("README.txt");
        fs::write(&bystander, b"not ours").unwrap();

        assert_eq!(purge_directory(&scratch.cache_dir()), Ok(2));
        assert!(scratch.artifacts().is_empty());
        assert!(bystander.is_file(), "purge deleted a file it did not write");

        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(compiler.calls(), 3, "a purged component was not recompiled");
    }

    #[test]
    fn purging_a_directory_that_was_never_created_is_not_an_error() {
        let scratch = Scratch::new("purge-missing");
        assert_eq!(purge_directory(&scratch.cache_dir()), Ok(0));
    }

    // -----------------------------------------------------------------------
    // The install key
    // -----------------------------------------------------------------------

    #[test]
    fn a_key_survives_the_round_trip_through_its_stored_form() {
        let key = [
            0x00, 0x01, 0x02, 0x7f, 0x80, 0xfe, 0xff, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16,
            0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24,
            0x25, 0x26, 0x27, 0x28,
        ];
        let encoded = encode_key(&key);
        assert_eq!(encoded.len(), KEY_BYTES * 2);
        assert_eq!(decode_key(&encoded), Some(key));
        assert_eq!(decode_key(&format!("  {encoded}\n")), Some(key));
    }

    #[test]
    fn a_stored_key_that_is_not_thirty_two_bytes_of_hex_is_refused() {
        for damaged in [
            "",
            "beef",
            &"aa".repeat(KEY_BYTES - 1),
            &"aa".repeat(KEY_BYTES + 1),
            &format!("{}zz", "aa".repeat(KEY_BYTES - 1)),
            &format!("{} ", "aa".repeat(KEY_BYTES - 1)),
        ] {
            assert_eq!(decode_key(damaged), None, "{damaged:?} decoded to a key");
        }
    }

    /// The keychain is the one thing these tests do not touch. Reading or writing
    /// an item from a `cargo test` binary asks macOS to trust code it has never
    /// seen, which is a password prompt on a machine nobody is watching — the
    /// same reason `sources.rs` keeps its connection directory secret-free. What
    /// the keychain path does is decode, generate and store, and the first two
    /// are covered above; this pins the third's shape without opening a store.
    #[test]
    fn a_generated_key_is_thirty_two_random_bytes() {
        let mut first = [0u8; KEY_BYTES];
        let mut second = [0u8; KEY_BYTES];
        getrandom::fill(&mut first).unwrap();
        getrandom::fill(&mut second).unwrap();
        assert_ne!(first, [0u8; KEY_BYTES]);
        assert_ne!(first, second);
        assert_eq!(decode_key(&encode_key(&first)), Some(first));
    }
}
