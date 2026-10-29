//! Count the objects a remote lacks and write them as a pack, as `git pack-objects --revs --stdout` does when
//! `pack_objects()` in Git's `send-pack.c` runs it for a push, with `--thin` for a thin pack and `--delta-base-offset`
//! where the remote supports `ofs-delta`.
use std::{
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};

use gix_hash::ObjectId;
use gix_object::{Kind, WriteTo};
use gix_pack::data::output::{self, entry::iter_from_counts::DeltaSearch};
use gix_utils::progress::{self, Count, DynNestedProgress, Progress};

use crate::{
    Repository, Result,
    config::{cache::util::ApplyLeniency, tree::Pack},
    error::{OptionExt, ResultExt, bail, cancelled, ensure, message},
};

/// The objects to send to a remote, as `git pack-objects --revs` counts them, and how to search their deltas.
pub(super) struct Selection {
    /// The objects to send and, for a thin pack, the preferred bases among them, in the order in which Git lists them.
    counts: Vec<output::Count>,
    /// Whether the pack is thin, so that deltas can refer to the preferred bases, which the remote has.
    thin: bool,
    /// The options of the delta search.
    search: DeltaSearch,
}

/// Count the objects reachable from `tips` but not from `haves`, as `git pack-objects --revs` counts them for
/// `<tips> --not <haves>` with [`revisions::objects()`](output::count::revisions::objects()), along with the preferred
/// bases of a thin pack if `thin` is set. Objects of `haves` that aren't available locally are ignored, as Git ignores
/// them, and the commits that the shallow file of `repo` lists have no parents, as for Git. The objects that the tags of
/// `repo` point to or peel to are [tagged](output::Count::tagged), as Git's `mark_tagged()` marks them.
pub(super) fn select(
    repo: &Repository,
    tips: impl IntoIterator<Item = ObjectId>,
    haves: impl IntoIterator<Item = ObjectId>,
    thin: bool,
    should_interrupt: &AtomicBool,
) -> Result<Selection> {
    // A pack carries the objects as they are stored, not their replacements.
    let mut repo = repo.clone();
    repo.objects.ignore_replacements = true;
    let search = delta_search(&repo)?;
    let objects = Objects {
        objects: &repo.objects,
        shallow: repo.shallow_commits()?,
    };
    let mut outcome = output::count::revisions::objects(
        &objects,
        tips,
        haves,
        &progress::Discard,
        should_interrupt,
        output::count::revisions::Options {
            thin,
            window: search.window,
        },
    )?;
    mark_tagged(&repo, &mut outcome.counts)?;
    Ok(Selection {
        counts: outcome.counts,
        thin,
        search,
    })
}

/// The options of the delta search of a push: the defaults of `git pack-objects`, or what the `pack.window`, `pack.depth`,
/// `pack.windowMemory`, `pack.deltaCacheSize` and `pack.deltaCacheLimit` keys of `repo` configure, and its
/// `core.bigFileThreshold`.
fn delta_search(repo: &Repository) -> Result<DeltaSearch> {
    let defaults = DeltaSearch::default();
    let (resolved, lenient) = (&repo.config.resolved, repo.options.lenient_config);
    Ok(DeltaSearch {
        window: Pack::WINDOW
            .try_into_window(resolved.integer_filter(Pack::WINDOW, &mut repo.filter_config_section()))
            .with_leniency(lenient)?
            .unwrap_or(defaults.window),
        depth: Pack::DEPTH
            .try_into_depth(resolved.integer_filter(Pack::DEPTH, &mut repo.filter_config_section()))
            .with_leniency(lenient)?
            .unwrap_or(defaults.depth),
        window_memory: Pack::WINDOW_MEMORY
            .try_into_u64(resolved.integer_filter(Pack::WINDOW_MEMORY, &mut repo.filter_config_section()))
            .with_leniency(lenient)?
            .unwrap_or(defaults.window_memory),
        delta_cache_size: Pack::DELTA_CACHE_SIZE
            .try_into_delta_cache_size(
                resolved.integer_filter(Pack::DELTA_CACHE_SIZE, &mut repo.filter_config_section()),
            )
            .with_leniency(lenient)?
            .unwrap_or(defaults.delta_cache_size),
        delta_cache_limit: Pack::DELTA_CACHE_LIMIT
            .try_into_delta_cache_limit(
                resolved.integer_filter(Pack::DELTA_CACHE_LIMIT, &mut repo.filter_config_section()),
            )
            .with_leniency(lenient)?
            .unwrap_or(defaults.delta_cache_limit),
        big_file_threshold: repo.big_file_threshold()?,
    })
}

/// Mark the `counts` of the objects that the references in `refs/tags/` of `repo` point to, after following symbolic
/// references, or peel to as tagged, as Git's `mark_tagged()` marks them for its write order. A reference that can't
/// be followed or peeled marks nothing, as Git skips it.
fn mark_tagged(repo: &Repository, counts: &mut [output::Count]) -> Result {
    let positions: gix_hashtable::HashMap<ObjectId, usize> = counts
        .iter()
        .enumerate()
        .map(|(index, count)| (count.id, index))
        .collect();
    let mut mark = |id: ObjectId| {
        if let Some(&index) = positions.get(&id) {
            counts[index].tagged = true;
        }
    };
    let references = repo.references()?;
    for reference in references.tags()? {
        let mut reference = reference?;
        if let Ok(id) = reference.follow_to_object() {
            mark(id.detach());
        }
        if let Ok(id) = reference.peel_to_id() {
            mark(id.detach());
        }
    }
    Ok(())
}

/// The objects of a repository as Git's revision walk reads them, with the commits of its shallow file without parents.
struct Objects<'a> {
    objects: &'a crate::OdbHandle,
    shallow: Option<crate::shallow::Commits>,
}

impl gix_object::Find for Objects<'_> {
    fn try_find<'a>(&self, id: &gix_hash::oid, buffer: &'a mut Vec<u8>) -> Result<Option<gix_object::Data<'a>>> {
        let is_shallow = self
            .shallow
            .as_ref()
            .is_some_and(|commits| commits.binary_search(&id.to_owned()).is_ok());
        let Some(data) = gix_object::Find::try_find(self.objects, id, buffer)? else {
            return Ok(None);
        };
        let (kind, object_hash) = (data.kind, data.object_hash);
        if is_shallow && kind == Kind::Commit {
            let mut commit = gix_object::CommitRef::from_bytes(data.data, object_hash)?;
            commit.parents.clear();
            let mut shallow_data = Vec::new();
            commit.write_to(&mut shallow_data).or_error()?;
            drop(commit);
            *buffer = shallow_data;
        }
        Ok(Some(gix_object::Data {
            kind,
            object_hash,
            data: buffer,
        }))
    }
}

/// Write the objects of `selection` from `repo` as a pack to `out`, as `arguments` allow the remote to receive it, as
/// `git pack-objects --revs --stdout` writes it with `--thin` if the selection is thin, and with `--delta-base-offset`
/// if the remote supports `ofs-delta`.
///
/// The pack is written with [`Mode::PackCopyAndDeltaSearch`](output::entry::iter_from_counts::Mode::PackCopyAndDeltaSearch)
/// and the options of the selection, in up to `pack.threads` threads: objects and deltas are copied from the packs of
/// `repo` where the base of the delta is in the pack or, for a thin pack, a preferred base, and the deltas of the other
/// objects are searched as `git pack-objects` searches them. A delta refers to a base in the pack by its offset, unless
/// the remote doesn't support `ofs-delta`, and to a preferred base by its id.
///
/// ### Deviation
///
/// * If `repo` doesn't store the empty tree, it is written after all other objects, while Git writes it in its place,
///   as it provides it like a stored object.
pub(super) fn write(
    repo: &Repository,
    selection: &Selection,
    out: &mut dyn Write,
    arguments: &gix_protocol::push::Arguments,
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

    let num_entries = selection.counts.iter().filter(|count| !count.preferred_base).count();
    let Ok(num_entries) = u32::try_from(num_entries) else {
        bail!(
            "The push needs {} objects, more than a pack can hold".unsupported(),
            num_entries
        );
    };
    // The pack needs the empty tree even if the repository doesn't store it.
    let empty_tree_id = ObjectId::empty_tree(object_hash);
    let mut counts = selection.counts.clone();
    let empty_tree_entry = match counts
        .iter()
        .position(|count| count.id == empty_tree_id && !count.preferred_base)
    {
        Some(index) if !gix_pack::Find::contains(&db, &empty_tree_id) => {
            let count = counts.remove(index);
            let data = gix_object::Data {
                kind: Kind::Tree,
                object_hash,
                data: &[],
            };
            Some(output::Entry::from_data(&count, &data, compression)?)
        }
        _ => None,
    };

    let ofs_delta = arguments.ofs_delta;
    let mut written_ids = Vec::with_capacity(counts.len());
    let entries = gix_parallel::InOrderIter::from(output::entry::iter_from_counts(
        counts,
        db,
        Box::new(progress.add_child("create entries".into())),
        should_interrupt,
        output::entry::iter_from_counts::Options {
            thread_limit,
            mode: output::entry::iter_from_counts::Mode::PackCopyAndDeltaSearch(selection.search),
            allow_thin_pack: selection.thin,
            compression,
            ..Default::default()
        },
    )?)
    .map(|entries| -> Result<_> {
        let mut entries = entries?;
        for entry in &mut entries {
            // Every object was found while it was counted, but one can disappear before it is written.
            ensure!(
                !entry.is_invalid(),
                message("An object to push disappeared while its pack was written").not_found()
            );
            // Without `ofs-delta`, deltas refer to their base by its id.
            if let output::entry::Kind::DeltaRef { object_index } = entry.kind
                && !ofs_delta
            {
                let id = written_ids
                    .get(object_index)
                    .copied()
                    .ok_or_raise(|| message("A delta of the pack to push refers to an object written after it"))?;
                entry.kind = output::entry::Kind::DeltaOid { id };
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
