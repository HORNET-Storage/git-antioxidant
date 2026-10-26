use gix_error::Result;
use std::num::NonZeroU32;

use gix_diff::{blob::TokenSource, rewrites::rename::MAX_SCORE, tree::Visit};
use gix_error::{ErrorExt, OptionExt, ResultExt, message};
use gix_hash::ObjectId;
use gix_object::{
    FindExt,
    bstr::{BStr, BString, ByteSlice},
    tree::EntryMode,
};
use gix_traverse::commit::find as find_commit;
use gix_worktree::attributes::{StateRef, search::Outcome as Attributes};
use smallvec::SmallVec;

use super::{Change, UnblamedHunk, process_changes};
use crate::{
    BlameEntry, Options, Outcome, Statistics,
    types::{BlamePathEntry, Start},
};

/// Produce a list of consecutive [`BlameEntry`] instances to indicate in which commits the ranges of the file
/// at `suspect:<file_path>` originated in.
///
/// ## Parameters
///
/// * `odb`
///    - Access to database objects, also for used for diffing.
///    - Should have an object cache for good diff performance.
/// * `start`
///    - Where to start the blame. Can be either a commit or contents of a worktree file that contains
///      untracked changes.
/// * `cache`
///    - Optionally, the commitgraph cache.
/// * `resource_cache`
///    - Used for diffing trees.
/// * `file_path`
///    - A *slash-separated* worktree-relative path to the file to blame.
/// * `options`
///    - An instance of [`Options`].
///
/// ## The algorithm
///
/// *For brevity, `HEAD` denotes the starting point of the blame operation. It could be any commit, or even commits that
/// represent the worktree state.
///
/// We begin with one or more *Unblamed Hunks* and a single suspect, usually the `HEAD` commit as the commit containing the
/// *Blamed File*, so that it contains the entire file, with the first commit being a candidate for the entire *Blamed File*.
/// We traverse the commit graph starting at the first suspect, and see if there have been changes to `file_path`.
/// If so, we have found a *Source File* and a *Suspect* commit, and have hunks that represent these changes.
/// Now the *Unblamed Hunk* is split at the boundaries of each matching change, creating a new *Unblamed Hunk* on each side,
/// along with a [`BlameEntry`] to represent the match.
/// This is repeated until there are no non-empty *Unblamed Hunk*s left.
///
/// At a high level, what we want to do is the following:
///
/// - get the commit
/// - walk through its parents
///   - for each parent, do a diff and mark lines that don’t have a suspect yet (this is the term
///     used in `libgit2`), but that have been changed in this commit
///
/// The algorithm in `libgit2` works by going through parents and keeping a linked list of blame
/// suspects. It can be visualized as follows:
///
/// <---------------------------------------->
/// <---------------><----------------------->
/// <---><----------><----------------------->
/// <---><----------><-------><-----><------->
/// <---><---><-----><-------><-----><------->
/// <---><---><-----><-------><-----><-><-><->
pub fn file(
    odb: impl gix_object::Find + gix_object::FindHeader,
    start: Start,
    cache: Option<gix_commitgraph::Graph>,
    resource_cache: &mut gix_diff::blob::Platform,
    file_path: &BStr,
    options: Options,
) -> Result<Outcome> {
    let _span = gix_trace::coarse!("gix_blame::file()", ?file_path, ?start);

    let mut stats = Statistics::default();
    let (mut buf, mut buf2, mut buf3) = (Vec::new(), Vec::new(), Vec::new());

    let InitialState {
        blamed_file_blob,
        mut hunks_to_blame,
        mut out,
        first_suspect,
    } = initial_state(
        &odb,
        start,
        cache.as_ref(),
        file_path,
        &options,
        &mut buf,
        &mut buf2,
        &mut stats,
    )?;

    if hunks_to_blame.is_empty() {
        out.sort_by_key(|entry| entry.start_in_blamed_file);
        return Ok(Outcome {
            entries: coalesce_blame_entries(out),
            blob: blamed_file_blob,
            statistics: stats,
            blame_path: None,
        });
    }

    let mut queue: gix_revwalk::PriorityQueue<gix_date::SecondsSinceUnixEpoch, ObjectId> =
        gix_revwalk::PriorityQueue::new();

    if let Some(first_suspect) = first_suspect {
        let commit = find_commit(cache.as_ref(), &odb, &first_suspect, &mut buf)?;
        queue.insert(commit.commit_time()?, first_suspect);
    }

    let mut diff_state = gix_diff::tree::State::default();
    let mut previous_entry: Option<(ObjectId, PathEntry)> = None;
    let mut blame_path = if options.debug_track_path {
        Some(Vec::new())
    } else {
        None
    };

    'outer: while let Some(suspect) = queue.pop_value() {
        stats.commits_traversed += 1;
        if hunks_to_blame.is_empty() {
            break;
        }

        let first_hunk_for_suspect = hunks_to_blame.iter().find(|hunk| hunk.has_suspect(&suspect));
        let Some(first_hunk_for_suspect) = first_hunk_for_suspect else {
            // There are no `UnblamedHunk`s associated with this `suspect`, so we can continue with
            // the next one.
            continue 'outer;
        };

        let current_file_path = first_hunk_for_suspect
            .source_file_name
            .clone()
            .unwrap_or_else(|| file_path.to_owned());

        let commit = find_commit(cache.as_ref(), &odb, &suspect, &mut buf)?;
        let commit_time = commit.commit_time()?;

        if let Some(since) = options.since
            && commit_time < since.seconds
        {
            if unblamed_to_out_is_done(&mut hunks_to_blame, &mut out, suspect) {
                break 'outer;
            }

            continue;
        }

        let parent_ids: ParentIds = collect_parents(commit, &odb, cache.as_ref(), &mut buf2)?;

        if parent_ids.is_empty() {
            // A commit without parents passes blame to no one, so the lines still blamed on it
            // originate in it. That holds while other suspects are still queued, as they are in a
            // history with more than one root commit, or when a descendant's commit time isn't newer
            // than the root commit's.
            let done = unblamed_to_out_is_done(&mut hunks_to_blame, &mut out, suspect);
            if let Some(ref mut blame_path) = blame_path {
                let entry = previous_entry
                    .filter(|(id, _)| *id == suspect)
                    .map(|(_, entry)| entry.id);

                let blame_path_entry = BlamePathEntry {
                    source_file_path: current_file_path.clone(),
                    previous_source_file_path: None,
                    commit_id: suspect,
                    blob_id: entry.unwrap_or(gix_hash::Kind::shortest().null()),
                    previous_blob_id: gix_hash::Kind::shortest().null(),
                    parent_index: 0,
                };
                blame_path.push(blame_path_entry);
            }

            if done {
                break 'outer;
            }
            // Other suspects still have lines to blame, keep looking.
            continue;
        }

        let mut entry = previous_entry
            .take()
            .filter(|(id, _)| *id == suspect)
            .map(|(_, entry)| entry);
        if entry.is_none() {
            entry = find_path_entry_in_commit(
                &odb,
                &suspect,
                current_file_path.as_ref(),
                cache.as_ref(),
                &mut buf,
                &mut buf2,
                &mut stats,
            )?;
        }

        let Some(entry) = entry else {
            continue;
        };
        let entry_id = entry.id;

        // This block asserts that, for every `UnblamedHunk`, all lines in the *Blamed File* are
        // identical to the corresponding lines in the *Source File*.
        #[cfg(debug_assertions)]
        {
            let source_blob = odb.find_blob(&entry_id, &mut buf)?.data.to_vec();
            let mut source_interner = gix_diff::blob::Interner::new(source_blob.len() / 100);
            let source_lines_as_tokens: Vec<_> = tokens_for_diffing(&source_blob)
                .tokenize()
                .map(|token| source_interner.intern(token))
                .collect();

            let mut blamed_interner = gix_diff::blob::Interner::new(blamed_file_blob.len() / 100);
            let blamed_lines_as_tokens: Vec<_> = tokens_for_diffing(&blamed_file_blob)
                .tokenize()
                .map(|token| blamed_interner.intern(token))
                .collect();

            for hunk in hunks_to_blame.iter() {
                if let Some(range_in_suspect) = hunk.get_range(&suspect) {
                    let range_in_blamed_file = hunk.range_in_blamed_file.clone();

                    let source_lines = range_in_suspect
                        .clone()
                        .map(|i| BString::new(source_interner[source_lines_as_tokens[i as usize]].into()))
                        .collect::<Vec<_>>();
                    let blamed_lines = range_in_blamed_file
                        .clone()
                        .map(|i| BString::new(blamed_interner[blamed_lines_as_tokens[i as usize]].into()))
                        .collect::<Vec<_>>();

                    assert_eq!(source_lines, blamed_lines);
                }
            }
        }

        for (pid, (parent_id, parent_commit_time)) in parent_ids.iter().enumerate() {
            if let Some(parent_entry) = find_path_entry_in_commit(
                &odb,
                parent_id,
                current_file_path.as_ref(),
                cache.as_ref(),
                &mut buf,
                &mut buf2,
                &mut stats,
            )? {
                // To Git's `find_origin()`, a file of another type at the path, such as a regular file where the
                // suspect has a symlink, is no version of the file, even with the same blob.
                if !is_same_type(parent_entry.mode, entry.mode) {
                    continue;
                }
                if pid == 0 {
                    previous_entry = Some((*parent_id, parent_entry));
                }
                if entry_id == parent_entry.id {
                    pass_blame_from_to(suspect, *parent_id, &mut hunks_to_blame);
                    queue.insert(*parent_commit_time, *parent_id);
                    continue 'outer;
                }
            }
        }

        // Find each parent's version of the file before passing blame to any of them, as Git's blame does: the first
        // parent with the file's content under the same name took all of the blame above, and when there's none, the
        // first parent with the content under the name that rename tracking pairs with the file takes all of it.
        let mut changes_per_parent = Vec::with_capacity(parent_ids.len());
        for (parent_id, _) in &parent_ids {
            changes_per_parent.push(tree_diff_at_file_path(
                &odb,
                current_file_path.as_ref(),
                suspect,
                *parent_id,
                cache.as_ref(),
                &mut stats,
                &mut diff_state,
                resource_cache,
                &mut buf,
                &mut buf2,
                &mut buf3,
                options.rewrites,
            )?);
        }

        let unchanged_under_other_name = changes_per_parent.iter().enumerate().find_map(|(index, change)| {
            let Some(TreeDiffChange::Rewrite {
                source_location,
                source_id,
                ..
            }) = change
            else {
                return None;
            };
            (*source_id == entry_id).then(|| (index, source_location.clone()))
        });
        if let Some((index, source_location)) = unchanged_under_other_name {
            let (parent_id, parent_commit_time) = parent_ids[index];
            pass_blame_from_to(suspect, parent_id, &mut hunks_to_blame);
            for hunk in hunks_to_blame.iter_mut().filter(|hunk| hunk.has_suspect(&parent_id)) {
                hunk.source_file_name = Some(source_location.clone());
            }
            if let Some(ref mut blame_path) = blame_path {
                let blame_path_entry = BlamePathEntry {
                    source_file_path: current_file_path.clone(),
                    previous_source_file_path: Some(source_location),
                    commit_id: suspect,
                    blob_id: entry_id,
                    previous_blob_id: entry_id,
                    parent_index: index,
                };
                blame_path.push(blame_path_entry);
            }
            queue.insert(parent_commit_time, parent_id);
            continue 'outer;
        }

        let more_than_one_parent = parent_ids.len() > 1;
        for (index, ((parent_id, parent_commit_time), changes_for_file_path)) in
            parent_ids.iter().zip(changes_per_parent).enumerate()
        {
            queue.insert(*parent_commit_time, *parent_id);
            let Some(modification) = changes_for_file_path else {
                if more_than_one_parent {
                    // None of the changes affected the file we’re currently blaming.
                    // Copy blame to parent.
                    for unblamed_hunk in &mut hunks_to_blame {
                        unblamed_hunk.clone_blame(suspect, *parent_id);
                    }
                } else {
                    pass_blame_from_to(suspect, *parent_id, &mut hunks_to_blame);
                }
                continue;
            };

            match modification {
                TreeDiffChange::Addition { id } => {
                    if more_than_one_parent {
                        // Do nothing under the assumption that this always (or almost always)
                        // implies that the file comes from a different parent, compared to which
                        // it was modified, not added.
                    } else if unblamed_to_out_is_done(&mut hunks_to_blame, &mut out, suspect) {
                        if let Some(ref mut blame_path) = blame_path {
                            let blame_path_entry = BlamePathEntry {
                                source_file_path: current_file_path.clone(),
                                previous_source_file_path: None,
                                commit_id: suspect,
                                blob_id: id,
                                previous_blob_id: gix_hash::Kind::shortest().null(),
                                parent_index: index,
                            };
                            blame_path.push(blame_path_entry);
                        }

                        break 'outer;
                    }
                }
                TreeDiffChange::Deletion => {
                    unreachable!("We already found file_path in suspect^{{tree}}, so it can't be deleted")
                }
                TreeDiffChange::Modification { previous_id, id } => {
                    let changes = blob_changes(
                        &odb,
                        resource_cache,
                        id,
                        previous_id,
                        file_path,
                        file_path,
                        options.diff_algorithm,
                        &mut stats,
                    )?;
                    hunks_to_blame = process_changes(hunks_to_blame, changes.clone(), suspect, *parent_id);
                    if let Some(ref mut blame_path) = blame_path {
                        let has_blame_been_passed = hunks_to_blame.iter().any(|hunk| hunk.has_suspect(parent_id));

                        if has_blame_been_passed {
                            let blame_path_entry = BlamePathEntry {
                                source_file_path: current_file_path.clone(),
                                previous_source_file_path: Some(current_file_path.clone()),
                                commit_id: suspect,
                                blob_id: id,
                                previous_blob_id: previous_id,
                                parent_index: index,
                            };
                            blame_path.push(blame_path_entry);
                        }
                    }
                }
                TreeDiffChange::Rewrite {
                    source_location,
                    source_id,
                    id,
                } => {
                    let changes = blob_changes(
                        &odb,
                        resource_cache,
                        id,
                        source_id,
                        file_path,
                        source_location.as_ref(),
                        options.diff_algorithm,
                        &mut stats,
                    )?;
                    hunks_to_blame = process_changes(hunks_to_blame, changes, suspect, *parent_id);

                    let mut has_blame_been_passed = false;

                    for hunk in hunks_to_blame.iter_mut() {
                        if hunk.has_suspect(parent_id) {
                            hunk.source_file_name = Some(source_location.clone());

                            has_blame_been_passed = true;
                        }
                    }

                    if has_blame_been_passed && let Some(ref mut blame_path) = blame_path {
                        let blame_path_entry = BlamePathEntry {
                            source_file_path: current_file_path.clone(),
                            previous_source_file_path: Some(source_location.clone()),
                            commit_id: suspect,
                            blob_id: id,
                            previous_blob_id: source_id,
                            parent_index: index,
                        };
                        blame_path.push(blame_path_entry);
                    }
                }
            }
        }

        hunks_to_blame.retain_mut(|unblamed_hunk| {
            if unblamed_hunk.suspects.len() == 1
                && let Some(entry) = BlameEntry::from_unblamed_hunk(unblamed_hunk, suspect)
            {
                // At this point, we have copied blame for every hunk to a parent. Hunks
                // that have only `suspect` left in `suspects` have not passed blame to any
                // parent, and so they can be converted to a `BlameEntry` and moved to
                // `out`.
                out.push(entry);
                return false;
            }
            unblamed_hunk.remove_blame(suspect);
            true
        });
    }

    debug_assert_eq!(
        hunks_to_blame,
        vec![],
        "only if there is no portion of the file left we have completed the blame"
    );

    // I don’t know yet whether it would make sense to use a data structure instead that preserves
    // order on insertion.
    out.sort_by_key(|a| a.start_in_blamed_file);
    Ok(Outcome {
        entries: coalesce_blame_entries(out),
        blob: blamed_file_blob,
        statistics: stats,
        blame_path,
    })
}

/// Pass ownership of each unblamed hunk of `from` to `to`.
///
/// This happens when `from` didn't actually change anything in the blamed file.
fn pass_blame_from_to(from: ObjectId, to: ObjectId, hunks_to_blame: &mut Vec<UnblamedHunk>) {
    for unblamed_hunk in hunks_to_blame {
        unblamed_hunk.pass_blame(from, to);
    }
}

/// Convert each of the unblamed hunk in `hunks_to_blame` into a [`BlameEntry`], consuming them in the process.
///
/// Return `true` if we are done because `hunks_to_blame` is empty.
fn unblamed_to_out_is_done(
    hunks_to_blame: &mut Vec<UnblamedHunk>,
    out: &mut Vec<BlameEntry>,
    suspect: ObjectId,
) -> bool {
    let mut without_suspect = Vec::new();
    out.extend(hunks_to_blame.drain(..).filter_map(|hunk| {
        BlameEntry::from_unblamed_hunk(&hunk, suspect).or_else(|| {
            without_suspect.push(hunk);
            None
        })
    }));
    *hunks_to_blame = without_suspect;
    hunks_to_blame.is_empty()
}

/// This function merges adjacent blame entries. It merges entries that are adjacent both in the
/// blamed file and in the source file that introduced them. This follows `git`’s
/// behaviour. `libgit2`, as of 2024-09-19, only checks whether two entries are adjacent in the
/// blamed file which can result in different blames in certain edge cases. See [the commit][1]
/// that introduced the extra check into `git` for context. See [this commit][2] for a way to test
/// for this behaviour in `git`.
///
/// [1]: https://github.com/git/git/commit/c2ebaa27d63bfb7c50cbbdaba90aee4efdd45d0a
/// [2]: https://github.com/git/git/commit/6dbf0c7bebd1c71c44d786ebac0f2b3f226a0131
fn coalesce_blame_entries(lines_blamed: Vec<BlameEntry>) -> Vec<BlameEntry> {
    let len = lines_blamed.len();
    lines_blamed
        .into_iter()
        .fold(Vec::with_capacity(len), |mut acc, entry| {
            let previous_entry = acc.last();

            if let Some(previous_entry) = previous_entry {
                let previous_blamed_range = previous_entry.range_in_blamed_file();
                let current_blamed_range = entry.range_in_blamed_file();
                let previous_source_range = previous_entry.range_in_source_file();
                let current_source_range = entry.range_in_source_file();
                if previous_entry.commit_id == entry.commit_id
                    && previous_blamed_range.end == current_blamed_range.start
                    // As of 2024-09-19, the check below only is in `git`, but not in `libgit2`.
                    && previous_source_range.end == current_source_range.start
                {
                    let coalesced_entry = BlameEntry {
                        start_in_blamed_file: previous_blamed_range.start as u32,
                        start_in_source_file: previous_source_range.start as u32,
                        len: NonZeroU32::new((current_source_range.end - previous_source_range.start) as u32)
                            .expect("BUG: hunks are never zero-sized"),
                        commit_id: previous_entry.commit_id,
                        source_file_name: previous_entry.source_file_name.clone(),
                    };

                    acc.pop();
                    acc.push(coalesced_entry);
                } else {
                    acc.push(entry);
                }

                acc
            } else {
                acc.push(entry);

                acc
            }
        })
}

/// A change of the blamed file from a parent to a commit, keeping only the blame-relevant information.
enum TreeDiffChange {
    Addition {
        id: ObjectId,
    },
    Deletion,
    Modification {
        previous_id: ObjectId,
        id: ObjectId,
    },
    Rewrite {
        source_location: BString,
        source_id: ObjectId,
        id: ObjectId,
    },
}

impl From<gix_diff::tree::recorder::Change> for TreeDiffChange {
    fn from(value: gix_diff::tree::recorder::Change) -> Self {
        use gix_diff::tree::recorder::Change;

        match value {
            Change::Addition { oid, .. } => Self::Addition { id: oid },
            Change::Deletion { .. } => Self::Deletion,
            Change::Modification { previous_oid, oid, .. } => Self::Modification {
                previous_id: previous_oid,
                id: oid,
            },
        }
    }
}

/// Find the change of the file at `file_path` from the parent `parent_id` to the commit `id` as Git's blame does with
/// `find_origin()`: a diff limited to the path finds the change there. If the commit added the file at the path and
/// `rewrites` is set, the file is followed to the file of the parent that it was renamed from, as `find_rename()`
/// finds it.
#[expect(clippy::too_many_arguments)]
fn tree_diff_at_file_path(
    odb: impl gix_object::Find + gix_object::FindHeader,
    file_path: &BStr,
    id: ObjectId,
    parent_id: ObjectId,
    cache: Option<&gix_commitgraph::Graph>,
    stats: &mut Statistics,
    state: &mut gix_diff::tree::State,
    resource_cache: &mut gix_diff::blob::Platform,
    commit_buf: &mut Vec<u8>,
    lhs_tree_buf: &mut Vec<u8>,
    rhs_tree_buf: &mut Vec<u8>,
    rewrites: Option<gix_diff::Rewrites>,
) -> Result<Option<TreeDiffChange>> {
    use gix_diff::tree::recorder::Change;

    let parent_tree_id = find_commit(cache, &odb, &parent_id, commit_buf)
        .or_raise(|| message("Could not find existing iterator over a tree"))?
        .tree_id()
        .or_raise(|| message("Failure to decode commit during traversal"))?;

    let parent_tree_iter = odb
        .find_tree_iter(&parent_tree_id, lhs_tree_buf)
        .or_raise(|| message("Could not find existing iterator over a tree"))?;
    stats.trees_decoded += 1;

    let tree_id = find_commit(cache, &odb, &id, commit_buf)
        .or_raise(|| message("Could not find existing iterator over a tree"))?
        .tree_id()
        .or_raise(|| message("Failure to decode commit during traversal"))?;

    let tree_iter = odb
        .find_tree_iter(&tree_id, rhs_tree_buf)
        .or_raise(|| message("Could not find existing iterator over a tree"))?;
    stats.trees_decoded += 1;

    let change = tree_diff_without_rewrites_at_file_path(&odb, file_path, stats, state, parent_tree_iter, tree_iter)?;
    let (mode, added_id) = match change {
        Some(Change::Addition { entry_mode, oid, .. }) => (entry_mode, oid),
        // To Git's `find_origin()`, a file of another type at the path, such as a regular file where the commit has a
        // symlink, is no version of the file, and `find_rename()` doesn't follow the file either: it only pairs files
        // that a commit added with files that it deleted.
        Some(Change::Modification {
            previous_entry_mode,
            entry_mode,
            oid,
            ..
        }) if !is_same_type(previous_entry_mode, entry_mode) => return Ok(Some(TreeDiffChange::Addition { id: oid })),
        other => return Ok(other.map(Into::into)),
    };
    let rename = match rewrites {
        Some(rewrites) => find_rename_at_file_path(
            &odb,
            file_path,
            mode,
            added_id,
            stats,
            state,
            resource_cache,
            parent_tree_iter,
            tree_iter,
            rewrites,
        )?,
        None => None,
    };
    Ok(Some(rename.unwrap_or(TreeDiffChange::Addition { id: added_id })))
}

/// Find the change at `file_path` from `parent_tree_iter` to `tree_iter` without rename tracking, as the diff that
/// Git's blame limits to the path finds it.
fn tree_diff_without_rewrites_at_file_path(
    odb: impl gix_object::Find + gix_object::FindHeader,
    file_path: &BStr,
    stats: &mut Statistics,
    state: &mut gix_diff::tree::State,
    parent_tree_iter: gix_object::TreeRefIter<'_>,
    tree_iter: gix_object::TreeRefIter<'_>,
) -> Result<Option<gix_diff::tree::recorder::Change>> {
    struct FindChangeToPath {
        inner: gix_diff::tree::Recorder,
        interesting_path: BString,
        change: Option<gix_diff::tree::recorder::Change>,
    }

    impl FindChangeToPath {
        fn new(interesting_path: BString) -> Self {
            let inner =
                gix_diff::tree::Recorder::default().track_location(Some(gix_diff::tree::recorder::Location::Path));

            FindChangeToPath {
                inner,
                interesting_path,
                change: None,
            }
        }
    }

    impl Visit for FindChangeToPath {
        fn pop_front_tracked_path_and_set_current(&mut self) {
            self.inner.pop_front_tracked_path_and_set_current();
        }

        fn push_back_tracked_path_component(&mut self, component: &BStr) {
            self.inner.push_back_tracked_path_component(component);
        }

        fn push_path_component(&mut self, component: &BStr) {
            self.inner.push_path_component(component);
        }

        fn pop_path_component(&mut self) {
            self.inner.pop_path_component();
        }

        fn visit(&mut self, change: gix_diff::tree::visit::Change) -> gix_diff::tree::visit::Action {
            use gix_diff::tree::visit::Change::*;

            if self.inner.path() == self.interesting_path {
                // Where a file replaced a directory, the deletion of the directory comes before the addition of the
                // file, which is the change of the file.
                if let Deletion { entry_mode, .. } = &change
                    && entry_mode.is_tree()
                {
                    return std::ops::ControlFlow::Continue(());
                }
                self.change = Some(match change {
                    Deletion {
                        entry_mode,
                        oid,
                        relation,
                    } => gix_diff::tree::recorder::Change::Deletion {
                        entry_mode,
                        oid,
                        path: self.inner.path_clone(),
                        relation,
                    },
                    Addition {
                        entry_mode,
                        oid,
                        relation,
                    } => gix_diff::tree::recorder::Change::Addition {
                        entry_mode,
                        oid,
                        path: self.inner.path_clone(),
                        relation,
                    },
                    Modification {
                        previous_entry_mode,
                        previous_oid,
                        entry_mode,
                        oid,
                    } => gix_diff::tree::recorder::Change::Modification {
                        previous_entry_mode,
                        previous_oid,
                        entry_mode,
                        oid,
                        path: self.inner.path_clone(),
                    },
                });

                std::ops::ControlFlow::Break(())
            } else {
                std::ops::ControlFlow::Continue(())
            }
        }
    }

    let mut recorder = FindChangeToPath::new(file_path.into());
    let result = gix_diff::tree(parent_tree_iter, tree_iter, state, &odb, &mut recorder);
    stats.trees_diffed += 1;

    match result {
        Ok(_) | Err(gix_diff::tree::Error::Cancelled) => Ok(recorder.change),
        Err(error) => Err(error.raise()),
    }
}

/// Find the file that the commit deleted and that it renamed to the file it added at `file_path` with `mode` and the
/// blob `id`, as Git's `find_rename()` does: the diff of the trees isn't limited to the path, every file that the
/// commit deleted is a source, and the added file is the only destination, so that no other added file can be paired
/// with its source. The source becomes that of the returned [`TreeDiffChange::Rewrite`].
#[expect(clippy::too_many_arguments)]
fn find_rename_at_file_path(
    odb: impl gix_object::Find + gix_object::FindHeader,
    file_path: &BStr,
    mode: EntryMode,
    id: ObjectId,
    stats: &mut Statistics,
    state: &mut gix_diff::tree::State,
    resource_cache: &mut gix_diff::blob::Platform,
    parent_tree_iter: gix_object::TreeRefIter<'_>,
    tree_iter: gix_object::TreeRefIter<'_>,
    rewrites: gix_diff::Rewrites,
) -> Result<Option<TreeDiffChange>> {
    let mut deletions = Deletions {
        inner: gix_diff::tree::Recorder::default().track_location(Some(gix_diff::tree::recorder::Location::Path)),
        files: Vec::new(),
    };
    let result = gix_diff::tree(parent_tree_iter, tree_iter, state, &odb, &mut deletions);
    stats.trees_diffed_with_rewrites += 1;
    if let Err(error) = result {
        return Err(error.raise());
    }

    let sources: Vec<_> = deletions
        .files
        .iter()
        .map(|(location, entry_mode, source_id)| gix_diff::rewrites::rename::Entry {
            location: location.as_bstr(),
            mode: *entry_mode,
            id: source_id,
        })
        .collect();
    let destination = gix_diff::rewrites::rename::Entry {
        location: file_path,
        mode,
        id: &id,
    };
    let mut attributes = Attributes::default();
    attributes.initialize_with_selection(&Default::default(), ["diff"]);
    let mut blobs = RenameBlobs {
        objects: &odb,
        attr_stack: &mut resource_cache.attr_stack,
        drivers: resource_cache.filter.drivers(),
        attributes,
    };
    let options = gix_diff::rewrites::rename::Options {
        min_score: rewrites.percentage.map_or(MAX_SCORE, |percentage| {
            (percentage.clamp(0.0, 1.0) * MAX_SCORE as f32).round() as u32
        }),
        limit: rewrites.limit,
    };
    let outcome = gix_diff::rewrites::rename::detect(&sources, &[destination], &mut blobs, options)?;
    Ok(outcome.renames.first().map(|rename| {
        let (source_location, _, source_id) = &deletions.files[rename.source];
        TreeDiffChange::Rewrite {
            source_location: source_location.clone(),
            source_id: *source_id,
            id,
        }
    }))
}

/// Records the files that a diff of two trees finds deleted.
struct Deletions {
    inner: gix_diff::tree::Recorder,
    /// The location, mode and blob of each deleted file.
    files: Vec<(BString, EntryMode, ObjectId)>,
}

impl Visit for Deletions {
    fn pop_front_tracked_path_and_set_current(&mut self) {
        self.inner.pop_front_tracked_path_and_set_current();
    }

    fn push_back_tracked_path_component(&mut self, component: &BStr) {
        self.inner.push_back_tracked_path_component(component);
    }

    fn push_path_component(&mut self, component: &BStr) {
        self.inner.push_path_component(component);
    }

    fn pop_path_component(&mut self) {
        self.inner.pop_path_component();
    }

    fn visit(&mut self, change: gix_diff::tree::visit::Change) -> gix_diff::tree::visit::Action {
        if let gix_diff::tree::visit::Change::Deletion { entry_mode, oid, .. } = change
            && !entry_mode.is_tree()
        {
            self.files.push((self.inner.path_clone(), entry_mode, oid));
        }
        std::ops::ControlFlow::Continue(())
    }
}

/// The blobs whose similarity rename detection estimates, read as `git diff` reads them: the `diff` attribute of a
/// regular file and the diff drivers decide whether its data is binary.
struct RenameBlobs<'a, O> {
    objects: &'a O,
    attr_stack: &'a mut gix_worktree::Stack,
    /// The configured diff drivers, sorted by name.
    drivers: &'a [gix_diff::blob::Driver],
    /// The `diff` attribute of the file being read.
    attributes: Attributes,
}

impl<O: gix_object::FindObjectOrHeader> RenameBlobs<'_, O> {
    /// Whether Git takes the blob of the file at `location` with `mode` for binary data before reading it:
    /// `Some(true)` for binary data, `Some(false)` for text and `None` if its bytes decide. The `diff` attribute of a
    /// regular file makes it text and `-diff` binary, and `diff=<name>` takes the `binary` setting of that driver. A
    /// name without a configured driver, a regular file without a `diff` attribute and any other file take the
    /// setting of the `default` driver.
    fn binary(&mut self, location: &BStr, mode: EntryMode) -> Result<Option<bool>> {
        let state = if mode.is_blob() {
            self.attr_stack
                .at_entry(location, None, self.objects)
                .or_raise(|| message!("Could not read the attributes of '{location}'"))?
                .matching_attributes(&mut self.attributes);
            self.attributes
                .iter_selected()
                .next()
                .map_or(StateRef::Unspecified, |attribute| attribute.assignment.state)
        } else {
            StateRef::Unspecified
        };
        let drivers = self.drivers;
        let driver = |name: &BStr| {
            drivers
                .binary_search_by(|driver| driver.name.as_bstr().cmp(name))
                .ok()
                .map(|index| &drivers[index])
        };
        Ok(match state {
            StateRef::Set => Some(false),
            StateRef::Unset => Some(true),
            StateRef::Value(name) => driver(name.as_bstr())
                .or_else(|| driver("default".into()))
                .and_then(|driver| driver.is_binary),
            StateRef::Unspecified => driver("default".into()).and_then(|driver| driver.is_binary),
        })
    }
}

impl<O: gix_object::FindObjectOrHeader> gix_diff::rewrites::rename::Blobs for RenameBlobs<'_, O> {
    fn size(&mut self, entry: &gix_diff::rewrites::rename::Entry<'_>) -> Result<u64> {
        let id = entry.id;
        Ok(self
            .objects
            .try_header(id)
            .or_raise(|| message!("Could not find object header for {id}"))?
            .ok_or_raise(|| message!("An object with id {id} could not be found").not_found())?
            .size)
    }

    fn content<'buf>(
        &mut self,
        entry: &gix_diff::rewrites::rename::Entry<'_>,
        buf: &'buf mut Vec<u8>,
    ) -> Result<(&'buf [u8], gix_diff::rewrites::rename::Content)> {
        let binary = self.binary(entry.location, entry.mode)?;
        let data = self.objects.find_blob(entry.id, buf)?.data;
        let content = match binary {
            Some(true) => gix_diff::rewrites::rename::Content::Binary,
            Some(false) => gix_diff::rewrites::rename::Content::Text,
            None => gix_diff::rewrites::rename::Content::from_data(data),
        };
        Ok((data, content))
    }
}

#[expect(clippy::too_many_arguments)]
fn blob_changes(
    odb: impl gix_object::Find + gix_object::FindHeader,
    resource_cache: &mut gix_diff::blob::Platform,
    oid: ObjectId,
    previous_oid: ObjectId,
    file_path: &BStr,
    previous_file_path: &BStr,
    diff_algorithm: gix_diff::blob::Algorithm,
    stats: &mut Statistics,
) -> Result<Vec<Change>> {
    resource_cache.set_resource(
        previous_oid,
        // TODO(blame): add a test to show of symlink blaming works.
        gix_object::tree::EntryKind::Blob,
        previous_file_path,
        gix_diff::blob::ResourceKind::OldOrSource,
        &odb,
    )?;
    resource_cache.set_resource(
        oid,
        gix_object::tree::EntryKind::Blob,
        file_path,
        gix_diff::blob::ResourceKind::NewOrDestination,
        &odb,
    )?;

    let outcome = resource_cache.prepare_diff()?;

    Ok(blob_changes_from_data(
        outcome.old.data.as_slice().unwrap_or_default(),
        outcome.new.data.as_slice().unwrap_or_default(),
        diff_algorithm,
        stats,
    ))
}

fn blob_changes_from_data(
    old_data: &[u8],
    new_data: &[u8],
    diff_algorithm: gix_diff::blob::Algorithm,
    stats: &mut Statistics,
) -> Vec<Change> {
    use gix_diff::blob::Hunk;

    // Git's blame diffs without context lines, so `xdi_diff()` leaves out most of the end both blobs share before
    // diffing them, which limits where a hunk next to that end can be placed. The lines it leaves out are unchanged.
    let left_out_len = shared_end_len(old_data, new_data);
    let old_data = &old_data[..old_data.len() - left_out_len];
    let (new_data, left_out) = new_data.split_at(new_data.len() - left_out_len);
    let input = gix_diff::blob::InternedInput::new(old_data, new_data);

    let mut diff = gix_diff::blob::Diff::compute(diff_algorithm, &input);
    diff.postprocess_lines(&input);

    let mut last_seen_after_end = 0;
    let mut changes = diff.hunks().fold(Vec::new(), |mut hunks, hunk| {
        let Hunk { before, after } = hunk;

        // This checks for unchanged hunks.
        if after.start > last_seen_after_end {
            hunks.push(Change::Unchanged(last_seen_after_end..after.start));
        }

        match (!before.is_empty(), !after.is_empty()) {
            (_, true) => {
                hunks.push(Change::AddedOrReplaced(
                    after.start..after.end,
                    before.end - before.start,
                ));
            }
            (true, false) => {
                hunks.push(Change::Deleted(after.start, before.end - before.start));
            }
            (false, false) => unreachable!("BUG: imara-diff provided a non-change"),
        }

        last_seen_after_end = after.end;

        hunks
    });

    let total_number_of_lines = input.after.len() as u32 + tokens_for_diffing(left_out).tokenize().count() as u32;
    if total_number_of_lines > last_seen_after_end {
        changes.push(Change::Unchanged(last_seen_after_end..total_number_of_lines));
    }

    stats.blobs_diffed += 1;
    changes
}

/// Return the length of the end that `old` and `new` share and that Git leaves out before diffing them without context
/// lines, as `trim_common_tail()` in Git's `xdiff-interface.c` computes it: the whole blocks of 1024 bytes at the end
/// that are equal in both, less the rest of the line the cut falls in, so that what remains of each ends with a
/// complete line.
fn shared_end_len(old: &[u8], new: &[u8]) -> usize {
    const BLOCK_LEN: usize = 1024;
    let shorter_len = old.len().min(new.len());
    let mut len = 0;
    while len + BLOCK_LEN <= shorter_len {
        let (old_end, new_end) = (old.len() - len, new.len() - len);
        if old[old_end - BLOCK_LEN..old_end] != new[new_end - BLOCK_LEN..new_end] {
            break;
        }
        len += BLOCK_LEN;
    }
    let blocks = &old[old.len() - len..];
    let rest_of_cut_line = blocks
        .iter()
        .position(|&byte| byte == b'\n')
        .map_or(len, |newline| newline + 1);
    len - rest_of_cut_line
}

/// The id and mode of the entry at a path in the tree of a commit.
#[derive(Clone, Copy)]
struct PathEntry {
    id: ObjectId,
    mode: EntryMode,
}

/// Whether entries with the modes `a` and `b` are of the same type, as the `S_IFMT` bits of Git's modes tell: two
/// regular files are, executable or not, as are two symlinks, two submodules or two trees.
fn is_same_type(a: EntryMode, b: EntryMode) -> bool {
    (a.is_blob() && b.is_blob()) || a.kind() == b.kind()
}

fn find_path_entry_in_commit(
    odb: &impl gix_object::Find,
    commit: &gix_hash::oid,
    file_path: &BStr,
    cache: Option<&gix_commitgraph::Graph>,
    buf: &mut Vec<u8>,
    buf2: &mut Vec<u8>,
    stats: &mut Statistics,
) -> Result<Option<PathEntry>> {
    let tree_id = find_commit(cache, odb, commit, buf)
        .or_raise(|| message("Could not find existing iterator over a tree"))?
        .tree_id()
        .or_raise(|| message("Failure to decode commit during traversal"))?;
    let tree_iter = odb
        .find_tree_iter(&tree_id, buf)
        .or_raise(|| message("Could not find existing iterator over a tree"))?;
    stats.trees_decoded += 1;

    let res = tree_iter
        .lookup_entry(
            odb,
            buf2,
            file_path.split(|b| *b == b'/').inspect(|_| stats.trees_decoded += 1),
        )
        .or_raise(|| message("Couldn't find commit or tree in the object database"))?;
    stats.trees_decoded -= 1;
    Ok(res.map(|e| PathEntry {
        id: e.oid,
        mode: e.mode,
    }))
}

type ParentIds = SmallVec<[(gix_hash::ObjectId, i64); 2]>;

fn collect_parents(
    commit: gix_traverse::commit::Either<'_, '_>,
    odb: &impl gix_object::Find,
    cache: Option<&gix_commitgraph::Graph>,
    buf: &mut Vec<u8>,
) -> Result<ParentIds> {
    let mut parent_ids: ParentIds = Default::default();
    match commit {
        gix_traverse::commit::Either::CachedCommit(commit) => {
            let cache = cache
                .as_ref()
                .expect("find returned a cached commit, so we expect cache to be present");
            for parent_pos in commit.iter_parents() {
                let parent = cache.commit_at(
                    parent_pos.or_raise(|| message("Failed to get parent from commitgraph during traversal"))?,
                );
                parent_ids.push((parent.id().to_owned(), parent.committer_timestamp() as i64));
            }
        }
        gix_traverse::commit::Either::CommitRefIter(commit_ref_iter) => {
            for id in commit_ref_iter.parent_ids() {
                let parent = odb.find_commit_iter(id.as_ref(), buf).ok();
                let parent_commit_time = parent
                    .and_then(|parent| parent.committer().ok().map(|committer| committer.seconds()))
                    .unwrap_or_default();
                parent_ids.push((id, parent_commit_time));
            }
        }
    }
    Ok(parent_ids)
}

/// Return an iterator over tokens for use in diffing. These are usually lines, but it's important
/// to unify them so the later access shows the right thing.
pub(crate) fn tokens_for_diffing(data: &[u8]) -> impl TokenSource<Token = &[u8]> {
    gix_diff::blob::sources::byte_lines(data)
}

/// The blame input after resolving [`Start`] and before traversing commit history.
struct InitialState {
    /// The final file contents whose lines are being attributed.
    blamed_file_blob: Vec<u8>,
    /// Requested ranges that still need attribution through commit traversal.
    hunks_to_blame: Vec<UnblamedHunk>,
    /// Entries already resolved while comparing worktree contents to the first suspect.
    out: Vec<BlameEntry>,
    /// The first commit to traverse, or `None` if no history traversal is needed.
    first_suspect: Option<ObjectId>,
}

#[expect(clippy::too_many_arguments)]
fn initial_state(
    odb: &impl gix_object::Find,
    start: Start<'_>,
    cache: Option<&gix_commitgraph::Graph>,
    file_path: &BStr,
    options: &Options,
    buf: &mut Vec<u8>,
    buf2: &mut Vec<u8>,
    stats: &mut Statistics,
) -> Result<InitialState> {
    match start {
        Start::Commit(suspect) => {
            let blamed_file_entry_id = find_path_entry_in_commit(&odb, &suspect, file_path, cache, buf, buf2, stats)?
                .ok_or_raise(|| {
                    message!("The file to blame at \"{file_path}\" wasn't found in the first commit at {suspect}")
                        .not_found()
                })?
                .id;
            let blamed_file_blob = odb
                .find_blob(&blamed_file_entry_id, buf)
                .or_raise(|| message("Could not find existing blob or commit"))?
                .data
                .to_vec();
            let num_lines_in_blamed = tokens_for_diffing(&blamed_file_blob).tokenize().count() as u32;

            // Binary or otherwise empty?
            if num_lines_in_blamed == 0 {
                return Ok(InitialState {
                    blamed_file_blob,
                    hunks_to_blame: Vec::new(),
                    out: Vec::new(),
                    first_suspect: None,
                });
            }

            let ranges_to_blame = options.ranges.to_zero_based_exclusive_ranges(num_lines_in_blamed);
            let hunks_to_blame = ranges_to_blame
                .into_iter()
                .map(|range| UnblamedHunk::new(range, suspect))
                .collect::<Vec<_>>();

            Ok(InitialState {
                blamed_file_blob,
                hunks_to_blame,
                out: Vec::new(),
                first_suspect: Some(suspect),
            })
        }
        Start::Contents {
            first_suspect,
            contents,
        } => {
            let null_id = first_suspect.kind().null();
            let blamed_file_blob = contents.into_owned();
            let num_lines_in_blamed = tokens_for_diffing(&blamed_file_blob).tokenize().count() as u32;

            if num_lines_in_blamed == 0 {
                return Ok(InitialState {
                    blamed_file_blob,
                    hunks_to_blame: Vec::new(),
                    out: Vec::new(),
                    first_suspect: None,
                });
            }

            let ranges_to_blame = options.ranges.to_zero_based_exclusive_ranges(num_lines_in_blamed);
            let mut hunks_to_blame = ranges_to_blame
                .into_iter()
                .map(|range| UnblamedHunk::new(range, null_id))
                .collect::<Vec<_>>();

            let mut out = Vec::new();

            let Some(first_suspect_entry) =
                find_path_entry_in_commit(odb, &first_suspect, file_path, cache, buf, buf2, stats)?
            else {
                // File does not exist in the starting commit. Treat the requested
                // ranges as entirely "not committed yet".
                out.extend(
                    hunks_to_blame
                        .iter()
                        .filter_map(|hunk| BlameEntry::from_unblamed_hunk(hunk, null_id)),
                );

                return Ok(InitialState {
                    blamed_file_blob,
                    hunks_to_blame: Vec::new(),
                    out,
                    first_suspect: None,
                });
            };

            let first_suspect_blob = odb
                .find_blob(&first_suspect_entry.id, buf)
                .or_raise(|| message("Could not find existing blob or commit"))?
                .data
                .to_vec();

            let changes = blob_changes_from_data(&first_suspect_blob, &blamed_file_blob, options.diff_algorithm, stats);
            hunks_to_blame = process_changes(hunks_to_blame, changes, null_id, first_suspect);

            unblamed_to_out_is_done(&mut hunks_to_blame, &mut out, null_id);

            let first_suspect = hunks_to_blame
                .iter()
                .any(|hunk| hunk.has_suspect(&first_suspect))
                .then_some(first_suspect);

            Ok(InitialState {
                blamed_file_blob,
                hunks_to_blame,
                out,
                first_suspect,
            })
        }
    }
}
