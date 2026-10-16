//! The Android storage-access picker, and nothing else.
//!
//! Orivo talks to Android from Rust. The one call it cannot make that way is the
//! one that *returns*: `ACTION_OPEN_DOCUMENT_TREE` answers through
//! `onActivityResult`, which needs a Java class to arrive on. This plugin owns
//! that class — see `README.md` for why it is a crate and not a file in
//! `src-tauri/gen/android` — and exposes exactly one operation to the host.
//!
//! Nothing here decides anything. What a granted tree may be, and what Orivo is
//! then allowed to read from it, lives in `winlator_saf.rs` next to its tests.

use serde::Deserialize;
use tauri::{
    Manager, Runtime,
    plugin::{Builder, TauriPlugin},
};

#[cfg(target_os = "android")]
const ANDROID_PLUGIN_IDENTIFIER: &str = "io.orivo.saf";

#[derive(Debug)]
pub enum Error {
    /// Every desktop platform, where there is no document provider to pick from.
    Unsupported,
    /// The Kotlin side failed, or the activity went away before it answered.
    Picker(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("this is only available on Android"),
            Self::Picker(detail) => write!(formatter, "Android did not answer: {detail}"),
        }
    }
}

impl std::error::Error for Error {}

/// One game installed on the phone, as `SafPlugin.listInstalledGames` reports
/// it. The pictures are file names inside the app cache's `media` folder, the
/// one directory the WebView may read local files from.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledGame {
    pub package_name: String,
    pub label: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub hero: Option<String>,
    #[serde(default)]
    pub installed_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
struct InstalledGames {
    #[serde(default)]
    games: Vec<InstalledGame>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchPackageArgs<'a> {
    package_name: &'a str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StartStreamArgs<'a> {
    pc_name: &'a str,
    app_name: Option<&'a str>,
}

/// What the picker resolved to. `None` is the ordinary answer: the user backed
/// out of the folder chooser, which is not a failure.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedDocumentTree {
    #[serde(default)]
    pub tree_uri: Option<String>,
}

pub struct OrivoSaf<R: Runtime> {
    #[cfg(target_os = "android")]
    handle: tauri::plugin::PluginHandle<R>,
    /// A function pointer rather than `PhantomData<R>`: the state this is managed
    /// in has to be `Send + Sync`, and a runtime is not required to be either.
    #[cfg(not(target_os = "android"))]
    marker: std::marker::PhantomData<fn() -> R>,
}

impl<R: Runtime> OrivoSaf<R> {
    /// Open the system folder chooser and, if the user picks a folder, take the
    /// persistable read permission on it before answering — the grant has to
    /// outlive this process, and only the thread that received the result may
    /// take it.
    pub async fn pick_document_tree(&self) -> Result<Option<String>, Error> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin_async::<PickedDocumentTree>("pickDocumentTree", ())
                .await
                .map(|picked| picked.tree_uri)
                .map_err(|error| Error::Picker(error.to_string()))
        }
        #[cfg(not(target_os = "android"))]
        {
            Err(Error::Unsupported)
        }
    }
}

impl<R: Runtime> OrivoSaf<R> {
    /// The games installed on this phone, with pictures drawn from their icons.
    pub async fn list_installed_games(&self) -> Result<Vec<InstalledGame>, Error> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin_async::<InstalledGames>("listInstalledGames", ())
                .await
                .map(|answer| answer.games)
                .map_err(|error| Error::Picker(error.to_string()))
        }
        #[cfg(not(target_os = "android"))]
        {
            Err(Error::Unsupported)
        }
    }

    /// Start an installed app through the launch intent its package declares.
    /// `Err` carries `not-installed` when Android knows no such app.
    pub async fn launch_package(&self, package_name: &str) -> Result<(), Error> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin_async::<serde::de::IgnoredAny>(
                    "launchPackage",
                    LaunchPackageArgs { package_name },
                )
                .await
                .map(|_| ())
                .map_err(|error| Error::Picker(error.to_string()))
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = LaunchPackageArgs { package_name };
            Err(Error::Unsupported)
        }
    }
}

impl<R: Runtime> OrivoSaf<R> {
    /// Hand a stream to Moonlight: the PC by the name Moonlight shows for it,
    /// and the game by the name its streaming host lists — or, with no game,
    /// that PC's list. `Err` carries `moonlight-missing` when Moonlight is not
    /// installed.
    pub async fn start_stream(&self, pc_name: &str, app_name: Option<&str>) -> Result<(), Error> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin_async::<serde::de::IgnoredAny>(
                    "startStream",
                    StartStreamArgs { pc_name, app_name },
                )
                .await
                .map(|_| ())
                .map_err(|error| Error::Picker(error.to_string()))
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = StartStreamArgs { pc_name, app_name };
            Err(Error::Unsupported)
        }
    }
}

pub trait OrivoSafExt<R: Runtime> {
    fn orivo_saf(&self) -> &OrivoSaf<R>;
}

impl<R: Runtime, T: Manager<R>> OrivoSafExt<R> for T {
    fn orivo_saf(&self) -> &OrivoSaf<R> {
        self.state::<OrivoSaf<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("orivo-saf")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            let saf = OrivoSaf {
                handle: api.register_android_plugin(ANDROID_PLUGIN_IDENTIFIER, "SafPlugin")?,
            };
            #[cfg(not(target_os = "android"))]
            let saf = {
                let _ = api;
                OrivoSaf::<R> {
                    marker: std::marker::PhantomData,
                }
            };
            app.manage(saf);
            Ok(())
        })
        .build()
}
