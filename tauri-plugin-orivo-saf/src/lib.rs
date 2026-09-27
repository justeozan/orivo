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
            Self::Unsupported => formatter.write_str("the document picker is Android-only"),
            Self::Picker(detail) => write!(formatter, "the document picker failed: {detail}"),
        }
    }
}

impl std::error::Error for Error {}

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
