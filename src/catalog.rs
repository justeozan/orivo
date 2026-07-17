use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Catalog {
    pub schema_version: u32,
    pub games: Vec<Game>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub executable_path: PathBuf,
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub metadata: Option<String>,
    #[serde(default)]
    pub artwork_path: Option<PathBuf>,
    #[serde(default)]
    pub cover_path: Option<PathBuf>,
    #[serde(default)]
    pub logo_path: Option<PathBuf>,
    #[serde(default)]
    pub hero_video_path: Option<PathBuf>,
    #[serde(default)]
    pub last_played_at: Option<String>,
    #[serde(default)]
    pub play_time_seconds: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug)]
pub enum CatalogError {
    Io(io::Error),
    Json(serde_json::Error),
    UnsupportedSchema { found: u32, current: u32 },
    Invalid(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "catalog I/O error: {error}"),
            Self::Json(error) => write!(f, "catalog format error: {error}"),
            Self::UnsupportedSchema { found, current } => {
                write!(
                    f,
                    "catalog schema {found} is newer than supported schema {current}"
                )
            }
            Self::Invalid(message) => write!(f, "invalid catalog: {message}"),
        }
    }
}

impl std::error::Error for CatalogError {}

impl From<io::Error> for CatalogError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for CatalogError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            games: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Self, CatalogError> {
        let contents = fs::read_to_string(path)?;
        let catalog: Self = serde_json::from_str(&contents)?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn save_atomically(&self, path: &Path) -> Result<(), CatalogError> {
        self.validate()?;
        let json = serde_json::to_string_pretty(self)? + "\n";
        let temporary_path = path.with_extension("json.tmp");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&temporary_path, json)?;
        fs::rename(temporary_path, path)?;
        Ok(())
    }

    pub fn add(&mut self, game: Game) -> Result<(), CatalogError> {
        game.validate()?;
        if self.games.iter().any(|existing| existing.id == game.id) {
            return Err(CatalogError::Invalid(format!(
                "duplicate game id: {}",
                game.id
            )));
        }
        self.games.push(game);
        Ok(())
    }

    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(CatalogError::UnsupportedSchema {
                found: self.schema_version,
                current: CURRENT_SCHEMA_VERSION,
            });
        }
        for game in &self.games {
            game.validate()?;
        }
        Ok(())
    }
}

pub fn default_path() -> PathBuf {
    if let Some(path) = std::env::var_os("ORIVO_CATALOG_PATH") {
        return PathBuf::from(path);
    }

    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Library/Application Support/Orivo/catalog.json");
    }

    #[cfg(target_os = "windows")]
    if let Some(app_data) = std::env::var_os("APPDATA") {
        return PathBuf::from(app_data).join("Orivo/catalog.json");
    }

    PathBuf::from("orivo-catalog.json")
}

impl Game {
    pub fn from_executable(path: impl Into<PathBuf>) -> Result<Self, CatalogError> {
        let selected_path = path.into();
        let executable_path = resolve_executable(&selected_path)?;
        let title_path = if selected_path
            .extension()
            .is_some_and(|extension| extension == "app")
        {
            selected_path.clone()
        } else {
            executable_path.clone()
        };
        let title = executable_path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| {
                title_path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .ok_or_else(|| CatalogError::Invalid("executable has no usable filename".into()))?;
        let title = if selected_path
            .extension()
            .is_some_and(|extension| extension == "app")
        {
            bundle_display_name(&selected_path).unwrap_or(title)
        } else {
            title
        };
        let artwork_path = discover_artwork(&selected_path, &executable_path);
        let id = executable_path.to_string_lossy().to_string();

        Ok(Self {
            id,
            title,
            working_directory: executable_path.parent().map(Path::to_path_buf),
            executable_path,
            arguments: Vec::new(),
            description: None,
            metadata: None,
            artwork_path,
            cover_path: None,
            logo_path: None,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        })
    }

    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.id.trim().is_empty() {
            return Err(CatalogError::Invalid("game id cannot be empty".into()));
        }
        if self.title.trim().is_empty() {
            return Err(CatalogError::Invalid(format!(
                "game {} has no title",
                self.id
            )));
        }
        if self.executable_path.as_os_str().is_empty() {
            return Err(CatalogError::Invalid(format!(
                "game {} has no executable",
                self.id
            )));
        }
        Ok(())
    }
}

fn resolve_executable(path: &Path) -> Result<PathBuf, CatalogError> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }

    if !path.exists() && path.extension().is_none_or(|extension| extension != "app") {
        return Ok(path.to_path_buf());
    }

    if path.extension().is_some_and(|extension| extension == "app") && path.is_dir() {
        let info_path = path.join("Contents/Info.plist");
        let executable_name = plist::Value::from_file(&info_path)
            .ok()
            .and_then(|value| value.into_dictionary())
            .and_then(|dictionary| dictionary.get("CFBundleExecutable").cloned())
            .and_then(|value| value.into_string());
        if let Some(executable_name) = executable_name {
            let executable = path.join("Contents/MacOS").join(executable_name);
            if executable.is_file() {
                return Ok(executable);
            }
        }
    }

    Err(CatalogError::Invalid(format!(
        "could not resolve an executable from {}",
        path.display()
    )))
}

fn bundle_display_name(path: &Path) -> Option<String> {
    let info_path = path.join("Contents/Info.plist");
    plist::Value::from_file(info_path)
        .ok()
        .and_then(|value| value.into_dictionary())
        .and_then(|dictionary| {
            dictionary
                .get("CFBundleDisplayName")
                .or_else(|| dictionary.get("CFBundleName"))
                .cloned()
        })
        .and_then(|value| value.into_string())
}

fn discover_artwork(selected_path: &Path, executable_path: &Path) -> Option<PathBuf> {
    let mut directories = Vec::new();
    if selected_path
        .extension()
        .is_some_and(|extension| extension == "app")
    {
        directories.push(selected_path.join("Contents/Resources"));
    }
    if let Some(parent) = executable_path.parent() {
        directories.push(parent.to_path_buf());
    }

    directories.into_iter().find_map(|directory| {
        let mut candidates = std::fs::read_dir(directory)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path.extension().is_some_and(|extension| {
                        matches!(
                            extension.to_str(),
                            Some("png" | "jpg" | "jpeg" | "bmp" | "webp")
                        )
                    })
            })
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.into_iter().next()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_manual_import_from_an_executable() {
        let game = Game::from_executable("/Games/Nightfall/Nightfall.app/Contents/MacOS/Nightfall")
            .unwrap();

        assert_eq!(game.title, "Nightfall");
        assert_eq!(
            game.working_directory,
            Some(PathBuf::from(
                "/Games/Nightfall/Nightfall.app/Contents/MacOS"
            ))
        );
        assert!(game.arguments.is_empty());
    }

    #[test]
    fn rejects_a_future_schema_without_mutating_data() {
        let catalog = Catalog {
            schema_version: CURRENT_SCHEMA_VERSION + 1,
            games: Vec::new(),
            extra: BTreeMap::new(),
        };

        assert!(matches!(
            catalog.validate(),
            Err(CatalogError::UnsupportedSchema { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_game_ids() {
        let game = Game::from_executable("/Games/Nightfall").unwrap();
        let mut catalog = Catalog::default();
        catalog.add(game.clone()).unwrap();

        assert!(
            matches!(catalog.add(game), Err(CatalogError::Invalid(message)) if message.contains("duplicate game id"))
        );
    }

    #[test]
    fn resolves_a_macos_app_bundle_to_its_declared_executable() {
        let root = std::env::temp_dir().join(format!("orivo-app-test-{}", std::process::id()));
        let bundle = root.join("Unrailed!.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleDisplayName</key><string>Unrailed!</string>
<key>CFBundleExecutable</key><string>UnrailedGame</string>
</dict></plist>"#,
        )
        .unwrap();
        std::fs::write(macos.join("UnrailedGame"), "#!/bin/sh\n").unwrap();

        let game = Game::from_executable(&bundle).unwrap();

        assert_eq!(game.title, "Unrailed!");
        assert_eq!(game.executable_path, macos.join("UnrailedGame"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
