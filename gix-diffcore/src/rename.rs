//! Pair files that were deleted with files that were added, as `git diff -M` does, and as a merge does for each of its
//! sides ([`merge`]).

use std::{cmp::Ordering, collections::HashMap};

use bstr::{BStr, ByteSlice};
use gix_error::{Result, ResultExt, message};
use gix_hash::oid;
use gix_object::tree::EntryMode;

use crate::similarity::Spans;

pub mod merge;

/// The similarity of identical files, of which [`Options::min_score`] and [`Rename::score`] are fractions.
///
/// Git shows a similarity as the percentage `score * 100 / MAX_SCORE`, rounded down.
pub const MAX_SCORE: u32 = 60_000;

/// How many of its most similar sources are kept for each destination while similarities are estimated.
const CANDIDATES_PER_DESTINATION: usize = 4;

/// How many sources with the blob of a destination are compared with it before one of them is taken.
const MAX_IDENTICAL_CANDIDATES: usize = 100;

/// How many bytes at the start of a blob [`Content::from_data()`] checks for a NUL byte.
const BINARY_CHECK_LEN: usize = 8000;

/// A file that was deleted or added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The path of the file, relative to the root of its tree.
    pub location: &'a BStr,
    /// The mode of the file.
    pub mode: EntryMode,
    /// The id of its blob, or that of the commit of a submodule.
    pub id: &'a oid,
}

/// How git treats the data of a blob when it estimates the similarity of files.
///
/// Git decides by the `diff` attribute of a file: `-diff` makes it binary and `diff` makes it text, and a named diff
/// driver decides with its `binary` setting if that is set. Otherwise, [`Content::from_data()`] decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// Text, in which a carriage return that precedes a line feed doesn't count.
    Text,
    /// Binary data, all of whose bytes count.
    Binary,
}

impl Content {
    /// Classify `data` as git does when no attribute decides: it is binary if a NUL byte appears among its first 8000
    /// bytes.
    pub fn from_data(data: &[u8]) -> Self {
        if data[..data.len().min(BINARY_CHECK_LEN)].contains(&0) {
            Content::Binary
        } else {
            Content::Text
        }
    }
}

/// Provides the blobs of the files whose similarity is estimated.
pub trait Blobs {
    /// Return the size of the blob of `entry` in bytes, which is the length of the data that [`Self::content()`]
    /// returns.
    fn size(&mut self, entry: &Entry<'_>) -> Result<u64>;

    /// Return the data of the blob of `entry`, for which the empty `buf` may be used, along with how git treats it.
    fn content<'buf>(&mut self, entry: &Entry<'_>, buf: &'buf mut Vec<u8>) -> Result<(&'buf [u8], Content)>;
}

/// Options for [`detect()`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// The similarity, out of [`MAX_SCORE`], that a destination needs with a source to be paired with it unless the two
    /// are identical. `git diff -M<n>%` sets it to `n` percent.
    ///
    /// 0 means half of [`MAX_SCORE`], as it does in git, and [`MAX_SCORE`] or more pairs identical files only.
    pub min_score: u32,
    /// Git's `diff.renameLimit`: similarities aren't estimated if the number of sources times the number of
    /// destinations that identical files and unique file names left unpaired exceeds the square of this limit.
    /// 0 means no limit.
    pub limit: usize,
}

impl Default for Options {
    /// Pair files that are at least 50% similar, and estimate similarities for up to 1000 sources and destinations, as
    /// `git diff -M` does.
    fn default() -> Self {
        Options {
            min_score: MAX_SCORE / 2,
            limit: 1000,
        }
    }
}

/// A source and a destination that were paired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rename {
    /// The index of the source among the `sources` passed to [`detect()`].
    pub source: usize,
    /// The index of the destination among the `destinations` passed to [`detect()`].
    pub destination: usize,
    /// The similarity of the two files, which is [`MAX_SCORE`] if they are identical.
    pub score: u32,
}

/// The result of [`detect()`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The sources and destinations that were paired, in the order of the locations of the destinations.
    pub renames: Vec<Rename>,
    /// If [`Options::limit`] prevented the similarity estimates, the limit that would have allowed them, which
    /// `git diff` suggests in its warning.
    pub needed_limit: Option<usize>,
}

/// Pair files that were added, the `destinations`, with files that were deleted, the `sources`, as `git diff -M` finds
/// renames among the changes between two trees. `blobs` provides the data of the files whose similarity is estimated.
///
/// Like git, this pairs files in three steps, each of which considers only the files that earlier steps left unpaired:
///
/// 1. Each destination, in the order of the locations, is paired with a source that has the same blob, preferring a
///    source with the same file name, the part of the location after its last `/`. Regular files are paired whether or
///    not they are executable, while other files need the same mode. Of equally good sources, the one that git's hash
///    table returns first is taken, and at most 100 of them are compared.
/// 2. A source and a destination with the same file name are paired if no other source or destination has that file
///    name and their similarity is at least halfway from [`Options::min_score`] to [`MAX_SCORE`].
/// 3. Unless [`Options::limit`] prevents it, the similarity of each source and destination is estimated, and the four
///    most similar sources of each destination are kept. Then pairs are taken from the most similar down to
///    [`Options::min_score`], preferring pairs whose file names are the same and otherwise in the order git takes them.
///
/// The similarity of two regular files is the number of bytes that the destination has of the source, out of the size
/// of the larger file and scaled to [`MAX_SCORE`]. The data of each file is cut into spans that end after a line feed
/// or after 64 bytes, and the files share the bytes of the spans whose hashes they have in common, up to the number of
/// these bytes that the other file has. Bytes that remain after the last span are a span too, as they are since git
/// 2.44. In text, a carriage return that precedes a line feed doesn't count. Files whose sizes differ too much to
/// reach the required similarity aren't read, and files that aren't both regular files are never similar.
///
/// Trees among `sources` and `destinations` are ignored, the files may be passed in any order, and each location should
/// appear only once among the sources and once among the destinations. Like git, this hashes only the first
/// `size % 2^32` bytes of a blob of 4 GiB or more.
pub fn detect(
    sources: &[Entry<'_>],
    destinations: &[Entry<'_>],
    blobs: &mut dyn Blobs,
    options: Options,
) -> Result<Outcome> {
    let min_score = effective_min_score(options.min_score);
    let mut state = State::new(sorted_files(sources), sorted_files(destinations), blobs);
    state.pair_identical();
    if min_score == MAX_SCORE {
        return Ok(state.into_outcome(None));
    }
    state.pair_file_names(basename_min_score(min_score), &|_| true, None)?;
    let sources = state.unpaired_sources();
    let needed_limit = state.pair_similar(min_score, options.limit, &sources, None)?;
    Ok(state.into_outcome(needed_limit))
}

/// A source or a destination, along with what was learned about its blob.
struct File<'a> {
    /// The index of the file among those passed to [`detect()`].
    index: usize,
    entry: Entry<'a>,
    /// The size of the blob, once it is known.
    size: Option<u64>,
    /// The spans of the data of the blob, once they are hashed.
    spans: Option<Spans>,
}

impl File<'_> {
    fn size(&mut self, blobs: &mut dyn Blobs) -> Result<u64> {
        if let Some(size) = self.size {
            return Ok(size);
        }
        let size = blobs
            .size(&self.entry)
            .or_raise(|| message!("Could not read the size of the blob of {}", self.entry.location))?;
        Ok(*self.size.insert(size))
    }

    fn spans(&mut self, blobs: &mut dyn Blobs, buf: &mut Vec<u8>) -> Result<&Spans> {
        let spans = match self.spans.take() {
            Some(spans) => spans,
            None => {
                buf.clear();
                let (data, content) = blobs
                    .content(&self.entry, buf)
                    .or_raise(|| message!("Could not read the blob of {}", self.entry.location))?;
                Spans::new(data, content == Content::Text)
            }
        };
        Ok(self.spans.insert(spans))
    }
}

/// The similarity that a pair needs for a `min_score` option: 0 stands for half of [`MAX_SCORE`], as git's
/// `DEFAULT_RENAME_SCORE` does, and more than [`MAX_SCORE`] for [`MAX_SCORE`].
fn effective_min_score(min_score: u32) -> u32 {
    match min_score {
        0 => MAX_SCORE / 2,
        min_score => min_score.min(MAX_SCORE),
    }
}

/// The similarity that a source and a destination with the same file name need to be paired by their file name:
/// halfway from `min_score` to [`MAX_SCORE`], as `diffcore_rename_extended()` computes it.
fn basename_min_score(min_score: u32) -> u32 {
    min_score + (MAX_SCORE - min_score) / 2
}

/// The files among `entries` that aren't trees and that `keep` accepts, in the order of `entries`.
fn files<'a>(entries: &[Entry<'a>], keep: impl Fn(&Entry<'a>) -> bool) -> Vec<File<'a>> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| !entry.mode.is_tree() && keep(entry))
        .map(|(index, &entry)| File {
            index,
            entry,
            size: None,
            spans: None,
        })
        .collect()
}

/// The files among `entries` that aren't trees, in the order of their locations.
fn sorted_files<'a>(entries: &[Entry<'a>]) -> Vec<File<'a>> {
    let mut files = files(entries, |_| true);
    files.sort_by_key(|file| file.entry.location);
    files
}

/// The files that are paired so far, along with what is needed to estimate their similarity.
struct State<'a, 'b> {
    /// The files that were deleted, in the order in which git considers them.
    sources: Vec<File<'a>>,
    /// The files that were added, in the order in which git considers them.
    destinations: Vec<File<'a>>,
    /// The source that each destination was paired with, and the similarity of the two.
    pairs: Vec<Option<(usize, u32)>>,
    /// Whether each source was paired with a destination.
    source_is_paired: Vec<bool>,
    blobs: &'b mut dyn Blobs,
    /// The buffer that the data of blobs is read into.
    buf: Vec<u8>,
}

impl<'a, 'b> State<'a, 'b> {
    fn new(sources: Vec<File<'a>>, destinations: Vec<File<'a>>, blobs: &'b mut dyn Blobs) -> Self {
        State {
            pairs: vec![None; destinations.len()],
            source_is_paired: vec![false; sources.len()],
            sources,
            destinations,
            blobs,
            buf: Vec::new(),
        }
    }

    /// Pair each destination with a source that has the same blob, as `find_exact_renames()` does.
    fn pair_identical(&mut self) {
        let mut sources_by_id = gix_hashtable::HashMap::<&oid, Vec<usize>>::default();
        for (source, file) in self.sources.iter().enumerate() {
            sources_by_id.entry(file.entry.id).or_default().push(source);
        }
        // Git's table of sources returns those with the same blob in the order of the sources, unless it grew when the
        // last source was added, which reversed that order.
        if git_table_grows_with_last_insertion(self.sources.len()) {
            for sources in sources_by_id.values_mut() {
                sources.reverse();
            }
        }

        for destination in 0..self.destinations.len() {
            let target = self.destinations[destination].entry;
            let Some(candidates) = sources_by_id.get(target.id) else {
                continue;
            };
            let target_name = file_name(target.location);
            let mut best = None;
            for &source in candidates
                .iter()
                .filter(|&&source| {
                    !self.source_is_paired[source] && modes_can_pair(self.sources[source].entry.mode, target.mode)
                })
                .take(MAX_IDENTICAL_CANDIDATES)
            {
                if file_name(self.sources[source].entry.location) == target_name {
                    best = Some(source);
                    break;
                }
                best.get_or_insert(source);
            }
            if let Some(source) = best {
                self.pair(source, destination, MAX_SCORE, None);
            }
        }
    }

    /// Pair sources with destinations of the same file name if their similarity reaches `min_score`, as
    /// `find_basename_matches()` does. Each unpaired source for which `wanted` holds is paired with the only unpaired
    /// destination of its file name if no other unpaired source has that name, and otherwise with the destination of
    /// that name in the directory that `directories` suggests for its directory, if there is one. `directories`
    /// counts each pair.
    fn pair_file_names(
        &mut self,
        min_score: u32,
        wanted: &dyn Fn(usize) -> bool,
        mut directories: Option<&mut merge::Directories<'a, '_>>,
    ) -> Result {
        let source_names = unique_file_names(
            self.sources
                .iter()
                .enumerate()
                .filter(|&(source, _)| !self.source_is_paired[source])
                .map(|(source, file)| (source, file.entry)),
        );
        let destination_names = unique_file_names(
            self.destinations
                .iter()
                .enumerate()
                .filter(|&(destination, _)| self.pairs[destination].is_none())
                .map(|(destination, file)| (destination, file.entry)),
        );

        for source in 0..self.sources.len() {
            if self.source_is_paired[source] || !wanted(source) {
                continue;
            }
            let location = self.sources[source].entry.location;
            let name = file_name(location);
            let Some(&unique_destination) = destination_names.get(name) else {
                continue;
            };
            let destination = match (source_names.get(name), unique_destination) {
                (Some(&Some(_)), Some(destination)) => Some(destination),
                _ => directories
                    .as_deref()
                    .and_then(|directories| directories.suggested_destination(location)),
            };
            let Some(destination) = destination.filter(|&destination| self.pairs[destination].is_none()) else {
                continue;
            };
            let score = self.estimate(source, destination, min_score)?;
            if score >= min_score {
                self.pair(source, destination, score, directories.as_deref_mut());
            }
        }
        Ok(())
    }

    /// Estimate the similarity of each of `sources` with each unpaired destination, keep the most similar sources of
    /// each destination, and pair the most similar ones first, as the end of `diffcore_rename_extended()` does, with
    /// `directories` counting each pair. If `limit` prevents the estimates, return the limit that would have allowed
    /// them instead.
    fn pair_similar(
        &mut self,
        min_score: u32,
        limit: usize,
        sources: &[usize],
        mut directories: Option<&mut merge::Directories<'a, '_>>,
    ) -> Result<Option<usize>> {
        let destinations: Vec<_> = (0..self.destinations.len())
            .filter(|&destination| self.pairs[destination].is_none())
            .collect();
        if sources.is_empty() || destinations.is_empty() {
            return Ok(None);
        }
        if limit > 0 && (sources.len() as u128) * (destinations.len() as u128) > (limit as u128).pow(2) {
            return Ok(Some(sources.len().max(destinations.len())));
        }

        let mut candidates = Vec::with_capacity(destinations.len() * CANDIDATES_PER_DESTINATION);
        for &destination in &destinations {
            let destination_name = file_name(self.destinations[destination].entry.location);
            let mut best = [None; CANDIDATES_PER_DESTINATION];
            // Like git, this offers every source, even one that isn't similar at all, as it takes a place that a more
            // similar source can take later, which decides the order of equally similar ones.
            for &source in sources {
                let candidate = Candidate {
                    source,
                    destination,
                    score: self.estimate(source, destination, min_score)?,
                    same_file_name: file_name(self.sources[source].entry.location) == destination_name,
                };
                keep_if_better(&mut best, candidate);
            }
            candidates.extend(best);
        }
        // Like git's, this sort is stable, which keeps equally good candidates in the order of their destinations and
        // of their places among the candidates of a destination.
        candidates.sort_by(|a, b| by_preference(a.as_ref(), b.as_ref()));
        for candidate in candidates
            .into_iter()
            .map_while(|candidate| candidate.filter(|candidate| candidate.score >= min_score))
        {
            if self.pairs[candidate.destination].is_none() && !self.source_is_paired[candidate.source] {
                self.pair(
                    candidate.source,
                    candidate.destination,
                    candidate.score,
                    directories.as_deref_mut(),
                );
            }
        }
        Ok(None)
    }

    /// Estimate the similarity of `source` and `destination` as `estimate_similarity()` does, without reading files
    /// whose sizes differ too much to reach `min_score`.
    fn estimate(&mut self, source: usize, destination: usize, min_score: u32) -> Result<u32> {
        let (source, destination) = (&mut self.sources[source], &mut self.destinations[destination]);
        if !source.entry.mode.is_blob() || !destination.entry.mode.is_blob() {
            return Ok(0);
        }
        let source_size = source.size(self.blobs)?;
        let destination_size = destination.size(self.blobs)?;
        let max_size = source_size.max(destination_size);
        let delta_size = max_size - source_size.min(destination_size);
        // Git computes this, like the similarity, with floating-point numbers.
        if max_size as f64 * f64::from(MAX_SCORE - min_score) < delta_size as f64 * f64::from(MAX_SCORE) {
            return Ok(0);
        }
        if destination_size == 0 {
            return Ok(0);
        }
        let copied = source
            .spans(self.blobs, &mut self.buf)?
            .copied_into(destination.spans(self.blobs, &mut self.buf)?);
        Ok((copied as f64 * f64::from(MAX_SCORE) / max_size as f64) as u32)
    }

    /// Pair `source` with `destination`, and count the rename in `directories`.
    fn pair(
        &mut self,
        source: usize,
        destination: usize,
        score: u32,
        directories: Option<&mut merge::Directories<'a, '_>>,
    ) {
        self.source_is_paired[source] = true;
        self.pairs[destination] = Some((source, score));
        if let Some(directories) = directories {
            directories.count(
                self.sources[source].entry.location,
                self.destinations[destination].entry.location,
            );
        }
    }

    /// The sources that aren't paired yet, in their order.
    fn unpaired_sources(&self) -> Vec<usize> {
        (0..self.sources.len())
            .filter(|&source| !self.source_is_paired[source])
            .collect()
    }

    fn into_outcome(self, needed_limit: Option<usize>) -> Outcome {
        let renames = self
            .pairs
            .iter()
            .zip(&self.destinations)
            .filter_map(|(pair, destination)| {
                pair.map(|(source, score)| Rename {
                    source: self.sources[source].index,
                    destination: destination.index,
                    score,
                })
            })
            .collect();
        Outcome { renames, needed_limit }
    }
}

/// Whether git's table of `count` sources grows when the last of them is added. `hashmap_init()` makes room for `count`
/// entries at a load of 80%, starting from 64 buckets in steps of 4, and `hashmap_add()` grows the table once it
/// holds more entries than that.
fn git_table_grows_with_last_insertion(count: usize) -> bool {
    let count = count as u64;
    let mut buckets = 64;
    while count * 100 / 80 > buckets {
        buckets <<= 2;
    }
    count > buckets * 80 / 100
}

/// Whether files of these modes can be identical: regular files whether or not they are executable, and other files
/// of the same mode.
fn modes_can_pair(a: EntryMode, b: EntryMode) -> bool {
    (a.is_blob() && b.is_blob()) || a.kind() == b.kind()
}

/// The part of `location` after its last `/`.
fn file_name(location: &BStr) -> &BStr {
    location
        .rfind_byte(b'/')
        .map_or(location, |slash| &location[slash + 1..])
}

/// Map the file name of each of `entries` to its index if no other entry has that file name, and to `None` otherwise.
fn unique_file_names<'a>(entries: impl Iterator<Item = (usize, Entry<'a>)>) -> HashMap<&'a BStr, Option<usize>> {
    let mut names = HashMap::new();
    for (index, entry) in entries {
        names
            .entry(file_name(entry.location))
            .and_modify(|unique| *unique = None)
            .or_insert(Some(index));
    }
    names
}

/// A source that is kept as one of the most similar to a destination.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    source: usize,
    destination: usize,
    score: u32,
    same_file_name: bool,
}

/// Replace the worst of `best`, the first of equally bad ones, with `candidate` if it is better, as
/// `record_if_better()` does. An empty place is worse than any candidate.
fn keep_if_better(best: &mut [Option<Candidate>; CANDIDATES_PER_DESTINATION], candidate: Candidate) {
    let mut worst = 0;
    for (place, kept) in best.iter().enumerate().skip(1) {
        if by_preference(kept.as_ref(), best[worst].as_ref()) == Ordering::Greater {
            worst = place;
        }
    }
    if by_preference(best[worst].as_ref(), Some(&candidate)) == Ordering::Greater {
        best[worst] = Some(candidate);
    }
}

/// Order candidates as `score_compare()` does: the more similar first, then those with the same file name, and empty
/// places last.
fn by_preference(a: Option<&Candidate>, b: Option<&Candidate>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.score.cmp(&a.score).then(b.same_file_name.cmp(&a.same_file_name)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::git_table_grows_with_last_insertion;

    #[test]
    fn gits_table_grows_with_the_last_source_for_some_numbers_of_sources() {
        let growing: Vec<_> = (0..=60_000)
            .filter(|&count| git_table_grows_with_last_insertion(count))
            .collect();
        assert_eq!(
            growing,
            [205, 3277, 52429],
            "a table sized for these numbers of sources is just too small to hold them all"
        );
    }
}
