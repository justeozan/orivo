//! Orivo's own plugins, found from their own GitHub releases.
//!
//! The signed index (`plugin_index.rs`) is a curation point: a plugin is in the
//! Store because a maintainer reviewed it and signed a document saying so. For
//! the plugins Orivo publishes itself that step was only a chore, and a chore
//! with a failure mode — the Moonlight plugin stayed at 0.1.0 in the index for
//! seven weeks after 0.2.1 was released, and streaming was broken for everyone
//! in between. So for the plugins the registry compiled into the binary names a
//! `repository` for, Orivo asks GitHub which release is the latest and builds
//! the package URL itself. Community plugins still come only through the index.
//!
//! What this does not move:
//!
//! - **Which plugins are official, and where they live, is compiled in.** Neither
//!   the index nor any response can add an id or name a repository: an index
//!   that could would be a signed document choosing which code skips curation.
//! - **The package has to be signed by Orivo's release key.** Nothing published
//!   a digest for these releases and none is invented
//!   ([`PackageIntegrity::ReleaseSigned`]); the archive's own signature is the
//!   integrity check, and the installer requires it on every registry download.
//!   An archive signed by anyone else, or by no one, is refused.
//! - **Everything after the download is the installer's, unchanged**: the
//!   archive's manifest has to name the id and version the tag named, a
//!   "latest" older than what is installed is not an upgrade, and the
//!   manifest's `minOrivoVersion` still decides whether it may run here.
//! - **No GitHub API.** Unauthenticated REST allows 60 requests an hour per
//!   address, shared with everything else behind it. `/releases/latest` answers
//!   with a redirect to the tag instead, which costs no quota; the redirect is
//!   read and never followed, and the tag is held to a strict grammar before any
//!   URL is built from it.
//! - **Off the display path, like the index.** What a check found is cached;
//!   the Store reads the cache, and a check that fails leaves it as it was.

use crate::plugin_index::{
    IndexEntry, PackageIntegrity, host_allowed, is_upgrade, url_host, valid_plugin_id,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// The only origin a release is looked up on or downloaded from. A constant
/// rather than a parameter so no caller can point the lookup elsewhere; it is
/// also on the index's allowlist, which every URL built here is checked against
/// again.
const GITHUB: &str = "https://github.com/";
/// GitHub's own limits are 39 characters for an owner and 100 for a
/// repository; anything longer is not a repository name.
const MAX_REPOSITORY_SEGMENT: usize = 100;
/// `v` and three numbers of at most nine digits each, with their dots. A tag
/// longer than this is not one Orivo published.
const MAX_TAG_LENGTH: usize = 32;
const MAX_LOCATION_LENGTH: usize = 512;
const MAX_URL_LENGTH: usize = 512;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

const CACHE_DIRECTORY: &str = ".cache";
const CACHE_FILE: &str = "official-releases.json";
const CACHE_FORMAT_VERSION: u32 = 1;
/// A handful of ids and version strings. Anything larger is not this file.
const MAX_CACHE_BYTES: u64 = 64 * 1024;
const MAX_OFFICIAL_PLUGINS: usize = 32;

// ---------------------------------------------------------------------------
// Which plugins are Orivo's
// ---------------------------------------------------------------------------

/// An official plugin and the repository its releases are published from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficialSource {
    pub id: String,
    pub repository: String,
}

#[derive(Debug, Deserialize)]
struct SourceDocument {
    id: String,
    #[serde(default)]
    repository: Option<String>,
}

/// The official plugins, read from the registry compiled into the binary — the
/// only document trusted to say which code is Orivo's. An entry without a
/// `repository` is still listed in the Store, it just is not followed.
pub fn official_sources(compiled_registry: &[u8]) -> Vec<OfficialSource> {
    serde_json::from_slice::<Vec<SourceDocument>>(compiled_registry)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|document| {
            let repository = document.repository?;
            (valid_plugin_id(&document.id) && valid_repository(&repository)).then_some(
                OfficialSource {
                    id: document.id,
                    repository,
                },
            )
        })
        .take(MAX_OFFICIAL_PLUGINS)
        .collect()
}

/// `owner/repo`, in the characters GitHub allows. Checked even though the value
/// is compiled in, because it is pasted into a URL and a slip there should be a
/// plugin that is not followed rather than a request to somewhere unexpected.
fn valid_repository(value: &str) -> bool {
    let Some((owner, repository)) = value.split_once('/') else {
        return false;
    };
    [owner, repository].iter().all(|segment| {
        !segment.is_empty()
            && segment.len() <= MAX_REPOSITORY_SEGMENT
            && !segment.starts_with('.')
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    })
}

// ---------------------------------------------------------------------------
// URLs
// ---------------------------------------------------------------------------

/// Where GitHub says which release is the latest, by redirecting to its tag.
pub fn latest_release_url(repository: &str) -> Option<String> {
    if !valid_repository(repository) {
        return None;
    }
    allowlisted(format!("{GITHUB}{repository}/releases/latest"))
}

/// The package a release of an official plugin carries. The asset name is the
/// one the plugin projects' release workflow produces:
/// `<id>-<version>.orivo-plugin` under tag `v<version>`.
pub fn package_url(repository: &str, plugin_id: &str, version: &str) -> Option<String> {
    if !valid_repository(repository) || !valid_plugin_id(plugin_id) || !release_version(version) {
        return None;
    }
    allowlisted(format!(
        "{GITHUB}{repository}/releases/download/v{version}/{plugin_id}-{version}.orivo-plugin"
    ))
}

/// The same check every index URL passes, applied to a URL this module built:
/// the format strings above are not what keeps a request on the allowlist.
fn allowlisted(url: String) -> Option<String> {
    (url.len() <= MAX_URL_LENGTH && url_host(&url).is_some_and(|host| host_allowed(&host)))
        .then_some(url)
}

/// The version a `/releases/latest` redirect names, or nothing.
///
/// Strict on purpose, because the answer is pasted into a download URL: the
/// same host and the same repository that were asked, `releases/tag/`, and a
/// tag that is `v` and a plain release version. A prerelease is refused —
/// GitHub never calls one "latest", so seeing it means something other than
/// GitHub answered. A repository GitHub redirects to after a rename is refused
/// too: the compiled-in name is the one that was reviewed.
pub fn release_version_from_location(location: &str, repository: &str) -> Option<String> {
    if location.len() > MAX_LOCATION_LENGTH {
        return None;
    }
    let (named, tag) = location
        .strip_prefix(GITHUB)?
        .split_once("/releases/tag/")?;
    if !named.eq_ignore_ascii_case(repository) || tag.len() > MAX_TAG_LENGTH {
        return None;
    }
    let version = tag.strip_prefix('v')?;
    release_version(version).then(|| version.to_string())
}

/// `MAJOR.MINOR.PATCH`, digits only, no leading zero. Narrower than the
/// index's grammar: what is accepted here becomes part of a URL.
fn release_version(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 9
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}

// ---------------------------------------------------------------------------
// Merging with the index
// ---------------------------------------------------------------------------

/// Offer an official plugin's latest release wherever it is newer than what
/// the compiled list and the index offer.
///
/// The higher version wins, so an index that is ahead — a maintainer pinning a
/// release before marking it latest — still works. A tie keeps the pinned
/// entry: the same release with a digest and a size is the stronger promise.
/// Name, summary, category and `minOrivoVersion` are the entry's: a release
/// says none of them, and the minimum is a floor a newer release can only
/// raise, which the archive's manifest then enforces.
///
/// An id with no entry is not added: nothing would say what to call it.
pub fn with_latest_releases(
    entries: &mut [IndexEntry],
    sources: &[OfficialSource],
    latest: &BTreeMap<String, String>,
) {
    for entry in entries {
        let Some(source) = sources.iter().find(|source| source.id == entry.id) else {
            continue;
        };
        let Some(version) = latest.get(&entry.id) else {
            continue;
        };
        if !is_upgrade(version, &entry.version) {
            continue;
        }
        let Some(url) = package_url(&source.repository, &entry.id, version) else {
            continue;
        };
        entry.version = version.clone();
        entry.url = url;
        entry.integrity = PackageIntegrity::ReleaseSigned;
    }
}

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// What the last check found, so the Store never waits on GitHub.
///
/// Ordinary user-writable state, and treated as such: it is read back through
/// the same grammar and only for the ids compiled in. Editing it can at most
/// name another version of an official plugin — whose package still has to
/// exist in the compiled-in repository, carry Orivo's release signature, say
/// that version in its manifest and be newer than what is installed.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedReleases {
    format_version: u32,
    checked_at_epoch_ms: u64,
    #[serde(default)]
    releases: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ReleaseCache {
    path: PathBuf,
}

impl ReleaseCache {
    pub fn new(plugin_root: &Path) -> Self {
        Self {
            path: plugin_root.join(CACHE_DIRECTORY).join(CACHE_FILE),
        }
    }

    fn read(&self) -> Option<CachedReleases> {
        let metadata = fs::symlink_metadata(&self.path).ok()?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_CACHE_BYTES {
            return None;
        }
        let cached = serde_json::from_slice::<CachedReleases>(&fs::read(&self.path).ok()?).ok()?;
        (cached.format_version == CACHE_FORMAT_VERSION).then_some(cached)
    }

    /// The latest version known for each official plugin. No network, ever.
    pub fn load(&self, sources: &[OfficialSource]) -> BTreeMap<String, String> {
        let Some(cached) = self.read() else {
            return BTreeMap::new();
        };
        cached
            .releases
            .into_iter()
            .filter(|(id, version)| {
                sources.iter().any(|source| &source.id == id) && release_version(version)
            })
            .collect()
    }

    fn is_fresh(&self, ttl: Duration) -> bool {
        self.read().is_some_and(|cached| {
            epoch_ms().saturating_sub(cached.checked_at_epoch_ms) < ttl.as_millis() as u64
        })
    }

    pub(crate) fn store(&self, releases: &BTreeMap<String, String>) -> Result<(), String> {
        let failed = || "The plugin release check could not be cached.".to_string();
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| failed())?;
        }
        let encoded = serde_json::to_vec(&CachedReleases {
            format_version: CACHE_FORMAT_VERSION,
            checked_at_epoch_ms: epoch_ms(),
            releases: releases.clone(),
        })
        .map_err(|_| failed())?;
        let temporary = self.path.with_extension("json.tmp");
        let mut file = fs::File::create(&temporary).map_err(|_| failed())?;
        file.write_all(&encoded)
            .and_then(|()| file.sync_all())
            .map_err(|_| failed())?;
        drop(file);
        fs::rename(&temporary, &self.path).map_err(|_| failed())
    }
}

// ---------------------------------------------------------------------------
// Checking
// ---------------------------------------------------------------------------

/// Ask GitHub for the latest release of every official plugin, at most once per
/// `ttl`. Never called from a path that renders.
///
/// A plugin whose check fails keeps the version found last time: a flaky
/// network, or a repository with no release yet, must not make Orivo forget a
/// release it already knew. `Err` only when nothing answered at all, which is
/// also when the cache's clock is left alone so the next check asks again.
pub async fn refresh_official_releases(
    cache: &ReleaseCache,
    sources: &[OfficialSource],
    ttl: Duration,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        // The redirect *is* the answer. Following it would cost a page load and
        // leave the tag to be dug out of HTML.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "The download client could not start.".to_string())?;
    refresh_with(cache, sources, ttl, cancelled, |url| {
        let request = client.get(url).send();
        async move {
            let response = request.await.ok()?;
            if !response.status().is_redirection() {
                return None;
            }
            response
                .headers()
                .get(reqwest::header::LOCATION)?
                .to_str()
                .ok()
                .map(str::to_owned)
        }
    })
    .await
}

/// The check, with the request passed in so the suite drives the real merge,
/// fallback and caching rules without a network.
async fn refresh_with<F, Fut>(
    cache: &ReleaseCache,
    sources: &[OfficialSource],
    ttl: Duration,
    cancelled: &AtomicBool,
    mut redirect_of: F,
) -> Result<(), String>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    if sources.is_empty() || cache.is_fresh(ttl) {
        return Ok(());
    }
    let mut releases = cache.load(sources);
    let mut answered = false;
    for source in sources {
        if cancelled.load(Ordering::Acquire) {
            return Err("Cancelled.".into());
        }
        let Some(url) = latest_release_url(&source.repository) else {
            continue;
        };
        let Some(location) = redirect_of(url).await else {
            continue;
        };
        if let Some(version) = release_version_from_location(&location, &source.repository) {
            releases.insert(source.id.clone(), version);
            answered = true;
        }
    }
    if !answered {
        return Err("Orivo could not find its plugins' releases.".into());
    }
    cache.store(&releases)
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    const MOONLIGHT: &str = "justeozan/orivo-plugin-moonlight";
    const TTL: Duration = Duration::from_secs(60 * 60);

    fn sources() -> Vec<OfficialSource> {
        vec![OfficialSource {
            id: "com.orivo.gamestream".into(),
            repository: MOONLIGHT.into(),
        }]
    }

    fn pinned(id: &str, version: &str) -> IndexEntry {
        IndexEntry {
            id: id.into(),
            name: "Moonlight / Sunshine".into(),
            version: version.into(),
            summary: String::new(),
            url: format!(
                "{GITHUB}{MOONLIGHT}/releases/download/v{version}/{id}-{version}.orivo-plugin"
            ),
            integrity: PackageIntegrity::Pinned {
                sha256: "a".repeat(64),
                size_bytes: 15_057,
            },
            min_orivo_version: Some("0.3.7".into()),
            category: Some("streaming".into()),
        }
    }

    fn latest(version: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("com.orivo.gamestream".to_string(), version.to_string())])
    }

    fn temporary_root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "orivo-official-releases-{}-{}-{}",
            std::process::id(),
            epoch_ms(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_latest_release_redirect_yields_its_version_and_nothing_else_does() {
        let location = |tag: &str| format!("{GITHUB}{MOONLIGHT}/releases/tag/{tag}");
        assert_eq!(
            release_version_from_location(&location("v0.2.1"), MOONLIGHT).as_deref(),
            Some("0.2.1")
        );
        assert_eq!(
            release_version_from_location(&location("v10.20.30"), MOONLIGHT).as_deref(),
            Some("10.20.30")
        );
        // GitHub canonicalises the owner's case; the repository is the same.
        assert_eq!(
            release_version_from_location(
                "https://github.com/JusteOzan/orivo-plugin-moonlight/releases/tag/v0.2.1",
                MOONLIGHT
            )
            .as_deref(),
            Some("0.2.1")
        );

        for refused in [
            // Never "latest" on GitHub, so not something GitHub said.
            location("v0.3.0-rc.1"),
            location("v0.3.0+build.7"),
            // Not a tag Orivo's release workflow writes.
            location("0.2.1"),
            location("latest"),
            location("v0.2"),
            location("v0.2.1.4"),
            location("v01.2.1"),
            location("v0.2.1/../../x"),
            location("v0.2.1?x=1"),
            location("v0.2.1#x"),
            location(&format!("v{}", "9".repeat(40))),
            location(&format!("v1.2.{}", "1".repeat(10))),
            // No release yet: GitHub sends the list instead of a tag.
            format!("{GITHUB}{MOONLIGHT}/releases"),
            // Another repository, a renamed one, another host, plain HTTP.
            format!("{GITHUB}justeozan/orivo-plugin-quiky/releases/tag/v0.2.1"),
            format!("{GITHUB}someone/orivo-plugin-moonlight/releases/tag/v0.2.1"),
            format!("https://github.com.attacker.example/{MOONLIGHT}/releases/tag/v0.2.1"),
            format!("http://github.com/{MOONLIGHT}/releases/tag/v0.2.1"),
            format!("/{MOONLIGHT}/releases/tag/v0.2.1"),
            String::new(),
            "garbage".into(),
            // Overlong, whatever it ends with.
            format!(
                "{GITHUB}{MOONLIGHT}/{}/releases/tag/v0.2.1",
                "a".repeat(600)
            ),
        ] {
            assert_eq!(
                release_version_from_location(&refused, MOONLIGHT),
                None,
                "accepted {refused}"
            );
        }
    }

    #[test]
    fn every_url_built_for_a_release_stays_on_the_allowlist() {
        let url = package_url(MOONLIGHT, "com.orivo.gamestream", "0.2.1").unwrap();
        assert_eq!(
            url,
            "https://github.com/justeozan/orivo-plugin-moonlight/releases/download/v0.2.1/com.orivo.gamestream-0.2.1.orivo-plugin"
        );
        let latest = latest_release_url(MOONLIGHT).unwrap();
        assert_eq!(
            latest,
            "https://github.com/justeozan/orivo-plugin-moonlight/releases/latest"
        );
        for built in [&url, &latest] {
            assert!(url_host(built).is_some_and(|host| host_allowed(&host)));
        }

        // Anything that would bend the URL is refused before it is built.
        for repository in [
            "justeozan",
            "justeozan/",
            "/orivo-plugin-moonlight",
            "../orivo-plugin-moonlight",
            "justeozan/..",
            "justeozan/a/b",
            "user@evil.example/x",
            "evil.example:443/x",
            "justeozan/orivo plugin",
        ] {
            assert_eq!(latest_release_url(repository), None, "{repository}");
            assert_eq!(
                package_url(repository, "com.orivo.gamestream", "0.2.1"),
                None,
                "{repository}"
            );
        }
        assert_eq!(package_url(MOONLIGHT, "../escape", "0.2.1"), None);
        assert_eq!(
            package_url(MOONLIGHT, "com.orivo.gamestream", "0.2.1-rc.1"),
            None
        );
        assert_eq!(
            package_url(MOONLIGHT, "com.orivo.gamestream", "0.2.1/x"),
            None
        );
    }

    #[test]
    fn only_the_compiled_in_registry_says_which_plugins_are_official() {
        let sources = official_sources(include_bytes!("../resources/plugin-registry.json"));
        assert!(sources.contains(&OfficialSource {
            id: "com.orivo.gamestream".into(),
            repository: MOONLIGHT.into(),
        }));

        // An entry without a repository is listed, not followed; one whose
        // repository could bend a URL is neither.
        let sources = official_sources(
            br#"[{"id":"com.orivo.a","repository":"o/a"},
                 {"id":"com.orivo.b"},
                 {"id":"com.orivo.c","repository":"../c"},
                 {"id":"not-an-id","repository":"o/d"}]"#,
        );
        assert_eq!(
            sources,
            vec![OfficialSource {
                id: "com.orivo.a".into(),
                repository: "o/a".into(),
            }]
        );
    }

    #[test]
    fn the_higher_of_the_index_and_the_latest_release_is_offered() {
        // Latest is ahead: its package, its URL, and no digest pretending.
        let mut entries = vec![pinned("com.orivo.gamestream", "0.1.0")];
        with_latest_releases(&mut entries, &sources(), &latest("0.2.1"));
        assert_eq!(entries[0].version, "0.2.1");
        assert_eq!(
            entries[0].url,
            package_url(MOONLIGHT, "com.orivo.gamestream", "0.2.1").unwrap()
        );
        assert_eq!(entries[0].integrity, PackageIntegrity::ReleaseSigned);
        // What a release does not say is kept from the entry.
        assert_eq!(entries[0].min_orivo_version.as_deref(), Some("0.3.7"));
        assert_eq!(entries[0].category.as_deref(), Some("streaming"));

        // The index is ahead: it stays, pinned.
        let mut entries = vec![pinned("com.orivo.gamestream", "0.3.0")];
        with_latest_releases(&mut entries, &sources(), &latest("0.2.1"));
        assert_eq!(entries, vec![pinned("com.orivo.gamestream", "0.3.0")]);

        // A tie keeps the entry that carries a digest.
        let mut entries = vec![pinned("com.orivo.gamestream", "0.2.1")];
        with_latest_releases(&mut entries, &sources(), &latest("0.2.1"));
        assert_eq!(entries, vec![pinned("com.orivo.gamestream", "0.2.1")]);

        // A community plugin is never redirected to a release, whatever the
        // map of versions holds.
        let mut entries = vec![pinned("com.example.community", "0.1.0")];
        let community = BTreeMap::from([("com.example.community".to_string(), "9.0.0".into())]);
        with_latest_releases(&mut entries, &sources(), &community);
        assert_eq!(entries, vec![pinned("com.example.community", "0.1.0")]);
    }

    #[test]
    fn a_check_keeps_what_it_knew_when_github_does_not_answer() {
        let root = temporary_root();
        let cache = ReleaseCache::new(&root);
        let cancelled = AtomicBool::new(false);
        let requests = std::cell::Cell::new(0);
        let check = |answer: Option<String>, ttl: Duration| {
            tauri::async_runtime::block_on(refresh_with(
                &cache,
                &sources(),
                ttl,
                &cancelled,
                |url| {
                    assert_eq!(url, latest_release_url(MOONLIGHT).unwrap());
                    requests.set(requests.get() + 1);
                    let answer = answer.clone();
                    async move { answer }
                },
            ))
        };

        // Nothing answered: an error, and nothing written.
        assert!(check(None, TTL).is_err());
        assert!(!cache.path.exists());

        let tag = |version: &str| Some(format!("{GITHUB}{MOONLIGHT}/releases/tag/v{version}"));
        assert_eq!(check(tag("0.2.1"), TTL), Ok(()));
        assert_eq!(cache.load(&sources()), latest("0.2.1"));

        // Inside the TTL no request is made at all.
        let before = requests.get();
        assert_eq!(check(tag("9.9.9"), TTL), Ok(()));
        assert_eq!(requests.get(), before);
        assert_eq!(cache.load(&sources()), latest("0.2.1"));

        // Past it, a failure or a garbage answer keeps the last good version.
        assert!(check(None, Duration::ZERO).is_err());
        assert!(check(tag("0.3.0-rc.1"), Duration::ZERO).is_err());
        assert_eq!(cache.load(&sources()), latest("0.2.1"));

        assert_eq!(check(tag("0.3.0"), Duration::ZERO), Ok(()));
        assert_eq!(cache.load(&sources()), latest("0.3.0"));
        fs::remove_dir_all(root).ok();
    }

    /// One repository answering is enough to write the cache, and writing it
    /// must not drop what another plugin's last check found — Quiky's repository
    /// failing is no reason to forget Moonlight's release, or the reverse.
    #[test]
    fn one_plugin_failing_does_not_erase_another_plugins_release() {
        let root = temporary_root();
        let cache = ReleaseCache::new(&root);
        let cancelled = AtomicBool::new(false);
        let quiky = "justeozan/orivo-plugin-quiky";
        let both = vec![
            sources().remove(0),
            OfficialSource {
                id: "com.orivo.quiky".into(),
                repository: quiky.into(),
            },
        ];
        let check = |answers: BTreeMap<String, String>| {
            tauri::async_runtime::block_on(refresh_with(
                &cache,
                &both,
                Duration::ZERO,
                &cancelled,
                |url| {
                    let answer = answers
                        .iter()
                        .find(|(repository, _)| url.contains(repository.as_str()))
                        .map(|(repository, version)| {
                            format!("{GITHUB}{repository}/releases/tag/v{version}")
                        });
                    async move { answer }
                },
            ))
        };

        assert_eq!(
            check(BTreeMap::from([
                (MOONLIGHT.to_string(), "0.2.1".to_string()),
                (quiky.to_string(), "0.2.0".to_string()),
            ])),
            Ok(())
        );
        // Only Moonlight answers this time.
        assert_eq!(
            check(BTreeMap::from([(
                MOONLIGHT.to_string(),
                "0.3.0".to_string()
            )])),
            Ok(())
        );
        assert_eq!(
            cache.load(&both),
            BTreeMap::from([
                ("com.orivo.gamestream".to_string(), "0.3.0".to_string()),
                ("com.orivo.quiky".to_string(), "0.2.0".to_string()),
            ])
        );
        fs::remove_dir_all(root).ok();
    }

    /// The cache is user-writable. An edit can make it forget, or name another
    /// well-formed version of an official plugin — never add a plugin, and
    /// never put something that is not a version into a URL.
    #[test]
    fn the_cache_is_read_through_the_same_grammar() {
        let root = temporary_root();
        let cache = ReleaseCache::new(&root);
        fs::create_dir_all(cache.path.parent().unwrap()).unwrap();
        fs::write(
            &cache.path,
            br#"{"formatVersion":1,"checkedAtEpochMs":0,"releases":{
                "com.orivo.gamestream":"0.2.1/../../evil",
                "com.example.community":"1.0.0"}}"#,
        )
        .unwrap();
        assert!(cache.load(&sources()).is_empty());

        fs::write(&cache.path, b"{ not json").unwrap();
        assert!(cache.load(&sources()).is_empty());
        assert!(!cache.is_fresh(TTL));
        fs::remove_dir_all(root).ok();
    }
}
