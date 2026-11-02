use bstr::{BStr, ByteSlice};

///
pub mod component {
    /// The error returned by [`component()`](super::component()).
    #[derive(Debug)]
    #[expect(missing_docs)]
    #[non_exhaustive]
    pub enum Error {
        Empty,
        PathSeparator,
        WindowsPathPrefix,
        WindowsReservedName,
        WindowsIllegalCharacter,
        DotGitDir,
        SymlinkedGitModules,
        Relative,
    }

    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Error::Empty => write!(f, "A path component must not be empty"),
                Error::PathSeparator => write!(f, r"Path separators like / or \ are not allowed"),
                Error::WindowsPathPrefix => write!(f, "Windows path prefixes are not allowed"),
                Error::WindowsReservedName => {
                    write!(f, "Windows device-names may have side-effects and are not allowed")
                }
                Error::WindowsIllegalCharacter => write!(
                    f,
                    r#"Trailing spaces or dots, and the following characters anywhere, are forbidden in Windows paths, along with non-printable ones: <>:"|?*"#
                ),
                Error::DotGitDir => write!(f, "The .git name may never be used"),
                Error::SymlinkedGitModules => write!(f, "The .gitmodules file must not be a symlink"),
                Error::Relative => write!(f, "Relative components '.' and '..' are disallowed"),
            }
        }
    }

    impl std::error::Error for Error {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(const { &gix_error::ClassificationMarker::VALIDATION })
        }
    }

    /// Further specify what to check for in [`component()`](super::component())
    ///
    /// Note that the `Default` implementation maximizes safety by enabling all protections.
    #[derive(Debug, Copy, Clone)]
    pub struct Options {
        /// This flag should be turned on when on Windows, but can be turned on when on other platforms
        /// as well to prevent path components that can cause trouble on Windows.
        pub protect_windows: bool,
        /// If `true`, protections for the MacOS HFS+ filesystem will be active, checking for
        /// special directories that we should never write while ignoring codepoints just like HFS+ would.
        ///
        /// This field is equivalent to `core.protectHFS`.
        pub protect_hfs: bool,
        /// If `true`, protections for Windows NTFS specific features will be active. This adds special handling
        /// for `8.3` filenames and alternate data streams, both of which could be used to mask the true name of
        /// what would be created on disk.
        ///
        /// This field is equivalent to `core.protectNTFS`.
        pub protect_ntfs: bool,
    }

    impl Default for Options {
        fn default() -> Self {
            Options {
                protect_windows: true,
                protect_hfs: true,
                protect_ntfs: true,
            }
        }
    }

    /// The kind of the entry that a component names as the last component of its path, if its name needs checks of
    /// its own.
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    pub enum Mode {
        /// The item is a symbolic link.
        Symlink,
    }
}

/// Assure the given `input` resembles a valid name for a tree or blob, and in that sense, a path component.
/// `mode` is the kind of the entry that `input` names as the last component of its path, or `None` for any other
/// component and for entries that need no checks of their own. [`entry()`] checks all components of a path.
///
/// `input` must not make it possible to exit the repository, or to specify absolute paths.
pub fn component(
    input: &BStr,
    mode: Option<component::Mode>,
    component::Options {
        protect_windows,
        protect_hfs,
        protect_ntfs,
    }: component::Options,
) -> Result<&BStr, component::Error> {
    if input.is_empty() {
        return Err(component::Error::Empty);
    }
    if input == ".." || input == "." {
        return Err(component::Error::Relative);
    }
    if protect_windows {
        if input.find_byteset(br"/\").is_some() {
            return Err(component::Error::PathSeparator);
        }
        if input.chars().nth(1) == Some(':') {
            return Err(component::Error::WindowsPathPrefix);
        }
    } else if input.find_byte(b'/').is_some() {
        return Err(component::Error::PathSeparator);
    }
    if protect_hfs {
        if component_is_hfs_dot(input, "git") {
            return Err(component::Error::DotGitDir);
        }
        if is_symlink(mode) && component_is_hfs_dot(input, "gitmodules") {
            return Err(component::Error::SymlinkedGitModules);
        }
    }

    if protect_ntfs {
        for name in ntfs_names(input) {
            if is_dot_git_ntfs(name) {
                return Err(component::Error::DotGitDir);
            }
            if is_symlink(mode) && is_dot_ntfs(name, "gitmodules", "gi7eba") {
                return Err(component::Error::SymlinkedGitModules);
            }
        }

        if protect_windows && let Some(err) = check_win_devices_and_illegal_characters(input) {
            return Err(err);
        }
    }

    if !(protect_hfs | protect_ntfs) {
        if input.eq_ignore_ascii_case(b".git") {
            return Err(component::Error::DotGitDir);
        }
        if is_symlink(mode) && input.eq_ignore_ascii_case(b".gitmodules") {
            return Err(component::Error::SymlinkedGitModules);
        }
    }
    Ok(input)
}

/// Assure that `path`, the slash-separated path of an index entry of the kind `mode`, consists of valid
/// [components](component()) as Git's `verify_path()` decides, with `mode` applying to the last component.
///
/// As in Git, a symbolic link can't lead through a directory named `.gitmodules` either, in any case and, with
/// `protect_hfs`, as HFS+ reads names. With `protect_ntfs`, a name that NTFS takes for `.gitmodules` refuses a
/// directory only if a `:` ends it, as Git's `is_ntfs_dotgitmodules()` ends a name at the end of the path or at a `:`,
/// but not at a directory separator.
///
/// The error of the first invalid component is returned. A path with an empty component, as at its start or its end,
/// is invalid.
pub fn entry(
    path: &BStr,
    mode: Option<component::Mode>,
    options: component::Options,
) -> Result<&BStr, component::Error> {
    let mut start = 0;
    let mut names = path.split_str("/").peekable();
    while let Some(name) = names.next() {
        let name = name.as_bstr();
        let end = start + name.len();
        if names.peek().is_none() {
            component(name, mode, options)?;
            break;
        }
        component(name, None, options)?;
        if is_symlink(mode) && is_dot_gitmodules_directory(name, path[start..=end].as_bstr(), options) {
            return Err(component::Error::SymlinkedGitModules);
        }
        start = end + 1;
    }
    Ok(path)
}

/// Return `true` if Git's `verify_path()` refuses to let a symbolic link lead through the directory `name`, as it takes
/// `name` for `.gitmodules`. `with_separator` is `name` with the `/` that follows it.
fn is_dot_gitmodules_directory(name: &BStr, with_separator: &BStr, options: component::Options) -> bool {
    name.eq_ignore_ascii_case(b".gitmodules")
        || (options.protect_hfs && component_is_hfs_dot(name, "gitmodules"))
        || (options.protect_ntfs && ntfs_names(with_separator).any(|name| is_dot_ntfs(name, "gitmodules", "gi7eba")))
}

/// Return `true` if the path component at `input` looks like a Windows device, like `CON`
/// or `LPT1` (case-insensitively).
///
/// This is relevant only on Windows, where one may be tricked into reading or writing to such devices.
/// When reading from `CON`, a console-program may block until the user provided input.
pub fn component_is_windows_device(input: &BStr) -> bool {
    is_win_device(input)
}

fn is_win_device(input: &BStr) -> bool {
    let Some(in3) = input.get(..3) else { return false };
    if in3.eq_ignore_ascii_case(b"AUX") && is_done_windows(input.get(3..)) {
        return true;
    }
    if in3.eq_ignore_ascii_case(b"NUL") && is_done_windows(input.get(3..)) {
        return true;
    }
    if in3.eq_ignore_ascii_case(b"PRN") && is_done_windows(input.get(3..)) {
        return true;
    }
    // Note that the following allows `COM0`, even though `LPT0` is not allowed.
    // Even though tests seem to indicate that neither `LPT0` nor `COM0` are valid
    // device names, it's unclear this truly is the case in all possible versions and editions
    // of Windows.
    // Hence, justification for this asymmetry is merely to do exactly the same as Git does,
    // and to have exactly the same behaviour during validation (for worktree-writes).
    if in3.eq_ignore_ascii_case(b"COM")
        && input.get(3).is_some_and(|n| *n >= b'1' && *n <= b'9')
        && is_done_windows(input.get(4..))
    {
        return true;
    }
    if in3.eq_ignore_ascii_case(b"LPT")
        && input.get(3).is_some_and(u8::is_ascii_digit)
        && is_done_windows(input.get(4..))
    {
        return true;
    }
    if in3.eq_ignore_ascii_case(b"CON")
        && (is_done_windows(input.get(3..))
            || (input.get(3..6).is_some_and(|n| n.eq_ignore_ascii_case(b"IN$")) && is_done_windows(input.get(6..)))
            || (input.get(3..7).is_some_and(|n| n.eq_ignore_ascii_case(b"OUT$")) && is_done_windows(input.get(7..))))
    {
        return true;
    }
    false
}

fn check_win_devices_and_illegal_characters(input: &BStr) -> Option<component::Error> {
    if is_win_device(input) {
        return Some(component::Error::WindowsReservedName);
    }
    if input.iter().any(|b| *b < 0x20 || b":<>\"|?*".contains(b)) {
        return Some(component::Error::WindowsIllegalCharacter);
    }
    if input.ends_with(b".") || input.ends_with(b" ") {
        return Some(component::Error::WindowsIllegalCharacter);
    }
    None
}

fn is_symlink(mode: Option<component::Mode>) -> bool {
    mode == Some(component::Mode::Symlink)
}

/// Return `true` if HFS+ takes the path component at `input` for a dot followed by `name`, like `.git` for `git`, as
/// Git's `is_hfs_dot_generic()` decides. `name` is lowercase ASCII.
///
/// HFS+ ignores case and some code points when it compares names. Like Git, this reads `input` as UTF-8 that ends at a
/// NUL byte, a directory separator of this platform or an invalid sequence, so a name that continues with invalid
/// UTF-8, U+FFFE or U+FFFF is taken for `name` as well.
pub fn component_is_hfs_dot(input: &BStr, name: &str) -> bool {
    let mut pos = Some(0);
    let mut next = || next_hfs_char(input, &mut pos);
    if next() != u32::from(b'.') {
        return false;
    }
    for expected in name.bytes() {
        let c = next();
        if c > 127 || (c as u8).to_ascii_lowercase() != expected {
            return false;
        }
    }
    let c = next();
    c == 0 || is_dir_sep(c)
}

/// The next code point of `path` at `pos` that HFS+ doesn't ignore, as Git's `next_hfs_char()` reads it: `0` at the
/// end, and `0` with `pos` becoming `None` at an invalid UTF-8 sequence.
fn next_hfs_char(path: &[u8], pos: &mut Option<usize>) -> u32 {
    loop {
        let Some(at) = *pos else { return 0 };
        let Some((c, len)) = utf8_char(path, at) else {
            *pos = None;
            return 0;
        };
        *pos = Some(at + len);
        match c {
            // Case-insensitive HFS+ skips these code points as "ignorable" when comparing filenames: zero-width
            // (non-)joiners and marks, directional formatting, shaping controls and the byte order mark. See:
            // https://github.com/git/git/commit/6162a1d323d24fd8cbbb1a6145a91fb849b2568f
            // https://developer.apple.com/library/archive/technotes/tn/tn1150.html#StringComparisonAlgorithm
            // https://github.com/apple-oss-distributions/hfs/blob/main/core/UCStringCompareData.h
            0x200c..=0x200f | 0x202a..=0x202e | 0x206a..=0x206f | 0xfeff => continue,
            _ => return c,
        }
    }
}

/// The code point that starts at `pos` in `path` and its length in bytes, as Git's `pick_one_utf8_char()` reads it
/// from NUL-terminated text, or `None` if the sequence is invalid, overlong, a surrogate, U+FFFE, U+FFFF or above
/// U+10FFFF.
fn utf8_char(path: &[u8], pos: usize) -> Option<(u32, usize)> {
    let s = |i: usize| u32::from(path.get(pos + i).copied().unwrap_or(0));
    let s0 = s(0);
    if s0 < 0x80 {
        Some((s0, 1))
    } else if s0 & 0xe0 == 0xc0 {
        if s(1) & 0xc0 != 0x80 || s0 & 0xfe == 0xc0 {
            return None;
        }
        Some((((s0 & 0x1f) << 6) | (s(1) & 0x3f), 2))
    } else if s0 & 0xf0 == 0xe0 {
        if s(1) & 0xc0 != 0x80
            || s(2) & 0xc0 != 0x80
            || (s0 == 0xe0 && s(1) & 0xe0 == 0x80)
            || (s0 == 0xed && s(1) & 0xe0 == 0xa0)
            || (s0 == 0xef && s(1) == 0xbf && s(2) & 0xfe == 0xbe)
        {
            return None;
        }
        Some((((s0 & 0x0f) << 12) | ((s(1) & 0x3f) << 6) | (s(2) & 0x3f), 3))
    } else if s0 & 0xf8 == 0xf0 {
        if s(1) & 0xc0 != 0x80
            || s(2) & 0xc0 != 0x80
            || s(3) & 0xc0 != 0x80
            || (s0 == 0xf0 && s(1) & 0xf0 == 0x80)
            || (s0 == 0xf4 && s(1) > 0x8f)
            || s0 > 0xf4
        {
            return None;
        }
        Some((
            ((s0 & 0x07) << 18) | ((s(1) & 0x3f) << 12) | ((s(2) & 0x3f) << 6) | (s(3) & 0x3f),
            4,
        ))
    } else {
        None
    }
}

/// Whether `c` separates directories on this platform, as Git's `is_dir_sep()` decides.
fn is_dir_sep(c: u32) -> bool {
    c == u32::from(b'/') || (cfg!(windows) && c == u32::from(b'\\'))
}

/// Return `input` and what follows each of its backslashes, as a backslash starts a new name on NTFS, as Git's
/// `verify_path()` looks at them on all platforms.
fn ntfs_names(input: &BStr) -> impl Iterator<Item = &BStr> {
    std::iter::successors(Some(input), |&name| name.find_byte(b'\\').map(|pos| &name[pos + 1..]))
}

/// As Git's `is_ntfs_dotgit()` does, this lets `.git` end at a backslash as well, as `.git` is a directory that a path can
/// lead into, unlike `.gitmodules`.
fn is_dot_git_ntfs(input: &BStr) -> bool {
    let input = input.find_byte(b'\\').map_or(input, |end| &input[..end]);
    if input.get(..4).is_some_and(|input| input.eq_ignore_ascii_case(b".git")) {
        return is_done_ntfs(input.get(4..));
    }
    if input.get(..5).is_some_and(|input| input.eq_ignore_ascii_case(b"git~1")) {
        return is_done_ntfs(input.get(5..));
    }
    false
}

/// The `search_case_insensitive` name is the actual name to look for (in a case-insensitive way).
/// Opposed to that there is the special `ntfs_shortname_prefix` which is derived from `search_case_insensitive`
/// but looks more like a hash, one that NTFS uses to disambiguate things, for when there is a lot of files
/// with the same prefix.
fn is_dot_ntfs(input: &BStr, search_case_insensitive: &str, ntfs_shortname_prefix: &str) -> bool {
    if input.first() == Some(&b'.') {
        let end_pos = 1 + search_case_insensitive.len();
        if input
            .get(1..end_pos)
            .is_some_and(|input| input.eq_ignore_ascii_case(search_case_insensitive.as_bytes()))
        {
            is_done_ntfs(input.get(end_pos..))
        } else {
            false
        }
    } else {
        let search_case_insensitive: &[u8] = search_case_insensitive.as_bytes();
        if search_case_insensitive
            .get(..6)
            .zip(input.get(..6))
            .is_some_and(|(ntfs_prefix, first_6_of_input)| {
                first_6_of_input.eq_ignore_ascii_case(ntfs_prefix)
                    && input.get(6) == Some(&b'~')
                    // It's notable that only `~1` to `~4` are possible before the disambiguation algorithm
                    // switches to using the `ntfs_shortname_prefix`, which is checked hereafter.
                    && input.get(7).is_some_and(|num| (b'1'..=b'4').contains(num))
            })
        {
            return is_done_ntfs(input.get(8..));
        }

        let ntfs_shortname_prefix: &[u8] = ntfs_shortname_prefix.as_bytes();
        let mut saw_tilde = false;
        let mut pos = 0;
        while pos < 8 {
            let Some(b) = input.get(pos).copied() else {
                return false;
            };
            if saw_tilde {
                if !b.is_ascii_digit() {
                    return false;
                }
            } else if b == b'~' {
                saw_tilde = true;
                pos += 1;
                let Some(b) = input.get(pos).copied() else {
                    return false;
                };
                if !(b'1'..=b'9').contains(&b) {
                    return false;
                }
            } else if pos >= 6
                || b & 0x80 == 0x80
                || ntfs_shortname_prefix
                    .get(pos)
                    .is_none_or(|ob| !b.eq_ignore_ascii_case(ob))
            {
                return false;
            }
            pos += 1;
        }
        is_done_ntfs(input.get(pos..))
    }
}

/// Check if trailing filename bytes leave a match to special files like `.git` unchanged in NTFS.
fn is_done_ntfs(input: Option<&[u8]>) -> bool {
    // Skip spaces and dots. Then return true if we are at the end or a colon.
    let Some(input) = input else { return true };
    for b in input.bytes() {
        if b == b':' {
            return true;
        }
        if b != b' ' && b != b'.' {
            return false;
        }
    }
    true
}

/// Check if trailing filename bytes leave a match to Windows reserved device names unchanged.
fn is_done_windows(input: Option<&[u8]>) -> bool {
    // Skip spaces. Then return true if we are at the end or a dot or colon.
    let Some(input) = input else { return true };
    let skip = input.bytes().take_while(|b| *b == b' ').count();
    let Some(next) = input.get(skip) else { return true };
    *next == b'.' || *next == b':'
}
