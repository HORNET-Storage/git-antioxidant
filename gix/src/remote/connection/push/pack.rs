//! Select the objects a remote lacks and write them as a pack, as `git pack-objects --revs --stdout` does when
//! `pack_objects()` in Git's `send-pack.c` runs it for a push.
use std::{
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};

use gix_hash::ObjectId;
use gix_hashtable::HashSet;
use gix_object::{FindExt, Kind};
use gix_pack::data::output;
use gix_utils::progress::{self, Count, DynNestedProgress, Progress};

use crate::{
    Repository, Result,
    config::{cache::util::ApplyLeniency, tree::Pack},
    error::{OptionExt, ResultExt, bail, cancelled, ensure, message},
};

/// The objects to send to a remote, along with those it has as far as their selection found out.
pub(super) struct Selection {
    /// The objects to send: commits, then annotated tags, then trees and blobs.
    ids: Vec<ObjectId>,
    /// The objects the remote has as far as the selection found out, which deltas of a thin pack may refer to.
    remote_has: HashSet<ObjectId>,
}

/// Select the objects reachable from `tips` but not from `haves`, as `git rev-list --objects <tips> --not <haves>` lists
/// them, leaving out the trees and blobs of the commits at the boundary of what is sent, which the remote has, as
/// `pack-objects` leaves out what is reachable from the edges it marks uninteresting. Objects of `haves` that aren't
/// available locally are ignored, as Git ignores them.
pub(super) fn select(
    repo: &Repository,
    tips: impl IntoIterator<Item = ObjectId>,
    haves: impl IntoIterator<Item = ObjectId>,
    should_interrupt: &AtomicBool,
) -> Result<Selection> {
    // A pack carries the objects as they are stored, not their replacements.
    let mut repo = repo.clone();
    repo.objects.ignore_replacements = true;
    let objects = &repo.objects;
    let mut buf = Vec::new();
    let mut seen = HashSet::default();

    let mut hidden = Vec::new();
    for id in haves {
        match peel(objects, id, |tag| _ = seen.insert(tag), &mut buf)? {
            Some((id, Kind::Commit)) => hidden.push(id),
            Some((id, Kind::Tree)) => add_tree(objects, id, &mut seen, None, &mut buf, should_interrupt)?,
            Some((id, Kind::Blob)) => _ = seen.insert(id),
            Some((_, Kind::Tag)) | None => {}
        }
    }

    let (mut tag_ids, mut tip_commits, mut tree_ids, mut blob_ids) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for id in tips {
        match peel(objects, id, |tag| tag_ids.push(tag), &mut buf)? {
            Some((id, Kind::Commit)) => tip_commits.push(id),
            Some((id, Kind::Tree)) => tree_ids.push(id),
            Some((id, Kind::Blob)) => blob_ids.push(id),
            Some((_, Kind::Tag)) => {}
            None => bail!("The object {id} to push, or an object it points to, is missing".not_found()),
        }
    }

    let mut commit_ids = Vec::new();
    let mut commit_tree_ids = Vec::new();
    let mut parent_ids = Vec::new();
    for info in repo.rev_walk(tip_commits.iter().copied()).with_hidden(hidden).all()? {
        if should_interrupt.load(Ordering::Relaxed) {
            bail!(cancelled("Interrupted while selecting the commits to push"));
        }
        let info = info?;
        commit_tree_ids.push(tree_of(objects, info.id, &mut buf)?);
        commit_ids.push(info.id);
        parent_ids.extend(info.parent_ids.iter().copied());
    }

    // The trees of the commits that the remote has and the sent commits build on, or that are pushed while it has
    // them already, contain what the remote has of the trees that are sent.
    let sent_commits: HashSet<_> = commit_ids.iter().copied().collect();
    for commit_id in parent_ids.into_iter().chain(tip_commits.iter().copied()) {
        if sent_commits.contains(&commit_id) || !seen.insert(commit_id) {
            continue;
        }
        let tree_id = tree_of(objects, commit_id, &mut buf)?;
        add_tree(objects, tree_id, &mut seen, None, &mut buf, should_interrupt)?;
    }
    let remote_has = seen.clone();

    let mut ids = commit_ids;
    for tag_id in tag_ids {
        if seen.insert(tag_id) {
            ids.push(tag_id);
        }
    }
    for tree_id in commit_tree_ids.into_iter().chain(tree_ids) {
        add_tree(objects, tree_id, &mut seen, Some(&mut ids), &mut buf, should_interrupt)?;
    }
    for blob_id in blob_ids {
        if seen.insert(blob_id) {
            ids.push(blob_id);
        }
    }
    Ok(Selection { ids, remote_has })
}

/// The tree of the commit `commit_id`.
fn tree_of(objects: &impl gix_object::Find, commit_id: ObjectId, buf: &mut Vec<u8>) -> Result<ObjectId> {
    objects
        .find_commit_iter(&commit_id, buf)?
        .tree_id()
        .or_raise(|| message!("Could not decode the commit {commit_id}").corrupted())
}

/// Follow the annotated tags starting at `id` to the object they point to, passing each tag to `on_tag`, and return
/// that object along with its kind, or `None` if an object on the way is missing.
fn peel(
    objects: &impl gix_object::Find,
    mut id: ObjectId,
    mut on_tag: impl FnMut(ObjectId),
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
        on_tag(id);
        id = gix_object::TagRefIter::from_bytes(object.data, object.object_hash)
            .target_id()
            .or_raise(|| message!("Could not decode the tag {id}").corrupted())?;
    }
}

/// Add the tree `tree_id` and the trees and blobs it contains to `seen`, without entering trees that are in `seen`
/// already, and append those that weren't to `sent` if it's set. The commits of submodules are left out, as they
/// belong to other repositories.
fn add_tree(
    objects: &impl gix_object::Find,
    tree_id: ObjectId,
    seen: &mut HashSet<ObjectId>,
    mut sent: Option<&mut Vec<ObjectId>>,
    buf: &mut Vec<u8>,
    should_interrupt: &AtomicBool,
) -> Result {
    let mut tree_ids = vec![tree_id];
    while let Some(tree_id) = tree_ids.pop() {
        if !seen.insert(tree_id) {
            continue;
        }
        if should_interrupt.load(Ordering::Relaxed) {
            bail!(cancelled("Interrupted while selecting the trees and blobs to push"));
        }
        if let Some(sent) = sent.as_deref_mut() {
            sent.push(tree_id);
        }
        // Git provides the empty tree whether or not a repository stores it.
        if tree_id.is_empty_tree() {
            continue;
        }
        for entry in objects.find_tree_iter(&tree_id, buf)? {
            let entry = entry.or_raise(|| message!("Could not decode the tree {tree_id}").corrupted())?;
            if entry.mode.is_tree() {
                tree_ids.push(entry.oid.to_owned());
            } else if !entry.mode.is_commit()
                && seen.insert(entry.oid.to_owned())
                && let Some(sent) = sent.as_deref_mut()
            {
                sent.push(entry.oid.to_owned());
            }
        }
    }
    Ok(())
}

/// Write the objects of `selection` from `repo` as a pack to `out`, as `arguments` allow the remote to receive it: with
/// deltas against objects the remote has if `thin` is set and the remote accepts it, and with deltas referring to their
/// base by offset if the remote supports it.
///
/// Objects are copied from the packs of `repo` where possible, a delta if its base is in the pack or, for a thin pack,
/// on the remote. As deltas aren't computed, all other objects are stored whole.
pub(super) fn write(
    repo: &Repository,
    selection: &Selection,
    out: &mut dyn Write,
    arguments: &gix_protocol::push::Arguments,
    thin: bool,
    progress: &mut dyn DynNestedProgress,
    should_interrupt: &AtomicBool,
) -> Result {
    let object_hash = repo.object_hash();
    let compression = repo.pack_compression()?;
    let thread_limit = Pack::THREADS
        .try_into_usize(
            repo.config
                .resolved
                .integer_filter(Pack::THREADS, &mut repo.filter_config_section()),
        )
        .with_leniency(repo.options.lenient_config)
        .or_raise(|| message("The configured pack thread count is invalid"))?;
    let mut db = repo
        .objects
        .clone()
        .into_arc()
        .or_raise(|| message("Could not prepare the object database to write the pack to push"))?
        .into_inner();
    db.prevent_pack_unload();
    db.ignore_replacements = true;

    let Ok(num_entries) = u32::try_from(selection.ids.len()) else {
        bail!(
            "The push needs {} objects, more than a pack can hold".unsupported(),
            selection.ids.len()
        );
    };
    // The pack needs the empty tree even if the repository doesn't store it.
    let empty_tree_id = ObjectId::empty_tree(object_hash);
    let mut ids = selection.ids.clone();
    let empty_tree_entry = match ids.iter().position(|id| *id == empty_tree_id) {
        Some(index) if !gix_pack::Find::contains(&db, &empty_tree_id) => {
            let count = output::Count {
                id: ids.remove(index),
                entry_pack_location: output::count::PackLocation::NotLookedUp,
            };
            let data = gix_object::Data {
                kind: Kind::Tree,
                object_hash,
                data: &[],
            };
            Some(output::Entry::from_data(&count, &data, compression)?)
        }
        _ => None,
    };
    let counts = ids
        .into_iter()
        .map(|id| output::Count {
            id,
            entry_pack_location: output::count::PackLocation::NotLookedUp,
        })
        .collect();

    let in_pack: HashSet<_> = selection.ids.iter().copied().collect();
    let objects = db.clone();
    let ofs_delta = arguments.ofs_delta;
    let mut written_ids = Vec::with_capacity(selection.ids.len());
    let mut buf = Vec::new();
    let entries = gix_parallel::InOrderIter::from(output::entry::iter_from_counts(
        counts,
        db,
        Box::new(progress.add_child("create entries".into())),
        output::entry::iter_from_counts::Options {
            thread_limit,
            allow_thin_pack: thin && arguments.thin_pack,
            compression,
            ..Default::default()
        },
    )?)
    .map(|entries| -> Result<_> {
        let mut entries = entries?;
        for entry in &mut entries {
            // Every object was found while it was selected, but one can disappear before it is written.
            ensure!(
                !entry.is_invalid(),
                message("An object to push disappeared while its pack was written").not_found()
            );
            match entry.kind {
                // Deltas may only refer to objects outside the pack that the remote is known to have.
                output::entry::Kind::DeltaOid { id }
                    if !in_pack.contains(&id) && !selection.remote_has.contains(&id) =>
                {
                    let count = output::Count {
                        id: entry.id,
                        entry_pack_location: output::count::PackLocation::NotLookedUp,
                    };
                    let data = gix_object::Find::try_find(&objects, &entry.id, &mut buf)?.ok_or_raise(|| {
                        message("An object to push disappeared while its pack was written").not_found()
                    })?;
                    *entry = output::Entry::from_data(&count, &data, compression)?;
                }
                // Without `ofs-delta`, deltas refer to their base by its id.
                output::entry::Kind::DeltaRef { object_index } if !ofs_delta => {
                    let id = written_ids
                        .get(object_index)
                        .copied()
                        .ok_or_raise(|| message("A delta of the pack to push refers to an object written after it"))?;
                    entry.kind = output::entry::Kind::DeltaOid { id };
                }
                _ => {}
            }
            written_ids.push(entry.id);
        }
        Ok(entries)
    })
    .chain(empty_tree_entry.map(|entry| Ok(vec![entry])));

    let mut writing = progress.add_child("write pack".into());
    writing.init(None, progress::bytes());
    let mut pack =
        output::bytes::FromEntriesIter::new(entries, out, num_entries, gix_pack::data::Version::V2, object_hash);
    for bytes in pack.by_ref() {
        if should_interrupt.load(Ordering::Relaxed) {
            bail!(cancelled("Interrupted while writing the pack to push"));
        }
        writing.inc_by(bytes? as usize);
    }
    Ok(())
}
