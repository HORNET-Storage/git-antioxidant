//! Commit the index as `git commit` does: with the parents of a merge in progress, the cleaned message, the commit
//! hooks and the reflog.

#[cfg(feature = "index")]
use std::path::Path;
#[cfg(all(feature = "index", feature = "hooks"))]
use std::{ffi::OsString, path::PathBuf};

#[cfg(feature = "index")]
use gix_hash::ObjectId;
#[cfg(feature = "index")]
use gix_ref::transaction::{PreviousValue, RefEdit};

use crate::{
    Repository, Result,
    commit::message::{Cleanup, CommentPrefix},
    config::{
        cache::util::ApplyLeniencyDefault,
        tree::{Commit, Core, Key},
    },
};
#[cfg(feature = "index")]
use crate::{
    bstr::{BStr, BString, ByteSlice},
    commit::{
        index::{Error as Refused, Merge, Options, Outcome},
        message as msg,
    },
    config::tree::Index,
    error::{ErrorExt, ResultExt, bail, message},
};

/// The file in which `git commit` prepares the message of a commit for its hooks, and from which it reads the message.
#[cfg(feature = "index")]
const COMMIT_EDITMSG: &str = "COMMIT_EDITMSG";
/// The file with the message that `git merge` prepared for the commit of a merge.
#[cfg(feature = "index")]
const MERGE_MSG: &str = "MERGE_MSG";
/// The file with the message that `git merge --squash` prepared.
#[cfg(feature = "index")]
const SQUASH_MSG: &str = "SQUASH_MSG";

impl Repository {
    /// Return the prefix of the comment lines of commit messages, which `core.commentChar` and `core.commentString` set:
    /// Git uses the last value of either key, and `#` if neither is set.
    pub fn comment_prefix(&self) -> Result<CommentPrefix> {
        let mut last = None;
        if let Some(sections) = self
            .config
            .resolved
            .sections_by_name_and_filter("core", self.filter_config_section())
        {
            for section in sections.filter(|section| section.header().subsection_name().is_none()) {
                let key: Option<&'static crate::config::tree::core::CommentPrefix> =
                    section.value_names().fold(None, |key, name| {
                        if name.eq_ignore_ascii_case(Core::COMMENT_CHAR.name()) {
                            Some(&Core::COMMENT_CHAR)
                        } else if name.eq_ignore_ascii_case(Core::COMMENT_STRING.name()) {
                            Some(&Core::COMMENT_STRING)
                        } else {
                            key
                        }
                    });
                if let Some(key) = key {
                    last = Some((key, section.value_implicit(key.name()).flatten()));
                }
            }
        }
        match last {
            Some((key, value)) => key
                .try_into_comment_prefix(value)
                .with_lenient_default(self.config.lenient_config),
            None => Ok(CommentPrefix::default()),
        }
    }

    /// Return how commit messages are cleaned unless something else is asked for, as `commit.cleanup` says, or
    /// [`Cleanup::Default`] if it isn't set.
    pub fn commit_cleanup(&self) -> Result<Cleanup> {
        match self
            .config
            .resolved
            .string_filter(Commit::CLEANUP, &mut self.filter_config_section())
        {
            Some(value) => Commit::CLEANUP
                .try_into_cleanup(value)
                .with_lenient_default(self.config.lenient_config),
            None => Ok(Cleanup::default()),
        }
    }
}

#[cfg(feature = "index")]
impl Repository {
    /// Commit the index as `git commit` does without paths and without an editor, and point `HEAD`, or the branch that
    /// it names, to the new commit, as `cmd_commit()` and `prepare_to_commit()` in `builtin/commit.c` do.
    ///
    /// * The tree of the commit is that of the index, written with [`gix_index::State::write_tree()`]. Before the hooks run,
    ///   the index gets the cache tree of its entries and is written back if that changed it, as Git leaves it.
    /// * The parents are the commit of `HEAD` and, while a merge is in progress, the commits that `MERGE_HEAD` lists, of
    ///   which those that are reachable from another parent are left out unless `MERGE_MODE` says `no-ff`, as
    ///   `reduce_heads()` leaves them out. The first commit of an unborn branch has no parent. With
    ///   [`amend`](Options::amend), the commit replaces that of `HEAD` and takes its parents, its extra headers but
    ///   its signature, and its message and author unless others are given.
    /// * The message is [`Options::message`], or that of `MERGE_MSG` preceded by `SQUASH_MSG` or that of `SQUASH_MSG`,
    ///   cleaned as [`Options::cleanup`] or [`commit.cleanup`](Self::commit_cleanup()) say, for a message that was edited
    ///   in an editor if [`Options::edited`] is set, with the comment prefix of
    ///   [`comment_prefix()`](Self::comment_prefix()). Git prepares it in `$GIT_DIR/COMMIT_EDITMSG`, whose leading and
    ///   trailing whitespace it removes unless the cleanup is `verbatim`, and which the hooks receive, may change and
    ///   from which it is read for the commit.
    /// * The author and committer are those of [`author()`](Self::author()) and [`committer()`](Self::committer()),
    ///   unless [`Options::author`] or the amended commit give the author, and the commit is signed through
    ///   [`commit_signing_options()`](Self::commit_signing_options()) if [`Options::sign`] or `commit.gpgSign` ask for
    ///   it. The parents of a merge that `MERGE_HEAD` names with signed tags get their `mergetag` headers.
    /// * The hooks run as Git runs them, with `GIT_INDEX_FILE`, `GIT_EDITOR=:` and the `GIT_AUTHOR_*` variables of the
    ///   author: `pre-commit` before the message is prepared, `prepare-commit-msg` with the message file and its
    ///   source after that, then `commit-msg`, and once `HEAD` changed, `post-commit` and, after an amended commit,
    ///   `post-rewrite` with `amend`. [`Options::no_verify`] skips `pre-commit` and `commit-msg`. No commit is made if
    ///   `pre-commit`, `prepare-commit-msg` or `commit-msg` fail, and the failures of the others are ignored. Without the
    ///   `hooks` feature of `gix`, no hook runs. With it, no commit is made in a repository whose Git directory isn't
    ///   fully trusted, as `Repository::hooks()` only returns the hooks of trusted ones.
    /// * `HEAD`, or its branch, changes with Git's reflog messages `commit: <subject>`, `commit (initial): <subject>`,
    ///   `commit (amend): <subject>` or `commit (merge): <subject>`, unless [`Options::reflog_action`] replaces the
    ///   action. Then the state of a merge goes away as `git commit` removes it: the pseudo-reference `AUTO_MERGE`,
    ///   `MERGE_HEAD`, `MERGE_MSG`, `MERGE_MODE` and `SQUASH_MSG`. Failures to remove them are ignored, as in Git.
    /// * With [`Options::merge`], the merge is committed as `git merge` commits it, as [`Merge`] describes.
    ///
    /// ### Refusals
    ///
    /// Where `git commit` refuses or stops, a [`commit::index::Error`](crate::commit::index::Error) is returned and no
    /// commit is made: in a bare repository, with unmerged entries, with [`amend`](Options::amend) without a commit or
    /// during a merge, if a hook fails, if the message is empty after cleaning unless
    /// [`allow_empty_message`](Options::allow_empty_message) is set, and if the commit records the tree of its first
    /// parent unless [`allow_empty`](Options::allow_empty) is set, it concludes a merge, or it amends a merge. A message
    /// with a NUL byte is refused as well.
    ///
    /// ### Without the `merge` feature
    ///
    /// `gix-merge` reads `MERGE_HEAD`, so without the `merge` feature of `gix`, no commit is made while `MERGE_HEAD`
    /// exists, as the commit would lose the merged parents.
    ///
    /// ### Errors after the commit
    ///
    /// The hooks that run after `HEAD` changed can fail to run, as when waiting for them fails, which returns an error
    /// even though the commit was made, as Git stops there too.
    ///
    /// ### Deviation
    ///
    /// * No editor is started, so unless [`Options::edited`] is set, `default` and `scissors` clean up whitespace only and
    ///   keep comment lines, as Git does for a message given with `-m`. `commit.template`, `commit.verbose`, `--signoff`
    ///   and trailers aren't supported.
    /// * The stat information of the entries of the index isn't refreshed before the commit, as Git refreshes it.
    /// * `CHERRY_PICK_HEAD` and `REVERT_HEAD` aren't read, so the commit of a cherry-pick or revert in progress is made
    ///   as any other commit, and they stay in place, where Git takes the author of the picked commit and removes them.
    /// * `MERGE_HEAD` is read and checked before the hooks run, where Git reads it after them.
    /// * `i18n.commitEncoding` isn't read: commits have no `encoding` header, and the message of an amended commit is
    ///   taken as it is.
    /// * Notes aren't copied to the amended commit as `notes.rewrite.amend` and `notes.rewriteRef` say, and
    ///   `MERGE_AUTOSTASH` isn't applied.
    #[doc(alias = "git commit")]
    pub fn commit_index(&self, options: Options) -> Result<Outcome> {
        if self.workdir().is_none() {
            bail!(Refused::BareRepository);
        }
        if let Some(merge) = &options.merge {
            return self.commit_merge(&options, merge);
        }
        let git_dir = self.current_dir().join(self.git_dir());
        let mut head = self.head()?;
        let current = if head.is_unborn() {
            None
        } else {
            Some(head.peel_to_commit()?)
        };
        let reference = match head.referent_name() {
            Some(name) => name.to_owned(),
            None => gix_ref::FullName::try_from("HEAD").or_error()?,
        };
        let previous = current.as_ref().map(|commit| commit.id);
        let merge = self.merge_in_progress(&git_dir)?;
        if options.amend {
            if current.is_none() {
                bail!(Refused::NothingToAmend);
            }
            if merge.is_some() {
                bail!(Refused::AmendInMerge);
            }
        }
        let amended = current.as_ref().filter(|_| options.amend);

        let author = match &options.author {
            Some(author) => author.clone(),
            None => match amended {
                Some(commit) => commit.decode()?.author()?.to_owned()?,
                None => self
                    .author()
                    .ok_or_else(|| message("Author identity is not configured").validation_error())??
                    .to_owned()?,
            },
        };
        let committer = self
            .committer()
            .ok_or_else(|| message("Committer identity is not configured").validation_error())??;

        // As `prepare_index()` does for a commit of the index as it is, write the trees of the index and keep its cache
        // tree, before the hooks see the index.
        let (index, mut written) = self.write_index_tree()?;

        let message_path = git_dir.join(COMMIT_EDITMSG);
        #[cfg(feature = "hooks")]
        let hooks = CommitHooks::new(self, &message_path, author_env(&author)?)?;
        #[cfg(feature = "hooks")]
        let pre_commit_ran =
            !options.no_verify && hooks.run_or_stop("pre-commit", gix_hook::Invocation::pre_commit(hooks.commit()))?;
        #[cfg(not(feature = "hooks"))]
        let pre_commit_ran = false;

        let mode = match options.cleanup {
            Some(cleanup) => cleanup,
            None => self.commit_cleanup()?,
        }
        .mode(options.edited);
        let (prepared, source) = prepare_message(&options, amended, merge.is_some(), &git_dir)?;
        let prepared = if mode == msg::Mode::None {
            prepared
        } else {
            msg::strip_space(prepared.as_bstr(), None)
        };
        std::fs::write(&message_path, &prepared)
            .or_raise(|| message!("Could not write the commit message to {}", message_path.display()))?;
        let comment_prefix = self.comment_prefix()?.for_message(prepared.as_bstr())?;

        // `prepare_to_commit()` decides with the index from before `pre-commit` whether there is anything to commit.
        let first_parent_tree = match current.as_ref() {
            Some(commit) if !options.amend => Some(commit.tree_id()?.detach()),
            Some(commit) => match commit.parent_ids().next() {
                Some(parent) => Some(self.find_commit(parent.detach())?.tree_id()?.detach()),
                None => None,
            },
            None => None,
        };
        let committable = match first_parent_tree {
            Some(tree) => written.tree_id != tree,
            None => index
                .entries()
                .iter()
                .any(|entry| !entry.flags.contains(gix_index::entry::Flags::INTENT_TO_ADD)),
        };
        let amends_merge = amended.is_some_and(|commit| commit.parent_ids().count() > 1);
        if !committable && merge.is_none() && !options.allow_empty && !amends_merge {
            bail!(Refused::NothingToCommit);
        }
        if pre_commit_ran {
            let index = self.read_index_file()?;
            written = index.write_tree(index.tree(), self, Default::default())?;
        }
        let tree_id = written.tree_id;

        #[cfg(feature = "hooks")]
        {
            hooks.run_or_stop(
                "prepare-commit-msg",
                gix_hook::Invocation::prepare_commit_msg(hooks.commit(), &hooks.message_file, source.to_hook()),
            )?;
            if !options.no_verify {
                hooks.run_or_stop(
                    "commit-msg",
                    gix_hook::Invocation::commit_msg(hooks.commit(), &hooks.message_file),
                )?;
            }
        }
        #[cfg(not(feature = "hooks"))]
        let _ = source;

        let (default_action, parents, merge_tags) = match (&current, &merge) {
            (None, _) => ("commit (initial)", Vec::new(), Vec::new()),
            (Some(commit), _) if options.amend => (
                "commit (amend)",
                commit.parent_ids().map(crate::Id::detach).collect(),
                Vec::new(),
            ),
            (Some(commit), Some(merge)) => {
                let (parents, tags) = self.merge_parents(commit.id, merge)?;
                ("commit (merge)", parents, tags)
            }
            (Some(commit), None) => ("commit", vec![commit.id], Vec::new()),
        };

        let raw = std::fs::read(&message_path)
            .or_raise(|| message!("Could not read the commit message from {}", message_path.display()))?;
        let cleaned = msg::clean(raw.as_bstr(), mode, comment_prefix.as_bstr());
        if msg::is_empty(cleaned.as_bstr(), mode) && !options.allow_empty_message {
            bail!(Refused::EmptyMessage);
        }
        if cleaned.contains(&0) {
            bail!(crate::error::validation(
                "a NUL byte in commit log message not allowed."
            ));
        }

        let extra_headers = match amended {
            Some(commit) => commit
                .decode()?
                .extra_headers
                .iter()
                .filter(|(name, _)| *name != "gpgsig" && *name != "gpgsig-sha256")
                .map(|(name, value)| ((*name).to_owned(), value.clone().into_owned()))
                .collect(),
            None => merge_tags,
        };
        let mut commit = gix_object::Commit {
            tree: tree_id,
            parents: parents.iter().copied().collect(),
            author,
            committer: committer.to_owned()?,
            encoding: None,
            message: cleaned.clone(),
            extra_headers,
        };
        if self.wants_signature(options.sign)? {
            commit = self.sign_commit(commit)?;
        }
        let commit_id = self.write_object(&commit)?.detach();

        // `update_head_with_reflog()` updates `HEAD` through to its branch.
        let action = options.reflog_action.clone().unwrap_or_else(|| default_action.into());
        let expected = match previous {
            Some(previous) => PreviousValue::MustExistAndMatch(gix_ref::Target::Object(previous)),
            None => PreviousValue::MustNotExist,
        };
        self.edit_references_as(
            Some(
                RefEdit::update(
                    gix_ref::FullName::try_from("HEAD").or_error()?,
                    commit_id,
                    expected,
                    msg::reflog_message(action.as_bstr(), cleaned.as_bstr()),
                )
                .with_deref(true),
            ),
            Some(committer),
        )?;
        self.remove_commit_state(&git_dir);

        #[cfg(feature = "hooks")]
        {
            hooks.run(gix_hook::Invocation::post_commit(hooks.commit()))?;
            if let Some(previous) = previous.filter(|_| options.amend) {
                hooks.run(gix_hook::Invocation::post_rewrite(
                    gix_hook::invocation::Rewrite::Amend,
                    [(previous.as_ref(), commit_id.as_ref())],
                ))?;
            }
        }

        Ok(Outcome {
            commit_id,
            tree_id,
            parents,
            message: cleaned,
            reference,
            previous,
        })
    }

    /// Commit `merge` as `git merge` commits a merge without conflicts, as [`Merge`] describes, for
    /// [`commit_index()`](Self::commit_index()) with [`Options::merge`].
    #[cfg(feature = "merge")]
    fn commit_merge(&self, options: &Options, merge: &Merge) -> Result<Outcome> {
        if options.amend {
            bail!(Refused::AmendInMerge);
        }
        let git_dir = self.current_dir().join(self.git_dir());
        let mut head = self.head()?;
        if head.is_unborn() {
            bail!(message("A merge can only be committed on top of the commit of HEAD").validation_error());
        }
        let current = head.peel_to_commit()?;
        let reference = match head.referent_name() {
            Some(name) => name.to_owned(),
            None => gix_ref::FullName::try_from("HEAD").or_error()?,
        };
        let author = match &options.author {
            Some(author) => author.clone(),
            None => self
                .author()
                .ok_or_else(|| message("Author identity is not configured").validation_error())??
                .to_owned()?,
        };
        let committer = self
            .committer()
            .ok_or_else(|| message("Committer identity is not configured").validation_error())??;

        // As `write_tree_trivial()` does, write the trees of the index before the hooks run, whose changes of the index
        // don't reach the commit.
        let (_, written) = self.write_index_tree()?;
        let mode = match options.cleanup {
            Some(cleanup) => cleanup,
            None => self.commit_cleanup()?,
        }
        .mode(options.edited);
        let (prepared, _) = prepare_message(options, None, true, &git_dir)?;
        let state = |message: BString| gix_merge::state::State {
            heads: merge.heads.clone(),
            message: Some(message),
            no_ff: merge.no_ff,
        };
        #[cfg(feature = "hooks")]
        let hooks = CommitHooks::new(
            self,
            &git_dir.join(MERGE_MSG),
            merge
                .hook_env
                .iter()
                .map(|(name, value)| (name.clone(), Some(value.clone())))
                .collect(),
        )?;
        // `prepare_to_commit()` in `builtin/merge.c` runs the hooks and reads the message back.
        let cleaned = (|| -> Result<BString> {
            #[cfg(feature = "hooks")]
            if !options.no_verify {
                hooks.run_or_stop(
                    "pre-merge-commit",
                    gix_hook::Invocation::pre_merge_commit(hooks.commit()),
                )?;
            }
            state(prepared.clone()).write(&git_dir)?;
            #[cfg(feature = "hooks")]
            {
                hooks.run_or_stop(
                    "prepare-commit-msg",
                    gix_hook::Invocation::prepare_commit_msg(
                        hooks.commit(),
                        &hooks.message_file,
                        Some(gix_hook::invocation::MessageSource::Merge),
                    ),
                )?;
                if !options.no_verify {
                    hooks.run_or_stop(
                        "commit-msg",
                        gix_hook::Invocation::commit_msg(hooks.commit(), &hooks.message_file),
                    )?;
                }
            }
            let path = git_dir.join(MERGE_MSG);
            let raw = std::fs::read(&path)
                .or_raise(|| message!("Could not read the merge message from {}", path.display()))?;
            let comment_prefix = match self.comment_prefix()? {
                CommentPrefix::Fixed(prefix) => prefix,
                CommentPrefix::Auto => "#".into(),
            };
            let cleaned = msg::clean(raw.as_bstr(), mode, comment_prefix.as_bstr());
            if cleaned.is_empty() && !options.allow_empty_message {
                bail!(Refused::EmptyMessage);
            }
            Ok(cleaned)
        })();
        let cleaned = match cleaned {
            Ok(cleaned) => cleaned,
            Err(err) => {
                // As `abort_commit()` does, leave the merge for `git commit` to conclude.
                let mut message = prepared;
                message.push(b'\n');
                state(message).write(&git_dir)?;
                return Err(err);
            }
        };
        if cleaned.contains(&0) {
            bail!(crate::error::validation(
                "a NUL byte in commit log message not allowed."
            ));
        }

        let (parents, merge_tags) = self.merge_parents(
            current.id,
            &MergeInProgress {
                heads: merge.heads.clone(),
                no_ff: merge.no_ff,
            },
        )?;
        let mut commit = gix_object::Commit {
            tree: written.tree_id,
            parents: parents.iter().copied().collect(),
            author,
            committer: committer.to_owned()?,
            encoding: None,
            message: cleaned.clone(),
            extra_headers: merge_tags,
        };
        if self.wants_signature(options.sign)? {
            commit = self.sign_commit(commit)?;
        }
        let commit_id = self.write_object(&commit)?.detach();

        // `finish()` updates `HEAD` through to its branch.
        self.edit_references_as(
            Some(
                RefEdit::update(
                    gix_ref::FullName::try_from("HEAD").or_error()?,
                    commit_id,
                    PreviousValue::MustExistAndMatch(gix_ref::Target::Object(current.id)),
                    msg::normalize_reflog_message(merge.reflog_message.as_bstr()),
                )
                .with_deref(true),
            ),
            Some(committer),
        )?;
        #[cfg(feature = "hooks")]
        let post_merge = hooks.run(gix_hook::Invocation::post_merge(false));
        self.remove_merge_state(&git_dir);
        #[cfg(feature = "hooks")]
        post_merge?;

        Ok(Outcome {
            commit_id,
            tree_id: written.tree_id,
            parents,
            message: cleaned,
            reference,
            previous: Some(current.id),
        })
    }

    /// Refuse to commit a merge as `git merge` does, as `gix-merge` writes the state of a merge.
    #[cfg(not(feature = "merge"))]
    fn commit_merge(&self, _options: &Options, _merge: &Merge) -> Result<Outcome> {
        bail!(Refused::MergeUnsupported)
    }

    /// Lock the index, refuse its unmerged entries and write its trees, as `prepare_index()` and `write_tree_trivial()` do
    /// for a commit of the index as it is, and give the index the cache tree of these trees, writing it back if that
    /// changed it. Return the index and the written trees.
    fn write_index_tree(&self) -> Result<(gix_index::File, gix_index::write_tree::Outcome)> {
        let LockedIndex { lock, mut index } = self.lock_index()?;
        if index.entries().iter().any(|entry| entry.stage_raw() != 0) {
            bail!(Refused::UnmergedEntries);
        }
        let written = index.write_tree(index.tree(), self, Default::default())?;
        if index.tree() == Some(&written.cache_tree) {
            drop(lock);
        } else {
            index.set_tree(written.cache_tree.clone());
            self.write_locked_index(lock, &index)?;
        }
        Ok((index, written))
    }

    /// Return whether a commit is signed: as `sign` says, or without it, as `commit.gpgSign` says.
    fn wants_signature(&self, sign: Option<bool>) -> Result<bool> {
        Ok(match sign {
            Some(sign) => sign,
            None => Commit::GPG_SIGN
                .enrich_error(self.config.resolved.boolean(Commit::GPG_SIGN))
                .with_lenient_default(self.config.lenient_config)?
                .unwrap_or_default(),
        })
    }

    /// Read the merge in progress that a commit concludes from `MERGE_HEAD` and `MERGE_MODE` in `git_dir`, or return
    /// `None` if `MERGE_HEAD` doesn't exist.
    fn merge_in_progress(&self, git_dir: &Path) -> Result<Option<MergeInProgress>> {
        #[cfg(feature = "merge")]
        {
            Ok(
                gix_merge::state::State::read(git_dir, self.object_hash())?.map(|state| MergeInProgress {
                    heads: state.heads,
                    no_ff: state.no_ff,
                }),
            )
        }
        #[cfg(not(feature = "merge"))]
        {
            if git_dir
                .join("MERGE_HEAD")
                .try_exists()
                .or_raise(|| message("Could not check whether MERGE_HEAD exists"))?
            {
                bail!(Refused::MergeUnsupported);
            }
            Ok(None)
        }
    }

    /// Return the parents of the commit of `merge` with `head`: `head` and the commits that `MERGE_HEAD` lists, which are
    /// peeled from tags, as `cmd_commit()` takes them with `get_merge_parent()`, without those that are reachable from
    /// another one unless the merge is `no-ff`, as `reduce_heads_replace()` leaves them out. Also return the `mergetag`
    /// headers of the parents that were named with signed tags, as `append_merge_tag_headers()` adds them.
    fn merge_parents(&self, head: ObjectId, merge: &MergeInProgress) -> Result<(Vec<ObjectId>, ExtraHeaders)> {
        let mut parents = vec![head];
        let mut tags: Vec<(ObjectId, ObjectId)> = Vec::new();
        for id in &merge.heads {
            let object = self.find_object(*id).or_raise(|| corrupt_merge_head(id))?;
            let tag = (object.kind == gix_object::Kind::Tag).then_some(object.id);
            let commit = object.peel_to_commit().or_raise(|| corrupt_merge_head(id))?.id;
            if let Some(tag) = tag
                && !tags.iter().any(|(tagged, _)| *tagged == commit)
            {
                tags.push((commit, tag));
            }
            parents.push(commit);
        }
        #[cfg(feature = "merge")]
        if !merge.no_ff {
            parents = self.reduce_heads(parents)?;
        }
        let mut headers = Vec::new();
        for parent in &parents {
            if let Some((_, tag)) = tags.iter().find(|(tagged, _)| tagged == parent)
                && let Some(header) = self.merge_tag_header(*tag)?
            {
                headers.push(header);
            }
        }
        Ok((parents, headers))
    }

    /// Return `heads` without duplicates and without the commits that are reachable from another one of them, keeping the
    /// order of the others, as `reduce_heads()` in `commit-reach.c` does.
    #[cfg(feature = "merge")]
    fn reduce_heads(&self, heads: Vec<ObjectId>) -> Result<Vec<ObjectId>> {
        let mut unique: Vec<ObjectId> = Vec::with_capacity(heads.len());
        for id in heads {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        if unique.len() < 2 {
            return Ok(unique);
        }
        let cache = self.commit_graph_if_enabled()?;
        let mut graph: gix_revwalk::Graph<'_, '_, gix_revwalk::graph::Commit<gix_revision::merge_base::Flags>> =
            self.revision_graph(cache.as_ref())?;
        let mut kept = Vec::with_capacity(unique.len());
        for (index, &candidate) in unique.iter().enumerate() {
            let mut reachable = false;
            for (other_index, &other) in unique.iter().enumerate() {
                if index == other_index {
                    continue;
                }
                let bases = gix_revision::merge_base(candidate, &[other], &mut graph)?;
                if bases.is_some_and(|bases| bases.iter().any(|base| *base == candidate)) {
                    reachable = true;
                    break;
                }
            }
            if !reachable {
                kept.push(candidate);
            }
        }
        Ok(kept)
    }

    /// Return the `mergetag` header for the tag with id `tag` if it is signed, which holds the whole tag, as
    /// `handle_signed_tag()` in `commit.c` writes it.
    fn merge_tag_header(&self, tag: ObjectId) -> Result<Option<(BString, BString)>> {
        let object = self.find_object(tag)?;
        let signed = object.try_to_tag_ref()?.signature.is_some();
        Ok(signed.then(|| ("mergetag".into(), object.data.as_bstr().to_owned())))
    }

    /// Sign `commit` with the signing options of the configuration, as `commit_tree_extended()` signs it.
    #[cfg(feature = "command")]
    fn sign_commit(&self, commit: gix_object::Commit) -> Result<gix_object::Commit> {
        let options = crate::commit::sign::signing_options(self)?;
        commit.sign(options).or_raise(|| message("Could not sign the commit"))
    }

    /// Refuse to sign `commit`, as no signing program can be started without the `command` feature.
    #[cfg(not(feature = "command"))]
    fn sign_commit(&self, _commit: gix_object::Commit) -> Result<gix_object::Commit> {
        bail!(Refused::SigningUnsupported)
    }

    /// Remove the state of a merge after a commit, as `cmd_commit()` removes it: the pseudo-reference `AUTO_MERGE`, as
    /// `sequencer_post_commit_cleanup()` deletes it, then `MERGE_HEAD`, `MERGE_MSG`, `MERGE_MODE` and `SQUASH_MSG`. As
    /// in Git, failures are ignored, as the commit was made.
    fn remove_commit_state(&self, git_dir: &Path) {
        let _ = self.delete_auto_merge();
        for name in ["MERGE_HEAD", MERGE_MSG, "MERGE_MODE", SQUASH_MSG] {
            let _ = std::fs::remove_file(git_dir.join(name));
        }
    }

    /// Remove the state of a merge as `remove_merge_branch_state()` in `branch.c` removes it once `git merge` is done or
    /// quit: `MERGE_HEAD`, `MERGE_RR`, `MERGE_MSG`, `MERGE_MODE` and the pseudo-reference `AUTO_MERGE`. As in Git, failures
    /// are ignored.
    #[cfg(feature = "merge")]
    pub(crate) fn remove_merge_state(&self, git_dir: &Path) {
        for name in ["MERGE_HEAD", "MERGE_RR", MERGE_MSG, "MERGE_MODE"] {
            let _ = std::fs::remove_file(git_dir.join(name));
        }
        let _ = self.delete_auto_merge();
    }

    /// Delete the pseudo-reference `AUTO_MERGE` along with its reflog if it exists, without following it, as
    /// `refs_delete_ref()` deletes it with `REF_NO_DEREF`.
    pub(crate) fn delete_auto_merge(&self) -> Result<()> {
        self.edit_reference(RefEdit::delete(
            gix_ref::FullName::try_from("AUTO_MERGE").or_error()?,
            PreviousValue::Any,
        ))?;
        Ok(())
    }

    /// Lock the index file and read the index, or start with an empty one if there is no index file, as Git does with
    /// `repo_hold_locked_index()` and `repo_read_index()` before it changes the index. The lock fails immediately if the
    /// index is locked already.
    pub(crate) fn lock_index(&self) -> Result<LockedIndex> {
        let shared_repository_permissions = crate::config::file_mut::shared_repository_permissions(
            &self.config.resolved,
            self.filter_config_section(),
        )?;
        let lock = gix_lock::File::acquire(
            self.index_path(),
            gix_lock::acquire::Fail::Immediately,
            None,
            shared_repository_permissions,
            Some(&gix_lock::acquire::resolve_symlink),
        )
        .or_raise(|| message("Could not lock the index"))?;
        Ok(LockedIndex {
            lock,
            index: self.read_index_file()?,
        })
    }

    /// Read the index file, or return an empty index if there is none.
    pub(crate) fn read_index_file(&self) -> Result<gix_index::File> {
        let path = self.index_path();
        if path
            .try_exists()
            .or_raise(|| message!("Could not check whether the index exists at {}", path.display()))?
        {
            self.open_index()
        } else {
            Ok(gix_index::File::from_state(
                gix_index::State::new(self.object_hash()),
                path,
            ))
        }
    }

    /// Write `index` into `lock` as `index.skipHash` says and replace the index file with it, as `write_locked_index()`
    /// does with `COMMIT_LOCK`.
    pub(crate) fn write_locked_index(&self, lock: gix_lock::File, index: &gix_index::File) -> Result<()> {
        let skip_hash = Index::SKIP_HASH
            .enrich_error(self.config.resolved.boolean(Index::SKIP_HASH))
            .with_lenient_default(self.config.lenient_config)?
            .unwrap_or_default();
        let mut out = std::io::BufWriter::new(lock);
        index.write_to(
            &mut out,
            gix_index::write::Options {
                skip_hash,
                ..Default::default()
            },
        )?;
        let lock = out
            .into_inner()
            .map_err(std::io::IntoInnerError::into_error)
            .or_raise(|| message("Could not write the index"))?;
        lock.commit()
            .or_raise(|| message("Could not replace the index with its lock file"))?;
        Ok(())
    }
}

/// The index of a repository, read while its lock file is held, as Git holds `index.lock` while it changes the index.
#[cfg(feature = "index")]
pub(crate) struct LockedIndex {
    /// The lock of the index file, which replaces the index file once it is committed.
    pub(crate) lock: gix_lock::File,
    /// The index as it was read once the lock was held, or an empty one if there was no index file.
    pub(crate) index: gix_index::File,
}

/// The extra headers of a commit, as names with their values.
#[cfg(feature = "index")]
type ExtraHeaders = Vec<(BString, BString)>;

/// A merge in progress, which a commit concludes.
#[cfg(feature = "index")]
#[cfg_attr(not(feature = "merge"), allow(dead_code))]
struct MergeInProgress {
    /// The commits that `MERGE_HEAD` lists.
    heads: Vec<ObjectId>,
    /// Whether `MERGE_MODE` says `no-ff`, which keeps all parents.
    no_ff: bool,
}

/// Where the message of a commit comes from, which `prepare-commit-msg` receives.
#[cfg(feature = "index")]
#[derive(Debug, Clone, Copy)]
enum Source {
    /// Nowhere, so the message is empty.
    None,
    /// The caller.
    Message,
    /// The amended commit of `HEAD`.
    Commit,
    /// `MERGE_MSG`, or a merge in progress without it.
    Merge,
    /// `SQUASH_MSG`.
    Squash,
}

#[cfg(all(feature = "index", feature = "hooks"))]
impl Source {
    /// Return the source as `prepare-commit-msg` receives it.
    fn to_hook(self) -> Option<gix_hook::invocation::MessageSource<'static>> {
        use gix_hook::invocation::MessageSource;
        match self {
            Source::None => None,
            Source::Message => Some(MessageSource::Message),
            Source::Commit => Some(MessageSource::Commit(std::ffi::OsStr::new("HEAD"))),
            Source::Merge => Some(MessageSource::Merge),
            Source::Squash => Some(MessageSource::Squash),
        }
    }
}

/// The hooks of a commit, with what Git passes to them.
#[cfg(all(feature = "index", feature = "hooks"))]
struct CommitHooks {
    /// The hooks of the repository.
    hooks: gix_hook::Hooks,
    /// The index of the commit as Git names it in `GIT_INDEX_FILE`.
    index_file: PathBuf,
    /// `COMMIT_EDITMSG` as Git names it to the hooks.
    message_file: PathBuf,
    /// The `GIT_AUTHOR_*` variables that `determine_author_info()` exports for the hooks.
    env: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>,
}

#[cfg(all(feature = "index", feature = "hooks"))]
impl CommitHooks {
    /// Prepare to run the hooks of `repo` for a commit whose message is prepared in `message_path`, which receive the
    /// variables of `env` in addition to those of the commit.
    fn new(repo: &Repository, message_path: &Path, env: Vec<(OsString, Option<OsString>)>) -> Result<Self> {
        Ok(CommitHooks {
            hooks: repo.hooks()?,
            index_file: hook_path(repo, &repo.index_path())?,
            message_file: hook_path(repo, message_path)?,
            env,
        })
    }

    /// Return what the hooks of a commit learn about it.
    fn commit(&self) -> gix_hook::invocation::Commit<'_> {
        gix_hook::invocation::Commit {
            index_file: &self.index_file,
            editor: gix_hook::invocation::Editor::NotUsed,
        }
    }

    /// Run the hooks of `invocation`, whose failure Git ignores.
    fn run(&self, mut invocation: gix_hook::Invocation<'_>) -> Result<()> {
        invocation.env.extend(self.env.iter().cloned());
        self.hooks.run(&invocation)?;
        Ok(())
    }

    /// Run the hooks of `invocation` for `event`, whose failure stops the commit, and return `true` if any ran.
    fn run_or_stop(&self, event: &'static str, mut invocation: gix_hook::Invocation<'_>) -> Result<bool> {
        invocation.env.extend(self.env.iter().cloned());
        let outcome = self.hooks.run(&invocation)?;
        if !outcome.success() {
            bail!(Refused::HookFailed {
                event,
                code: outcome.code(),
            });
        }
        Ok(!outcome.hooks.is_empty())
    }
}

/// Return the `GIT_AUTHOR_*` variables that `determine_author_info()` exports for the hooks of a commit by `author`.
#[cfg(all(feature = "index", feature = "hooks"))]
fn author_env(author: &gix_actor::Signature) -> Result<Vec<(OsString, Option<OsString>)>> {
    let mut date = b"@".to_vec();
    author.time.write_to(&mut date).or_error()?;
    [
        ("GIT_AUTHOR_NAME", author.name.as_bstr()),
        ("GIT_AUTHOR_EMAIL", author.email.as_bstr()),
        ("GIT_AUTHOR_DATE", date.as_bstr()),
    ]
    .into_iter()
    .map(|(name, value)| {
        Ok((
            name.into(),
            Some(gix_path::from_bstr(value)?.into_owned().into_os_string()),
        ))
    })
    .collect()
}

/// Return `path` as Git passes it to hooks: relative to the root of the worktree if the Git directory of `repo` is the
/// `.git` directory there, as `git` names its files when it runs in such a worktree, and absolute otherwise, with slashes
/// on Windows as in Git for Windows.
#[cfg(all(feature = "index", feature = "hooks"))]
fn hook_path(repo: &Repository, path: &Path) -> Result<PathBuf> {
    let current_dir = repo.current_dir();
    let path = current_dir.join(path);
    let relative = repo
        .workdir()
        .map(|workdir| current_dir.join(workdir))
        .filter(|workdir| current_dir.join(repo.git_dir()) == workdir.join(".git"))
        .and_then(|workdir| path.strip_prefix(workdir).ok().map(Path::to_path_buf));
    let path = gix_path::to_unix_separators_on_windows(gix_path::into_bstr(relative.unwrap_or(path))?);
    Ok(gix_path::from_bstr(path)?.into_owned())
}

/// Return the message that `prepare_to_commit()` prepares without an editor and where it comes from: that of `options`,
/// that of the `amended` commit, that of `MERGE_MSG` preceded by `SQUASH_MSG` if both exist, or that of `SQUASH_MSG`.
/// While `merging` without them, it is empty and comes from the merge.
#[cfg(feature = "index")]
fn prepare_message(
    options: &Options,
    amended: Option<&crate::Commit<'_>>,
    merging: bool,
    git_dir: &Path,
) -> Result<(BString, Source)> {
    if let Some(message) = &options.message {
        return Ok((message.clone(), Source::Message));
    }
    if let Some(commit) = amended {
        return Ok((skip_blank_lines(commit.decode()?.message).to_owned(), Source::Commit));
    }
    let squash = read_optional(&git_dir.join(SQUASH_MSG))?;
    Ok(match (read_optional(&git_dir.join(MERGE_MSG))?, squash) {
        (Some(merge), Some(mut squash)) => {
            squash.extend_from_slice(&merge);
            (squash.into(), Source::Squash)
        }
        (Some(merge), None) => (merge.into(), Source::Merge),
        (None, Some(squash)) => (squash.into(), Source::Squash),
        (None, None) if merging => (BString::default(), Source::Merge),
        (None, None) => (BString::default(), Source::None),
    })
}

/// Return `message` without its leading lines that have nothing but whitespace, as `skip_blank_lines()` in `commit.c`
/// skips them.
#[cfg(feature = "index")]
fn skip_blank_lines(message: &BStr) -> &BStr {
    let mut rest = message;
    while let Some(line) = rest.lines_with_terminator().next() {
        if line.iter().any(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r')) {
            break;
        }
        rest = &rest[line.len()..];
    }
    rest
}

/// Return the content of the file at `path`, or `None` if it doesn't exist.
#[cfg(feature = "index")]
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.and_raise(message!("Could not read {}", path.display()))),
    }
}

/// Return the error that `cmd_commit()` reports for a line of `MERGE_HEAD` that names no commit.
#[cfg(feature = "index")]
fn corrupt_merge_head(id: &ObjectId) -> gix_error::Message {
    message!("Corrupt MERGE_HEAD file ({id})").corrupted()
}
