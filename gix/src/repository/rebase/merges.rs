//! The commands of the todo lists of `--rebase-merges` and `--update-refs`: `label`, `reset`, `merge` and `update-ref`,
//! performed as `do_label()`, `do_reset()`, `do_merge()` and `do_update_ref()` perform them.

use gix_hash::ObjectId;
use gix_ref::transaction::{PreviousValue, RefEdit};
use gix_sequencer::{
    rebase::{AuthorScript, Progress},
    refs,
    todo::Instruction,
};

use super::{
    Sequencer,
    commit::{Committed, Flags, MERGE_MSG, remove_file, write_file},
    message_from_subject,
    pick::{Merged, Step},
};
use crate::{
    Result,
    bstr::{BStr, BString, ByteSlice},
    error::ResultExt,
    rebase::StopReason,
};

impl Sequencer<'_> {
    /// Point `refs/rewritten/<label>` to `HEAD` and record it for removal with the state, as `do_label()` does.
    pub(super) fn do_label(&self, label: &BStr) -> Result<Step> {
        let Some(head) = self.head_commit()? else {
            return Ok(Step::Reschedule("could not read HEAD".into()));
        };
        let edit = match refs::label(label, head) {
            Ok(edit) => edit,
            Err(err) => return Ok(Step::Reschedule(err.to_string())),
        };
        if let Err(err) = self.repo.edit_reference(edit) {
            return Ok(Step::Reschedule(err.to_string()));
        }
        self.dir.add_label(label)?;
        Ok(Step::Next { check_todo: false })
    }

    /// Reset `HEAD`, the index and the worktree to the commit of the label that starts `arg`, or to the empty commit of
    /// `[new root]`, which is made if there is none yet, as `do_reset()` does with the reflog message
    /// `rebase (reset): '<label>'`.
    pub(super) fn do_reset(&mut self, arg: &BStr) -> Result<Step> {
        let (target, name) = if arg == "[new root]" {
            (self.squash_onto()?, arg)
        } else {
            let label = arg.find_byteset(b" \t\n\r").map_or(arg, |end| arg[..end].as_bstr());
            match self.lookup_label(label)? {
                Some(commit) => (commit, label),
                None => return Ok(Step::Reschedule(format!("could not resolve '{label}'"))),
            }
        };
        if self
            .repo
            .read_index_file()?
            .entries()
            .iter()
            .any(|entry| entry.stage_raw() != 0)
        {
            return Ok(Step::Reschedule(
                "Rebasing is not possible because you have unmerged files.".into(),
            ));
        }
        if let Some(refused) = self.reset_worktree(target, gix_worktree_state::reset::Mode::Merge)? {
            return Ok(Step::Reschedule(refused));
        }
        let mut text = BString::from("'");
        text.extend_from_slice(name);
        text.push(b'\'');
        self.repo.edit_reference(
            RefEdit::update(
                gix_ref::FullName::try_from("HEAD").or_error()?,
                target,
                PreviousValue::Any,
                self.reflog_message("reset", Some(&text)),
            )
            .with_deref(true),
        )?;
        Ok(Step::Next { check_todo: false })
    }

    /// Merge the commit of the label of `instruction` into `HEAD` as `do_merge()` does, with the message and author of
    /// the original merge commit of `merge -C` or `merge -c`, or with the message after `#` or `Merge branch '<label>'`
    /// and the configured author otherwise. The merge is taken as it is if `HEAD` and the label are the parents of the
    /// original merge already, and skipped if the label is an ancestor of `HEAD`. Merges of several labels and merge
    /// strategies other than `ort` aren't supported and are rescheduled.
    pub(super) fn do_merge(&mut self, progress: &Progress, instruction: &Instruction) -> Result<Step> {
        let Some(head) = self.head_commit()? else {
            return Ok(Step::Reschedule("cannot merge without a current revision".into()));
        };
        let arg = instruction.arg.as_bstr();
        let (labels, oneline) = split_merge_arg(arg);
        let mut to_merge = Vec::new();
        for label in &labels {
            match self.lookup_label(label)? {
                Some(commit) => to_merge.push((commit, self.label_ref_name(label)?)),
                None => return Ok(Step::Reschedule(format!("unable to parse '{label}'"))),
            }
        }
        let Some((merge_commit, merge_name)) = to_merge.first().cloned() else {
            return Ok(Step::Reschedule(format!("nothing to merge: '{arg}'")));
        };
        let original = instruction.commit;
        let step = if self.state.squash_onto == Some(head) {
            if to_merge.len() > 1 {
                return Ok(Step::Reschedule(
                    "octopus merge cannot be executed on top of a [new root]".into(),
                ));
            }
            match self.fast_forward_to(merge_commit, Some(head))? {
                Some(refused) => return Ok(Step::Reschedule(refused)),
                None => Step::Next { check_todo: false },
            }
        } else if self.allow_ff && original.is_some_and(|commit| self.merges_into(commit, head, &to_merge)) {
            let commit = original.unwrap_or(merge_commit);
            if let Some(refused) = self.fast_forward_to(commit, Some(head))? {
                return Ok(Step::Reschedule(refused));
            }
            if instruction.flags.edit_merge_message {
                self.edit_merge_message(original, arg)?
            } else {
                Step::Next { check_todo: false }
            }
        } else {
            let author = match original {
                Some(commit) => {
                    let commit = self.repo.find_commit(commit)?;
                    let decoded = commit.decode()?;
                    self.message = message_from_subject(decoded.message).to_owned();
                    AuthorScript::from_author_header(decoded.author)
                }
                None => {
                    let author = self.repo.author().ok_or_else(|| {
                        crate::error::message("Author identity is not configured").validation_error()
                    })??;
                    let mut header = BString::default();
                    author.trim().write_to(&mut *header).or_error()?;
                    self.message = match oneline {
                        Some(oneline) => oneline.to_owned(),
                        None => format!(
                            "Merge {} '{}'",
                            if to_merge.len() > 1 { "branches" } else { "branch" },
                            labels_text(&labels)
                        )
                        .into(),
                    };
                    AuthorScript::from_author_header(header.as_bstr())
                }
            };
            self.dir.write_author_script(&author)?;
            self.have_message = true;
            let git_dir = self.git_dir();
            write_file(&git_dir.join(MERGE_MSG), &self.message)?;
            let strategy = self
                .state
                .options
                .strategy
                .as_ref()
                .filter(|strategy| *strategy != "ort" && *strategy != "recursive");
            if to_merge.len() > 1 || strategy.is_some() || !self.state.options.strategy_options.is_empty() {
                return Ok(Step::Reschedule(
                    "merges of several labels and merge strategies aren't supported".into(),
                ));
            }
            let mut commits = gix_rebase::Commits::new(&self.repo.objects, self.repo.object_hash());
            let bases = gix_rebase::merge_bases(&mut commits, head, &[merge_commit])?;
            if bases.first() == Some(&merge_commit) {
                Step::Next { check_todo: false }
            } else {
                self.merge_label(head, merge_commit, merge_name.as_bstr(), original, arg, instruction)?
            }
        };
        if let Some(commit) = original.filter(|_| !matches!(step, Step::Reschedule(_))) {
            let head = self.head_commit()?;
            self.dir.record_rewritten(commit, progress.peek_command(1), head)?;
        }
        Ok(step)
    }

    /// Merge `merge_commit`, which `merge_name` names, into `head` and commit the merge as `do_merge()` does with
    /// `merge_ort_recursive()` and `run_git_commit()`, or stop at its conflicts.
    fn merge_label(
        &mut self,
        head: ObjectId,
        merge_commit: ObjectId,
        merge_name: &BStr,
        original: Option<ObjectId>,
        arg: &BStr,
        instruction: &Instruction,
    ) -> Result<Step> {
        let git_dir = self.git_dir();
        write_file(
            &git_dir.join("MERGE_HEAD"),
            merge_commit.to_hex().to_string().as_bytes(),
        )?;
        write_file(&git_dir.join("MERGE_MODE"), b"no-ff")?;
        let labels = gix_merge::blob::builtin_driver::text::Labels {
            ancestor: None,
            current: Some("HEAD".into()),
            other: Some(merge_name),
        };
        let options =
            crate::merge::commit::Options::from(self.repo.tree_merge_options()?).with_allow_missing_merge_base(true);
        let mut outcome = self.repo.merge_commits(head, merge_commit, labels, options)?;
        let head_tree = self.tree_of(head)?;
        match self.apply_merge(head_tree, &mut outcome.tree_merge)? {
            Merged::Refused(reason) => {
                remove_file(&git_dir.join(MERGE_MSG))?;
                Ok(Step::Reschedule(format!(
                    "{reason}\ncould not even attempt to merge '{arg}'"
                )))
            }
            Merged::Conflicts => Ok(Step::Stop(self.error_with_patch(
                original,
                arg,
                true,
                false,
                StopReason::Conflicts { commit: original },
                String::new(),
            )?)),
            Merged::Clean => {
                let message_file = git_dir.join(MERGE_MSG);
                match self.git_commit(Some(&message_file), "merge", Flags::default())? {
                    Committed::Made if instruction.flags.edit_merge_message => self.edit_merge_message(original, arg),
                    Committed::Made => Ok(Step::Next { check_todo: false }),
                    Committed::Refused(reason) => Ok(Step::Stop(self.error_with_patch(
                        original,
                        arg,
                        true,
                        false,
                        stop_reason(original, reason),
                        String::new(),
                    )?)),
                }
            }
        }
    }

    /// Let the editor edit the message of the merge of `HEAD` for `merge -c` and amend it, as `do_merge()` does after
    /// `fast_forward_edit`.
    fn edit_merge_message(&self, original: Option<ObjectId>, arg: &BStr) -> Result<Step> {
        let flags = Flags {
            amend: true,
            edit: true,
            verify: true,
            ..Default::default()
        };
        Ok(match self.git_commit(None, "merge", flags)? {
            Committed::Made => Step::Next { check_todo: true },
            Committed::Refused(reason) => Step::Stop(self.error_with_patch(
                original,
                arg,
                true,
                false,
                stop_reason(original, reason),
                String::new(),
            )?),
        })
    }

    /// Point `HEAD` to `head` in the `update-refs` of the state for the branch `name`, as `do_update_ref()` records it.
    pub(super) fn do_update_ref(&self, name: &BStr) -> Result<Step> {
        let Some(head) = self.head_commit()? else {
            return Ok(Step::Reschedule("could not read HEAD".into()));
        };
        self.dir.record_update_ref(name, head)?;
        Ok(Step::Next { check_todo: false })
    }

    /// Return the empty commit that `reset [new root]` resets to, making it and recording it in `squash-onto` if the
    /// rebase has none yet, as `do_reset()` does with `commit_tree()`.
    fn squash_onto(&mut self) -> Result<ObjectId> {
        if let Some(squash_onto) = self.state.squash_onto {
            return Ok(squash_onto);
        }
        let squash_onto = super::start::empty_root_commit(self.repo)?;
        self.dir.write_squash_onto(squash_onto)?;
        self.state.squash_onto = Some(squash_onto);
        Ok(squash_onto)
    }

    /// Return the commit of `label`: that of `refs/rewritten/<label>`, or else the commit that `label` names as a
    /// revision, as `lookup_label()` finds it.
    fn lookup_label(&self, label: &BStr) -> Result<Option<ObjectId>> {
        if let Ok(name) = refs::label_name(label)
            && let Some(mut reference) = self.repo.try_find_reference(name.as_ref())?
        {
            return Ok(Some(reference.peel_to_id()?.detach()));
        }
        Ok(super::todo::resolve_commit(self.repo, label))
    }

    /// Return the name of the label as `lookup_label()` leaves it for the conflict markers of a merge:
    /// `refs/rewritten/<label>` if that reference exists, and the label otherwise.
    fn label_ref_name(&self, label: &BStr) -> Result<BString> {
        if let Ok(name) = refs::label_name(label)
            && self.repo.try_find_reference(name.as_ref())?.is_some()
        {
            return Ok(name.as_bstr().to_owned());
        }
        Ok(label.to_owned())
    }

    /// Return `true` if `commit`, the original merge, has `head` as first parent and the commits of `to_merge` as its
    /// other parents, so that the merge can be taken as it is.
    fn merges_into(&self, commit: ObjectId, head: ObjectId, to_merge: &[(ObjectId, BString)]) -> bool {
        let Ok(commit) = self.repo.find_commit(commit) else {
            return false;
        };
        let parents: Vec<ObjectId> = commit.parent_ids().map(crate::Id::detach).collect();
        parents.first() == Some(&head)
            && parents.len() == to_merge.len() + 1
            && parents[1..]
                .iter()
                .zip(to_merge)
                .all(|(parent, (merged, _))| parent == merged)
    }
}

/// Split the argument of a `merge` command into its labels and the oneline after `#`, as `do_merge()` reads it.
fn split_merge_arg(arg: &BStr) -> (Vec<&BStr>, Option<&BStr>) {
    let mut labels = Vec::new();
    let mut rest = arg.trim_start_with(|c| matches!(c, ' ' | '\t' | '\n'));
    while !rest.is_empty() {
        if rest[0] == b'#' && rest.get(1).is_none_or(u8::is_ascii_whitespace) {
            let oneline = rest[1..].trim_start_with(|c| matches!(c, ' ' | '\t' | '\n'));
            return (labels, Some(oneline.as_bstr()));
        }
        let end = rest.find_byteset(b" \t\n").unwrap_or(rest.len());
        labels.push(rest[..end].as_bstr());
        rest = rest[end..].trim_start_with(|c| matches!(c, ' ' | '\t' | '\n'));
    }
    (labels, None)
}

/// Return the labels of a `merge` command as Git names them in its default message: the argument up to the last label.
fn labels_text(labels: &[&BStr]) -> BString {
    let mut out = BString::default();
    for (index, label) in labels.iter().enumerate() {
        if index > 0 {
            out.push(b' ');
        }
        out.extend_from_slice(label);
    }
    out
}

/// Return the reason of a stop at a merge whose commit was refused for `reason`.
fn stop_reason(original: Option<ObjectId>, reason: String) -> StopReason {
    match original {
        Some(commit) => StopReason::Uncommitted { commit, reason },
        None => StopReason::Conflicts { commit: None },
    }
}
