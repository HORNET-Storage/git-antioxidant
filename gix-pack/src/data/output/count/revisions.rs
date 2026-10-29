//! Count the objects that `git pack-objects --revs` packs for revisions, as a push sends them: the commits that the tips
//! reach and the haves don't, and the tags, trees and blobs that the tips and these commits reach and the haves don't,
//! each with the name hash of the path at which the walk finds it first, and with the preferred bases of a thin pack.
use std::sync::atomic::{AtomicBool, Ordering};

use gix_error::{Result, ResultExt, bail, cancelled, message};
use gix_hash::{ObjectId, oid};
use gix_hashtable::{HashMap, HashSet};
use gix_object::{Find, FindExt, Kind};
use gix_traverse::commit::{
    Simple,
    simple::{CommitTimeOrder, Sorting},
};

use super::{preferred::PreferredBases, walk::Walk};
use crate::data::{delta::search::name_hash, output};

/// Options for [`objects()`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Whether to add the preferred bases of a thin pack, as `git pack-objects --thin` does.
    pub thin: bool,
    /// The trees of at most this many commits at the edge of what is sent give preferred bases, as Git takes them from as
    /// many commits as its window has objects (`pack.window`).
    pub window: usize,
}

impl Default for Options {
    /// No preferred bases, from at most as many commits as the default window of `git pack-objects` has objects, 10.
    fn default() -> Self {
        Options {
            thin: false,
            window: 10,
        }
    }
}

/// What [`objects()`] found.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// The objects to pack and the preferred bases among them, in the order in which `git pack-objects` lists them.
    pub counts: Vec<output::Count>,
    /// The objects that the receiver has as far as the walk found out, which deltas of a thin pack may refer to: those
    /// of the haves, the trees and blobs in the trees of the haves, and the commits at the edge of what is sent with the
    /// trees and blobs in their trees.
    pub uninteresting: HashSet<ObjectId>,
}

/// Count the objects that `git pack-objects --revs` packs for the revisions `<tips> --not <haves>`, as `pack_objects()` in
/// Git's `send-pack.c` passes them for a push, reading them from `objects`, which must return the objects that it is
/// asked for rather than their replacements.
///
/// * `tips` are the objects to send, which can be commits, trees, blobs and annotated tags of any of them. It is an error
///   if one of them or an object that it points to is missing.
/// * `haves` are objects that the receiver has, along with the history of the commits among them. The objects among
///   them that are missing are ignored, as Git ignores them.
/// * `progress` counts the objects that are listed.
/// * `should_interrupt` is checked regularly, and once it is set, counting stops with an error.
///
/// The objects are listed as Git's `traverse_commit_list()` lists them, with the name hash of the path at which it finds
/// each first: the commits that the tips reach and the haves don't, the newest first, then the annotated tags, trees and
/// blobs of the tips in their order, a tag with the name hash of its name, and then the trees and blobs of the commits,
/// the tree of the newest commit first, each tree before its entries and these in their order. Trees and blobs that the
/// trees of the haves contain are left out, and so are those that the trees of the commits at the edge of what is sent
/// contain: the commits that the haves reach and that sent commits name as parents.
///
/// With [`Options::thin`], [preferred bases](output::Count::preferred_base) are listed as `git pack-objects --thin` adds
/// them: before each tag, tree and blob, the objects at its path in the trees of the first [`Options::window`] commits at
/// the edge are listed, unless a path of the same name hash was looked up before, and the trees themselves before each
/// tree without a path.
///
/// ### Deviation
///
/// * Git marks the trees of the edge as uninteresting along the paths that the sent trees have, as `pack.useSparse`
///   lets it by default, and may thus send objects that these trees have at other paths, while here all their objects
///   are left out, as Git leaves them out with `pack.useSparse=false`.
/// * Git walks the commits in the order of their dates, as here, but only stops looking for commits that the haves
///   reach once all commits in its queue are such commits, so commits with skewed dates can come in another order.
/// * For a thin pack of a shallow repository, which `git push` asks for with `--shallow`, Git's
///   `--objects-edge-aggressive` also takes the commits that are given as haves, and the tips that the haves reach, as
///   edges whose trees are left out and give preferred bases, while here the edges are only the commits that the haves
///   reach and that sent commits name as parents.
pub fn objects<F: Find>(
    objects: &F,
    tips: impl IntoIterator<Item = ObjectId>,
    haves: impl IntoIterator<Item = ObjectId>,
    progress: &dyn gix_utils::progress::Count,
    should_interrupt: &AtomicBool,
    options: Options,
) -> Result<Outcome> {
    let counted = progress.counter();
    let mut buf = Vec::new();
    let mut uninteresting = HashSet::default();

    let mut hidden = Vec::new();
    for id in haves {
        match peel(objects, id, |tag, _| _ = uninteresting.insert(tag), &mut buf)? {
            Some((id, Kind::Commit)) => hidden.push(id),
            Some((id, Kind::Tree)) => mark_uninteresting(objects, id, &mut uninteresting, &mut buf, should_interrupt)?,
            Some((id, Kind::Blob)) => _ = uninteresting.insert(id),
            Some((_, Kind::Tag)) | None => {}
        }
    }

    // The objects of the tips other than commits, in the order in which Git's `prepare_revision_walk()` leaves them
    // pending: the tags that each tip peels through, and the tree or blob that it ends in.
    let (mut pending, mut tip_commits) = (Vec::new(), Vec::new());
    for id in tips {
        match peel(
            objects,
            id,
            |tag, name| pending.push(Pending::Tag(tag, name.to_vec())),
            &mut buf,
        )? {
            Some((id, Kind::Commit)) => tip_commits.push(id),
            Some((id, Kind::Tree)) => pending.push(Pending::Tree(id)),
            Some((id, Kind::Blob)) => pending.push(Pending::Blob(id)),
            Some((_, Kind::Tag)) => {}
            None => bail!("The object {id} to pack, or an object it points to, is missing".not_found()),
        }
    }

    let (mut commit_ids, mut parent_ids) = (Vec::new(), Vec::new());
    if !tip_commits.is_empty() {
        let walk = Simple::new(tip_commits.iter().copied(), objects)
            .sorting(Sorting::ByCommitTime(CommitTimeOrder::NewestFirst))?
            .hide(hidden)?;
        for info in walk {
            if should_interrupt.load(Ordering::Relaxed) {
                bail!(cancelled("Interrupted while walking the commits to pack"));
            }
            let info = info?;
            commit_ids.push(info.id);
            parent_ids.extend(info.parent_ids.iter().copied());
        }
    }

    // The trees of the commits at the edge, which Git's `mark_edges_uninteresting()` finds as the parents of sent commits
    // that aren't sent and shows to `show_edge()` in this order, contain what the receiver has of the trees that are sent.
    let sent: HashSet<ObjectId> = commit_ids.iter().copied().collect();
    let mut edges = Vec::new();
    for &commit_id in &parent_ids {
        if !sent.contains(&commit_id) && uninteresting.insert(commit_id) {
            edges.push(commit_id);
            let tree_id = tree_of(objects, commit_id, &mut buf)?;
            mark_uninteresting(objects, tree_id, &mut uninteresting, &mut buf, should_interrupt)?;
        }
    }

    let mut listing = Listing {
        counts: Vec::with_capacity(commit_ids.len()),
        positions: HashMap::default(),
    };
    for &commit_id in &commit_ids {
        listing.object(commit_id, 0);
        counted.fetch_add(1, Ordering::Relaxed);
    }
    let mut preferred = if options.thin {
        Some(PreferredBases::new(objects, &edges, options.window)?)
    } else {
        None
    };
    let mut found = |id: &oid, _kind: Kind, path: &[u8]| -> Result {
        if let Some(preferred) = preferred.as_mut() {
            preferred.add(objects, path, &mut |base_id, hash| {
                listing.preferred_base(base_id, hash);
            })?;
        }
        listing.object(id.to_owned(), name_hash(path));
        counted.fetch_add(1, Ordering::Relaxed);
        Ok(())
    };

    let mut seen = uninteresting.clone();
    let mut walk = Walk::new(objects, should_interrupt);
    for item in pending {
        match item {
            Pending::Tag(id, name) => {
                if seen.insert(id) {
                    found(&id, Kind::Tag, &name)?;
                }
            }
            Pending::Tree(id) => walk.tree(id, &mut seen, &mut found)?,
            Pending::Blob(id) => {
                if seen.insert(id) {
                    found(&id, Kind::Blob, b"")?;
                }
            }
        }
    }
    for &commit_id in &commit_ids {
        let tree_id = tree_of(objects, commit_id, &mut buf)?;
        walk.tree(tree_id, &mut seen, &mut found)?;
    }
    Ok(Outcome {
        counts: listing.counts,
        uninteresting,
    })
}

/// An object of the tips that Git leaves pending for its walk of trees and blobs.
enum Pending {
    /// An annotated tag, with its name.
    Tag(ObjectId, Vec<u8>),
    Tree(ObjectId),
    Blob(ObjectId),
}

/// The objects listed so far, as Git's `add_object_entry()` lists them.
struct Listing {
    counts: Vec<output::Count>,
    /// The index of each listed object in `counts`.
    positions: HashMap<ObjectId, usize>,
}

impl Listing {
    /// List the object `id` to be packed with `name_hash`, unless it is listed already.
    fn object(&mut self, id: ObjectId, name_hash: u32) {
        self.add(id, name_hash, false);
    }

    /// List the object `id` as a preferred base with `name_hash`, or make it a preferred base if it is listed already,
    /// as Git does.
    fn preferred_base(&mut self, id: ObjectId, name_hash: u32) {
        if let Some(&position) = self.positions.get(&id) {
            self.counts[position].preferred_base = true;
            return;
        }
        self.add(id, name_hash, true);
    }

    fn add(&mut self, id: ObjectId, name_hash: u32, preferred_base: bool) {
        if self.positions.contains_key(&id) {
            return;
        }
        self.positions.insert(id, self.counts.len());
        self.counts.push(output::Count {
            id,
            entry_pack_location: output::count::PackLocation::NotLookedUp,
            name_hash,
            preferred_base,
            tagged: false,
        });
    }
}

/// The tree of the commit `commit_id`.
fn tree_of(objects: &impl Find, commit_id: ObjectId, buf: &mut Vec<u8>) -> Result<ObjectId> {
    objects
        .find_commit_iter(&commit_id, buf)?
        .tree_id()
        .or_raise(|| message!("Could not decode the commit {commit_id}").corrupted())
}

/// Follow the annotated tags starting at `id` to the object they point to, passing each tag and its name to `on_tag`, and
/// return that object along with its kind, or `None` if an object on the way is missing.
fn peel(
    objects: &impl Find,
    mut id: ObjectId,
    mut on_tag: impl FnMut(ObjectId, &[u8]),
    buf: &mut Vec<u8>,
) -> Result<Option<(ObjectId, Kind)>> {
    loop {
        // Git provides the empty tree whether or not a repository stores it.
        if id.is_empty_tree() {
            return Ok(Some((id, Kind::Tree)));
        }
        let Some(object) = objects.try_find(&id, buf)? else {
            return Ok(None);
        };
        if object.kind != Kind::Tag {
            return Ok(Some((id, object.kind)));
        }
        let (mut target, mut name): (_, &[u8]) = (None, &[]);
        for token in gix_object::TagRefIter::from_bytes(object.data, object.object_hash) {
            match token.or_raise(|| message!("Could not decode the tag {id}").corrupted())? {
                gix_object::tag::ref_iter::Token::Target { id } => target = Some(id),
                gix_object::tag::ref_iter::Token::Name(tag_name) => {
                    name = tag_name.as_ref();
                    break;
                }
                _ => {}
            }
        }
        on_tag(id, name);
        let Some(target) = target else {
            bail!(message!("Could not decode the tag {id}").corrupted());
        };
        id = target;
    }
}

/// Add the tree `tree_id` and the trees and blobs it contains to `uninteresting`, without entering trees that are in it
/// already. The commits of submodules are left out, as they belong to other repositories.
fn mark_uninteresting(
    objects: &impl Find,
    tree_id: ObjectId,
    uninteresting: &mut HashSet<ObjectId>,
    buf: &mut Vec<u8>,
    should_interrupt: &AtomicBool,
) -> Result {
    let mut tree_ids = vec![tree_id];
    while let Some(tree_id) = tree_ids.pop() {
        if !uninteresting.insert(tree_id) {
            continue;
        }
        if should_interrupt.load(Ordering::Relaxed) {
            bail!(cancelled(
                "Interrupted while finding the trees and blobs that the receiver has"
            ));
        }
        // Git provides the empty tree whether or not a repository stores it.
        if tree_id.is_empty_tree() {
            continue;
        }
        for entry in objects.find_tree_iter(&tree_id, buf)? {
            let entry = entry.or_raise(|| message!("Could not decode the tree {tree_id}").corrupted())?;
            if entry.mode.is_tree() {
                tree_ids.push(entry.oid.to_owned());
            } else if !entry.mode.is_commit() {
                uninteresting.insert(entry.oid.to_owned());
            }
        }
    }
    Ok(())
}
