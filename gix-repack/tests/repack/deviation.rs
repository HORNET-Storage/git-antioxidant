//! Where [`gix_repack::repack()`] deviates from git, as the crate's documentation describes.

use gix_testtools::TestResult;

use crate::repack;

#[test]
fn promisor_packs_are_not_combined() -> TestResult {
    let repacked = repack("promisors")?;
    let case = &repacked.case;
    assert_eq!(
        case.record("packs-after")?.len(),
        2,
        "git combined the two promisor packs into one"
    );
    assert_eq!(
        case.record("new-objects")?.len(),
        2,
        "the pack that git wrote holds the objects of both promisor packs"
    );
    assert_eq!(case.packs()?, repacked.packs_before, "here, every pack remains");
    assert_eq!(repacked.outcome.pack, None, "no pack was written");
    Ok(())
}

#[test]
fn an_incremental_multi_pack_index_is_removed_whenever_packs_are() -> TestResult {
    let repacked = repack("midx-incremental")?;
    repacked.assert_packs_and_objects_as_git()?;
    let case = &repacked.case;
    assert!(
        case.record("midx-after")?
            .iter()
            .any(|path| path == "multi-pack-index.d/multi-pack-index-chain"),
        "git kept the chain, as it lists only the pack that remains"
    );
    assert_eq!(
        case.multi_pack_indices()?,
        Vec::<String>::new(),
        "here, the chain and its layers were removed"
    );
    assert!(
        case.pack_dir().join("multi-pack-index.d").is_dir(),
        "the directory of the chain remains"
    );
    Ok(())
}

#[test]
fn fanout_directories_remain_when_their_loose_objects_are_removed() -> TestResult {
    let repacked = repack("loose-only")?;
    let case = &repacked.case;
    assert_eq!(case.loose()?, Vec::<String>::new(), "every loose object was removed");
    for name in &repacked.loose_before {
        let fanout = case.objects_dir().join(&name[..2]);
        assert!(fanout.is_dir(), "{} remains", fanout.display());
    }
    Ok(())
}
