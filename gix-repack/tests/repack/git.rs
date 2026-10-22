//! The cases that [`gix_repack::repack()`] repacks as git does.

use gix_repack::Options;
use gix_testtools::TestResult;

use crate::{Repacked, file_name, file_names, repack};

/// Repack a copy of the case `name`, and check that the same packs, loose objects and files of multi-pack-indices remain
/// as when git repacked it, and that the new pack holds the same objects as the pack that git wrote.
fn agrees_with_git(name: &'static str) -> TestResult<Repacked> {
    let repacked = repack(name)?;
    repacked.assert_packs_and_objects_as_git()?;
    assert_eq!(
        repacked.case.multi_pack_indices()?,
        repacked.case.record("midx-after")?,
        "{name}: the files of multi-pack-indices that git left remain"
    );
    Ok(repacked)
}

#[test]
fn the_smallest_packs_and_the_loose_objects_are_combined() -> TestResult {
    let Repacked { case, outcome, .. } = agrees_with_git("geometric")?;
    assert_eq!(
        outcome.removed_packs.len(),
        3,
        "the three smallest of the packs that aren't kept were combined"
    );
    assert_eq!(outcome.retained_packs.len(), 1, "the largest pack remains");
    assert_eq!(
        outcome.pack.map(|pack| pack.num_objects),
        Some(9),
        "the new pack holds the objects that neither the largest pack nor the kept pack holds"
    );
    assert_eq!(
        outcome.removed_loose_objects, 5,
        "every loose object is in a pack now, also the one that the largest pack held already"
    );

    assert!(
        !gix_repack::needs_repack(&case.objects_dir(), gix_testtools::object_hash(), 2, 1)?,
        "the packs form a progression now, and no object is loose"
    );
    let again = case.repack(Options::default())?;
    assert_eq!(
        (again.pack, again.removed_packs.len()),
        (None, 0),
        "a second repack changes nothing"
    );
    Ok(())
}

#[test]
fn a_multi_pack_index_of_remaining_packs_remains() -> TestResult {
    let repacked = agrees_with_git("midx-retained")?;
    let index = gix_pack::multi_index::File::at(repacked.case.pack_dir().join("multi-pack-index"), None)?;
    assert_eq!(
        file_names(index.index_names())?,
        file_names(&repacked.outcome.retained_packs)?,
        "the multi-pack-index lists the pack that remains"
    );
    assert_eq!(
        repacked.outcome.removed_packs.len(),
        2,
        "the two packs that it doesn't list were combined"
    );
    Ok(())
}

#[test]
fn packs_with_long_chains_of_deltas_are_combined() -> TestResult {
    let Repacked { outcome, .. } = agrees_with_git("chain")?;
    assert_eq!(outcome.removed_packs.len(), 2, "both packs were combined");
    assert_eq!(
        outcome.pack.map(|pack| pack.num_objects),
        Some(768),
        "the new pack holds every object"
    );
    Ok(())
}

#[test]
fn only_loose_objects_are_packed_where_the_packs_form_a_progression() -> TestResult {
    let Repacked { outcome, .. } = agrees_with_git("loose-only")?;
    assert_eq!(outcome.removed_packs.len(), 0, "no pack was combined");
    assert_eq!(outcome.retained_packs.len(), 2, "both packs remain");
    assert_eq!(
        outcome.pack.map(|pack| pack.num_objects),
        Some(3),
        "the loose objects of a commit were packed"
    );
    assert_eq!(
        outcome.removed_loose_objects, 3,
        "the packed loose objects were removed"
    );
    Ok(())
}

#[test]
fn nothing_is_written_where_the_packs_form_a_progression_and_no_object_is_loose() -> TestResult {
    let Repacked { outcome, .. } = agrees_with_git("nothing")?;
    assert_eq!(outcome.pack, None, "no pack was written");
    assert_eq!(outcome.removed_packs.len(), 0, "no pack was combined");
    assert_eq!(outcome.retained_packs.len(), 2, "both packs remain");
    Ok(())
}

#[test]
fn combined_packs_whose_objects_a_kept_pack_holds_are_removed() -> TestResult {
    let repacked = agrees_with_git("kept-only")?;
    assert_eq!(repacked.outcome.pack, None, "no object needed to be packed");
    assert_eq!(
        repacked.outcome.removed_packs.len(),
        2,
        "the two smaller packs were combined nevertheless"
    );
    let packs_after = repacked.case.packs()?;
    for pack in repacked.survey_before.packs.iter().filter(|pack| pack.keep) {
        assert!(
            packs_after.contains(&file_name(&pack.index_path)?),
            "the kept pack remains"
        );
    }
    Ok(())
}

#[test]
fn objects_that_a_cruft_pack_holds_as_well_are_copied() -> TestResult {
    let repacked = agrees_with_git("cruft")?;
    assert_eq!(
        repacked.outcome.pack.as_ref().map(|pack| pack.num_objects),
        Some(7),
        "the new pack holds the objects of both combined packs, also the one that the cruft pack holds"
    );
    let packs_after = repacked.case.packs()?;
    let mut cruft_packs = repacked.survey_before.packs.iter().filter(|pack| pack.cruft).peekable();
    assert!(cruft_packs.peek().is_some(), "the case has a cruft pack");
    for pack in cruft_packs {
        assert!(
            packs_after.contains(&file_name(&pack.index_path)?),
            "the cruft pack remains"
        );
    }
    Ok(())
}

#[test]
fn objects_that_a_promisor_pack_holds_as_well_are_copied() -> TestResult {
    let repacked = agrees_with_git("promisor")?;
    assert_eq!(
        repacked.outcome.pack.as_ref().map(|pack| pack.num_objects),
        Some(6),
        "the new pack holds the objects of both combined packs, also the one that the promisor pack holds"
    );
    let packs_after = repacked.case.packs()?;
    let mut promisor_packs = repacked
        .survey_before
        .packs
        .iter()
        .filter(|pack| pack.promisor)
        .peekable();
    assert!(promisor_packs.peek().is_some(), "the case has a promisor pack");
    for pack in promisor_packs {
        assert!(
            packs_after.contains(&file_name(&pack.index_path)?),
            "the promisor pack remains"
        );
    }
    Ok(())
}

#[test]
fn a_combined_pack_that_the_new_pack_has_the_name_of_remains() -> TestResult {
    let repacked = agrees_with_git("same-name")?;
    let [smaller, larger] = repacked.survey_before.packs.as_slice() else {
        return Err("the case has two packs".into());
    };
    let written = repacked
        .outcome
        .pack
        .as_ref()
        .ok_or("the two packs were combined into a new one")?;
    assert_eq!(
        written.index_path.file_name(),
        larger.index_path.file_name(),
        "the new pack holds the objects of the larger pack in the same order, so it has its name"
    );
    assert_eq!(
        repacked.outcome.removed_packs,
        vec![smaller.index_path.clone()],
        "only the smaller pack was removed"
    );
    Ok(())
}
