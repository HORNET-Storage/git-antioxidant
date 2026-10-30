//! Starting a rebase as `cmd_rebase()`, `run_sequencer_rebase()` and `do_interactive_rebase()` in `builtin/rebase.c`
//! start it.

use std::sync::atomic::AtomicBool;

use gix_hash::ObjectId;
use gix_rebase::{Commits, plan::Plan, script, walk};
use gix_ref::transaction::{PreviousValue, RefEdit};
use gix_sequencer::{
    rebase::{Options as StateOptions, State},
    refs,
};

use super::{
    Sequencer,
    commit::remove_file,
    programs::allow_exec,
    refs::{attach_head, detach_head},
    state_dir,
    todo::{Completion, resolve_commit},
    worktree_changes,
};
use crate::{
    Repository, Result,
    bstr::{BStr, BString, ByteSlice},
    config::{
        cache::util::ApplyLeniency,
        tree::{Rebase, keys},
    },
    error::{ResultExt, bail, message},
    rebase::{Empty, Error, Merges, Options, Outcome, Programs},
};

/// Start the rebase of `repo` that `options` describe, as `cmd_rebase()` does without an action, and perform its
/// commands as `run_sequencer_rebase()` does.
pub(super) fn rebase(repo: &Repository, options: Options, should_interrupt: &AtomicBool) -> Result<Outcome> {
    if repo.workdir().is_none() {
        bail!(Error::BareRepository);
    }
    let dir = state_dir(repo);
    let git_dir = repo.current_dir().join(repo.git_dir());
    for state_dir in [dir.path().to_owned(), git_dir.join("rebase-apply")] {
        if state_dir.is_dir() {
            bail!(Error::InProgress { state_dir });
        }
    }
    if options.keep_base {
        if options.onto.is_some() {
            bail!(Error::IncompatibleOptions {
                first: "--keep-base",
                second: "--onto",
            });
        }
        if options.root {
            bail!(Error::IncompatibleOptions {
                first: "--keep-base",
                second: "--root",
            });
        }
    }
    // As `fork_point` in `cmd_rebase()`: negative while unset, which the missing upstream turns into `--fork-point`.
    let mut fork_point: i8 = match options.fork_point {
        Some(fork_point) => i8::from(fork_point),
        None if config_bool(repo, &Rebase::FORK_POINT)? == Some(false) => 0,
        None => -1,
    };
    if options.keep_base && fork_point < 0 {
        fork_point = 0;
    }
    if options.root && fork_point > 0 {
        bail!(Error::IncompatibleOptions {
            first: "--root",
            second: "--fork-point",
        });
    }
    for command in &options.exec {
        if command.contains(&b'\n') {
            bail!(message("exec commands cannot contain newlines").validation_error());
        }
        if command
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\x0c' | b'\x0b'))
        {
            bail!(message("empty exec command").validation_error());
        }
    }
    if !options.exec.is_empty() {
        allow_exec(repo, options.programs)?;
    }

    let interactive = options.interactive;
    let allow_preemptive_ff = !(interactive || !options.exec.is_empty() || options.autosquash == Some(true));
    let reapply_cherry_picks = options.reapply_cherry_picks.unwrap_or(options.keep_base);
    let update_refs = match options.update_refs {
        Some(update_refs) => update_refs,
        None => config_bool(repo, &Rebase::UPDATE_REFS)?.unwrap_or(false),
    };
    let rebase_merges = match options.rebase_merges {
        Merges::Drop => None,
        Merges::Rebase(mode) => Some(mode),
        Merges::AsConfigured => configured_rebase_merges(repo)?,
    };
    let autosquash = match options.autosquash {
        Some(autosquash) => autosquash,
        None => interactive && config_bool(repo, &Rebase::AUTO_SQUASH)?.unwrap_or(false),
    };
    let empty = options.empty.unwrap_or(if interactive {
        Empty::Stop
    } else if !options.exec.is_empty() {
        Empty::Keep
    } else {
        Empty::Drop
    });
    let reschedule_failed_exec = match options.reschedule_failed_exec {
        Some(reschedule) => reschedule,
        None => config_bool(repo, &Rebase::RESCHEDULE_FAILED_EXEC)?.unwrap_or(false),
    };
    let object_hash = repo.object_hash();
    let mut commits = Commits::new(&repo.objects, object_hash);

    let (upstream_name, upstream) = if options.root {
        (None, None)
    } else {
        let name: BString = match &options.upstream {
            Some(name) if name == "-" => "@{-1}".into(),
            Some(name) => name.clone(),
            None => {
                let head_name = repo.head_name()?.ok_or(Error::NoUpstream).or_error()?;
                let fetch = crate::remote::Direction::Fetch;
                // As `set_merge()` takes it, the upstream of a branch whose remote is `.` is the local reference to merge.
                let is_local = repo
                    .branch_remote_name(head_name.shorten(), fetch)
                    .is_some_and(|remote| remote.as_bstr() == ".");
                let tracking = if is_local {
                    repo.branch_remote_ref_name(head_name.as_ref(), fetch)
                } else {
                    repo.branch_remote_tracking_ref_name(head_name.as_ref(), fetch)
                }
                .transpose()?
                .ok_or(Error::NoUpstream)
                .or_error()?;
                if fork_point < 0 {
                    fork_point = 1;
                }
                tracking.as_bstr().to_owned()
            }
        };
        let upstream = resolve_commit(repo, name.as_bstr())
            .ok_or_else(|| Error::InvalidUpstream { name: name.clone() })
            .or_error()?;
        (Some(name), Some(upstream))
    };
    let (onto_name, squash_onto, root_with_onto) = match (&options.onto, options.root) {
        (None, true) => {
            let squash_onto = empty_root_commit(repo)?;
            (Some(BString::from(squash_onto.to_string())), Some(squash_onto), false)
        }
        (Some(onto), root) => (Some(onto.clone()), None, root),
        (None, false) => (None, None, false),
    };

    let (head_name, orig_head, branch_name, switch_to) = match &options.branch {
        Some(branch) => {
            let mut full = BString::from("refs/heads/");
            full.extend_from_slice(branch);
            let reference = repo
                .try_find_reference(full.as_bstr())?
                .filter(|reference| reference.name().as_bstr() == full);
            match reference {
                Some(mut reference) => {
                    refuse_if_used_by_other_worktree(repo, reference.name())?;
                    (
                        Some(reference.name().to_owned()),
                        reference.peel_to_id()?.detach(),
                        branch.clone(),
                        Some(branch.clone()),
                    )
                }
                None => (
                    None,
                    resolve_commit(repo, branch.as_bstr())
                        .ok_or_else(|| Error::InvalidBranch { name: branch.clone() })
                        .or_error()?,
                    branch.clone(),
                    Some(branch.clone()),
                ),
            }
        }
        None => {
            let head_name = repo.head_name()?;
            let branch_name = match &head_name {
                Some(name) => name
                    .as_bstr()
                    .strip_prefix(b"refs/heads/")
                    .map_or_else(|| name.as_bstr().to_owned(), |short| short.as_bstr().to_owned()),
                None => "HEAD".into(),
            };
            let orig_head = repo
                .head_id()
                .ok()
                .and_then(|id| id.object().ok()?.peel_to_commit().ok())
                .map(|commit| commit.id)
                .ok_or_else(|| message("Could not resolve HEAD to a commit").validation_error())?;
            (head_name, orig_head, branch_name, None)
        }
    };

    let onto_name = if options.keep_base {
        let mut name = upstream_name.clone().unwrap_or_default();
        name.extend_from_slice(b"...");
        name.extend_from_slice(&branch_name);
        name
    } else {
        onto_name
            .or_else(|| upstream_name.clone())
            .ok_or_else(|| message("a base commit must be provided with --upstream or --onto").validation_error())?
    };
    let (onto, branch_base) = match onto_name.find("...") {
        Some(position) => {
            let side = |name: &[u8]| -> Option<ObjectId> {
                resolve_commit(repo, if name.is_empty() { "HEAD".into() } else { name.as_bstr() })
            };
            let base = match (side(&onto_name[..position]), side(&onto_name[position + 3..])) {
                (Some(left), Some(right)) => match gix_rebase::merge_bases(&mut commits, left, &[right])?.as_slice() {
                    [base] => Some(*base),
                    _ => None,
                },
                _ => None,
            };
            let Some(base) = base else {
                bail!(match (options.keep_base, &upstream_name) {
                    (true, Some(upstream_name)) => Error::MergeBase {
                        name: upstream_name.clone(),
                        with_branch: true,
                    },
                    _ => Error::MergeBase {
                        name: onto_name.clone(),
                        with_branch: false,
                    },
                });
            };
            (base, Some(base))
        }
        None => {
            let onto = resolve_commit(repo, onto_name.as_bstr())
                .ok_or_else(|| Error::InvalidOnto {
                    name: onto_name.clone(),
                })
                .or_error()?;
            let branch_base = match gix_rebase::merge_bases(&mut commits, onto, &[orig_head])?.as_slice() {
                [base] => Some(*base),
                _ => None,
            };
            (onto, branch_base)
        }
    };
    let upstream = if options.keep_base && reapply_cherry_picks {
        Some(onto)
    } else {
        upstream
    };
    let restrict = match upstream_name.as_ref().filter(|_| fork_point > 0) {
        Some(name) => {
            let Some(mut reference) = repo.try_find_reference(name.as_bstr())? else {
                bail!(Error::NoSuchRef { name: name.clone() });
            };
            let current = reference.peel_to_id()?.detach();
            let entries = gix_rebase::fork_point::reflog_entries(&repo.refs, reference.name())?;
            gix_rebase::fork_point::fork_point(&mut commits, &entries, current, orig_head)?
        }
        None => None,
    };

    let changes = worktree_changes(repo)?;
    if changes.unstaged || changes.uncommitted {
        bail!(Error::DirtyWorktree {
            unstaged: changes.unstaged,
            uncommitted: changes.uncommitted,
        });
    }

    let plan = Plan {
        onto,
        upstream,
        restrict,
        branch_base,
        head: orig_head,
    };
    let gpg_sign = repo.config.may_sign_commits()?.then(BString::default);
    let state = State {
        head_name: head_name.clone(),
        onto,
        orig_head,
        options: StateOptions {
            drop_redundant_commits: empty == Empty::Drop,
            keep_redundant_commits: empty == Empty::Keep,
            reschedule_failed_exec,
            gpg_sign,
            ..Default::default()
        },
        squash_onto,
    };
    let reflog_action = options.reflog_action.clone();
    let mut sequencer = Sequencer::new(repo, dir, state, reflog_action, options.programs, should_interrupt)?;
    sequencer.allow_ff = !options.force_rebase;

    if allow_preemptive_ff && !options.force_rebase && plan.can_fast_forward(&mut commits)? {
        if let Some(switch_to) = &switch_to {
            sequencer.checkout_up_to_date(head_name.as_ref(), orig_head, switch_to.as_bstr())?;
        }
        sequencer.apply_edits([refs::delete(refs::REBASE_HEAD)?, refs::delete(refs::AUTO_MERGE)?])?;
        return Ok(Outcome::UpToDate { name: branch_name });
    }

    #[cfg(feature = "hooks")]
    if !options.no_verify {
        let upstream_arg = match &upstream_name {
            Some(name) => gix_path::from_bstr(name.as_bstr())?.into_owned().into_os_string(),
            None => "--root".into(),
        };
        let branch = match &options.branch {
            Some(branch) => Some(gix_path::from_bstr(branch.as_bstr())?.into_owned().into_os_string()),
            None => None,
        };
        if !repo.hooks()?.pre_rebase(&upstream_arg, branch.as_deref())?.success() {
            bail!(Error::PreRebaseHookRefused);
        }
    }

    let abbreviate_commands = config_bool(repo, &Rebase::ABBREVIATE_COMMANDS)?.unwrap_or(false);
    let shorthead = sequencer.short_id(&orig_head);
    let shortrevisions: BString = match plan.upstream {
        Some(upstream) => format!("{}..{shorthead}", sequencer.short_id(&upstream)).into(),
        None => shorthead.into(),
    };
    let walk = plan.walk(
        &mut commits,
        walk::Options {
            reapply_cherry_picks,
            rebase_merges: rebase_merges.is_some(),
        },
    )?;
    let comment = sequencer.comment.clone();
    let list = sequencer.make_script(
        &walk,
        &script::Options {
            rebase_merges,
            root_with_onto,
            comment: comment.as_bstr(),
            ..Default::default()
        },
    )?;

    sequencer.dir.start(&sequencer.state)?;
    remove_file(&git_dir.join("logs").join(refs::REBASE_HEAD))?;
    sequencer.complete_action(
        list,
        Completion {
            shortrevisions,
            onto_name,
            onto,
            orig_head,
            exec: &options.exec,
            autosquash,
            update_refs,
            abbreviate_commands,
            edit: interactive && options.programs == Programs::Allow,
        },
    )
}

impl Sequencer<'_> {
    /// Check out `orig_head`, the branch `head_name` or a detached `HEAD`, as `checkout_up_to_date()` does when the branch
    /// to rebase was given and is up to date: the index and the worktree switch to it, `HEAD` points to it with the
    /// reflog message `<action>: checkout <switch_to>`, and `post-checkout` runs, whose failure Git ignores.
    fn checkout_up_to_date(
        &self,
        head_name: Option<&gix_ref::FullName>,
        orig_head: ObjectId,
        switch_to: &BStr,
    ) -> Result<()> {
        #[cfg(feature = "hooks")]
        let hooks = self.repo.hooks()?;
        let head = self.head_commit()?;
        if let Some(refused) = self.switch_worktree(head, orig_head)? {
            bail!(message!("{refused}\ncould not switch to {switch_to}").validation());
        }
        let mut log_message = self.reflog_action.clone();
        log_message.extend_from_slice(b": checkout ");
        log_message.extend_from_slice(switch_to);
        match head_name {
            Some(branch) => {
                self.repo.edit_reference(RefEdit::update(
                    branch.clone(),
                    orig_head,
                    PreviousValue::Any,
                    log_message.clone(),
                ))?;
                attach_head(self.repo, branch, log_message.as_bstr())?;
            }
            None => detach_head(self.repo, orig_head, log_message.as_bstr())?,
        }
        #[cfg(feature = "hooks")]
        hooks.post_checkout(
            &head.unwrap_or_else(|| self.repo.object_hash().null()),
            &orig_head,
            gix_hook::invocation::Checkout::Branch,
        )?;
        Ok(())
    }
}

/// Make the empty root commit onto which `git rebase --root` without `--onto` and `reset [new root]` replay commits,
/// as `commit_tree()` makes it with an empty message, the empty tree, no parent, and the configured author and
/// committer.
pub(super) fn empty_root_commit(repo: &Repository) -> Result<ObjectId> {
    let author = repo
        .author()
        .ok_or_else(|| message("Author identity is not configured").validation_error())??
        .to_owned()?;
    let committer = repo
        .committer()
        .ok_or_else(|| message("Committer identity is not configured").validation_error())??
        .to_owned()?;
    // The checkouts of the rebase read the empty tree from the object database.
    let tree = repo.write_object(gix_object::Tree::empty())?.detach();
    let commit = gix_object::Commit {
        tree,
        parents: Default::default(),
        author,
        committer,
        encoding: None,
        message: BString::default(),
        extra_headers: Vec::new(),
    };
    Ok(repo.write_object(&commit)?.detach())
}

/// Return the boolean value of `key` in the configuration of `repo`, or `None` if it isn't set.
fn config_bool(repo: &Repository, key: &'static keys::Boolean) -> Result<Option<bool>> {
    key.enrich_error(repo.config.resolved.boolean(key))
        .with_leniency(repo.config.lenient_config)
}

/// Return what `rebase.rebaseMerges` of `repo` asks for: `None` if it is `false` or isn't set, and the mode of
/// `--rebase-merges` otherwise, as `rebase_config()` reads it.
fn configured_rebase_merges(repo: &Repository) -> Result<Option<script::RebaseMerges>> {
    let Some(value) = repo.config.resolved.string_implicit(Rebase::REBASE_MERGES) else {
        return Ok(None);
    };
    let rebase_cousins = Rebase::REBASE_MERGES
        .try_into_rebase_merges(value)
        .map(Some)
        .with_leniency(repo.config.lenient_config)?
        .flatten();
    Ok(rebase_cousins.map(|rebase_cousins| {
        if rebase_cousins {
            script::RebaseMerges::RebaseCousins
        } else {
            script::RebaseMerges::NoRebaseCousins
        }
    }))
}

/// Refuse to rebase `branch` if a worktree other than the current one uses it, as `die_if_checked_out()` does when it
/// ignores the current worktree: if the worktree has it checked out, or rebases or bisects it with a detached `HEAD`.
fn refuse_if_used_by_other_worktree(repo: &Repository, branch: &gix_ref::FullNameRef) -> Result<()> {
    let Some(workdirs) = repo.checked_out_branches()?.remove(branch) else {
        return Ok(());
    };
    let id = |repo: &Repository| -> Result<Option<BString>> {
        Ok(match repo.worktree() {
            Some(worktree) => worktree.id()?.map(ToOwned::to_owned),
            None => None,
        })
    };
    let current = id(repo)?;
    for worktree in repo.worktrees_including_main()? {
        let Ok(worktree) = worktree else {
            continue;
        };
        if id(&worktree)? == current {
            continue;
        }
        if let Some(path) = worktree.workdir().filter(|path| workdirs.iter().any(|dir| dir == path)) {
            bail!(Error::UsedByWorktree {
                name: branch.shorten().to_owned(),
                path: path.to_owned(),
            });
        }
    }
    Ok(())
}
