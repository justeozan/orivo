//! User-provided wallpaper search credentials, stored on disk in the app data
//! directory.
//!
//! `preferences.rs` deliberately owns no credentials, so these live in their
//! own document. The values are what a user pasted in Settings; environment
//! variables still act as a fallback for development and CI. Nothing here is
//! encrypted — it is a convenience store on the user's own machine, and the
//! WebView never names a URL or a filesystem path.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub const CREDENTIALS_FILE: &str = "credentials.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WallpaperCredentialsDto {
    pub igdb_client_id: String,
    pub igdb_client_secret: String,
    pub google_api_key: String,
    pub google_cse_id: String,
    /// Optional. SteamGridDB is the one source that publishes 4K-class art for
    /// all three library roles, so a reset prefers it when a key is present and
    /// falls back to Steam's official artwork when it is not.
    #[serde(default)]
    pub steamgriddb_api_key: String,
    /// Per-row search term templates for the keyword-driven sources, the way
    /// Playnite exposes one editable term per media field. Empty means "use the
    /// built-in default", so a user who never opens this form still gets the
    /// tuned query rather than a blank one.
    #[serde(default)]
    pub search_term_cover: String,
    #[serde(default)]
    pub search_term_landscape: String,
    #[serde(default)]
    pub search_term_background: String,
    #[serde(default)]
    pub search_term_logo: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WallpaperCredentialsUpdate {
    pub igdb_client_id: Option<String>,
    pub igdb_client_secret: Option<String>,
    pub google_api_key: Option<String>,
    pub google_cse_id: Option<String>,
    pub steamgriddb_api_key: Option<String>,
    pub search_term_cover: Option<String>,
    pub search_term_landscape: Option<String>,
    pub search_term_background: Option<String>,
    pub search_term_logo: Option<String>,
}

/// Shared, mutable credential store. The same instance backs the Settings
/// command and the wallpaper search service, so a saved key is visible to the
/// next search without a restart.
#[derive(Debug)]
pub struct WallpaperCredentialsService {
    path: PathBuf,
    inner: Mutex<WallpaperCredentialsDto>,
}

impl WallpaperCredentialsService {
    pub fn load(path: PathBuf) -> Self {
        let stored = if path.is_file() {
            match fs::read_to_string(&path).and_then(|encoded| {
                serde_json::from_str::<WallpaperCredentialsDto>(&encoded).map_err(io::Error::other)
            }) {
                Ok(dto) => trim_all(dto),
                Err(error) => {
                    eprintln!(
                        "orivo: wallpaper credentials are unreadable ({error}); starting empty"
                    );
                    WallpaperCredentialsDto::default()
                }
            }
        } else {
            WallpaperCredentialsDto::default()
        };
        Self {
            path,
            inner: Mutex::new(stored),
        }
    }

    /// The SteamGridDB key, or `None` when the user has not entered one.
    pub fn steamgriddb_api_key(&self) -> Option<String> {
        let key = self.dto().steamgriddb_api_key;
        (!key.is_empty()).then_some(key)
    }

    pub fn dto(&self) -> WallpaperCredentialsDto {
        self.inner
            .lock()
            .map(|stored| stored.clone())
            .unwrap_or_default()
    }

    pub fn update(
        &self,
        update: WallpaperCredentialsUpdate,
    ) -> Result<WallpaperCredentialsDto, String> {
        let mut next = self.dto();
        if let Some(value) = update.igdb_client_id {
            next.igdb_client_id = value.trim().to_owned();
        }
        if let Some(value) = update.igdb_client_secret {
            next.igdb_client_secret = value.trim().to_owned();
        }
        if let Some(value) = update.google_api_key {
            next.google_api_key = value.trim().to_owned();
        }
        if let Some(value) = update.google_cse_id {
            next.google_cse_id = value.trim().to_owned();
        }
        if let Some(value) = update.steamgriddb_api_key {
            next.steamgriddb_api_key = value.trim().to_owned();
        }
        if let Some(value) = update.search_term_cover {
            next.search_term_cover = value.trim().to_owned();
        }
        if let Some(value) = update.search_term_landscape {
            next.search_term_landscape = value.trim().to_owned();
        }
        if let Some(value) = update.search_term_background {
            next.search_term_background = value.trim().to_owned();
        }
        if let Some(value) = update.search_term_logo {
            next.search_term_logo = value.trim().to_owned();
        }
        self.save(&next)?;
        Ok(next)
    }

    fn save(&self, dto: &WallpaperCredentialsDto) -> Result<(), String> {
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
            ".{CREDENTIALS_FILE}.{}.{}.tmp",
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

fn trim_all(dto: WallpaperCredentialsDto) -> WallpaperCredentialsDto {
    WallpaperCredentialsDto {
        igdb_client_id: dto.igdb_client_id.trim().to_owned(),
        igdb_client_secret: dto.igdb_client_secret.trim().to_owned(),
        google_api_key: dto.google_api_key.trim().to_owned(),
        google_cse_id: dto.google_cse_id.trim().to_owned(),
        steamgriddb_api_key: dto.steamgriddb_api_key.trim().to_owned(),
        search_term_cover: dto.search_term_cover.trim().to_owned(),
        search_term_landscape: dto.search_term_landscape.trim().to_owned(),
        search_term_background: dto.search_term_background.trim().to_owned(),
        search_term_logo: dto.search_term_logo.trim().to_owned(),
    }
}

#[tauri::command]
pub fn get_wallpaper_credentials(
    service: tauri::State<'_, std::sync::Arc<WallpaperCredentialsService>>,
) -> Result<WallpaperCredentialsDto, String> {
    Ok(service.dto())
}

#[tauri::command]
pub fn update_wallpaper_credentials(
    update: WallpaperCredentialsUpdate,
    service: tauri::State<'_, std::sync::Arc<WallpaperCredentialsService>>,
) -> Result<WallpaperCredentialsDto, String> {
    service.update(update)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "orivo-wallpaper-credentials-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn service(&self) -> WallpaperCredentialsService {
            WallpaperCredentialsService::load(self.path.join(CREDENTIALS_FILE))
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn starts_empty_when_nothing_has_been_saved() {
        let root = TestRoot::new("empty");
        assert_eq!(root.service().dto(), WallpaperCredentialsDto::default());
    }

    #[test]
    fn persisted_credentials_survive_a_new_service_instance() {
        let root = TestRoot::new("persist");
        let saved = root
            .service()
            .update(WallpaperCredentialsUpdate {
                igdb_client_id: Some("  client-id  ".into()),
                google_api_key: Some("key".into()),
                ..WallpaperCredentialsUpdate::default()
            })
            .unwrap();
        assert_eq!(saved.igdb_client_id, "client-id");
        assert_eq!(saved.google_api_key, "key");
        assert_eq!(
            root.service().dto(),
            WallpaperCredentialsDto {
                igdb_client_id: "client-id".into(),
                google_api_key: "key".into(),
                ..WallpaperCredentialsDto::default()
            }
        );
    }

    #[test]
    fn clearing_a_field_stores_an_empty_string() {
        let root = TestRoot::new("clear");
        let service = root.service();
        service
            .update(WallpaperCredentialsUpdate {
                igdb_client_secret: Some("secret".into()),
                ..WallpaperCredentialsUpdate::default()
            })
            .unwrap();
        service
            .update(WallpaperCredentialsUpdate {
                igdb_client_secret: Some("".into()),
                ..WallpaperCredentialsUpdate::default()
            })
            .unwrap();
        assert_eq!(service.dto().igdb_client_secret, "");
    }

    #[test]
    fn an_unreadable_file_degrades_to_empty_credentials() {
        let root = TestRoot::new("corrupt");
        fs::write(root.path.join(CREDENTIALS_FILE), b"not json").unwrap();
        assert_eq!(root.service().dto(), WallpaperCredentialsDto::default());
    }

    #[test]
    fn update_is_scoped_to_the_documented_fields() {
        let root = TestRoot::new("scoped");
        let service = root.service();
        let dto = service
            .update(WallpaperCredentialsUpdate {
                google_cse_id: Some("cse".into()),
                ..WallpaperCredentialsUpdate::default()
            })
            .unwrap();
        assert_eq!(dto.google_cse_id, "cse");
        assert_eq!(dto.igdb_client_id, "");
        assert!(Path::new(&root.path.join(CREDENTIALS_FILE)).is_file());
    }
}
