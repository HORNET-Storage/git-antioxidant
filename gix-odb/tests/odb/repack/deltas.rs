//! Deltas are copied from the combined packs, and chains of deltas that are longer than the delta depth are cut as git
//! cuts them. The search for deltas is left out with a window of 0, so that the objects that the cut chains leave whole
//! remain whole; `computed.rs` compares the deltas of repacks with the search with those of git.

use gix_odb::repack::Options;
use gix_testtools::TestResult;

use super::Case;

#[test]
fn chains_are_cut_every_delta_depth_plus_one_deltas() -> TestResult {
    for delta_depth in [50, 3, 0] {
        let case = Case::writable("chain")?;
        let before = case.delta_depths(&case.pack_indices()?)?;
        let longest = before.values().copied().max().unwrap_or_default();
        assert_eq!(
            case.fixture_file("chain.depth-before")?,
            [longest.to_string()],
            "git reports the same longest chain as the fixture recorded"
        );
        assert!(
            longest > 50,
            "the packs hold chains that are longer than git's default depth: {longest}"
        );

        let outcome = case.repack(Options {
            delta_depth,
            window: 0,
            ..Options::default()
        })?;

        let written = outcome.pack.ok_or("both packs were combined into a new one")?;
        let after = case.delta_depths(&[written.index_path])?;
        let modulus = delta_depth + 1;
        let expected: Vec<(_, u32)> = before.iter().map(|(id, depth)| (*id, depth % modulus)).collect();
        assert_eq!(
            after.into_iter().collect::<Vec<_>>(),
            expected,
            "with depth {delta_depth}, objects at a multiple of {modulus} deltas become whole, others keep their deltas"
        );
    }
    Ok(())
}
