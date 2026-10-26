use gix_pack::data::{delta::Index, entry::Header};
use gix_testtools::TestResult;

/// Each delta that `git pack-objects` wrote into the pack of `make_deltas.sh` is the delta that `data::delta` computes
/// from the same base and target.
#[test]
fn deltas_that_git_writes_have_the_same_bytes_here() -> TestResult {
    let object_hash = gix_testtools::object_hash();
    let dir = gix_testtools::scripted_fixture_read_only("make_deltas.sh")?.join("packed");
    let mut packs = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "pack") {
            packs.push(path);
        }
    }
    let [pack_path] = packs.as_slice() else {
        return Err(format!("git pack-objects wrote one pack, but {dir:?} holds {packs:?}").into());
    };
    let pack = gix_pack::data::File::at(pack_path, object_hash)?;

    let mut inflate = gix_zlib::Inflate::default();
    let (mut delta, mut ours) = (Vec::new(), Vec::new());
    let mut deltas = 0;
    for entry in pack.streaming_iter()? {
        let entry = entry?;
        let Header::OfsDelta { base_distance } = entry.header else {
            assert!(
                !matches!(entry.header, Header::RefDelta { .. }),
                "git was asked to refer to bases by their offset"
            );
            continue;
        };
        let target_entry = pack.entry(entry.pack_offset)?;
        delta.resize(usize::try_from(target_entry.decompressed_size)?, 0);
        pack.decompress_entry(&target_entry, &mut inflate, &mut delta)?;
        let base_offset = target_entry.base_pack_offset(base_distance);
        let base = decode(&pack, base_offset, &mut inflate)?;
        let target = decode(&pack, entry.pack_offset, &mut inflate)?;

        let index = Index::new(&base).expect("git computes no deltas against empty bases");
        assert!(index.encode(&base, &target, None, &mut ours), "the target isn't empty");
        assert!(
            ours == delta,
            "the delta of the object at offset {} against the one at {base_offset} has {} bytes here and {} in git",
            entry.pack_offset,
            ours.len(),
            delta.len()
        );
        deltas += 1;
    }
    assert!(deltas >= 40, "git stored only {deltas} objects as deltas");
    Ok(())
}

/// The object at `offset` of `pack`.
fn decode(pack: &gix_pack::data::File, offset: u64, inflate: &mut gix_zlib::Inflate) -> TestResult<Vec<u8>> {
    let mut out = Vec::new();
    pack.decode_entry(
        pack.entry(offset)?,
        &mut out,
        inflate,
        &|_, _| Ok(None),
        &mut gix_pack::cache::Never,
    )?;
    Ok(out)
}
