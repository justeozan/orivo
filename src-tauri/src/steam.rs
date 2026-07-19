//! Local, read-only Steam discovery.
//!
//! Steam's files are a loosely documented KeyValue (VDF) format. This module
//! intentionally owns that parsing boundary and returns ordinary Rust values;
//! it has no dependency on Tauri, the catalog, or the WebView. Callers can run
//! it in a worker and decide how to surface partial filesystem failures.

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

const STEAMAPPS: &str = "steamapps";
const FULLY_INSTALLED: u32 = 4;
const STEAMWORKS_REDISTRIBUTABLES_APP_ID: u32 = 228_980;
// These files are normally only a few KiB. Bounds keep a corrupt local VDF
// from consuming unbounded memory or recursion in the background scanner.
const MAX_VDF_INPUT_BYTES: usize = 1_048_576;
const MAX_VDF_TOKENS: usize = 50_000;
const MAX_VDF_STRING_BYTES: usize = 16_384;
const MAX_VDF_DEPTH: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SteamDiscovery {
    /// Kept in the Rust boundary only. It must never be serialized to the
    /// WebView because it reveals a user filesystem location.
    pub steam_root: Option<PathBuf>,
    pub libraries: Vec<SteamLibrary>,
    pub games: Vec<SteamGame>,
    pub issues: Vec<SteamIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamLibrary {
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamGame {
    pub app_id: u32,
    pub title: String,
    /// The physical install directory is retained for backend validation and
    /// source refreshes, never sent to the WebView.
    pub installation_path: PathBuf,
    pub manifest_path: PathBuf,
    pub last_updated: Option<u64>,
    pub cover_path: Option<PathBuf>,
    pub hero_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamIssue {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamAppManifest {
    pub app_id: u32,
    pub title: String,
    pub install_dir: String,
    pub state_flags: u32,
    pub last_updated: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamParseError {
    message: String,
}

impl SteamParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SteamParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SteamParseError {}

/// Discover the current machine's Steam data. `ORIVO_STEAM_ROOT` is an
/// intentional test/development override; production discovery then follows
/// the native platform conventions without relying on a shell command.
pub fn discover_default() -> SteamDiscovery {
    if let Some(root) = env::var_os("ORIVO_STEAM_ROOT") {
        return discover_at(Path::new(&root));
    }

    default_roots()
        .into_iter()
        .find(|root| steamapps_path(root).is_dir())
        .map_or_else(SteamDiscovery::default, |root| discover_at(&root))
}

/// Scan one Steam root. A root may be the Steam data directory or its
/// `steamapps` child, which makes fixtures and migrated installations simple.
pub fn discover_at(root: &Path) -> SteamDiscovery {
    let root = normalize_root(root);
    let root_steamapps = steamapps_path(&root);
    if !root_steamapps.is_dir() {
        return SteamDiscovery {
            steam_root: None,
            ..SteamDiscovery::default()
        };
    }

    let mut discovery = SteamDiscovery {
        steam_root: Some(root.clone()),
        ..SteamDiscovery::default()
    };
    let mut libraries = vec![root.clone()];
    let library_folders = root_steamapps.join("libraryfolders.vdf");

    if library_folders.is_file() {
        match read_vdf_file(&library_folders)
            .map_err(|error| error.to_string())
            .and_then(|contents| {
                parse_libraryfolders_vdf(&contents).map_err(|error| error.to_string())
            }) {
            Ok(paths) => libraries.extend(paths),
            Err(message) => discovery.issues.push(SteamIssue {
                path: library_folders,
                message: format!("could not read Steam library folders: {message}"),
            }),
        }
    }

    let mut seen_libraries = BTreeSet::new();
    let mut seen_games = BTreeSet::new();
    for library in libraries {
        let library = normalize_root(&library);
        let key = library.to_string_lossy().to_string();
        if !seen_libraries.insert(key) || !steamapps_path(&library).is_dir() {
            continue;
        }
        discovery.libraries.push(SteamLibrary {
            path: library.clone(),
        });
        scan_library(
            &root,
            &library,
            &mut seen_games,
            &mut discovery.games,
            &mut discovery.issues,
        );
    }

    discovery.games.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| left.app_id.cmp(&right.app_id))
    });
    discovery
}

/// Parse both modern and legacy `libraryfolders.vdf` forms. The caller owns
/// the returned paths and verifies their `steamapps` contents before scanning.
pub fn parse_libraryfolders_vdf(input: &str) -> Result<Vec<PathBuf>, SteamParseError> {
    let root = parse_vdf(input)?;
    let folders = root
        .get("libraryfolders")
        .and_then(VdfValue::as_object)
        .ok_or_else(|| SteamParseError::new("libraryfolders object is missing"))?;

    let mut result = Vec::new();
    for (key, value) in folders {
        if key.parse::<u32>().is_err() {
            continue;
        }
        match value {
            VdfValue::String(path) if !path.trim().is_empty() => result.push(PathBuf::from(path)),
            VdfValue::Object(properties) => {
                if let Some(path) = properties.get("path").and_then(VdfValue::as_string)
                    && !path.trim().is_empty()
                {
                    result.push(PathBuf::from(path));
                }
            }
            _ => {}
        }
    }
    Ok(result)
}

pub fn parse_appmanifest_acf(input: &str) -> Result<SteamAppManifest, SteamParseError> {
    let root = parse_vdf(input)?;
    let app_state = root
        .get("AppState")
        .and_then(VdfValue::as_object)
        .ok_or_else(|| SteamParseError::new("AppState object is missing"))?;

    let string = |key: &str| {
        app_state
            .get(key)
            .and_then(VdfValue::as_string)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
    };
    let app_id = string("appid")
        .ok_or_else(|| SteamParseError::new("appid is missing"))?
        .parse::<u32>()
        .map_err(|_| SteamParseError::new("appid is not a positive integer"))?;
    if app_id == 0 {
        return Err(SteamParseError::new("appid cannot be zero"));
    }
    let title = string("name").ok_or_else(|| SteamParseError::new("name is missing"))?;
    let install_dir =
        string("installdir").ok_or_else(|| SteamParseError::new("installdir is missing"))?;
    if !is_safe_install_dir(&install_dir) {
        return Err(SteamParseError::new(
            "installdir must be a normal relative path",
        ));
    }
    let state_flags = string("StateFlags")
        .ok_or_else(|| SteamParseError::new("StateFlags is missing"))?
        .parse::<u32>()
        .map_err(|_| SteamParseError::new("StateFlags is not an integer"))?;
    let last_updated = string("LastUpdated").and_then(|value| value.parse::<u64>().ok());

    Ok(SteamAppManifest {
        app_id,
        title,
        install_dir,
        state_flags,
        last_updated,
    })
}

fn scan_library(
    steam_root: &Path,
    library: &Path,
    seen_games: &mut BTreeSet<u32>,
    games: &mut Vec<SteamGame>,
    issues: &mut Vec<SteamIssue>,
) {
    let steamapps = steamapps_path(library);
    let entries = match fs::read_dir(&steamapps) {
        Ok(entries) => entries,
        Err(error) => {
            issues.push(SteamIssue {
                path: steamapps,
                message: format!("could not enumerate Steam manifests: {error}"),
            });
            return;
        }
    };

    for entry in entries.filter_map(Result::ok) {
        let manifest_path = entry.path();
        let Some(app_id) = app_id_from_manifest_path(&manifest_path) else {
            continue;
        };
        let contents = match read_vdf_file(&manifest_path) {
            Ok(contents) => contents,
            Err(error) => {
                issues.push(SteamIssue {
                    path: manifest_path,
                    message: format!("could not read Steam manifest: {error}"),
                });
                continue;
            }
        };
        let manifest = match parse_appmanifest_acf(&contents) {
            Ok(manifest) => manifest,
            Err(error) => {
                issues.push(SteamIssue {
                    path: manifest_path,
                    message: format!("invalid Steam manifest: {error}"),
                });
                continue;
            }
        };
        if manifest.app_id != app_id {
            issues.push(SteamIssue {
                path: manifest_path,
                message: "manifest appid does not match its filename".into(),
            });
            continue;
        }
        if !is_importable(&manifest) {
            continue;
        }
        // `parse_appmanifest_acf` has already rejected rooted and traversing
        // paths. Keep the join here deliberately narrow: it must remain under
        // Steam's `common` directory even for a malformed local manifest.
        let installation_path = steamapps.join("common").join(&manifest.install_dir);
        if !installation_path.is_dir() {
            issues.push(SteamIssue {
                path: manifest_path,
                message: "Steam manifest is not backed by an installed game directory".into(),
            });
            continue;
        }
        if !seen_games.insert(manifest.app_id) {
            continue;
        }
        let (cover_path, hero_path) = discover_artwork(steam_root, manifest.app_id);
        games.push(SteamGame {
            app_id: manifest.app_id,
            title: manifest.title,
            installation_path,
            manifest_path,
            last_updated: manifest.last_updated,
            cover_path,
            hero_path,
        });
    }
}

fn is_importable(manifest: &SteamAppManifest) -> bool {
    if manifest.app_id == STEAMWORKS_REDISTRIBUTABLES_APP_ID
        || manifest.state_flags & FULLY_INSTALLED == 0
    {
        return false;
    }
    let title = manifest.title.to_lowercase();
    !title.contains("soundtrack")
        && !title.contains("steamworks common redistributables")
        && !title.starts_with("proton ")
        && !title.starts_with("steam linux runtime")
}

fn discover_artwork(steam_root: &Path, app_id: u32) -> (Option<PathBuf>, Option<PathBuf>) {
    let directory = steam_root.join("appcache").join("librarycache");
    let cover = find_first_asset(
        &directory,
        app_id,
        &["library_600x900_2x", "library_600x900", "header"],
    );
    let hero = find_first_asset(
        &directory,
        app_id,
        &["library_hero_2x", "library_hero", "header"],
    );
    (cover, hero)
}

fn find_first_asset(directory: &Path, app_id: u32, suffixes: &[&str]) -> Option<PathBuf> {
    const EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];
    suffixes.iter().find_map(|suffix| {
        EXTENSIONS.iter().find_map(|extension| {
            let candidate = directory.join(format!("{app_id}_{suffix}.{extension}"));
            candidate.is_file().then_some(candidate)
        })
    })
}

fn app_id_from_manifest_path(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    let app_id = name
        .strip_prefix("appmanifest_")?
        .strip_suffix(".acf")?
        .parse::<u32>()
        .ok()?;
    (app_id > 0).then_some(app_id)
}

fn is_safe_install_dir(value: &str) -> bool {
    let path = Path::new(value);
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn normalize_root(path: &Path) -> PathBuf {
    if path.file_name().is_some_and(|name| name == STEAMAPPS) {
        path.parent().unwrap_or(path).to_path_buf()
    } else {
        path.to_path_buf()
    }
}

fn steamapps_path(root: &Path) -> PathBuf {
    root.join(STEAMAPPS)
}

fn default_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        #[cfg(target_os = "macos")]
        roots.push(home.join("Library/Application Support/Steam"));
        #[cfg(target_os = "linux")]
        {
            roots.push(home.join(".steam/steam"));
            roots.push(home.join(".local/share/Steam"));
        }
    }
    #[cfg(target_os = "windows")]
    if let Some(program_files) = env::var_os("PROGRAMFILES(X86)") {
        roots.push(PathBuf::from(program_files).join("Steam"));
    }
    roots
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VdfValue {
    String(String),
    Object(BTreeMap<String, VdfValue>),
}

impl VdfValue {
    fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            Self::Object(_) => None,
        }
    }

    fn as_object(&self) -> Option<&BTreeMap<String, VdfValue>> {
        match self {
            Self::Object(value) => Some(value),
            Self::String(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VdfToken {
    String(String),
    OpenBrace,
    CloseBrace,
}

fn parse_vdf(input: &str) -> Result<BTreeMap<String, VdfValue>, SteamParseError> {
    if input.len() > MAX_VDF_INPUT_BYTES {
        return Err(SteamParseError::new("VDF input exceeds the supported size"));
    }
    let tokens = tokenize_vdf(input)?;
    let mut index = 0;
    let object = parse_object(&tokens, &mut index, false, 0)?;
    if index != tokens.len() {
        return Err(SteamParseError::new("unexpected tokens after VDF object"));
    }
    Ok(object)
}

/// Read only a bounded amount before parsing. Checking metadata alone is not
/// sufficient because a file may grow between the check and the read.
fn read_vdf_file(path: &Path) -> Result<String, io::Error> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > MAX_VDF_INPUT_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "VDF input exceeds the supported size",
        ));
    }

    let mut reader = file.take((MAX_VDF_INPUT_BYTES + 1) as u64);
    let mut contents = String::new();
    reader.read_to_string(&mut contents)?;
    if contents.len() > MAX_VDF_INPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "VDF input exceeds the supported size",
        ));
    }
    Ok(contents)
}

fn parse_object(
    tokens: &[VdfToken],
    index: &mut usize,
    closes_on_brace: bool,
    depth: usize,
) -> Result<BTreeMap<String, VdfValue>, SteamParseError> {
    let mut object = BTreeMap::new();
    while let Some(token) = tokens.get(*index) {
        if matches!(token, VdfToken::CloseBrace) {
            if !closes_on_brace {
                return Err(SteamParseError::new("unexpected closing brace"));
            }
            *index += 1;
            return Ok(object);
        }
        let VdfToken::String(key) = token else {
            return Err(SteamParseError::new("expected a VDF key"));
        };
        let key = key.clone();
        *index += 1;
        let value = match tokens.get(*index) {
            Some(VdfToken::String(value)) => {
                *index += 1;
                VdfValue::String(value.clone())
            }
            Some(VdfToken::OpenBrace) => {
                if depth >= MAX_VDF_DEPTH {
                    return Err(SteamParseError::new(
                        "VDF nesting exceeds the supported depth",
                    ));
                }
                *index += 1;
                VdfValue::Object(parse_object(tokens, index, true, depth + 1)?)
            }
            Some(VdfToken::CloseBrace) | None => {
                return Err(SteamParseError::new("VDF key has no value"));
            }
        };
        object.insert(key, value);
    }
    if closes_on_brace {
        Err(SteamParseError::new("unclosed VDF object"))
    } else {
        Ok(object)
    }
}

fn tokenize_vdf(input: &str) -> Result<Vec<VdfToken>, SteamParseError> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            byte if byte.is_ascii_whitespace() => index += 1,
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'{' => {
                push_vdf_token(&mut tokens, VdfToken::OpenBrace)?;
                index += 1;
            }
            b'}' => {
                push_vdf_token(&mut tokens, VdfToken::CloseBrace)?;
                index += 1;
            }
            b'"' => {
                let (value, next) = read_quoted_vdf_string(input, index)?;
                push_vdf_token(&mut tokens, VdfToken::String(value))?;
                index = next;
            }
            _ => return Err(SteamParseError::new("VDF values must be quoted")),
        }
    }
    Ok(tokens)
}

fn push_vdf_token(tokens: &mut Vec<VdfToken>, token: VdfToken) -> Result<(), SteamParseError> {
    if tokens.len() >= MAX_VDF_TOKENS {
        return Err(SteamParseError::new("VDF contains too many tokens"));
    }
    tokens.push(token);
    Ok(())
}

fn read_quoted_vdf_string(
    input: &str,
    opening_quote: usize,
) -> Result<(String, usize), SteamParseError> {
    let bytes = input.as_bytes();
    let mut index = opening_quote + 1;
    let mut value = String::new();
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Ok((value, index + 1)),
            b'\\' => {
                let escaped = *bytes
                    .get(index + 1)
                    .ok_or_else(|| SteamParseError::new("unfinished VDF escape"))?;
                match escaped {
                    b'"' => value.push('"'),
                    b'\\' => value.push('\\'),
                    b'n' => value.push('\n'),
                    b't' => value.push('\t'),
                    other => {
                        value.push('\\');
                        value.push(other as char);
                    }
                }
                index += 2;
            }
            _ => {
                let rest = &input[index..];
                let character = rest
                    .chars()
                    .next()
                    .ok_or_else(|| SteamParseError::new("unterminated VDF string"))?;
                value.push(character);
                index += character.len_utf8();
            }
        }
        if value.len() > MAX_VDF_STRING_BYTES {
            return Err(SteamParseError::new(
                "VDF string exceeds the supported size",
            ));
        }
    }
    Err(SteamParseError::new("unterminated VDF string"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_and_legacy_library_folder_entries() {
        let paths = parse_libraryfolders_vdf(
            r#"
                "libraryfolders"
                {
                  "0" { "path" "/Steam" }
                  "1" "/Volumes/Games/SteamLibrary"
                  "contentstatsid" "123"
                }
            "#,
        )
        .unwrap();

        assert_eq!(
            paths,
            vec![
                PathBuf::from("/Steam"),
                PathBuf::from("/Volumes/Games/SteamLibrary")
            ]
        );
    }

    #[test]
    fn parses_an_installed_manifest_with_escaped_unicode_text() {
        let manifest = parse_appmanifest_acf(
            r#"
              "AppState"
              {
                "appid" "480"
                "name" "L\"été des jeux"
                "StateFlags" "260"
                "installdir" "Spacewar"
                "LastUpdated" "1710000000"
              }
            "#,
        )
        .unwrap();

        assert_eq!(manifest.app_id, 480);
        assert_eq!(manifest.title, "L\"été des jeux");
        assert_eq!(manifest.state_flags, 260);
        assert_eq!(manifest.last_updated, Some(1_710_000_000));
    }

    #[test]
    fn rejects_zero_or_incomplete_manifests() {
        assert!(parse_appmanifest_acf(r#""AppState" { "appid" "0" }"#).is_err());
        assert!(parse_appmanifest_acf(r#""AppState" { "appid" "480" }"#).is_err());
    }

    #[test]
    fn rejects_rooted_or_traversing_install_directories() {
        for install_dir in [
            "../Outside",
            "/tmp/Outside",
            "./Spacewar",
            "Games/../Outside",
        ] {
            assert!(
                parse_appmanifest_acf(&manifest_fixture(480, "Spacewar", install_dir, 4)).is_err()
            );
        }
    }

    #[test]
    fn bounds_vdf_depth_and_file_reads() {
        let mut deeply_nested = String::new();
        for _ in 0..=MAX_VDF_DEPTH {
            deeply_nested.push_str(r#""key" {"#);
        }
        deeply_nested.push_str(r#""value" "value""#);
        for _ in 0..=MAX_VDF_DEPTH {
            deeply_nested.push('}');
        }
        assert!(parse_vdf(&deeply_nested).is_err());

        let root = temp_path("oversized-vdf");
        let vdf_path = root.join("libraryfolders.vdf");
        fs::write(&vdf_path, vec![b' '; MAX_VDF_INPUT_BYTES + 1]).unwrap();
        assert!(read_vdf_file(&vdf_path).is_err());
        remove_temp(&root);
    }

    #[test]
    fn discovers_only_installed_game_manifests_and_keeps_partial_errors() {
        let root = temp_path("discovery");
        let steamapps = root.join("steamapps");
        let common = steamapps.join("common");
        fs::create_dir_all(common.join("Spacewar")).unwrap();
        fs::write(
            steamapps.join("appmanifest_480.acf"),
            manifest_fixture(480, "Spacewar", "Spacewar", 4),
        )
        .unwrap();
        fs::write(
            steamapps.join("appmanifest_481.acf"),
            manifest_fixture(481, "Not complete", "Missing", 2),
        )
        .unwrap();
        fs::write(
            steamapps.join("appmanifest_228980.acf"),
            manifest_fixture(
                228_980,
                "Steamworks Common Redistributables",
                "Redistributable",
                4,
            ),
        )
        .unwrap();
        fs::write(steamapps.join("appmanifest_999.acf"), "broken").unwrap();

        let discovery = discover_at(&root);

        assert_eq!(discovery.games.len(), 1);
        assert_eq!(discovery.games[0].app_id, 480);
        assert_eq!(discovery.libraries.len(), 1);
        assert_eq!(discovery.issues.len(), 1);
        remove_temp(&root);
    }

    #[test]
    fn discovers_secondary_libraries_and_deduplicates_app_ids() {
        let root = temp_path("secondary");
        let secondary = root.join("Secondary");
        let root_steamapps = root.join("steamapps");
        let secondary_steamapps = secondary.join("steamapps");
        fs::create_dir_all(root_steamapps.join("common/Primary")).unwrap();
        fs::create_dir_all(secondary_steamapps.join("common/Secondary")).unwrap();
        fs::write(
            root_steamapps.join("libraryfolders.vdf"),
            format!(
                r#""libraryfolders" {{ "1" {{ "path" "{}" }} }}"#,
                secondary.display()
            ),
        )
        .unwrap();
        fs::write(
            root_steamapps.join("appmanifest_480.acf"),
            manifest_fixture(480, "Primary", "Primary", 4),
        )
        .unwrap();
        fs::write(
            secondary_steamapps.join("appmanifest_480.acf"),
            manifest_fixture(480, "Secondary", "Secondary", 4),
        )
        .unwrap();

        let discovery = discover_at(&root);

        assert_eq!(discovery.libraries.len(), 2);
        assert_eq!(discovery.games.len(), 1);
        assert_eq!(discovery.games[0].title, "Primary");
        remove_temp(&root);
    }

    #[test]
    fn finds_only_known_local_artwork_names() {
        let root = temp_path("artwork");
        let cache = root.join("appcache/librarycache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("480_library_600x900_2x.jpg"), "cover").unwrap();
        fs::write(cache.join("480_library_hero.jpg"), "hero").unwrap();

        let (cover, hero) = discover_artwork(&root, 480);
        assert_eq!(cover, Some(cache.join("480_library_600x900_2x.jpg")));
        assert_eq!(hero, Some(cache.join("480_library_hero.jpg")));
        remove_temp(&root);
    }

    fn manifest_fixture(app_id: u32, title: &str, install_dir: &str, flags: u32) -> String {
        format!(
            r#""AppState" {{ "appid" "{app_id}" "name" "{title}" "StateFlags" "{flags}" "installdir" "{install_dir}" }}"#
        )
    }

    fn temp_path(label: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "orivo-steam-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn remove_temp(path: &Path) {
        fs::remove_dir_all(path).unwrap();
    }
}
