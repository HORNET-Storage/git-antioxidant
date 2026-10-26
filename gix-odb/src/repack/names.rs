use std::{
    cmp::{Ordering, Reverse},
    collections::BinaryHeap,
    sync::atomic::{AtomicBool, Ordering::Relaxed},
};

use gix_error::{Result, bail, cancelled, message};
use gix_hash::{ObjectId, oid};
use gix_object::Find;
use gix_pack::data::delta::search::name_hash;

/// How many objects are read between two checks of the interrupt flag.
const OBJECTS_PER_INTERRUPT_CHECK: usize = 4096;

/// Give the objects of the new pack the name hashes that `git pack-objects --stdin-packs` gives them, from the paths at
/// which the walk of its `read_stdin_packs()` finds them first, as its `show_object_pack_hint()` does.
///
/// * `objects` reads the commits and trees of the walk. Commits and trees that can't be read are skipped, as git's walk
///   ignores missing links.
/// * `commits` are the commits that the walk starts at: those of the combined packs, the pack modified last first and
///   each in the order of the pack, and then the loose commits in the order in which they are listed. A commit that
///   can't be read or parsed among them is an error, as it is for git.
/// * `is_retained` tells whether a pack that remains holds a commit. Such commits are walked through, but their trees
///   aren't, as git ignores the commits of the excluded packs.
/// * `indices` maps the id of each object of the new pack to its index in `name_hashes`, which the hash of the path at
///   which the object is found first is written to. The hashes of the objects that the walk doesn't find remain as they
///   are.
///
/// Like git's revision walk, the walk takes the commit with the newest committer date first, and of commits with the
/// same date the one it was given or found first, and then the parents of each commit in their order, until a parent
/// can't be read. After all commits, it visits the trees of the commits that it doesn't ignore in the order of the
/// commits, each tree before its entries and these in their order, and every object at the first path at which it is
/// found.
pub(crate) fn assign(
    objects: &impl Find,
    object_hash: gix_hash::Kind,
    commits: &[ObjectId],
    is_retained: impl Fn(&oid) -> bool,
    indices: &gix_hashtable::HashMap<ObjectId, u32>,
    name_hashes: &mut [u32],
    should_interrupt: &AtomicBool,
) -> Result {
    let mut walk = Walk {
        objects,
        object_hash,
        buf: Vec::new(),
        reads: 0,
        should_interrupt,
    };
    let roots = walk.commits(commits, is_retained)?;
    let mut found = |id: &oid, path: &[u8]| {
        if let Some(&index) = indices.get(id) {
            name_hashes[index as usize] = name_hash(path);
        }
    };
    walk.trees(roots, &mut found)
}

/// A commit as git's revision walk knows it.
struct Commit {
    tree: ObjectId,
    parents: Vec<ObjectId>,
    /// The committer date, as git's `parse_commit_date()` reads it.
    date: u64,
}

/// A commit in the queue of the walk.
struct Queued {
    id: ObjectId,
    commit: Commit,
    /// The position of the commit in the order in which it entered the queue, which decides between commits of the
    /// same date as in git's `prio_queue`.
    sequence: u64,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Queued {}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Queued {
    /// The commit that the walk takes first is the greatest: the newest one, and of those of the same date the one that
    /// entered the queue first.
    fn cmp(&self, other: &Self) -> Ordering {
        self.commit
            .date
            .cmp(&other.commit.date)
            .then(other.sequence.cmp(&self.sequence))
    }
}

/// The kind of an entry of a tree, as git's walk tells them apart.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Tree,
    /// A submodule, which the walk ignores.
    Commit,
    /// A blob of any mode, including symbolic links.
    Blob,
}

/// The kind, the name and the id of each entry of a tree, in the order of the tree.
type TreeEntries = Vec<(EntryKind, Vec<u8>, ObjectId)>;

/// A tree that the walk visits, with its entries and the length of its path followed by a slash, unless it's a root.
struct Level {
    entries: TreeEntries,
    next: usize,
    path_len: usize,
}

struct Walk<'a, F> {
    objects: &'a F,
    object_hash: gix_hash::Kind,
    buf: Vec<u8>,
    /// The amount of objects read so far, to check the interrupt flag every so often.
    reads: usize,
    should_interrupt: &'a AtomicBool,
}

impl<F: Find> Walk<'_, F> {
    /// Walk the history of `starts` as git's `prepare_revision_walk()` and `get_revision()` do, and return the trees of
    /// the commits that aren't ignored, in the order of the walk.
    fn commits(&mut self, starts: &[ObjectId], is_retained: impl Fn(&oid) -> bool) -> Result<Vec<ObjectId>> {
        let mut seen = gix_hashtable::HashSet::default();
        let mut listed = Vec::new();
        for &id in starts {
            if !seen.insert(id) {
                continue;
            }
            let Some(commit) = self.commit(&id)? else {
                bail!(message!("Could not parse the commit {id} that the walk for name hashes starts at").corrupted());
            };
            listed.push((id, commit));
        }
        // A stable sort keeps the order in which they were given among commits of the same date.
        listed.sort_by_key(|(_, commit)| Reverse(commit.date));
        let mut sequence = 0;
        let mut queue: BinaryHeap<Queued> = listed
            .into_iter()
            .map(|(id, commit)| {
                sequence += 1;
                Queued { id, commit, sequence }
            })
            .collect();

        let mut roots = Vec::new();
        while let Some(Queued { id, commit, .. }) = queue.pop() {
            for parent in &commit.parents {
                if seen.contains(parent) {
                    continue;
                }
                // Like git, give up on the remaining parents of a commit at the first one that can't be read.
                let Some(parent_commit) = self.commit(parent)? else {
                    break;
                };
                seen.insert(*parent);
                sequence += 1;
                queue.push(Queued {
                    id: *parent,
                    commit: parent_commit,
                    sequence,
                });
            }
            if !is_retained(&id) {
                roots.push(commit.tree);
            }
        }
        Ok(roots)
    }

    /// Visit `roots` and the trees and blobs in them as git's `traverse_non_commits()` does, and call `found` with each
    /// tree and blob at the first path at which it is found.
    fn trees(&mut self, roots: Vec<ObjectId>, found: &mut dyn FnMut(&oid, &[u8])) -> Result {
        let mut seen = gix_hashtable::HashSet::default();
        let mut path = Vec::new();
        for root in roots {
            if seen.contains(&root) {
                continue;
            }
            // Trees that can't be read are skipped and not marked as seen, as in git.
            let Some(entries) = self.tree(&root)? else {
                continue;
            };
            seen.insert(root);
            found(&root, b"");
            let mut stack = vec![Level {
                entries,
                next: 0,
                path_len: 0,
            }];
            while let Some(level) = stack.last_mut() {
                let Some((kind, name, id)) = level.entries.get(level.next) else {
                    stack.pop();
                    continue;
                };
                level.next += 1;
                path.truncate(level.path_len);
                path.extend_from_slice(name);
                let id = *id;
                match kind {
                    EntryKind::Commit => {}
                    EntryKind::Blob => {
                        if seen.insert(id) {
                            found(&id, &path);
                        }
                    }
                    EntryKind::Tree => {
                        if seen.contains(&id) {
                            continue;
                        }
                        let Some(entries) = self.tree(&id)? else {
                            continue;
                        };
                        seen.insert(id);
                        found(&id, &path);
                        path.push(b'/');
                        stack.push(Level {
                            entries,
                            next: 0,
                            path_len: path.len(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Read the commit `id`, or return `None` if it can't be read or parsed as git's `repo_parse_commit_gently()`.
    fn commit(&mut self, id: &oid) -> Result<Option<Commit>> {
        self.count_read()?;
        Ok(match self.objects.try_find(id, &mut self.buf) {
            Ok(Some(data)) if data.kind == gix_object::Kind::Commit => parse_commit(data.data, self.object_hash),
            _ => None,
        })
    }

    /// Read the entries of the tree `id`, or return `None` if it can't be read. A tree that ends with a malformed entry
    /// has the entries before it.
    fn tree(&mut self, id: &oid) -> Result<Option<TreeEntries>> {
        self.count_read()?;
        let data = match self.objects.try_find(id, &mut self.buf) {
            Ok(Some(data)) if data.kind == gix_object::Kind::Tree => data,
            _ => return Ok(None),
        };
        let entries = gix_object::TreeRefIter::from_bytes(data.data, self.object_hash)
            .map_while(std::result::Result::ok)
            .map(|entry| {
                let kind = if entry.mode.is_tree() {
                    EntryKind::Tree
                } else if entry.mode.is_commit() {
                    EntryKind::Commit
                } else {
                    EntryKind::Blob
                };
                (kind, entry.filename.to_vec(), entry.oid.to_owned())
            })
            .collect();
        Ok(Some(entries))
    }

    fn count_read(&mut self) -> Result {
        self.reads += 1;
        if self.reads.is_multiple_of(OBJECTS_PER_INTERRUPT_CHECK) && self.should_interrupt.load(Relaxed) {
            bail!(cancelled(
                "Interrupted while walking the history for the paths of objects"
            ));
        }
        Ok(())
    }
}

/// Parse `data` as git's `parse_commit_buffer()` does: a tree line, then parent lines, and the committer date after
/// them, or return `None` if the tree line or a parent line is malformed or ends the commit.
fn parse_commit(data: &[u8], object_hash: gix_hash::Kind) -> Option<Commit> {
    let hex_len = object_hash.len_in_hex();
    let id_line = |data: &[u8], prefix: &[u8]| -> Option<ObjectId> {
        let hex = data.strip_prefix(prefix)?.get(..hex_len)?;
        let end = prefix.len() + hex_len;
        (data.len() > end + 1 && data[end] == b'\n')
            .then(|| ObjectId::from_hex(hex).ok())
            .flatten()
    };
    let tree = id_line(data, b"tree ")?;
    let mut rest = &data[5 + hex_len + 1..];
    let mut parents = Vec::new();
    while rest.len() > 7 + hex_len && rest.starts_with(b"parent ") {
        parents.push(id_line(rest, b"parent ")?);
        rest = &rest[7 + hex_len + 1..];
    }
    Some(Commit {
        tree,
        parents,
        date: commit_date(rest),
    })
}

/// The committer date of a commit whose author line starts `data`, as git's `parse_commit_date()` reads it: the number
/// after the last `>` of the committer line that follows the author line, or 0 if there is none.
fn commit_date(data: &[u8]) -> u64 {
    if data.len() <= 6 || !data.starts_with(b"author") {
        return 0;
    }
    let Some(committer) = data.iter().position(|&byte| byte == b'\n').map(|eol| &data[eol + 1..]) else {
        return 0;
    };
    if committer.len() <= 9 || !committer.starts_with(b"committer") {
        return 0;
    }
    let Some(eol) = committer.iter().position(|&byte| byte == b'\n') else {
        return 0;
    };
    let line = &committer[..eol];
    let Some(email_end) = line.iter().rposition(|&byte| byte == b'>') else {
        return 0;
    };
    let date = &line[email_end + 1..];
    let date = &date[date
        .iter()
        .position(|&byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        .unwrap_or(date.len())..];
    // Like `strtoumax()`, take an optional minus sign and the digits after it, and give `u64::MAX` on overflow.
    let (negative, digits) = match date.strip_prefix(b"-") {
        Some(digits) => (true, digits),
        None if date.first().is_some_and(u8::is_ascii_digit) => (false, date),
        None => return 0,
    };
    let mut value = 0u64;
    for &digit in digits.iter().take_while(|byte| byte.is_ascii_digit()) {
        match value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(digit - b'0')))
        {
            Some(next) => value = next,
            None => return u64::MAX,
        }
    }
    if negative { value.wrapping_neg() } else { value }
}

#[cfg(test)]
mod tests {
    use super::commit_date;

    #[test]
    fn commit_dates_are_read_as_git_reads_them() {
        let commit = |committer: &str| format!("author a <a@example.com> 1 +0000\ncommitter {committer}\n\nmessage\n");
        assert_eq!(
            commit_date(commit("c <c@example.com> 946771200 +0000").as_bytes()),
            946_771_200
        );
        assert_eq!(
            commit_date(commit("c <c@example.com>946771200+0100").as_bytes()),
            946_771_200,
            "digits right after the email and before the time zone are read"
        );
        assert_eq!(
            commit_date(commit("c <c> d <c@example.com> 12 +0000").as_bytes()),
            12,
            "the date follows the last '>'"
        );
        assert_eq!(commit_date(commit("c <c@example.com> -1 +0000").as_bytes()), u64::MAX);
        assert_eq!(
            commit_date(commit("c <c@example.com> 99999999999999999999 +0000").as_bytes()),
            u64::MAX,
            "overflowing dates are the largest one"
        );
        assert_eq!(commit_date(commit("c <c@example.com> soon").as_bytes()), 0);
        assert_eq!(
            commit_date(b"committer c <c@example.com> 5 +0000\n"),
            0,
            "the author line comes first"
        );
        assert_eq!(
            commit_date(b"author a <a@example.com> 1 +0000\ncommitter c <c@example.com> 5 +0000"),
            0,
            "the committer line ends with a line feed"
        );
    }
}
