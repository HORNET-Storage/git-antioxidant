//! Combine the packs of an object database into a geometric progression and pack its loose objects, as
//! `git repack -d -l --geometric=<factor>` does for the `geometric-repack` task of `git maintenance`.
//!
//! Every fetch, push or bundle import adds a pack, and readers look for an object in one pack after the other, so an
//! object database whose packs are never combined becomes slower with every pack, and a `gix-odb` handle can't open
//! more than 32,767 pack indices. [`repack()`] keeps the amount of packs logarithmic in the amount of objects: it orders
//! the packs by their amount of objects and combines the smallest ones until the others form a geometric progression,
//! each holding at least `split_factor` times as many objects as the next smaller one and the smallest of them at
//! least `split_factor` times as many as the combined packs together. The objects of the combined packs and the loose
//! objects go into one new pack, and then the combined packs and every loose object that a pack holds are removed.
//! [`needs_repack()`] tells whether a repack would combine packs or there are many loose objects, as the auto
//! condition of the task does.
//!
//! The new pack is written into a directory inside the pack directory, where readers don't look for packs, and indexed
//! with `gix-pack`, which computes the id of each object from its data. Only once its index lists exactly the planned
//! objects is it moved into the pack directory, and only then are packs and loose objects removed, so that every object
//! remains available. A reader that looks up an object while its pack is removed must then look for new packs, as
//! `gix-odb` does. Only one repack may run in an object database at a time, which callers ensure with a lock, like the
//! `objects/maintenance.lock` that `git maintenance` takes, and so a repack first removes the directories that repacks
//! which ended early left in the pack directory.
//!
//! ### Deviation
//!
//! * Objects aren't selected by reachability, and no cruft pack is written. Where every pack would be combined,
//!   `git maintenance` repacks the reachable objects into one pack and all others into a cruft pack, while here these
//!   packs are combined like any others, so no object is ever removed.
//! * No deltas are computed. Like `git pack-objects`, a delta is copied from its pack if its base goes into the new
//!   pack as well, unless that makes a cycle or a chain of more than `delta_depth` deltas; all other objects are stored
//!   whole. The new pack holds the objects in the order they were selected in, each delta after its base.
//! * Alternates aren't read. Objects that their packs hold go into the new pack nevertheless, and a loose object is
//!   removed only if a local pack holds it.
//! * Cruft packs are left as they are. With `repack.midxMustContainCruft` set to false, git also copies the objects of
//!   cruft packs that the objects of the combined packs refer to, directly or not, into the new pack, while here they
//!   remain only in their cruft pack.
//! * Promisor packs aren't combined. Git combines them into a new promisor pack until they form a geometric progression
//!   of their own.
//! * No reverse index, multi-pack-index, bitmap or `objects/info/packs` is written. A multi-pack-index that lists a
//!   removed pack or that can't be read is removed with its bitmaps and reverse indices, and an incremental one
//!   whenever packs are removed, as `gix-odb` doesn't read it.
//! * Fanout directories that no loose object remains in aren't removed, as a writer of loose objects may be about to use
//!   one.
//! * Loose objects that can't be read or whose data doesn't hash to their name are left as they are and reported.
//! * Packs with the same amount of objects are ordered by their path, where git leaves the order unspecified.
#![deny(unsafe_code, missing_docs)]

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use gix_error::{Result, bail, cancelled};
use gix_hash::ObjectId;
use gix_utils::progress::DynNestedProgress;

mod loose;
mod plan;
mod remove;
mod survey;
mod write;

pub use loose::too_many_loose_objects;
pub use survey::{Pack, Survey};

/// Options for [`repack()`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// The smallest packs are combined until each of the others holds at least this many times as many objects as the
    /// next smaller one, and the smallest of them this many times as many as the combined packs together, as with
    /// `git repack --geometric=<factor>` and `maintenance.geometric-repack.splitFactor`. Below 2, no packs are
    /// combined, and only loose objects are packed.
    pub split_factor: u32,
    /// The longest chain of deltas that is copied into the new pack, like `pack.depth`.
    pub delta_depth: u32,
    /// The amount of threads to use at most to index the new pack, or `None` to use as many as there are logical cores.
    pub thread_limit: Option<usize>,
    /// The compression level of the objects that aren't copied from a pack.
    pub compression: gix_zlib::Compression,
}

impl Default for Options {
    /// The defaults of `git maintenance`: a split factor of 2, a delta depth of 50 and git's compression level for packs.
    fn default() -> Self {
        Options {
            split_factor: 2,
            delta_depth: 50,
            thread_limit: None,
            compression: gix_zlib::Compression::DEFAULT,
        }
    }
}

/// What [`repack()`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The pack that was written, or `None` if no object needed to be packed.
    pub pack: Option<Written>,
    /// The indices of the packs that were combined and then removed.
    pub removed_packs: Vec<PathBuf>,
    /// The indices of the packs of the progression that were left as they are.
    pub retained_packs: Vec<PathBuf>,
    /// The amount of loose objects that were removed because a pack holds them.
    pub removed_loose_objects: usize,
    /// The loose objects that couldn't be read or whose data doesn't hash to their name, which were left as they are.
    pub skipped_loose_objects: Vec<ObjectId>,
}

/// The pack that [`repack()`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The path of its index, which ends in `.idx`.
    pub index_path: PathBuf,
    /// The path of its data, which ends in `.pack`.
    pub data_path: PathBuf,
    /// The amount of objects it holds.
    pub num_objects: u32,
}

/// Combine the smallest packs of the object database in `objects_dir` and its loose objects into a new pack, then remove
/// the combined packs and the loose objects that a pack holds, as `git repack -d -l --geometric=<factor>` does.
///
/// * `object_hash` is the hash of the object ids of the object database.
/// * `progress` shows how many objects were selected for the new pack, and how many bytes were written and indexed.
/// * `should_interrupt` is checked regularly, and once it is set, the repack stops with an error. Every object remains
///   available, as packs and loose objects are removed only once the new pack is in the pack directory.
///
/// Packs that a `.keep` file protects, promisor packs and cruft packs aren't combined or removed. Objects that the packs
/// remaining in the progression or kept packs hold are left out of the new pack, while objects of promisor and cruft
/// packs aren't. If no object needs to go into the new pack, none is written, but the combined packs are removed
/// nevertheless, as the remaining packs hold all their objects.
pub fn repack(
    objects_dir: &Path,
    object_hash: gix_hash::Kind,
    progress: &mut dyn DynNestedProgress,
    should_interrupt: &AtomicBool,
    options: Options,
) -> Result<Outcome> {
    write::remove_left_over_staging(&objects_dir.join("pack"));
    let survey = Survey::at(objects_dir, object_hash)?;
    let geometric: Vec<&Pack> = survey.geometric().collect();
    let (combined, retained) = geometric.split_at(survey.split(options.split_factor));
    let excluded: Vec<&Pack> = retained
        .iter()
        .copied()
        .chain(survey.packs.iter().filter(|pack| pack.keep))
        .collect();

    let mut plan = plan::Plan::new(
        objects_dir,
        object_hash,
        combined,
        &excluded,
        options.delta_depth,
        progress,
        should_interrupt,
    )?;
    let pack = if plan.objects.is_empty() {
        None
    } else {
        Some(write::write(
            objects_dir,
            object_hash,
            &plan,
            progress,
            should_interrupt,
            &options,
        )?)
    };
    let skipped_loose_objects = std::mem::take(&mut plan.skipped_loose_objects);
    // Close the combined packs before they are removed.
    drop(plan);
    if should_interrupt.load(Ordering::Relaxed) {
        bail!(cancelled("Interrupted before the combined packs were removed"));
    }

    let removed_packs = remove::remove_packs(&objects_dir.join("pack"), combined, pack.as_ref())?;
    let removed_loose_objects = loose::prune(objects_dir, object_hash, should_interrupt)?;
    Ok(Outcome {
        pack,
        removed_packs,
        retained_packs: retained.iter().map(|pack| pack.index_path.clone()).collect(),
        removed_loose_objects,
        skipped_loose_objects,
    })
}

/// Whether [`repack()`] with `split_factor` would combine packs of the object database in `objects_dir`, or it has more
/// than `auto` loose objects by the estimate of [`too_many_loose_objects()`], as the auto condition of the
/// `geometric-repack` task of `git maintenance` decides with `maintenance.geometric-repack.auto` set to `auto`.
///
/// If `auto` is 0, a repack is never needed, and if it's negative, a repack is always needed.
pub fn needs_repack(objects_dir: &Path, object_hash: gix_hash::Kind, split_factor: u32, auto: i64) -> Result<bool> {
    Ok(match u64::try_from(auto) {
        Err(_) => true,
        Ok(0) => false,
        Ok(limit) => {
            Survey::at(objects_dir, object_hash)?.split(split_factor) != 0
                || too_many_loose_objects(objects_dir, object_hash, limit)
        }
    })
}
