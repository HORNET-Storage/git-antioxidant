//! The checks of trees, as Git's `fsck_tree()` performs them.

use std::cmp::Ordering;

use bstr::ByteSlice;
use gix_hash::{ObjectId, oid};
use gix_hashtable::HashSet;
use gix_object::Kind;

use super::{MessageId, Reports, name};

const MODE_TYPE_MASK: u16 = 0o170000;
const MODE_DIRECTORY: u16 = 0o040000;
const MODE_SYMLINK: u16 = 0o120000;

/// Check the tree `id` with the content `data` as Git's `fsck_tree()` does and return `true` if it fails.
///
/// The IDs of entries that a file system takes for `.gitmodules` or `.gitattributes`, and that aren't symbolic
/// links, are added to `gitmodules` and `gitattributes` to check them as blobs, also if the tree is skipped.
pub(super) fn check(
    reports: &mut Reports<'_>,
    id: &oid,
    data: &[u8],
    object_hash: gix_hash::Kind,
    gitmodules: &mut HashSet<ObjectId>,
    gitattributes: &mut HashSet<ObjectId>,
) -> bool {
    let hash_len = object_hash.len_in_bytes();
    let strict = reports.options.is_strict();
    let max_name_len = reports.options.max_path_component_len();
    let report =
        |reports: &mut Reports<'_>, message_id, message: &str| reports.report(id, Kind::Tree, message_id, message);
    let mut failed = false;
    let mut entry = match data {
        [] => None,
        _ => match Entry::decode(data, hash_len) {
            Some(entry) => Some(entry),
            None => return report(reports, MessageId::BadTree, "cannot be parsed as a tree"),
        },
    };
    let mut found = Found::default();
    let mut previous: Option<(u16, &[u8])> = None;
    let mut candidates = Vec::new();
    let mut start = 0;
    while let Some(current) = entry {
        let symlink = current.mode & MODE_TYPE_MASK == MODE_SYMLINK;
        found.null_sha1 |= current.id.iter().all(|&b| b == 0);
        found.full_pathname |= current.name.contains(&b'/');
        found.dot |= current.name == b".";
        found.dotdot |= current.name == b"..";
        found.dotgit |= name::is_hfs_dotgit(current.name) || name::is_ntfs_dotgit(current.name);
        found.zero_padded_mode |= data[start] == b'0';
        found.large_name |= i64::try_from(current.name.len()).unwrap_or(i64::MAX) > max_name_len;

        let entry_id = || ObjectId::from_bytes_or_panic(current.id);
        if name::is_hfs_dotgitmodules(current.name) || name::is_ntfs_dotgitmodules(current.name) {
            if symlink {
                failed |= report(reports, MessageId::GitmodulesSymlink, ".gitmodules is a symbolic link");
            } else {
                gitmodules.insert(entry_id());
            }
        }
        if name::is_hfs_dotgitattributes(current.name) || name::is_ntfs_dotgitattributes(current.name) {
            if symlink {
                failed |= report(reports, MessageId::GitattributesSymlink, ".gitattributes is a symlink");
            } else {
                gitattributes.insert(entry_id());
            }
        }
        if symlink {
            if name::is_hfs_dotgitignore(current.name) || name::is_ntfs_dotgitignore(current.name) {
                failed |= report(reports, MessageId::GitignoreSymlink, ".gitignore is a symlink");
            }
            if name::is_hfs_dotmailmap(current.name) || name::is_ntfs_dotmailmap(current.name) {
                failed |= report(reports, MessageId::MailmapSymlink, ".mailmap is a symlink");
            }
        }
        // Git treats backslashes as directory separators, as NTFS does, and checks the name after each of them.
        let mut rest = current.name;
        while let Some(backslash) = rest.find_byte(b'\\') {
            rest = &rest[backslash + 1..];
            found.dotgit |= name::is_ntfs_dotgit(rest);
            if name::is_ntfs_dotgitmodules(rest) {
                if symlink {
                    failed |= report(reports, MessageId::GitmodulesSymlink, ".gitmodules is a symbolic link");
                } else {
                    gitmodules.insert(entry_id());
                }
            }
        }

        // Like Git, read the next entry before checking the mode and order of this one, which a malformed next
        // entry leaves unchecked.
        start += current.len;
        entry = match &data[start..] {
            [] => None,
            rest => match Entry::decode(rest, hash_len) {
                Some(next) => Some(next),
                None => {
                    failed |= report(reports, MessageId::BadTree, "cannot be parsed as a tree");
                    break;
                }
            },
        };

        match current.mode {
            0o100755 | 0o100644 | MODE_SYMLINK | MODE_DIRECTORY | 0o160000 => {}
            // Early versions of Git wrote this mode.
            0o100664 if !strict => {}
            _ => found.bad_mode = true,
        }
        if let Some((previous_mode, previous_name)) = previous {
            match verify_order(
                previous_mode,
                previous_name,
                current.mode,
                current.name,
                &mut candidates,
            ) {
                Order::Sorted => {}
                Order::Unsorted => found.unsorted = true,
                Order::Duplicate => found.duplicate = true,
            }
        }
        previous = Some((current.mode, current.name));
    }

    for (present, message_id, message) in [
        (
            found.null_sha1,
            MessageId::NullSha1,
            "contains entries pointing to null sha1",
        ),
        (found.full_pathname, MessageId::FullPathname, "contains full pathnames"),
        (found.dot, MessageId::HasDot, "contains '.'"),
        (found.dotdot, MessageId::HasDotdot, "contains '..'"),
        (found.dotgit, MessageId::HasDotgit, "contains '.git'"),
        (
            found.zero_padded_mode,
            MessageId::ZeroPaddedFilemode,
            "contains zero-padded file modes",
        ),
        (found.bad_mode, MessageId::BadFilemode, "contains bad file modes"),
        (
            found.duplicate,
            MessageId::DuplicateEntries,
            "contains duplicate file entries",
        ),
        (found.unsorted, MessageId::TreeNotSorted, "not properly sorted"),
        (
            found.large_name,
            MessageId::LargePathname,
            "contains excessively large pathname",
        ),
    ] {
        if present {
            failed |= report(reports, message_id, message);
        }
    }
    failed
}

/// What the entries of a tree have shown so far. Git reports each of these once per tree, after reading all entries.
#[derive(Default)]
struct Found {
    null_sha1: bool,
    full_pathname: bool,
    dot: bool,
    dotdot: bool,
    dotgit: bool,
    zero_padded_mode: bool,
    bad_mode: bool,
    duplicate: bool,
    unsorted: bool,
    large_name: bool,
}

/// A tree entry as Git's `decode_tree_entry()` reads it.
struct Entry<'a> {
    /// The mode as Git keeps it: its octal digits read into 32 bits, of which the lower 16 are kept.
    mode: u16,
    name: &'a [u8],
    id: &'a [u8],
    /// The length of the entry in bytes.
    len: usize,
}

impl<'a> Entry<'a> {
    /// Read the entry at the start of `rest`, which holds the remainder of a tree with object IDs of `hash_len`
    /// bytes, or return `None` if Git can't read it.
    ///
    /// Like Git, this requires the byte before the last object ID's worth of `rest` to be NUL, which bounds the
    /// mode and name of this entry, and then reads the name up to its first NUL.
    fn decode(rest: &'a [u8], hash_len: usize) -> Option<Self> {
        if rest.len() < hash_len + 3 || rest[rest.len() - hash_len - 1] != 0 || rest[0] == b' ' {
            return None;
        }
        let space = rest.find_byte(b' ')?;
        let digits = &rest[..space];
        if !digits.iter().all(|digit| (b'0'..=b'7').contains(digit)) {
            return None;
        }
        let mode = digits
            .iter()
            .fold(0u32, |mode, digit| (mode << 3) | u32::from(digit - b'0'));
        let name_start = space + 1;
        let name_len = rest[name_start..].find_byte(0)?;
        if name_len == 0 {
            return None;
        }
        let id_start = name_start + name_len + 1;
        Some(Entry {
            mode: mode as u16,
            name: &rest[name_start..id_start - 1],
            id: rest.get(id_start..id_start + hash_len)?,
            len: id_start + hash_len,
        })
    }
}

enum Order {
    Sorted,
    Unsorted,
    Duplicate,
}

/// Compare the entry `name1` with mode `mode1` with the entry `name2` with mode `mode2` that follows it, in Git's
/// order of tree entries, in which the name of a tree compares as if it ended in `/`, as Git's `verify_ordered()`
/// does.
///
/// As a file and a tree of the same name need not be adjacent, as in `foo`, `foo.bar`, `foo/`, `candidates` holds
/// the names of earlier files that a later tree may duplicate.
fn verify_order<'a>(mode1: u16, name1: &'a [u8], mode2: u16, name2: &'a [u8], candidates: &mut Vec<&'a [u8]>) -> Order {
    let len = name1.len().min(name2.len());
    match name1[..len].cmp(&name2[..len]) {
        Ordering::Less => return Order::Sorted,
        Ordering::Greater => return Order::Unsorted,
        Ordering::Equal => {}
    }
    let is_less_than_slash = |c: u8| c != 0 && c < b'/';
    let is_directory = |mode: u16| mode & MODE_TYPE_MASK == MODE_DIRECTORY;
    let mut c1 = name1.get(len).copied().unwrap_or(0);
    let mut c2 = name2.get(len).copied().unwrap_or(0);
    if c1 == 0 && c2 == 0 {
        return Order::Duplicate;
    }
    if c1 == 0 && is_directory(mode1) {
        c1 = b'/';
    }
    if c2 == 0 && is_directory(mode2) {
        c2 = b'/';
    }
    if c1 == 0 && is_less_than_slash(c2) {
        candidates.push(name1);
    } else if c2 == b'/' && is_less_than_slash(c1) {
        while let Some(candidate) = candidates.pop() {
            let Some(rest) = name2.strip_prefix(candidate) else {
                continue;
            };
            match rest.first() {
                None => return Order::Duplicate,
                Some(&c) if is_less_than_slash(c) => {
                    candidates.push(candidate);
                    break;
                }
                Some(_) => {}
            }
        }
    }
    if c1 < c2 { Order::Sorted } else { Order::Unsorted }
}
