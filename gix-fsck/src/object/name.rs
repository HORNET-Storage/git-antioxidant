//! Tree entry names that a file system takes for `.git`, `.gitmodules`, `.gitattributes`, `.gitignore` or `.mailmap`,
//! as Git's `is_hfs_dot*()` and `is_ntfs_dot*()` recognize them.
//!
//! Names are read as Git reads them, as text that ends at a NUL byte, so reading past the end of a name yields NUL.

/// Whether HFS+ takes `name` for `.git`.
pub(super) fn is_hfs_dotgit(name: &[u8]) -> bool {
    is_hfs_dot(name, b"git")
}

/// Whether HFS+ takes `name` for `.gitmodules`.
pub(super) fn is_hfs_dotgitmodules(name: &[u8]) -> bool {
    is_hfs_dot(name, b"gitmodules")
}

/// Whether HFS+ takes `name` for `.gitignore`.
pub(super) fn is_hfs_dotgitignore(name: &[u8]) -> bool {
    is_hfs_dot(name, b"gitignore")
}

/// Whether HFS+ takes `name` for `.gitattributes`.
pub(super) fn is_hfs_dotgitattributes(name: &[u8]) -> bool {
    is_hfs_dot(name, b"gitattributes")
}

/// Whether HFS+ takes `name` for `.mailmap`.
pub(super) fn is_hfs_dotmailmap(name: &[u8]) -> bool {
    is_hfs_dot(name, b"mailmap")
}

/// Whether NTFS takes `name`, or the part of it before a directory separator or `:`, for `.git`.
pub(super) fn is_ntfs_dotgit(name: &[u8]) -> bool {
    let at = |i: usize| name.get(i).copied().unwrap_or(0);
    let mut i = match at(0) {
        b'.' if at(1).eq_ignore_ascii_case(&b'g')
            && at(2).eq_ignore_ascii_case(&b'i')
            && at(3).eq_ignore_ascii_case(&b't') =>
        {
            4
        }
        b'g' | b'G'
            if at(1).eq_ignore_ascii_case(&b'i')
                && at(2).eq_ignore_ascii_case(&b't')
                && at(3) == b'~'
                && at(4) == b'1' =>
        {
            5
        }
        _ => return false,
    };
    loop {
        match at(i) {
            0 | b'/' | b'\\' | b':' => return true,
            b'.' | b' ' => i += 1,
            _ => return false,
        }
    }
}

/// Whether NTFS takes `name` for `.gitmodules`.
pub(super) fn is_ntfs_dotgitmodules(name: &[u8]) -> bool {
    is_ntfs_dot(name, b"gitmodules", b"gi7eba")
}

/// Whether NTFS takes `name` for `.gitignore`.
pub(super) fn is_ntfs_dotgitignore(name: &[u8]) -> bool {
    is_ntfs_dot(name, b"gitignore", b"gi250a")
}

/// Whether NTFS takes `name` for `.gitattributes`.
pub(super) fn is_ntfs_dotgitattributes(name: &[u8]) -> bool {
    is_ntfs_dot(name, b"gitattributes", b"gi7d29")
}

/// Whether NTFS takes `name` for `.mailmap`.
pub(super) fn is_ntfs_dotmailmap(name: &[u8]) -> bool {
    is_ntfs_dot(name, b"mailmap", b"maba30")
}

/// Whether HFS+ takes `path` for `.` followed by `needle`, ignoring case and the code points it ignores, as Git's
/// `is_hfs_dot_generic()` does.
///
/// Like Git, a name whose remainder isn't valid UTF-8 matches, as Git reads the invalid sequence as the end of
/// the name.
fn is_hfs_dot(path: &[u8], needle: &[u8]) -> bool {
    let mut pos = Some(0);
    let mut next = || next_hfs_char(path, &mut pos);
    if next() != u32::from(b'.') {
        return false;
    }
    for &expected in needle {
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
            // Zero-width (non-)joiners and marks, directional formatting, shaping controls and the BOM.
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

/// Whether NTFS takes `name` for `.` followed by `dotgit_name`, as Git's `is_ntfs_dot_generic()` decides: the name
/// itself, its regular short name made of its first six characters, `~` and a digit from 1 to 4, or a short name
/// with the hashed `short_prefix`, each ignoring case and followed by nothing but spaces and periods up to the end
/// or an alternate data stream.
fn is_ntfs_dot(name: &[u8], dotgit_name: &[u8], short_prefix: &[u8; 6]) -> bool {
    let at = |i: usize| name.get(i).copied().unwrap_or(0);
    let only_spaces_and_periods = |mut i: usize| loop {
        match at(i) {
            0 | b':' => return true,
            b' ' | b'.' => i += 1,
            _ => return false,
        }
    };
    let starts_with_ignore_case = |start: usize, prefix: &[u8]| {
        name.get(start..start + prefix.len())
            .is_some_and(|part| part.eq_ignore_ascii_case(prefix))
    };
    if at(0) == b'.' && starts_with_ignore_case(1, dotgit_name) {
        return only_spaces_and_periods(dotgit_name.len() + 1);
    }
    if starts_with_ignore_case(0, &dotgit_name[..6]) && at(6) == b'~' && (b'1'..=b'4').contains(&at(7)) {
        return only_spaces_and_periods(8);
    }
    let mut saw_tilde = false;
    let mut i = 0;
    while i < 8 {
        let c = at(i);
        if c == 0 {
            return false;
        } else if saw_tilde {
            if !c.is_ascii_digit() {
                return false;
            }
        } else if c == b'~' {
            i += 1;
            if !(b'1'..=b'9').contains(&at(i)) {
                return false;
            }
            saw_tilde = true;
        } else if i >= 6 || c & 0x80 != 0 || c.to_ascii_lowercase() != short_prefix[i] {
            return false;
        }
        i += 1;
    }
    only_spaces_and_periods(i)
}
