use std::{cmp::Ordering, collections::HashSet};

use gix_error::{Result, ResultExt, message};
use gix_hash::ObjectId;
use gix_object::{Find, Kind};

use crate::data::delta::search::name_hash;

/// The preferred bases of a thin pack, the objects that `git pack-objects --thin` takes from the trees of the commits at
/// the edge of what it sends, at the paths of the objects that it sends, as its `add_preferred_base()` and
/// `add_preferred_base_object()` do.
pub(super) struct PreferredBases {
    /// The ids and the data of the trees of the commits at the edge, the one added last first, as Git's `pbase_tree`
    /// lists them.
    trees: Vec<(ObjectId, Vec<u8>, gix_hash::Kind)>,
    /// Whether there are commits at the edge, as Git's `num_preferred_base` tells.
    has_edges: bool,
    /// The name hashes of the paths that were looked up, as Git's `done_pbase_paths` holds them.
    done: HashSet<u32>,
}

impl PreferredBases {
    /// Take the trees of the first `window` of the commits at the `edges` of what is sent, as Git's `add_preferred_base()`
    /// does for each commit that its `show_edge()` shows, in that order. Commits whose tree can't be read are left out,
    /// and so are trees taken already.
    pub(super) fn new(objects: &impl Find, edges: &[ObjectId], window: usize) -> Result<Self> {
        let mut trees: Vec<(ObjectId, Vec<u8>, gix_hash::Kind)> = Vec::new();
        let mut buf = Vec::new();
        for &edge in edges.iter().take(window) {
            let Some(tree_id) = peel_to_tree(objects, edge, &mut buf)? else {
                continue;
            };
            if trees.iter().any(|(id, ..)| *id == tree_id) {
                continue;
            }
            if let Some((data, object_hash)) = read_tree(objects, tree_id, &mut buf)? {
                trees.push((tree_id, data, object_hash));
            }
        }
        trees.reverse();
        Ok(PreferredBases {
            trees,
            has_edges: !edges.is_empty(),
            done: HashSet::new(),
        })
    }

    /// Pass the preferred bases at `path` to `add` along with the name hash of the path, as Git's
    /// `add_preferred_base_object()` does before the object at `path` is listed: the objects at `path` in each tree, or
    /// the trees themselves for the empty path, which have the name hash 0. Paths with the name hash of a path that was
    /// looked up already are skipped, as in Git.
    pub(super) fn add(&mut self, objects: &impl Find, path: &[u8], add: &mut dyn FnMut(ObjectId, u32)) -> Result {
        let hash = name_hash(path);
        if !self.has_edges || !self.done.insert(hash) {
            return Ok(());
        }
        let first_len = component_len(path);
        for (tree_id, data, object_hash) in &self.trees {
            if first_len == 0 {
                add(*tree_id, 0);
            } else {
                add_at(objects, data, *object_hash, path, first_len, hash, add)?;
            }
        }
        Ok(())
    }
}

/// Pass the object at `name` in the tree with `data` to `add` with `hash`, as Git's `add_pbase_object()` finds it: the
/// first component of `name` has `len` bytes, and is looked up among the entries of the same length, other than those of
/// submodules, in their order, until one sorts after it.
fn add_at(
    objects: &impl Find,
    data: &[u8],
    object_hash: gix_hash::Kind,
    name: &[u8],
    len: usize,
    hash: u32,
    add: &mut dyn FnMut(ObjectId, u32),
) -> Result {
    for entry in gix_object::TreeRefIter::from_bytes(data, object_hash).map_while(std::result::Result::ok) {
        if entry.mode.is_commit() {
            continue;
        }
        let filename: &[u8] = entry.filename;
        let order = if filename.len() == len {
            name[..len].cmp(filename)
        } else {
            Ordering::Greater
        };
        match order {
            Ordering::Greater => continue,
            Ordering::Less => return Ok(()),
            Ordering::Equal => {}
        }
        if name.get(len) != Some(&b'/') {
            add(entry.oid.to_owned(), hash);
            return Ok(());
        }
        if entry.mode.is_tree() {
            let mut buf = Vec::new();
            let Some((subtree, object_hash)) = read_tree(objects, entry.oid.to_owned(), &mut buf)? else {
                return Ok(());
            };
            let rest = &name[len + 1..];
            add_at(objects, &subtree, object_hash, rest, component_len(rest), hash, add)?;
        }
    }
    Ok(())
}

/// The length of the first component of `path`, which ends before a slash or a line feed, as Git's `name_cmp_len()`
/// measures it.
fn component_len(path: &[u8]) -> usize {
    path.iter()
        .position(|&byte| byte == b'/' || byte == b'\n')
        .unwrap_or(path.len())
}

/// The tree that `id` peels to, through tags and commits, or `None` if an object on the way can't be read, as Git's
/// `odb_read_object_peeled()` peels it.
fn peel_to_tree(objects: &impl Find, mut id: ObjectId, buf: &mut Vec<u8>) -> Result<Option<ObjectId>> {
    loop {
        if id.is_empty_tree() {
            return Ok(Some(id));
        }
        let Some(object) = objects.try_find(&id, buf)? else {
            return Ok(None);
        };
        id = match object.kind {
            Kind::Tree => return Ok(Some(id)),
            Kind::Blob => return Ok(None),
            Kind::Commit => gix_object::CommitRefIter::from_bytes(object.data, object.object_hash)
                .tree_id()
                .or_raise(|| message!("Could not decode the commit {id}").corrupted())?,
            Kind::Tag => gix_object::TagRefIter::from_bytes(object.data, object.object_hash)
                .target_id()
                .or_raise(|| message!("Could not decode the tag {id}").corrupted())?,
        };
    }
}

/// The data and the object hash of the tree `id`, or `None` if it can't be read or isn't a tree. Git provides the empty
/// tree whether or not `objects` stores it.
fn read_tree(objects: &impl Find, id: ObjectId, buf: &mut Vec<u8>) -> Result<Option<(Vec<u8>, gix_hash::Kind)>> {
    if id.is_empty_tree() {
        return Ok(Some((Vec::new(), id.kind())));
    }
    Ok(objects
        .try_find(&id, buf)?
        .filter(|object| object.kind == Kind::Tree)
        .map(|object| (object.data.to_vec(), object.object_hash)))
}
