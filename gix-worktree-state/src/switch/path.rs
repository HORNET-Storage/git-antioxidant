//! Check the directories that lead to paths in the worktree, and find the current working directory in it.

use std::path::{Path, PathBuf};

use bstr::{BStr, BString, ByteSlice};
use gix_error::{ErrorExt, Result, message};

/// What the directories that lead to a path in the worktree are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeadingPath {
    /// They all exist and are directories.
    Directories,
    /// One of them doesn't exist.
    Missing,
    /// The leading path of this length is a file or a symbolic link.
    NotADirectory(usize),
}

/// Checks the directories that lead to paths in the worktree. The leading path that consists of directories is
/// remembered, so that the paths in a directory only check what they don't share.
pub(crate) struct LeadingPaths<'a> {
    worktree: &'a Path,
    verified: BString,
    path: PathBuf,
}

impl<'a> LeadingPaths<'a> {
    /// Check the directories that lead to paths in `worktree`.
    pub(crate) fn new(worktree: &'a Path) -> Self {
        LeadingPaths {
            worktree,
            verified: BString::default(),
            path: PathBuf::new(),
        }
    }

    /// Return what the directories that lead to `rela_path` are, as Git's `check_leading_path()` finds out.
    pub(crate) fn check(&mut self, rela_path: &BStr) -> Result<LeadingPath> {
        let Some(directory_len) = rela_path.rfind_byte(b'/') else {
            return Ok(LeadingPath::Directories);
        };
        let directory = &rela_path[..directory_len];
        let mut verified = common_directory_len(&self.verified, directory);
        self.verified.truncate(verified);
        while verified < directory.len() {
            let start = if verified == 0 { 0 } else { verified + 1 };
            let end = directory[start..]
                .find_byte(b'/')
                .map_or(directory.len(), |slash| start + slash);
            let leading = directory[..end].as_bstr();
            self.path.clear();
            self.path.push(self.worktree);
            self.path.push(gix_path::from_bstr(leading)?);
            match std::fs::symlink_metadata(&self.path) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => return Ok(LeadingPath::NotADirectory(end)),
                Err(err) if gix_fs::io_err::is_not_found(err.kind(), err.raw_os_error()) => {
                    return Ok(LeadingPath::Missing);
                }
                Err(err) => return Err(err.and_raise(message!("Could not read the metadata of '{leading}'"))),
            }
            self.verified.extend_from_slice(&directory[verified..end]);
            verified = end;
        }
        Ok(LeadingPath::Directories)
    }
}

/// Return the length of the leading directories that the directories `a` and `b` share.
fn common_directory_len(a: &[u8], b: &[u8]) -> usize {
    let common = a.iter().zip(b).take_while(|(a, b)| a == b).count();
    let ends_component = |path: &[u8]| path.get(common).is_none_or(|byte| *byte == b'/');
    if ends_component(a) && ends_component(b) {
        common
    } else {
        a[..common].rfind_byte(b'/').unwrap_or(0)
    }
}

/// Return the path of the current working directory `current_dir` relative to `worktree`, with slashes, if it is a
/// directory in `worktree`, as Git finds the directory that it never removes. Both paths are resolved with
/// [`gix_path::realpath_opts()`] and precomposed as `fs` says, and a relative `worktree` is relative to `current_dir`.
/// The worktree itself is `None`, as it is never removed anyway.
pub(crate) fn relative_current_dir(worktree: &Path, current_dir: &Path, fs: gix_fs::Capabilities) -> Option<BString> {
    let realpath = |path: &Path| -> Option<BString> {
        let path = gix_path::realpath_opts(path, current_dir, gix_path::realpath::MAX_SYMLINKS).ok()?;
        let path = if fs.precompose_unicode {
            gix_utils::str::precompose_path(path.into()).into_owned()
        } else {
            path
        };
        let path = gix_path::into_bstr(path).ok()?;
        Some(gix_path::to_unix_separators_on_windows(path).into_owned())
    };
    let worktree = realpath(worktree)?;
    let current_dir = realpath(current_dir)?;
    let worktree = worktree.strip_suffix(b"/").unwrap_or(&worktree);
    let (head, rest) = current_dir.split_at_checked(worktree.len())?;
    match rest {
        [b'/', rest @ ..] if !rest.is_empty() && equals(head, worktree, fs.ignore_case) => Some(rest.into()),
        _ => None,
    }
}

/// Return `true` if the directory `cwd` is `rela_path` or in it.
pub(crate) fn is_at_or_in(cwd: &BStr, rela_path: &BStr, ignore_case: bool) -> bool {
    cwd.get(..rela_path.len())
        .is_some_and(|head| equals(head, rela_path, ignore_case))
        && cwd.get(rela_path.len()).is_none_or(|byte| *byte == b'/')
}

/// Return `true` if the directory `cwd` is `rela_path`.
pub(crate) fn is_at(cwd: &BStr, rela_path: &BStr, ignore_case: bool) -> bool {
    cwd.len() == rela_path.len() && equals(cwd, rela_path, ignore_case)
}

/// Return `true` if `a` and `b` are the same path, ignoring the case of ASCII letters if `ignore_case` is set.
fn equals(a: &[u8], b: &[u8], ignore_case: bool) -> bool {
    if ignore_case { a.eq_ignore_ascii_case(b) } else { a == b }
}
