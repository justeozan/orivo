//! Validated application preferences and explicitly derived cache management.
//!
//! This module deliberately owns neither the game catalog nor credentials. Its
//! commands accept no filesystem paths from the WebView and its cache cleanup
//! is an allowlist of host-owned, reconstructible targets.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

const PREFERENCES_FILE: &str = "preferences.json";
const STORE_CACHE_FILE: &str = "store-cache.json";
/// Pre-release builds kept the derived Store catalog in SQLite. The cache is
/// rebuildable, so clearing simply removes whichever form is on disk.
const LEGACY_STORE_CACHE_FILES: &[&str] = &[
    "store-cache.sqlite",
    "store-cache.sqlite-wal",
    "store-cache.sqlite-shm",
];
const DERIVED_CACHE_DIRECTORIES: &[&str] = &["store", "store-derived", "derived"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartPage {
    #[default]
    Library,
    Store,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreRegion {
    #[default]
    Automatic,
    Us,
    Ca,
    Gb,
    Fr,
    De,
    Jp,
    Au,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MotionPreference {
    #[default]
    System,
    Reduced,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreferencesDto {
    pub start_page: StartPage,
    pub store_region: StoreRegion,
    pub motion: MotionPreference,
    /// Debug-only: seed the library with the bundled demo (showcase) games.
    /// Off by default so a real library shows only the user's own games.
    #[serde(default)]
    pub show_showcase_games: bool,
    /// Debug-only: fill a game's detail page with sample achievements, friends
    /// and activity so the social sections can be exercised without a backend
    /// feed. Off by default.
    #[serde(default)]
    pub debug_sample_social: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreferencesUpdate {
    pub start_page: Option<StartPage>,
    pub store_region: Option<StoreRegion>,
    pub motion: Option<MotionPreference>,
    pub show_showcase_games: Option<bool>,
    pub debug_sample_social: Option<bool>,
    #[serde(default)]
    pub reset: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataUsageDto {
    pub derived_cache_bytes: u64,
    pub derived_cache_entries: u64,
    pub refreshed_at_epoch_ms: Option<u64>,
}

/// Filesystem service kept independent from the private application state.
/// `lib.rs` only needs to declare the module and register the four commands.
#[derive(Debug, Clone)]
pub struct PreferencesService {
    preferences_path: PathBuf,
    app_data_dir: PathBuf,
    app_cache_dir: PathBuf,
}

impl PreferencesService {
    pub fn from_app(app: &AppHandle) -> Result<Self, String> {
        let app_data_dir = app.path().app_data_dir().map_err(preference_error)?;
        let app_cache_dir = app.path().app_cache_dir().map_err(preference_error)?;
        Ok(Self::new(app_data_dir, app_cache_dir))
    }

    pub fn new(app_data_dir: PathBuf, app_cache_dir: PathBuf) -> Self {
        Self {
            preferences_path: app_data_dir.join(PREFERENCES_FILE),
            app_data_dir,
            app_cache_dir,
        }
    }

    pub fn load(&self) -> Result<PreferencesDto, String> {
        if !self.preferences_path.is_file() {
            return Ok(PreferencesDto::default());
        }
        let encoded = fs::read_to_string(&self.preferences_path).map_err(preference_error)?;
        serde_json::from_str(&encoded)
            .map_err(|_| "Orivo preferences are unreadable. Reset them to restore defaults.".into())
    }

    pub fn update(&self, update: PreferencesUpdate) -> Result<PreferencesDto, String> {
        let mut preferences = if update.reset {
            PreferencesDto::default()
        } else {
            self.load()?
        };
        if !update.reset {
            if let Some(start_page) = update.start_page {
                preferences.start_page = start_page;
            }
            if let Some(store_region) = update.store_region {
                preferences.store_region = store_region;
            }
            if let Some(motion) = update.motion {
                preferences.motion = motion;
            }
            if let Some(show_showcase_games) = update.show_showcase_games {
                preferences.show_showcase_games = show_showcase_games;
            }
            if let Some(debug_sample_social) = update.debug_sample_social {
                preferences.debug_sample_social = debug_sample_social;
            }
        }
        self.save_atomically(&preferences)?;
        Ok(preferences)
    }

    pub fn data_usage(&self) -> Result<DataUsageDto, String> {
        let mut usage = DataUsageDto::default();
        for target in self.derived_cache_targets() {
            accumulate_usage(&target, &mut usage).map_err(cache_error)?;
        }
        Ok(usage)
    }

    pub fn clear_derived_cache(&self) -> Result<DataUsageDto, String> {
        for target in self.derived_cache_targets() {
            let metadata = match fs::symlink_metadata(&target) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(cache_error(error)),
            };
            if metadata.file_type().is_symlink() || metadata.is_file() {
                fs::remove_file(&target).map_err(cache_error)?;
            } else if metadata.is_dir() {
                fs::remove_dir_all(&target).map_err(cache_error)?;
            }
        }
        self.data_usage()
    }

    fn derived_cache_targets(&self) -> Vec<PathBuf> {
        let mut targets = vec![self.app_data_dir.join(STORE_CACHE_FILE)];
        targets.extend(
            LEGACY_STORE_CACHE_FILES
                .iter()
                .map(|file| self.app_data_dir.join(file)),
        );
        targets.extend(
            DERIVED_CACHE_DIRECTORIES
                .iter()
                .map(|directory| self.app_cache_dir.join(directory)),
        );
        targets
    }

    fn save_atomically(&self, preferences: &PreferencesDto) -> Result<(), String> {
        let parent = self
            .preferences_path
            .parent()
            .ok_or_else(|| "Orivo could not resolve its preferences directory.".to_string())?;
        fs::create_dir_all(parent).map_err(preference_error)?;
        let sequence = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = parent.join(format!(
            ".{PREFERENCES_FILE}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        let result = (|| -> Result<(), io::Error> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            serde_json::to_writer_pretty(&mut file, preferences).map_err(io::Error::other)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.preferences_path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(preference_error)
    }
}

#[tauri::command]
pub fn get_preferences(app: AppHandle) -> Result<PreferencesDto, String> {
    PreferencesService::from_app(&app)?.load()
}

#[tauri::command]
pub fn update_preferences(
    app: AppHandle,
    update: PreferencesUpdate,
) -> Result<PreferencesDto, String> {
    PreferencesService::from_app(&app)?.update(update)
}

#[tauri::command]
pub fn get_data_usage(app: AppHandle) -> Result<DataUsageDto, String> {
    PreferencesService::from_app(&app)?.data_usage()
}

#[tauri::command]
pub fn clear_derived_cache(app: AppHandle) -> Result<DataUsageDto, String> {
    PreferencesService::from_app(&app)?.clear_derived_cache()
}

fn accumulate_usage(path: &Path, usage: &mut DataUsageDto) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_file() {
        usage.derived_cache_bytes = usage.derived_cache_bytes.saturating_add(metadata.len());
        usage.derived_cache_entries = usage.derived_cache_entries.saturating_add(1);
        if let Ok(modified) = metadata.modified() {
            let epoch_ms = modified
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u64::MAX as u128) as u64;
            usage.refreshed_at_epoch_ms = Some(
                usage
                    .refreshed_at_epoch_ms
                    .map_or(epoch_ms, |current| current.max(epoch_ms)),
            );
        }
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            accumulate_usage(&entry?.path(), usage)?;
        }
    }
    Ok(())
}

fn preference_error(error: impl std::fmt::Display) -> String {
    format!("Orivo could not save its preferences: {error}")
}

fn cache_error(error: impl std::fmt::Display) -> String {
    format!("Orivo could not update its derived cache: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectories {
        root: PathBuf,
        data: PathBuf,
        cache: PathBuf,
    }

    impl TestDirectories {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "orivo-preferences-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            let data = root.join("data");
            let cache = root.join("cache");
            fs::create_dir_all(&data).unwrap();
            fs::create_dir_all(&cache).unwrap();
            Self { root, data, cache }
        }

        fn service(&self) -> PreferencesService {
            PreferencesService::new(self.data.clone(), self.cache.clone())
        }
    }

    impl Drop for TestDirectories {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn preferences_persist_across_service_instances() {
        let dirs = TestDirectories::new("persist");
        let saved = dirs
            .service()
            .update(PreferencesUpdate {
                start_page: Some(StartPage::Store),
                store_region: Some(StoreRegion::Fr),
                motion: Some(MotionPreference::Reduced),
                show_showcase_games: Some(true),
                debug_sample_social: Some(true),
                reset: false,
            })
            .unwrap();
        assert_eq!(dirs.service().load().unwrap(), saved);
        assert_eq!(saved.start_page, StartPage::Store);
    }

    #[test]
    fn reset_changes_only_preferences() {
        let dirs = TestDirectories::new("reset");
        fs::write(dirs.data.join("catalog.json"), b"catalog").unwrap();
        fs::write(dirs.data.join("game-state.json"), b"state").unwrap();
        fs::create_dir_all(dirs.data.join("game-media")).unwrap();
        fs::write(dirs.data.join("game-media/selected.jpg"), b"media").unwrap();
        dirs.service()
            .update(PreferencesUpdate {
                start_page: Some(StartPage::Store),
                ..PreferencesUpdate::default()
            })
            .unwrap();
        let reset = dirs
            .service()
            .update(PreferencesUpdate {
                reset: true,
                ..PreferencesUpdate::default()
            })
            .unwrap();
        assert_eq!(reset, PreferencesDto::default());
        assert_eq!(
            fs::read(dirs.data.join("catalog.json")).unwrap(),
            b"catalog"
        );
        assert_eq!(
            fs::read(dirs.data.join("game-state.json")).unwrap(),
            b"state"
        );
        assert_eq!(
            fs::read(dirs.data.join("game-media/selected.jpg")).unwrap(),
            b"media"
        );
    }

    #[test]
    fn clearing_derived_cache_uses_a_strict_allowlist() {
        let dirs = TestDirectories::new("cache-safety");
        fs::write(dirs.data.join(STORE_CACHE_FILE), b"derived").unwrap();
        fs::create_dir_all(dirs.cache.join("store")).unwrap();
        fs::write(dirs.cache.join("store/page.json"), b"derived").unwrap();
        fs::write(dirs.data.join("catalog.json"), b"catalog").unwrap();
        fs::write(dirs.data.join(PREFERENCES_FILE), b"preferences").unwrap();
        fs::create_dir_all(dirs.cache.join("media")).unwrap();
        fs::write(dirs.cache.join("media/library.jpg"), b"library-media").unwrap();
        fs::create_dir_all(dirs.data.join("game-media")).unwrap();
        fs::write(dirs.data.join("game-media/selected.jpg"), b"selected").unwrap();

        let before = dirs.service().data_usage().unwrap();
        assert_eq!(before.derived_cache_entries, 2);
        let after = dirs.service().clear_derived_cache().unwrap();
        assert_eq!(after, DataUsageDto::default());
        assert!(dirs.data.join("catalog.json").is_file());
        assert!(dirs.data.join(PREFERENCES_FILE).is_file());
        assert!(dirs.cache.join("media/library.jpg").is_file());
        assert!(dirs.data.join("game-media/selected.jpg").is_file());
    }
}
