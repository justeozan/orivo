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
//! maps them executable without revalidating anything a compiler would have
//! validated. A cache file that arrives from anywhere but Orivo would therefore
//! be a way to run arbitrary native code inside Orivo, with all of Orivo's
//! authority — worse than anything a plugin can do, because a plugin is behind a
//! sandbox and this would not be.
//!
//! So the rule here is narrow and absolute: **an artifact is deserialised only
//! when an HMAC-SHA256 tag, keyed by a 256-bit value kept in the system
//! keychain, says Orivo wrote it — for this component and this engine.** The tag
//! covers a domain string, a fingerprint of the `Engine`, the SHA-256 of the
//! source component and the artifact itself.
//!
//! ## What that key is, and what it is not
//!
//! It is **confidential storage, not an authenticated channel**, and the
//! difference decides what this cache can promise.
//!
//! What the tag does refuse, and these are the failures that actually happen:
//!
//! * an artifact corrupted, truncated or half-written;
//! * an artifact left by a previous Wasmtime or a changed `Engine` configuration,
//!   which would otherwise be loaded by a version that cannot run it;
//! * an artifact carried in from **another installation or another user account**
//!   — a copied cache directory, a restored backup, a synchronised folder — since
//!   it is tagged with another key, and across accounts the keychain item is not
//!   readable at all;
//! * an artifact one component's slot borrowed from another's;
//! * **a plugin forging one.** A plugin gets `files.read` on folders the user
//!   picked and no write anywhere, so it cannot even place the file — and if it
//!   could, it has no way to reach the key.
//!
//! What it does **not** refuse, on any platform: **another program running as
//! this user.**
//!
//! * On Linux the Secret Service and on Windows the Credential Manager hand a
//!   stored secret to any process of the same user. The key is simply readable.
//! * On macOS the keyring crate looks in the user's *default* keychain only
//!   (`apple-native-keyring-store`, `keychain.rs`), so a program of this user can
//!   create `io.orivo.desktop.plugin-compile-cache.v1` before Orivo ever runs —
//!   `security add-generic-password -A` — and know the key Orivo will read; or
//!   make its own keychain the default (`security default-keychain -s`, no
//!   administrator rights) and receive the item Orivo creates. An ad-hoc-signed
//!   build has no stable code identity for a keychain ACL to name, so the ACL is
//!   not a boundary either.
//!
//! This is stated rather than papered over because the same attacker can already
//! replace Orivo itself: macOS builds ship without a Developer ID signature or
//! the hardened runtime, and the Windows installation is per-user. A program that
//! can write to this directory *and* read that keychain item can already edit the
//! binary that reads both. The cache is therefore not the weakest link today, and
//! it must not become one: when macOS builds are Developer ID signed with the
//! hardened runtime, this key has to move to the data-protection keychain behind
//! an access group, or it will be. That is recorded as a follow-up on the pull
//! request that introduced this module, not as a comment nobody will find.
//!
//! One thing in that shape *was* a defect of this module rather than of the
//! platform, and is fixed: replacing a damaged entry deletes it and creates a new
//! one, and **gives up if the delete fails** rather than falling back to a write.
//! `SecKeychain::set_generic_password` finds an existing item and rewrites its
//! password in place, keeping that item's access control list, so an item planted
//! with garbage and an "any application" ACL would otherwise have been handed
//! Orivo's real key. What that closes is the in-place rewrite and nothing more:
//! a program that recreates the item between the delete and the write, or that
//! planted a plausible key in the first place, is the same residual case as
//! everything else above. See [`create_key`].
//!
//! ## Everything else follows from "an artifact is regenerable"
//!
//! Missing, corrupted, truncated, foreign, oversized, not even a regular file, or
//! from another engine is a silent recompilation from the source bytes the
//! registry already verified — never an error the user sees, and never a doubtful
//! load. The cache is bounded, written atomically, reclaims what a previous
//! engine generation left behind, and can be purged whole at any time, which is
//! the plugin plan's sixth promise: a plugin's caches are the only thing Orivo
//! may delete.
//!
//! And nothing opens it, or the keychain behind it, until the user does something
//! about a plugin. Naming the directory is all `configure` does; a cache exists
//! only once [`permit`] has been called, and that has exactly four callers, each
//! an explicit action: opening Settings › Plugins, asking for a title to be
//! installed, and the two runner doors (the emulator flow's listing, and every
//! path that loads a runner package — a profile, a grant, an import, a launch).
//!
//! Stated that way rather than as "never on the startup path", because the first
//! two attempts at that sentence were both false. The first missed that the
//! startup update check reached `prepare_component` at all; the second permitted
//! from `QuikyService::plugin`, which the Store page calls merely by being
//! rendered — and a user whose start page is the Store renders it at launch.
//! Rendering a page is not a gesture about a plugin. If this list grows, the
//! question to ask of each entry is whether a user could reach it without having
//! asked for anything.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
    /// Refused before Wasmtime was asked: not a regular file, larger than any
    /// artifact may be, the wrong format, or a tag this installation did not
    /// write for this component and this engine.
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

/// Written by hand because `derive` would print [`ComponentCache::key`]. The key
/// is the whole boundary this module rests on, and `{:?}` on a struct that holds
/// one is how it reaches a log line, a panic message or a test snapshot.
impl std::fmt::Debug for ComponentCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComponentCache")
            .field("directory", &self.directory)
            .field("generation", &self.generation())
            .field("limits", &self.limits)
            .field("counters", &self.counters)
            .finish_non_exhaustive()
    }
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
        let stored = match read_slot(&path, self.limits.max_artifact_bytes + HEADER_BYTES as u64) {
            Stored::Bytes(stored) => stored,
            // Nothing to count and nothing to remove: the next store puts an
            // artifact at this name.
            Stored::Absent => return None,
            Stored::Unusable => return self.refuse(&path, &self.counters.unauthenticated),
        };
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
        //   `read_slot` opens with `O_NOFOLLOW` and asks the descriptor what it
        //   is, so the artifact cannot have been swapped for a link, a device or
        //   a directory between choosing the name and reading it.
        // * Orivo wrote them, in this installation, for this component. The tag
        //   is HMAC-SHA256 over the artifact, the source digest and the engine
        //   fingerprint, under a key held in the system keychain — so corruption,
        //   another installation's cache, another component's slot and a plugin's
        //   forgery all stop here. A program running as this user and able to
        //   read that keychain item is *not* stopped, on any platform; the module
        //   header says why that is stated rather than claimed otherwise.
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
    ///
    /// `remove_dir` is the second attempt because a *directory* can be sitting at
    /// a slot name, and `rename` will not replace one. Without the fallback the
    /// slot stays occupied by something that can never be an artifact, and the
    /// component behind it is recompiled on every open for the rest of the
    /// installation's life.
    fn refuse(&self, path: &Path, counter: &AtomicU64) -> Option<Component> {
        counter.fetch_add(1, Ordering::Relaxed);
        if fs::remove_file(path).is_err() {
            let _ = fs::remove_dir(path);
        }
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
        if !self.prepare_directory() {
            return;
        }

        let temporary = self.directory.join(format!(
            "{TEMPORARY_PREFIX}{}.{}",
            std::process::id(),
            unique_suffix()
        ));
        let written = (|| -> io::Result<()> {
            let mut file = create_exclusive(&temporary)?;
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

    /// Make the cache directory exist, prove it is a directory and not something
    /// standing in for one, and leave it readable only by its owner.
    ///
    /// A link in place of the cache directory is not a way to beat this module —
    /// the tag still decides what runs — but two things here act on a *path*
    /// rather than on a descriptor, and a link would aim them elsewhere:
    /// permissions, and the eviction sweep's `remove_file`. The permissions half
    /// is closed by construction, because `fchmod` names the directory the
    /// descriptor already is and `O_NOFOLLOW` means the kernel refused a link
    /// rather than this function checking for one. The sweep is gated by this
    /// call and bounded by its name filter, which is weaker: a swap between the
    /// two is a race with a program of this user, and that is not a race this
    /// module claims to win — see the module header.
    fn prepare_directory(&self) -> bool {
        if fs::create_dir_all(&self.directory).is_err() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::{fs::OpenOptionsExt, io::AsRawFd};

            let Ok(handle) = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&self.directory)
            else {
                return false;
            };
            // SAFETY: an open descriptor this function owns and has not closed.
            // Nobody but this user needs to read compiled components, and the
            // tag is not an excuse to publish them.
            unsafe { libc::fchmod(handle.as_raw_fd(), 0o700) };
            true
        }
        #[cfg(not(unix))]
        {
            // Windows has no `O_NOFOLLOW` for a directory open and the app data
            // tree is already per-user, so the check is the weaker one: refuse a
            // reparse point by name.
            fs::symlink_metadata(&self.directory)
                .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        }
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

/// What was found at a slot name.
enum Stored {
    /// Nothing usable is there, and nothing was learned about why. A plain miss:
    /// the next store puts an artifact at this name, whatever is there now,
    /// because `rename` replaces the name rather than the object.
    Absent,
    /// Something is there and it cannot be an artifact of Orivo's: not a regular
    /// file, or bigger than any artifact is allowed to be. Removed and counted,
    /// like a corrupt one — an object that can never be loaded must not be
    /// re-examined on every open.
    ///
    /// Neither of the two checks that produce this can be isolated by a test, and
    /// that is worth saying rather than hiding: every object an unprivileged
    /// program of this user can put at a slot name is *also* refused by
    /// `O_NONBLOCK` plus the header check or by the tag, so removing either check
    /// leaves the suite green. They earn their place by refusing earlier — before
    /// a device that never ends is read up to the ceiling, and before a
    /// sixty-four-megabyte HMAC is computed over something that was never an
    /// artifact — not by being the only thing that refuses.
    Unusable,
    Bytes(Vec<u8>),
}

/// Read a slot, which must be a regular file, without following a link and
/// without blocking.
///
/// `symlink_metadata` followed by a read would check one object and read
/// another; the descriptor is asked instead, the same way `plugin_runtime.rs`
/// reads a file inside a granted folder. `take(ceiling + 1)` is what makes an
/// oversized file visible rather than silently truncated.
fn read_slot(path: &Path, ceiling: u64) -> Stored {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // `O_NOFOLLOW` refuses a link planted at this name instead of resolving
        // it. `O_NONBLOCK` is the other half, and it is not optional: opening a
        // FIFO for reading *blocks until someone writes to it*, so without this
        // the `is_file` check below never runs and a named pipe dropped into the
        // cache directory parks every discovery pass for the rest of the
        // session. #35 fixed exactly this one directory over, in `read_file`.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Opens a junction or a symbolic link as itself, so the check below sees
        // the link rather than whatever it points at.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    // A refused open — a link (`ELOOP`), a permission, a device that will not
    // answer — is `Absent` rather than `Unusable`, because telling those apart
    // needs a second syscall that would itself be racing whatever made the first
    // one fail. The next store overwrites the name either way.
    let Ok(file) = options.open(path) else {
        return Stored::Absent;
    };
    let Ok(metadata) = file.metadata() else {
        return Stored::Absent;
    };
    if !metadata.is_file() {
        return Stored::Unusable;
    }
    let mut bytes = Vec::new();
    if file
        .take(ceiling.saturating_add(1))
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Stored::Unusable;
    }
    if bytes.len() as u64 > ceiling {
        return Stored::Unusable;
    }
    Stored::Bytes(bytes)
}

/// Create a file that must not already exist, and never through a link.
///
/// `create_new` is what refuses an object somebody else put at this name —
/// `O_EXCL` fails on a symbolic link rather than following it — and `O_NOFOLLOW`
/// says the same thing twice on purpose, because the consequence of writing
/// through a link here is a file Orivo believes it owns somewhere it does not.
/// The name is unpredictable as well, but unpredictable is not a check.
fn create_exclusive(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path)
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

/// Where the install key lives, behind a seam.
///
/// Not an abstraction for its own sake. The branches below — a missing entry, a
/// damaged one, a store that will not write, a value that does not read back as
/// what was written — are the whole of this module's key handling, and the real
/// implementation cannot be driven from a test: a `cargo test` binary asking
/// macOS for a keychain item asks it to trust code it has never seen, which is a
/// password prompt on a machine nobody is watching. With the seam, every branch
/// has a test and the platform call has one shape to get right.
trait KeyStore {
    /// `Ok(None)` is "there is no entry"; `Err` is "this store could not be
    /// asked", which are very different answers and must not collapse.
    fn read(&self) -> Result<Option<String>, KeyStoreUnavailable>;
    fn write(&self, value: &str) -> Result<(), KeyStoreUnavailable>;
    fn delete(&self) -> Result<(), KeyStoreUnavailable>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyStoreUnavailable;

struct SystemKeyStore;

impl SystemKeyStore {
    fn entry(&self) -> Result<keyring::Entry, KeyStoreUnavailable> {
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(|_| KeyStoreUnavailable)
    }
}

impl KeyStore for SystemKeyStore {
    fn read(&self) -> Result<Option<String>, KeyStoreUnavailable> {
        match self.entry()?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => {
                // The value is never logged. That it could not be read is enough
                // to explain a locked keychain or a denied ACL.
                eprintln!("orivo: the plugin compile cache key could not be read ({error})");
                Err(KeyStoreUnavailable)
            }
        }
    }

    fn write(&self, value: &str) -> Result<(), KeyStoreUnavailable> {
        self.entry()?.set_password(value).map_err(|error| {
            eprintln!("orivo: the plugin compile cache key could not be stored ({error})");
            KeyStoreUnavailable
        })
    }

    fn delete(&self) -> Result<(), KeyStoreUnavailable> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(KeyStoreUnavailable),
        }
    }
}

/// The per-installation key, read once and memoised.
///
/// Lazily, and this matters twice. macOS asks for the keychain password every
/// time an application whose code signature it does not recognise reads an item,
/// which is why `sources.rs` keeps a secret-free directory of connected stores;
/// and the plugin plan forbids anything on the startup path from depending on a
/// plugin. So this is reached only by [`shared`], which answers `None` until a
/// user-initiated plugin surface has called [`permit`] — see there.
///
/// A store that could not be read once stays unread for the session. The
/// alternative is asking again, which on macOS means a second password prompt
/// for a cache the user did not ask about.
fn install_key() -> Option<[u8; KEY_BYTES]> {
    static KEY: OnceLock<Option<[u8; KEY_BYTES]>> = OnceLock::new();
    *KEY.get_or_init(|| read_or_create_key(&SystemKeyStore))
}

fn read_or_create_key(store: &impl KeyStore) -> Option<[u8; KEY_BYTES]> {
    match store.read() {
        Ok(Some(stored)) => {
            if let Some(key) = decode_key(&stored) {
                return Some(key);
            }
            // A damaged entry is replaced rather than refused: everything it
            // authenticated is regenerable, and refusing would leave the cache
            // off until someone deleted a keychain item by hand.
        }
        Ok(None) => {}
        // A store that cannot be asked must not be *written* either. Creating a
        // key here would be a prompt, or an overwrite, on a path where the user
        // asked for neither.
        Err(KeyStoreUnavailable) => return None,
    }
    create_key(store)
}

fn create_key(store: &impl KeyStore) -> Option<[u8; KEY_BYTES]> {
    let mut key = [0u8; KEY_BYTES];
    if getrandom::fill(&mut key).is_err() {
        return None;
    }
    let encoded = encode_key(&key);

    // Delete first, and **fail closed if the delete fails**. On macOS
    // `SecKeychain::set_generic_password` *finds* an existing item and rewrites
    // its password — `security-framework/src/os/macos/passwords.rs:275-277`,
    // `Ok((_, mut item)) => item.set_password(password)` — which keeps that
    // item's access control list. Another program of this user can create
    // `io.orivo.desktop.plugin-compile-cache.v1` before Orivo ever runs, with
    // garbage in it and an ACL that lets any application read it; writing into
    // that item would hand it Orivo's real key. So a delete that does not
    // succeed is the end of it: no key, no cache, and a compile on every open —
    // which costs milliseconds, where the other costs the key.
    //
    // What this closes is only the *in-place* rewrite. Two things remain open
    // and are not claimed otherwise: a program that recreates the item between
    // this delete and the write below gets the same result, and one that simply
    // planted a valid-looking key in the first place is never noticed at all.
    // Both are the same residual case as the rest of the module header — a
    // program running as this user — and neither is closeable from here: an
    // ad-hoc-signed build has no stable code identity for an ACL to name, and a
    // program that makes its own keychain the default receives whatever Orivo
    // creates.
    if store.delete().is_err() {
        eprintln!(
            "orivo: the plugin compile cache key could not be replaced; components will be compiled on every open"
        );
        return None;
    }
    if store.write(&encoded).is_err() {
        return None;
    }

    // And read it back. This catches a write that landed where the read does not
    // resolve to — another keychain, another item shadowing this one — which
    // would otherwise leave every artifact written this session unverifiable on
    // the next run, and the cache churning forever. It proves nothing about who
    // else can read the item.
    if store.read().ok().flatten().as_deref() != Some(encoded.as_str()) {
        eprintln!(
            "orivo: the plugin compile cache key did not read back as written; components will be compiled on every open"
        );
        return None;
    }
    Some(key)
}

// ---------------------------------------------------------------------------
// The process-wide cache
// ---------------------------------------------------------------------------

static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
static PERMITTED: AtomicBool = AtomicBool::new(false);

/// Names the cache directory, once, from `lib.rs` during setup.
///
/// Nothing is read, created or unlocked here. Naming a directory is all this
/// does, so setup stays free and the plan's first promise — the shell appears
/// without waiting for a plugin — is not spent on a cache.
pub fn configure(directory: PathBuf) {
    let _ = DIRECTORY.set(directory);
}

/// The user has opened something that discovers or runs plugins, so the cache
/// may be used from here on.
///
/// This latch exists because being configured is not the same as being wanted.
/// Orivo's startup task asks the registry what is installed — to see whether a
/// consented automatic update is pending — and that pass reaches the same
/// `prepare_component` a settings panel does. Opening the cache there means
/// reading the install key at launch, which on an ad-hoc-signed build is an
/// unsolicited keychain prompt, and the plan forbids the startup path from
/// touching a plugin's cache at all.
///
/// So the switch is off by default and every caller that does not turn it on
/// gets exactly the behaviour that predates this module: a compile. Fail-closed
/// in the direction that costs milliseconds rather than the one that costs a
/// prompt, which also means a background path added later is cacheless until
/// someone says otherwise, instead of quietly inheriting a key.
///
/// **The bar for a call site is an action, not a surface.** `get_quiky_status`
/// looked like one and is not: the Store page calls it on render, and a user whose
/// start page is the Store renders it at launch, so permitting there read the
/// install key at every start. `start_quiky_install` is the same flow's actual
/// gesture, and that is where it moved.
///
/// A refusal is never cached. `PluginRuntime::compile_cache` remembers a cache it
/// obtained and not a `None`, because this runtime is process-wide: caching the
/// first refusal would mean a background update installing before the user
/// touched anything left the whole session without a cache.
pub fn permit() {
    PERMITTED.store(true, Ordering::Release);
}

/// The cache for `engine`, or `None` when no directory was configured, no
/// user-initiated surface has permitted one, or the install key is unavailable.
/// `None` is the whole feature off: every caller behaves exactly as it did
/// before this module existed.
pub fn shared(engine: &Engine) -> Option<ComponentCache> {
    open_shared(
        engine,
        DIRECTORY.get(),
        PERMITTED.load(Ordering::Acquire),
        install_key,
    )
}

/// The body of [`shared`], with its three inputs as parameters so a test can
/// drive the rule instead of the process.
///
/// The order matters and is the point: permission is decided **before** `key` is
/// called, because reading the install key is the thing that must not happen
/// unasked — on an ad-hoc-signed macOS build it is a password prompt. A test
/// passes a `key` that panics if it is reached.
fn open_shared(
    engine: &Engine,
    configured: Option<&PathBuf>,
    permitted: bool,
    key: impl FnOnce() -> Option<[u8; KEY_BYTES]>,
) -> Option<ComponentCache> {
    if !permitted {
        return None;
    }
    let directory = configured?.clone();
    Some(ComponentCache::open(
        engine.clone(),
        directory,
        key()?,
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

/// A cache directory that is a link is not one Orivo made, and this function
/// deletes files. Refusing by name is weaker than the descriptor check
/// [`ComponentCache::prepare_directory`] does, and it is the strongest thing a
/// free function without a descriptor can do.
fn is_plain_directory(directory: &Path) -> bool {
    fs::symlink_metadata(directory)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

/// Only this module's own files, by name. The directory belongs to Orivo, but a
/// purge that removed whatever it found there would be a purge that could be
/// pointed at something else by a future mistake in [`configure`].
fn purge_directory(directory: &Path) -> Result<usize, String> {
    if directory.exists() && !is_plain_directory(directory) {
        return Err("Orivo's plugin cache is not a directory it wrote.".into());
    }
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

    // -----------------------------------------------------------------------
    // The tag's own construction
    // -----------------------------------------------------------------------

    /// The domain string cannot be shown to matter by planting a file, because
    /// this key tags exactly one kind of thing today. What can be pinned is that
    /// it is *in* the input — and #42 is why it has to be: one key signed a
    /// package and an index, and the package's signature turned out to be a valid
    /// index signature. The next thing this key authenticates must not inherit
    /// that.
    #[test]
    fn the_tag_covers_its_domain_string() {
        let scratch = Scratch::new("tag-domain");
        let runtime = runtime();
        let cache = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        let digest = digest_of(FIXTURE);

        let mut undomained = <ArtifactTag as Mac>::new_from_slice(&KEY_A).unwrap();
        undomained.update(&cache.engine_fingerprint);
        undomained.update(&digest);
        undomained.update(FIXTURE);
        let undomained: [u8; TAG_BYTES] = undomained.finalize().into_bytes().into();

        assert_ne!(
            cache.tag(&digest, FIXTURE),
            undomained,
            "the domain string is not part of the tag"
        );
    }

    /// Same argument one layer out: the slot digest is domain-separated too, so a
    /// second thing ever hashed into a name under this scheme cannot collide with
    /// an artifact slot.
    #[test]
    fn a_slot_name_covers_its_domain_string() {
        let scratch = Scratch::new("slot-domain");
        let runtime = runtime();
        let cache = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        let digest = digest_of(FIXTURE);

        let mut undomained = Sha256::new();
        undomained.update(digest);
        let undomained = format!(
            "{}-{:x}{ARTIFACT_SUFFIX}",
            cache.generation(),
            undomained.finalize()
        );
        assert_ne!(
            cache.slot_name(&digest),
            undomained,
            "the slot digest is not domain-separated"
        );
    }

    // -----------------------------------------------------------------------
    // What the cache will not read, write or print
    // -----------------------------------------------------------------------

    /// A named pipe in a slot used to park the caller forever. `O_RDONLY` on a
    /// FIFO blocks until somebody writes to it, so the `is_file` check that would
    /// have refused it never ran — and the caller is a discovery pass behind
    /// Settings › Plugins.
    ///
    /// This covers the class. `O_NONBLOCK` is what it isolates; `is_file` is not
    /// isolable on its own, for the reason given on [`Stored::Unusable`].
    #[cfg(unix)]
    #[test]
    fn a_named_pipe_in_a_slot_does_not_park_the_caller() {
        let scratch = Scratch::new("fifo");
        let runtime = runtime();
        let cache = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        fs::create_dir_all(scratch.cache_dir()).unwrap();
        let slot = scratch
            .cache_dir()
            .join(cache.slot_name(&digest_of(FIXTURE)));
        let raw = std::ffi::CString::new(slot.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a path this process owns, in a directory it just created.
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o644) }, 0);

        // Bounded, and on another thread, so a regression fails this test instead
        // of hanging the suite with no message.
        let (report, answer) = std::sync::mpsc::channel();
        let engine = runtime.engine().clone();
        std::thread::spawn(move || {
            let outcome = cache.component(FIXTURE, || {
                Component::new(&engine, FIXTURE).map_err(|error| error.to_string())
            });
            let _ = report.send((outcome.is_ok(), cache.counts()));
        });
        let (compiled, counts) = answer
            .recv_timeout(Duration::from_secs(20))
            .expect("the caller is still parked on a named pipe");
        assert!(compiled);
        assert_eq!(counts.hits, 0);
        assert_eq!(counts.unauthenticated, 1);

        // Removed rather than met again on the next open — and the slot now holds
        // the artifact the recompilation produced, which the next cache reuses.
        use std::os::unix::fs::FileTypeExt;
        assert!(
            !fs::symlink_metadata(&slot).unwrap().file_type().is_fifo(),
            "the pipe was left in the cache"
        );
        let compiler = Compiler::new(&runtime);
        let after = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        after
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(after.counts().hits, 1);
    }

    /// The ceiling's job is refusing *early*: the tag would catch an oversized
    /// slot too, after hashing all of it. So this test pins the promise — refused,
    /// removed, and the caller still served — rather than the branch, which no
    /// test can isolate. See [`Stored::Unusable`].
    #[test]
    fn a_file_larger_than_an_artifact_may_be_is_refused_without_being_read_whole() {
        let scratch = Scratch::new("oversized");
        let runtime = runtime();
        let cache = ComponentCache::open(
            runtime.engine().clone(),
            scratch.cache_dir(),
            KEY_A,
            CacheLimits {
                max_artifact_bytes: 4096,
                max_total_bytes: 64 * 1024 * 1024,
            },
        );
        fs::create_dir_all(scratch.cache_dir()).unwrap();
        let slot = scratch
            .cache_dir()
            .join(cache.slot_name(&digest_of(FIXTURE)));
        fs::write(&slot, vec![0x5a; 4096 + HEADER_BYTES + 1]).unwrap();

        let compiler = Compiler::new(&runtime);
        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(cache.counts().hits, 0);
        assert_eq!(
            cache.counts().unauthenticated,
            1,
            "an oversized slot was not refused"
        );
        // Removed rather than re-read on every open: nothing this large can ever
        // become an artifact of ours.
        assert!(!slot.exists(), "the oversized file was left to be re-read");
    }

    /// A directory at a slot name is the one obstruction `rename` cannot clear, so
    /// without the `remove_dir` fallback in [`ComponentCache::refuse`] the slot
    /// stays occupied and its component is recompiled on every open, forever.
    #[test]
    fn a_directory_planted_at_a_slot_name_is_refused_and_cleared() {
        let scratch = Scratch::new("slot-dir");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        let cache = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        fs::create_dir_all(scratch.cache_dir()).unwrap();
        let slot = scratch
            .cache_dir()
            .join(cache.slot_name(&digest_of(FIXTURE)));
        fs::create_dir(&slot).unwrap();

        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .expect("an obstructed slot is not a failure");
        assert_eq!(cache.counts().hits, 0);
        assert_eq!(compiler.calls(), 1);

        // The proof that it was cleared: the next cache finds an artifact there.
        let after = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        after
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(
            after.counts().hits,
            1,
            "the slot is still occupied, so this component recompiles forever"
        );
    }

    /// The read ceiling *can* be isolated, and here is the case that does it: an
    /// artifact tagged correctly by this installation, for this component and this
    /// engine, and one byte larger than any artifact may be. The tag verifies, so
    /// nothing downstream refuses it — without the ceiling those bytes reach
    /// `Component::deserialize`, which is exactly what the ceiling exists to keep
    /// them out of. The refusal must therefore be `unauthenticated` and never
    /// `unloadable`.
    #[test]
    fn an_authentic_artifact_over_the_ceiling_never_reaches_wasmtime() {
        let scratch = Scratch::new("authentic-oversized");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        let cache = ComponentCache::open(
            runtime.engine().clone(),
            scratch.cache_dir(),
            KEY_A,
            CacheLimits {
                max_artifact_bytes: 2048,
                max_total_bytes: 64 * 1024 * 1024,
            },
        );
        let digest = digest_of(FIXTURE);
        let oversized = vec![0x33; 2049];

        fs::create_dir_all(scratch.cache_dir()).unwrap();
        let slot = scratch.cache_dir().join(cache.slot_name(&digest));
        let mut body = Vec::new();
        body.extend_from_slice(ARTIFACT_MAGIC);
        body.extend_from_slice(&cache.tag(&digest, &oversized));
        body.extend_from_slice(&oversized);
        fs::write(&slot, &body).unwrap();

        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(cache.counts().hits, 0);
        assert_eq!(
            cache.counts().unauthenticated,
            1,
            "an authentic artifact over the ceiling was not refused by the ceiling"
        );
        assert_eq!(
            cache.counts().unloadable,
            0,
            "Wasmtime was handed an artifact larger than any artifact may be"
        );
        assert_eq!(compiler.calls(), 1);
    }

    #[test]
    fn the_install_key_is_not_in_the_caches_debug_output() {
        let scratch = Scratch::new("debug");
        let runtime = runtime();
        let cache = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        let printed = format!("{cache:?}");
        assert!(
            !printed.contains(&encode_key(&KEY_A)),
            "the key is in Debug output: {printed}"
        );
        // A byte of it, in the shape a `[u8; 32]` would print, is enough to fail.
        assert!(
            !printed.contains("17, 17, 17"),
            "the key bytes leak: {printed}"
        );
        assert!(printed.contains("ComponentCache"));
    }

    /// A link where the cache directory should be would aim two things somewhere
    /// else: the mode change, and the eviction sweep's `remove_file`. The mode
    /// change is the sharp one — a `chmod 0700` on a directory Orivo does not own.
    #[test]
    fn a_cache_directory_that_is_a_link_is_refused() {
        let scratch = Scratch::new("dir-link");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        let elsewhere = scratch.path.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        let bystander = elsewhere.join("keep-me");
        fs::write(&bystander, b"not ours").unwrap();
        redirect_directory(&scratch.cache_dir(), &elsewhere);

        let cache = open_cache(&runtime, scratch.cache_dir(), KEY_A);
        cache
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .expect("a refused directory is not a failure");
        assert_eq!(
            cache.counts().stores,
            0,
            "an artifact was written through a link"
        );
        assert!(bystander.is_file());
        assert!(
            fs::read_dir(&elsewhere)
                .unwrap()
                .flatten()
                .all(|entry| entry.file_name() == "keep-me"),
            "something was written into the link's target"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                fs::metadata(&elsewhere).unwrap().permissions().mode() & 0o777,
                0o700,
                "the link's target was chmodded"
            );
        }
        assert!(
            purge_directory(&scratch.cache_dir()).is_err(),
            "purge followed the link"
        );
        remove_directory_redirect(&scratch.cache_dir());
    }

    /// A directory redirection in the form each platform lets an unprivileged
    /// program create: a symbolic link on Unix, a junction on Windows. Copied from
    /// `plugin_runtime.rs`, which needed the same distinction in #47.
    fn redirect_directory(link: &Path, target: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;

            // `raw_arg`, because `cmd /C` applies its own quote-stripping and
            // Rust's ordinary escaping produces a form it mangles.
            let output = std::process::Command::new("cmd")
                .raw_arg(format!(
                    "/C mklink /J \"{}\" \"{}\"",
                    link.display(),
                    target.display()
                ))
                .output()
                .expect("cmd is on PATH");
            assert!(
                output.status.success(),
                "mklink /J did not create the junction: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn remove_directory_redirect(link: &Path) {
        #[cfg(unix)]
        let _ = fs::remove_file(link);
        #[cfg(windows)]
        let _ = fs::remove_dir(link);
    }

    #[cfg(unix)]
    #[test]
    fn the_cache_directory_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = Scratch::new("mode");
        let runtime = runtime();
        let compiler = Compiler::new(&runtime);
        // Created wide open first, so the assertion is about this module setting
        // the mode and not about whatever the umask happened to be.
        fs::create_dir_all(scratch.cache_dir()).unwrap();
        fs::set_permissions(scratch.cache_dir(), fs::Permissions::from_mode(0o755)).unwrap();

        open_cache(&runtime, scratch.cache_dir(), KEY_A)
            .component(FIXTURE, || compiler.compile(FIXTURE))
            .unwrap();
        assert_eq!(
            fs::metadata(scratch.cache_dir())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn a_temporary_file_is_never_written_through_a_name_that_exists() {
        let scratch = Scratch::new("exclusive");
        let occupied = scratch.path.join("occupied");
        fs::write(&occupied, b"someone else's").unwrap();
        assert!(create_exclusive(&occupied).is_err());
        assert_eq!(fs::read(&occupied).unwrap(), b"someone else's");

        #[cfg(unix)]
        {
            let target = scratch.path.join("target");
            fs::write(&target, b"outside the cache").unwrap();
            let planted = scratch.path.join("planted");
            std::os::unix::fs::symlink(&target, &planted).unwrap();
            assert!(create_exclusive(&planted).is_err());
            assert_eq!(fs::read(&target).unwrap(), b"outside the cache");
        }
    }

    // -----------------------------------------------------------------------
    // When a cache may open at all
    // -----------------------------------------------------------------------

    /// Configured is not the same as wanted. Orivo's startup task asks the
    /// registry what is installed, and that used to reach the same
    /// `prepare_component` a settings panel does — which with a cache behind it
    /// means reading the install key at launch, and on an ad-hoc-signed macOS
    /// build that is an unsolicited password prompt.
    ///
    /// Driven through [`open_shared`], the body `shared` calls, rather than
    /// through a predicate beside it: a `shared` that read `DIRECTORY` and
    /// ignored `PERMITTED` would leave a test of the predicate alone green.
    #[test]
    fn a_cache_opens_only_once_a_user_surface_has_permitted_one() {
        let scratch = Scratch::new("latch");
        let runtime = runtime();
        let configured = scratch.cache_dir();

        // Not permitted: no cache, and — the half that matters — the key is never
        // even asked for, so nothing prompts.
        assert!(
            open_shared(runtime.engine(), Some(&configured), false, || {
                panic!("the install key was read before anything permitted a cache")
            })
            .is_none(),
            "a configured cache opened before anything asked for it"
        );

        assert!(open_shared(runtime.engine(), None, true, || Some(KEY_A)).is_none());
        assert!(open_shared(runtime.engine(), None, false, || Some(KEY_A)).is_none());

        let opened = open_shared(runtime.engine(), Some(&configured), true, || Some(KEY_A))
            .expect("permitted and configured");
        assert_eq!(opened.directory, configured);
    }

    /// And an unavailable key is a refusal that does not stick: the next call asks
    /// again. `PluginRuntime::compile_cache` used to memoise the first answer for
    /// the life of the process-wide runtime, so a background update installing
    /// before the user touched anything left the whole session uncached.
    #[test]
    fn a_refused_cache_is_not_remembered_by_the_runtime() {
        let scratch = Scratch::new("retry");
        let runtime = runtime();
        let digest = format!("{:x}", Sha256::digest(FIXTURE));

        // No cache attached, so `compile_cache` asks the process-wide rule and is
        // refused — this is the startup shape.
        runtime.prepare_component(FIXTURE, &digest).unwrap();
        assert_eq!(runtime.compile_cache_counts(), None);

        // Now a surface permits one. The same runtime must take it.
        runtime.use_compile_cache(open_cache(&runtime, scratch.cache_dir(), KEY_A));
        runtime.prepare_component(FIXTURE, &digest).unwrap();
        assert_eq!(
            runtime.compile_cache_counts().map(|counts| counts.stores),
            Some(1),
            "the runtime kept refusing after a cache became available"
        );
    }

    // -----------------------------------------------------------------------
    // The install key
    // -----------------------------------------------------------------------

    /// A key store that records what was asked of it, in order.
    ///
    /// The order is the point of two of the tests below: the macOS keychain
    /// rewrites an existing item's password *in place*, keeping that item's access
    /// control list, so replacing a damaged entry has to be a delete followed by a
    /// create and not a write.
    #[derive(Default)]
    struct FakeStore {
        value: std::sync::Mutex<Option<String>>,
        log: std::sync::Mutex<Vec<&'static str>>,
        read_fails: bool,
        write_fails: bool,
        delete_fails: bool,
        /// What the store keeps when asked to write, if not what it was given.
        writes_instead: Option<String>,
    }

    impl FakeStore {
        fn holding(value: Option<&str>) -> Self {
            Self {
                value: std::sync::Mutex::new(value.map(str::to_owned)),
                ..Self::default()
            }
        }

        fn log(&self) -> Vec<&'static str> {
            self.log.lock().unwrap().clone()
        }

        fn stored(&self) -> Option<String> {
            self.value.lock().unwrap().clone()
        }
    }

    impl KeyStore for FakeStore {
        fn read(&self) -> Result<Option<String>, KeyStoreUnavailable> {
            self.log.lock().unwrap().push("read");
            if self.read_fails {
                return Err(KeyStoreUnavailable);
            }
            Ok(self.stored())
        }

        fn write(&self, value: &str) -> Result<(), KeyStoreUnavailable> {
            self.log.lock().unwrap().push("write");
            if self.write_fails {
                return Err(KeyStoreUnavailable);
            }
            *self.value.lock().unwrap() = Some(
                self.writes_instead
                    .clone()
                    .unwrap_or_else(|| value.to_owned()),
            );
            Ok(())
        }

        fn delete(&self) -> Result<(), KeyStoreUnavailable> {
            self.log.lock().unwrap().push("delete");
            if self.delete_fails {
                return Err(KeyStoreUnavailable);
            }
            *self.value.lock().unwrap() = None;
            Ok(())
        }
    }

    #[test]
    fn an_absent_entry_becomes_a_new_key() {
        let store = FakeStore::holding(None);
        let key = read_or_create_key(&store).expect("a key is created");
        assert_eq!(store.stored().as_deref(), Some(encode_key(&key).as_str()));
        assert_eq!(store.log(), vec!["read", "delete", "write", "read"]);
    }

    #[test]
    fn an_existing_key_is_used_as_it_is() {
        let existing = encode_key(&KEY_B);
        let store = FakeStore::holding(Some(&existing));
        assert_eq!(read_or_create_key(&store), Some(KEY_B));
        assert_eq!(store.log(), vec!["read"], "an existing key was rewritten");
        assert_eq!(store.stored().as_deref(), Some(existing.as_str()));
    }

    /// The (c) of the security review: `SecKeychain::set_generic_password` finds an
    /// existing item and rewrites its password, keeping its ACL. A program of this
    /// user can plant `io.orivo.desktop.plugin-compile-cache.v1` with garbage and
    /// an "any application" ACL before Orivo first runs; an in-place update would
    /// then hand it Orivo's real key. The item that holds the key has to be one
    /// this process created, which means the delete comes first.
    #[test]
    fn a_damaged_entry_is_deleted_before_a_new_key_is_written() {
        let store = FakeStore::holding(Some("not a key"));
        let key = read_or_create_key(&store).expect("a key replaces the damaged entry");
        assert_eq!(
            store.log(),
            vec!["read", "delete", "write", "read"],
            "the damaged entry was updated in place"
        );
        assert_eq!(store.stored().as_deref(), Some(encode_key(&key).as_str()));
    }

    #[test]
    fn a_store_that_cannot_be_read_disables_the_cache_without_writing() {
        let store = FakeStore {
            read_fails: true,
            ..FakeStore::holding(None)
        };
        assert_eq!(read_or_create_key(&store), None);
        assert_eq!(
            store.log(),
            vec!["read"],
            "a store that could not be read was written to anyway"
        );
    }

    /// The delete is not a formality. If it fails and the write goes ahead anyway,
    /// macOS rewrites the existing item's password in place and keeps its access
    /// control list — which is the whole defect the delete exists to close, back
    /// again. No delete, no key, no cache.
    #[test]
    fn a_delete_that_fails_leaves_no_key_rather_than_writing_in_place() {
        let store = FakeStore {
            delete_fails: true,
            ..FakeStore::holding(Some("not a key"))
        };
        assert_eq!(read_or_create_key(&store), None);
        assert_eq!(
            store.log(),
            vec!["read", "delete"],
            "the key was written into an item this process did not create"
        );
        assert_eq!(
            store.stored().as_deref(),
            Some("not a key"),
            "the planted value was overwritten"
        );
    }

    #[test]
    fn a_store_that_cannot_be_written_disables_the_cache() {
        let store = FakeStore {
            write_fails: true,
            ..FakeStore::holding(None)
        };
        assert_eq!(read_or_create_key(&store), None);
    }

    /// A write that lands where the read does not resolve to — another keychain,
    /// another item shadowing this one — would leave every artifact written this
    /// session unverifiable on the next run, and the cache churning forever.
    #[test]
    fn a_key_that_does_not_read_back_as_written_is_not_used() {
        let store = FakeStore {
            writes_instead: Some(encode_key(&KEY_A)),
            ..FakeStore::holding(None)
        };
        assert_eq!(read_or_create_key(&store), None);
        assert_eq!(store.log(), vec!["read", "delete", "write", "read"]);
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
