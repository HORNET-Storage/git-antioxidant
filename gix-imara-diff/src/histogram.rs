// Modified for gitoxide from the upstream imara-diff crate.
// Upstream source: git cat-file -p 32d1e45d3df061e6ccba6db7fdce92db29e345d8:src/histogram.rs

use crate::histogram::lcs::{Split, find_lcs};
use crate::histogram::list_pool::{ListHandle, ListPool};
use crate::intern::Token;
use crate::myers;

mod lcs;
mod list_pool;

/// Git's `max_chain_length`: when every token that both sides of a region contain occurs more often
/// than this in `before`, the region is diffed with Myers algorithm instead.
const MAX_CHAIN_LEN: u32 = 64;

/// State for computing histogram-based diffs.
struct Histogram {
    /// Tracks where each token appears in the "before" sequence.
    token_occurrences: Vec<ListHandle>,
    /// Memory pool for efficiently storing occurrence lists.
    pool: ListPool,
}

/// Computes a diff using the histogram algorithm, as Git's `xdiff/xhistogram.c` does.
///
/// Like Git, the whole sequences are diffed, without first setting aside a common prefix or
/// postfix, as these tokens take part in choosing the subsequences the diff is split at.
///
/// # Parameters
///
/// * `before` - The token sequence from the first file, before changes.
/// * `after` - The token sequence from the second file, after changes.
/// * `removed` - Output array marking removed tokens
/// * `added` - Output array marking added tokens
/// * `num_tokens` - The total number of distinct tokens
pub fn diff(before: &[Token], after: &[Token], removed: &mut [bool], added: &mut [bool], num_tokens: u32) {
    let mut histogram = Histogram::new(num_tokens);
    histogram.run(before, after, removed, added);
}

impl Histogram {
    fn new(num_buckets: u32) -> Histogram {
        Histogram {
            token_occurrences: vec![ListHandle::default(); num_buckets as usize],
            pool: ListPool::new(2 * num_buckets),
        }
    }

    fn clear(&mut self) {
        self.pool.clear();
    }

    fn token_occurrences(&self, token: Token) -> &[u32] {
        self.token_occurrences[token.0 as usize].as_slice(&self.pool)
    }

    fn num_token_occurrences(&self, token: Token) -> u32 {
        self.token_occurrences[token.0 as usize].len(&self.pool)
    }

    fn populate(&mut self, file: &[Token]) {
        for (i, &token) in file.iter().enumerate() {
            self.token_occurrences[token.0 as usize].push(i as u32, &mut self.pool);
        }
    }

    fn run(&mut self, mut before: &[Token], mut after: &[Token], mut removed: &mut [bool], mut added: &mut [bool]) {
        loop {
            if before.is_empty() {
                added.fill(true);
                return;
            } else if after.is_empty() {
                removed.fill(true);
                return;
            }

            self.populate(before);
            match find_lcs(before, after, self) {
                Split::NothingInCommon => {
                    added.fill(true);
                    removed.fill(true);
                    return;
                }
                Split::At(lcs) => {
                    self.run(
                        &before[..lcs.before_start as usize],
                        &after[..lcs.after_start as usize],
                        &mut removed[..lcs.before_start as usize],
                        &mut added[..lcs.after_start as usize],
                    );

                    // this is equivalent to (tail) recursion but implement as a loop for efficiency reasons
                    let before_end = lcs.before_start + lcs.len;
                    before = &before[before_end as usize..];
                    removed = &mut removed[before_end as usize..];

                    let after_end = lcs.after_start + lcs.len;
                    after = &after[after_end as usize..];
                    added = &mut added[after_end as usize..];
                }
                Split::FallBack => {
                    // Like Git, diff the region with Myers algorithm as if its two sides were
                    // whole files, to avoid the quadratic cost of tokens that repeat this often.
                    myers::diff(before, after, removed, added, false);
                    return;
                }
            }
        }
    }
}
