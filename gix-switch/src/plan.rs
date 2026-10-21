//! Compare the old tree with the new one, and decide what happens to each path that they change, as Git's two-way merge
//! decides it.

use bstr::{BStr, BString, ByteSlice};
use gix_diff::tree::recorder::Change as Record;
use gix_error::{Result, ResultExt, bail, message};
use gix_hash::{ObjectId, oid};
use gix_index::{State, entry::Mode};
use gix_object::{FindExt, tree::EntryMode};
use gix_worktree::validate::path::component;

/// The mode and object of an entry of a tree or the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Side {
    pub(crate) mode: Mode,
    pub(crate) id: ObjectId,
}

/// A path whose entry differs between the old and the new tree.
#[derive(Debug)]
pub(crate) struct Change {
    pub(crate) path: BString,
    /// The entry of the old tree at this path, if it has one.
    pub(crate) old: Option<Side>,
    /// The entry of the new tree at this path, if it has one.
    pub(crate) new: Option<Side>,
}

/// What happens to the path of a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// The entry of the index stays: it is the entry of the new tree already, or a staged change.
    Keep,
    /// The entry at this position of the index is removed, along with its file.
    Delete(usize),
    /// The entry at this position of the index becomes the entry of the new tree, whose file is checked out.
    Update(usize, Side),
    /// The entry of the new tree is added, and its file is checked out.
    Add(Side),
    /// The path has no entry and keeps having none, as only the old tree has this entry at it.
    Absent(Side),
}

/// What happens to an entry of the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    Kept,
    Deleted,
    Updated,
}

/// What happens to each path that the trees change.
pub(crate) struct Plan {
    /// The changes, sorted by path.
    pub(crate) changes: Vec<Change>,
    /// The action for each change.
    pub(crate) actions: Vec<Action>,
    /// What happens to each entry of the index.
    marks: Vec<Mark>,
}

impl Plan {
    /// Decide what happens to the paths of `changes` given the entries of `index`, and add the paths of the entries that
    /// differ from both trees, or are in the way of entries of the new tree, to `staged_changes`.
    pub(crate) fn new(index: &State, changes: Vec<Change>, staged_changes: &mut Vec<BString>) -> Self {
        let entries = index.entries();
        let mut marks = vec![Mark::Kept; entries.len()];
        let mut actions = Vec::with_capacity(changes.len());
        for change in &changes {
            let current = index
                .entry_index_by_path(change.path.as_bstr())
                .ok()
                .map(|entry_index| {
                    let entry = &entries[entry_index];
                    (
                        entry_index,
                        Side {
                            mode: entry.mode,
                            id: entry.id,
                        },
                    )
                });
            let action = match (current, change.old, change.new) {
                (Some((_, current)), _, Some(new)) if current == new => Action::Keep,
                (Some((entry_index, current)), Some(old), None) if current == old => {
                    marks[entry_index] = Mark::Deleted;
                    Action::Delete(entry_index)
                }
                (Some((entry_index, current)), Some(old), Some(new)) if current == old => {
                    marks[entry_index] = Mark::Updated;
                    Action::Update(entry_index, new)
                }
                (Some(_), _, _) | (None, Some(_), Some(_)) => {
                    staged_changes.push(change.path.clone());
                    Action::Keep
                }
                (None, None, Some(new)) => Action::Add(new),
                (None, Some(old), None) => Action::Absent(old),
                (None, None, None) => Action::Keep,
            };
            actions.push(action);
        }
        let mut plan = Plan {
            changes,
            actions,
            marks,
        };
        plan.refuse_entries_in_the_way(index, staged_changes);
        plan
    }

    /// Add the paths of the entries that stay although they are in the way of an added or changed entry to
    /// `staged_changes`: those at the paths of the directories that lead to it, and those in a directory at its path. An
    /// addition that such an entry is in the way of doesn't happen, so its path isn't checked.
    fn refuse_entries_in_the_way(&mut self, index: &State, staged_changes: &mut Vec<BString>) {
        let entries = index.entries();
        for (change, action) in self.changes.iter().zip(self.actions.iter_mut()) {
            if !matches!(action, Action::Add(_) | Action::Update(..)) {
                continue;
            }
            let path = change.path.as_bstr();
            let mut in_the_way = false;
            let mut end = 0;
            while let Some(slash) = path[end..].find_byte(b'/') {
                end += slash;
                let leading = path[..end].as_bstr();
                if let Ok(entry_index) = index.entry_index_by_path(leading)
                    && self.marks[entry_index] == Mark::Kept
                {
                    staged_changes.push(leading.to_owned());
                    in_the_way = true;
                }
                end += 1;
            }
            let mut directory = path.to_owned();
            directory.push(b'/');
            if let Some(range) = index.prefixed_entries_range(directory.as_bstr()) {
                for entry_index in range.filter(|entry_index| self.marks[*entry_index] == Mark::Kept) {
                    staged_changes.push(entries[entry_index].path(index).to_owned());
                    in_the_way = true;
                }
            }
            if in_the_way && matches!(action, Action::Add(_)) {
                *action = Action::Keep;
            }
        }
    }

    /// Return `true` if the entry at this position of the index is removed.
    pub(crate) fn is_deleted(&self, entry_index: usize) -> bool {
        self.marks[entry_index] == Mark::Deleted
    }

    /// Return `true` if the file at `path` is checked out, as its entry is added or changed.
    pub(crate) fn checks_out(&self, path: &BStr) -> bool {
        self.changes
            .binary_search_by(|change| change.path.as_bstr().cmp(path))
            .is_ok_and(|position| matches!(self.actions[position], Action::Add(_) | Action::Update(..)))
    }
}

/// Return the paths whose entries differ between `old_tree` and `new_tree`, sorted, leaving out the entries of trees.
/// Without `old_tree`, every entry of `new_tree` is a change.
///
/// Fails if a path appears more than once, if an entry has a mode that is neither that of a tree, a file, a symbolic link
/// nor a submodule, or if a path can't be checked out, as `validate` decides.
pub(crate) fn changes<O>(
    old_tree: Option<&oid>,
    new_tree: &oid,
    objects: &O,
    validate: component::Options,
) -> Result<Vec<Change>>
where
    O: gix_object::Find,
{
    let objects = WithEmptyTree(objects);
    let (mut old_buf, mut new_buf) = (Vec::new(), Vec::new());
    let old = match old_tree {
        Some(old_tree) => objects
            .find_tree_iter(old_tree, &mut old_buf)
            .or_raise(|| message!("Could not read the old tree {old_tree}"))?,
        None => gix_object::TreeRefIter::from_bytes(&[], new_tree.kind()),
    };
    let new = objects
        .find_tree_iter(new_tree, &mut new_buf)
        .or_raise(|| message!("Could not read the new tree {new_tree}"))?;
    let mut recorder = gix_diff::tree::Recorder::default();
    gix_diff::tree(old, new, gix_diff::tree::State::default(), &objects, &mut recorder)
        .or_raise(|| message("Could not compare the old tree with the new one"))?;

    let mut changes = Vec::with_capacity(recorder.records.len());
    for record in recorder.records {
        let (path, old, new) = match record {
            Record::Addition {
                entry_mode, oid, path, ..
            } => {
                let new = side(entry_mode, oid, path.as_bstr())?;
                (path, None, new)
            }
            Record::Deletion {
                entry_mode, oid, path, ..
            } => {
                let old = side(entry_mode, oid, path.as_bstr())?;
                (path, old, None)
            }
            Record::Modification {
                previous_entry_mode,
                previous_oid,
                entry_mode,
                oid,
                path,
            } => {
                let old = side(previous_entry_mode, previous_oid, path.as_bstr())?;
                let new = side(entry_mode, oid, path.as_bstr())?;
                (path, old, new)
            }
        };
        // Modes that only differ in permissions other than the executable bit are the same, as Git compares them.
        if old != new {
            changes.push(Change { path, old, new });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    if let Some(pair) = changes.windows(2).find(|pair| pair[0].path == pair[1].path) {
        bail!(
            "The path '{}' appears more than once in the trees".corrupted(),
            pair[0].path
        );
    }
    for change in &changes {
        let is_symlink = change.new.is_some_and(|new| new.mode == Mode::SYMLINK);
        validate_path(change.path.as_bstr(), is_symlink, validate)?;
    }
    Ok(changes)
}

/// The entry with `mode` and `id` at `path` in a tree, or `None` if it is a tree itself.
fn side(mode: EntryMode, id: ObjectId, path: &BStr) -> Result<Option<Side>> {
    let mode = match mode.value() & 0o170000 {
        0o040000 => return Ok(None),
        0o100000 if mode.is_executable() => Mode::FILE_EXECUTABLE,
        0o100000 => Mode::FILE,
        0o120000 => Mode::SYMLINK,
        0o160000 => Mode::COMMIT,
        _ => bail!("The tree entry at '{path}' has an invalid mode".corrupted()),
    };
    Ok(Some(Side { mode, id }))
}

/// Fail if `path` can't be checked out, as `options` decide. `is_symlink` is `true` if its entry is a symbolic link.
fn validate_path(path: &BStr, is_symlink: bool, options: component::Options) -> Result<()> {
    let mut names = path.split_str("/").peekable();
    while let Some(name) = names.next() {
        let mode = (is_symlink && names.peek().is_none()).then_some(component::Mode::Symlink);
        component(name.as_bstr(), mode, options)
            .or_raise(|| message!("The path '{path}' can't be checked out").validation())?;
    }
    Ok(())
}

/// Finds objects in the object database it wraps, and the empty tree even if the object database doesn't have it, as Git
/// finds it.
struct WithEmptyTree<'a, O>(&'a O);

impl<O> gix_object::Find for WithEmptyTree<'_, O>
where
    O: gix_object::Find,
{
    fn try_find<'a>(&self, id: &oid, buffer: &'a mut Vec<u8>) -> Result<Option<gix_object::Data<'a>>> {
        if id.is_empty_tree() {
            return Ok(Some(gix_object::Data {
                kind: gix_object::Kind::Tree,
                object_hash: id.kind(),
                data: &[],
            }));
        }
        self.0.try_find(id, buffer)
    }
}
