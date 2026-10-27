//! Git's rules for trusting repositories that aren't owned by the current user with `safe.directory`.
//!
//! Like Git, values are only read from protected configuration and interpolated like paths. Then they are compared
//! with the directory Git associates with the repository, after both were resolved like Git's `strbuf_realpath()`.
use std::{
    borrow::Cow,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use gix_error::{ErrorExt, ResultExt, bail, message, validation};

use crate::{
    Result,
    bstr::{BStr, BString, ByteSlice, ByteVec},
    config::cache::interpolate_context,
    open::permissions,
};

/// The environment variable that makes Git consider no path owned by the current user, so `safe.directory` decides.
const ASSUME_DIFFERENT_OWNER: &str = "GIT_TEST_ASSUME_DIFFERENT_OWNER";

/// A repository that isn't owned by the current user and isn't allowed by `safe.directory`, which Git refuses to use
/// due to its *dubious ownership*.
///
/// It's raised when opening a repository with [`Options::bail_if_untrusted()`](crate::open::Options::bail_if_untrusted()),
/// and when discovering one with [`TrustPolicy::Required(Full)`](gix_discover::upwards::TrustPolicy::Required).
/// It's classified as [`gix_error::Class::PermissionDenied`] and can be found with
/// [`gix_error::Error::downcast_any_ref()`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DubiousOwnership {
    /// The directory that Git compares with the values of `safe.directory`, resolved and spelled like Git does it,
    /// which makes it the value to add to `safe.directory` to trust the repository.
    ///
    /// It's spelled as it was given if it can't be resolved, which also makes Git refuse the repository.
    pub directory: BString,
    /// The first path of the repository that isn't owned by the current user, along with its owner.
    ///
    /// It's `None` if ownership wasn't checked, like when `GIT_TEST_ASSUME_DIFFERENT_OWNER` is set, or when trust
    /// was set with [`Options::with()`](crate::open::Options::with()).
    pub not_owned: Option<(PathBuf, gix_sec::identity::Ownership)>,
}

impl std::fmt::Display for DubiousOwnership {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let directory = self.directory.as_bstr();
        write!(f, "Detected dubious ownership in repository at '{directory}'")?;
        if let Some((path, ownership)) = &self.not_owned {
            write!(f, "\n'{}' {ownership}", path.display())?;
        }
        // Like Git for Windows, prefix network paths with `%(prefix)/` so the shell of Git for Windows doesn't alter them.
        let mut value = BString::default();
        if directory.starts_with(b"//") {
            write!(f, "\n'{directory}' may refer to a non-local directory")?;
            value.push_str("%(prefix)/");
        }
        value.push_str(directory);
        write!(
            f,
            "\nTo add an exception for this directory, call:\n\n\tgit config --global --add safe.directory {}",
            quote(value.as_bstr())
        )
    }
}

impl std::error::Error for DubiousOwnership {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(const { &gix_error::ClassificationMarker::PERMISSION_DENIED })
    }
}

/// Return `true` if `GIT_TEST_ASSUME_DIFFERENT_OWNER` may be read with the `git_prefix` permission of `env` and is true.
/// Git then considers no path owned by the current user, so `safe.directory` alone decides which repositories are trusted.
///
/// Like Git, fail if the value isn't a boolean.
pub(crate) fn assume_different_owner(env: &permissions::Environment) -> Result<bool> {
    let Some(value) = env
        .git_prefix
        .check_opt(ASSUME_DIFFERENT_OWNER)
        .and_then(std::env::var_os)
    else {
        return Ok(false);
    };
    let value = BString::from(Vec::from_os_str_lossy(&value).into_owned());
    match git_bool(value.as_bstr()) {
        Some(is_set) => Ok(is_set),
        None => bail!(validation(format!(
            "bad boolean environment value '{value}' for '{ASSUME_DIFFERENT_OWNER}'"
        ))),
    }
}

/// Return `true` if `values` of `safe.directory`, in the order they were read from protected configuration, allow the
/// repository whose directory resolves to `resolved` as returned by [`git_realpath()`], like Git's `safe_directory_cb()`.
///
/// An empty value resets prior matches, `*` allows all repositories, and `.` the current directory. All other values
/// must be absolute after interpolation, and are resolved before they are compared. With a trailing `/*`, they allow
/// all directories within, but not the directory itself. `current_dir` is the current directory, and `home` is used for
/// values starting with `~`.
///
/// Like Git, evaluate all values, and fail if one of them can't be interpolated, or is `:(optional)` and can't be accessed.
pub(crate) fn is_safe(resolved: &BStr, values: &[BString], current_dir: &Path, home: Option<&Path>) -> Result<bool> {
    is_safe_with(
        resolved,
        values,
        |value| interpolate(value, current_dir, home),
        |path| git_realpath(path, current_dir),
    )
}

/// Like [`is_safe()`], but with `interpolate()` and `realpath()` to obtain the paths to compare.
fn is_safe_with(
    resolved: &BStr,
    values: &[BString],
    mut interpolate: impl FnMut(&BStr) -> Result<Option<BString>>,
    mut realpath: impl FnMut(&BStr) -> Option<BString>,
) -> Result<bool> {
    let mut is_safe = false;
    for value in values {
        let value = value.as_bstr();
        if value.is_empty() {
            is_safe = false;
            continue;
        } else if value.as_bytes() == b"*" {
            is_safe = true;
            continue;
        }
        let Some(allowed) = interpolate(value)? else {
            continue;
        };
        if !is_absolute(allowed.as_bstr()) && allowed.as_bytes() != b"." {
            gix_trace::warn!("safe.directory '{allowed}' not absolute");
            continue;
        }
        // Git doesn't consider values that can't be resolved, as these may be paths on another machine.
        let Some(normalized) = realpath(allowed.as_bstr()) else {
            continue;
        };
        if normalized.ends_with(b"/*") {
            is_safe |= resolved.starts_with(&normalized[..normalized.len() - 1]);
        } else {
            is_safe |= normalized.as_bstr() == resolved;
        }
    }
    Ok(is_safe)
}

/// Interpolate `value` like Git's `git_config_pathname()`: strip the `:(optional)` prefix, expand `%(prefix)/` and a
/// leading `~`, and return `None` if the path is optional and missing.
///
/// As only Git knows its installation prefix, return `None` with a warning if the path is relative after `%(prefix)/`.
/// `~` is the `home` directory, which is `HOME` in Git, and `~user` the home directory of that user.
fn interpolate(value: &BStr, current_dir: &Path, home: Option<&Path>) -> Result<Option<BString>> {
    let (is_optional, value) = match value.strip_prefix(b":(optional)") {
        Some(value) => (true, value.as_bstr()),
        None => (false, value),
    };
    let path: BString = if let Some(path) = value.strip_prefix(b"%(prefix)/") {
        if !is_absolute(path.as_bstr()) {
            gix_trace::warn!(
                "safe.directory '{value}' is relative to the installation prefix of Git, which is unknown"
            );
            return Ok(None);
        }
        path.into()
    } else if value.starts_with(b"~") {
        let path = gix_config::Path::from(value.to_owned())
            .interpolate(interpolate_context(None, home))
            .or_raise(|| message!("failed to expand user dir in: '{value}'"))?;
        gix_path::into_bstr(path)?.into_owned()
    } else {
        value.to_owned()
    };
    if is_optional {
        match std::fs::metadata(current_dir.join(gix_path::from_bstr(path.as_bstr())?)) {
            Ok(_) => {}
            Err(err) if is_missing(&err) => return Ok(None),
            Err(err) => return Err(err.and_raise(message!("could not stat '{path}'"))),
        }
    }
    Ok(Some(path))
}

/// Parse `value` like Git's `git_parse_maybe_bool()`, as `true`, `yes`, `on`, `false`, `no` or `off` in any case,
/// as `false` if it's empty, or as an integer like [`git_int()`] which is `true` unless it's zero.
fn git_bool(value: &BStr) -> Option<bool> {
    if value.is_empty() {
        return Some(false);
    }
    for (name, is_set) in [
        ("true", true),
        ("yes", true),
        ("on", true),
        ("false", false),
        ("no", false),
        ("off", false),
    ] {
        if value.eq_ignore_ascii_case(name.as_bytes()) {
            return Some(is_set);
        }
    }
    git_int(value).map(|value| value != 0)
}

/// Parse `value` like Git's `git_parse_int()`: an integer as C's `strtoimax()` reads it with automatic base, so with
/// optional leading whitespace and sign, and a `0x` prefix for hexadecimal or `0` prefix for octal digits. It may be
/// followed by one of the units `k`, `m` or `g` in any case, and must fit into an `int` after applying it.
fn git_int(value: &BStr) -> Option<i64> {
    let value = value.as_bytes();
    let value = &value[value.iter().position(|byte| !is_c_space(*byte))?..];
    let (is_negative, value) = match value.split_first() {
        Some((b'-', value)) => (true, value),
        Some((b'+', value)) => (false, value),
        _ => (false, value),
    };
    let (radix, digits) = match value {
        [b'0', b'x' | b'X', digits @ ..] if digits.first().is_some_and(u8::is_ascii_hexdigit) => (16, digits),
        [b'0', ..] => (8, value),
        _ => (10, value),
    };
    let digits_len = digits
        .iter()
        .take_while(|byte| char::from(**byte).is_digit(radix))
        .count();
    if digits_len == 0 {
        return None;
    }
    let (digits, unit) = digits.split_at(digits_len);
    let number = u64::from_str_radix(std::str::from_utf8(digits).ok()?, radix).ok()?;
    let number = if is_negative {
        0_i64.checked_sub_unsigned(number)?
    } else {
        i64::try_from(number).ok()?
    };
    let factor: i64 = match unit {
        [] => 1,
        [unit] if unit.eq_ignore_ascii_case(&b'k') => 1 << 10,
        [unit] if unit.eq_ignore_ascii_case(&b'm') => 1 << 20,
        [unit] if unit.eq_ignore_ascii_case(&b'g') => 1 << 30,
        _ => return None,
    };
    let max = i64::from(i32::MAX);
    if (number < 0 && -max / factor > number) || (number > 0 && max / factor < number) {
        return None;
    }
    Some(number * factor)
}

/// Return `true` if `byte` is whitespace in the C locale.
fn is_c_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

/// Quote `value` for the shell like Git's `sq_quote_buf_pretty()`, which leaves values alone that don't need quoting.
fn quote(value: &BStr) -> Cow<'_, BStr> {
    const OK_PUNCTUATION: &[u8] = b"+,-./:=@_^";
    if !value.is_empty()
        && value
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || OK_PUNCTUATION.contains(byte))
    {
        return Cow::Borrowed(value);
    }
    let mut quoted = BString::from("'");
    for &byte in value.iter() {
        if matches!(byte, b'\'' | b'!') {
            quoted.push_str("'\\");
            quoted.push(byte);
            quoted.push(b'\'');
        } else {
            quoted.push(byte);
        }
    }
    quoted.push(b'\'');
    Cow::Owned(quoted)
}

/// The directory that was compared with the values of `safe.directory` when a repository was opened, to compare it
/// again when the same repository is opened with its options, like [`crate::Repository::reload()`] does.
#[derive(Debug, Clone)]
pub(crate) struct Compared {
    /// The absolute git directory of the repository.
    pub git_dir: PathBuf,
    /// The absolute directory that was compared.
    pub directory: PathBuf,
}

/// Return the first of `paths` that isn't owned by the current user, along with its owner. Missing paths are skipped.
pub(crate) fn first_not_owned(
    paths: impl IntoIterator<Item = PathBuf>,
) -> Option<(PathBuf, gix_sec::identity::Ownership)> {
    paths.into_iter().find_map(|path| {
        let ownership = gix_sec::identity::path_ownership(&path).ok()?;
        (!ownership.is_current_user()).then_some((path, ownership))
    })
}

/// Return the directory Git compares with `safe.directory` after discovering a repository with `git_dir` from `start`:
/// the directory in which the `.git` file or directory was found, or `git_dir` itself if discovery started within it,
/// like it does in bare repositories.
///
/// `worktree_dir` is the worktree inferred by discovery, and relative paths are relative to `current_dir`.
pub(crate) fn discovered_directory(
    start: &Path,
    git_dir: &Path,
    worktree_dir: Option<&Path>,
    current_dir: &Path,
) -> PathBuf {
    let realpath = |path: &Path| gix_path::realpath_opts(path, current_dir, gix_path::realpath::MAX_SYMLINKS).ok();
    let git_dir = current_dir.join(git_dir);
    if realpath(start)
        .zip(realpath(&git_dir))
        .is_some_and(|(start, git_dir)| start.starts_with(git_dir))
    {
        return git_dir;
    }
    match worktree_dir {
        Some(worktree_dir) => current_dir.join(worktree_dir),
        // A `.git` directory with `core.bare = true` was found in its parent directory, which is used by Git.
        None if git_dir.file_name() == Some(OsStr::new(gix_discover::DOT_GIT_DIR)) => {
            let parent = git_dir.parent().map(Path::to_path_buf);
            parent.unwrap_or(git_dir)
        }
        None => git_dir,
    }
}

/// Resolve `path` relative to `current_dir` like Git's `strbuf_realpath()` without dying, or return `None` if that
/// fails, which happens if anything but the last component is missing.
///
/// On Windows, Git for Windows lets the file system resolve existing paths, or the directory of a missing last
/// component, which yields the spelling of the file system, including the case of letters. Otherwise, like for values
/// such as `<dir>/*` which aren't valid names on Windows, all components retain their spelling.
/// Separators are always `/`.
pub(crate) fn git_realpath(path: &BStr, current_dir: &Path) -> Option<BString> {
    #[cfg(windows)]
    if let Some(resolved) = windows::realpath(path, current_dir) {
        return Some(resolved);
    }
    generic_realpath(path, current_dir)
}

/// Resolve `path` with Git's portable `strbuf_realpath()` algorithm, which resolves symbolic links and retains the
/// spelling of all other components. Only the last component may be missing.
fn generic_realpath(path: &BStr, current_dir: &Path) -> Option<BString> {
    if path.is_empty() {
        return None;
    }
    let mut remaining = path.to_owned();
    let mut resolved = root_part(&mut remaining);
    if resolved.is_empty() {
        resolved = current_dir_spelling(current_dir)?;
    }
    let mut num_symlinks = 0;
    while !remaining.is_empty() {
        let next = next_component(&mut remaining);
        if next.is_empty() || next.as_bytes() == b"." {
            continue;
        } else if next.as_bytes() == b".." {
            strip_last_component(&mut resolved);
            continue;
        }
        if resolved.last().is_none_or(|byte| !is_dir_sep(*byte)) {
            resolved.push(b'/');
        }
        resolved.extend_from_slice(&next);
        let resolved_path = gix_path::from_bstr(resolved.as_bstr()).ok()?.into_owned();
        match std::fs::symlink_metadata(&resolved_path) {
            Err(err) => {
                if !is_missing(&err) || !remaining.is_empty() {
                    return None;
                }
            }
            Ok(metadata) if metadata.file_type().is_symlink() => {
                num_symlinks += 1;
                if num_symlinks > gix_path::realpath::MAX_SYMLINKS {
                    return None;
                }
                let mut target = link_target_spelling(std::fs::read_link(&resolved_path).ok()?)?;
                if is_absolute(target.as_bstr()) {
                    resolved = root_part(&mut target);
                } else {
                    strip_last_component(&mut resolved);
                }
                if !remaining.is_empty() {
                    target.push(b'/');
                    target.extend_from_slice(&remaining);
                }
                remaining = target;
            }
            Ok(_) => {}
        }
    }
    Some(resolved)
}

/// Return `true` if `err` means that a path is missing as Git sees it when obtaining its metadata.
///
/// Git for Windows considers all failures missing paths except those denying access or lacking memory, which includes
/// names that are invalid on Windows, like the `*` of `<dir>/*`.
fn is_missing(err: &std::io::Error) -> bool {
    if cfg!(windows) {
        /// `ERROR_ACCESS_DENIED`, `ERROR_NOT_ENOUGH_MEMORY`, `ERROR_SHARING_VIOLATION`, `ERROR_LOCK_VIOLATION`,
        /// `ERROR_SHARING_BUFFER_EXCEEDED` and `ERROR_BUFFER_OVERFLOW`, the errors Git for Windows reports differently.
        const NOT_MISSING: [i32; 6] = [5, 8, 32, 33, 36, 111];
        err.raw_os_error()
            .map_or(err.kind() == std::io::ErrorKind::NotFound, |code| {
                !NOT_MISSING.contains(&code)
            })
    } else {
        err.kind() == std::io::ErrorKind::NotFound
    }
}

/// Spell `current_dir` like Git's `strbuf_getcwd()`, which Git for Windows resolves like an existing path.
fn current_dir_spelling(current_dir: &Path) -> Option<BString> {
    #[cfg(windows)]
    if let Ok(resolved) = std::fs::canonicalize(current_dir) {
        return windows::spelling(resolved);
    }
    let mut current_dir = gix_path::into_bstr(current_dir).ok()?.into_owned();
    convert_slashes(&mut current_dir);
    Some(current_dir)
}

/// Spell the `target` of a symbolic link like Git's `readlink()`, which Git for Windows turns into a regular path with
/// `/` as separator.
fn link_target_spelling(target: PathBuf) -> Option<BString> {
    if cfg!(windows) {
        let target = target.into_os_string().into_string().ok()?;
        Some(nt_path_spelling(&target).into())
    } else {
        Some(gix_path::into_bstr(target).ok()?.into_owned())
    }
}

/// Spell a Windows path like Git for Windows' `normalize_ntpath()`: without the `\\?\` prefix of verbatim paths, with
/// `//server/share` for network paths, and with `/` as separator.
#[cfg_attr(not(windows), allow(dead_code))]
fn nt_path_spelling(path: &str) -> String {
    let path = if path.starts_with('\\') {
        let path = path
            .strip_prefix(r"\??\")
            .or_else(|| path.strip_prefix(r"\\?\"))
            .unwrap_or(path);
        match path.get(..4) {
            Some(unc) if unc.eq_ignore_ascii_case("UNC\\") => format!(r"\\{}", &path[4..]),
            _ => path.to_owned(),
        }
    } else {
        path.to_owned()
    };
    path.replace('\\', "/")
}

/// Return `true` if `path` is absolute for Git, which on Windows includes paths that start with a drive letter.
fn is_absolute(path: &BStr) -> bool {
    path.first().is_some_and(|byte| is_dir_sep(*byte)) || (cfg!(windows) && has_dos_drive_prefix(path))
}

/// Return `true` if `path` starts with a drive letter followed by a colon.
fn has_dos_drive_prefix(path: &[u8]) -> bool {
    matches!(path, [drive, b':', ..] if drive.is_ascii_alphabetic())
}

/// Return `true` if `byte` separates path components, which on Windows includes backslashes.
fn is_dir_sep(byte: u8) -> bool {
    byte == b'/' || (cfg!(windows) && byte == b'\\')
}

/// Return the length of the root of `path` like Git's `offset_1st_component()`: a leading separator, and on Windows a
/// drive prefix or the server and share of a network path, each with the separator that follows.
fn offset_1st_component(path: &[u8]) -> usize {
    let mut offset = 0;
    if cfg!(windows) {
        if has_dos_drive_prefix(path) {
            offset = 2;
        } else if path.len() >= 2 && is_dir_sep(path[0]) && is_dir_sep(path[1]) {
            let Some(server_len) = path[2..].iter().position(|byte| is_dir_sep(*byte)) else {
                return 0;
            };
            offset = 2 + server_len + 1;
            offset += path[offset..]
                .iter()
                .position(|byte| is_dir_sep(*byte))
                .unwrap_or(path.len() - offset);
        }
    }
    offset + usize::from(path.get(offset).is_some_and(|byte| is_dir_sep(*byte)))
}

/// Remove the root of `path` as determined by [`offset_1st_component()`], and return it with Git's separators.
fn root_part(path: &mut BString) -> BString {
    let offset = offset_1st_component(path);
    let mut root = BString::from(path.drain(..offset).collect::<Vec<u8>>());
    convert_slashes(&mut root);
    root
}

/// Turn backslashes into slashes on Windows, like Git's `convert_slashes()`.
fn convert_slashes(path: &mut BString) {
    if cfg!(windows) {
        for byte in path.iter_mut().filter(|byte| **byte == b'\\') {
            *byte = b'/';
        }
    }
}

/// Remove the next component from `remaining` along with the separators preceding it, and return it, like Git's
/// `get_next_component()`.
fn next_component(remaining: &mut BString) -> BString {
    let start = remaining
        .iter()
        .position(|byte| !is_dir_sep(*byte))
        .unwrap_or(remaining.len());
    let end = remaining[start..]
        .iter()
        .position(|byte| is_dir_sep(*byte))
        .map_or(remaining.len(), |len| start + len);
    let component = remaining[start..end].into();
    remaining.drain(..end);
    component
}

/// Remove the last component of `path` along with the separators preceding it, but keep its root, like Git's
/// `strip_last_component()`.
fn strip_last_component(path: &mut BString) {
    let offset = offset_1st_component(path);
    let mut len = path.len();
    while offset < len && !is_dir_sep(path[len - 1]) {
        len -= 1;
    }
    while offset < len && is_dir_sep(path[len - 1]) {
        len -= 1;
    }
    path.truncate(len);
}

#[cfg(windows)]
mod windows {
    use std::path::{Path, PathBuf};

    use crate::bstr::{BStr, BString, ByteSlice, ByteVec};

    /// The Windows error code for a missing file, as opposed to a missing directory leading to it.
    const ERROR_FILE_NOT_FOUND: i32 = 2;

    /// Resolve `path` with the file system like Git for Windows' `mingw_strbuf_realpath()`, or return `None` to let
    /// the caller fall back to the portable algorithm, as Git does.
    pub(super) fn realpath(path: &BStr, current_dir: &Path) -> Option<BString> {
        let path = path.to_str().ok()?;
        match std::fs::canonicalize(current_dir.join(path)) {
            Ok(resolved) => spelling(resolved),
            Err(err) if err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => {
                // The last component may be missing, so resolve the directory containing it instead.
                let separator = path.rfind(['/', '\\']).filter(|pos| *pos > 0)?;
                // Keep the separator after a drive, as `C:` is the current directory of that drive.
                let directory = if path.as_bytes()[separator - 1] == b':' {
                    &path[..=separator]
                } else {
                    &path[..separator]
                };
                let mut resolved = spelling(std::fs::canonicalize(current_dir.join(directory)).ok()?)?;
                if !resolved.ends_with(b"/") {
                    resolved.push(b'/');
                }
                resolved.push_str(&path[separator + 1..]);
                Some(resolved)
            }
            Err(_) => None,
        }
    }

    /// Spell a path that the file system resolved like Git for Windows does, see [`super::nt_path_spelling()`].
    pub(super) fn spelling(resolved: PathBuf) -> Option<BString> {
        let resolved = resolved.into_os_string().into_string().ok()?;
        Some(super::nt_path_spelling(&resolved).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(values: &[&str]) -> Vec<BString> {
        values.iter().map(|value| BString::from(*value)).collect()
    }

    /// Decide with values that are already resolved, and a current directory of `/cwd`.
    fn is_safe(resolved: &str, safe_dirs: &[&str]) -> bool {
        is_safe_with(
            resolved.into(),
            &values(safe_dirs),
            |value| Ok(Some(value.to_owned())),
            |path| {
                Some(if path.as_bytes() == b"." {
                    "/cwd".into()
                } else {
                    path.to_owned()
                })
            },
        )
        .expect("interpolation doesn't fail")
    }

    #[test]
    fn values_are_evaluated_in_order() {
        assert!(!is_safe("/r/repo", &[]), "nothing is safe by default");
        assert!(is_safe("/r/repo", &["/r/repo"]));
        assert!(
            !is_safe("/r/repo", &["/r/repo", ""]),
            "an empty value resets prior matches"
        );
        assert!(is_safe("/r/repo", &["", "/r/repo"]));
        assert!(
            is_safe("/r/repo", &["/r/repo", "/r/other"]),
            "later values don't undo a match"
        );
        assert!(is_safe("/r/repo", &["*"]));
        assert!(!is_safe("/r/repo", &["*", ""]));
        assert!(is_safe("/r/repo", &["*", "/r/other", "", "/r/repo"]));
    }

    #[test]
    fn values_ending_in_a_wildcard_allow_directories_within() {
        assert!(is_safe("/r/repo", &["/r/*"]));
        assert!(is_safe("/r/a/b/repo", &["/r/*"]));
        assert!(is_safe("/r/repo", &["/*"]));
        assert!(!is_safe("/r/repo", &["/r/repo/*"]), "the directory itself isn't within");
        assert!(!is_safe("/r/repo", &["/r*"]), "only `/*` is a wildcard");
        assert!(!is_safe("/r/repo", &["/r/re*"]));
        assert!(!is_safe("/rr/repo", &["/r/*"]));
        assert!(!is_safe("/r/repo", &["/r/*/"]));
        assert!(!is_safe("/r/repo", &["/r/*/repo"]));
    }

    #[test]
    fn values_must_be_absolute_or_a_dot() {
        assert!(!is_safe("/cwd/repo", &["repo"]));
        assert!(!is_safe("/cwd", &["./"]));
        assert!(is_safe("/cwd", &["."]), "the current directory");
        assert!(!is_safe("/cwd/repo", &["."]));
    }

    #[test]
    fn values_that_cant_be_resolved_are_skipped() {
        let is_safe = |safe_dirs: &[&str]| {
            is_safe_with(
                "/r/repo".into(),
                &values(safe_dirs),
                |value| Ok(Some(value.to_owned())),
                |path| (path.as_bytes() != b"/r/missing").then(|| path.to_owned()),
            )
            .expect("interpolation doesn't fail")
        };
        assert!(!is_safe(&["/r/missing"]));
        assert!(is_safe(&["/r/missing", "/r/repo"]));
        assert!(is_safe(&["/r/repo", "/r/missing"]));
    }

    #[test]
    fn interpolation_failures_are_errors_even_after_a_match() {
        let outcome = is_safe_with(
            "/r/repo".into(),
            &values(&["/r/repo", "~nobody/repo"]),
            |value| {
                if value.starts_with(b"~") {
                    Err(message!("failed to expand user dir in: '{value}'").raise())
                } else {
                    Ok(Some(value.to_owned()))
                }
            },
            |path| Some(path.to_owned()),
        );
        assert!(outcome.is_err(), "Git dies even if a prior value matched");
    }

    #[test]
    fn optional_and_prefixed_values_are_interpolated_like_git() -> gix_testtools::Result {
        let dir = gix_testtools::tempfile::tempdir()?;
        let existing = gix_path::into_bstr(dir.path())?.into_owned();
        let mut optional = BString::from(":(optional)");
        optional.push_str(&existing);
        assert_eq!(
            interpolate(optional.as_bstr(), dir.path(), None)?,
            Some(existing.clone())
        );
        let mut missing = optional.clone();
        missing.push_str("/missing");
        assert_eq!(
            interpolate(missing.as_bstr(), dir.path(), None)?,
            None,
            "skipped if missing"
        );

        let mut prefixed = BString::from("%(prefix)/");
        prefixed.push_str(&existing);
        assert_eq!(
            interpolate(prefixed.as_bstr(), dir.path(), None)?,
            Some(existing.clone()),
            "an absolute path after the prefix is used as is"
        );
        assert_eq!(
            interpolate("%(prefix)/relative".into(), dir.path(), None)?,
            None,
            "only Git knows where it is installed"
        );

        assert_eq!(
            interpolate("~".into(), dir.path(), Some(dir.path()))?,
            Some(existing.clone())
        );
        assert!(
            interpolate("~/repo".into(), dir.path(), None).is_err(),
            "like Git, fail if there is no home directory"
        );
        assert_eq!(
            interpolate("relative".into(), dir.path(), None)?,
            Some("relative".into()),
            "other values are used as is"
        );
        Ok(())
    }

    #[test]
    fn booleans_are_parsed_like_git() {
        // These values were tried with `GIT_TEST_ASSUME_DIFFERENT_OWNER` and Git 2.55.0.
        for value in ["1", "true", "yes", "on", "TRUE", "2", "-1", "1k", " 1", "0x1", "07"] {
            assert_eq!(git_bool(value.into()), Some(true), "{value:?}");
        }
        for value in ["0", "false", "no", "off", "", "00", "0k", "0x0", "-0"] {
            assert_eq!(git_bool(value.into()), Some(false), "{value:?}");
        }
        for value in ["maybe", "1 ", "08", "0x", "1kb", " ", "-", "2147483648", "2097152k"] {
            assert_eq!(git_bool(value.into()), None, "{value:?}");
        }
        assert_eq!(git_int("2147483647".into()), Some(2147483647));
        assert_eq!(git_int("-2147483647".into()), Some(-2147483647));
        assert_eq!(git_int("2097151k".into()), Some(2097151 * 1024));
        assert_eq!(git_int("-1g".into()), Some(-(1 << 30)));
    }

    #[test]
    fn values_are_quoted_like_git() {
        assert_eq!(&*quote("C:/Users/a/repo".into()), "C:/Users/a/repo");
        assert_eq!(&*quote("/home/a/my repo".into()), "'/home/a/my repo'");
        assert_eq!(&*quote("/a/it's!".into()), r"'/a/it'\''s'\!''");
        assert_eq!(&*quote("".into()), "''");
        assert_eq!(
            &*quote("%(prefix)///server/share/repo".into()),
            "'%(prefix)///server/share/repo'"
        );
    }

    #[test]
    fn network_paths_are_explained() {
        let error = DubiousOwnership {
            directory: "//server/share/repo".into(),
            not_owned: None,
        };
        assert_eq!(
            error.to_string(),
            "Detected dubious ownership in repository at '//server/share/repo'\n\
             '//server/share/repo' may refer to a non-local directory\n\
             To add an exception for this directory, call:\n\n\
             \tgit config --global --add safe.directory '%(prefix)///server/share/repo'"
        );
    }

    #[test]
    fn windows_paths_are_spelled_like_git_for_windows() {
        assert_eq!(nt_path_spelling(r"\\?\C:\Users\a"), "C:/Users/a");
        assert_eq!(nt_path_spelling(r"\\?\UNC\server\share\a"), "//server/share/a");
        assert_eq!(nt_path_spelling(r"\??\C:\a"), "C:/a");
        assert_eq!(nt_path_spelling(r"C:\a\b"), "C:/a/b");
        assert_eq!(nt_path_spelling(r"..\a"), "../a");
    }

    #[test]
    fn roots_are_found_like_git() {
        assert_eq!(offset_1st_component(b"/a/b"), 1);
        assert_eq!(offset_1st_component(b"a/b"), 0);
        if cfg!(windows) {
            assert_eq!(offset_1st_component(b"C:/a"), 3);
            assert_eq!(offset_1st_component(b"C:a"), 2);
            assert_eq!(offset_1st_component(br"C:\a"), 3);
            assert_eq!(offset_1st_component(b"//server/share/a"), 15);
            assert_eq!(offset_1st_component(b"//server/share"), 14);
            assert_eq!(offset_1st_component(b"//server"), 0, "malformed");
        } else {
            assert_eq!(offset_1st_component(b"C:/a"), 0);
            assert_eq!(offset_1st_component(b"//server/share/a"), 1);
        }

        let mut path = BString::from("/a/b//");
        strip_last_component(&mut path);
        assert_eq!(path, "/a/b", "like Git, only trailing separators are removed");
        strip_last_component(&mut path);
        assert_eq!(path, "/a");
        strip_last_component(&mut path);
        assert_eq!(path, "/", "the root remains");

        let mut remaining = BString::from("//a//b/");
        assert_eq!(next_component(&mut remaining), "a");
        assert_eq!(remaining, "//b/");
        assert_eq!(next_component(&mut remaining), "b");
        assert_eq!(
            remaining, "/",
            "a trailing separator remains to fail on missing last components"
        );
    }

    #[test]
    fn only_the_last_component_may_be_missing() -> gix_testtools::Result {
        let dir = gix_testtools::tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join("sub"))?;
        // Like the current directory of a process, this is resolved.
        let cwd = std::fs::canonicalize(dir.path())?;
        let root = git_realpath(gix_path::into_bstr(dir.path())?.as_ref(), &cwd).expect("exists");
        assert_eq!(git_realpath(".".into(), &cwd), Some(root.clone()));
        let mut missing = root.clone();
        missing.push_str("/missing");
        assert_eq!(git_realpath(missing.as_bstr(), &cwd), Some(missing.clone()));
        let mut trailing = missing.clone();
        trailing.push_str("/");
        assert_eq!(git_realpath(trailing.as_bstr(), &cwd), None);
        let mut deeper = missing.clone();
        deeper.push_str("/deeper");
        assert_eq!(git_realpath(deeper.as_bstr(), &cwd), None);
        let mut wildcard = root.clone();
        wildcard.push_str("/*");
        assert_eq!(
            git_realpath(wildcard.as_bstr(), &cwd),
            Some(wildcard.clone()),
            "a wildcard is resolved like a missing last component"
        );
        let mut dots = root.clone();
        dots.push_str("/./sub/..");
        assert_eq!(git_realpath(dots.as_bstr(), &cwd), Some(root));
        Ok(())
    }
}
