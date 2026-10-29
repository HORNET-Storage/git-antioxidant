use std::sync::atomic::AtomicBool;

use gix_error::Result;
use gix_pack::data::delta::{
    chain,
    search::{self, InPack},
};
use gix_utils::progress::Count;

use crate::repack::{Options, plan::Object};

/// What `git pack-objects` knows of an object of the new pack when it decides whether to search a delta for it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Facts {
    pub(crate) kind: gix_object::Kind,
    /// The size of the object's data.
    pub(crate) size: u64,
    /// The size that git compares with `core.bigFileThreshold`: the length of the delta that a combined pack stores the
    /// object as if that delta could be copied before chains were cut, as git compares sizes before it cuts chains,
    /// and otherwise `size`.
    pub(crate) threshold_size: u64,
    /// The combined pack that the object is copied from and whether it stores the object as a delta, or `None` for
    /// loose objects.
    pub(crate) in_pack: Option<InPack>,
}

/// Search deltas for the `objects` of the new pack, whose `facts` and `name_hashes` are known and which are copied as
/// deltas of `bases`, as `git pack-objects` does in its `prepare_pack()`. Each delta that is found makes its base the
/// base of its object in `bases`, and the delta is returned at the index of its object.
///
/// As in git, objects that are copied as deltas aren't searched, nor are objects whose `threshold_size` exceeds
/// [`Options::big_file_threshold`]. The others are given to the search in the order of `objects`, which is git's list
/// order, each with the depth of the longest chain of copied deltas that depends on it, so that its delta doesn't make
/// that chain longer than [`Options::delta_depth`]. `find` reads the objects for all threads of the search, one at a
/// time, as git reads them for its threads, so that the cache of delta bases that it holds serves them all, as git's
/// does.
#[expect(clippy::too_many_arguments)]
pub(crate) fn find<F>(
    objects: &[Object],
    facts: &[Facts],
    name_hashes: &[u32],
    bases: &mut [Option<u32>],
    find: F,
    options: &Options,
    progress: &dyn Count,
    should_interrupt: &AtomicBool,
) -> Result<Vec<Option<search::Delta>>>
where
    F: gix_object::Find + Send,
{
    let dependents = chain::dependents_depths(bases);
    let (mut list, mut indices) = (Vec::new(), Vec::new());
    for (index, ((object, facts), base)) in objects.iter().zip(facts).zip(bases.iter()).enumerate() {
        if base.is_some() || facts.threshold_size > options.big_file_threshold {
            continue;
        }
        list.push(search::Object {
            id: object.id,
            kind: facts.kind,
            size: facts.size,
            name_hash: name_hashes[index],
            preferred_base: false,
            in_pack: facts.in_pack,
            dependents_depth: dependents[index],
        });
        indices.push(index as u32);
    }

    let deltas = search::find_deltas(
        &list,
        find,
        progress,
        should_interrupt,
        search::Options {
            window: options.window,
            window_memory: options.window_memory,
            depth: options.delta_depth.min(search::MAX_DEPTH),
            reuse_delta: true,
            delta_cache_size: options.delta_cache_size,
            delta_cache_limit: options.delta_cache_limit.min(search::MAX_DELTA_CACHE_LIMIT),
            thread_limit: options.thread_limit,
        },
    )?;
    let mut computed: Vec<Option<search::Delta>> = std::iter::repeat_with(|| None).take(objects.len()).collect();
    for (position, delta) in deltas.into_iter().enumerate() {
        let Some(delta) = delta else {
            continue;
        };
        let index = indices[position] as usize;
        bases[index] = Some(indices[delta.base]);
        computed[index] = Some(delta);
    }
    Ok(computed)
}
