use gix_object::Kind::Blob;
use gix_pack::data::delta::{Index, encode};
use gix_testtools::TestResult;

use crate::data::delta::{Packed, Rng, assert_pack_holds};

#[test]
fn edited_text_is_mostly_copied() -> TestResult {
    let mut rng = Rng(1);
    let source = rng.text(400);
    let target = rng.edit_lines(&source);
    let delta = encode(&source, &target).expect("short enough to be indexed");
    assert!(
        delta.len() < target.len() / 10,
        "{} bytes of delta for {} bytes",
        delta.len(),
        target.len()
    );
    assert_pack_holds(&[Packed::whole(Blob, &source), Packed::delta(Blob, &target, 0, delta)])
}

#[test]
fn chains_of_edited_objects_build_every_object() -> TestResult {
    let mut rng = Rng(2);
    let mut versions = vec![rng.text(200)];
    let mut bases = vec![None];
    for _ in 0..30 {
        let base = rng.below(versions.len());
        let edited = rng.edit_lines(&versions[base]);
        versions.push(edited);
        bases.push(Some(base));
    }
    let first_binary = versions.len();
    versions.push(rng.bytes(5000));
    bases.push(None);
    for _ in 0..30 {
        let base = first_binary + rng.below(versions.len() - first_binary);
        let edited = rng.edit_bytes(&versions[base]);
        versions.push(edited);
        bases.push(Some(base));
    }

    let mut packed = Vec::new();
    for (version, base) in versions.iter().zip(&bases) {
        packed.push(match base {
            Some(base) => Packed::delta(
                Blob,
                version,
                *base,
                encode(&versions[*base], version).expect("short enough to be indexed"),
            ),
            None => Packed::whole(Blob, version),
        });
    }
    assert_pack_holds(&packed)
}

#[test]
fn short_and_unrelated_objects_round_trip_and_empty_ones_have_no_delta() -> TestResult {
    assert_eq!(
        encode(b"", b"new"),
        None,
        "Git computes no delta against an empty source"
    );
    assert_eq!(encode(b"old", b""), None, "Git computes no delta of an empty target");
    let mut rng = Rng(3);
    let unrelated = (rng.bytes(3000), rng.bytes(2000));
    let cases: [(&[u8], &[u8]); 5] = [
        (b"short", b"shorter"),
        (b"0123456789abcdef", b"0123456789abcdefg"),
        (b"0123456789abcdefX", b"Y0123456789abcdef"),
        (
            b"0123456789abcdef0123456789abcdef",
            b"-0123456789abcdef0123456789abcdef-",
        ),
        (&unrelated.0, &unrelated.1),
    ];
    for (source, target) in cases {
        let delta = encode(source, target).expect("neither is empty");
        assert_pack_holds(&[Packed::whole(Blob, source), Packed::delta(Blob, target, 0, delta)])?;
    }
    Ok(())
}

#[test]
fn unrelated_objects_are_inserted_whole() {
    let mut rng = Rng(4);
    let (source, target) = (rng.bytes(3000), rng.bytes(2000));
    let delta = encode(&source, &target).expect("neither is empty");
    let header = 2 + 2;
    let inserts = target.len().div_ceil(127);
    assert_eq!(
        delta.len(),
        header + inserts + target.len(),
        "no 16 bytes of the target are in the source"
    );
}

#[test]
fn identical_objects_are_copied_64_kib_at_a_time() {
    let mut rng = Rng(5);
    let data = rng.bytes(100_000);
    let delta = encode(&data, &data).expect("not empty");
    assert_eq!(
        delta,
        [0xA0, 0x8D, 0x06, 0xA0, 0x8D, 0x06, 0x80, 0xB4, 0x01, 0xA0, 0x86],
        "two sizes of 100000 bytes, a copy of 0x10000 bytes from offset 0, and one of the remaining 34464 bytes"
    );
}

#[test]
fn repetitive_sources_are_matched() -> TestResult {
    let source = b"ab".repeat(20_000);
    let mut target = source.clone();
    target.splice(10_001..10_001, b"inserted".iter().copied());
    target.truncate(35_000);
    let delta = encode(&source, &target).expect("neither is empty");
    assert!(delta.len() < 40, "{} bytes of delta", delta.len());
    assert_pack_holds(&[Packed::whole(Blob, &source), Packed::delta(Blob, &target, 0, delta)])
}

#[test]
fn copies_from_offsets_past_16_mib_round_trip() -> TestResult {
    let mut rng = Rng(6);
    let source = rng.bytes((1 << 24) + 300_000);
    let index = Index::new(&source).expect("not empty");

    // The whole source is copied 64 KiB at a time, and the offsets from 2^24 on have four bytes.
    let mut longer = source.clone();
    longer.extend_from_slice(b"appended");
    let mut longer_delta = Vec::new();
    assert!(index.encode(&source, &longer, None, &mut longer_delta));

    let mut tail = source[(1 << 24) + 1000..].to_vec();
    tail.splice(5000..5010, b"inserted bytes".iter().copied());
    let mut tail_delta = Vec::new();
    assert!(index.encode(&source, &tail, None, &mut tail_delta));

    assert!(
        longer_delta.len() < 600 && tail_delta.len() < 100,
        "the deltas copy all but a few bytes: {} and {} bytes",
        longer_delta.len(),
        tail_delta.len()
    );
    assert_pack_holds(&[
        Packed::whole(Blob, &source),
        Packed::delta(Blob, &longer, 0, longer_delta),
        Packed::delta(Blob, &tail, 0, tail_delta),
    ])
}

#[test]
fn the_limit_stops_the_delta_the_first_time_that_it_is_exceeded() {
    let mut rng = Rng(7);
    let (start, source) = (rng.bytes(16), rng.bytes(100));
    let target = [start.as_slice(), &source].concat();
    let index = Index::new(&source).expect("not empty");
    let mut delta = Vec::new();
    assert!(index.encode(&source, &target, None, &mut delta));
    let mut expected = vec![100, 116, 16];
    expected.extend_from_slice(&start);
    expected.extend_from_slice(&[0x90, 100]);
    assert_eq!(
        delta, expected,
        "the first 16 bytes are inserted, and one copy takes over the bytes inserted after them"
    );

    // Until the copy is found, the first 16 bytes of the source are inserted too, and the delta has 35 bytes.
    let mut out = b"previous content".to_vec();
    assert!(
        index.encode(&source, &target, Some(35), &mut out),
        "the delta never has more than 35 bytes"
    );
    assert_eq!(out, expected, "the limit doesn't change the delta");
    assert!(!index.encode(&source, &target, Some(34), &mut out));
    assert!(out.is_empty(), "nothing of the delta is left");
    assert!(
        !index.encode(&source, &target, Some(expected.len()), &mut out),
        "as in Git, the delta is refused although it would have become short enough"
    );
}
