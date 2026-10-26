//! Pair files that were added with files that were deleted, modified or that didn't change, as `git diff -C` finds
//! copies and renames.
//!
//! With `-C`, diffcore-rename takes the files that were modified as sources as well, as they were before the change,
//! and with `--find-copies-harder`, or `-C -C`, also the files that didn't change. A source that is still there after
//! the change is copied to each destination paired with it, while of the destinations paired with a deleted source, the
//! last in the order of their locations is its rename and the others are its copies. [`detect()`] does that.

use gix_error::Result;

use super::{Blobs, Entry, MAX_SCORE, State, effective_min_score, limit_is_exceeded, sorted_files};

/// What happened to a file that is a [`Source`] of copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// The file was deleted, so a destination can be its rename as well as its copy.
    Deleted,
    /// The file was modified, or is otherwise accounted for, like a deleted file whose rename is known already, so
    /// destinations can only be its copies.
    Modified,
    /// The file didn't change, which makes it a source only for `git diff -C -C`, and destinations can only be its
    /// copies.
    Unchanged,
}

/// A file that destinations may have been copied or renamed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source<'a> {
    /// The file as it was before the change, which for a modified file is its old blob and mode.
    pub entry: Entry<'a>,
    /// What happened to the file.
    pub kind: SourceKind,
}

/// Options for [`detect()`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// The similarity, out of [`MAX_SCORE`], that a destination needs with a deleted source to be its rename unless the
    /// two are identical, with the meaning of [`super::Options::min_score`].
    pub rename_min_score: u32,
    /// The similarity, out of [`MAX_SCORE`], that a destination needs with any source to be its copy unless the two
    /// are identical, with the meaning of [`super::Options::min_score`]. `git diff -C<n>%` sets both to `n` percent.
    pub copy_min_score: u32,
    /// As [`super::Options::limit`], for all sources, as git doesn't leave out the sources of identical files when it
    /// looks for copies, or for the deleted sources that are still unpaired if `copy_min_score` takes identical files
    /// only.
    pub limit: usize,
}

impl Default for Options {
    /// Pair files that are at least 50% similar, and estimate similarities for up to 1000 sources and destinations, as
    /// `git diff -C` does.
    fn default() -> Self {
        Options {
            rename_min_score: MAX_SCORE / 2,
            copy_min_score: MAX_SCORE / 2,
            limit: 1000,
        }
    }
}

/// A source and a destination that were paired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pair {
    /// The index of the source among the `sources` passed to [`detect()`].
    pub source: usize,
    /// The index of the destination among the `destinations` passed to [`detect()`].
    pub destination: usize,
    /// The similarity of the two files, which is [`MAX_SCORE`] if they are identical.
    pub score: u32,
    /// Whether the destination is a copy of the source rather than its rename.
    pub copy: bool,
}

/// The result of [`detect()`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The sources and destinations that were paired, in the order of the locations of the destinations.
    pub pairs: Vec<Pair>,
    /// As [`super::Outcome::needed_limit`].
    pub needed_limit: Option<usize>,
    /// Whether [`Options::limit`] allowed the similarity estimates only without the sources that are
    /// [unchanged](SourceKind::Unchanged), which were then left out, as git warns that it "only found copies from
    /// modified paths due to too many files".
    pub unchanged_sources_left_out: bool,
    /// As [`super::Outcome::estimates`].
    pub estimates: usize,
    /// As [`super::Outcome::skipped_estimates`].
    pub skipped_estimates: usize,
}

/// Pair files that were added, the `destinations`, with the `sources` that they were copied or renamed from, as
/// `git diff -C` finds copies and renames among the changes between two trees. `blobs` provides the data of the files
/// whose similarity is estimated.
///
/// The steps are those of [`super::detect()`] without the one for file names, which git skips when it looks for copies,
/// and with these differences:
///
/// 1. A destination is paired with a source that has the same blob even if that source is paired already or isn't
///    [deleted](SourceKind::Deleted): a deleted source that is still unpaired is preferred, then a source with the
///    file name of the destination.
/// 2. [`Options::limit`] bounds all sources. If it is exceeded, but would allow the estimates without the sources that
///    are [unchanged](SourceKind::Unchanged), these are left out. If [`Options::copy_min_score`] takes identical files
///    only, which git can't be told as it uses one similarity for both, only the deleted sources that are still
///    unpaired are estimated and bounded, as only they can be paired by their similarity.
/// 3. Of the most similar pairs, first those that reach [`Options::rename_min_score`] are taken from the deleted
///    sources that are still unpaired, and then those that reach [`Options::copy_min_score`] from any source.
///
/// Then a destination whose source isn't deleted is a copy of it, while of the destinations paired with a deleted
/// source, the last in the order of the locations is its rename and the others are its copies.
pub fn detect(
    sources: &[Source<'_>],
    destinations: &[Entry<'_>],
    blobs: &mut dyn Blobs,
    options: Options,
) -> Result<Outcome> {
    let entries: Vec<_> = sources.iter().map(|source| source.entry).collect();
    let mut state = State::new(sorted_files(&entries), sorted_files(destinations), blobs);
    let kinds: Vec<_> = state.sources.iter().map(|file| sources[file.index].kind).collect();
    // Like git, which counts it in `rename_used`, take a source that remains as one in use from the start.
    for (in_use, kind) in state.source_is_paired.iter_mut().zip(&kinds) {
        *in_use = *kind != SourceKind::Deleted;
    }
    state.pair_identical(true);

    let mut outcome = Outcome::default();
    // A pass that pairs identical files only has nothing left to do.
    let passes: Vec<(u32, bool)> = [
        (effective_min_score(options.rename_min_score), false),
        (effective_min_score(options.copy_min_score), true),
    ]
    .into_iter()
    .filter(|&(min_score, _)| min_score < MAX_SCORE)
    .collect();
    // Without a pass for copies, only the deleted sources that are still unpaired can be paired.
    let copy_pass = passes.iter().any(|&(_, copies)| copies);
    let mut estimated_sources: Vec<usize> = (0..state.sources.len())
        .filter(|&source| copy_pass || !state.source_is_paired[source])
        .collect();
    let destinations = state.unpaired_destinations();
    if let Some(min_score) = passes.iter().map(|&(min_score, _)| min_score).min()
        && !estimated_sources.is_empty()
        && !destinations.is_empty()
    {
        if limit_is_exceeded(estimated_sources.len(), destinations.len(), options.limit) {
            let all = estimated_sources.len();
            outcome.needed_limit = Some(all.max(destinations.len()));
            let changed: Vec<_> = estimated_sources
                .iter()
                .copied()
                .filter(|&source| kinds[source] != SourceKind::Unchanged)
                .collect();
            if changed.len() < all && !limit_is_exceeded(changed.len(), destinations.len(), options.limit) {
                outcome.unchanged_sources_left_out = true;
                estimated_sources = changed;
            } else {
                estimated_sources.clear();
            }
            outcome.skipped_estimates = (all - estimated_sources.len()).saturating_mul(destinations.len());
        }
        if !estimated_sources.is_empty() {
            let candidates = state.estimate_candidates(min_score, &estimated_sources, &destinations)?;
            for (min_score, copies) in passes {
                for candidate in candidates
                    .iter()
                    .copied()
                    .map_while(|candidate| candidate.filter(|candidate| candidate.score >= min_score))
                {
                    if state.pairs[candidate.destination].is_none()
                        && (copies || !state.source_is_paired[candidate.source])
                    {
                        state.pair(candidate.source, candidate.destination, candidate.score, None);
                    }
                }
            }
        }
    }

    // Like `diff_resolve_rename_copy()`, go through the destinations in the order of their locations: each use of a
    // source that remains is a copy, and the last use of a deleted source is its rename.
    let mut uses = vec![0_usize; state.sources.len()];
    for &(source, _) in state.pairs.iter().flatten() {
        uses[source] += 1;
    }
    for (pair, destination) in state.pairs.iter().zip(&state.destinations) {
        let Some((source, score)) = *pair else {
            continue;
        };
        uses[source] -= 1;
        outcome.pairs.push(Pair {
            source: state.sources[source].index,
            destination: destination.index,
            score,
            copy: kinds[source] != SourceKind::Deleted || uses[source] > 0,
        });
    }
    outcome.estimates = state.estimates;
    Ok(outcome)
}
