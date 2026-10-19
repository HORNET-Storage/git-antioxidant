use std::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};

use gix_error::{Result, ResultExt, bail, message};
use gix_hash::{ObjectId, oid};
use gix_object::{Exists, Find, Kind};

/// Objects found missing while checking connectivity. Only the first few are kept to name them.
#[derive(Default)]
pub(crate) struct Missing {
    first: Vec<ObjectId>,
    count: usize,
}

impl Missing {
    /// The amount of missing objects that are kept to name them.
    const KEPT: usize = 10;

    pub(crate) fn add(&mut self, id: &oid) {
        if self.first.len() < Self::KEPT {
            self.first.push(id.to_owned());
        }
        self.count += 1;
    }

    pub(crate) fn count(&self) -> usize {
        self.count
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The ids of the missing objects, listing only the first few if there are many.
    pub(crate) fn describe(&self) -> String {
        let named = self
            .first
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        match self.count - self.first.len() {
            0 => named,
            unnamed => format!("{named} and {unnamed} more"),
        }
    }
}

/// The end of a chain of annotated tags.
pub(crate) enum Peeled {
    /// The object that isn't a tag, with its kind.
    Present(ObjectId, Kind),
    /// The first object of the chain that is missing.
    Missing(ObjectId),
}

/// Find the objects that `objects` lacks among those reachable from `tips`, which may be objects of any kind.
///
/// Commits reachable from `complete`, whose history is known to be present, are neither walked nor checked,
/// as is the case for git's `rev-list --objects <tips> --not <complete>`.
pub(crate) fn missing_objects(
    objects: &(impl Find + Exists),
    tips: impl IntoIterator<Item = ObjectId>,
    complete: impl IntoIterator<Item = ObjectId>,
    should_interrupt: &AtomicBool,
) -> Result<Missing> {
    let missing = RefCell::new(Missing::default());
    let mut commit_ids = Vec::new();
    let mut tree_ids = Vec::new();
    let mut buf = Vec::new();
    for tip in tips {
        match peel(objects, tip, &mut Vec::new(), &mut buf)? {
            Peeled::Missing(id) => missing.borrow_mut().add(&id),
            Peeled::Present(commit_id, Kind::Commit) => commit_ids.push(commit_id),
            Peeled::Present(tree_id, Kind::Tree) => tree_ids.push(tree_id),
            Peeled::Present(_, Kind::Blob | Kind::Tag) => {}
        }
    }

    {
        let mut connectivity =
            gix_fsck::Connectivity::new(objects, |id: &ObjectId, _kind: Kind| missing.borrow_mut().add(id));
        let walk = gix_traverse::commit::Simple::filtered(commit_ids, objects, |commit_id: &oid| {
            let exists = objects.exists(commit_id);
            if !exists {
                missing.borrow_mut().add(commit_id);
            }
            exists
        })
        .hide(complete)?;
        for commit in walk {
            if should_interrupt.load(Ordering::Relaxed) {
                bail!(gix_error::cancelled(
                    "Interrupted while checking the objects of a bundle"
                ));
            }
            let commit = commit.or_raise(|| message("Could not walk the commits whose objects are checked"))?;
            connectivity.check_commit(&commit.id)?;
        }
        for tree_id in &tree_ids {
            connectivity.check_tree(tree_id);
        }
    }
    Ok(missing.into_inner())
}

/// Follow `id` through annotated tags to the object the chain ends in, checking that each tag names the kind of
/// its target correctly, and append the ids of the tags to `tag_ids`.
pub(crate) fn peel(
    objects: &impl Find,
    mut id: ObjectId,
    tag_ids: &mut Vec<ObjectId>,
    buf: &mut Vec<u8>,
) -> Result<Peeled> {
    let mut expected_kind = None;
    loop {
        let Some(object) = objects.try_find(&id, buf)? else {
            return Ok(Peeled::Missing(id));
        };
        if let Some(expected) = expected_kind
            && object.kind != expected
        {
            bail!(
                "A tag points to {id} as a {expected}, but it is a {}".corrupted(),
                object.kind
            );
        }
        if object.kind != Kind::Tag {
            return Ok(Peeled::Present(id, object.kind));
        }
        let tag = gix_object::TagRef::from_bytes(object.data, object.object_hash)
            .or_raise(|| message!("Could not decode the tag {id}").corrupted())?;
        tag_ids.push(id);
        expected_kind = Some(tag.target_kind);
        id = tag.target();
    }
}
