//! Store folders the Mac App Store edition may read outside its sandbox.
//!
//! The App Store build runs in the App Sandbox, where `$HOME` is the app's own
//! container and Steam's and Epic's folders cannot be read. Each store's library
//! still comes from the account the player signs in with; what the folders add
//! is which of those games are installed. So the player is asked once, with
//! the system folder picker already open on the store's folder, and macOS
//! remembers the answer as a security-scoped bookmark that is reopened at every
//! launch. Every other build reads the folders directly and has nothing to ask.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// The stores whose folders Orivo reads, and where each lives under the home
/// directory. A granted folder must contain `marker`, so that a wrong pick is
/// refused rather than remembered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreFolder {
    Steam,
    Epic,
}

impl StoreFolder {
    pub fn parse(store: &str) -> Option<Self> {
        match store {
            "steam" => Some(Self::Steam),
            "epic" => Some(Self::Epic),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Steam => "steam",
            Self::Epic => "epic",
        }
    }

    /// Relative to the home directory.
    fn default_location(self) -> &'static str {
        match self {
            Self::Steam => "Library/Application Support/Steam",
            Self::Epic => "Library/Application Support/Epic",
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Steam => "steamapps",
            Self::Epic => "EpicGamesLauncher/Data/Manifests",
        }
    }

    #[cfg(all(target_os = "macos", feature = "app-store"))]
    fn title(self) -> &'static str {
        match self {
            Self::Steam => "Allow Orivo to see which Steam games are installed",
            Self::Epic => "Allow Orivo to see which Epic Games are installed",
        }
    }

    /// The folder the player picked, or the store folder inside it, if it
    /// is one.
    fn accept(self, picked: &Path) -> Option<PathBuf> {
        let candidates = [
            picked.to_path_buf(),
            picked.join(match self {
                Self::Steam => "Steam",
                Self::Epic => "Epic",
            }),
        ];
        candidates
            .into_iter()
            .find(|candidate| candidate.join(self.marker()).is_dir())
    }
}

/// What the frontend shows: whether this build has to ask at all, and which
/// stores are already readable.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoreFolderAccess {
    pub needs_grant: bool,
    pub steam: bool,
    pub epic: bool,
}

#[cfg(all(target_os = "macos", feature = "app-store"))]
mod sandboxed {
    use super::StoreFolder;
    use objc2_foundation::{NSData, NSString, NSURLBookmarkCreationOptions, NSURL};
    use objc2_foundation::NSURLBookmarkResolutionOptions;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// Folders opened for this session, by store.
    static GRANTED: Mutex<BTreeMap<&'static str, PathBuf>> = Mutex::new(BTreeMap::new());

    const DIRECTORY: &str = "folder-access";

    fn bookmark_file(app_data: &Path, store: StoreFolder) -> PathBuf {
        app_data.join(DIRECTORY).join(format!("{}.bookmark", store.key()))
    }

    pub fn granted(store: StoreFolder) -> Option<PathBuf> {
        GRANTED.lock().ok()?.get(store.key()).cloned()
    }

    fn remember(store: StoreFolder, path: PathBuf) {
        if let Ok(mut granted) = GRANTED.lock() {
            granted.insert(store.key(), path);
        }
    }

    /// The player's own home directory. `$HOME` is the sandbox container.
    pub fn real_home() -> Option<PathBuf> {
        use std::ffi::CStr;
        use std::os::unix::ffi::OsStrExt;
        let mut buffer = vec![0 as libc::c_char; 4096];
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut entry,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut found,
            )
        };
        if status != 0 || found.is_null() || entry.pw_dir.is_null() {
            return None;
        }
        let dir = unsafe { CStr::from_ptr(entry.pw_dir) };
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
    }

    fn url_for(path: &Path) -> Option<objc2::rc::Retained<NSURL>> {
        let path = path.to_str()?;
        Some(NSURL::fileURLWithPath(&NSString::from_str(path)))
    }

    fn path_of(url: &NSURL) -> Option<PathBuf> {
        url.path().map(|path| PathBuf::from(path.to_string()))
    }

    /// Reopen every folder granted in an earlier session.
    pub fn restore(app_data: &Path) {
        for store in [StoreFolder::Steam, StoreFolder::Epic] {
            let file = bookmark_file(app_data, store);
            let Ok(bytes) = std::fs::read(&file) else {
                continue;
            };
            let data = NSData::with_bytes(&bytes);
            let mut stale = objc2::runtime::Bool::NO;
            let resolved = unsafe {
                NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
                    &data,
                    NSURLBookmarkResolutionOptions::WithSecurityScope
                        | NSURLBookmarkResolutionOptions::WithoutUI,
                    None,
                    &mut stale,
                )
            };
            let Ok(url) = resolved else {
                // The folder is gone or moved out of reach: ask again.
                let _ = std::fs::remove_file(&file);
                continue;
            };
            if !unsafe { url.startAccessingSecurityScopedResource() } {
                continue;
            }
            let Some(path) = path_of(&url) else { continue };
            if stale.as_bool() {
                let _ = save(app_data, store, &url);
            }
            remember(store, path);
        }
    }

    fn save(app_data: &Path, store: StoreFolder, url: &NSURL) -> Result<(), String> {
        let data = url
            .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
                NSURLBookmarkCreationOptions::WithSecurityScope
                    | NSURLBookmarkCreationOptions::SecurityScopeAllowOnlyReadAccess,
                None,
                None,
            )
            .map_err(|error| error.localizedDescription().to_string())?;
        let file = bookmark_file(app_data, store);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(&file, data.to_vec()).map_err(|error| error.to_string())
    }

    /// Ask for the store's folder with the picker already open on it. Runs on
    /// the main thread (a synchronous command), as AppKit requires.
    pub fn grant(app_data: &Path, store: StoreFolder) -> Result<bool, String> {
        let mut dialog = rfd::FileDialog::new().set_title(store.title());
        if let Some(home) = real_home() {
            let usual = home.join(store.default_location());
            dialog = dialog.set_directory(if usual.is_dir() { usual } else { home });
        }
        let Some(picked) = dialog.pick_folder() else {
            return Ok(false);
        };
        let Some(folder) = store.accept(&picked) else {
            return Err(match store {
                StoreFolder::Steam => {
                    "This is not Steam's folder. It is usually Library › Application Support › Steam in your home folder.".to_string()
                }
                StoreFolder::Epic => {
                    "This is not the Epic Games Launcher's folder. It is usually Library › Application Support › Epic in your home folder.".to_string()
                }
            });
        };
        let url = url_for(&folder).ok_or("This folder cannot be used.")?;
        save(app_data, store, &url)?;
        remember(store, folder);
        Ok(true)
    }
}

/// The folder a store keeps its data in, as this build may read it.
pub fn store_root(store: StoreFolder) -> Option<PathBuf> {
    #[cfg(all(target_os = "macos", feature = "app-store"))]
    {
        sandboxed::granted(store)
    }
    #[cfg(not(all(target_os = "macos", feature = "app-store")))]
    {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(store.default_location()))
    }
}

/// Reopen the folders granted in an earlier session. Nothing to do outside the
/// sandbox.
pub fn restore(_app_data: &Path) {
    #[cfg(all(target_os = "macos", feature = "app-store"))]
    sandboxed::restore(_app_data);
}

pub fn access() -> StoreFolderAccess {
    StoreFolderAccess {
        needs_grant: cfg!(all(target_os = "macos", feature = "app-store")),
        steam: store_root(StoreFolder::Steam)
            .is_some_and(|root| root.join(StoreFolder::Steam.marker()).is_dir()),
        epic: store_root(StoreFolder::Epic)
            .is_some_and(|root| root.join(StoreFolder::Epic.marker()).is_dir()),
    }
}

#[tauri::command]
pub fn get_store_folder_access() -> StoreFolderAccess {
    access()
}

/// Synchronous on purpose: Tauri runs it on the main thread, where the folder
/// picker has to open.
#[tauri::command]
pub fn grant_store_folder_access(
    _app: tauri::AppHandle,
    store: String,
) -> Result<StoreFolderAccess, String> {
    let store = StoreFolder::parse(&store).ok_or("Unknown store.")?;
    #[cfg(all(target_os = "macos", feature = "app-store"))]
    {
        use tauri::Manager;
        let app_data = _app.path().app_data_dir().map_err(|error| error.to_string())?;
        sandboxed::grant(&app_data, store)?;
    }
    let _ = store;
    Ok(access())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_of_the_store_folder_or_its_parent_is_accepted() {
        let root = std::env::temp_dir().join(format!("orivo-folder-access-{}", std::process::id()));
        let steam = root.join("Steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();

        assert_eq!(StoreFolder::Steam.accept(&steam), Some(steam.clone()));
        assert_eq!(StoreFolder::Steam.accept(&root), Some(steam.clone()));
        assert_eq!(StoreFolder::Steam.accept(&steam.join("steamapps")), None);
        assert_eq!(StoreFolder::Epic.accept(&root), None);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn only_the_two_stores_are_known() {
        assert_eq!(StoreFolder::parse("steam"), Some(StoreFolder::Steam));
        assert_eq!(StoreFolder::parse("epic"), Some(StoreFolder::Epic));
        assert_eq!(StoreFolder::parse("../etc"), None);
    }
}
