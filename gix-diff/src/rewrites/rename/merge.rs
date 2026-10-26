//! Pair deleted with added files as merge-ort has diffcore-rename do it for each side of a merge.
//!
//! merge-ort (`merge-ort.c`) asks `diffcore_rename_extended()` to find the renames of one side only for the deleted
//! files whose new location matters to the merge, the relevant sources, and to count where the files of the
//! directories that the side removed went, from which it detects directory renames. [`detect()`] does that.

use std::{collections::HashMap, hash::BuildHasher};

use bstr::{BStr, BString, ByteSlice};
use gix_error::Result;

use super::{Blobs, Entry, MAX_SCORE, Rename, State, basename_min_score, effective_min_score, files};
use crate::rewrites::strmap;

/// The directory that stands for the destinations still unknown in the counts of a directory, `UNKNOWN_DIR`, which
/// can't be the path of a directory.
const UNKNOWN_DIRECTORY: &[u8] = b"/";

/// Why a merge needs to know where a file went that one side deleted, as merge-ort records it in its
/// `relevant_sources`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRelevance {
    /// The other side changed or deleted the file as well, so its content is merged where the file went.
    Content,
    /// The other side kept the file as it was, but where the file went may show where its directory, or one above it,
    /// went, which the other side added a file to.
    Location,
}

/// How much a merge needs to know where a directory went that one side removed, as merge-ort records it in its
/// `dirs_removed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryRelevance {
    /// Where the directory went doesn't matter, but the renames of its files still suggest where its files went whose
    /// names aren't unique.
    NotRelevant,
    /// Where the directory went matters only for where a directory above it went.
    ForAncestor,
    /// The other side added a file to the directory, so where the directory went matters.
    ForSelf,
}

/// A file that one side of a merge deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source<'a> {
    /// The file as the merge-base has it.
    pub entry: Entry<'a>,
    /// Why the merge needs to know where the file went, or `None` if it doesn't. Such a file can still be paired with
    /// an identical destination, and its file name still counts when file names are compared.
    pub relevance: Option<SourceRelevance>,
}

/// Options for [`detect()`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// As [`super::Options::min_score`]: merge-ort passes its rename threshold, which is 50% unless the merge sets
    /// another.
    pub min_score: u32,
    /// As [`super::Options::limit`], 0 meaning no limit: merge-ort passes `merge.renameLimit`, or 7000 if that isn't
    /// positive.
    pub limit: usize,
    /// Whether files whose blob is empty are paired, which merge-ort never does.
    pub rename_empty: bool,
}

impl Default for Options {
    /// Pair files that are at least 50% similar, estimate similarities for up to 7000 sources and destinations, and
    /// leave files with empty blobs unpaired, as merge-ort does by default.
    fn default() -> Self {
        Options {
            min_score: MAX_SCORE / 2,
            limit: 7000,
            rename_empty: false,
        }
    }
}

/// The result of [`detect()`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The sources and destinations that were paired, in the order of the destinations.
    pub renames: Vec<Rename>,
    /// Where the files of removed directories went: for each directory of `dirs_removed` that isn't
    /// [`DirectoryRelevance::NotRelevant`] and that a rename counted for, the directories to which renames moved its
    /// files, each with the number of these renames, as merge-ort's `dir_rename_count` holds them after rename
    /// detection. A directory can map to no directory if all of its counts were of destinations still unknown.
    pub directory_renames: HashMap<BString, HashMap<BString, usize>>,
    /// As [`super::Outcome::needed_limit`].
    pub needed_limit: Option<usize>,
}

/// Pair the files that one side of a merge added, the `destinations`, with the files that it deleted, the `sources`,
/// as merge-ort's `detect_regular_renames()` has `diffcore_rename_extended()` do it, and count where the files of the
/// directories in `dirs_removed` went. `blobs` provides the data of the files whose similarity is estimated.
///
/// Sources and destinations are considered in the order of `sources` and `destinations`, which is the order in which
/// merge-ort collects them and which decides between equally good pairs. The steps are those of [`super::detect()`],
/// with these differences:
///
/// - Files whose blob is empty are left unpaired unless [`Options::rename_empty`] is set.
/// - If [`Options::min_score`] is [`MAX_SCORE`] or more, only identical files are paired and nothing is counted.
/// - A pair counts for the directory of its source if that directory is a key of `dirs_removed`. Then the walk goes up
///   from that directory as long as it isn't [`DirectoryRelevance::NotRelevant`] and neither location has reached the
///   top: each directory above counts if it is a key of `dirs_removed`, the name of the directory just left is the same
///   in both locations, and it is [`DirectoryRelevance::ForSelf`], while one of the first two conditions failing ends
///   the walk. A rename of `a/b/c/f` to `x/y/c/f` counts `a/b/c` for `x/y/c`, and `a/b` for `x/y` if `a/b/c` isn't
///   `NotRelevant` and `a/b` is `ForSelf`.
/// - Only sources with a [relevance](Source::relevance) are paired by their file name or by their similarity. A
///   source whose file name another unpaired source shares, or that more than one unpaired destination has, is paired
///   by its file name with the destination of that name in the directory to which the identical files of its
///   directory went most often, if that destination is still unpaired.
/// - Before similarities are estimated, each unpaired relevant source counts as going to an unknown directory for
///   each removed directory above it up to the first `NotRelevant` one. A `ForSelf` directory whose most common
///   destination has more renames than the second most common one and the unknown ones together is
///   [`DirectoryRelevance::ForAncestor`] from then on, and a source of [`SourceRelevance::Location`] takes no further
///   part unless a `ForSelf` directory lies above it with only `ForAncestor` ones in between.
///
/// merge-ort calls this only for a side that deleted or added files and has at least one relevant source. The root
/// directory, whose path is empty, is never a removed directory.
pub fn detect(
    sources: &[Source<'_>],
    destinations: &[Entry<'_>],
    dirs_removed: &HashMap<BString, DirectoryRelevance, impl BuildHasher>,
    blobs: &mut dyn Blobs,
    options: Options,
) -> Result<Outcome> {
    let min_score = effective_min_score(options.min_score);
    let pairable = |entry: &Entry<'_>| options.rename_empty || !entry.id.is_empty_blob();
    let source_entries: Vec<_> = sources.iter().map(|source| source.entry).collect();
    let mut state = State::new(files(&source_entries, pairable), files(destinations, pairable), blobs);
    if state.sources.is_empty() || state.destinations.is_empty() {
        return Ok(Outcome::default());
    }
    state.pair_identical(false);
    if min_score == MAX_SCORE {
        return Ok(Outcome {
            renames: state.into_outcome(None).renames,
            ..Outcome::default()
        });
    }

    let relevance: Vec<_> = state.sources.iter().map(|file| sources[file.index].relevance).collect();
    let removed = dirs_removed
        .iter()
        .map(|(directory, &relevance)| (directory.as_bstr(), relevance))
        .collect();
    let mut directories = Directories::new(&state, removed);
    state.pair_file_names(
        basename_min_score(min_score),
        &|source| relevance[source].is_some(),
        Some(&mut directories),
    )?;
    let relevant_sources: Vec<_> = state
        .unpaired_sources()
        .into_iter()
        .filter(|&source| relevance[source].is_some())
        .collect();
    let relevant_sources = directories.drop_sources_of_known_renames(&state, relevant_sources, &relevance);
    let needed_limit = state.pair_similar(min_score, options.limit, &relevant_sources, Some(&mut directories))?;
    let directory_renames = directories.into_counts();
    let outcome = state.into_outcome(needed_limit);
    Ok(Outcome {
        renames: outcome.renames,
        directory_renames,
        needed_limit: outcome.needed_limit,
    })
}

/// What merge-ort's `struct dir_rename_info` holds while the renames of one side are found.
pub(super) struct Directories<'a, 'r> {
    /// The directories that the side removed, `dirs_removed`, as rename detection updates them. Their paths are also
    /// the directories whose renames are counted, `relevant_source_dirs`.
    removed: HashMap<&'r BStr, DirectoryRelevance>,
    /// For each directory, the directories that its files went to and how often, `dir_rename_count`.
    counts: HashMap<BString, Counts>,
    /// For each directory, the directory to which its identical files went most often, `dir_rename_guess`.
    suggestions: HashMap<BString, BString>,
    /// The destinations without an identical source, by location, `idx_map`.
    unpaired_destinations: HashMap<&'a BStr, usize>,
}

impl<'a, 'r> Directories<'a, 'r> {
    /// Count the pairs of identical files in `state` and suggest, for each directory they left, the directory to which
    /// they went most often, as `initialize_dir_rename_info()` does.
    fn new(state: &State<'a, '_>, removed: HashMap<&'r BStr, DirectoryRelevance>) -> Self {
        let mut directories = Directories {
            removed,
            counts: HashMap::new(),
            suggestions: HashMap::new(),
            unpaired_destinations: HashMap::new(),
        };
        for (destination, file) in state.destinations.iter().enumerate() {
            match state.pairs[destination] {
                Some((source, _)) => directories.count(state.sources[source].entry.location, file.entry.location),
                None => {
                    directories
                        .unpaired_destinations
                        .insert(file.entry.location, destination);
                }
            }
        }
        directories.suggestions = directories
            .counts
            .iter()
            .filter_map(|(directory, counts)| Some((directory.clone(), counts.most_common()?.to_owned())))
            .collect();
        directories
    }

    /// The unpaired destination that has the file name of `location` in the directory suggested for the directory of
    /// `location`, if there is one, as `idx_possible_rename()` finds it.
    pub(super) fn suggested_destination(&self, location: &BStr) -> Option<usize> {
        let (directory, name) = split(location);
        let mut path = self.suggestions.get(directory)?.clone();
        path.push(b'/');
        path.extend_from_slice(name);
        self.unpaired_destinations.get(path.as_bstr()).copied()
    }

    /// Count the rename of the file at `old` to `new` as `update_dir_rename_counts()` does, for which
    /// [`detect()`] describes the rules.
    pub(super) fn count(&mut self, old: &BStr, new: &BStr) {
        let (mut old, mut new) = (old, new);
        let mut first = true;
        loop {
            let (old_directory, old_name) = split(old);
            if !self.removed.contains_key(old_directory) {
                break;
            }
            let (new_directory, new_name) = split(new);
            if !first && old_name != new_name {
                break;
            }
            let relevance = self.relevance(old_directory);
            if first || relevance == DirectoryRelevance::ForSelf {
                self.counts
                    .entry(old_directory.to_owned())
                    .or_default()
                    .increment(new_directory);
            }
            first = false;
            if relevance == DirectoryRelevance::NotRelevant || old_directory.is_empty() || new_directory.is_empty() {
                break;
            }
            (old, new) = (old_directory, new_directory);
        }
    }

    /// How much the merge needs to know where `directory` went: `NotRelevant` if it wasn't removed.
    fn relevance(&self, directory: &BStr) -> DirectoryRelevance {
        self.removed
            .get(directory)
            .copied()
            .unwrap_or(DirectoryRelevance::NotRelevant)
    }

    /// Leave out those of `sources` whose renames can't change where a directory went that matters, as
    /// `handle_early_known_dir_renames()` does, for which [`detect()`] describes the rules.
    fn drop_sources_of_known_renames(
        &mut self,
        state: &State<'a, '_>,
        sources: Vec<usize>,
        relevance: &[Option<SourceRelevance>],
    ) -> Vec<usize> {
        for &source in &sources {
            let mut directory = split(state.sources[source].entry.location).0;
            while !directory.is_empty() && self.relevance(directory) != DirectoryRelevance::NotRelevant {
                self.counts
                    .entry(directory.to_owned())
                    .or_default()
                    .increment(UNKNOWN_DIRECTORY.as_bstr());
                directory = split(directory).0;
            }
        }
        for (directory, counts) in &self.counts {
            let Some(relevance) = self.removed.get_mut(directory.as_bstr()) else {
                continue;
            };
            if *relevance == DirectoryRelevance::ForSelf && counts.majority_is_known() {
                *relevance = DirectoryRelevance::ForAncestor;
            }
        }

        sources
            .into_iter()
            .filter(|&source| {
                relevance[source] != Some(SourceRelevance::Location) || {
                    let mut directory = split(state.sources[source].entry.location).0;
                    loop {
                        match self.relevance(directory) {
                            DirectoryRelevance::ForSelf => break true,
                            DirectoryRelevance::ForAncestor if !directory.is_empty() => {
                                directory = split(directory).0;
                            }
                            _ => break false,
                        }
                    }
                }
            })
            .collect()
    }

    /// The counts of the directories whose renames matter, without the unknown destinations, as
    /// `cleanup_dir_rename_info()` leaves them.
    fn into_counts(self) -> HashMap<BString, HashMap<BString, usize>> {
        let removed = self.removed;
        self.counts
            .into_iter()
            .filter(|(directory, _)| {
                removed
                    .get(directory.as_bstr())
                    .is_some_and(|relevance| *relevance != DirectoryRelevance::NotRelevant)
            })
            .map(|(directory, counts)| (directory, counts.into_map()))
            .collect()
    }
}

/// The number of renames from one directory to each directory, along with the order in which the directories were first
/// counted, which decides the order in which git's `strintmap` visits them.
#[derive(Default)]
struct Counts {
    /// The directories, in the order in which they were first counted.
    directories: Vec<BString>,
    /// The number of renames to each of `directories`.
    renames: Vec<usize>,
    /// The index of each directory among `directories`.
    index: HashMap<BString, usize>,
}

impl Counts {
    fn increment(&mut self, directory: &BStr) {
        if let Some(&index) = self.index.get(directory) {
            self.renames[index] += 1;
        } else {
            self.index.insert(directory.to_owned(), self.directories.len());
            self.directories.push(directory.to_owned());
            self.renames.push(1);
        }
    }

    /// The directory with the most renames, the first of them in the order of git's `strintmap`, as
    /// `get_highest_rename_path()` takes it.
    fn most_common(&self) -> Option<&BStr> {
        let mut most_common: Option<usize> = None;
        for index in strmap::iteration_order(&self.directories) {
            if most_common.is_none_or(|most| self.renames[index] > self.renames[most]) {
                most_common = Some(index);
            }
        }
        most_common.map(|index| self.directories[index].as_bstr())
    }

    /// Whether the directory with the most renames has more of them than the one with the second most and the unknown
    /// destinations together, as `dir_rename_already_determinable()` decides.
    fn majority_is_known(&self) -> bool {
        let (mut first, mut second, mut unknown) = (0, 0, 0);
        for (directory, &renames) in self.directories.iter().zip(&self.renames) {
            if directory == UNKNOWN_DIRECTORY {
                unknown = renames;
            } else if renames >= first {
                second = first;
                first = renames;
            } else if renames >= second {
                second = renames;
            }
        }
        first > second + unknown
    }

    /// The number of renames to each directory, without the unknown destinations.
    fn into_map(self) -> HashMap<BString, usize> {
        self.directories
            .into_iter()
            .zip(self.renames)
            .filter(|(directory, _)| directory != UNKNOWN_DIRECTORY)
            .collect()
    }
}

/// The directory and the file name of `location`: the parts before and after its last `/`, or the empty path and all
/// of `location`.
fn split(location: &BStr) -> (&BStr, &BStr) {
    match location.rfind_byte(b'/') {
        Some(slash) => (&location[..slash], &location[slash + 1..]),
        None => (b"".as_bstr(), location),
    }
}
