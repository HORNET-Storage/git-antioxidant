//! The todo list of a rebase: made as `sequencer_make_script()` makes it, completed and edited as `complete_action()`
//! and `edit_todo_list()` do, and checked as `todo_list_check()` checks it.

use gix_hash::ObjectId;
use gix_rebase::{
    Commits,
    script::{self, Decorations},
};
use gix_sequencer::{
    rebase::name,
    todo::{
        Command, Format, Instruction, Item, Line, LineError, List, MissingCommitsCheck, ParseOptions, Resolved, Start,
        append_help, missing_commits, missing_commits_report, strip_space,
    },
};

use super::{
    Sequencer,
    programs::{self, Edited},
};
use crate::{
    Repository, Result,
    bstr::{BStr, BString, ByteSlice},
    config::tree::Rebase,
    error::{bail, message},
    ext::ObjectIdExt,
    rebase::{Error, Outcome, Stop, StopReason},
};

/// What `complete_action()` completes the todo list of a starting rebase with.
pub(super) struct Completion<'a> {
    /// The commits that are replayed as the help of the todo list names them, like `5fd91cf..b361970`.
    pub shortrevisions: BString,
    /// The name of the commit to replay onto, which the reflog message of the checkout names.
    pub onto_name: BString,
    /// The commit to replay onto.
    pub onto: ObjectId,
    /// The commit that is rebased.
    pub orig_head: ObjectId,
    /// The commands of `--exec`.
    pub exec: &'a [BString],
    /// Whether `--autosquash` rearranges the list.
    pub autosquash: bool,
    /// Whether `--update-refs` adds `update-ref` commands.
    pub update_refs: bool,
    /// Whether commands are written with their letter, as `rebase.abbreviateCommands` says.
    pub abbreviate_commands: bool,
    /// Whether the sequence editor edits the list, which it does in an interactive rebase that may start programs.
    pub edit: bool,
}

impl Sequencer<'_> {
    /// Complete `list`, the todo list that a rebase starts with, as `complete_action()` does: add the commands of
    /// `--update-refs`, `--autosquash` and `--exec`, let the sequence editor edit the list, check it, skip the picks
    /// that leave their commits as they are, check out the commit to replay onto and perform the commands.
    pub(super) fn complete_action(&mut self, mut list: List, completion: Completion<'_>) -> Result<Outcome> {
        let object_hash = self.repo.object_hash();
        let comment = self.comment.clone();
        // Without commits to replay, the list does nothing, and the rebase still moves the branch to where it starts.
        if list.is_empty() {
            list.lines
                .push(Line::new(Item::Command(Instruction::new(Command::Noop, None, ""))));
        }
        if completion.update_refs {
            let decorations = branch_decorations(self.repo)?;
            let head_ref = self.repo.head_name()?;
            let worktrees = checked_out_branches(self.repo)?;
            let records = gix_rebase::todo::add_update_refs(
                &mut list,
                &decorations,
                head_ref.as_ref().map(AsRef::as_ref),
                comment.as_bstr(),
                &mut |name| {
                    worktrees
                        .iter()
                        .find(|(branch, _)| branch.as_ref() == name)
                        .map(|(_, path)| path.clone())
                },
                &mut |name| current_commit(self.repo, name.as_bstr()),
                object_hash,
            );
            self.dir.write_update_refs(&records)?;
        }
        if completion.autosquash {
            let mut commits = Commits::new(&self.repo.objects, object_hash);
            gix_rebase::todo::rearrange_squash(&mut list, &mut commits, &mut |name| resolve_commit(self.repo, name))?;
        }
        gix_rebase::todo::add_exec_commands(&mut list, completion.exec);
        if list.count_commands() == 0 {
            let edits = self.dir.remove()?;
            self.apply_edits(edits)?;
            bail!(Error::NothingToDo);
        }

        let check = missing_commits_check(self.repo)?;
        let shortonto = BString::from(self.short_id(&completion.onto));
        let start = Start {
            revisions: completion.shortrevisions.as_bstr(),
            onto: shortonto.as_bstr(),
        };
        let mut shorten = |id: &gix_hash::oid| -> Result<BString> { Ok(self.short_id(id).into()) };
        let mut todo = list.to_bytes(Format {
            abbreviate_commands: completion.abbreviate_commands,
            shorten_ids: Some(&mut shorten),
        })?;
        append_help(&mut todo, list.count_commands(), Some(start), comment.as_bstr(), check);
        self.dir.write_file(name::TODO, &todo)?;
        let mut backup = list.to_bytes(Format {
            abbreviate_commands: completion.abbreviate_commands,
            shorten_ids: None,
        })?;
        append_help(
            &mut backup,
            list.count_commands(),
            Some(start),
            comment.as_bstr(),
            check,
        );
        self.dir.write_file(name::TODO_BACKUP, &backup)?;

        if completion.edit {
            let editor = programs::sequence_editor(self.repo)?;
            if let Edited::Failed(reason) = programs::edit(self.repo, editor, &self.dir.file(name::TODO), &[])? {
                let edits = self.dir.remove()?;
                self.apply_edits(edits)?;
                bail!(message!("{reason}").validation());
            }
        }
        let edited = self.dir.read_file(name::TODO)?.unwrap_or_default();
        let edited = strip_space(&edited, Some(comment.as_bstr()));
        if edited.is_empty() {
            let edits = self.dir.remove()?;
            self.apply_edits(edits)?;
            bail!(Error::NothingToDo);
        }
        let (new_list, errors) = self.parse_todo(&edited)?;
        if !errors.is_empty() {
            self.checkout_onto(completion.onto_name.as_bstr(), completion.onto, completion.orig_head)?;
            return Ok(Outcome::Stopped(invalid_todo_list(&errors)));
        }
        if let Some(report) = self.check_todo_list(&list, &new_list, check)? {
            self.dir.write_file(name::DROPPED, b"")?;
            self.checkout_onto(completion.onto_name.as_bstr(), completion.onto, completion.orig_head)?;
            return Ok(Outcome::Stopped(report));
        }
        self.dir
            .filter_update_refs(&new_list, &mut |name| current_commit(self.repo, name))?;

        let (skip, base) = if self.allow_ff {
            let mut commits = Commits::new(&self.repo.objects, object_hash);
            gix_rebase::todo::skip_unnecessary_picks(&new_list, completion.onto, &mut commits)?
        } else {
            (0, completion.onto)
        };
        let mut progress = self.dir.start_todo(
            &new_list,
            skip,
            Format {
                abbreviate_commands: completion.abbreviate_commands,
                shorten_ids: None,
            },
        )?;
        self.checkout_onto(completion.onto_name.as_bstr(), base, completion.orig_head)?;
        let changes = self.worktree_changes()?;
        if changes.unstaged || changes.uncommitted {
            bail!(Error::DirtyWorktree {
                unstaged: changes.unstaged,
                uncommitted: changes.uncommitted,
            });
        }
        self.pick_commits(&mut progress)
    }

    /// Check the todo list of the rebase in progress, `list`, against the list that the editor got, as
    /// `todo_list_check_against_backup()` does before the rebase continues after an edit dropped commits, and return
    /// the stop if `rebase.missingCommitsCheck` refuses it still.
    pub(super) fn check_against_backup(&self, list: &List) -> Result<Option<Stop>> {
        let Some(backup) = self
            .dir
            .read_file(name::TODO_BACKUP)?
            .filter(|backup| !backup.is_empty())
        else {
            return Ok(None);
        };
        let (backup, _) = self.parse_todo(&backup)?;
        self.check_todo_list(&backup, list, missing_commits_check(self.repo)?)
    }

    /// Check that `new` names the commits of `old` as `todo_list_check()` does with `check`, and return the stop that
    /// Git makes if commits were dropped without `drop` and `check` is [`MissingCommitsCheck::Error`].
    fn check_todo_list(&self, old: &List, new: &List, check: MissingCommitsCheck) -> Result<Option<Stop>> {
        if check != MissingCommitsCheck::Error {
            return Ok(None);
        }
        let missing = missing_commits(old, new);
        if missing.is_empty() {
            return Ok(None);
        }
        let mut shorten = |id: &gix_hash::oid| -> Result<BString> { Ok(self.short_id(id).into()) };
        let report = missing_commits_report(&missing, &mut shorten)?;
        Ok(Some(Stop {
            reason: StopReason::InvalidTodoList,
            message: report,
        }))
    }

    /// Parse `todo`, a todo list of the rebase, as `todo_list_parse_insn_buffer()` does, finding its commits by any
    /// name of a revision.
    fn parse_todo(&self, todo: &[u8]) -> Result<(List, Vec<LineError>)> {
        let options = ParseOptions {
            comment: self.comment.as_bstr(),
            rebase: true,
            after_done: self.dir.has_file(name::DONE),
        };
        List::parse(todo, options, &mut resolver(self.repo))
    }

    /// Make the todo list that replays the commits of `walk`, as `sequencer_make_script()` and
    /// `make_script_with_merges()` make it with `options`, with the commits formatted as `rebase.instructionFormat`
    /// says, behind the comment `# ` that Git puts in front of a format that doesn't start with one.
    pub(super) fn make_script(&self, walk: &gix_rebase::walk::Walk, options: &script::Options<'_>) -> Result<List> {
        let object_hash = self.repo.object_hash();
        let mut commits = Commits::new(&self.repo.objects, object_hash);
        let format = self
            .repo
            .config
            .resolved
            .string(Rebase::INSTRUCTION_FORMAT)
            .filter(|format| !format.is_empty())
            .map(|format| {
                if format.starts_with(b"#") {
                    format
                } else {
                    let mut commented = BString::from("# ");
                    commented.extend_from_slice(&format);
                    commented
                }
            });
        let repo = self.repo;
        let mut oneline = |id: &gix_hash::oid, commit: &gix_rebase::Commit| -> Result<BString> {
            Ok(match &format {
                Some(format) => format_commit(repo, id, commit, format.as_bstr()),
                None => script::default_oneline(id, commit)?,
            })
        };
        let mut abbreviate = |id: &gix_hash::oid| -> Result<BString> { Ok(self.short_id(id).into()) };
        let decorations = if options.rebase_merges.is_some() {
            branch_decorations(self.repo)?
        } else {
            Decorations::default()
        };
        let mut context = script::Context {
            commits: &mut commits,
            oneline: &mut oneline,
            abbreviate: &mut abbreviate,
            decorations: &decorations,
        };
        script::make(&mut context, walk, options)
    }
}

/// Return the level of `rebase.missingCommitsCheck` of `repo`, which ignores values that Git ignores.
fn missing_commits_check(repo: &Repository) -> Result<MissingCommitsCheck> {
    Ok(repo
        .config
        .resolved
        .string(Rebase::MISSING_COMMITS_CHECK)
        .and_then(|value| MissingCommitsCheck::from_config_value(value.as_ref()))
        .unwrap_or_default())
}

/// Return the stop of a rebase whose todo list has the lines of `errors`, which Git reports with its advice.
fn invalid_todo_list(errors: &[LineError]) -> Stop {
    let mut message = BString::default();
    for error in errors {
        message.extend_from_slice(format!("error: {error}\n").as_bytes());
        if let Some(advice) = error.reason.advice() {
            message.extend_from_slice(format!("hint: {advice}\n").as_bytes());
        }
    }
    message.extend_from_slice(
        b"You can fix this with 'git rebase --edit-todo' and then run 'git rebase --continue'.\nOr you can abort the rebase with 'git rebase --abort'.\n",
    );
    Stop {
        reason: StopReason::InvalidTodoList,
        message,
    }
}

/// Return a [`Resolve`](gix_sequencer::todo::Resolve) that finds commits in `repo` by any name of a revision, as
/// `parse_insn_line()` does with `repo_get_oid()`.
pub(super) fn resolver(repo: &Repository) -> impl FnMut(&BStr) -> Result<Resolved> + '_ {
    move |name: &BStr| {
        let Some(id) = revision(repo, name) else {
            return Ok(Resolved::Nothing);
        };
        let object = id.object()?.peel_tags_to_end()?;
        Ok(match object.kind {
            gix_object::Kind::Commit => {
                let commit = object.into_commit();
                Resolved::Commit {
                    id: commit.id,
                    is_merge: commit.parent_ids().count() > 1,
                }
            }
            kind => Resolved::NotACommit { id: object.id, kind },
        })
    }
}

/// Return the commit that `name` names in `repo` as a revision, or `None` if it names none.
pub(super) fn resolve_commit(repo: &Repository, name: &BStr) -> Option<ObjectId> {
    revision(repo, name)?
        .object()
        .ok()?
        .peel_to_commit()
        .ok()
        .map(|commit| commit.id)
}

/// Return the object that `name` names in `repo` as a revision, as `repo_get_oid()` finds it, or `None` if it names
/// none.
pub(super) fn revision<'repo>(repo: &'repo Repository, name: &BStr) -> Option<crate::Id<'repo>> {
    repo.rev_parse_single(name).ok()
}

/// Return the commit to which the reference `name` of `repo` points, or `None` if it can't be read.
fn current_commit(repo: &Repository, name: &BStr) -> Option<ObjectId> {
    repo.try_find_reference(name)
        .ok()??
        .peel_to_id()
        .ok()
        .map(crate::Id::detach)
}

/// Return the branches of `repo`, which decorate the commits of `--update-refs` and `--rebase-merges`, as
/// `load_branch_decorations()` loads them.
fn branch_decorations(repo: &Repository) -> Result<Decorations> {
    let mut branches = Vec::new();
    for reference in repo.references()?.local_branches()? {
        let Ok(mut reference) = reference else {
            continue;
        };
        if let Ok(id) = reference.peel_to_id() {
            branches.push((reference.name().to_owned(), id.detach()));
        }
    }
    Ok(Decorations::new(branches))
}

/// Return the branches that the worktrees of `repo` have checked out, each with the path of its worktree, as
/// `branch_checked_out()` finds them.
fn checked_out_branches(repo: &Repository) -> Result<Vec<(gix_ref::FullName, BString)>> {
    let mut branches = Vec::new();
    for worktree in repo.worktrees_including_main()? {
        let Ok(worktree) = worktree else {
            continue;
        };
        if let (Some(branch), Some(path)) = (worktree.head_name()?, worktree.workdir()) {
            branches.push((branch, gix_path::into_bstr(path)?.into_owned()));
        }
    }
    Ok(branches)
}

/// Return the text of `commit` with the id `id` after its id in the todo list, formatted with `format`, the value of
/// `rebase.instructionFormat`, as `pretty_print_commit()` formats it with `%H`, `%h`, `%T`, `%t`, `%P`, `%p`, `%an`,
/// `%ae`, `%s`, `%n` and `%%`. Other placeholders are kept as they are.
fn format_commit(repo: &Repository, id: &gix_hash::oid, commit: &gix_rebase::Commit, format: &BStr) -> BString {
    let short = |id: &gix_hash::oid| id.to_owned().attach(repo).shorten_or_id().to_string();
    let author = gix_actor::SignatureRef::from_bytes(commit.author.as_ref()).ok();
    let mut out = BString::default();
    let mut rest = format.as_bytes();
    while let Some(position) = rest.find_byte(b'%') {
        out.extend_from_slice(&rest[..position]);
        rest = &rest[position..];
        let (replacement, length): (Option<BString>, usize) = match rest.get(1..3) {
            Some(b"an") => (author.map(|author| author.name.to_owned()), 3),
            Some(b"ae") => (author.map(|author| author.email.to_owned()), 3),
            _ => match rest.get(1) {
                Some(b'H') => (Some(id.to_string().into()), 2),
                Some(b'h') => (Some(short(id).into()), 2),
                Some(b'T') => (Some(commit.tree.to_string().into()), 2),
                Some(b't') => (Some(short(&commit.tree).into()), 2),
                Some(b'P') => (Some(join(commit.parents.iter().map(ToString::to_string))), 2),
                Some(b'p') => (Some(join(commit.parents.iter().map(|parent| short(parent)))), 2),
                Some(b's') => (Some(commit.subject()), 2),
                Some(b'n') => (Some("\n".into()), 2),
                Some(b'%') => (Some("%".into()), 2),
                _ => (None, 1),
            },
        };
        match replacement {
            Some(replacement) => out.extend_from_slice(&replacement),
            None => out.extend_from_slice(&rest[..length]),
        }
        rest = &rest[length..];
    }
    out.extend_from_slice(rest);
    out
}

/// Join `parts` with spaces.
fn join(parts: impl Iterator<Item = String>) -> BString {
    parts.collect::<Vec<_>>().join(" ").into()
}
