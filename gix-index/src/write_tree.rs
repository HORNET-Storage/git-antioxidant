//! Write the trees that the entries of an index describe, as `git write-tree` does.
//!
//! An index lists the files of the next commit by their paths. [`State::write_tree()`] turns these entries into one
//! tree object per directory, from the deepest directories up to the root tree that a commit records, as Git's
//! `cache-tree.c` does. It also returns the cache tree, the index's `TREE` extension, which records the tree of each
//! directory along with the number of entries in it. Given the cache tree of an earlier call, directories that didn't
//! change keep their tree without their entries being read again. For that, [`Tree::invalidate_path()`] has to be
//! called for every entry that was added, changed or removed in between, as Git does whenever it changes the index.
//!
//! [`State::repair_cache_tree()`] computes the cache tree without writing any object, as Git does after a checkout or
//! a merge changed the index: the node of a directory is valid only if the object database already has its tree.

use std::{cmp::Ordering, ops::Range};

use bstr::{BStr, ByteSlice};
use gix_error::{Exn, Message, OptionExt, Result, ResultExt, bail, message};
use gix_hash::ObjectId;
use gix_object::{
    TreeRef, WriteTo,
    tree::{EntryKind, EntryRef},
};

use crate::{Entry, State, entry, extension::Tree};

/// The number of unmerged entries or conflicting paths that an error lists before it ends the list with `...`, as Git
/// does.
const MAX_LISTED: usize = 10;

/// Options for [`State::write_tree()`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// If `true`, entries may refer to objects that the object database doesn't have, as with `git write-tree --missing-ok`.
    ///
    /// Otherwise the object of every entry has to exist, except for the commits of submodules, which belong to other
    /// repositories.
    pub missing_ok: bool,
}

/// The trees that [`State::write_tree()`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The id of the root tree.
    pub tree_id: ObjectId,
    /// The cache tree of the entries, to be stored as the index's `TREE` extension with [`State::set_tree()`].
    ///
    /// Each node records the tree of its directory along with the number of entries in it. A node is invalid if its
    /// directory has entries that were added with intent to add, as no tree contains them.
    pub cache_tree: Tree,
    /// The number of trees that were written because the object database didn't have them yet.
    pub written_trees: usize,
}

impl State {
    /// Write the trees of the entries of this index to `objects` and return the id of the root tree, as
    /// `git write-tree` does.
    ///
    /// Each directory becomes a tree that lists its files, symbolic links and submodule commits along with the trees
    /// of its subdirectories. Entries that were added with intent to add are left out, and so are directories that
    /// contain nothing else. A sparse directory entry is the tree whose id it records. Entries flagged with
    /// [`REMOVE`](entry::Flags::REMOVE) are ignored, as they aren't written with the index.
    ///
    /// `cache_tree` is usually the index's own `TREE` extension, [`tree()`](Self::tree()). A directory whose node in
    /// it is valid, counts as many entries as the directory has and names a tree that `objects` has keeps that tree,
    /// and its entries aren't read. This is correct only if every entry that was added, changed or removed since the
    /// cache tree was computed was passed to [`Tree::invalidate_path()`].
    ///
    /// Trees are hashed in memory and only written if `objects` doesn't have them yet.
    ///
    /// ### Errors
    ///
    /// Unmerged entries are refused, listing up to ten of them as Git does, and so are paths that are both a file and
    /// a directory. Entries that aren't sorted, or whose paths can't be the paths of tree entries, are refused as
    /// corrupt. So is an entry with a null object id, and an entry whose object `objects` doesn't have is refused
    /// unless [`Options::missing_ok`] is set. The commits of submodules are never looked up, as they belong to other
    /// repositories.
    pub fn write_tree<O>(&self, cache_tree: Option<&Tree>, objects: &O, options: Options) -> Result<Outcome>
    where
        O: gix_object::Write + gix_object::Exists + ?Sized,
    {
        let _span = gix_trace::coarse!("gix_index::State::write_tree()");
        let entries = entries_of(self)?;
        let write = |data: &[u8], id: ObjectId| objects.write_buf_with_known_id(gix_object::Kind::Tree, data, id);
        let mut writer = Writer {
            entries: &entries,
            objects,
            object_hash: self.object_hash(),
            mode: Mode::Write {
                write: &write,
                missing_ok: options.missing_ok,
            },
            written_trees: 0,
            buf: Vec::new(),
        };
        let root = writer.write_directory(0..entries.len(), 0, b"".as_bstr(), cache_tree)?;
        Ok(Outcome {
            tree_id: root.tree_id,
            cache_tree: root.node,
            written_trees: writer.written_trees,
        })
    }

    /// Return the cache tree of the entries of this index without writing any object, as Git repairs the cache tree
    /// after a checkout or a merge changed the index.
    ///
    /// `cache_tree` is usually the cache tree of the index before it changed, with [`Tree::invalidate_path()`] called
    /// for every entry that was added, changed or removed since. Its valid nodes are kept as
    /// [`write_tree()`](Self::write_tree()) keeps them. Every other directory is hashed from its entries, and its node
    /// is valid only if `objects` has that tree, so that a later [`write_tree()`](Self::write_tree()) can use it
    /// without writing it. A directory is invalid as well if it has entries that were added with intent to add,
    /// entries with a null object id, or subdirectories whose nodes are invalid. The objects of the entries aren't
    /// looked up, as a tree that `objects` has records their ids already.
    ///
    /// ### Errors
    ///
    /// The entries that [`write_tree()`](Self::write_tree()) refuses are refused, except for those whose objects are
    /// null or missing.
    pub fn repair_cache_tree<O>(&self, cache_tree: Option<&Tree>, objects: &O) -> Result<Tree>
    where
        O: gix_object::Exists + ?Sized,
    {
        let _span = gix_trace::coarse!("gix_index::State::repair_cache_tree()");
        let entries = entries_of(self)?;
        let mut writer = Writer {
            entries: &entries,
            objects,
            object_hash: self.object_hash(),
            mode: Mode::Repair,
            written_trees: 0,
            buf: Vec::new(),
        };
        Ok(writer
            .write_directory(0..entries.len(), 0, b"".as_bstr(), cache_tree)?
            .node)
    }
}

/// The entries of an index that aren't flagged for removal, with their paths.
type IndexEntries<'a> = [(&'a BStr, &'a Entry)];

/// The entries of `state` that aren't flagged for removal, with their paths, once those that can't be the entries of
/// trees were refused.
fn entries_of(state: &State) -> Result<Vec<(&BStr, &Entry)>> {
    let entries: Vec<_> = state
        .entries()
        .iter()
        .filter(|entry| !entry.flags.contains(entry::Flags::REMOVE))
        .map(|entry| (entry.path(state), entry))
        .collect();
    refuse_unmerged_entries(&entries)?;
    refuse_invalid_paths(&entries)?;
    refuse_files_used_as_directories(&entries)?;
    Ok(entries)
}

/// Refuse entries that aren't merged, listing up to [`MAX_LISTED`] of them as Git does.
fn refuse_unmerged_entries(entries: &IndexEntries<'_>) -> Result<()> {
    let mut unmerged = entries
        .iter()
        .filter(|(_, entry)| entry.stage() != entry::Stage::Unconflicted)
        .map(|(path, entry)| message!("{path}: unmerged ({})", entry.id));
    let listed: Vec<_> = unmerged.by_ref().take(MAX_LISTED).collect();
    if listed.is_empty() {
        return Ok(());
    }
    let more = unmerged.next().map(|_| message("..."));
    Err(Exn::raise_all(
        listed.into_iter().chain(more),
        message("The index has unmerged entries").conflict(),
    )
    .into())
}

/// Refuse entries whose paths can't be the paths of tree entries, and entries that aren't sorted by their paths.
fn refuse_invalid_paths(entries: &IndexEntries<'_>) -> Result<()> {
    let mut previous_path = None::<&BStr>;
    for (path, entry) in entries {
        let path = *path;
        let components = match (path.as_bytes().strip_suffix(b"/"), entry.mode.is_sparse()) {
            (Some(directory), true) => directory,
            (None, false) => path.as_bytes(),
            (None, true) => bail!("The sparse directory entry '{path}' doesn't end in a slash".corrupted()),
            (Some(_), false) => bail!("The entry '{path}' ends in a slash but isn't a sparse directory".corrupted()),
        };
        if components.split(|byte| *byte == b'/').any(<[u8]>::is_empty) {
            bail!("The path of the entry '{path}' has an empty component".corrupted());
        }
        if components.contains(&0) {
            bail!("The path of the entry '{path}' contains a NUL byte".corrupted());
        }
        if let Some(previous_path) = previous_path.replace(path) {
            match previous_path.cmp(path) {
                Ordering::Less => {}
                Ordering::Equal => bail!("The index has more than one entry at '{path}'".corrupted()),
                Ordering::Greater => {
                    bail!("The index entries aren't sorted as '{previous_path}' precedes '{path}'".corrupted())
                }
            }
        }
    }
    Ok(())
}

/// Refuse entries whose paths other entries use as directories, listing up to [`MAX_LISTED`] of them.
///
/// Git compares each entry only with the entry after it, which misses a file `a` when `a-b` sorts between it and `a/c`.
/// Files that may still be followed by entries in their directory are kept until the entries are past it.
fn refuse_files_used_as_directories(entries: &IndexEntries<'_>) -> Result<()> {
    let mut files = Vec::<&BStr>::new();
    let mut conflicts = Vec::new();
    'entries: for (path, entry) in entries {
        while let Some(file) = files.last().copied() {
            match path
                .as_bytes()
                .strip_prefix(file.as_bytes())
                .and_then(|rest| rest.first())
            {
                Some(b'/') => {
                    if conflicts.len() == MAX_LISTED {
                        conflicts.push(message("..."));
                        break 'entries;
                    }
                    conflicts.push(message!("You have both {file} and {path}"));
                    files.pop();
                }
                Some(byte) if *byte < b'/' => break,
                _ => {
                    files.pop();
                }
            }
        }
        if !entry.mode.is_sparse() {
            files.push(path);
        }
    }
    if conflicts.is_empty() {
        return Ok(());
    }
    Err(Exn::raise_all(
        conflicts,
        message("Index entries are files at paths that other entries use as directories").corrupted(),
    )
    .into())
}

/// The message with which Git refuses an entry whose object is null or missing.
fn invalid_object(mode: u32, id: &gix_hash::oid, path: &BStr) -> Message {
    message!("Invalid object {mode:06o} {id} for '{path}'")
}

/// What [`Writer`] does with the trees that it hashes.
#[derive(Clone, Copy)]
enum Mode<'a> {
    /// Write the trees that the object database doesn't have yet with `write`, and refuse entries whose objects are
    /// missing unless `missing_ok` is set.
    Write {
        write: &'a dyn Fn(&[u8], ObjectId) -> Result<ObjectId>,
        missing_ok: bool,
    },
    /// Write nothing, and make the nodes of the directories whose trees the object database doesn't have invalid.
    Repair,
}

/// Hashes the tree of each directory, holding what all of them need.
struct Writer<'a, O: ?Sized> {
    entries: &'a IndexEntries<'a>,
    objects: &'a O,
    object_hash: gix_hash::Kind,
    mode: Mode<'a>,
    written_trees: usize,
    /// The encoding of the tree that is hashed next.
    buf: Vec<u8>,
}

/// A directory whose tree was hashed, or whose existing tree was found.
struct Directory {
    /// The id of the directory's tree. It is the empty tree if the directory only has entries that were added with intent
    /// to add, and null if its tree isn't known while repairing.
    tree_id: ObjectId,
    /// The directory's node in the cache tree.
    node: Tree,
    /// Whether `tree_id` is what a sparse directory entry recorded, so that it isn't known to exist yet.
    from_sparse_entry: bool,
}

/// An entry of the tree of a directory.
enum Item<'a> {
    /// An entry of the index, with its file name and its path.
    Entry {
        name: &'a BStr,
        path: &'a BStr,
        entry: &'a Entry,
    },
    /// A subdirectory, with its name, its path and its position among the subdirectories.
    Directory {
        name: &'a BStr,
        path: &'a BStr,
        index: usize,
    },
}

impl<'a, O> Writer<'a, O>
where
    O: gix_object::Exists + ?Sized,
{
    /// Return the tree of the directory `name`, whose entries are in `range` of the entries and whose path is `base_len`
    /// bytes long, including its trailing slash. `cached` is the directory's node in the cache tree.
    fn write_directory(
        &mut self,
        range: Range<usize>,
        base_len: usize,
        name: &BStr,
        cached: Option<&Tree>,
    ) -> Result<Directory> {
        let all: &'a IndexEntries<'a> = self.entries;
        let entries = &all[range.clone()];
        if let Some((path, entry)) = entries
            .first()
            .filter(|(path, entry)| entry.mode.is_sparse() && path.len() == base_len)
        {
            if let Some((inner_path, _)) = entries.get(1) {
                bail!("The sparse directory entry '{path}' is followed by '{inner_path}' inside of it".corrupted());
            }
            return Ok(Directory {
                tree_id: entry.id,
                node: Tree {
                    name: name.as_bytes().into(),
                    id: entry.id,
                    num_entries: Some(1),
                    children: Vec::new(),
                },
                from_sparse_entry: true,
            });
        }

        let num_entries =
            u32::try_from(entries.len()).or_raise(|| message("A cache tree can't count more than u32::MAX entries"))?;
        if let Some(cached) =
            cached.filter(|cached| cached.num_entries == Some(num_entries) && self.has_object(&cached.id))
        {
            return Ok(Directory {
                tree_id: cached.id,
                node: Tree {
                    name: name.as_bytes().into(),
                    ..cached.clone()
                },
                from_sparse_entry: false,
            });
        }

        // Write the trees of the subdirectories first, as their ids are part of this tree.
        let mut items = Vec::new();
        let mut directories = Vec::new();
        let mut index = range.start;
        while index < range.end {
            let (path, entry) = all[index];
            let rest = &path[base_len..];
            let Some(name_len) = rest.find_byte(b'/') else {
                items.push(Item::Entry {
                    name: rest,
                    path,
                    entry,
                });
                index += 1;
                continue;
            };
            let prefix = &path[..base_len + name_len + 1];
            let end =
                index + 1 + all[index + 1..range.end].partition_point(|(other_path, _)| other_path.starts_with(prefix));
            let directory_name = &rest[..name_len];
            let directory_cached = cached.and_then(|cached| {
                cached
                    .children
                    .iter()
                    .find(|child| child.name.as_slice() == directory_name.as_bytes())
            });
            let directory = self.write_directory(index..end, prefix.len(), directory_name, directory_cached)?;
            items.push(Item::Directory {
                name: directory_name,
                path: &prefix[..prefix.len() - 1],
                index: directories.len(),
            });
            directories.push(directory);
            index = end;
        }

        // Like Git, look at the entries of this directory only after those of its subdirectories.
        let repair = matches!(self.mode, Mode::Repair);
        let mut contains_intent_to_add = false;
        // While repairing, whether an entry or a subdirectory keeps the tree of this directory from being known.
        let mut unknown = false;
        let mut tree_entries = Vec::with_capacity(items.len());
        for item in &items {
            match *item {
                Item::Entry { name, path, entry } => {
                    let mode = entry.mode.bits();
                    if entry.id.is_null() {
                        if repair {
                            unknown = true;
                            continue;
                        }
                        bail!(invalid_object(mode, &entry.id, path).corrupted());
                    }
                    if entry.flags.contains(entry::Flags::INTENT_TO_ADD) {
                        contains_intent_to_add = true;
                        continue;
                    }
                    if let Mode::Write { missing_ok: false, .. } = self.mode
                        && !(entry.mode.is_submodule() || self.has_object(&entry.id))
                    {
                        bail!(invalid_object(mode, &entry.id, path).not_found());
                    }
                    let tree_mode = entry
                        .mode
                        .to_tree_entry_mode()
                        .ok_or_raise(|| message!("The entry '{path}' has the invalid mode {mode:o}").corrupted())?;
                    tree_entries.push(EntryRef {
                        mode: tree_mode,
                        filename: name,
                        oid: &entry.id,
                    });
                }
                Item::Directory { name, path, index } => {
                    let directory = &directories[index];
                    if directory.from_sparse_entry {
                        let mode = entry::Mode::DIR.bits();
                        match self.mode {
                            Mode::Repair => {
                                if directory.tree_id.is_null() || !self.has_object(&directory.tree_id) {
                                    unknown = true;
                                    continue;
                                }
                            }
                            Mode::Write { missing_ok, .. } => {
                                if directory.tree_id.is_null() {
                                    bail!(invalid_object(mode, &directory.tree_id, path).corrupted());
                                }
                                if !(missing_ok || self.has_object(&directory.tree_id)) {
                                    bail!(invalid_object(mode, &directory.tree_id, path).not_found());
                                }
                            }
                        }
                    }
                    if directory.node.num_entries.is_none() {
                        if repair {
                            unknown = true;
                            continue;
                        }
                        contains_intent_to_add = true;
                        if directory.tree_id.is_empty_tree() {
                            continue;
                        }
                    }
                    tree_entries.push(EntryRef {
                        mode: EntryKind::Tree.into(),
                        filename: name,
                        oid: &directory.tree_id,
                    });
                }
            }
        }

        let tree_id = if unknown || (repair && contains_intent_to_add) {
            None
        } else {
            self.buf.clear();
            TreeRef { entries: tree_entries }
                .write_to(&mut self.buf)
                .or_raise(|| message!("Could not encode the tree of '{name}'"))?;
            let tree_id = gix_object::compute_hash(self.object_hash, gix_object::Kind::Tree, &self.buf)?;
            match self.mode {
                Mode::Write { write, .. } => {
                    if !self.objects.exists(&tree_id) {
                        write(&self.buf, tree_id)
                            .or_raise(|| message!("Could not write the tree {tree_id} of '{name}'"))?;
                        self.written_trees += 1;
                    }
                    Some(tree_id)
                }
                Mode::Repair => self.has_object(&tree_id).then_some(tree_id),
            }
        };

        let mut children: Vec<_> = directories.into_iter().map(|directory| directory.node).collect();
        children.sort_by(|a, b| a.name.cmp(&b.name));
        let valid_tree_id = tree_id.filter(|_| !contains_intent_to_add);
        Ok(Directory {
            tree_id: tree_id.unwrap_or_else(|| self.object_hash.null()),
            node: Tree {
                name: name.as_bytes().into(),
                id: valid_tree_id.unwrap_or_else(|| self.object_hash.null()),
                num_entries: valid_tree_id.map(|_| num_entries),
                children,
            },
            from_sparse_entry: false,
        })
    }

    /// Return `true` if the object database has `id`, or if it's the empty tree, which Git always has.
    fn has_object(&self, id: &gix_hash::oid) -> bool {
        id.is_empty_tree() || self.objects.exists(id)
    }
}
