//! The commits of a rebase, made as the sequencer makes them: in the process as `try_to_commit()` makes them, and as
//! the `git commit` that `run_git_commit()` starts makes them where the sequencer leaves a commit to it, with the hooks
//! where these run them.

use std::path::Path;

use gix_hash::ObjectId;
use gix_ref::{
    Target,
    transaction::{PreviousValue, RefEdit},
};
use gix_sequencer::{
    rebase::{AuthorScript, Progress},
    refs,
};

use super::{Sequencer, message_from_subject, programs::Edited};
#[cfg(feature = "hooks")]
use crate::repository::commit::{CommitHooks, author_env, hook_path};
use crate::{
    Result,
    bstr::{BStr, BString, ByteSlice},
    commit::message::{self as msg, Cleanup, Mode},
    error::{ResultExt, bail, message},
    repository::commit::LockedIndex,
};

/// The file in which `git commit` prepares the message, which its hooks and the editor receive.
const COMMIT_EDITMSG: &str = "COMMIT_EDITMSG";
/// The file with the message of a merge or a pick, which a commit made from it removes.
pub(super) const MERGE_MSG: &str = "MERGE_MSG";

/// How a commit of the rebase is made, as the flags of `try_to_commit()` and `run_git_commit()` say.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Flags {
    /// `ALLOW_EMPTY`: the commit may record the tree of its first parent.
    pub allow_empty: bool,
    /// `EDIT_MSG`: the message is edited in the editor.
    pub edit: bool,
    /// `AMEND_MSG`: the commit replaces that of `HEAD`.
    pub amend: bool,
    /// `CLEANUP_MSG`: the message loses its comments without an editor.
    pub cleanup: bool,
    /// `VERIFY_MSG`: `pre-commit` and `commit-msg` run.
    pub verify: bool,
    /// `CREATE_ROOT_COMMIT`: the commit has no parent.
    pub create_root: bool,
}

/// What came of a commit of the rebase.
pub(super) enum Committed {
    /// The commit was made, and `HEAD` points to it.
    Made,
    /// No commit was made, for the reason that Git gives, which stops the rebase.
    Refused(String),
}

impl Sequencer<'_> {
    /// Commit the index as `do_commit()` does: in the process as `try_to_commit()` does unless the message is edited or
    /// verified, and otherwise, or if that commit would be empty, as `git commit` does once `REBASE_HEAD` points to
    /// `commit`. The message is that of `message_file`, or that of `HEAD` for an amended commit without it, and
    /// `author` is the author of a commit that doesn't amend `HEAD`.
    pub(super) fn do_commit(
        &self,
        message_file: Option<&Path>,
        author: Option<&gix_actor::Signature>,
        sub_action: &str,
        flags: Flags,
        commit: Option<ObjectId>,
    ) -> Result<Committed> {
        if !flags.edit && !flags.verify {
            let message = match message_file {
                Some(path) => {
                    Some(BString::from(std::fs::read(path).or_raise(|| {
                        message!("unable to read commit message from '{}'", path.display())
                    })?))
                }
                None => None,
            };
            if let Some(committed) = self.try_to_commit(message, author, sub_action, flags)? {
                if matches!(committed, Committed::Made) {
                    self.apply_edits([refs::delete(refs::CHERRY_PICK_HEAD)?])?;
                    remove_file(&self.git_dir().join(MERGE_MSG))?;
                }
                return Ok(committed);
            }
        }
        if let Some(commit) = commit {
            self.apply_edits([refs::rebase_head(commit)])?;
        }
        self.git_commit(message_file, sub_action, flags)
    }

    /// Commit the index with `message` by `author` as `try_to_commit()` does, or return `None` if the commit would
    /// record the tree of its first parent without [`Flags::allow_empty`], which `git commit` then reports.
    ///
    /// An amended commit takes the author, the parents and the extra headers but the signature of `HEAD`, and its
    /// message if there is none. `prepare-commit-msg` runs if it exists, the message is cleaned as `commit.cleanup`
    /// says if it is set, and `HEAD` gets the commit with the reflog message `<action> (<sub_action>): <subject>`, after
    /// which `post-commit` runs, and `post-rewrite` for an amended commit.
    fn try_to_commit(
        &self,
        message: Option<BString>,
        author: Option<&gix_actor::Signature>,
        sub_action: &str,
        flags: Flags,
    ) -> Result<Option<Committed>> {
        let current = self.head_commit()?;
        let mut message_from_head = false;
        let (mut message, author, parents, extra_headers) = match current.filter(|_| flags.amend) {
            Some(head) => {
                let commit = self.repo.find_commit(head)?;
                let decoded = commit.decode()?;
                let message = match message {
                    Some(message) => message,
                    None => {
                        message_from_head = true;
                        message_from_subject(decoded.message).to_owned()
                    }
                };
                (
                    message,
                    decoded.author()?.to_owned()?,
                    commit.parent_ids().map(crate::Id::detach).collect::<Vec<_>>(),
                    extra_headers_without_signature(&decoded),
                )
            }
            None if flags.amend => bail!(crate::error::message("could not parse HEAD").validation_error()),
            None => (
                message.unwrap_or_default(),
                author
                    .cloned()
                    .ok_or_else(|| crate::error::message("unable to parse commit author").validation_error())?,
                current.filter(|_| !flags.create_root).into_iter().collect(),
                Vec::new(),
            ),
        };
        let tree = self.write_index_tree()?;
        if !flags.allow_empty {
            let first_parent = if flags.amend { parents.first().copied() } else { current };
            let parent_tree = match first_parent {
                Some(parent) => self.tree_of(parent)?,
                None => ObjectId::empty_tree(self.repo.object_hash()),
            };
            if parent_tree == tree {
                return Ok(None);
            }
        }

        #[cfg(feature = "hooks")]
        let hooks = CommitHooks::new(self.repo, gix_hook::invocation::Editor::NotUsed, Vec::new())?;
        #[cfg(feature = "hooks")]
        if hooks.exists("prepare-commit-msg") {
            let path = self.git_dir().join(COMMIT_EDITMSG);
            write_file(&path, &message)?;
            let source = if message_from_head {
                gix_hook::invocation::MessageSource::Commit(std::ffi::OsStr::new("HEAD"))
            } else {
                gix_hook::invocation::MessageSource::Message
            };
            let message_file = hook_path(self.repo, &path)?;
            let outcome = hooks.run(gix_hook::Invocation::prepare_commit_msg(
                hooks.commit(),
                &message_file,
                Some(source),
            ))?;
            if !outcome.success() {
                return Ok(Some(Committed::Refused("'prepare-commit-msg' hook failed".into())));
            }
            message = read_file(&path)?;
        }
        #[cfg(not(feature = "hooks"))]
        let _ = message_from_head;

        let mode = if flags.cleanup {
            Mode::All
        } else {
            match self.cleanup {
                None | Some(Cleanup::Default | Cleanup::Verbatim) => Mode::None,
                Some(Cleanup::Whitespace | Cleanup::Scissors) => Mode::Space,
                Some(Cleanup::Strip) => Mode::All,
            }
        };
        if mode != Mode::None {
            message = msg::strip_space(message.as_bstr(), (mode == Mode::All).then_some(self.comment.as_bstr()));
        }
        let commit_id = self.write_commit(tree, parents, author, message.clone(), extra_headers)?;
        self.update_head(current, commit_id, sub_action, message.as_bstr())?;
        #[cfg(feature = "hooks")]
        {
            hooks.run(gix_hook::Invocation::post_commit(hooks.commit()))?;
            if let Some(previous) = current.filter(|_| flags.amend) {
                hooks.run(gix_hook::Invocation::post_rewrite(
                    gix_hook::invocation::Rewrite::Amend,
                    [(previous.as_ref(), commit_id.as_ref())],
                ))?;
            }
        }
        Ok(Some(Committed::Made))
    }

    /// Commit the index as the `git commit` that `run_git_commit()` starts commits it with `flags`, with the message of
    /// `message_file`, or with that of `HEAD` without it, and the reflog message `<action> (<sub_action>): <subject>`.
    ///
    /// The author is that of the commit that `CHERRY_PICK_HEAD` names if it is the commit of `REBASE_HEAD` or another
    /// commit that is picked, else that of `HEAD` if the message is that of `HEAD` or `HEAD` is amended, else that of
    /// `author-script`, which `git commit` gets from the sequencer unless `HEAD` is amended without a message file.
    /// `pre-commit` and `commit-msg` run with [`Flags::verify`], `prepare-commit-msg` always, and the editor edits the
    /// message with [`Flags::edit`]. Where `git commit` refuses, the reason is returned.
    pub(super) fn git_commit(&self, message_file: Option<&Path>, sub_action: &str, flags: Flags) -> Result<Committed> {
        let script = if message_file.is_some() || !flags.amend {
            match self.dir.author_script()? {
                Some(script) => Some(script),
                None => return Ok(Committed::Refused(self.staged_changes_advice())),
            }
        } else {
            None
        };
        let git_dir = self.git_dir();
        let head = self.head_commit()?;
        if flags.amend && head.is_none() {
            return Ok(Committed::Refused("You have nothing to amend.".into()));
        }
        let in_merge = git_dir.join("MERGE_HEAD").is_file();
        let cherry_pick_head = self.pseudo_reference(refs::CHERRY_PICK_HEAD)?.filter(|_| !in_merge);
        let from_rebase_pick =
            cherry_pick_head.is_some() && cherry_pick_head == self.pseudo_reference(refs::REBASE_HEAD)?;
        if flags.amend {
            if in_merge {
                return Ok(Committed::Refused(
                    "You are in the middle of a merge -- cannot amend.".into(),
                ));
            } else if from_rebase_pick {
                return Ok(Committed::Refused(
                    "You are in the middle of a rebase -- cannot amend.".into(),
                ));
            } else if cherry_pick_head.is_some() {
                return Ok(Committed::Refused(
                    "You are in the middle of a cherry-pick -- cannot amend.".into(),
                ));
            }
        }
        let message_from_head = message_file.is_none();
        let author = match (cherry_pick_head, head) {
            (Some(picked), _) => self.author_of(picked)?,
            (None, Some(head)) if message_from_head || flags.amend => self.author_of(head)?,
            _ => match &script {
                Some(script) => author_from_script(script)?,
                None => self
                    .repo
                    .author()
                    .ok_or_else(|| message("Author identity is not configured").validation_error())??
                    .to_owned()?,
            },
        };

        #[cfg(feature = "hooks")]
        let hooks = {
            let mut env = author_env(&author)?;
            env.push((
                "GIT_REFLOG_ACTION".into(),
                Some(
                    gix_path::from_bstr(self.reflog_message(sub_action, None).as_bstr())?
                        .into_owned()
                        .into(),
                ),
            ));
            let editor = if flags.edit {
                gix_hook::invocation::Editor::Used
            } else {
                gix_hook::invocation::Editor::NotUsed
            };
            CommitHooks::new(self.repo, editor, env)?
        };
        let tree = self.write_index_tree()?;
        // The tree is written again after `pre-commit`, which may have changed the index.
        #[cfg(feature = "hooks")]
        let tree = if flags.verify && hooks.exists("pre-commit") {
            if !hooks.run(gix_hook::Invocation::pre_commit(hooks.commit()))?.success() {
                return Ok(Committed::Refused("the pre-commit hook failed".into()));
            }
            self.write_index_tree()?
        } else {
            tree
        };

        let mode = if flags.cleanup {
            Cleanup::Strip
        } else if flags.edit {
            self.cleanup.unwrap_or_default()
        } else {
            self.cleanup.unwrap_or(Cleanup::Verbatim)
        }
        .mode(flags.edit);
        let prepared = match (message_file, head) {
            (Some(path), _) => read_file(path)?,
            (None, Some(head)) => {
                let commit = self.repo.find_commit(head)?;
                message_from_subject(commit.decode()?.message).to_owned()
            }
            (None, None) => BString::default(),
        };
        let prepared = if mode == Mode::None {
            prepared
        } else {
            msg::strip_space(prepared.as_bstr(), None)
        };
        let message_path = git_dir.join(COMMIT_EDITMSG);
        write_file(&message_path, &prepared)?;
        let comment_prefix = self.repo.comment_prefix()?.for_message(prepared.as_bstr())?;

        let parents = match head {
            Some(head) if flags.amend => self
                .repo
                .find_commit(head)?
                .parent_ids()
                .map(crate::Id::detach)
                .collect::<Vec<_>>(),
            Some(head) => {
                let mut parents = vec![head];
                if in_merge {
                    parents.extend(self.merge_heads(&git_dir)?);
                }
                parents
            }
            None => Vec::new(),
        };
        let parent_tree = match (head, parents.first()) {
            (Some(head), _) if !flags.amend => Some(self.tree_of(head)?),
            (_, Some(first)) if flags.amend => Some(self.tree_of(*first)?),
            _ => None,
        };
        let committable = match parent_tree {
            Some(parent_tree) => parent_tree != tree,
            None => tree != ObjectId::empty_tree(self.repo.object_hash()),
        };
        let amends_merge = flags.amend && parents.len() > 1;
        if !committable && !in_merge && !flags.allow_empty && !amends_merge {
            let reason = if flags.amend {
                "You asked to amend the most recent commit, but doing so would make\nit empty. You can repeat your command with --allow-empty, or you can\nremove the commit entirely with \"git reset HEAD^\"."
            } else if from_rebase_pick {
                "The previous cherry-pick is now empty, possibly due to conflict resolution.\nOtherwise, please use 'git rebase --skip'"
            } else if cherry_pick_head.is_some() {
                "The previous cherry-pick is now empty, possibly due to conflict resolution."
            } else {
                "nothing to commit"
            };
            return Ok(Committed::Refused(reason.into()));
        }

        #[cfg(feature = "hooks")]
        {
            let source = if message_from_head {
                gix_hook::invocation::MessageSource::Commit(std::ffi::OsStr::new("HEAD"))
            } else {
                gix_hook::invocation::MessageSource::Message
            };
            let message_file = hook_path(self.repo, &message_path)?;
            let outcome = hooks.run(gix_hook::Invocation::prepare_commit_msg(
                hooks.commit(),
                &message_file,
                Some(source),
            ))?;
            if !outcome.success() {
                return Ok(Committed::Refused("the prepare-commit-msg hook failed".into()));
            }
        }
        if flags.edit {
            let index_file = self.repo.index_path();
            #[cfg(feature = "hooks")]
            let index_file = hook_path(self.repo, &index_file)?;
            let env = [("GIT_INDEX_FILE".into(), index_file.into_os_string())];
            if let Edited::Failed(reason) = self.edit_message(&message_path, &env)? {
                return Ok(Committed::Refused(format!(
                    "{reason}\nPlease supply the message using either -m or -F option."
                )));
            }
        }
        #[cfg(feature = "hooks")]
        if flags.verify && hooks.exists("commit-msg") {
            let message_file = hook_path(self.repo, &message_path)?;
            if !hooks
                .run(gix_hook::Invocation::commit_msg(hooks.commit(), &message_file))?
                .success()
            {
                return Ok(Committed::Refused("the commit-msg hook failed".into()));
            }
        }

        let edited = read_file(&message_path)?;
        let cleaned = msg::clean(edited.as_bstr(), mode, comment_prefix.as_bstr());
        if flags.edit && msg::is_empty(cleaned.as_bstr(), mode) {
            return Ok(Committed::Refused(
                "Aborting commit due to empty commit message.".into(),
            ));
        }
        let extra_headers = match head.filter(|_| flags.amend) {
            Some(head) => extra_headers_without_signature(&self.repo.find_commit(head)?.decode()?),
            None => Vec::new(),
        };
        let commit_id = self.write_commit(tree, parents, author, cleaned.clone(), extra_headers)?;
        self.update_head(head, commit_id, sub_action, cleaned.as_bstr())?;

        // As `sequencer_post_commit_cleanup()` and `cmd_commit()` do, the state of the picked commit and of the merge goes.
        let mut edits = Vec::new();
        for name in [refs::CHERRY_PICK_HEAD, refs::REVERT_HEAD, refs::AUTO_MERGE] {
            edits.push(refs::delete(name)?);
        }
        self.apply_edits(edits)?;
        for name in ["MERGE_HEAD", MERGE_MSG, "MERGE_MODE", "SQUASH_MSG"] {
            remove_file(&git_dir.join(name))?;
        }
        #[cfg(feature = "hooks")]
        {
            hooks.run(gix_hook::Invocation::post_commit(hooks.commit()))?;
            if let Some(previous) = head.filter(|_| flags.amend) {
                hooks.run(gix_hook::Invocation::post_rewrite(
                    gix_hook::invocation::Rewrite::Amend,
                    [(previous.as_ref(), commit_id.as_ref())],
                ))?;
            }
        }
        Ok(Committed::Made)
    }

    /// Commit the staged changes before the rebase continues, as `commit_staged_changes()` does with the steps of
    /// [`Dir::prepare_continue()`](gix_sequencer::rebase::Dir::prepare_continue()) for the next command of `progress`.
    pub(super) fn commit_staged_changes(&self, progress: &Progress) -> Result<()> {
        let changes = self.worktree_changes()?;
        let head = self.head_commit()?;
        let head_message = match head {
            Some(head) => {
                let commit = self.repo.find_commit(head)?;
                Some(BString::from(message_from_subject(commit.decode()?.message)))
            }
            None => None,
        };
        let outcome = self.dir.prepare_continue(
            &self.state,
            progress,
            changes,
            head,
            head_message.as_deref().map(|m| m.as_bytes()),
        )?;
        self.apply_edits(outcome.edits)?;
        if outcome.remove_merge_message {
            remove_file(&self.git_dir().join(MERGE_MSG))?;
        }
        let Some(commit) = outcome.commit else {
            return Ok(());
        };
        let flags = Flags {
            allow_empty: true,
            edit: commit.edit_message,
            amend: commit.amend,
            cleanup: commit.cleanup_message,
            ..Default::default()
        };
        if let Committed::Refused(reason) = self.git_commit(commit.message_file.as_deref(), "continue", flags)? {
            bail!(message!("{reason}\ncould not commit staged changes.").validation());
        }
        let edits = self.dir.committed(&commit)?;
        self.apply_edits(edits)
    }

    /// Point `HEAD` from `previous`, or from nothing, to `new` with the reflog message
    /// `<action> (<sub_action>): <subject of message>`, as `update_head_with_reflog()` does.
    pub(super) fn update_head(
        &self,
        previous: Option<ObjectId>,
        new: ObjectId,
        sub_action: &str,
        message: &BStr,
    ) -> Result<()> {
        let expected = match previous {
            Some(previous) => PreviousValue::MustExistAndMatch(Target::Object(previous)),
            None => PreviousValue::MustNotExist,
        };
        let action = self.reflog_message(sub_action, None);
        self.repo.edit_reference(
            RefEdit::update(
                gix_ref::FullName::try_from("HEAD").or_error()?,
                new,
                expected,
                msg::reflog_message(action.as_bstr(), message),
            )
            .with_deref(true),
        )?;
        Ok(())
    }

    /// Write the trees of the index and give it their cache tree, writing it back if that changed it, as
    /// `write_index_as_tree()` does, and return the tree of the index.
    pub(super) fn write_index_tree(&self) -> Result<ObjectId> {
        let LockedIndex { lock, mut index } = self.repo.lock_index()?;
        if index.entries().iter().any(|entry| entry.stage_raw() != 0) {
            bail!(message("git write-tree failed to write a tree").validation_error());
        }
        let written = index.write_tree(index.tree(), self.repo, Default::default())?;
        if index.tree() == Some(&written.cache_tree) {
            drop(lock);
        } else {
            index.set_tree(written.cache_tree);
            self.repo.write_locked_index(lock, &index)?;
        }
        Ok(written.tree_id)
    }

    /// Write the commit of `tree` with `parents`, `author`, the committer of the configuration, `message` and
    /// `extra_headers`, signed if the rebase signs its commits, as `commit_tree_extended()` does.
    fn write_commit(
        &self,
        tree: ObjectId,
        parents: Vec<ObjectId>,
        author: gix_actor::Signature,
        message: BString,
        extra_headers: Vec<(BString, BString)>,
    ) -> Result<ObjectId> {
        let committer = self
            .repo
            .committer()
            .ok_or_else(|| crate::error::message("Committer identity is not configured").validation_error())??;
        let mut commit = gix_object::Commit {
            tree,
            parents: parents.into_iter().collect(),
            author,
            committer: committer.to_owned()?,
            encoding: None,
            message,
            extra_headers,
        };
        if self.state.options.gpg_sign.is_some() {
            let options = crate::commit::sign::signing_options(self.repo)?;
            commit = commit
                .sign(options)
                .or_raise(|| crate::error::message("Could not sign the commit"))?;
        }
        Ok(self.repo.write_object(&commit)?.detach())
    }

    /// Return the author of the commit `id`.
    pub(super) fn author_of(&self, id: ObjectId) -> Result<gix_actor::Signature> {
        let commit = self.repo.find_commit(id)?;
        let decoded = commit.decode()?;
        decoded.author()?.to_owned()
    }

    /// Return the commit to which the pseudo-reference `name` points, or `None` if it doesn't exist.
    pub(super) fn pseudo_reference(&self, name: &str) -> Result<Option<ObjectId>> {
        Ok(match self.repo.try_find_reference(name)? {
            Some(reference) if reference.name().as_bstr() == name => Some(reference.into_fully_peeled_id()?.detach()),
            _ => None,
        })
    }

    /// Return the commits of `MERGE_HEAD` in `git_dir`, which a commit concludes as `git commit` does during a merge.
    fn merge_heads(&self, git_dir: &Path) -> Result<Vec<ObjectId>> {
        let content = read_file(&git_dir.join("MERGE_HEAD"))?;
        content
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| ObjectId::from_hex(line).or_raise(|| message!("Corrupt MERGE_HEAD file ({})", line.as_bstr())))
            .collect()
    }

    /// Return the advice of the sequencer for staged changes that it can't commit, `staged_changes_advice`.
    pub(super) fn staged_changes_advice(&self) -> String {
        let gpg_sign = if self.state.options.gpg_sign.is_some() {
            "'-S'"
        } else {
            ""
        };
        format!(
            "you have staged changes in your working tree\n\
If these changes are meant to be squashed into the previous commit, run:\n\n  git commit --amend {gpg_sign}\n\n\
If they are meant to go into a new commit, run:\n\n  git commit {gpg_sign}\n\n\
In both cases, once you're done, continue with:\n\n  git rebase --continue\n"
        )
    }
}

/// Return the extra headers of `commit` without its signatures, which an amended commit keeps, as
/// `read_commit_extra_headers()` returns them with `gpgsig` and `gpgsig-sha256` excluded.
fn extra_headers_without_signature(commit: &gix_object::CommitRef<'_>) -> Vec<(BString, BString)> {
    commit
        .extra_headers
        .iter()
        .filter(|(name, _)| *name != "gpgsig" && *name != "gpgsig-sha256")
        .map(|(name, value)| ((*name).to_owned(), value.clone().into_owned()))
        .collect()
}

/// Return the author of `script`, as `git commit` takes it from `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL` and
/// `GIT_AUTHOR_DATE`.
fn author_from_script(script: &AuthorScript) -> Result<gix_actor::Signature> {
    let date = script.date.strip_prefix(b"@").unwrap_or(&script.date);
    let time = date
        .to_str()
        .ok()
        .and_then(|date| date.parse::<gix_date::Time>().ok())
        .ok_or_else(|| message!("invalid date format: {}", script.date).validation_error())?;
    Ok(gix_actor::Signature {
        name: script.name.clone(),
        email: script.email.clone(),
        time,
    })
}

/// Return the content of the file at `path`.
pub(super) fn read_file(path: &Path) -> Result<BString> {
    Ok(std::fs::read(path)
        .or_raise(|| message!("could not read '{}'", path.display()))?
        .into())
}

/// Write `content` to the file at `path`.
pub(super) fn write_file(path: &Path, content: &[u8]) -> Result<()> {
    std::fs::write(path, content).or_raise(|| message!("could not write '{}'", path.display()))
}

/// Remove the file at `path` if it exists.
pub(super) fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).or_raise(|| message!("could not remove '{}'", path.display())),
    }
}
