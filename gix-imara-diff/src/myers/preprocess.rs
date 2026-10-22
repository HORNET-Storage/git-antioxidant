// Modified for gitoxide from the upstream imara-diff crate.
// Upstream source: git cat-file -p 32d1e45d3df061e6ccba6db7fdce92db29e345d8:src/myers/preprocess.rs

use std::ops::Range;

use crate::intern::Token;
use crate::myers::sqrt;

/// Takes the tokens out of `region1` of `before` and `region2` of `after` that Git's
/// `xdl_cleanup_records()` keeps out of Myers algorithm, and marks them as changed: tokens that the
/// other file lacks, and frequent tokens surrounded by such tokens.
///
/// As in Git, how often a token occurs is counted over the whole other file, the limit for frequent
/// tokens comes from the length of the token's own file, and with `minimal` no token is frequent.
///
/// This optimization reduces the problem size for the Myers algorithm, improving performance
/// for files with many unique tokens.
pub fn preprocess(
    before: &[Token],
    after: &[Token],
    region1: Range<usize>,
    region2: Range<usize>,
    removed: &mut [bool],
    added: &mut [bool],
    minimal: bool,
) -> (PreprocessedFile, PreprocessedFile) {
    let (occurrences_before, occurrences_after) =
        token_occurrences(before, after, region1.clone(), region2.clone(), minimal);
    let file1 = PreprocessedFile::new(&occurrences_before, &before[region1.clone()], region1.start, removed);
    let file2 = PreprocessedFile::new(&occurrences_after, &after[region2.clone()], region2.start, added);
    (file1, file2)
}

/// Classifies the tokens in `region1` of `file1` and `region2` of `file2` by how often they occur
/// in the other file.
fn token_occurrences(
    file1: &[Token],
    file2: &[Token],
    region1: Range<usize>,
    region2: Range<usize>,
    minimal: bool,
) -> (Vec<Occurrences>, Vec<Occurrences>) {
    const MAX_EQLIMIT: u32 = 1024;

    // compute the limit after which tokens are treated as `Occurrences::COMMON`
    let eqlimit = |file: &[Token]| {
        if minimal {
            u32::MAX
        } else {
            sqrt(file.len()).min(MAX_EQLIMIT)
        }
    };
    let eqlimit1 = eqlimit(file1);
    let eqlimit2 = eqlimit(file2);

    let occurrences1 = count_occurrences(file1);
    let occurrences2 = count_occurrences(file2);
    let classify = |tokens: &[Token], other_occurrences: &[u32], eqlimit: u32| -> Vec<Occurrences> {
        tokens
            .iter()
            .map(|token| {
                let occurrences = other_occurrences.get(token.0 as usize).copied().unwrap_or(0);
                Occurrences::from_occurrences(occurrences, eqlimit)
            })
            .collect()
    };
    (
        classify(&file1[region1], &occurrences2, eqlimit1),
        classify(&file2[region2], &occurrences1, eqlimit2),
    )
}

/// Counts how often each token occurs in `file`, indexed by the token.
fn count_occurrences(file: &[Token]) -> Vec<u32> {
    let mut occurrences = Vec::new();
    for token in file {
        let bucket = token.0 as usize;
        if bucket >= occurrences.len() {
            occurrences.resize(bucket + 1, 0u32);
        }
        occurrences[bucket] += 1;
    }
    occurrences
}

/// Categorizes how frequently a token appears in a file.
#[derive(Clone, Copy, Debug)]
enum Occurrences {
    /// Token does not occur in the other file.
    None,
    /// Token occurs at least once in the other file.
    Some,
    /// Token occurs very frequently in the other file (exact threshold depends on file size).
    /// Such tokens are usually empty lines or braces and are often not meaningful to a diff.
    Common,
}

impl Occurrences {
    pub fn from_occurrences(occurrences: u32, eqlimit: u32) -> Occurrences {
        if occurrences == 0 {
            Occurrences::None
        } else if occurrences >= eqlimit {
            Occurrences::Common
        } else {
            Occurrences::Some
        }
    }
}

/// A file after preprocessing has removed unmatched tokens.
#[derive(Debug)]
pub struct PreprocessedFile {
    /// Maps from new token positions to original positions in the unpreprocessed file.
    pub indices: Vec<u32>,
    /// The tokens that remain after preprocessing.
    pub tokens: Vec<Token>,
}

impl PreprocessedFile {
    /// Keeps the tokens of `tokens`, which start at `offset` in their file, that take part in the
    /// search, and marks the others as changed in `changed`, which covers the whole file.
    fn new(
        token_occurrences: &[Occurrences],
        tokens: &[Token],
        offset: usize,
        changed: &mut [bool],
    ) -> PreprocessedFile {
        let (tokens, indices) = prune_unmatched_tokens(tokens, token_occurrences, offset, changed);
        PreprocessedFile { indices, tokens }
    }
}

fn prune_unmatched_tokens(
    file: &[Token],
    token_status: &[Occurrences],
    offset: usize,
    changed: &mut [bool],
) -> (Vec<Token>, Vec<u32>) {
    assert_eq!(token_status.len(), file.len());
    file.iter()
        .zip(token_status)
        .enumerate()
        .filter_map(|(i, (&token, &status))| {
            let prune = match status {
                Occurrences::None => true,
                Occurrences::Some => false,
                Occurrences::Common => should_prune_common_line(token_status, i),
            };
            let pos = offset + i;
            if prune {
                changed[pos] = true;
                None
            } else {
                Some((token, pos as u32))
            }
        })
        .unzip()
}

// TODO do not unnecessarily rescan lines
fn should_prune_common_line(token_status: &[Occurrences], pos: usize) -> bool {
    const WINDOW_SIZE: usize = 100;

    // Git's `xdl_clean_mmatch` starts both `rpdis0` and `rpdis1` at 1 and sums them,
    // so the line under test is counted twice. Here, the backward scan stops just
    // before the candidate so we initialize the count to 1 to account for it. In the
    // forward scan we'll start from the candidate line and count it there, leaving a
    // count of 2; matching `xdl_clean_mmatch`'s behavior.
    let mut unmatched_before = 0;
    let mut common_before = 1;

    let start = pos.saturating_sub(WINDOW_SIZE);
    for status in token_status[start..pos].iter().rev() {
        match status {
            Occurrences::None => {
                unmatched_before += 1;
            }
            Occurrences::Common => {
                common_before += 1;
            }
            Occurrences::Some => break,
        }
    }

    if unmatched_before == 0 {
        return false;
    }

    // The candidate line gets walked over and counted in this scan so the
    // count starts at 0 here. We also make sure to walk a full `WINDOW_SIZE`
    // lines after the candidate and not just walk `WINDOW_SIZE` in total.
    let end = token_status.len().min(pos + WINDOW_SIZE + 1);
    let mut unmatched_after = 0;
    let mut common_after = 0;
    for status in token_status[pos..end].iter() {
        match status {
            Occurrences::None => {
                unmatched_after += 1;
            }
            Occurrences::Common => {
                common_after += 1;
            }
            Occurrences::Some => break,
        }
    }

    if unmatched_after == 0 {
        return false;
    }

    let common = common_before + common_after;
    let unmatched = unmatched_before + unmatched_after;

    unmatched > 3 * common
}

#[cfg(test)]
mod tests {
    use super::{Occurrences, should_prune_common_line};

    #[test]
    fn common_line_pruning_ignores_distant_context() {
        let mut token_status = vec![Occurrences::Some; 700];
        token_status[100..400].fill(Occurrences::None);
        token_status[400..450].fill(Occurrences::None);
        token_status[450..500].fill(Occurrences::Common);
        token_status[500..550].fill(Occurrences::Common);
        token_status[550..600].fill(Occurrences::None);

        assert!(
            !should_prune_common_line(&token_status, 500),
            "only the last 100 items before the current line should contribute to the backward scan"
        );
    }

    #[test]
    fn candidate_line_counts_exactly_twice() {
        // `xdl_clean_mmatch` starts both frequent-line counters at 1 and sums them, so the
        // line under test contributes 2 and six unmatched lines around it is the boundary:
        // `6 > 3 * 2` is false and git keeps the line, seven discards it.
        let run = |before: usize, after: usize| {
            let mut token_status = vec![Occurrences::Some; 40];
            let pos = 20;
            token_status[pos - before..pos].fill(Occurrences::None);
            token_status[pos] = Occurrences::Common;
            token_status[pos + 1..=pos + after].fill(Occurrences::None);
            should_prune_common_line(&token_status, pos)
        };
        assert!(
            !run(3, 3),
            "six unmatched lines must keep the candidate: counting it twice gives `6 > 3 * 2`, which is false"
        );
        assert!(
            run(3, 4),
            "seven unmatched lines must prune the candidate: counting it twice gives `7 > 3 * 2`, which is true"
        );
    }

    #[test]
    fn forward_scan_reaches_a_hundred_lines_past_the_candidate() {
        // git's forward scan runs while `(i + r) <= i + XDL_SIMSCAN_WINDOW`, covering the
        // 100 lines after the candidate. Here the hundredth is what tips the ratio.
        let pos = 120;
        let mut token_status = vec![Occurrences::Some; 300];
        token_status[pos - 3..pos].fill(Occurrences::None);
        token_status[pos..pos + 25].fill(Occurrences::Common);
        token_status[pos + 25..=pos + 100].fill(Occurrences::None);

        assert!(
            should_prune_common_line(&token_status, pos),
            "the unmatched line 100 positions after the candidate must be counted to exceed the pruning threshold"
        );

        token_status[pos + 100] = Occurrences::Some;
        assert!(
            !should_prune_common_line(&token_status, pos),
            "without the hundredth unmatched line, the candidate must stay at the threshold and be kept"
        );
    }
}
