//! The entries that [`iter_from_counts()`](super::iter_from_counts()) writes with
//! [`Mode::PackCopyAndDeltaSearch`](super::iter_from_counts::Mode::PackCopyAndDeltaSearch): the deltas that it copies from
//! packs, the deltas that it searches for the other objects and the order of the entries, as `git pack-objects` plans them
//! in its `prepare_pack()` and orders them in its `compute_write_order()`.
use std::{
    collections::HashMap,
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

use gix_error::{OptionExt, Result, ResultExt, bail, cancelled, ensure, message, not_found};
use gix_hash::{ObjectId, oid};
use gix_object::Kind;
use gix_parallel as parallel;
use gix_utils::progress::{Count, DynNestedProgress, Progress};

use super::{
    DeltaBase, compressed_data,
    iter_from_counts::{DeltaSearch, Outcome, ProgressId},
};
use crate::data::{
    self,
    delta::{chain, search},
    entry::Header,
    output,
};

/// With more counts than this, their objects are examined in several threads.
const COUNTS_FOR_THREADS: usize = 4_000;

/// How the object of a count is written.
pub(super) enum How {
    /// It is a preferred base, which isn't written.
    PreferredBase,
    /// The object database doesn't have it, so it is written as an [invalid entry](output::Entry::invalid()).
    Missing,
    /// It is copied from its pack, which stores it whole.
    CopyWhole,
    /// It is copied from its pack, which stores it as a delta of the object of the count at `base`.
    CopyDelta { base: u32 },
    /// It is read and stored whole.
    Whole,
    /// It is stored as the `delta` that the search found against the object of the count at `base`.
    Computed { base: u32, delta: search::Delta },
}

/// The entries of a pack and their order.
pub(super) struct Plan {
    /// The index of the count of each entry, in the order in which the entries are written.
    pub(super) order: Vec<u32>,
    /// How the object of each count is written.
    how: Vec<How>,
    /// The position of the entry of each count in `order`, for the counts that are written.
    positions: Vec<u32>,
}

impl Plan {
    /// Plan the entries of `counts`, whose pack locations are resolved, as `git pack-objects` does, with the objects that
    /// `db` has:
    ///
    /// * A delta that a pack stores is copied if its base is among the `counts`, as Git's `check_object()` copies deltas
    ///   whose base is in its packing list, unless the delta refers to its base by id, which isn't copied. Then the chains
    ///   of copied deltas are cut where they are longer than [`DeltaSearch::depth`], as Git's `break_delta_chains()` does.
    /// * The other objects that aren't larger than [`DeltaSearch::big_file_threshold`] are searched for deltas in the order
    ///   of `counts` with [`find_deltas()`](search::find_deltas()), along with the preferred bases, in up to
    ///   `thread_limit` threads. For an object that is copied as a delta, the threshold applies to the length of its
    ///   delta, as Git compares it before it cuts chains.
    /// * The objects are written in the order of [`chain::write_order()`].
    ///
    /// The objects are examined in chunks of `chunk_size` counts, and `progress` receives the progress of the examination
    /// and of the search. Once `should_interrupt` is set, planning stops with an error.
    #[expect(clippy::too_many_arguments)]
    pub(super) fn new<F>(
        counts: &[output::Count],
        db: &F,
        options: DeltaSearch,
        version: data::Version,
        chunk_size: usize,
        thread_limit: Option<usize>,
        progress: &mut dyn DynNestedProgress,
        should_interrupt: &AtomicBool,
    ) -> Result<Self>
    where
        F: crate::Find + Send + Clone,
    {
        let len = counts.len();
        ensure!(
            u32::try_from(len).is_ok_and(|len| len < u32::MAX),
            "A pack can't hold {} objects".unsupported(),
            len
        );

        let mut examining = progress.add_child_with_id("examining".into(), ProgressId::ExamineCounts.into());
        examining.init(Some(len), gix_utils::progress::count("objects"));
        let mut details = vec![Details::default(); len];
        {
            let db = db.clone();
            let examined = examining.counter();
            parallel::in_parallel_if(
                || len > COUNTS_FOR_THREADS,
                details
                    .chunks_mut(chunk_size.max(1))
                    .zip(counts.chunks(chunk_size.max(1))),
                thread_limit,
                |_| Vec::new(),
                move |(details, counts): (&mut [Details], &[output::Count]), buf: &mut Vec<u8>| -> Result {
                    for (details, count) in details.iter_mut().zip(counts) {
                        *details = examine(&db, count, version, buf)?;
                    }
                    examined.fetch_add(counts.len(), Ordering::Relaxed);
                    Ok(())
                },
                parallel::IdentityWithResult::<(), gix_error::Error>::default(),
            )?;
        }
        check_interrupt(should_interrupt)?;

        let mut bases = copied_bases(counts, &details, db)?;
        // Like Git, compare the length of a delta that can be copied with `core.bigFileThreshold` before chains are cut.
        let threshold_sizes: Vec<Option<u64>> = bases
            .iter()
            .zip(&details)
            .map(|(base, details)| {
                base.and_then(|_| details.stored.and_then(|stored| stored.delta))
                    .map(|delta| delta.size)
            })
            .collect();
        chain::break_chains(&mut bases, options.depth.min(search::MAX_DEPTH));

        // The objects that their packs store as deltas that aren't copied are read, as Git reads them.
        let mut reads: Vec<(u32, Option<(Kind, u64)>)> = (0..len)
            .filter(|&index| details[index].kind.is_none() && details[index].stored.is_some() && bases[index].is_none())
            .map(|index| (index as u32, None))
            .collect();
        {
            let db = db.clone();
            let reads_len = reads.len();
            parallel::in_parallel_if(
                || reads_len > COUNTS_FOR_THREADS,
                reads.chunks_mut(chunk_size.max(1)),
                thread_limit,
                |_| Vec::new(),
                move |reads: &mut [(u32, Option<(Kind, u64)>)], buf: &mut Vec<u8>| -> Result {
                    for (index, found) in reads {
                        *found = db
                            .try_find(&counts[*index as usize].id, buf)
                            .or_raise(|| message("Could not find object while generating pack"))?
                            .map(|(object, _location)| (object.kind, object.data.len() as u64));
                    }
                    Ok(())
                },
                parallel::IdentityWithResult::<(), gix_error::Error>::default(),
            )?;
        }
        for (index, found) in reads {
            let details = &mut details[index as usize];
            match found {
                Some((kind, size)) => {
                    details.kind = Some(kind);
                    details.size = size;
                }
                None => details.stored = None,
            }
        }
        examining.set(len);
        check_interrupt(should_interrupt)?;

        let dependents = chain::dependents_depths(&bases);
        let (mut list, mut indices) = (Vec::new(), Vec::new());
        for (index, (count, details)) in counts.iter().zip(&details).enumerate() {
            let Some(kind) = details.kind.filter(|_| bases[index].is_none()) else {
                continue;
            };
            if threshold_sizes[index].unwrap_or(details.size) > options.big_file_threshold {
                continue;
            }
            list.push(search::Object {
                id: count.id,
                kind,
                size: details.size,
                name_hash: count.name_hash,
                preferred_base: count.preferred_base,
                in_pack: details.stored.map(|stored| search::InPack {
                    pack_id: stored.pack_id,
                    is_delta: stored.delta.is_some(),
                }),
                dependents_depth: dependents[index],
            });
            indices.push(index as u32);
        }
        let mut searching = progress.add_child_with_id("finding deltas".into(), ProgressId::FindDeltas.into());
        searching.init(None, gix_utils::progress::count("objects"));
        let found = search::find_deltas(
            &list,
            Objects(db.clone()),
            &searching,
            should_interrupt,
            search::Options {
                window: options.window,
                window_memory: options.window_memory,
                depth: options.depth.min(search::MAX_DEPTH),
                reuse_delta: true,
                delta_cache_size: options.delta_cache_size,
                delta_cache_limit: options.delta_cache_limit.min(search::MAX_DELTA_CACHE_LIMIT),
                thread_limit,
            },
        )?;
        drop(list);
        let mut computed: Vec<Option<search::Delta>> = (0..len).map(|_| None).collect();
        for (position, delta) in found.into_iter().enumerate() {
            let Some(delta) = delta else {
                continue;
            };
            let index = indices[position] as usize;
            bases[index] = Some(indices[delta.base]);
            computed[index] = Some(delta);
        }

        let objects: Vec<chain::Object> = counts
            .iter()
            .zip(&details)
            .zip(&bases)
            .map(|((count, details), &base)| chain::Object {
                base,
                kind: details.kind,
                tagged: count.tagged,
                preferred_base: count.preferred_base,
            })
            .collect();
        let order = chain::write_order(&objects);
        drop(objects);
        let mut positions = vec![u32::MAX; len];
        for (position, &index) in order.iter().enumerate() {
            positions[index as usize] = position as u32;
        }
        let how = counts
            .iter()
            .zip(details)
            .zip(bases.into_iter().zip(computed))
            .map(|((count, details), (base, computed))| match (base, computed) {
                _ if count.preferred_base => How::PreferredBase,
                (Some(base), Some(delta)) => How::Computed { base, delta },
                (Some(base), None) => How::CopyDelta { base },
                (None, _) if details.kind.is_none() => How::Missing,
                (None, _) if details.stored.is_some_and(|stored| stored.delta.is_none()) => How::CopyWhole,
                (None, _) => How::Whole,
            })
            .collect();
        Ok(Plan { order, how, positions })
    }

    /// The entries at `range` of the [order](Self::order) of the entries of `counts`, read from `db` and compressed with
    /// `compression` where they aren't copied, with copied entries in `version`. `stats` counts how they were made.
    #[expect(clippy::too_many_arguments)]
    pub(super) fn entries(
        &self,
        range: Range<usize>,
        counts: &[output::Count],
        db: &impl crate::Find,
        version: data::Version,
        compression: gix_zlib::Compression,
        buf: &mut Vec<u8>,
        stats: &mut Outcome,
    ) -> Result<Vec<output::Entry>> {
        let base_of = |base: u32| {
            let base_count = &counts[base as usize];
            if base_count.preferred_base {
                DeltaBase::Id(base_count.id)
            } else {
                DeltaBase::Index(self.positions[base as usize] as usize)
            }
        };
        let mut out = Vec::with_capacity(range.len());
        for &index in &self.order[range] {
            let count = &counts[index as usize];
            let entry = match &self.how[index as usize] {
                How::PreferredBase => continue,
                How::Missing => {
                    stats.missing_objects += 1;
                    output::Entry::invalid()
                }
                How::Whole => whole(count, db, buf, compression, stats)?,
                How::CopyWhole => match copy(count, db, version, None)? {
                    Some(entry) => {
                        stats.objects_copied_from_pack += 1;
                        entry
                    }
                    None => whole(count, db, buf, compression, stats)?,
                },
                How::CopyDelta { base } => match copy(count, db, version, Some(base_of(*base)))? {
                    Some(entry) => {
                        stats.objects_copied_from_pack += 1;
                        entry
                    }
                    None => whole(count, db, buf, compression, stats)?,
                },
                How::Computed { base, delta } => {
                    stats.computed_delta_objects += 1;
                    output::Entry::from_found_delta(
                        count.id,
                        base_of(*base),
                        &counts[*base as usize].id,
                        delta,
                        &Objects(db),
                        compression,
                    )?
                }
            };
            if matches!(entry.kind, output::entry::Kind::DeltaOid { .. }) {
                stats.ref_delta_objects += 1;
            }
            out.push(entry);
        }
        Ok(out)
    }
}

/// What is known of the object of a count before the delta search, as Git's `check_object()` finds it out.
#[derive(Debug, Clone, Copy, Default)]
struct Details {
    /// The kind of the object, or `None` if the object database doesn't have it or if it is stored as a delta and wasn't
    /// read.
    kind: Option<Kind>,
    /// The size of the object's data, if its kind is known.
    size: u64,
    /// The pack entry that the object can be copied from.
    stored: Option<Stored>,
}

/// A pack entry that an object can be copied from.
#[derive(Debug, Clone, Copy)]
struct Stored {
    /// The pack of the entry.
    pack_id: u32,
    /// The delta that the entry holds, or `None` if it holds the object whole.
    delta: Option<StoredDelta>,
}

/// A delta that a pack entry holds.
#[derive(Debug, Clone, Copy)]
struct StoredDelta {
    /// The length of the delta.
    size: u64,
    /// The offset of the entry of its base in the same pack, or `None` if the delta refers to its base by id.
    base_offset: Option<data::Offset>,
}

/// What the pack entry of `count`, if it can be copied into a pack of `version`, or else the object tells of it.
fn examine(db: &impl crate::Find, count: &output::Count, version: data::Version, buf: &mut Vec<u8>) -> Result<Details> {
    if let Some(location) = count.entry_pack_location.as_ref()
        && let Some(entry) = db.entry_by_location(location)
        && entry.version == version
    {
        let pack_entry = data::Entry::from_bytes(&entry.data, 0, count.id.kind())?;
        let delta = |base_offset| StoredDelta {
            size: pack_entry.decompressed_size,
            base_offset,
        };
        let (kind, delta) = match pack_entry.header {
            Header::OfsDelta { base_distance } => {
                let Some(base_offset) = Header::verified_base_pack_offset(location.pack_offset, base_distance) else {
                    bail!(
                        "The delta of {} refers to a base before the start of its pack".corrupted(),
                        count.id
                    );
                };
                (None, Some(delta(Some(base_offset))))
            }
            Header::RefDelta { .. } => (None, Some(delta(None))),
            header => (header.as_kind(), None),
        };
        return Ok(Details {
            kind,
            size: if delta.is_none() {
                pack_entry.decompressed_size
            } else {
                0
            },
            stored: Some(Stored {
                pack_id: location.pack_id,
                delta,
            }),
        });
    }
    Ok(
        match db
            .try_find(&count.id, buf)
            .or_raise(|| message("Could not find object while generating pack"))?
        {
            Some((object, _location)) => Details {
                kind: Some(object.kind),
                size: object.data.len() as u64,
                stored: None,
            },
            None => Details::default(),
        },
    )
}

/// For each of `counts` with `details`, the index of the count whose object its pack stores it as a delta of, if that
/// delta can be copied: it refers to its base by offset, and the base is among `counts`, at the same location or
/// elsewhere. Preferred bases don't copy deltas.
fn copied_bases(counts: &[output::Count], details: &[Details], db: &impl crate::Find) -> Result<Vec<Option<u32>>> {
    let mut by_location = HashMap::new();
    let mut by_id = gix_hashtable::HashMap::default();
    for (index, count) in counts.iter().enumerate() {
        if let Some(location) = count.entry_pack_location.as_ref() {
            by_location.insert((location.pack_id, location.pack_offset), index as u32);
        }
        by_id.insert(count.id, index as u32);
    }
    // The offsets of the entries of each pack and their objects, ordered by offset, once they are needed.
    let mut pack_offsets: HashMap<u32, Vec<(data::Offset, ObjectId)>> = HashMap::new();
    let mut bases = vec![None; counts.len()];
    for ((count, details), base) in counts.iter().zip(details).zip(&mut bases) {
        let Some(Stored {
            pack_id,
            delta: Some(StoredDelta {
                base_offset: Some(base_offset),
                ..
            }),
        }) = details.stored
        else {
            continue;
        };
        if count.preferred_base {
            continue;
        }
        if let Some(&index) = by_location.get(&(pack_id, base_offset)) {
            *base = Some(index);
            continue;
        }
        // Like Git, look for the base by its id, which another pack may hold for the pack being written.
        let offsets = match pack_offsets.entry(pack_id) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut offsets = db
                    .pack_offsets_and_oid(pack_id)?
                    .ok_or_raise(|| not_found("Pack used for counts is no longer available"))?;
                offsets.sort_by_key(|(offset, _id)| *offset);
                entry.insert(offsets)
            }
        };
        *base = offsets
            .binary_search_by_key(&base_offset, |(offset, _id)| *offset)
            .ok()
            .and_then(|position| by_id.get(&offsets[position].1).copied());
    }
    Ok(bases)
}

/// The entry of `count` read from `db` and compressed whole with `compression`, or an invalid entry if `db` doesn't have
/// it anymore.
fn whole(
    count: &output::Count,
    db: &impl crate::Find,
    buf: &mut Vec<u8>,
    compression: gix_zlib::Compression,
    stats: &mut Outcome,
) -> Result<output::Entry> {
    match db
        .try_find(&count.id, buf)
        .or_raise(|| message("Could not find object while generating pack"))?
    {
        Some((object, _location)) => {
            stats.decoded_and_recompressed_objects += 1;
            output::Entry::from_data(count, &object, compression)
        }
        None => {
            stats.missing_objects += 1;
            Ok(output::Entry::invalid())
        }
    }
}

/// The entry of `count` copied from its pack entry in `version`, as the delta of `base` that the entry holds or whole if
/// `base` is `None`, or `None` if the entry isn't available anymore or doesn't hold the object as planned.
fn copy(
    count: &output::Count,
    db: &impl crate::Find,
    version: data::Version,
    base: Option<DeltaBase>,
) -> Result<Option<output::Entry>> {
    let Some(entry) = count
        .entry_pack_location
        .as_ref()
        .and_then(|location| db.entry_by_location(location))
        .filter(|entry| entry.version == version)
    else {
        return Ok(None);
    };
    let pack_entry = data::Entry::from_bytes(&entry.data, 0, count.id.kind())?;
    let kind = match (base, pack_entry.header.as_kind()) {
        (Some(DeltaBase::Index(object_index)), None) => output::entry::Kind::DeltaRef { object_index },
        (Some(DeltaBase::Id(id)), None) => output::entry::Kind::DeltaOid { id },
        (None, Some(kind)) => output::entry::Kind::Base(kind),
        _ => return Ok(None),
    };
    Ok(Some(output::Entry {
        id: count.id,
        kind,
        decompressed_size: pack_entry.decompressed_size as usize,
        compressed_data: compressed_data(entry.data, pack_entry.data_offset),
    }))
}

fn check_interrupt(should_interrupt: &AtomicBool) -> Result {
    if should_interrupt.load(Ordering::Relaxed) {
        bail!(cancelled("Interrupted while planning the entries of the pack"));
    }
    Ok(())
}

/// The objects of a [`crate::Find`], as the delta search reads them.
struct Objects<F>(F);

impl<F: crate::Find> gix_object::Find for Objects<F> {
    fn try_find<'a>(&self, id: &oid, buffer: &'a mut Vec<u8>) -> Result<Option<gix_object::Data<'a>>> {
        Ok(self.0.try_find(id, buffer)?.map(|(object, _location)| object))
    }
}
