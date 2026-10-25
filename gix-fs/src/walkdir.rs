use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::HashMap,
    ffi::OsStr,
    fs::FileType,
    path::{Path, PathBuf},
};

pub use walkdir::Error;
use walkdir::{DirEntry as DirEntryImpl, WalkDir as WalkDirImpl};

/// A directory entry returned by [DirEntryIter].
pub type DirEntry = crate::precompose::DirEntry<DirEntryImpl>;
/// A platform to create a [DirEntryIter] from.
pub type WalkDir = crate::precompose::WalkDir<WalkDirImpl>;

impl crate::precompose::DirEntryApi for DirEntryImpl {
    fn path(&self) -> Cow<'_, Path> {
        self.path().into()
    }

    fn file_name(&self) -> Cow<'_, OsStr> {
        self.file_name().into()
    }

    fn file_type(&self) -> std::io::Result<FileType> {
        Ok(self.file_type())
    }
}

impl IntoIterator for WalkDir {
    type Item = Result<DirEntry, walkdir::Error>;
    type IntoIter = DirEntryIter;

    fn into_iter(self) -> Self::IntoIter {
        DirEntryIter {
            inner: self.inner.expect("always set (builder fix)").into_iter(),
            precompose_unicode: self.precompose_unicode,
        }
    }
}

impl WalkDir {
    /// Set the minimum component depth of paths of entries.
    pub fn min_depth(mut self, min: usize) -> Self {
        self.inner = Some(self.inner.take().expect("always set").min_depth(min));
        self
    }
    /// Set the maximum component depth of paths of entries.
    pub fn max_depth(mut self, max: usize) -> Self {
        self.inner = Some(self.inner.take().expect("always set").max_depth(max));
        self
    }
    /// Follow symbolic links.
    ///
    /// A walk from [`walkdir_sorted_new()`] keeps ordering links like files; use
    /// [`walkdir_sorted_new_follow_links()`] to order links to directories as directories.
    pub fn follow_links(mut self, toggle: bool) -> Self {
        self.inner = Some(self.inner.take().expect("always set").follow_links(toggle));
        self
    }
}

/// Instantiate a new directory iterator which will not skip hidden files.
///
/// Use `precompose_unicode` to represent the `core.precomposeUnicode` configuration option.
pub fn walkdir_new(root: &Path, precompose_unicode: bool) -> WalkDir {
    WalkDir {
        inner: WalkDirImpl::new(root).into(),
        precompose_unicode,
    }
}

/// Instantiate  new directory iterator which will not skip hidden files and uses Git's directory-aware sorting.
///
/// Use `precompose_unicode` to represent the `core.precomposeUnicode` configuration option.
/// Use `max_depth` to limit the depth of the recursive walk.
///   * `0`
///       - Returns only the root path with no children
///   * `1`
///       - Root directory and children.
///   * `1..n`
///       - Root directory, children and {n}-grandchildren
///
/// Symbolic links aren't followed, and are ordered like files.
pub fn walkdir_sorted_new(root: &Path, max_depth: usize, precompose_unicode: bool) -> WalkDir {
    WalkDir {
        inner: WalkDirImpl::new(root)
            .max_depth(max_depth)
            .sort_by(|a, b| git_directory_order(a, a.file_type().is_dir(), b, b.file_type().is_dir()))
            .into(),
        precompose_unicode,
    }
}

/// Like [`walkdir_sorted_new()`], but follow symbolic links as `follow_links(true)` does, and order each link by the
/// type of its target: a link to a directory is walked and ordered as a directory, which is how Git lists loose
/// references.
///
/// A link whose target can't be read is ordered like a file, and the walk returns an error in its place, as it does
/// for a link to a directory that is already being walked.
pub fn walkdir_sorted_new_follow_links(root: &Path, max_depth: usize, precompose_unicode: bool) -> WalkDir {
    // Each link is classified once, so that its position can't change while its directory is sorted.
    let mut link_is_dir = HashMap::<PathBuf, bool>::new();
    let mut is_dir = move |entry: &DirEntryImpl| {
        if entry.file_type().is_symlink() {
            *link_is_dir
                .entry(entry.path().to_owned())
                .or_insert_with(|| std::fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir()))
        } else {
            entry.file_type().is_dir()
        }
    };
    WalkDir {
        inner: WalkDirImpl::new(root)
            .max_depth(max_depth)
            .follow_links(true)
            .sort_by(move |a, b| {
                let (a_is_dir, b_is_dir) = (is_dir(a), is_dir(b));
                git_directory_order(a, a_is_dir, b, b_is_dir)
            })
            .into(),
        precompose_unicode,
    }
}

/// Order the entries `a` and `b` of one directory as Git does: by name, with directories ordered as if their names
/// ended in `/`.
fn git_directory_order(a: &DirEntryImpl, a_is_dir: bool, b: &DirEntryImpl, b_is_dir: bool) -> Ordering {
    let storage_a;
    let storage_b;
    let a_name = match gix_path::os_str_into_bstr(a.file_name()) {
        Ok(f) => f,
        Err(_) => {
            storage_a = a.file_name().to_string_lossy();
            storage_a.as_ref().into()
        }
    };
    let b_name = match gix_path::os_str_into_bstr(b.file_name()) {
        Ok(f) => f,
        Err(_) => {
            storage_b = b.file_name().to_string_lossy();
            storage_b.as_ref().into()
        }
    };
    // "common." < "common/" < "common0"
    let common = a_name.len().min(b_name.len());
    a_name[..common].cmp(&b_name[..common]).then_with(|| {
        let a = a_name.get(common).or_else(|| a_is_dir.then_some(&b'/'));
        let b = b_name.get(common).or_else(|| b_is_dir.then_some(&b'/'));
        a.cmp(&b)
    })
}

/// The Iterator yielding directory items
pub type DirEntryIter = crate::precompose::WalkDirIter<walkdir::IntoIter, DirEntryImpl, walkdir::Error>;
