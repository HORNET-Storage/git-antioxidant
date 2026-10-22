// Modified for gitoxide from the upstream imara-diff crate.
// Upstream source: git cat-file -p 32d1e45d3df061e6ccba6db7fdce92db29e345d8:src/myers/middle_snake.rs

use std::ptr::NonNull;

use crate::myers::slice::FileSlice;
use crate::util::{common_postfix, common_prefix};

/// Minimum snake length to be considered a "good" snake for heuristics.
const SNAKE_CNT: u32 = 20;
/// Heuristic multiplier used to evaluate snake quality.
const K_HEUR: u32 = 4;

/// Performs forward or backward search for the middle snake in Myers' algorithm.
///
/// The `BACK` const generic parameter determines the search direction:
/// `false` for forward search, `true` for backward search.
#[derive(Debug)]
pub struct MiddleSnakeSearch<const BACK: bool> {
    /// Pointer to the k-vector storage for this search direction.
    kvec: NonNull<i32>,
    /// Minimum k-diagonal currently being searched.
    kmin: i32,
    /// Maximum k-diagonal currently being searched.
    kmax: i32,
    /// Minimum possible k-diagonal value.
    dmin: i32,
    /// Maximum possible k-diagonal value.
    dmax: i32,
}

impl<const BACK: bool> MiddleSnakeSearch<BACK> {
    /// # Safety
    /// `data` must be valid for reads and writes between `-file2.len() - 1` and `file1.len() + 1`
    pub unsafe fn new(data: NonNull<i32>, file1: &FileSlice, file2: &FileSlice) -> Self {
        let dmin = -(file2.len() as i32);
        let dmax = file1.len() as i32;
        let kmid = if BACK { dmin + dmax } else { 0 };
        let mut res = Self {
            kvec: data,
            kmin: kmid,
            kmax: kmid,
            dmin,
            dmax,
        };
        let init = if BACK { file1.len() as i32 } else { 0 };
        res.write_xpos_at_diagonal(kmid, init);
        res
    }

    pub fn contains(&self, k: i32) -> bool {
        (self.kmin..=self.kmax).contains(&k)
    }

    pub fn bounds_check(&self, k: i32) {
        debug_assert!((self.dmin - 1..=self.dmax + 1).contains(&k));
    }

    fn write_xpos_at_diagonal(&mut self, k: i32, token_idx1: i32) {
        self.bounds_check(k);
        unsafe { self.kvec.as_ptr().offset(k as isize).write(token_idx1) }
    }

    pub fn x_pos_at_diagonal(&self, diagonal: i32) -> i32 {
        self.bounds_check(diagonal);
        unsafe { self.kvec.as_ptr().offset(diagonal as isize).read() }
    }

    pub fn pos_at_diagonal(&self, diagonal: i32) -> (i32, i32) {
        self.bounds_check(diagonal);
        let token_idx1 = unsafe { self.kvec.as_ptr().offset(diagonal as isize).read() };
        let token_idx2 = token_idx1 - diagonal;
        (token_idx1, token_idx2)
    }

    /// We need to extend the diagonal "domain" by one. If the next
    /// values exits the box boundaries we need to change it in the
    /// opposite direction because (max - min) must be a power of
    /// two.
    ///
    /// Also we initialize the external K value to -1 so that we can
    /// avoid extra conditions in the check inside the core loop.
    pub fn next_d(&mut self) {
        let init_val = if BACK {
            // value should always be larger then bounds
            i32::MAX
        } else {
            // value should always be smaller then bounds
            i32::MIN
        };

        if self.kmin > self.dmin {
            self.kmin -= 1;
            self.write_xpos_at_diagonal(self.kmin - 1, init_val);
        } else {
            self.kmin += 1;
        }

        if self.kmax < self.dmax {
            self.kmax += 1;
            self.write_xpos_at_diagonal(self.kmax + 1, init_val);
        } else {
            self.kmax -= 1;
        }
    }

    pub fn run(
        &mut self,
        file1: &FileSlice,
        file2: &FileSlice,
        mut f: impl FnMut(i32, i32) -> bool,
    ) -> Option<SearchResult> {
        let mut res = None;
        let mut k = self.kmax;
        while k >= self.kmin {
            #[cfg(test)]
            cov_mark::hit!(SPLIT_SEARCH_ITER);
            let mut token_idx1 = if BACK {
                if self.x_pos_at_diagonal(k - 1) < self.x_pos_at_diagonal(k + 1) {
                    self.x_pos_at_diagonal(k - 1)
                } else {
                    self.x_pos_at_diagonal(k + 1) - 1
                }
            } else if self.x_pos_at_diagonal(k - 1) >= self.x_pos_at_diagonal(k + 1) {
                self.x_pos_at_diagonal(k - 1) + 1
            } else {
                self.x_pos_at_diagonal(k + 1)
            };

            let mut token_idx2 = token_idx1 - k;
            let off = if BACK {
                if token_idx1 > 0 && token_idx2 > 0 {
                    let tokens1 = &file1.tokens[..token_idx1 as usize];
                    let tokens2 = &file2.tokens[..token_idx2 as usize];
                    common_postfix(tokens1, tokens2)
                } else {
                    0
                }
            } else if token_idx1 < file1.len() as i32 && token_idx2 < file2.len() as i32 {
                let tokens1 = &file1.tokens[token_idx1 as usize..];
                let tokens2 = &file2.tokens[token_idx2 as usize..];
                common_prefix(tokens1, tokens2)
            } else {
                0
            };

            if off > SNAKE_CNT {
                res = Some(SearchResult::Snake)
            }

            if BACK {
                token_idx1 -= off as i32;
                token_idx2 -= off as i32;
            } else {
                token_idx1 += off as i32;
                token_idx2 += off as i32;
            }
            self.write_xpos_at_diagonal(k, token_idx1);

            if f(k, token_idx1) {
                return Some(SearchResult::Found { token_idx1, token_idx2 });
            }

            k -= 2;
        }

        res
    }

    pub fn best_position(&self, file1: &FileSlice, file2: &FileSlice) -> (isize, i32) {
        let mut best_distance: isize = if BACK { isize::MAX } else { -1 };
        let mut best_token_idx1 = if BACK { i32::MAX } else { -1 };
        let mut k = self.kmax;
        while k >= self.kmin {
            let mut token_idx1 = self.x_pos_at_diagonal(k);
            if BACK {
                token_idx1 = token_idx1.max(0);
            } else {
                token_idx1 = token_idx1.min(file1.len() as i32);
            }
            let mut token_idx2 = token_idx1 - k;
            if BACK {
                if token_idx2 < 0 {
                    token_idx1 = k;
                    token_idx2 = 0;
                }
            } else if token_idx2 > file2.len() as i32 {
                token_idx1 = file2.len() as i32 + k;
                token_idx2 = file2.len() as i32;
            }

            let distance = token_idx1 as isize + token_idx2 as isize;
            if BACK && distance < best_distance || !BACK && distance > best_distance {
                best_distance = distance;
                best_token_idx1 = token_idx1;
            }

            k -= 2;
        }
        (best_distance, best_token_idx1)
    }

    /// Picks the furthest-reaching path to stop the search at, as the heuristic in Git's
    /// `xdl_split()` does. A path scores the tokens it covered minus its distance from the middle
    /// diagonal of its search. Among the paths that score more than `K_HEUR * ec` and whose end is
    /// preceded, or for the backward search followed, by `SNAKE_CNT` tokens that are equal on both
    /// sides, the best-scoring one wins.
    pub fn found_snake(&self, ec: u32, file1: &FileSlice, file2: &FileSlice) -> Option<(i32, i32)> {
        let len1 = file1.len() as i32;
        let len2 = file2.len() as i32;
        let snake_cnt = SNAKE_CNT as i32;
        let middle_diagonal = if BACK { len1 - len2 } else { 0 };
        let mut best_score = 0;
        let mut best = None;
        let mut k = self.kmax;
        while k >= self.kmin {
            let (token_idx1, token_idx2) = self.pos_at_diagonal(k);
            let distance = if BACK {
                i64::from(len1 - token_idx1) + i64::from(len2 - token_idx2)
            } else {
                i64::from(token_idx1) + i64::from(token_idx2)
            };
            let score = distance - i64::from((k - middle_diagonal).unsigned_abs());
            let in_bounds = if BACK {
                0 < token_idx1 && token_idx1 <= len1 - snake_cnt && 0 < token_idx2 && token_idx2 <= len2 - snake_cnt
            } else {
                snake_cnt <= token_idx1 && token_idx1 < len1 && snake_cnt <= token_idx2 && token_idx2 < len2
            };
            if score > i64::from(K_HEUR * ec) && score > best_score && in_bounds {
                let is_snake = if BACK {
                    (0..snake_cnt)
                        .all(|i| file1.tokens[(token_idx1 + i) as usize] == file2.tokens[(token_idx2 + i) as usize])
                } else {
                    (1..=snake_cnt)
                        .all(|i| file1.tokens[(token_idx1 - i) as usize] == file2.tokens[(token_idx2 - i) as usize])
                };
                if is_snake {
                    best = Some((token_idx1, token_idx2));
                    best_score = score;
                }
            }

            k -= 2;
        }

        best
    }
}

/// The result of a middle snake search iteration.
#[derive(Debug)]
pub enum SearchResult {
    /// A good snake was found but not necessarily the middle snake.
    Snake,
    /// The middle snake was found at the specified token positions.
    Found {
        /// Token index in the first sequence.
        token_idx1: i32,
        /// Token index in the second sequence.
        token_idx2: i32,
    },
}
