//! Tree entry names that a file system takes for `.git`, `.gitmodules`, `.gitattributes`, `.gitignore` or `.mailmap`,
//! as Git's `is_hfs_dot*()` and `is_ntfs_dot*()` recognize them.
//!
//! Names are read as Git reads them, as text that ends at a NUL byte, so reading past the end of a name yields NUL.

use bstr::ByteSlice;
use gix_validate::path::component_is_hfs_dot;

/// Whether HFS+ takes `name` for `.git`.
pub(super) fn is_hfs_dotgit(name: &[u8]) -> bool {
    component_is_hfs_dot(name.as_bstr(), "git")
}

/// Whether HFS+ takes `name` for `.gitmodules`.
pub(super) fn is_hfs_dotgitmodules(name: &[u8]) -> bool {
    component_is_hfs_dot(name.as_bstr(), "gitmodules")
}

/// Whether HFS+ takes `name` for `.gitignore`.
pub(super) fn is_hfs_dotgitignore(name: &[u8]) -> bool {
    component_is_hfs_dot(name.as_bstr(), "gitignore")
}

/// Whether HFS+ takes `name` for `.gitattributes`.
pub(super) fn is_hfs_dotgitattributes(name: &[u8]) -> bool {
    component_is_hfs_dot(name.as_bstr(), "gitattributes")
}

/// Whether HFS+ takes `name` for `.mailmap`.
pub(super) fn is_hfs_dotmailmap(name: &[u8]) -> bool {
    component_is_hfs_dot(name.as_bstr(), "mailmap")
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
