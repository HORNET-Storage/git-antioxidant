//! Patch IDs, as `diff_get_patch_id()` computes them for `git rev-list --cherry-pick` and for the commits that a rebase
//! leaves out as already upstream.
//!
//! A patch ID is the sum of a hash for each changed file of a diff without renames, so that the order of the files
//! doesn't matter, as `git patch-id --stable` computes it. The hash of a file covers its header and the lines of its diff
//! with three lines of context and without hunk headers, all without whitespace, so that a change hashes the same
//! wherever it applies. A binary file is hashed with the ids of its blobs instead of its lines.
//!
//! `git patch-id --stable` reads the text of a diff, which has no `---` and `+++` lines for changes of a mode alone or
//! for empty new files, while `diff_get_patch_id()`, which this module follows, hashes these lines for every file that
//! isn't binary. After a binary file, `git patch-id` also adds the hash of nothing at the end of the diff and skips the
//! header of the next file. The patch IDs of a rebase and of `git format-patch --base` are those of
//! `diff_get_patch_id()`.

use gix_error::{Result, ResultExt, bail, message};
use gix_hash::{ObjectId, oid};
use gix_imara_diff::{Algorithm, Diff, InternedInput};
use gix_object::tree::{EntryKind, EntryMode};

use crate::commits::{ByRef, Commits};

/// The number of bytes that tell whether a blob is binary, as `buffer_is_binary()` looks for a NUL in them.
const FIRST_FEW_BYTES: usize = 8000;

/// The lines of context around each change.
const CONTEXT: u32 = 3;

/// Return the patch ID of `commit` against its first parent, or against the empty tree for a root commit, as
/// `commit_patch_id()` computes it, or `None` for a merge commit, which has none.
pub fn of_commit(commits: &mut Commits<'_>, commit: &oid) -> Result<Option<ObjectId>> {
    let (tree, parents) = {
        let commit = commits.get(commit)?;
        (commit.tree, commit.parents.clone())
    };
    let parent_tree = match parents.as_slice() {
        [] => None,
        [parent] => Some(commits.get(parent)?.tree),
        _ => return Ok(None),
    };
    of_trees(commits.objects(), commits.object_hash(), parent_tree.as_deref(), &tree).map(Some)
}

/// Return the patch ID of the changes from `old_tree`, or from the empty tree if it is `None`, to `new_tree` in
/// `objects`, as `diff_get_patch_id()` computes it for a recursive diff without renames. It is the null id if nothing
/// changed.
pub fn of_trees(
    objects: &dyn gix_object::Find,
    object_hash: gix_hash::Kind,
    old_tree: Option<&oid>,
    new_tree: &oid,
) -> Result<ObjectId> {
    let (mut old_buf, mut new_buf) = (Vec::new(), Vec::new());
    let old = match old_tree {
        Some(id) => tree_data(objects, object_hash, id, &mut old_buf)?,
        None => &[],
    };
    let new = tree_data(objects, object_hash, new_tree, &mut new_buf)?;
    let mut recorder = gix_diff::tree::Recorder::default();
    gix_diff::tree(
        gix_object::TreeRefIter::from_bytes(old, object_hash),
        gix_object::TreeRefIter::from_bytes(new, object_hash),
        gix_diff::tree::State::default(),
        ByRef(objects),
        &mut recorder,
    )
    .or_raise(|| message!("unable to diff the trees for the patch id of {new_tree}"))?;

    let mut sum = vec![0u8; object_hash.len_in_bytes()];
    for change in recorder.records {
        use gix_diff::tree::recorder::Change;
        let (path, old, new) = match change {
            Change::Addition {
                entry_mode, oid, path, ..
            } => (path, None, Some((entry_mode, oid))),
            Change::Deletion {
                entry_mode, oid, path, ..
            } => (path, Some((entry_mode, oid)), None),
            Change::Modification {
                previous_entry_mode,
                previous_oid,
                entry_mode,
                oid,
                path,
            } => (path, Some((previous_entry_mode, previous_oid)), Some((entry_mode, oid))),
        };
        if old.is_some_and(|(mode, _)| mode.is_tree()) || new.is_some_and(|(mode, _)| mode.is_tree()) {
            continue;
        }
        let canonical = |side: Side| side.map(|(mode, id)| (canonical_mode(mode), id));
        if canonical(old) == canonical(new) {
            continue;
        }
        let file = file_id(objects, object_hash, &path, old, new)?;
        let mut carry = 0u16;
        for (total, byte) in sum.iter_mut().zip(file.as_bytes()) {
            carry += u16::from(*total) + u16::from(*byte);
            *total = carry as u8;
            carry >>= 8;
        }
    }
    Ok(ObjectId::from_bytes_or_panic(&sum))
}

/// Return the data of the tree `id`, or nothing for the empty tree, which may not be stored.
fn tree_data<'a>(
    objects: &dyn gix_object::Find,
    object_hash: gix_hash::Kind,
    id: &oid,
    buf: &'a mut Vec<u8>,
) -> Result<&'a [u8]> {
    if id == object_hash.empty_tree() {
        return Ok(&[]);
    }
    let Some(data) = objects.try_find(id, buf)? else {
        bail!(message!("could not read tree {id}").not_found());
    };
    if data.kind != gix_object::Kind::Tree {
        bail!(message!("object {id} is a {} and not a tree", data.kind).validation());
    }
    Ok(data.data)
}

/// A side of a changed file, with its mode and the id of its blob or commit.
type Side = Option<(EntryMode, ObjectId)>;

/// Return the hash of the file at `path` that changed from `old` to `new`, where `None` stands for a file that didn't
/// exist, as each round of the loop of `diff_get_patch_id()` computes it.
fn file_id(
    objects: &dyn gix_object::Find,
    object_hash: gix_hash::Kind,
    path: &[u8],
    old: Side,
    new: Side,
) -> Result<ObjectId> {
    let path: Vec<u8> = path.iter().copied().filter(|byte| !is_space(*byte)).collect();
    let mut hasher = gix_hash::hasher(object_hash);
    hasher.update(b"diff--git");
    hasher.update(b"a/");
    hasher.update(&path);
    hasher.update(b"b/");
    hasher.update(&path);
    match (old, new) {
        (None, Some((mode, _))) => {
            hasher.update(b"newfilemode");
            hasher.update(octal_mode(mode).as_bytes());
        }
        (Some((mode, _)), None) => {
            hasher.update(b"deletedfilemode");
            hasher.update(octal_mode(mode).as_bytes());
        }
        (Some((old_mode, _)), Some((new_mode, _))) if canonical_mode(old_mode) != canonical_mode(new_mode) => {
            hasher.update(b"oldmode");
            hasher.update(octal_mode(old_mode).as_bytes());
            hasher.update(b"newmode");
            hasher.update(octal_mode(new_mode).as_bytes());
        }
        _ => {}
    }
    let mut buf = Vec::new();
    let old_data = content(objects, old, &mut buf)?;
    let mut buf = Vec::new();
    let new_data = content(objects, new, &mut buf)?;
    if is_binary(&old_data) || is_binary(&new_data) {
        for side in [old, new] {
            let id = side.map_or_else(|| object_hash.null(), |(_, id)| id);
            hasher.update(id.to_hex().to_string().as_bytes());
        }
    } else {
        match (old, new) {
            (None, _) => {
                hasher.update(b"---/dev/null");
                hasher.update(b"+++b/");
                hasher.update(&path);
            }
            (_, None) => {
                hasher.update(b"---a/");
                hasher.update(&path);
                hasher.update(b"+++/dev/null");
            }
            _ => {
                hasher.update(b"---a/");
                hasher.update(&path);
                hasher.update(b"+++b/");
                hasher.update(&path);
            }
        }
        diff_lines(&old_data, &new_data, &mut |prefix, line| {
            // The prefix of a context line is a space, which goes with the other whitespace.
            let line: Vec<u8> = prefix
                .iter()
                .chain(line)
                .copied()
                .filter(|byte| !is_space(*byte))
                .collect();
            hasher.update(&line);
        });
    }
    hasher.try_finalize()
}

/// Return the content of `side` as the diff reads it: a blob as it is, the commit of a submodule as
/// `Subproject commit <id>` on a line, and nothing for a file that doesn't exist.
fn content(objects: &dyn gix_object::Find, side: Side, buf: &mut Vec<u8>) -> Result<Vec<u8>> {
    let Some((mode, id)) = side else {
        return Ok(Vec::new());
    };
    if mode.is_commit() {
        return Ok(format!("Subproject commit {id}\n").into_bytes());
    }
    let Some(data) = objects.try_find(&id, buf)? else {
        bail!(message!("unable to read files to diff: blob {id} is missing").not_found());
    };
    Ok(data.data.to_vec())
}

/// Return `true` if `data` is binary, as `buffer_is_binary()` tells by a NUL in its first bytes.
fn is_binary(data: &[u8]) -> bool {
    data[..data.len().min(FIRST_FEW_BYTES)].contains(&0)
}

/// Return the mode of a file as the diff writes it, which keeps only whether a blob is executable.
fn canonical_mode(mode: EntryMode) -> u32 {
    match mode.kind() {
        EntryKind::Blob => 0o100644,
        EntryKind::BlobExecutable => 0o100755,
        EntryKind::Link => 0o120000,
        EntryKind::Commit => 0o160000,
        EntryKind::Tree => 0o040000,
    }
}

/// Return `mode` in six octal digits, as `patch_id_add_mode()` writes it.
fn octal_mode(mode: EntryMode) -> String {
    format!("{:06o}", canonical_mode(mode))
}

/// Return `true` for the bytes that `isspace()` takes for whitespace in Git, which `remove_space()` removes.
fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

/// Call `emit` with each line of the diff from `old` to `new` with its prefix, `-` for a removed line, `+` for an added
/// one and ` ` for context, as `xdl_emit_diff()` emits them with three lines of context and without hunk headers for a
/// Myers diff without the indent heuristic. Context lines are those of `new`.
fn diff_lines(old: &[u8], new: &[u8], emit: &mut dyn FnMut(&[u8], &[u8])) {
    let input = InternedInput::new(old, new);
    let mut diff = Diff::compute(Algorithm::Myers, &input);
    diff.postprocess_no_heuristic(&input);
    let changes: Vec<_> = diff.hunks().collect();
    let before: Vec<&[u8]> = input.before.iter().map(|token| input.interner[*token]).collect();
    let after: Vec<&[u8]> = input.after.iter().map(|token| input.interner[*token]).collect();
    let (old_len, new_len) = (before.len() as u32, after.len() as u32);
    let mut start = 0;
    while start < changes.len() {
        // A hunk takes the changes that are at most two contexts apart, as `xdl_get_hunk()` groups them.
        let mut end = start;
        while end + 1 < changes.len() && changes[end + 1].before.start - changes[end].before.end <= 2 * CONTEXT {
            end += 1;
        }
        let first = &changes[start];
        for index in first.after.start.saturating_sub(CONTEXT)..first.after.start {
            emit(b" ", after[index as usize]);
        }
        for (position, change) in changes[start..=end].iter().enumerate() {
            if position > 0 {
                let previous = &changes[start + position - 1];
                for index in previous.after.end..change.after.start {
                    emit(b" ", after[index as usize]);
                }
            }
            for index in change.before.clone() {
                emit(b"-", before[index as usize]);
            }
            for index in change.after.clone() {
                emit(b"+", after[index as usize]);
            }
        }
        let last = &changes[end];
        let context_after = CONTEXT.min(old_len - last.before.end).min(new_len - last.after.end);
        for index in last.after.end..last.after.end + context_after {
            emit(b" ", after[index as usize]);
        }
        start = end + 1;
    }
}
