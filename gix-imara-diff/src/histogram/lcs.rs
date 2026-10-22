// Modified for gitoxide from the upstream imara-diff crate.
// Upstream source: git cat-file -p 32d1e45d3df061e6ccba6db7fdce92db29e345d8:src/histogram/lcs.rs

use crate::histogram::{Histogram, MAX_CHAIN_LEN};
use crate::intern::Token;

/// Finds the common subsequence to split `before` and `after` at, as `find_lcs()` in Git's
/// `xdiff/xhistogram.c` does.
///
/// The tokens of `after` are visited in order. Every occurrence in `before` of the visited token is
/// extended backward and forward over equal tokens, and the extended match becomes the split point
/// if it is longer than the current one, or if the rarest of its tokens occurs less often in
/// `before` than the rarest token of the current one. Tokens that occur more often in `before` than
/// that are passed over, and the visit continues behind the furthest match end in `after`.
pub(super) fn find_lcs(before: &[Token], after: &[Token], histogram: &mut Histogram) -> Split {
    let mut search = LcsSearch {
        lcs: None,
        extent: 0,
        min_occurrences: MAX_CHAIN_LEN + 1,
        found_cs: false,
    };
    search.run(before, after, histogram);
    if search.found_cs && search.min_occurrences > MAX_CHAIN_LEN {
        Split::FallBack
    } else if let Some(lcs) = search.lcs {
        Split::At(lcs)
    } else {
        Split::NothingInCommon
    }
}

/// How the histogram algorithm continues with a region, as decided by [`find_lcs`].
pub(super) enum Split {
    /// Split the region around this common subsequence.
    At(Lcs),
    /// The two sides have no token in common.
    NothingInCommon,
    /// Every token that both sides contain occurs more than [`MAX_CHAIN_LEN`] times in `before`,
    /// so the region is diffed with Myers algorithm instead.
    FallBack,
}

/// A common subsequence that the histogram algorithm splits a region at.
#[derive(Debug)]
pub(super) struct Lcs {
    /// Starting position in the "before" sequence.
    pub before_start: u32,
    /// Starting position in the "after" sequence.
    pub after_start: u32,
    /// Length of the common subsequence.
    pub len: u32,
}

/// State for searching for the common subsequence to split a region at.
struct LcsSearch {
    /// The best common subsequence found so far.
    lcs: Option<Lcs>,
    /// The length of `lcs` minus one, zero before anything is found as in Git, where a candidate
    /// of a single token replaces nothing unless its token is rarer.
    extent: u32,
    /// How often the rarest token of `lcs` occurs in `before`, starting at `MAX_CHAIN_LEN + 1`.
    min_occurrences: u32,
    /// Whether any token of `after` occurs in `before`.
    found_cs: bool,
}

impl LcsSearch {
    fn run(&mut self, before: &[Token], after: &[Token], histogram: &mut Histogram) {
        let mut pos = 0;
        while (pos as usize) < after.len() {
            pos = self.try_lcs(pos, before, after, histogram);
        }
        histogram.clear();
    }

    /// Tries every occurrence in `before` of the token at `after_pos`, as `try_lcs()` in Git's
    /// `xdiff/xhistogram.c` does, and returns the position in `after` to visit next.
    fn try_lcs(&mut self, after_pos: u32, before: &[Token], after: &[Token], histogram: &Histogram) -> u32 {
        let mut next_after_pos = after_pos + 1;
        let token = after[after_pos as usize];
        let token_occurrences = histogram.num_token_occurrences(token);
        if token_occurrences == 0 {
            return next_after_pos;
        }
        self.found_cs = true;
        if token_occurrences > self.min_occurrences {
            return next_after_pos;
        }

        let occurrences = histogram.token_occurrences(token);
        let mut next = 0;
        loop {
            let mut start1 = occurrences[next];
            next += 1;
            let mut start2 = after_pos;
            // The last tokens of the match, which includes them.
            let mut end1 = start1;
            let mut end2 = after_pos;
            let mut min_occurrences = token_occurrences;
            while start1 > 0 && start2 > 0 && before[start1 as usize - 1] == after[start2 as usize - 1] {
                start1 -= 1;
                start2 -= 1;
                min_occurrences = min_occurrences.min(histogram.num_token_occurrences(before[start1 as usize]));
            }
            while end1 as usize + 1 < before.len()
                && end2 as usize + 1 < after.len()
                && before[end1 as usize + 1] == after[end2 as usize + 1]
            {
                end1 += 1;
                end2 += 1;
                min_occurrences = min_occurrences.min(histogram.num_token_occurrences(before[end1 as usize]));
            }

            if next_after_pos <= end2 {
                next_after_pos = end2 + 1;
            }
            if self.extent < end1 - start1 || min_occurrences < self.min_occurrences {
                self.lcs = Some(Lcs {
                    before_start: start1,
                    after_start: start2,
                    len: end1 - start1 + 1,
                });
                self.extent = end1 - start1;
                self.min_occurrences = min_occurrences;
            }

            // Continue with the first occurrence behind this match in `before`.
            loop {
                match occurrences.get(next) {
                    None => return next_after_pos,
                    Some(&pos) if pos <= end1 => next += 1,
                    Some(_) => break,
                }
            }
        }
    }
}
