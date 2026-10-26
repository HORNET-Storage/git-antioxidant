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
//! ### Deltas
//!
//! The new pack stores objects as deltas as `git pack-objects --stdin-packs`, which `git repack` runs, stores them:
//!
//! * The objects are listed as git lists them: those of the combined packs, the pack modified last first and each in
//!   the order of its entries, and then the loose objects, by fanout directory and within each in the order of the file
//!   system.
//! * An object that a combined pack stores as a delta remains a delta, with the bytes of that pack, if its base goes
//!   into the new pack as well, unless that makes a cycle or a chain of more than [`delta_depth`](Options::delta_depth)
//!   deltas.
//! * For the other objects, [`find_deltas()`](gix_pack::data::delta::search::find_deltas()) searches deltas, given the
//!   objects in the order of the list without those larger than [`big_file_threshold`](Options::big_file_threshold),
//!   each with the name hash of the path at which git's walk finds it first. The walk goes through the history of the
//!   commits of the combined packs and of the loose commits, the newest commit first, and then through the trees of the
//!   commits that the packs which remain don't hold.
//! * The objects are written in the order of the list, each delta after its base. The deltas that the search computed
//!   are compressed at [`compression`](Options::compression), like the objects that are stored whole and not copied.
//!
//! With a single [thread](Options::thread_limit), the objects that the pack of
//! `git -c pack.threads=1 repack -d -l --geometric=<factor>` stores as deltas are thus stored as deltas of the same
//! bases, with the same deltas. With several threads, which deltas are found depends on the speed of the threads, as
//! in git. Like git's, the threads read the objects one at a time and through one cache of delta bases, so that
//! reading paces them as it paces git's.
//!
//! ### Deviation
//!
//! * Objects aren't selected by reachability, and no cruft pack is written. Where every pack would be combined,
//!   `git maintenance` repacks the reachable objects into one pack and all others into a cruft pack, while here these
//!   packs are combined like any others, so no object is ever removed.
//! * Refs aren't read, as [`repack()`] works on the object database alone. From the first object of the list that a tag
//!   points to on, git writes the objects that tags point to first, then the commits and tags, the trees, and the other
//!   objects, while here all objects are written in the order of the list, as git writes them if no tag points to an
//!   object of the new pack.
//! * The `delta` attribute isn't read, as [`repack()`] works on the object database alone, without the files and the
//!   configuration that git reads attributes from. The objects at paths that unset it are searched for deltas like any
//!   others.
//! * Delta islands, name hash version 2 and the path walk aren't supported. Git's geometric repack uses them only when
//!   they are asked for (`repack.useDeltaIslands`, `--name-hash-version=2`, `--path-walk`).
//! * The objects and deltas that aren't copied are compressed with `gix-zlib`, which gives other bytes than the zlib
//!   that git uses, so that their entries can be a few bytes longer or shorter than git's.
//! * The cache of delta bases holds 96 MiB, the default of `core.deltaBaseCacheLimit`, which isn't read. Its size
//!   changes how fast objects are read, and so which deltas several threads find, but not those of a single thread.
//! * Alternates aren't read. Objects that their packs hold go into the new pack nevertheless, and a loose object is
//!   removed only if a local pack holds it.
//! * Cruft packs are left as they are. With `repack.midxMustContainCruft` set to false, git also copies the objects of
//!   cruft packs that the objects of the combined packs refer to, directly or not, into the new pack, while here they
//!   remain only in their cruft pack. A loose object that a cruft pack holds as well is read from its file, where git
//!   copies it from that pack.
//! * Promisor packs aren't combined. Git combines them into a new promisor pack until they form a geometric progression
//!   of their own.
//! * No reverse index, multi-pack-index, bitmap or `objects/info/packs` is written. A multi-pack-index that lists a
//!   removed pack or that can't be read is removed with its bitmaps and reverse indices, and an incremental one
//!   whenever packs are removed, as `gix-odb` doesn't read it.
//! * Fanout directories that no loose object remains in aren't removed, as a writer of loose objects may be about to use
//!   one.
//! * Loose objects that can't be read or whose data doesn't hash to their name are left as they are and reported.
//! * Packs with the same amount of objects are ordered by their path, where git leaves the order unspecified, and so
//!   are combined packs that were modified in the same second.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use gix_error::{Result, bail, cancelled};
use gix_hash::ObjectId;
use gix_utils::progress::DynNestedProgress;

mod deltas;
mod loose;
mod names;
mod plan;
mod remove;
mod survey;
mod write;

pub use loose::too_many_loose_objects;
pub use survey::{Pack, Survey};

/// Options for [`repack()`], with the meanings of the configuration of `git repack` that each of them names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// The smallest packs are combined until each of the others holds at least this many times as many objects as the
    /// next smaller one, and the smallest of them this many times as many as the combined packs together, as with
    /// `git repack --geometric=<factor>` and `maintenance.geometric-repack.splitFactor`. Below 2, no packs are
    /// combined, and only loose objects are packed.
    pub split_factor: u32,
    /// The longest chain of deltas in the new pack (`pack.depth`), which git limits to 4095. It applies to the chains
    /// of deltas that are copied, which are cut to this many deltas, and to the search, which computes no delta that
    /// makes a chain longer. With 0, all objects are stored whole.
    pub delta_depth: u32,
    /// How many of the objects before an object in the order of the search are tried as its base (`pack.window`). With
    /// 0, no deltas are computed, and only the deltas of the combined packs are copied.
    pub window: usize,
    /// The most bytes that the objects in the window of each thread of the search may take (`pack.windowMemory`), or 0
    /// for no limit.
    pub window_memory: u64,
    /// The most bytes of computed deltas that are kept in memory until they are written (`pack.deltaCacheSize`), or 0
    /// for no limit. The deltas that aren't kept are computed again when they are written, which gives the same bytes.
    pub delta_cache_size: u64,
    /// Computed deltas shorter than this many bytes are kept while they fit into `delta_cache_size`
    /// (`pack.deltaCacheLimit`), which git limits to 1 MiB less one byte. Longer ones are kept only if they are short
    /// compared to their objects.
    pub delta_cache_limit: u64,
    /// Objects larger than this many bytes aren't searched for deltas (`core.bigFileThreshold`). For an object that a
    /// combined pack stores as a delta of an object of the new pack, the length of that delta is compared, as in git.
    pub big_file_threshold: u64,
    /// The amount of threads to use at most to search deltas and to index the new pack (`pack.threads`), or `None` to
    /// use as many as there are logical cores. Without the `parallel` feature of `gix-parallel`, one thread is used.
    pub thread_limit: Option<usize>,
    /// The compression level of the objects that aren't copied from a pack and of the computed deltas
    /// (`pack.compression`).
    pub compression: gix_zlib::Compression,
}

impl Default for Options {
    /// The defaults of `git maintenance` and `git repack`: a split factor of 2, a delta depth of 50, a window of 10
    /// objects without a memory limit, up to 256 MiB of kept deltas with a limit of 1000 bytes for short ones, a
    /// threshold of 512 MiB for big files, as many threads as there are logical cores, and git's compression level for
    /// packs.
    fn default() -> Self {
        Options {
            split_factor: 2,
            delta_depth: 50,
            window: 10,
            window_memory: 0,
            delta_cache_size: 256 * 1024 * 1024,
            delta_cache_limit: 1000,
            big_file_threshold: 512 * 1024 * 1024,
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
/// * `progress` shows how many objects were selected for the new pack and tried as deltas, and how many bytes were
///   written and indexed.
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
    let kept: Vec<&Pack> = survey.packs.iter().filter(|pack| pack.keep).collect();

    let mut plan = plan::Plan::new(
        objects_dir,
        object_hash,
        combined,
        retained,
        &kept,
        &options,
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
