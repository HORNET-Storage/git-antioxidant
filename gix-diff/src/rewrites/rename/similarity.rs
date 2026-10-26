//! The similarity estimate of `diffcore-delta.c`.

use std::collections::HashMap;

/// The number of hash values of spans, a prime that git chose between 2^16 and 2^17.
const HASH_BASE: u32 = 107_927;

/// The length after which a span ends if no line feed ended it before.
const MAX_SPAN_LEN: u64 = 64;

/// The spans of the data of a blob, summed up by their hash values.
///
/// Git cuts the data into spans, each of which ends after a line feed or after 64 bytes, and hashes each span. Bytes
/// that remain after the last span are a span too, as they are since git 2.44. Two files share the bytes of the spans
/// whose hash values they have in common, up to the number of these bytes that the other file has.
pub(crate) struct Spans {
    /// The total length of the spans with each hash value, ordered by the hash values.
    lengths_by_hash: Vec<(u32, u64)>,
}

impl Spans {
    /// Cut `data` into spans and hash them as `hash_chars()` does. In text, a carriage return that precedes a line feed
    /// is skipped, as if it weren't there. Of data of 4 GiB or more, only the first [`hashed_len()`] bytes are hashed.
    pub(crate) fn new(data: &[u8], is_text: bool) -> Self {
        let data = &data[..hashed_len(data.len())];
        let mut lengths = HashMap::<u32, u64>::new();
        let (mut accum1, mut accum2, mut len) = (0u32, 0u32, 0u64);
        let mut bytes = data.iter().copied().peekable();
        while let Some(byte) = bytes.next() {
            if is_text && byte == b'\r' && bytes.peek() == Some(&b'\n') {
                continue;
            }
            (accum1, accum2) = (
                ((accum1 << 7) ^ (accum2 >> 25)).wrapping_add(u32::from(byte)),
                (accum2 << 7) ^ (accum1 >> 25),
            );
            len += 1;
            if len == MAX_SPAN_LEN || byte == b'\n' {
                *lengths.entry(hash(accum1, accum2)).or_default() += len;
                (accum1, accum2, len) = (0, 0, 0);
            }
        }
        if len > 0 {
            *lengths.entry(hash(accum1, accum2)).or_default() += len;
        }
        let mut lengths_by_hash: Vec<_> = lengths.into_iter().collect();
        lengths_by_hash.sort_unstable_by_key(|&(hash, _)| hash);
        Spans { lengths_by_hash }
    }

    /// The number of bytes that `destination` has of these spans, which git counts as copied from the source to the
    /// destination: for each hash value, the smaller of the lengths that both have of it.
    pub(crate) fn copied_into(&self, destination: &Spans) -> u64 {
        let mut destination = destination.lengths_by_hash.iter().peekable();
        let mut copied = 0;
        for &(hash, len) in &self.lengths_by_hash {
            while destination.next_if(|&&(other, _)| other < hash).is_some() {}
            if let Some(&(_, other_len)) = destination.next_if(|&&(other, _)| other == hash) {
                copied += len.min(other_len);
            }
        }
        copied
    }
}

/// The number of bytes at the start of data of length `len` that git hashes. `hash_chars()` holds the length in an
/// `unsigned int`, so of data of 4 GiB or more, it hashes only the first `len % 2^32` bytes.
fn hashed_len(len: usize) -> usize {
    len as u32 as usize
}

/// The hash value of a span whose bytes were accumulated into `accum1` and `accum2`.
fn hash(accum1: u32, accum2: u32) -> u32 {
    accum1.wrapping_add(accum2.wrapping_mul(0x61)) % HASH_BASE
}

#[cfg(test)]
mod tests {
    use super::Spans;

    /// The lengths of the spans of `data` by their hash values.
    fn lengths(data: &[u8], is_text: bool) -> Vec<(u32, u64)> {
        Spans::new(data, is_text).lengths_by_hash
    }

    #[test]
    fn spans_are_hashed_as_git_hashes_them() {
        assert_eq!(lengths(b"a\n", true), [(12426, 2)], "the span ends with its line feed");
        assert_eq!(
            lengths(b"abcde\n", true),
            [(8874, 6)],
            "the bits that leave one accumulator enter the other"
        );
        assert_eq!(
            lengths(b"a\ny", true),
            [(121, 1), (12426, 2)],
            "the data ends the last span, and spans are ordered by their hash values"
        );
        assert!(lengths(b"", true).is_empty(), "empty data has no spans");
    }

    #[test]
    fn a_carriage_return_before_a_line_feed_counts_only_in_binary_data() {
        assert_eq!(
            lengths(b"a\r\n", true),
            [(12426, 2)],
            "in text, the carriage return is neither hashed nor counted"
        );
        assert_eq!(
            lengths(b"a\r\n", false),
            [(79944, 3)],
            "in binary data, the carriage return is part of the span"
        );
        assert_eq!(
            lengths(b"a\r", true),
            [(12429, 2)],
            "in text, a carriage return without a line feed after it is part of the span"
        );
    }

    #[test]
    fn a_span_ends_after_64_bytes_without_a_line_feed() {
        let spans = lengths(&[b'x'; 65], false);
        assert_eq!(spans.len(), 2, "the 65th byte starts another span");
        assert!(spans.contains(&(120, 1)), "the last span holds the 65th byte alone");
        assert!(
            spans.iter().any(|&(_, len)| len == 64),
            "the first span holds the first 64 bytes"
        );
    }

    #[test]
    fn spans_with_the_same_hash_value_are_summed_up() {
        assert_eq!(lengths(b"a\na\n", true), [(12426, 4)], "both spans count");
    }

    #[test]
    fn copied_bytes_are_the_smaller_length_of_each_shared_hash_value() {
        let source = Spans::new(b"a\nb\na\n", true);
        let destination = Spans::new(b"a\na\na\nc\n", true);
        assert_eq!(
            source.copied_into(&destination),
            4,
            "both spans `a` of the source were copied, but not its span `b`"
        );
        assert_eq!(
            destination.copied_into(&source),
            4,
            "two of the three spans `a` of the destination are in the source, but not its span `c`"
        );
        assert_eq!(
            Spans::new(b"", true).copied_into(&destination),
            0,
            "nothing is copied from empty data"
        );
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn of_data_of_4_gib_or_more_only_the_first_len_mod_2_pow_32_bytes_are_hashed() {
        use super::hashed_len;
        assert_eq!(
            hashed_len(u32::MAX as usize),
            u32::MAX as usize,
            "data of less than 4 GiB is hashed whole"
        );
        assert_eq!(hashed_len(1 << 32), 0, "of data of exactly 4 GiB, no byte is hashed");
        assert_eq!(
            hashed_len((1 << 32) + 3),
            3,
            "of data of 4 GiB and 3 bytes, the first 3 bytes are hashed"
        );
    }
}
