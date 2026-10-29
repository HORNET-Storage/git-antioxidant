use std::sync::atomic::AtomicBool;

use gix_error::Result;
use gix_hash::{ObjectId, oid};
use gix_object::Find;
use gix_pack::data::{delta::search::name_hash, output::count::walk::Walk};

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
/// The walk goes through the history as [`Walk::commit_trees()`] does, and after all commits, it visits the trees of the
/// commits that it doesn't ignore in the order of the commits, each tree before its entries and these in their order,
/// and every object at the first path at which it is found.
pub(crate) fn assign(
    objects: &impl Find,
    commits: &[ObjectId],
    is_retained: impl Fn(&oid) -> bool,
    indices: &gix_hashtable::HashMap<ObjectId, u32>,
    name_hashes: &mut [u32],
    should_interrupt: &AtomicBool,
) -> Result {
    let mut walk = Walk::new(objects, should_interrupt).ignore_missing_links();
    let roots = walk.commit_trees(commits, is_retained)?;
    let mut seen = gix_hashtable::HashSet::default();
    let mut found = |id: &oid, _kind: gix_object::Kind, path: &[u8]| -> Result {
        if let Some(&index) = indices.get(id) {
            name_hashes[index as usize] = name_hash(path);
        }
        Ok(())
    };
    for root in roots {
        walk.tree(root, &mut seen, &mut found)?;
    }
    Ok(())
}
