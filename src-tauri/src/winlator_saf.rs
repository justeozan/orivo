//! Reading Winlator's export folder through the storage access framework.
//!
//! Winlator writes its exported shortcuts into shared storage, and on API 30+ a
//! `.desktop` file there is unreachable by pathname without
//! `MANAGE_EXTERNAL_STORAGE`. Orivo's manifest asks for `INTERNET` and nothing
//! else, and `src-tauri/gen/` is not tracked, so a manifest permission is not a
//! thing this repository can even deliver. SAF is the way in that needs no
//! manifest at all: the user points at the folder once, Orivo keeps a
//! persistable grant on it, and every read goes through a `ContentResolver`.
//!
//! That trade has one sharp edge, and it is the reason this module exists.
//! Winlator's `shortcut_path` extra is a **file path**, not a content URI — it
//! opens the file itself, with its own permissions. So Orivo has to be able to
//! say, without guessing, when a document it can read *is* a file at a path
//! Winlator can open. Exactly one provider allows that claim to be made:
//! `com.android.externalstorage.documents`, for documents on the primary volume,
//! whose identifiers are `primary:<relative path>` under
//! `Environment.getExternalStorageDirectory()`. Every other authority, volume or
//! shape is refused with a sentence instead of a guessed path.
//!
//! Everything above the JNI boundary is pure, and tested against a fake tree.

use crate::winlator_runner::WinlatorRunnerError;
use std::path::{Path, PathBuf};

/// The one `DocumentsProvider` whose documents Orivo can turn back into a path.
/// Downloads (`com.android.providers.downloads.documents`), media, and every
/// cloud provider hand out identifiers that name a row, not a file.
pub const EXTERNAL_STORAGE_AUTHORITY: &str = "com.android.externalstorage.documents";

/// The primary shared volume. A removable volume is `<serial>:` and mounts at a
/// path Orivo would have to infer, so it is refused rather than guessed.
pub const PRIMARY_VOLUME_PREFIX: &str = "primary:";

/// `DocumentsContract.Document.MIME_TYPE_DIR`.
pub const DIRECTORY_MIME_TYPE: &str = "vnd.android.document/directory";

/// A document identifier is one row of a provider's table; these bounds keep a
/// hostile or broken provider from turning a listing into unbounded work.
const MAX_DOCUMENT_ID_BYTES: usize = 1_024;
const MAX_TREE_URI_BYTES: usize = 2_048;

/// One folder the user granted through `ACTION_OPEN_DOCUMENT_TREE`, already
/// resolved to the filesystem directory it stands for.
///
/// Holding both halves is the point: the tree URI is how Orivo *reads* the
/// folder, and the directory is what Winlator *opens*. They are checked against
/// each other once, here, so no later step has to trust one and hope about the
/// other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentTreeGrant {
    tree_uri: String,
    tree_document_id: String,
    /// What every identifier inside this grant starts with. It is not always the
    /// tree's own identifier plus a separator: a grant on the whole shared volume
    /// is `primary:`, and its children are `primary:Download/…` with no slash of
    /// their own.
    child_prefix: String,
    directory: PathBuf,
}

impl DocumentTreeGrant {
    /// Resolve a persisted tree URI against the device's primary shared volume.
    ///
    /// `external_storage_root` is `Environment.getExternalStorageDirectory()` on
    /// a device. It is a parameter rather than a constant because hard-coding
    /// `/storage/emulated/0` is an assumption about a device, and because it is
    /// what makes this function testable on a host.
    pub fn parse(
        tree_uri: &str,
        external_storage_root: &Path,
    ) -> Result<Self, WinlatorRunnerError> {
        if tree_uri.len() > MAX_TREE_URI_BYTES || !external_storage_root.is_absolute() {
            return Err(WinlatorRunnerError::ExportFolderUnsupported);
        }
        let rest = tree_uri
            .strip_prefix("content://")
            .ok_or(WinlatorRunnerError::ExportFolderUnsupported)?;
        let (authority, path) = rest
            .split_once('/')
            .ok_or(WinlatorRunnerError::ExportFolderUnsupported)?;
        if authority != EXTERNAL_STORAGE_AUTHORITY {
            return Err(WinlatorRunnerError::ExportFolderUnsupported);
        }
        // A picked tree is exactly `tree/<document id>`. The longer
        // `tree/<id>/document/<id>` form addresses one document inside a tree
        // and is not a grant, so it is refused rather than truncated.
        let encoded = path
            .strip_prefix("tree/")
            .ok_or(WinlatorRunnerError::ExportFolderUnsupported)?
            .trim_end_matches('/');
        if encoded.contains('/') || encoded.contains('?') || encoded.contains('#') {
            return Err(WinlatorRunnerError::ExportFolderUnsupported);
        }
        let tree_document_id =
            percent_decode(encoded).ok_or(WinlatorRunnerError::ExportFolderUnsupported)?;
        let relative = primary_volume_path(&tree_document_id)
            .ok_or(WinlatorRunnerError::ExportFolderUnsupported)?;
        let directory = if relative.is_empty() {
            external_storage_root.to_path_buf()
        } else {
            external_storage_root.join(relative)
        };
        let child_prefix = if tree_document_id.ends_with(':') {
            tree_document_id.clone()
        } else {
            format!("{tree_document_id}/")
        };
        Ok(Self {
            tree_uri: tree_uri.to_string(),
            tree_document_id,
            child_prefix,
            directory,
        })
    }

    pub fn tree_uri(&self) -> &str {
        &self.tree_uri
    }

    pub fn tree_document_id(&self) -> &str {
        &self.tree_document_id
    }

    /// The filesystem directory this grant stands for — what a shortcut path
    /// inside it is checked against, and what Winlator ultimately opens.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The path a document inside this grant has on the device.
    ///
    /// Both halves are checked: the identifier has to sit under the granted
    /// tree, *and* the part below it has to be a plain relative path. A `..` in
    /// there looks perfectly inside the tree to the provider and would leave the
    /// granted folder on the filesystem.
    pub fn path_for(&self, document_id: &str) -> Result<PathBuf, WinlatorRunnerError> {
        let relative = document_id
            .strip_prefix(&self.child_prefix)
            .filter(|relative| plain_relative_path(relative))
            .ok_or(WinlatorRunnerError::ShortcutOutsideScope)?;
        Ok(self.directory.join(relative))
    }

    /// The document identifier for a path inside this grant — the inverse of
    /// [`Self::path_for`], and how a stored inventory path is read back through
    /// SAF at launch without storing a second copy of it in the catalog.
    pub fn document_id_for(&self, path: &Path) -> Result<String, WinlatorRunnerError> {
        let relative = path
            .strip_prefix(&self.directory)
            .ok()
            .and_then(|relative| relative.to_str())
            .filter(|relative| plain_relative_path(relative))
            .ok_or(WinlatorRunnerError::ShortcutOutsideScope)?;
        let document_id = format!("{}{relative}", self.child_prefix);
        (document_id.len() <= MAX_DOCUMENT_ID_BYTES)
            .then_some(document_id)
            .ok_or(WinlatorRunnerError::ShortcutOutsideScope)
    }
}

/// The relative path a `primary:…` identifier names under the shared volume, or
/// `None` when it names something Orivo cannot turn into a path.
fn primary_volume_path(document_id: &str) -> Option<&str> {
    if document_id.len() > MAX_DOCUMENT_ID_BYTES {
        return None;
    }
    let relative = document_id.strip_prefix(PRIMARY_VOLUME_PREFIX)?;
    (relative.is_empty() || plain_relative_path(relative)).then_some(relative)
}

/// Is this a relative path made only of ordinary names? Leading, trailing and
/// doubled separators, `.`, `..` and control characters are all refused rather
/// than normalised: a shortcut is either exactly where it was found or nowhere.
fn plain_relative_path(relative: &str) -> bool {
    !relative.is_empty()
        && relative.len() <= MAX_DOCUMENT_ID_BYTES
        && relative.split('/').all(|component| {
            !component.is_empty()
                && component != "."
                && component != ".."
                && !component.chars().any(char::is_control)
        })
}

/// Decode the one percent-encoded segment a tree URI carries. `+` is left alone
/// on purpose: it means a plus in a path segment, and a space only in a query.
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = value.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// One row of a `DocumentsContract` children listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeDocument {
    pub document_id: String,
    pub display_name: String,
    pub mime_type: String,
    pub size: Option<u64>,
}

impl TreeDocument {
    pub fn is_directory(&self) -> bool {
        self.mime_type == DIRECTORY_MIME_TYPE
    }
}

/// The only thing the Android side of SAF is asked to do. Keeping it this narrow
/// is what lets the scanner, the bounds, the cancellation and the fingerprint be
/// exercised on a host with a fake tree.
pub trait DocumentTree {
    /// List one directory's immediate children. Order is the provider's, so
    /// callers sort.
    fn children(&self, document_id: &str) -> Result<Vec<TreeDocument>, WinlatorRunnerError>;

    /// Read one document, refusing anything larger than `max_bytes` rather than
    /// returning a truncated file that would hash to nothing meaningful.
    fn read(&self, document_id: &str, max_bytes: u64) -> Result<Vec<u8>, WinlatorRunnerError>;
}

#[cfg(target_os = "android")]
pub use android::{external_storage_root, persisted_read_tree_uris};
#[cfg(not(target_os = "android"))]
pub use desktop::{external_storage_root, persisted_read_tree_uris};

/// The reader for one granted tree on this platform.
///
/// Off Android this is a reader that refuses. Compiling and wiring it everywhere
/// is deliberate: the rules above — which provider, which volume, which document
/// may become which path — are host rules, they are tested on a host, and a
/// desktop build that met a profile carrying a tree URI has to refuse it rather
/// than quietly read the pathname instead.
pub fn document_tree(tree_uri: &str) -> Box<dyn DocumentTree> {
    #[cfg(target_os = "android")]
    {
        Box::new(android::AndroidDocumentTree::new(tree_uri))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = tree_uri;
        Box::new(desktop::NoDocumentProvider)
    }
}

/// There is no document provider on a desktop, and no honest way to invent one.
#[cfg(not(target_os = "android"))]
mod desktop {
    use super::{DocumentTree, TreeDocument};
    use crate::winlator_runner::WinlatorRunnerError;
    use std::path::PathBuf;

    pub fn external_storage_root() -> Result<PathBuf, WinlatorRunnerError> {
        Err(WinlatorRunnerError::ExportFolderUnsupported)
    }

    pub fn persisted_read_tree_uris() -> Result<Vec<String>, WinlatorRunnerError> {
        Err(WinlatorRunnerError::ExportFolderUnsupported)
    }

    pub struct NoDocumentProvider;

    impl DocumentTree for NoDocumentProvider {
        fn children(&self, _document_id: &str) -> Result<Vec<TreeDocument>, WinlatorRunnerError> {
            Err(WinlatorRunnerError::ExportFolderUnsupported)
        }

        fn read(
            &self,
            _document_id: &str,
            _max_bytes: u64,
        ) -> Result<Vec<u8>, WinlatorRunnerError> {
            Err(WinlatorRunnerError::ExportFolderUnsupported)
        }
    }
}

/// The JNI half. Every call attaches the calling thread to the JVM rather than
/// hopping to the main thread: a folder listing and a handful of file reads have
/// no business on the thread that draws the library.
#[cfg(target_os = "android")]
mod android {
    use super::{DocumentTree, TreeDocument};
    use crate::winlator_runner::WinlatorRunnerError;
    use jni::{
        JNIEnv, JavaVM,
        objects::{GlobalRef, JObject, JObjectArray, JString, JValue},
    };
    use std::{
        path::PathBuf,
        sync::{OnceLock, mpsc},
        time::Duration,
    };

    /// Reaching the activity is one dispatch onto a thread that is already
    /// running. A wait this long only expires when that thread is wedged.
    const CONTEXT_TIMEOUT: Duration = Duration::from_secs(5);
    /// One transfer buffer per read, sized for a file that is a few `key=value`
    /// lines. A document larger than the caller's bound is refused, not looped.
    const READ_CHUNK_BYTES: usize = 8 * 1024;

    /// `DocumentsContract.Document` column names. These are public platform
    /// constants; reading them through JNI would cost three calls to learn what
    /// the documentation already pins.
    const COLUMNS: [&str; 4] = ["document_id", "_display_name", "mime_type", "_size"];

    struct AndroidContext {
        vm: JavaVM,
        activity: GlobalRef,
    }

    /// The VM pointer and the activity are process-lived, so they are taken once
    /// and reused. `ndk_context` is deliberately not used: it is initialised by
    /// the credential store on the main thread, and depending on that order from
    /// a background scan would be a race nobody could see in a test.
    fn android_context() -> Result<&'static AndroidContext, WinlatorRunnerError> {
        static CONTEXT: OnceLock<Option<AndroidContext>> = OnceLock::new();
        CONTEXT
            .get_or_init(|| {
                let (sender, receiver) = mpsc::sync_channel(1);
                tauri::wry::prelude::dispatch(move |env, activity, _webview| {
                    let context = match (env.get_java_vm(), env.new_global_ref(activity)) {
                        (Ok(vm), Ok(activity)) => Some(AndroidContext { vm, activity }),
                        _ => None,
                    };
                    let _ = sender.send(context);
                });
                receiver.recv_timeout(CONTEXT_TIMEOUT).ok().flatten()
            })
            .as_ref()
            .ok_or(WinlatorRunnerError::ExportFolderAccessLost)
    }

    /// Run one JNI unit of work on the calling thread, turning any pending Java
    /// exception into an Orivo error instead of leaving it armed for the next
    /// call on this thread.
    fn with_env<T>(
        work: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> jni::errors::Result<T>,
    ) -> Result<T, WinlatorRunnerError> {
        let context = android_context()?;
        let mut guard = context
            .vm
            .attach_current_thread()
            .map_err(|_| WinlatorRunnerError::ExportFolderAccessLost)?;
        let activity = context.activity.as_obj();
        match work(&mut guard, activity) {
            Ok(value) => Ok(value),
            Err(_) => Err(classify_pending_exception(&mut guard)),
        }
    }

    /// A revoked or never-taken grant surfaces as `SecurityException`, which is
    /// the one case worth its own sentence: the user has to reconnect the folder.
    fn classify_pending_exception(env: &mut JNIEnv<'_>) -> WinlatorRunnerError {
        let Ok(throwable) = env.exception_occurred() else {
            return WinlatorRunnerError::AccessDenied;
        };
        // Logcat is the only place a device-side failure can be read afterwards.
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        if throwable.is_null() {
            return WinlatorRunnerError::AccessDenied;
        }
        match java_class_name(env, &throwable).as_deref() {
            Some("java.lang.SecurityException") => WinlatorRunnerError::ExportFolderAccessLost,
            _ => WinlatorRunnerError::AccessDenied,
        }
    }

    fn java_class_name(env: &mut JNIEnv<'_>, throwable: &JObject<'_>) -> Option<String> {
        let class = env
            .call_method(throwable, "getClass", "()Ljava/lang/Class;", &[])
            .and_then(|class| class.l())
            .ok()?;
        let name = env
            .call_method(&class, "getName", "()Ljava/lang/String;", &[])
            .and_then(|name| name.l())
            .ok()?;
        env.get_string(&name.into())
            .ok()
            .map(|name| name.to_string_lossy().into_owned())
    }

    fn java_string(env: &mut JNIEnv<'_>, value: &JObject<'_>) -> Option<String> {
        if value.is_null() {
            return None;
        }
        let value: &JString<'_> = value.into();
        env.get_string(value)
            .ok()
            .map(|value| value.to_string_lossy().into_owned())
    }

    fn content_resolver<'local>(
        env: &mut JNIEnv<'local>,
        activity: &JObject<'_>,
    ) -> jni::errors::Result<JObject<'local>> {
        env.call_method(
            activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )?
        .l()
    }

    fn parse_uri<'local>(
        env: &mut JNIEnv<'local>,
        value: &str,
    ) -> jni::errors::Result<JObject<'local>> {
        let value = env.new_string(value)?;
        env.call_static_method(
            "android/net/Uri",
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[(&value).into()],
        )?
        .l()
    }

    /// `Environment.getExternalStorageDirectory()`. Asked for rather than
    /// assumed, because `/storage/emulated/0` is a convention and not a promise.
    pub fn external_storage_root() -> Result<PathBuf, WinlatorRunnerError> {
        with_env(|env, _activity| {
            let directory = env
                .call_static_method(
                    "android/os/Environment",
                    "getExternalStorageDirectory",
                    "()Ljava/io/File;",
                    &[],
                )?
                .l()?;
            let path = env
                .call_method(&directory, "getAbsolutePath", "()Ljava/lang/String;", &[])?
                .l()?;
            Ok(java_string(env, &path).map(PathBuf::from))
        })?
        .ok_or(WinlatorRunnerError::ExportFolderAccessLost)
    }

    /// Every tree URI this process can still read after a restart. A grant the
    /// user revoked from the system settings simply stops being listed here,
    /// which is how Orivo learns about it without reading the folder first.
    pub fn persisted_read_tree_uris() -> Result<Vec<String>, WinlatorRunnerError> {
        with_env(|env, activity| {
            let resolver = content_resolver(env, activity)?;
            let permissions = env
                .call_method(
                    &resolver,
                    "getPersistedUriPermissions",
                    "()Ljava/util/List;",
                    &[],
                )?
                .l()?;
            let count = env.call_method(&permissions, "size", "()I", &[])?.i()?;
            let mut trees = Vec::new();
            for index in 0..count {
                let permission = env
                    .call_method(
                        &permissions,
                        "get",
                        "(I)Ljava/lang/Object;",
                        &[JValue::Int(index)],
                    )?
                    .l()?;
                if !env
                    .call_method(&permission, "isReadPermission", "()Z", &[])?
                    .z()?
                {
                    continue;
                }
                let uri = env
                    .call_method(&permission, "getUri", "()Landroid/net/Uri;", &[])?
                    .l()?;
                let uri = env
                    .call_method(&uri, "toString", "()Ljava/lang/String;", &[])?
                    .l()?;
                if let Some(uri) = java_string(env, &uri) {
                    trees.push(uri);
                }
            }
            Ok(trees)
        })
    }

    /// A [`DocumentTree`] backed by the platform `ContentResolver`, bound to one
    /// granted tree URI.
    pub struct AndroidDocumentTree {
        tree_uri: String,
    }

    impl AndroidDocumentTree {
        pub fn new(tree_uri: &str) -> Self {
            Self {
                tree_uri: tree_uri.to_string(),
            }
        }
    }

    impl DocumentTree for AndroidDocumentTree {
        fn children(&self, document_id: &str) -> Result<Vec<TreeDocument>, WinlatorRunnerError> {
            let tree_uri = self.tree_uri.clone();
            let document_id = document_id.to_string();
            with_env(move |env, activity| {
                let resolver = content_resolver(env, activity)?;
                let tree = parse_uri(env, &tree_uri)?;
                let parent = env.new_string(&document_id)?;
                let children = env
                    .call_static_method(
                        "android/provider/DocumentsContract",
                        "buildChildDocumentsUriUsingTree",
                        "(Landroid/net/Uri;Ljava/lang/String;)Landroid/net/Uri;",
                        &[(&tree).into(), (&parent).into()],
                    )?
                    .l()?;

                let string_class = env.find_class("java/lang/String")?;
                let projection: JObjectArray<'_> =
                    env.new_object_array(COLUMNS.len() as i32, &string_class, JObject::null())?;
                for (index, column) in COLUMNS.iter().enumerate() {
                    let column = env.new_string(column)?;
                    env.set_object_array_element(&projection, index as i32, &column)?;
                }

                let cursor = env
                    .call_method(
                        &resolver,
                        "query",
                        "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
                        &[
                            (&children).into(),
                            (&projection).into(),
                            (&JObject::null()).into(),
                            (&JObject::null()).into(),
                            (&JObject::null()).into(),
                        ],
                    )?
                    .l()?;
                if cursor.is_null() {
                    // A provider that answers nothing is not an error the user
                    // can act on; an empty folder reads the same way.
                    return Ok(Vec::new());
                }

                let mut documents = Vec::new();
                while env.call_method(&cursor, "moveToNext", "()Z", &[])?.z()? {
                    let identifier = env
                        .call_method(
                            &cursor,
                            "getString",
                            "(I)Ljava/lang/String;",
                            &[JValue::Int(0)],
                        )?
                        .l()?;
                    let display_name = env
                        .call_method(
                            &cursor,
                            "getString",
                            "(I)Ljava/lang/String;",
                            &[JValue::Int(1)],
                        )?
                        .l()?;
                    let mime_type = env
                        .call_method(
                            &cursor,
                            "getString",
                            "(I)Ljava/lang/String;",
                            &[JValue::Int(2)],
                        )?
                        .l()?;
                    let size = if env
                        .call_method(&cursor, "isNull", "(I)Z", &[JValue::Int(3)])?
                        .z()?
                    {
                        None
                    } else {
                        u64::try_from(
                            env.call_method(&cursor, "getLong", "(I)J", &[JValue::Int(3)])?
                                .j()?,
                        )
                        .ok()
                    };
                    let (Some(identifier), Some(display_name)) = (
                        java_string(env, &identifier),
                        java_string(env, &display_name),
                    ) else {
                        continue;
                    };
                    documents.push(TreeDocument {
                        document_id: identifier,
                        display_name,
                        mime_type: java_string(env, &mime_type).unwrap_or_default(),
                        size,
                    });
                }
                env.call_method(&cursor, "close", "()V", &[])?;
                Ok(documents)
            })
        }

        fn read(&self, document_id: &str, max_bytes: u64) -> Result<Vec<u8>, WinlatorRunnerError> {
            let tree_uri = self.tree_uri.clone();
            let document_id = document_id.to_string();
            with_env(move |env, activity| {
                let resolver = content_resolver(env, activity)?;
                let tree = parse_uri(env, &tree_uri)?;
                let identifier = env.new_string(&document_id)?;
                let document = env
                    .call_static_method(
                        "android/provider/DocumentsContract",
                        "buildDocumentUriUsingTree",
                        "(Landroid/net/Uri;Ljava/lang/String;)Landroid/net/Uri;",
                        &[(&tree).into(), (&identifier).into()],
                    )?
                    .l()?;
                let stream = env
                    .call_method(
                        &resolver,
                        "openInputStream",
                        "(Landroid/net/Uri;)Ljava/io/InputStream;",
                        &[(&document).into()],
                    )?
                    .l()?;
                if stream.is_null() {
                    return Ok(None);
                }

                let buffer = env.new_byte_array(READ_CHUNK_BYTES as i32)?;
                let mut bytes: Vec<u8> = Vec::new();
                let outcome = loop {
                    let read = env
                        .call_method(
                            &stream,
                            "read",
                            "([BII)I",
                            &[
                                (&buffer).into(),
                                JValue::Int(0),
                                JValue::Int(READ_CHUNK_BYTES as i32),
                            ],
                        )?
                        .i()?;
                    if read <= 0 {
                        break Some(bytes);
                    }
                    let mut chunk = vec![0i8; read as usize];
                    env.get_byte_array_region(&buffer, 0, &mut chunk)?;
                    bytes.extend(chunk.into_iter().map(|byte| byte as u8));
                    // One byte past the bound is enough to know the document is
                    // too large; the caller decides what to say about it.
                    if bytes.len() as u64 > max_bytes {
                        break None;
                    }
                };
                env.call_method(&stream, "close", "()V", &[])?;
                Ok(outcome)
            })?
            .ok_or(WinlatorRunnerError::ShortcutTooLarge)
        }
    }
}

/// A `DocumentTree` that answers from memory, so the bounded walk, the
/// cancellation, the scope checks and the fingerprint are all exercised on the
/// host — and so a provider that lies can be written down as a test rather than
/// imagined.
///
/// Handles share one state, because a test has to be able to rewrite a document
/// *after* handing the tree to the source that reads it — which is exactly what
/// Winlator does when the user re-exports a shortcut.
#[cfg(test)]
pub(crate) mod fake {
    use super::{DIRECTORY_MIME_TYPE, DocumentTree, TreeDocument};
    use crate::winlator_runner::WinlatorRunnerError;
    use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

    #[derive(Default)]
    struct FakeTreeState {
        documents: BTreeMap<String, Vec<u8>>,
        directories: BTreeMap<String, Vec<String>>,
        /// Rows a real provider could never return for this parent, kept as data
        /// so "the provider lied" is a test and not a hypothetical.
        strays: BTreeMap<String, Vec<TreeDocument>>,
        unreadable: Vec<String>,
        reads: usize,
    }

    #[derive(Clone, Default)]
    pub(crate) struct FakeDocumentTree {
        state: Rc<RefCell<FakeTreeState>>,
    }

    impl FakeDocumentTree {
        pub(crate) fn new(root: &str) -> Self {
            let tree = Self::default();
            tree.state
                .borrow_mut()
                .directories
                .insert(root.to_string(), Vec::new());
            tree
        }

        pub(crate) fn with_document(self, document_id: &str, contents: &str) -> Self {
            self.write(document_id, contents);
            self
        }

        pub(crate) fn with_directory(self, document_id: &str) -> Self {
            self.attach(document_id);
            self.state
                .borrow_mut()
                .directories
                .insert(document_id.to_string(), Vec::new());
            self
        }

        pub(crate) fn with_stray_row(self, parent: &str, row: TreeDocument) -> Self {
            self.state
                .borrow_mut()
                .strays
                .entry(parent.to_string())
                .or_default()
                .push(row);
            self
        }

        pub(crate) fn with_unreadable(self, document_id: &str) -> Self {
            self.state
                .borrow_mut()
                .unreadable
                .push(document_id.to_string());
            self
        }

        /// Rewrite a document that is already in the tree, or add one.
        pub(crate) fn write(&self, document_id: &str, contents: &str) {
            self.attach(document_id);
            self.state
                .borrow_mut()
                .documents
                .insert(document_id.to_string(), contents.as_bytes().to_vec());
        }

        pub(crate) fn reads(&self) -> usize {
            self.state.borrow().reads
        }

        fn attach(&self, document_id: &str) {
            let Some((parent, _)) = document_id.rsplit_once('/') else {
                return;
            };
            let mut state = self.state.borrow_mut();
            let children = state.directories.entry(parent.to_string()).or_default();
            if !children.iter().any(|child| child == document_id) {
                children.push(document_id.to_string());
            }
        }

        fn row(&self, document_id: &str) -> TreeDocument {
            let state = self.state.borrow();
            let name = document_id
                .rsplit_once('/')
                .map(|(_, name)| name)
                .unwrap_or(document_id);
            TreeDocument {
                document_id: document_id.to_string(),
                display_name: name.to_string(),
                mime_type: if state.directories.contains_key(document_id) {
                    DIRECTORY_MIME_TYPE.into()
                } else {
                    "application/octet-stream".into()
                },
                size: state
                    .documents
                    .get(document_id)
                    .map(|bytes| bytes.len() as u64),
            }
        }
    }

    impl DocumentTree for FakeDocumentTree {
        fn children(&self, document_id: &str) -> Result<Vec<TreeDocument>, WinlatorRunnerError> {
            let (children, strays) = {
                let state = self.state.borrow();
                let children = state
                    .directories
                    .get(document_id)
                    .cloned()
                    .ok_or(WinlatorRunnerError::AccessDenied)?;
                (
                    children,
                    state.strays.get(document_id).cloned().unwrap_or_default(),
                )
            };
            let mut rows = children
                .iter()
                .map(|child| self.row(child))
                .collect::<Vec<_>>();
            rows.extend(strays);
            Ok(rows)
        }

        fn read(&self, document_id: &str, max_bytes: u64) -> Result<Vec<u8>, WinlatorRunnerError> {
            let mut state = self.state.borrow_mut();
            state.reads += 1;
            if state.unreadable.iter().any(|id| id == document_id) {
                return Err(WinlatorRunnerError::AccessDenied);
            }
            let bytes = state
                .documents
                .get(document_id)
                .ok_or(WinlatorRunnerError::ShortcutMissing)?;
            if bytes.len() as u64 > max_bytes {
                return Err(WinlatorRunnerError::ShortcutTooLarge);
            }
            Ok(bytes.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRONTEND_TREE: &str = "content://com.android.externalstorage.documents/tree/primary%3ADownload%2FWinlator%2FFrontend";

    fn grant() -> DocumentTreeGrant {
        DocumentTreeGrant::parse(FRONTEND_TREE, Path::new("/storage/emulated/0")).unwrap()
    }

    #[test]
    fn resolves_the_folder_winlator_exports_into_to_the_path_winlator_opens() {
        let grant = grant();
        assert_eq!(
            grant.tree_document_id(),
            "primary:Download/Winlator/Frontend"
        );
        assert_eq!(
            grant.directory(),
            Path::new("/storage/emulated/0/Download/Winlator/Frontend")
        );
    }

    /// The whole design rests on this round trip: Orivo reads a document by its
    /// identifier and hands Winlator the path, so the two must describe the same
    /// file in both directions.
    #[test]
    fn maps_a_document_to_a_path_and_back() {
        let grant = grant();
        let document_id = "primary:Download/Winlator/Frontend/Celeste.desktop";
        let path = grant.path_for(document_id).unwrap();
        assert_eq!(
            path,
            Path::new("/storage/emulated/0/Download/Winlator/Frontend/Celeste.desktop")
        );
        assert_eq!(grant.document_id_for(&path).unwrap(), document_id);
    }

    #[test]
    fn maps_a_document_in_a_subfolder_too() {
        let grant = grant();
        let document_id = "primary:Download/Winlator/Frontend/RPG/Ys.desktop";
        let path = grant.path_for(document_id).unwrap();
        assert_eq!(
            path,
            Path::new("/storage/emulated/0/Download/Winlator/Frontend/RPG/Ys.desktop")
        );
        assert_eq!(grant.document_id_for(&path).unwrap(), document_id);
    }

    /// A document identifier is a provider's row, and a provider is free to
    /// answer with any row at all. Anything that is not inside the grant is
    /// refused before it can become a path.
    #[test]
    fn refuses_a_document_outside_the_granted_tree() {
        let grant = grant();
        for document_id in [
            "primary:Download/Other/Celeste.desktop",
            "primary:Download/Winlator/FrontendEvil/Celeste.desktop",
            "primary:Download/Winlator/Frontend",
            "primary:",
            "1234-ABCD:Download/Winlator/Frontend/Celeste.desktop",
        ] {
            assert_eq!(
                grant.path_for(document_id),
                Err(WinlatorRunnerError::ShortcutOutsideScope),
                "accepted {document_id}"
            );
        }
    }

    /// `..` inside a document identifier is the one that would escape on the
    /// filesystem side while looking perfectly inside the tree to the provider.
    #[test]
    fn refuses_a_document_identifier_that_walks_up() {
        let grant = grant();
        for document_id in [
            "primary:Download/Winlator/Frontend/../../../etc/hosts",
            "primary:Download/Winlator/Frontend/./Celeste.desktop",
            "primary:Download/Winlator/Frontend//Celeste.desktop",
        ] {
            assert_eq!(
                grant.path_for(document_id),
                Err(WinlatorRunnerError::ShortcutOutsideScope),
                "accepted {document_id}"
            );
        }
    }

    #[test]
    fn refuses_a_path_outside_the_granted_folder() {
        let grant = grant();
        for path in [
            "/storage/emulated/0/Download/Celeste.desktop",
            "/storage/emulated/0/Download/Winlator/Frontend",
            "/etc/hosts",
        ] {
            assert_eq!(
                grant.document_id_for(Path::new(path)),
                Err(WinlatorRunnerError::ShortcutOutsideScope),
                "accepted {path}"
            );
        }
    }

    /// Every other provider hands out identifiers that name a row rather than a
    /// file, so no path can be claimed from them without inventing one.
    #[test]
    fn refuses_every_provider_that_is_not_primary_external_storage() {
        for tree_uri in [
            "content://com.android.providers.downloads.documents/tree/downloads",
            "content://com.android.externalstorage.documents/tree/1234-ABCD%3AWinlator",
            "content://com.google.android.apps.docs.storage/tree/abc123",
            "content://com.android.externalstorage.documents/document/primary%3ADownload",
            "content://com.android.externalstorage.documents/tree/primary%3ADownload/document/primary%3ADownload%2FCeleste.desktop",
            "file:///storage/emulated/0/Download",
            "content://com.android.externalstorage.documents",
        ] {
            assert_eq!(
                DocumentTreeGrant::parse(tree_uri, Path::new("/storage/emulated/0")),
                Err(WinlatorRunnerError::ExportFolderUnsupported),
                "accepted {tree_uri}"
            );
        }
    }

    /// Granting the whole primary volume is legitimate — a user may well pick
    /// the storage root — and it resolves to the root itself, not to a path with
    /// an empty component in the middle.
    #[test]
    fn resolves_a_grant_on_the_whole_primary_volume() {
        let grant = DocumentTreeGrant::parse(
            "content://com.android.externalstorage.documents/tree/primary%3A",
            Path::new("/storage/emulated/0"),
        )
        .unwrap();
        assert_eq!(grant.directory(), Path::new("/storage/emulated/0"));
        assert_eq!(
            grant.path_for("primary:Download/Celeste.desktop").unwrap(),
            Path::new("/storage/emulated/0/Download/Celeste.desktop")
        );
    }

    #[test]
    fn decodes_only_what_a_path_segment_encodes() {
        assert_eq!(
            percent_decode("primary%3AA%20B%2Bc").as_deref(),
            Some("primary:A B+c")
        );
        assert_eq!(percent_decode("primary%3"), None);
        assert_eq!(percent_decode("primary%zz"), None);
    }
}
