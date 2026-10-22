//! Whether a repack is needed is decided as the auto condition of the `geometric-repack` task of `git maintenance`
//! decides whether the task runs.

use gix_repack::{needs_repack, too_many_loose_objects};
use gix_testtools::TestResult;

use crate::FIXTURE;

#[test]
fn needs_repack_decides_as_git_maintenance_does() -> TestResult {
    let dir = gix_testtools::scripted_fixture_read_only(FIXTURE)?;
    let object_hash = gix_testtools::object_hash();
    let mut compared = 0;
    for line in std::fs::read_to_string(dir.join("decisions"))?.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        let [name, limit, decision] = fields.as_slice() else {
            return Err(format!("decisions has the malformed line {line:?}").into());
        };
        let expected = match *decision {
            "repack" => true,
            "skip" => false,
            other => return Err(format!("decisions has the unknown decision {other:?}").into()),
        };
        let objects_dir = dir.join(*name).join(".git").join("objects");
        assert_eq!(
            needs_repack(&objects_dir, object_hash, 2, limit.parse()?)?,
            expected,
            "{name} with maintenance.geometric-repack.auto={limit}"
        );
        compared += 1;
    }
    assert_eq!(compared, 6, "every decision of the fixture was compared");
    Ok(())
}

#[test]
fn split_factors_below_2_combine_no_packs() -> TestResult {
    let dir = gix_testtools::scripted_fixture_read_only(FIXTURE)?;
    let objects_dir = dir.join("midx-retained").join(".git").join("objects");
    let object_hash = gix_testtools::object_hash();
    assert!(
        needs_repack(&objects_dir, object_hash, 2, 1_000_000)?,
        "with a split factor of 2, two of the packs are combined"
    );
    for split_factor in [0, 1] {
        assert!(
            !needs_repack(&objects_dir, object_hash, split_factor, 1_000_000)?,
            "with a split factor of {split_factor}, no pack is combined, and no object is loose"
        );
    }
    Ok(())
}

#[test]
fn too_many_loose_objects_are_estimated_from_the_fanout_directory_17() -> TestResult {
    let object_hash = gix_testtools::object_hash();
    let tmp = gix_testtools::tempfile::TempDir::new()?;
    let objects_dir = tmp.path();
    assert!(
        !too_many_loose_objects(objects_dir, object_hash, 0),
        "without the directory 17, there are no loose objects"
    );

    let fanout = objects_dir.join("17");
    std::fs::create_dir(&fanout)?;
    let width = object_hash.len_in_hex() - 2;
    for name in [
        "tmp_obj_123456".to_owned(),
        "A".repeat(width),
        "a".repeat(width - 1),
        "a".repeat(width + 1),
    ] {
        std::fs::write(fanout.join(name), b"")?;
    }
    assert!(
        !too_many_loose_objects(objects_dir, object_hash, 0),
        "files that aren't named like loose objects aren't counted"
    );

    let add = |n: usize| std::fs::write(fanout.join(format!("{n:0width$x}")), b"");
    add(1)?;
    assert!(too_many_loose_objects(objects_dir, object_hash, 0), "1 is more than 0");
    assert!(
        !too_many_loose_objects(objects_dir, object_hash, 256),
        "1 is no more than 256 divided by 256"
    );
    add(2)?;
    assert!(
        too_many_loose_objects(objects_dir, object_hash, 256),
        "2 is more than 256 divided by 256"
    );
    assert!(
        !too_many_loose_objects(objects_dir, object_hash, 257),
        "2 is no more than 257 divided by 256 and rounded up"
    );
    Ok(())
}
