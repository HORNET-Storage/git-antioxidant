use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use gix_error::{Result, ResultExt, bail, cancelled, ensure, message};
use gix_hash::ObjectId;
use gix_pack::data::entry::Header;
use gix_utils::progress::{self, Count, DynNestedProgress, Progress};

use crate::repack::Pack;

/// How many objects are looked at between two checks of the interrupt flag.
const OBJECTS_PER_INTERRUPT_CHECK: usize = 4096;

/// A combined pack, opened to copy its entries.
pub(crate) struct Source {
    pub(crate) data: gix_pack::data::File,
    pub(crate) index: gix_pack::index::File,
    /// The offset of each entry and its position in `index`, ordered by offset.
    entries: Vec<(u64, u32)>,
}

impl Source {
    fn open(pack: &Pack, object_hash: gix_hash::Kind) -> Result<Self> {
        let index = open_index(pack, object_hash)?;
        let data_path = pack.data_path();
        let data = gix_pack::data::File::at(&data_path, object_hash)
            .or_raise(|| message!("Could not open the pack {}", data_path.display()))?;
        ensure!(
            data.num_objects() == index.num_objects() && data.checksum() == index.pack_checksum(),
            "The pack {} doesn't match its index".corrupted(),
            data_path.display()
        );
        let mut entries: Vec<(u64, u32)> = (0..index.num_objects())
            .map(|position| (index.pack_offset_at_index(position), position))
            .collect();
        entries.sort_unstable();
        let pack_end = data.pack_end() as u64;
        ensure!(
            entries
                .first()
                .is_none_or(|&(offset, _)| offset >= gix_pack::data::header::SIZE as u64)
                && entries.last().is_none_or(|&(offset, _)| offset < pack_end)
                && entries.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "The index of {} lists entries outside of the pack or several at one offset".corrupted(),
            data_path.display()
        );
        Ok(Source { data, index, entries })
    }

    /// The id of the object that the entry at `offset` applies to, if it's a delta.
    fn delta_base(&self, offset: u64) -> Result<Option<ObjectId>> {
        let entry = self.data.entry(offset).or_raise(|| {
            message!(
                "Could not read the entry at offset {offset} of {}",
                self.data.path().display()
            )
        })?;
        Ok(match entry.header {
            Header::RefDelta { base_id } => Some(base_id),
            Header::OfsDelta { base_distance } => {
                let position = Header::verified_base_pack_offset(offset, base_distance).and_then(|base_offset| {
                    self.entries
                        .binary_search_by_key(&base_offset, |&(entry_offset, _)| entry_offset)
                        .ok()
                });
                let Some(position) = position else {
                    bail!(
                        "The delta at offset {offset} of {} doesn't apply to an entry before it".corrupted(),
                        self.data.path().display()
                    );
                };
                Some(self.index.oid_at_index(self.entries[position].1).to_owned())
            }
            Header::Commit | Header::Tree | Header::Blob | Header::Tag => None,
        })
    }
}

/// Where an object of the new pack is copied or read from.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Location {
    /// The entry of [`Plan::sources`]`[source]` at `offset`, which ends at `end` and is at `position` in its index.
    Packed {
        source: usize,
        offset: u64,
        end: u64,
        position: u32,
    },
    /// A loose object.
    Loose,
}

/// An object of the new pack.
pub(crate) struct Object {
    pub(crate) id: ObjectId,
    pub(crate) location: Location,
}

/// The objects that go into the new pack, and how.
pub(crate) struct Plan {
    /// The combined packs, the one modified last first.
    pub(crate) sources: Vec<Source>,
    /// The objects of the new pack, in the order they were selected in.
    pub(crate) objects: Vec<Object>,
    /// For each object, the object that its delta applies to as an index into `objects`, if the delta is copied from
    /// its pack, or `None` if the object is stored whole.
    pub(crate) bases: Vec<Option<u32>>,
    /// The indices into `objects` in the order the objects are written in, each delta after the object it applies to.
    pub(crate) order: Vec<u32>,
    /// The loose objects that couldn't be read or whose data doesn't hash to their name.
    pub(crate) skipped_loose_objects: Vec<ObjectId>,
}

impl Plan {
    /// Select the objects of the `combined` packs and the loose objects of the object database in `objects_dir` that
    /// none of the `excluded` packs holds, as `git pack-objects --stdin-packs --unpacked` does if the `combined` packs
    /// are its included and the `excluded` packs its excluded packs, and plan which deltas to copy, in chains of at most
    /// `delta_depth` deltas.
    pub(crate) fn new(
        objects_dir: &Path,
        object_hash: gix_hash::Kind,
        combined: &[&Pack],
        excluded: &[&Pack],
        delta_depth: u32,
        progress: &mut dyn DynNestedProgress,
        should_interrupt: &AtomicBool,
    ) -> Result<Self> {
        let mut selecting = progress.add_child("select objects".into());
        selecting.init(None, progress::count("objects"));
        let excluded = excluded
            .iter()
            .map(|pack| open_index(pack, object_hash))
            .collect::<Result<Vec<_>>>()?;
        let is_excluded = |id: &ObjectId| excluded.iter().any(|index| index.lookup(id).is_some());

        // Like git, copy each object from the combined pack modified last among those that hold it.
        let mut combined = combined.to_vec();
        combined.sort_by(|a, b| {
            b.modified
                .cmp(&a.modified)
                .then_with(|| a.index_path.cmp(&b.index_path))
        });
        let sources = combined
            .iter()
            .map(|pack| Source::open(pack, object_hash))
            .collect::<Result<Vec<_>>>()?;

        let mut selection = Selection::default();
        for (source_index, source) in sources.iter().enumerate() {
            for (entry, &(offset, position)) in source.entries.iter().enumerate() {
                check_interrupt(entry, should_interrupt)?;
                let id = source.index.oid_at_index(position).to_owned();
                if selection.contains(&id) || is_excluded(&id) {
                    continue;
                }
                let end = source
                    .entries
                    .get(entry + 1)
                    .map_or(source.data.pack_end() as u64, |&(next_offset, _)| next_offset);
                selection.add(
                    id,
                    Location::Packed {
                        source: source_index,
                        offset,
                        end,
                        position,
                    },
                )?;
                selecting.inc();
            }
        }

        let loose = crate::loose::Store::at(objects_dir, object_hash);
        let mut skipped_loose_objects = Vec::new();
        let mut buf = Vec::new();
        for (count, id) in loose.iter().enumerate() {
            check_interrupt(count, should_interrupt)?;
            let id = id.or_raise(|| message!("Could not list the loose objects in {}", objects_dir.display()))?;
            if selection.contains(&id) || is_excluded(&id) {
                continue;
            }
            match loose.try_find(&id, &mut buf) {
                Ok(Some(data))
                    if gix_object::compute_hash(object_hash, data.kind, data.data).is_ok_and(|actual| actual == id) =>
                {
                    selection.add(id, Location::Loose)?;
                    selecting.inc();
                }
                // The object was removed since its directory was listed.
                Ok(None) => {}
                _ => {
                    gix_trace::warn!(
                        "Leaving the loose object {} as it is, as it can't be read or its data doesn't hash to its name",
                        id
                    );
                    skipped_loose_objects.push(id);
                }
            }
        }

        let Selection { objects, indices } = selection;
        let mut bases = Vec::with_capacity(objects.len());
        for object in &objects {
            bases.push(match object.location {
                Location::Packed { source, offset, .. } => sources[source]
                    .delta_base(offset)?
                    .and_then(|base_id| indices.get(&base_id).copied()),
                Location::Loose => None,
            });
        }
        // As each object is copied from the first combined pack that holds it, deltas form cycles only where a pack
        // holds a delta whose base it doesn't hold. Chains can be longer than `delta_depth` in any pack.
        break_delta_chains(&mut bases, delta_depth);
        let order = write_order(&bases);
        Ok(Plan {
            sources,
            objects,
            bases,
            order,
            skipped_loose_objects,
        })
    }
}

/// The objects selected so far.
#[derive(Default)]
struct Selection {
    objects: Vec<Object>,
    /// The index of each selected object in `objects`.
    indices: gix_hashtable::HashMap<ObjectId, u32>,
}

impl Selection {
    fn contains(&self, id: &ObjectId) -> bool {
        self.indices.contains_key(id)
    }

    fn add(&mut self, id: ObjectId, location: Location) -> Result {
        let Some(index) = u32::try_from(self.objects.len()).ok().filter(|&index| index < u32::MAX) else {
            bail!("The new pack would hold more objects than a pack can".unsupported());
        };
        self.indices.insert(id, index);
        self.objects.push(Object { id, location });
        Ok(())
    }
}

fn open_index(pack: &Pack, object_hash: gix_hash::Kind) -> Result<gix_pack::index::File> {
    gix_pack::index::File::at(&pack.index_path, object_hash)
        .or_raise(|| message!("Could not open the pack index {}", pack.index_path.display()))
}

fn check_interrupt(count: usize, should_interrupt: &AtomicBool) -> Result {
    if count.is_multiple_of(OBJECTS_PER_INTERRUPT_CHECK) && should_interrupt.load(Ordering::Relaxed) {
        bail!(cancelled("Interrupted while selecting the objects of the new pack"));
    }
    Ok(())
}

/// Drop the deltas in `bases` that would form a cycle or a chain of more than `depth` deltas, as git's
/// `break_delta_chains()` does: a chain that is too long is cut into chains of `depth` deltas on one object stored
/// whole, and one chain with the deltas that remain, which the object the chain was followed from is in.
fn break_delta_chains(bases: &mut [Option<u32>], depth: u32) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Unseen,
        /// On the chain that is being followed.
        Active,
        /// Its depth is final.
        Done,
    }
    let modulus = u64::from(depth) + 1;
    let mut state = vec![State::Unseen; bases.len()];
    let mut depths = vec![0u32; bases.len()];
    for start in 0..bases.len() {
        // Follow the chain from `start` down to an object stored whole or one whose depth is known, and drop the delta
        // that would lead back into the chain.
        let mut total_depth = 0u64;
        let mut current = start;
        loop {
            if state[current] == State::Done {
                total_depth += u64::from(depths[current]);
                break;
            }
            let Some(base) = bases[current].map(|base| base as usize) else {
                state[current] = State::Done;
                break;
            };
            state[current] = State::Active;
            if state[base] == State::Active {
                bases[current] = None;
                state[current] = State::Done;
                break;
            }
            current = base;
            total_depth += 1;
        }
        // Then assign the depths from `start` on, and store each object whose depth would be a multiple of `depth + 1`
        // whole.
        let mut current = Some(start);
        while let Some(index) = current {
            if state[index] == State::Done {
                break;
            }
            current = bases[index].map(|base| base as usize);
            let object_depth = total_depth % modulus;
            total_depth -= 1;
            depths[index] = object_depth as u32;
            if object_depth == 0 {
                bases[index] = None;
            }
            state[index] = State::Done;
        }
    }
}

/// The order in which to write the objects with `bases`, as they were selected but with each delta after the object it
/// applies to. `bases` must not contain cycles.
fn write_order(bases: &[Option<u32>]) -> Vec<u32> {
    let mut order = Vec::with_capacity(bases.len());
    let mut placed = vec![false; bases.len()];
    let mut chain = Vec::new();
    for start in 0..bases.len() {
        let mut current = start;
        while !placed[current] {
            placed[current] = true;
            chain.push(current as u32);
            match bases[current] {
                Some(base) => current = base as usize,
                None => break,
            }
        }
        order.extend(chain.drain(..).rev());
    }
    order
}

#[cfg(test)]
mod tests {
    use super::{break_delta_chains, write_order};

    /// The amount of deltas below each object, after which the chains end in objects stored whole.
    fn depths(bases: &[Option<u32>]) -> Vec<usize> {
        (0..bases.len())
            .map(|start| {
                let mut depth = 0;
                let mut current = start;
                while let Some(base) = bases[current] {
                    depth += 1;
                    assert!(depth <= bases.len(), "a cycle remains");
                    current = base as usize;
                }
                depth
            })
            .collect()
    }

    fn assert_bases_come_first(bases: &[Option<u32>]) {
        let order = write_order(bases);
        let mut position = vec![usize::MAX; bases.len()];
        for (index, &object) in order.iter().enumerate() {
            assert_eq!(position[object as usize], usize::MAX, "each object is written once");
            position[object as usize] = index;
        }
        assert!(
            position.iter().all(|&index| index != usize::MAX),
            "every object is written"
        );
        for (object, base) in bases.iter().enumerate() {
            if let Some(base) = base {
                assert!(
                    position[*base as usize] < position[object],
                    "the object a delta applies to is written before it"
                );
            }
        }
    }

    #[test]
    fn long_chains_are_cut_into_chains_of_depth_deltas() {
        // Each object is a delta against the one selected before it.
        let mut bases = vec![None, Some(0), Some(1), Some(2), Some(3), Some(4)];
        break_delta_chains(&mut bases, 2);
        assert_eq!(bases, [None, Some(0), Some(1), None, Some(3), Some(4)]);
        assert_bases_come_first(&bases);

        // Each object is a delta against the one selected after it, so the chain is followed from its delta at the top.
        let mut bases = vec![Some(1), Some(2), Some(3), Some(4), None];
        break_delta_chains(&mut bases, 2);
        assert_eq!(
            bases,
            [Some(1), None, Some(3), Some(4), None],
            "as in git, the chain of 2 deltas at the bottom is kept whole, and the delta left over stays above it"
        );
        assert_eq!(depths(&bases), [1, 0, 2, 1, 0]);
        assert_bases_come_first(&bases);
    }

    #[test]
    fn chains_within_the_depth_are_kept() {
        let mut bases = vec![Some(1), Some(2), None, Some(1)];
        break_delta_chains(&mut bases, 50);
        assert_eq!(bases, [Some(1), Some(2), None, Some(1)]);
        assert_eq!(write_order(&bases), [2, 1, 0, 3]);
    }

    #[test]
    fn a_depth_of_zero_stores_every_object_whole() {
        let mut bases = vec![None, Some(0), Some(1)];
        break_delta_chains(&mut bases, 0);
        assert_eq!(bases, [None, None, None]);
    }

    #[test]
    fn cycles_are_broken_where_they_close() {
        let mut bases = vec![Some(1), Some(2), Some(0)];
        break_delta_chains(&mut bases, 50);
        assert_eq!(
            bases,
            [Some(1), Some(2), None],
            "the delta that leads back to the first object is dropped"
        );
        assert_bases_come_first(&bases);

        let mut bases = vec![Some(0)];
        break_delta_chains(&mut bases, 50);
        assert_eq!(bases, [None], "a delta against itself is dropped");

        let mut bases = vec![Some(1), Some(2), Some(1), Some(0)];
        break_delta_chains(&mut bases, 50);
        assert_eq!(bases, [Some(1), Some(2), None, Some(0)]);
        assert_eq!(depths(&bases), [2, 1, 0, 3]);
        assert_bases_come_first(&bases);
    }
}
