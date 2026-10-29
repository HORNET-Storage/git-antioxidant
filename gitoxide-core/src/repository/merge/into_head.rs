//! `gix merge [<commit>]`, which merges a commit into `HEAD` as `git merge` does, and concludes, aborts or forgets the
//! merge in progress with `--continue`, `--abort` and `--quit`.

use std::path::PathBuf;

use gix::bstr::BString;

/// A part of the message of a merge, as `-m` and `-F` give them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessagePart {
    /// A paragraph, as `-m` gives it.
    Paragraph(BString),
    /// The file to read a part of the message from, as `-F` names it.
    File(PathBuf),
}

/// The options of `gix merge`, named after those of `git merge`.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The commit to merge, or `None` for the upstream of the current branch, which `git merge` merges without commit.
    pub commit: Option<BString>,
    /// How the merge may fast-forward, as `--ff`, `--no-ff` and `--ff-only` say, or `None` for `merge.ff`.
    pub fast_forward: Option<gix::merge::into_head::FastForward>,
    /// If `true`, change the index and the worktree as the merge does, but neither commit nor move `HEAD`, as `--squash`
    /// does.
    pub squash: bool,
    /// If `true`, stop before the commit of the merge, as `--no-commit` does.
    pub no_commit: bool,
    /// The parts of the message in the order of the command line, which are joined as `git merge` joins them.
    pub message: Vec<MessagePart>,
    /// How to clean the message, as `--cleanup` names it, or `None` for `commit.cleanup`.
    pub cleanup: Option<String>,
    /// If `true`, skip the `pre-merge-commit` and `commit-msg` hooks, as `--no-verify` does.
    pub no_verify: bool,
    /// If `true`, merge a commit that shares no history with `HEAD`, as `--allow-unrelated-histories` does.
    pub allow_unrelated_histories: bool,
    /// If `true`, conclude the merge in progress with its commit, as `--continue` does.
    pub continue_merge: bool,
    /// If `true`, abort the merge in progress, as `--abort` does.
    pub abort: bool,
    /// If `true`, forget the merge in progress and keep the index and the worktree as they are, as `--quit` does.
    pub quit: bool,
}

pub(crate) mod function {
    use std::io::Write;

    use gix::{
        Result,
        bstr::{BStr, BString, ByteSlice},
        error::{ResultExt, bail, message},
        merge::into_head::{Changes, Error, Outcome},
        prelude::ObjectIdExt,
    };

    use super::{MessagePart, Options};
    use crate::repository::{advice_enabled, advise, advise_if_enabled, commit, diffstat};

    /// The advice `diverging` that `die_ff_impossible()` in Git's `advice.c` gives before it refuses a merge that can't
    /// fast-forward.
    const DIVERGING: &str = "Diverging branches can't be fast-forwarded, you need to either:\n\n\tgit merge --no-ff\n\nor:\n\n\tgit rebase\n";

    /// The advice `resolveConflict` that `error_resolve_conflict()` in Git's `advice.c` gives while the index has
    /// unmerged entries.
    const RESOLVE_CONFLICT: &str = "Fix them up in the work tree, and then use 'git add/rm <file>'\nas appropriate to mark resolution and make a commit.";

    /// Merge into `HEAD` as `git merge` does with the arguments of `options`, as `cmd_merge()` in Git's `builtin/merge.c`
    /// does, and print what it prints.
    ///
    /// * Without commit, the upstream of the current branch is merged, as `setup_with_upstream()` finds it.
    /// * `Already up to date.` if `HEAD` contains the commit.
    /// * `Updating <old>..<new>` and `Fast-forward` for a fast-forward, which `Squash commit -- not updating HEAD`
    ///   follows with `--squash`, and `Merge made by the 'ort' strategy.` for a merge commit, both followed by the
    ///   statistics of the changes as `merge.stat` asks for them, as `finish()` prints them. A fast-forward of an unborn
    ///   branch prints nothing.
    /// * For a merge that stops before its commit, `Automatic merge went well; stopped before committing as requested`
    ///   on `err`, or the failure `Automatic merge failed; fix conflicts and then commit the result.` if it has
    ///   conflicts.
    /// * Git's advice before a refusal for which Git gives one, on `err`.
    /// * After `--continue`, the summary of the merge commit, as `git commit` prints it.
    ///
    /// `GIT_REFLOG_ACTION` replaces the action of the reflog messages, as in Git.
    pub fn into_head(
        repo: gix::Repository,
        mut out: impl std::io::Write,
        mut err: impl std::io::Write,
        Options {
            commit,
            fast_forward,
            squash,
            no_commit,
            message,
            cleanup,
            no_verify,
            allow_unrelated_histories,
            continue_merge,
            abort,
            quit,
        }: Options,
    ) -> Result<()> {
        let other_arguments = commit.is_some()
            || fast_forward.is_some()
            || squash
            || no_commit
            || !message.is_empty()
            || cleanup.is_some()
            || no_verify
            || allow_unrelated_histories;
        let operations = [(abort, "--abort"), (quit, "--quit"), (continue_merge, "--continue")];
        let given = operations.iter().filter(|(given, _)| *given).count();
        if let Some((_, operation)) = operations.iter().find(|(given, _)| *given)
            && (other_arguments || given > 1)
        {
            bail!(message!("{operation} expects no arguments").validation());
        }
        if abort {
            let outcome = repo.merge_abort(&gix::interrupt::IS_INTERRUPTED)?;
            return report_changes(outcome.changes, "HEAD".into(), &mut err);
        }
        if quit {
            return repo.merge_quit();
        }
        if continue_merge {
            let outcome = match repo.merge_continue() {
                Ok(outcome) => outcome,
                Err(error) => return Err(refused(&repo, error, &mut err)?),
            };
            return commit::print_summary(&repo, &outcome, false, &mut out);
        }

        let cleanup = cleanup.as_deref().map(commit::cleanup_mode).transpose()?;
        let message = join_message(&message)?;
        let fast_forward_line = if message.is_some() {
            "Fast-forward (no commit created; -m option ignored)"
        } else {
            "Fast-forward"
        };
        let name = match commit {
            Some(name) => name,
            None => upstream(&repo)?,
        };
        let outcome = repo.merge_into_head(
            name.as_bstr(),
            gix::merge::into_head::Options {
                fast_forward,
                no_commit,
                squash,
                message,
                cleanup,
                allow_unrelated_histories,
                no_verify,
                reflog_action: crate::repository::reflog_action(),
                ..Default::default()
            },
            &gix::interrupt::IS_INTERRUPTED,
        );
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => return Err(refused(&repo, error, &mut err)?),
        };

        let stat = diffstat::Format::configured(&repo);
        match outcome {
            Outcome::UpToDate => {
                let line = if squash {
                    "Already up to date. (nothing to squash)"
                } else {
                    "Already up to date."
                };
                writeln!(out, "{line}").or_error()
            }
            Outcome::FastForward {
                previous: None,
                changes,
                ..
            } => report_changes(changes, name.as_bstr(), &mut err),
            Outcome::FastForward {
                previous: Some(previous),
                commit_id,
                changes,
            } => {
                writeln!(
                    out,
                    "Updating {}..{}",
                    previous.attach(&repo).shorten_or_id(),
                    commit_id.attach(&repo).shorten_or_id()
                )
                .or_error()?;
                report_changes(changes, name.as_bstr(), &mut err)?;
                writeln!(out, "{fast_forward_line}").or_error()?;
                if squash {
                    writeln!(out, "Squash commit -- not updating HEAD").or_error()?;
                }
                match stat {
                    Some(format) => diffstat::write(&repo, previous, commit_id, format, &mut out),
                    None => Ok(()),
                }
            }
            Outcome::Merged {
                applied,
                commit: Some(commit),
            } => {
                report_changes(applied.changes, name.as_bstr(), &mut err)?;
                writeln!(out, "Merge made by the 'ort' strategy.").or_error()?;
                match (stat, commit.previous) {
                    (Some(format), Some(previous)) => {
                        diffstat::write(&repo, previous, commit.commit_id, format, &mut out)
                    }
                    _ => Ok(()),
                }
            }
            Outcome::Merged { applied, commit: None } => {
                let conflicts = applied.conflicts;
                report_changes(applied.changes, name.as_bstr(), &mut err)?;
                if squash {
                    writeln!(out, "Squash commit -- not updating HEAD").or_error()?;
                }
                if conflicts {
                    bail!(gix::error::conflict(
                        "Automatic merge failed; fix conflicts and then commit the result."
                    ));
                }
                writeln!(err, "Automatic merge went well; stopped before committing as requested").or_error()
            }
        }
    }

    /// Write the advice that Git gives before it refuses as `error` says to `err`, and return the error to fail with,
    /// which is `error` unless Git words the refusal differently: only the first line of the refusal of a merge while a
    /// merge or a cherry-pick is in progress if `advice.resolveConflict` is disabled, as `cmd_merge()` words it.
    fn refused(repo: &gix::Repository, error: gix::Error, err: &mut dyn Write) -> Result<gix::Error> {
        if let Some(refusal) = error.downcast_any_ref::<Error>() {
            match refusal {
                Error::NotPossibleToFastForward => advise_if_enabled(repo, "diverging", DIVERGING, err)?,
                Error::UnmergedEntries if advice_enabled(repo, "resolveConflict") => advise(RESOLVE_CONFLICT, err)?,
                Error::MergeInProgress if !advice_enabled(repo, "resolveConflict") => {
                    return Ok(
                        gix::error::conflict("You have not concluded your merge (MERGE_HEAD exists).").conflict_error(),
                    );
                }
                Error::CherryPickInProgress if !advice_enabled(repo, "resolveConflict") => {
                    return Ok(gix::error::conflict(
                        "You have not concluded your cherry-pick (CHERRY_PICK_HEAD exists).",
                    )
                    .conflict_error());
                }
                _ => {}
            }
        } else if matches!(
            error.downcast_any_ref::<gix::commit::index::Error>(),
            Some(gix::commit::index::Error::UnmergedEntries)
        ) && advice_enabled(repo, "resolveConflict")
        {
            advise(RESOLVE_CONFLICT, err)?;
        }
        Ok(error)
    }

    /// Return the name of the upstream of the current branch, which `git merge` merges without commit unless
    /// `merge.defaultToUpstream` is `false`, as `setup_with_upstream()` in Git's `builtin/merge.c` finds it: the
    /// remote-tracking branch of `branch.<name>.merge`, or that branch itself for the remote `.`.
    fn upstream(repo: &gix::Repository) -> Result<BString> {
        if repo.config_snapshot().boolean("merge.defaultToUpstream") == Some(false) {
            bail!(gix::error::validation(
                "No commit specified and merge.defaultToUpstream not set."
            ));
        }
        let Some(branch) = repo.head_name()? else {
            bail!(gix::error::validation("No current branch."));
        };
        let Some(remote) = repo.branch_remote_name(branch.shorten(), gix::remote::Direction::Fetch) else {
            bail!(gix::error::validation("No remote for the current branch."));
        };
        let Some(merge) = repo
            .branch_remote_ref_name(branch.as_ref(), gix::remote::Direction::Fetch)
            .transpose()?
        else {
            bail!(gix::error::validation(
                "No default upstream defined for the current branch."
            ));
        };
        if remote.as_bstr() == "." {
            return Ok(merge.as_bstr().to_owned());
        }
        match repo
            .branch_remote_tracking_ref_name(branch.as_ref(), gix::remote::Direction::Fetch)
            .transpose()?
        {
            Some(tracking) => Ok(tracking.as_bstr().to_owned()),
            None => bail!(
                message!(
                    "No remote-tracking branch for {} from {}",
                    merge.as_bstr(),
                    remote.as_bstr()
                )
                .validation()
            ),
        }
    }

    /// Join the parts of the message of `-m` and `-F` in the order of the command line, as `option_parse_message()` and
    /// `option_read_message()` in Git's `builtin/merge.c` join them: a paragraph follows what is before it after an
    /// empty line, and the content of a file after a line feed. Return `None` without parts.
    fn join_message(parts: &[MessagePart]) -> Result<Option<BString>> {
        if parts.is_empty() {
            return Ok(None);
        }
        let mut joined = BString::default();
        for part in parts {
            match part {
                MessagePart::Paragraph(paragraph) => {
                    if !joined.is_empty() {
                        joined.extend_from_slice(b"\n\n");
                    }
                    joined.extend_from_slice(paragraph);
                }
                MessagePart::File(path) => {
                    if !joined.is_empty() {
                        joined.push(b'\n');
                    }
                    let content =
                        std::fs::read(path).or_raise(|| message!("could not read file '{}'", path.display()))?;
                    joined.extend_from_slice(&content);
                }
            }
        }
        Ok(Some(joined))
    }

    /// Warn on `err` about the files that the switch of the index and the worktree to the merge of `name` couldn't remove,
    /// as `unlink_entry()` in Git's `unpack-trees.c` warns, and fail if the checkout stopped before it wrote all files.
    fn report_changes(changes: Changes, name: &BStr, err: &mut dyn Write) -> Result<()> {
        for unremoved in &changes.unremoved {
            writeln!(
                err,
                "warning: unable to unlink '{}': {}",
                unremoved.path, unremoved.error
            )
            .or_error()?;
        }
        match changes.checkout_error {
            Some(error) => Err(error).or_raise(|| message!("Could not check out all files of '{name}'")),
            None => Ok(()),
        }
    }
}
