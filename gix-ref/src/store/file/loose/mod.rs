use std::{io, path::Path};

use crate::{BString, FullName, Kind, Target};

/// A git _ref_ which is stored in a file.
#[derive(Debug, PartialOrd, PartialEq, Ord, Eq, Hash, Clone)]
pub struct Reference {
    /// The path to uniquely identify this ref within its store.
    pub name: FullName,
    /// The target of the reference, either a symbolic reference by full name or an object by its id.
    pub target: Target,
}

impl Reference {
    /// Return the kind of ref.
    pub fn kind(&self) -> Kind {
        self.target.kind()
    }
}

/// What [`open()`] found at the path of a loose reference.
pub(crate) enum Opened {
    /// The file to read the reference from, which may be the target of a symbolic link that doesn't name a reference.
    File(std::fs::File),
    /// The contents of a symbolic reference, for a symbolic link whose text names a reference.
    SymbolicLink(Vec<u8>),
}

/// Open the loose reference at `path` as Git's files backend reads it.
///
/// A symbolic link whose text is a full reference name starting with `refs/` is a symbolic reference to that name,
/// wherever the link leads. Any other link is followed, and its target is read as the reference. Errors are those of
/// opening `path`, or the target of a link that doesn't name a reference.
pub(crate) fn open(path: &Path) -> io::Result<Opened> {
    // Like Git, look at `path` again if a link is replaced while it is being read.
    let mut retries = 2;
    loop {
        if let gix_fs::FileOrSymlink::File(file) = gix_fs::open_read_only_no_follow(path)? {
            return Ok(Opened::File(file));
        }
        match std::fs::read_link(path) {
            Ok(text) => {
                return match link_reference_name(&text) {
                    Some(name) => {
                        let mut contents = b"ref: ".to_vec();
                        contents.extend_from_slice(&name);
                        Ok(Opened::SymbolicLink(contents))
                    }
                    None => std::fs::File::open(path).map(Opened::File),
                };
            }
            Err(err) if retries > 0 && matches!(err.kind(), io::ErrorKind::NotFound | io::ErrorKind::InvalidInput) => {
                retries -= 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Return the reference that the `text` of a symbolic link names, if it is a full name in `refs/` as Git requires.
fn link_reference_name(text: &Path) -> Option<BString> {
    let text = gix_path::to_unix_separators_on_windows(gix_path::into_bstr(text).ok()?);
    (text.starts_with(b"refs/") && gix_validate::reference::name(&text).is_ok()).then(|| text.into_owned())
}

///
pub(crate) mod reflog;

///
pub(crate) mod iter;
///
pub mod reference;

mod init {
    use std::path::PathBuf;

    use crate::store_impl::file;

    impl file::Store {
        /// Create a new instance at the given `git_dir`, which commonly is a standard git repository with a
        /// `refs/` subdirectory.
        /// Use [`at_opts()`](Self::at_opts) to adjust options.
        ///
        /// Note that if [`precompose_unicode`](crate::store::init::Options::precompose_unicode) is set in the options,
        /// the `git_dir` is also expected to use precomposed unicode, or else some operations that strip prefixes will fail.
        pub fn at(git_dir: PathBuf, object_hash: gix_hash::Kind) -> Self {
            Self::at_opts(git_dir, object_hash, Default::default())
        }

        /// Create a new instance at the given `git_dir`, which commonly is a standard git repository with a
        /// `refs/` subdirectory.
        /// Use [`Options`](crate::store::init::Options) to adjust settings.
        ///
        /// Note that if [`precompose_unicode`](crate::store::init::Options::precompose_unicode) is set in the options,
        /// the `git_dir` is also expected to use precomposed unicode, or else some operations that strip prefixes will fail.
        pub fn at_opts(
            git_dir: PathBuf,
            object_hash: gix_hash::Kind,
            crate::store::init::Options {
                write_reflog,
                precompose_unicode,
                prohibit_windows_device_names,
            }: crate::store::init::Options,
        ) -> Self {
            file::Store {
                git_dir,
                packed_buffer_mmap_threshold: packed_refs_mmap_threshold(),
                common_dir: None,
                write_reflog,
                namespace: None,
                prohibit_windows_device_names,
                packed: gix_fs::SharedFileSnapshotMut::new().into(),
                object_hash,
                precompose_unicode,
            }
        }

        /// Like [`at()`][file::Store::at()], but for _linked_ work-trees which use `git_dir` as private ref store and `common_dir` for
        /// shared references.
        /// Use [`for_linked_worktree_opts()`](Self::for_linked_worktree_opts) to adjust options.
        ///
        /// Note that if [`precompose_unicode`](crate::store::init::Options::precompose_unicode) is set, the `git_dir` and
        /// `common_dir` are also expected to use precomposed unicode, or else some operations that strip prefixes will fail.
        pub fn for_linked_worktree(git_dir: PathBuf, common_dir: PathBuf, object_hash: gix_hash::Kind) -> Self {
            Self::for_linked_worktree_opts(git_dir, common_dir, object_hash, Default::default())
        }

        /// Like [`at_opts()`][Self::at_opts()], but for _linked_ work-trees which use `git_dir` as private ref store and `common_dir` for
        /// shared references.
        ///
        /// Note that if [`precompose_unicode`](crate::store::init::Options::precompose_unicode) is set, the `git_dir` and
        /// `common_dir` are also expected to use precomposed unicode, or else some operations that strip prefixes will fail.
        pub fn for_linked_worktree_opts(
            git_dir: PathBuf,
            common_dir: PathBuf,
            object_hash: gix_hash::Kind,
            crate::store::init::Options {
                write_reflog,
                precompose_unicode,
                prohibit_windows_device_names,
            }: crate::store::init::Options,
        ) -> Self {
            file::Store {
                git_dir,
                packed_buffer_mmap_threshold: packed_refs_mmap_threshold(),
                common_dir: Some(common_dir),
                write_reflog,
                namespace: None,
                prohibit_windows_device_names,
                packed: gix_fs::SharedFileSnapshotMut::new().into(),
                object_hash,
                precompose_unicode,
            }
        }
    }

    fn packed_refs_mmap_threshold() -> u64 {
        if cfg!(windows) { u64::MAX } else { 32 * 1024 }
    }
}
