// Modified for gitoxide from the upstream imara-diff crate.
// Upstream source: git cat-file -p 32d1e45d3df061e6ccba6db7fdce92db29e345d8:src/postprocess.rs

use crate::intern::{InternedInput, Token};
use crate::slider_heuristic::SliderHeuristic;
use crate::util::{find_hunk_end, find_hunk_start};
use crate::{Algorithm, Diff, Hunk, myers};

impl Diff {
    /// Postprocesses the diff using explicit token sequences and a custom heuristic.
    ///
    /// This is a lower-level method that works directly with token sequences rather than
    /// an `InternedInput`. Use [`postprocess_with_heuristic`](Self::postprocess_with_heuristic)
    /// for a more convenient API.
    ///
    /// As Git's `xdl_change_compact()` does, the hunks of `before` are moved first, then those of
    /// `after`. In a diff computed with [`Algorithm::Histogram`], a hunk that moved or absorbed other
    /// hunks may now share tokens with the hunk it faces on the other side, so like Git, the two are
    /// diffed again with Myers algorithm.
    ///
    /// # Parameters
    ///
    /// * `before` - The token sequence from the first file, before changes
    /// * `after` - The token sequence from the second file, after changes
    /// * `heuristic` - The slider heuristic to use for positioning hunks
    pub fn postprocess_with(&mut self, before: &[Token], after: &[Token], mut heuristic: impl SliderHeuristic) {
        let rediff = self.algorithm == Algorithm::Histogram;
        // Compact removals first so ambiguous additions align with them, as in Git.
        Postprocessor {
            added: &mut self.removed,
            removed: &mut self.added,
            tokens: before,
            other_tokens: after,
            hunk: Hunk {
                before: 0..0,
                after: 0..0,
            },
            heuristic: &mut heuristic,
            rediff,
        }
        .run();
        Postprocessor {
            added: &mut self.added,
            removed: &mut self.removed,
            tokens: after,
            other_tokens: before,
            hunk: Hunk {
                before: 0..0,
                after: 0..0,
            },
            heuristic: &mut heuristic,
            rediff,
        }
        .run()
    }

    /// Postprocesses the diff using an `InternedInput` and a custom heuristic.
    ///
    /// This is a convenience wrapper around [`postprocess_with`](Self::postprocess_with)
    /// that extracts the token sequences from the input automatically.
    ///
    /// # Parameters
    ///
    /// * `input` - The interned input containing the token sequences
    /// * `heuristic` - The slider heuristic to use for positioning hunks
    pub fn postprocess_with_heuristic<T>(&mut self, input: &InternedInput<T>, heuristic: impl SliderHeuristic) {
        self.postprocess_with(&input.before, &input.after, heuristic);
    }
}

/// Internal state for postprocessing a diff to improve readability.
///
/// The hunks of one side are moved, and `hunk.after` is the range of the current one. `hunk.before`
/// is the range of the hunk it faces on the other side, which is empty unless that side changed
/// tokens there.
struct Postprocessor<'a, H> {
    /// Which tokens of `tokens` are changed; the hunks they form are moved.
    added: &'a mut [bool],
    /// Which tokens of `other_tokens` are changed.
    removed: &'a mut [bool],
    /// The token sequence whose hunks are moved.
    tokens: &'a [Token],
    /// The token sequence of the other side.
    other_tokens: &'a [Token],
    /// The current hunk being processed in the iteration.
    hunk: Hunk,
    /// The heuristic used to determine optimal hunk positions.
    heuristic: &'a mut H,
    /// Whether a hunk that moved or absorbed other hunks is diffed again with the hunk it faces.
    rediff: bool,
}

impl<H: SliderHeuristic> Postprocessor<'_, H> {
    fn run(mut self) {
        loop {
            // find a hunk
            if !self.hunk.next_hunk(self.removed, self.added) {
                break;
            }
            let original_hunk = self.hunk.after.clone();

            let mut earliest_end;
            let mut is_modification;
            loop {
                // move hunk up as far as possible to possibly merge it with other hunks
                // and discover if there are other possible positions
                while self.slide_up() {}
                earliest_end = self.hunk.after.end;
                is_modification = self.hunk.before.start != self.hunk.before.end;

                let hunk_size_unexpanded = self.hunk.after.len();
                // move hunk down as far as possible (and merge with other hunks it if
                // possible) sliding down is often the most preferred position
                while self.slide_down() {
                    is_modification |= self.hunk.before.start != self.hunk.before.end;
                }
                // if this hunk was merged with another hunk while sliding down we might
                // be able to slide up more otherwise we are done
                if hunk_size_unexpanded == self.hunk.after.len() {
                    break;
                }
            }

            if self.hunk.after.end != earliest_end {
                if is_modification {
                    // hunk can be moved and there is a removed hunk in the same region
                    // move the hunk so it align with the other hunk to produce a single
                    // MODIFIED hunk instead of two separate ADDED/REMOVED hunks
                    while self.hunk.before.start == self.hunk.before.end {
                        let success = self.slide_up();
                        debug_assert!(success);
                    }
                } else {
                    let slider_end = self
                        .heuristic
                        .best_slider_end(self.tokens, self.hunk.after.clone(), earliest_end);
                    for _ in slider_end..self.hunk.after.end {
                        let success = self.slide_up();
                        debug_assert!(success);
                    }
                }
            }

            if self.rediff && !self.hunk.before.is_empty() && self.hunk.after != original_hunk {
                self.rediff_hunk();
            }
        }
    }

    /// Diffs the current hunk again with the hunk it faces, as Git's `xdl_change_compact()` does for
    /// a histogram diff. Like Git, both are diffed with Myers algorithm as if they were whole files,
    /// the hunk being moved as the first one, and their tokens that match stay unchanged.
    fn rediff_hunk(&mut self) {
        let hunk = self.hunk.after.start as usize..self.hunk.after.end as usize;
        let other_hunk = self.hunk.before.start as usize..self.hunk.before.end as usize;
        let changed = &mut self.added[hunk.clone()];
        let other_changed = &mut self.removed[other_hunk.clone()];
        changed.fill(false);
        other_changed.fill(false);
        myers::diff(
            &self.tokens[hunk],
            &self.other_tokens[other_hunk],
            changed,
            other_changed,
            false,
        );
    }

    /// Slides a hunk down by one token/line, potentially merging it with a subsequent hunk.
    fn slide_down(&mut self) -> bool {
        let Some(&next_token) = self.tokens.get(self.hunk.after.end as usize) else {
            return false;
        };
        if self.tokens[self.hunk.after.start as usize] != next_token {
            return false;
        }
        self.added[self.hunk.after.start as usize] = false;
        self.added[self.hunk.after.end as usize] = true;
        self.hunk.after.start += 1;
        self.hunk.after.end = find_hunk_end(self.added, self.hunk.after.end);
        // move the end of the remove range one down to keep the unchanged lines aligned
        self.hunk.before.start = self.hunk.before.end + 1;
        self.hunk.before.end = find_hunk_end(self.removed, self.hunk.before.start);
        true
    }

    /// Slides a hunk up by one token/line, potentially merging it with a previous hunk.
    fn slide_up(&mut self) -> bool {
        if self.hunk.after.start == 0 {
            return false;
        }
        if self.tokens[self.hunk.after.start as usize - 1] != self.tokens[self.hunk.after.end as usize - 1] {
            return false;
        }
        self.added[self.hunk.after.start as usize - 1] = true;
        self.added[self.hunk.after.end as usize - 1] = false;
        self.hunk.after.end -= 1;
        self.hunk.after.start = find_hunk_start(self.added, self.hunk.after.start - 1);
        // move the start of the remove range one up to keep the unchanged lines aligned
        self.hunk.before.end = self.hunk.before.start - 1;
        self.hunk.before.start = find_hunk_start(self.removed, self.hunk.before.start - 1);
        true
    }
}
