//! The chains of deltas of a pack, as `git pack-objects` cuts them in its `break_delta_chains()`, measures them in its
//! `check_delta_limit()`, and writes them in its `compute_write_order()` and `write_one()`.
use gix_object::Kind;
use gix_pack::data::delta::chain::{self, Object};

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

/// Blobs with `bases` that no tag points to.
fn blobs(bases: &[Option<u32>]) -> Vec<Object> {
    bases
        .iter()
        .map(|&base| Object {
            base,
            kind: Some(Kind::Blob),
            tagged: false,
            preferred_base: false,
        })
        .collect()
}

fn assert_bases_come_first(bases: &[Option<u32>]) {
    let order = chain::write_order(&blobs(bases));
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
    // Each object is a delta against the one listed before it.
    let mut bases = vec![None, Some(0), Some(1), Some(2), Some(3), Some(4)];
    chain::break_chains(&mut bases, 2);
    assert_eq!(bases, [None, Some(0), Some(1), None, Some(3), Some(4)]);
    assert_bases_come_first(&bases);

    // Each object is a delta against the one listed after it, so the chain is followed from its delta at the top.
    let mut bases = vec![Some(1), Some(2), Some(3), Some(4), None];
    chain::break_chains(&mut bases, 2);
    assert_eq!(
        bases,
        [Some(1), None, Some(3), Some(4), None],
        "as in Git, the chain of 2 deltas at the bottom is kept whole, and the delta left over stays above it"
    );
    assert_eq!(depths(&bases), [1, 0, 2, 1, 0]);
    assert_bases_come_first(&bases);
}

#[test]
fn chains_within_the_depth_are_kept() {
    let mut bases = vec![Some(1), Some(2), None, Some(1)];
    chain::break_chains(&mut bases, 50);
    assert_eq!(bases, [Some(1), Some(2), None, Some(1)]);
    assert_eq!(
        chain::write_order(&blobs(&bases)),
        [2, 1, 0, 3],
        "each base is written right before the first delta that needs it"
    );
}

#[test]
fn a_depth_of_zero_stores_every_object_whole() {
    let mut bases = vec![None, Some(0), Some(1)];
    chain::break_chains(&mut bases, 0);
    assert_eq!(bases, [None, None, None]);
}

#[test]
fn cycles_are_broken_where_they_close() {
    let mut bases = vec![Some(1), Some(2), Some(0)];
    chain::break_chains(&mut bases, 50);
    assert_eq!(
        bases,
        [Some(1), Some(2), None],
        "the delta that leads back to the first object is dropped"
    );
    assert_bases_come_first(&bases);

    let mut bases = vec![Some(0)];
    chain::break_chains(&mut bases, 50);
    assert_eq!(bases, [None], "a delta against itself is dropped");

    let mut bases = vec![Some(1), Some(2), Some(1), Some(0)];
    chain::break_chains(&mut bases, 50);
    assert_eq!(bases, [Some(1), Some(2), None, Some(0)]);
    assert_eq!(depths(&bases), [2, 1, 0, 3]);
    assert_bases_come_first(&bases);
}

#[test]
fn dependents_depths_are_those_of_the_longest_chain_below_each_object() {
    // 1 and 2 are deltas of 0, 3 of 2, 4 of 3, and 5 is stored whole.
    let bases = [None, Some(0), Some(0), Some(2), Some(3), None];
    assert_eq!(chain::dependents_depths(&bases), [3, 0, 2, 1, 0, 0]);
}

#[test]
fn tagged_objects_come_first_then_commits_and_tags_then_trees_then_families_of_deltas() {
    let object = |base: Option<u32>, kind: Option<Kind>| Object {
        base,
        kind,
        tagged: false,
        preferred_base: false,
    };
    let objects = [
        object(None, Some(Kind::Commit)),
        Object {
            tagged: true,
            ..object(None, Some(Kind::Commit))
        },
        object(Some(5), Some(Kind::Blob)),
        object(None, Some(Kind::Tree)),
        object(None, Some(Kind::Tag)),
        object(None, Some(Kind::Blob)),
        object(Some(5), Some(Kind::Blob)),
        Object {
            tagged: true,
            ..object(None, Some(Kind::Tree))
        },
        // A delta copied from a pack, whose kind Git doesn't know.
        object(Some(2), None),
        Object {
            preferred_base: true,
            ..object(None, Some(Kind::Blob))
        },
        object(Some(9), Some(Kind::Blob)),
    ];
    assert_eq!(
        chain::write_order(&objects),
        [0, 1, 7, 4, 3, 5, 2, 6, 8, 10],
        "the objects before the first tagged one keep their place, then come the tagged ones, the commits and tags, \
         the trees, and each object stored whole with its deltas and theirs, without the preferred base"
    );
}

#[test]
fn preferred_bases_are_not_written_but_their_deltas_are() {
    let objects = [
        Object {
            base: None,
            kind: Some(Kind::Blob),
            tagged: false,
            preferred_base: true,
        },
        Object {
            base: Some(0),
            kind: Some(Kind::Blob),
            tagged: false,
            preferred_base: false,
        },
        Object {
            base: None,
            kind: Some(Kind::Blob),
            tagged: false,
            preferred_base: false,
        },
    ];
    assert_eq!(chain::write_order(&objects), [1, 2]);
    assert_eq!(
        chain::dependents_depths(&objects.iter().map(|object| object.base).collect::<Vec<_>>()),
        [1, 0, 0],
        "the deltas of preferred bases count towards the depth of their chains"
    );
}
