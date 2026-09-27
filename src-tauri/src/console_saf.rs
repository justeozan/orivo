//! Reading a ROM folder through the storage access framework.
//!
//! The rules are Winlator's, because they are the platform's: on API 30+ a file
//! in shared storage that is not media is unreachable by pathname without
//! `MANAGE_EXTERNAL_STORAGE`, Orivo's manifest asks for `INTERNET` and nothing
//! else, and `src-tauri/gen/` is not tracked — so a manifest permission is not
//! something a commit here could deliver. `ACTION_OPEN_DOCUMENT_TREE` needs no
//! manifest at all. [`crate::winlator_saf::DocumentTreeGrant`] already decides
//! which provider and which volume may become a path, and refuses a grant on a
//! folder every app can drop a file into; both are reused here rather than
//! written a second time, because a second copy of a security rule is a second
//! place for it to be wrong.
//!
//! What is new is the *shape of the read*. A `.desktop` file is a few hundred
//! bytes and is read whole; a ROM can be a 1.5 GB disc image, and neither an
//! import nor a launch may read all of it. So this module asks a document for its
//! length and for a bounded prefix, and [`crate::console_runner`] decides what
//! that is enough to prove.
//!
//! Everything above the JNI boundary is pure, and tested against a fake provider.

use crate::console_runner::ConsoleRunnerError;
use crate::winlator_saf::TreeDocument;

/// A document's byte length, and as much of its head as was asked for.
///
/// `length` is the *file's* length, never the number of bytes returned: the
/// fingerprint is built from both, and a length that silently meant "how much I
/// read" would make every large image hash the same.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomBytes {
    pub length: u64,
    pub head: Vec<u8>,
}

/// The two things the Android side of a ROM folder is asked to do.
///
/// Keeping it this narrow is what lets the bounded walk, the extensions, the
/// cancellation and the fingerprint be exercised on a host against a fake.
pub trait RomDocumentTree {
    /// List one directory's immediate children. Order is the provider's, so
    /// callers sort.
    fn children(&self, document_id: &str) -> Result<Vec<TreeDocument>, ConsoleRunnerError>;

    /// One document's length, and up to `max_bytes` of its head.
    fn read_head(&self, document_id: &str, max_bytes: u64) -> Result<RomBytes, ConsoleRunnerError>;
}

/// The reader for one granted tree on this platform.
///
/// Off Android this is a reader that refuses. Compiling and wiring it everywhere
/// is deliberate, for the same reason Winlator's is: the rules about which
/// document may become which path are host rules, they are tested on a host, and
/// a desktop build that met a profile carrying a tree URI has to refuse it rather
/// than quietly read the pathname instead.
pub fn rom_document_tree(tree_uri: &str) -> Box<dyn RomDocumentTree> {
    #[cfg(target_os = "android")]
    {
        Box::new(android::AndroidRomTree::new(tree_uri))
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
    use super::{ConsoleRunnerError, RomBytes, RomDocumentTree, TreeDocument};

    pub struct NoDocumentProvider;

    impl RomDocumentTree for NoDocumentProvider {
        fn children(&self, _document_id: &str) -> Result<Vec<TreeDocument>, ConsoleRunnerError> {
            Err(ConsoleRunnerError::RomFolderUnsupported)
        }

        fn read_head(
            &self,
            _document_id: &str,
            _max_bytes: u64,
        ) -> Result<RomBytes, ConsoleRunnerError> {
            Err(ConsoleRunnerError::RomFolderUnsupported)
        }
    }
}

/// The JNI half.
///
/// The listing is Winlator's, verbatim: the same `DocumentsContract` query, and
/// in particular the same one-local-reference-frame-per-row discipline, which
/// exists because Android 7 aborts the process rather than returning an error
/// when a frame's 512 references run out. Reading a ROM is the part that is not
/// Winlator's, because a document too large to read is the normal case here
/// rather than a refusal.
#[cfg(target_os = "android")]
mod android {
    use super::{ConsoleRunnerError, RomBytes, RomDocumentTree, TreeDocument};
    use crate::winlator_saf::{
        DocumentTree,
        android::{content_resolver, parse_uri, with_env},
        document_tree,
    };
    use jni::objects::{JObject, JObjectArray, JValue};

    /// One transfer buffer per chunk. A ROM head is measured in mebibytes, so
    /// this is a compromise between JNI round trips and a byte array the
    /// collector has to keep alive.
    const READ_CHUNK_BYTES: usize = 64 * 1024;

    /// `OpenableColumns.SIZE`. Asked for by name because a provider answers a
    /// projection, and the one column that matters is cheaper than a full row.
    const SIZE_COLUMN: &str = "_size";

    pub struct AndroidRomTree {
        tree_uri: String,
        /// The listing, which is exactly Winlator's and is not rewritten here.
        listing: Box<dyn DocumentTree>,
    }

    impl AndroidRomTree {
        pub fn new(tree_uri: &str) -> Self {
            Self {
                tree_uri: tree_uri.to_string(),
                listing: document_tree(tree_uri),
            }
        }
    }

    impl RomDocumentTree for AndroidRomTree {
        fn children(&self, document_id: &str) -> Result<Vec<TreeDocument>, ConsoleRunnerError> {
            Ok(self.listing.children(document_id)?)
        }

        fn read_head(
            &self,
            document_id: &str,
            max_bytes: u64,
        ) -> Result<RomBytes, ConsoleRunnerError> {
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
                let mut head: Vec<u8> = Vec::new();
                // One byte past the bound is what tells a bounded prefix from a
                // whole file, which is the difference between the two kinds of
                // fingerprint this feeds.
                let mut ended = false;
                while (head.len() as u64) <= max_bytes {
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
                        ended = true;
                        break;
                    }
                    let mut chunk = vec![0i8; read as usize];
                    env.get_byte_array_region(&buffer, 0, &mut chunk)?;
                    head.extend(chunk.into_iter().map(|byte| byte as u8));
                }
                env.call_method(&stream, "close", "()V", &[])?;

                // A file the read reached the end of has an exact length already.
                // Anything larger has to be asked for, and a provider that will
                // not say is refused rather than fingerprinted without it: a
                // digest of a prefix with no length attached would be the same
                // for a file and for that file with anything appended.
                let length = if ended {
                    Some(head.len() as u64)
                } else {
                    document_size(env, &resolver, &document)?
                };
                head.truncate(max_bytes as usize);
                Ok(length.map(|length| RomBytes { length, head }))
            })?
            .ok_or(ConsoleRunnerError::RomUnreadable)
        }
    }

    /// `_size` for one document, or `None` when the provider does not answer it.
    fn document_size(
        env: &mut jni::JNIEnv<'_>,
        resolver: &JObject<'_>,
        document: &JObject<'_>,
    ) -> jni::errors::Result<Option<u64>> {
        let string_class = env.find_class("java/lang/String")?;
        let projection: JObjectArray<'_> =
            env.new_object_array(1, &string_class, JObject::null())?;
        let column = env.new_string(SIZE_COLUMN)?;
        env.set_object_array_element(&projection, 0, &column)?;
        let cursor = env
            .call_method(
                resolver,
                "query",
                "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
                &[
                    document.into(),
                    (&projection).into(),
                    (&JObject::null()).into(),
                    (&JObject::null()).into(),
                    (&JObject::null()).into(),
                ],
            )?
            .l()?;
        if cursor.is_null() {
            return Ok(None);
        }
        let size = read_size_row(env, &cursor);
        // The cursor is closed whether or not the row was readable: one left open
        // holds the provider's own resources.
        let _ = env.call_method(&cursor, "close", "()V", &[]);
        size
    }

    fn read_size_row(
        env: &mut jni::JNIEnv<'_>,
        cursor: &JObject<'_>,
    ) -> jni::errors::Result<Option<u64>> {
        if !env.call_method(cursor, "moveToFirst", "()Z", &[])?.z()? {
            return Ok(None);
        }
        if env
            .call_method(cursor, "isNull", "(I)Z", &[JValue::Int(0)])?
            .z()?
        {
            return Ok(None);
        }
        let size = env
            .call_method(cursor, "getLong", "(I)J", &[JValue::Int(0)])?
            .j()?;
        Ok(u64::try_from(size).ok())
    }
}

/// A `RomDocumentTree` that answers from memory, so the bounded walk, the
/// cancellation, the scope checks and both kinds of fingerprint are exercised on
/// the host — and so a provider that lies, or one that will not say how large a
/// file is, can be written down as a test rather than imagined.
///
/// Handles share one state, because a test has to be able to rewrite a document
/// *after* handing the tree to the source that reads it. That is what another app
/// dropping a file into the folder looks like from in here.
#[cfg(test)]
pub(crate) mod fake {
    use super::{ConsoleRunnerError, RomBytes, RomDocumentTree, TreeDocument};
    use crate::winlator_saf::DIRECTORY_MIME_TYPE;
    use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

    #[derive(Default)]
    struct FakeRomState {
        documents: BTreeMap<String, Vec<u8>>,
        directories: BTreeMap<String, Vec<String>>,
        /// Rows a real provider could never return for this parent.
        strays: BTreeMap<String, Vec<TreeDocument>>,
        unreadable: Vec<String>,
        /// Documents whose `_size` the provider refuses to answer.
        sizeless: Vec<String>,
        /// A length the provider claims that the bytes do not have, so "trust the
        /// row, not the file" is a test rather than a hope.
        claimed_length: BTreeMap<String, u64>,
        reads: usize,
    }

    #[derive(Clone, Default)]
    pub(crate) struct FakeRomTree {
        state: Rc<RefCell<FakeRomState>>,
    }

    impl FakeRomTree {
        pub(crate) fn new(root: &str) -> Self {
            let tree = Self::default();
            tree.state
                .borrow_mut()
                .directories
                .insert(root.to_string(), Vec::new());
            tree
        }

        pub(crate) fn with_document(self, document_id: &str, contents: &[u8]) -> Self {
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

        pub(crate) fn with_no_size_for(self, document_id: &str) -> Self {
            self.state
                .borrow_mut()
                .sizeless
                .push(document_id.to_string());
            self
        }

        /// Stand in for a file too large to hold in a test's memory: the bytes are
        /// whatever was written, the length is what the provider reports.
        pub(crate) fn with_claimed_length(self, document_id: &str, length: u64) -> Self {
            self.state
                .borrow_mut()
                .claimed_length
                .insert(document_id.to_string(), length);
            self
        }

        pub(crate) fn write(&self, document_id: &str, contents: &[u8]) {
            self.attach(document_id);
            self.state
                .borrow_mut()
                .documents
                .insert(document_id.to_string(), contents.to_vec());
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

    impl RomDocumentTree for FakeRomTree {
        fn children(&self, document_id: &str) -> Result<Vec<TreeDocument>, ConsoleRunnerError> {
            let (children, strays) = {
                let state = self.state.borrow();
                let children = state
                    .directories
                    .get(document_id)
                    .cloned()
                    .ok_or(ConsoleRunnerError::AccessDenied)?;
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

        fn read_head(
            &self,
            document_id: &str,
            max_bytes: u64,
        ) -> Result<RomBytes, ConsoleRunnerError> {
            let mut state = self.state.borrow_mut();
            state.reads += 1;
            if state.unreadable.iter().any(|id| id == document_id) {
                return Err(ConsoleRunnerError::AccessDenied);
            }
            let bytes = state
                .documents
                .get(document_id)
                .cloned()
                .ok_or(ConsoleRunnerError::RomMissing)?;
            // A provider that will not say how large a document is has not
            // answered half of what identifies it, so the read fails rather than
            // producing a digest with no length behind it.
            if state.sizeless.iter().any(|id| id == document_id) {
                return Err(ConsoleRunnerError::RomUnreadable);
            }
            let length = state
                .claimed_length
                .get(document_id)
                .copied()
                .unwrap_or(bytes.len() as u64);
            let mut head = bytes;
            head.truncate(max_bytes as usize);
            Ok(RomBytes { length, head })
        }
    }
}
