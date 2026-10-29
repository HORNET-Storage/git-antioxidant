//! The state of a rebase in `.git/rebase-merge`, read and written as `sequencer.c` and `builtin/rebase.c` do, and the
//! steps of a rebase through its todo list.
//!
//! [`State`] is what a rebase does: the branch that it rebases, the commit onto which it replays commits, the commit at
//! which it started and its options. [`Dir::start()`] writes it as `init_basic_state()` and `write_basic_state()` do, and
//! [`Dir::state()`] reads it as `read_basic_state()` and `read_populate_opts()` do.
//!
//! The todo list goes through a [`Progress`]: [`Dir::start_todo()`] writes the list that the rebase starts with as
//! `complete_action()` does, [`Dir::progress()`] reads it back as `read_populate_todo()` does, and [`Dir::begin()`]
//! starts its next command as each round of `pick_commits()` does. The other files record what the commands did, for
//! `git rebase --continue`, for the `post-rewrite` hook and for the branches that the rebase updates when it ends.
//!
//! The caller performs the commands, makes the commits, merges and resets, runs the hooks, and applies the reference
//! edits that the functions here return, in their order. [`Dir::prepare_continue()`], [`Dir::skip()`],
//! [`Dir::abort()`] and [`Dir::remove()`] take the steps of `git rebase --continue`, `--skip`, `--abort` and `--quit`,
//! and [`Dir::finish()`] those at the end of a rebase.

use std::path::{Path, PathBuf};

use bstr::{BStr, BString, ByteSlice};
use gix_error::{ErrorExt, Result, ResultExt, bail, message};
use gix_hash::ObjectId;
use gix_ref::{
    FullName, Target,
    transaction::{LogChange, PreviousValue, RefEdit},
};

use crate::{
    file, refs,
    todo::{Command, Format, Item, List, ParseOptions, Resolve},
};

mod author_script;
pub use author_script::AuthorScript;

/// The name of the directory of the state of a rebase in the git directory.
pub const DIR: &str = "rebase-merge";

/// The text of `head-name` for a rebase of a detached `HEAD`.
pub const DETACHED_HEAD: &str = "detached HEAD";

/// The names of the files in the directory of a rebase, as `sequencer.c` names them.
pub mod name {
    /// The branch that the rebase updates when it ends, or `detached HEAD`.
    pub const HEAD_NAME: &str = "head-name";
    /// The commit onto which the rebase replays commits.
    pub const ONTO: &str = "onto";
    /// The commit at which `HEAD` was when the rebase started.
    pub const ORIG_HEAD: &str = "orig-head";
    /// The commands that the rebase has yet to perform.
    pub const TODO: &str = "git-rebase-todo";
    /// The todo list as the editor got it, to find the commits that an edit of the list dropped.
    pub const TODO_BACKUP: &str = "git-rebase-todo.backup";
    /// The commands that the rebase performed, ending with the current one.
    pub const DONE: &str = "done";
    /// The number of the current command.
    pub const MSGNUM: &str = "msgnum";
    /// The number of all commands.
    pub const END: &str = "end";
    /// The mark of a rebase with the merge backend, which is empty.
    pub const INTERACTIVE: &str = "interactive";
    /// The message of the commit that `git rebase --continue` makes.
    pub const MESSAGE: &str = "message";
    /// The messages of a chain of `fixup` and `squash` commands with a `squash`, so far.
    pub const SQUASH_MESSAGE: &str = "message-squash";
    /// The message of the commit into which a chain of `fixup` commands melds commits.
    pub const FIXUP_MESSAGE: &str = "message-fixup";
    /// The `fixup` and `squash` commands of the current chain, so far.
    pub const CURRENT_FIXUPS: &str = "current-fixups";
    /// The author of the commit that is being made.
    pub const AUTHOR_SCRIPT: &str = "author-script";
    /// The commit that `git rebase --continue` amends.
    pub const AMEND: &str = "amend";
    /// The commit at which the rebase stopped.
    pub const STOPPED_SHA: &str = "stopped-sha";
    /// The patch of the commit at which the rebase stopped.
    pub const PATCH: &str = "patch";
    /// The commits that the rebase rewrote, each with the commit that it made of it.
    pub const REWRITTEN_LIST: &str = "rewritten-list";
    /// The commits that the rebase rewrote, whose new commit a `fixup` or `squash` has yet to make.
    pub const REWRITTEN_PENDING: &str = "rewritten-pending";
    /// The empty commit to which `reset [new root]` resets.
    pub const SQUASH_ONTO: &str = "squash-onto";
    /// The references of the `label` commands, which go with the state.
    pub const REFS_TO_DELETE: &str = "refs-to-delete";
    /// The branches that the rebase updates when it ends.
    pub const UPDATE_REFS: &str = "update-refs";
    /// The mark of an edit of the todo list that dropped commits, which is empty.
    pub const DROPPED: &str = "dropped";
    /// The option `--quiet`, which is empty.
    pub const QUIET: &str = "quiet";
    /// The option `--verbose`, which is empty.
    pub const VERBOSE: &str = "verbose";
    /// The merge strategy.
    pub const STRATEGY: &str = "strategy";
    /// The options of the merge strategy.
    pub const STRATEGY_OPTS: &str = "strategy_opts";
    /// The option `--rerere-autoupdate` or `--no-rerere-autoupdate`.
    pub const ALLOW_RERERE_AUTOUPDATE: &str = "allow_rerere_autoupdate";
    /// The option `-S` with the key that signs the commits.
    pub const GPG_SIGN_OPT: &str = "gpg_sign_opt";
    /// The option `--signoff`.
    pub const SIGNOFF: &str = "signoff";
    /// The option `--empty=drop`, which is empty.
    pub const DROP_REDUNDANT_COMMITS: &str = "drop_redundant_commits";
    /// The option `--empty=keep`, which is empty.
    pub const KEEP_REDUNDANT_COMMITS: &str = "keep_redundant_commits";
    /// The option `--committer-date-is-author-date`, which is empty.
    pub const CDATE_IS_ADATE: &str = "cdate_is_adate";
    /// The option `--ignore-date`, which is empty.
    pub const IGNORE_DATE: &str = "ignore_date";
    /// The option `--reschedule-failed-exec`, which is empty.
    pub const RESCHEDULE_FAILED_EXEC: &str = "reschedule-failed-exec";
    /// The option `--no-reschedule-failed-exec`, which is empty.
    pub const NO_RESCHEDULE_FAILED_EXEC: &str = "no-reschedule-failed-exec";
    /// The trailers that `--trailer` adds to each commit, one per line.
    pub const TRAILER: &str = "trailer";
}

/// The files of a stopped merge in the git directory, which `remove_branch_state()` removes.
const MERGE_FILES: [&str; 5] = ["MERGE_HEAD", "MERGE_RR", "MERGE_MSG", "MERGE_MODE", "SQUASH_MSG"];

/// The options of a rebase that its state directory keeps, as `write_basic_state()` writes them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Options {
    /// `--quiet`, kept in `quiet`.
    pub quiet: bool,
    /// `--verbose`, kept in `verbose`.
    pub verbose: bool,
    /// The merge strategy of `--strategy`, kept in `strategy`.
    pub strategy: Option<BString>,
    /// The options of `--strategy-option` for the merge strategy, without a leading `--`, kept in `strategy_opts`.
    pub strategy_options: Vec<BString>,
    /// `--rerere-autoupdate` if `true` and `--no-rerere-autoupdate` if `false`, kept in `allow_rerere_autoupdate`.
    pub rerere_autoupdate: Option<bool>,
    /// The key of `--gpg-sign`, which is empty for the default key, kept in `gpg_sign_opt` after `-S`.
    pub gpg_sign: Option<BString>,
    /// `--signoff`, kept in `signoff`.
    pub signoff: bool,
    /// `--empty=drop`, kept in `drop_redundant_commits`.
    pub drop_redundant_commits: bool,
    /// `--empty=keep`, kept in `keep_redundant_commits`.
    pub keep_redundant_commits: bool,
    /// `--committer-date-is-author-date`, kept in `cdate_is_adate`.
    pub committer_date_is_author_date: bool,
    /// `--ignore-date`, kept in `ignore_date`.
    pub ignore_date: bool,
    /// `--reschedule-failed-exec`, kept in `reschedule-failed-exec`, or in `no-reschedule-failed-exec` if `false`.
    pub reschedule_failed_exec: bool,
    /// The trailers of `--trailer`, kept in `trailer`.
    pub trailers: Vec<BString>,
}

/// What a rebase does, as its state directory keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// The branch that the rebase updates when it ends, or `None` for a detached `HEAD`, kept in `head-name`.
    pub head_name: Option<FullName>,
    /// The commit onto which the rebase replays commits, kept in `onto`.
    pub onto: ObjectId,
    /// The commit at which `HEAD` was when the rebase started, kept in `orig-head`.
    pub orig_head: ObjectId,
    /// The options of the rebase.
    pub options: Options,
    /// The empty commit to which `reset [new root]` resets, kept in `squash-onto`. `git rebase --root` without `--onto`
    /// makes it when it starts, and a `reset [new root]` otherwise.
    pub squash_onto: Option<ObjectId>,
}

/// A todo list that a rebase performs, as `struct todo_list` holds it while `pick_commits()` goes through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// The commands that the rebase has yet to perform, with the current one at `current` once it began.
    pub list: List,
    /// The index of the current command in `list`.
    pub current: usize,
    /// The number of commands that the rebase began, as `msgnum` holds it.
    pub done: usize,
    /// The number of all commands, as `end` holds it.
    pub total: usize,
}

impl Progress {
    /// Return the command of the first line from `offset` after the current one that doesn't do nothing, as
    /// `peek_command()` does, or `None` if there is none.
    pub fn peek_command(&self, offset: usize) -> Option<Command> {
        self.list.peek_command(self.current + offset)
    }
}

/// The command with which a round of `pick_commits()` began, as [`Dir::begin()`] returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Begun {
    /// The index of the command in the list of the progress.
    pub index: usize,
    /// The edits that remove `AUTO_MERGE` and `REBASE_HEAD`, which each command starts without.
    pub edits: Vec<RefEdit>,
}

/// Why a rebase stops at a command, which decides the files that [`Dir::stop()`] writes, as `error_with_patch()` writes
/// them.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stop<'a> {
    /// The commit of the command, which `stopped-sha` and `REBASE_HEAD` name. It is `None` for a `merge` without a
    /// commit.
    pub commit: Option<ObjectId>,
    /// The patch of `commit` against its first parent, as `make_patch()` writes it with `log_tree_commit()`, which is the
    /// caller's to make. `patch` isn't written if it is `None`.
    pub patch: Option<&'a [u8]>,
    /// The message that the command prepared for its commit, which `message` gets unless a failed `fixup` or `squash`
    /// wrote it before.
    pub prepared_message: Option<&'a [u8]>,
    /// The message of `commit` from its subject on, which `message` gets with a line feed after it if nothing wrote it
    /// before, as `make_patch()` writes it.
    pub commit_message: Option<&'a [u8]>,
    /// The commit that `git rebase --continue` amends, `HEAD` after an `edit` or after a `reword` that fast-forwarded,
    /// which `amend` gets, as `intend_to_amend()` writes it.
    pub amend: Option<ObjectId>,
}

/// A branch that a rebase updates when it ends, as an entry of `update-refs` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRef {
    /// The full name of the branch, as the `update-ref` command names it.
    pub name: BString,
    /// The commit to which the branch pointed when the rebase started, or the null id if it didn't exist.
    pub before: ObjectId,
    /// The commit that the `update-ref` command recorded for the branch, or the null id if it didn't run yet.
    pub after: ObjectId,
}

/// What the caller found for `git rebase --continue` in the worktree and the index.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Changes {
    /// The worktree has changes that aren't staged, ignoring submodules, as `has_unstaged_changes()` tells.
    pub unstaged: bool,
    /// The index has changes against `HEAD`, as `has_uncommitted_changes()` tells.
    pub uncommitted: bool,
}

/// What `git rebase --continue` does with the staged changes before it performs the next command, as
/// `commit_staged_changes()` decides it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Continue {
    /// The edits to apply first: the removal of `CHERRY_PICK_HEAD`, if it exists, when nothing is staged.
    pub edits: Vec<RefEdit>,
    /// Whether to remove `MERGE_MSG` from the git directory first, as Git does when nothing is staged.
    pub remove_merge_message: bool,
    /// The commit to make, or `None` if there is nothing to commit.
    pub commit: Option<Commit>,
}

/// How `git rebase --continue` commits the staged changes, as `commit_staged_changes()` runs `git commit`, which
/// allows an empty commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Amend `HEAD` instead of making a new commit, as `AMEND_MSG` says when the rebase stopped for amending.
    pub amend: bool,
    /// Open the editor for the message, as `EDIT_MSG` says.
    pub edit_message: bool,
    /// Clean up the message without opening the editor, as `CLEANUP_MSG` says for the last `fixup` of a chain without
    /// `squash`.
    pub cleanup_message: bool,
    /// The file with the message, `message` of the state directory, or `None` to keep the message of the amended
    /// commit.
    pub message_file: Option<PathBuf>,
    /// Whether the commit ends a chain of `fixup` and `squash` commands, after which `message-fixup` and
    /// `message-squash` go.
    pub final_fixup: bool,
    /// Whether `current-fixups` goes after the commit, as the chain had commands.
    pub remove_current_fixups: bool,
}

/// The steps that the end of a rebase leaves to the caller, as `pick_commits()` ends it, in their order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finish {
    /// The edits that point the branch of the rebase at `HEAD` and `HEAD` back at the branch, which are none for a
    /// detached `HEAD`.
    pub branch_edits: Vec<RefEdit>,
    /// The content of `rewritten-list` if it isn't empty, which `git notes copy --for-rewrite=rebase` and the
    /// `post-rewrite` hook with the argument `rebase` read.
    pub rewritten: Option<Vec<u8>>,
    /// The edits that update the branches of `update-refs`, as `do_update_refs()` updates them.
    pub update_ref_edits: Vec<RefEdit>,
}

/// The state directory of a rebase, `rebase-merge` in a git directory.
#[derive(Debug, Clone)]
pub struct Dir {
    git_dir: PathBuf,
    path: PathBuf,
    object_hash: gix_hash::Kind,
}

/// Lifecycle and files
impl Dir {
    /// Return the state directory of a rebase in `git_dir`, the git directory of a worktree, in which objects have ids of
    /// `object_hash`.
    pub fn new(git_dir: impl Into<PathBuf>, object_hash: gix_hash::Kind) -> Self {
        let git_dir = git_dir.into();
        Dir {
            path: git_dir.join(DIR),
            git_dir,
            object_hash,
        }
    }

    /// Return the path of the directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the path of the file `name` in the directory, one of the [names](name).
    pub fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Return `true` if a rebase is in progress, as the directory exists.
    pub fn is_in_progress(&self) -> bool {
        self.path.is_dir()
    }

    /// Return `true` if the file `name` exists in the directory.
    pub fn has_file(&self, name: &str) -> bool {
        file::exists(&self.file(name))
    }

    /// Return the content of the file `name`, or `None` if it doesn't exist.
    pub fn read_file(&self, name: &str) -> Result<Option<Vec<u8>>> {
        file::read(&self.file(name))
    }

    /// Return the content of the file `name` without the line feed at its end and a carriage return before it, as
    /// `read_oneliner()` reads it, or `None` if it doesn't exist.
    pub fn read_line(&self, name: &str) -> Result<Option<BString>> {
        file::read_line(&self.file(name))
    }

    /// Replace the file `name` with `content`.
    pub fn write_file(&self, name: &str, content: &[u8]) -> Result<()> {
        file::write(&self.file(name), content)
    }

    /// Remove the file `name` if it exists.
    pub fn remove_file(&self, name: &str) -> Result<()> {
        file::remove(&self.file(name))
    }

    /// Remove the files of a stopped merge from the git directory, `MERGE_HEAD`, `MERGE_RR`, `MERGE_MSG`, `MERGE_MODE`
    /// and `SQUASH_MSG`, as `remove_branch_state()` removes them.
    pub fn remove_merge_files(&self) -> Result<()> {
        for name in MERGE_FILES {
            file::remove(&self.git_dir.join(name))?;
        }
        Ok(())
    }

    /// Remove the state of the rebase and return the edits that remove the references of its labels, as
    /// `sequencer_remove_state()` does when a rebase ends, is aborted or quits: each line of `refs-to-delete` names a
    /// reference, which goes with the reference log message `(rebase) cleanup`.
    pub fn remove(&self) -> Result<Vec<RefEdit>> {
        let mut edits = Vec::new();
        if let Some(content) = self.read_file(name::REFS_TO_DELETE)? {
            for line in content.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()) {
                let name = FullName::try_from(line.as_bstr())
                    .or_raise(|| message!("could not delete '{}'", line.as_bstr()))?;
                edits.push(refs::cleanup(name));
            }
        }
        file::remove_dir(&self.path)?;
        Ok(edits)
    }
}

/// The state
impl Dir {
    /// Create the directory and write `state` into it, as `init_basic_state()` and `write_basic_state()` do when a
    /// rebase starts. Git also removes the reference log of `REBASE_HEAD` then, which is the caller's to remove.
    pub fn start(&self, state: &State) -> Result<()> {
        file::create_dir(&self.path)?;
        self.write_file(name::INTERACTIVE, b"")?;
        let head_name = state
            .head_name
            .as_ref()
            .map_or(DETACHED_HEAD.as_bytes(), |name| name.as_bstr().as_bytes());
        self.write_file(name::HEAD_NAME, &line(head_name))?;
        self.write_file(name::ONTO, &hex_line(&state.onto))?;
        self.write_file(name::ORIG_HEAD, &hex_line(&state.orig_head))?;
        self.write_options(&state.options)?;
        if let Some(squash_onto) = &state.squash_onto {
            self.write_file(name::SQUASH_ONTO, &hex_line(squash_onto))?;
        }
        Ok(())
    }

    /// Read the state of the rebase, as `read_basic_state()` and `read_populate_opts()` read it for
    /// `git rebase --continue`.
    pub fn state(&self) -> Result<State> {
        let head_name = self.required_line(name::HEAD_NAME)?;
        let onto_text = self.required_line(name::ONTO)?;
        let head_name = if head_name.starts_with(b"refs/") {
            Some(FullName::try_from(head_name.as_bstr()).or_raise(|| message!("invalid head-name: '{}'", head_name))?)
        } else {
            None
        };
        let onto = file::id_at_start(&onto_text, self.object_hash)
            .ok_or_else(|| message!("invalid onto: '{}'", onto_text).corrupted())?;
        let orig_head_text = if self.has_file(name::ORIG_HEAD) {
            self.required_line(name::ORIG_HEAD)?
        } else {
            self.required_line("head")?
        };
        let orig_head = file::id_at_start(&orig_head_text, self.object_hash)
            .ok_or_else(|| message!("invalid orig-head: '{}'", orig_head_text).corrupted())?;
        let squash_onto = match self.read_line(name::SQUASH_ONTO)? {
            Some(text) => {
                Some(file::id(&text, self.object_hash).ok_or_else(|| message!("unusable squash-onto").corrupted())?)
            }
            None => None,
        };
        Ok(State {
            head_name,
            onto,
            orig_head,
            options: self.options()?,
            squash_onto,
        })
    }

    /// Write the files of `options`, as `write_basic_state()` writes them.
    fn write_options(&self, options: &Options) -> Result<()> {
        for (enabled, name) in [(options.quiet, name::QUIET), (options.verbose, name::VERBOSE)] {
            if enabled {
                self.write_file(name, b"")?;
            }
        }
        if let Some(strategy) = &options.strategy {
            self.write_file(name::STRATEGY, &line(strategy))?;
        }
        if !options.strategy_options.is_empty() {
            self.write_file(name::STRATEGY_OPTS, &line(&quote_cmdline(&options.strategy_options)))?;
        }
        match options.rerere_autoupdate {
            Some(true) => self.write_file(name::ALLOW_RERERE_AUTOUPDATE, b"--rerere-autoupdate\n")?,
            Some(false) => self.write_file(name::ALLOW_RERERE_AUTOUPDATE, b"--no-rerere-autoupdate\n")?,
            None => {}
        }
        if let Some(key) = &options.gpg_sign {
            let mut content = BString::from("-S");
            content.extend_from_slice(key);
            self.write_file(name::GPG_SIGN_OPT, &line(&content))?;
        }
        if options.signoff {
            self.write_file(name::SIGNOFF, b"--signoff\n")?;
        }
        for (enabled, name) in [
            (options.drop_redundant_commits, name::DROP_REDUNDANT_COMMITS),
            (options.keep_redundant_commits, name::KEEP_REDUNDANT_COMMITS),
            (options.committer_date_is_author_date, name::CDATE_IS_ADATE),
            (options.ignore_date, name::IGNORE_DATE),
        ] {
            if enabled {
                self.write_file(name, b"")?;
            }
        }
        let reschedule = if options.reschedule_failed_exec {
            name::RESCHEDULE_FAILED_EXEC
        } else {
            name::NO_RESCHEDULE_FAILED_EXEC
        };
        self.write_file(reschedule, b"")?;
        if !options.trailers.is_empty() {
            let mut content = BString::default();
            for trailer in &options.trailers {
                content.extend_from_slice(&line(trailer));
            }
            self.write_file(name::TRAILER, &content)?;
        }
        Ok(())
    }

    /// Read the options of the rebase, as `read_populate_opts()` reads them.
    pub fn options(&self) -> Result<Options> {
        let gpg_sign = self
            .read_line(name::GPG_SIGN_OPT)?
            .and_then(|text| text.strip_prefix(b"-S").map(BString::from));
        let rerere_autoupdate = match self
            .read_line(name::ALLOW_RERERE_AUTOUPDATE)?
            .as_ref()
            .map(|text| text.as_bytes())
        {
            Some(b"--rerere-autoupdate") => Some(true),
            Some(b"--no-rerere-autoupdate") => Some(false),
            _ => None,
        };
        let strategy = self.read_line(name::STRATEGY)?;
        let strategy_options = match (&strategy, self.read_line(name::STRATEGY_OPTS)?) {
            (Some(_), Some(text)) => split_cmdline(text.strip_prefix(b" ").unwrap_or(&text))
                .ok_or_else(|| message!("could not split '{}'", text).corrupted())?
                .into_iter()
                .map(|arg| arg.strip_prefix(b"--").map_or(arg.clone(), BString::from))
                .collect(),
            _ => Vec::new(),
        };
        let trailers = match self.read_file(name::TRAILER)? {
            Some(content) if content.is_empty() => bail!("trailers file is empty".corrupted()),
            Some(content) => {
                let mut trailers = Vec::new();
                let mut rest = content.as_slice();
                while let Some(end) = rest.find_byte(b'\n') {
                    if end == 0 {
                        bail!("trailers file contains empty line".corrupted());
                    }
                    trailers.push(BString::from(&rest[..end]));
                    rest = &rest[end + 1..];
                }
                trailers
            }
            None => Vec::new(),
        };
        Ok(Options {
            quiet: self.has_file(name::QUIET),
            verbose: self.has_file(name::VERBOSE),
            strategy,
            strategy_options,
            rerere_autoupdate,
            gpg_sign,
            signoff: self.has_file(name::SIGNOFF),
            drop_redundant_commits: self.has_file(name::DROP_REDUNDANT_COMMITS),
            keep_redundant_commits: self.has_file(name::KEEP_REDUNDANT_COMMITS),
            committer_date_is_author_date: self.has_file(name::CDATE_IS_ADATE),
            ignore_date: self.has_file(name::IGNORE_DATE),
            reschedule_failed_exec: self.has_file(name::RESCHEDULE_FAILED_EXEC),
            trailers,
        })
    }

    /// Return the line of the file `name`, which must exist.
    fn required_line(&self, name: &str) -> Result<BString> {
        let path = self.file(name);
        file::read_line(&path)?.ok_or_else(|| message!("could not read '{}'", path.display()).not_found().into())
    }
}

/// The todo list
impl Dir {
    /// Write `list`, the todo list that a rebase starts with once the user edited it, to `git-rebase-todo` with `format`,
    /// after the first `skip` lines, which go to `done` as fast-forwarded, and return the progress through it, as
    /// `complete_action()` and `skip_unnecessary_picks()` do.
    ///
    /// The progress holds each line with its full command and commit, as Git expands the list before it writes it, so
    /// that `git-rebase-todo` and `done` get the full commands once the rebase begins a command. `end` gets the number of
    /// all commands of `list`. If the first command after the skipped lines is a `fixup` or `squash`, the last skipped
    /// `pick` is recorded as rewritten, to be flushed with the commit that the chain makes.
    pub fn start_todo(&self, list: &List, skip: usize, format: Format<'_>) -> Result<Progress> {
        let mut expanded = List::from_items(list.items().cloned());
        let total = expanded.count_commands();
        let skipped = List {
            lines: expanded.lines.drain(..skip.min(expanded.lines.len())).collect(),
        };
        if !skipped.is_empty() {
            self.write_file(name::DONE, &skipped.to_bytes(Format::default())?)?;
            let next = expanded.peek_command(0);
            if next.is_some_and(Command::is_fixup)
                && let Some(base) = skipped
                    .instructions()
                    .filter(|instruction| instruction.command == Command::Pick)
                    .filter_map(|instruction| instruction.commit)
                    .last()
            {
                self.record_rewritten(base, next, None)?;
            }
        }
        self.write_file(name::TODO, &expanded.to_bytes(format)?)?;
        self.write_count(name::END, total)?;
        Ok(Progress {
            list: expanded,
            current: 0,
            done: skipped.lines.len(),
            total,
        })
    }

    /// Read the todo list of the rebase with comments that start with `comment`, finding its commits with `resolve`, and
    /// return the progress through it, as `read_populate_todo()` does for `git rebase --continue`: the commands of
    /// `done` count as begun, if Git can parse it, and `end` gets the number of all commands.
    pub fn progress(&self, comment: &BStr, resolve: &mut dyn Resolve) -> Result<Progress> {
        let path = self.file(name::TODO);
        let content = file::read(&path)?
            .ok_or_else(|| message!("could not open '{}' for reading", path.display()).not_found())?;
        let options = ParseOptions {
            comment,
            rebase: true,
            after_done: self.has_file(name::DONE),
        };
        let (list, errors) = List::parse(&content, options, resolve)?;
        if !errors.is_empty() {
            return Err(message!("please fix this using 'git rebase --edit-todo'.")
                .validation()
                .raise_all(errors)
                .into());
        }
        if list.is_empty() && !options.after_done {
            bail!("no commits parsed.".validation());
        }
        let done = match self.read_file(name::DONE)? {
            Some(content) if !content.is_empty() => match List::parse(&content, options, resolve)? {
                (done, errors) if errors.is_empty() => done.count_commands(),
                _ => 0,
            },
            _ => 0,
        };
        let total = done + list.count_commands();
        self.write_count(name::END, total)?;
        Ok(Progress {
            list,
            current: 0,
            done,
            total,
        })
    }

    /// Remove the files of the command at which the rebase stopped, `message`, `stopped-sha`, `amend` and `patch`, as
    /// `pick_commits()` does before it performs commands.
    pub fn clear_stop(&self) -> Result<()> {
        for name in [name::MESSAGE, name::STOPPED_SHA, name::AMEND, name::PATCH] {
            self.remove_file(name)?;
        }
        Ok(())
    }

    /// Begin the current command of `progress`, as each round of `pick_commits()` does, or return `None` if no command
    /// is left.
    ///
    /// `git-rebase-todo` gets the commands after it and `done` gets its line. Unless it is a comment, it counts as begun
    /// and `msgnum` gets its number. `author-script` and `MERGE_HEAD` in the git directory go, and the returned edits
    /// remove `AUTO_MERGE` and `REBASE_HEAD`. A `break` stops the rebase right away, with the current command still the
    /// `break`.
    pub fn begin(&self, progress: &mut Progress) -> Result<Option<Begun>> {
        let index = progress.current;
        let Some(is_comment) = progress
            .list
            .lines
            .get(index)
            .map(|line| matches!(line.item, Item::Comment(_)))
        else {
            return Ok(None);
        };
        self.save_todo(progress, false)?;
        if !is_comment {
            progress.done += 1;
            self.write_count(name::MSGNUM, progress.done)?;
        }
        self.remove_file(name::AUTHOR_SCRIPT)?;
        file::remove(&self.git_dir.join("MERGE_HEAD"))?;
        Ok(Some(Begun {
            index,
            edits: vec![refs::delete(refs::AUTO_MERGE)?, refs::delete(refs::REBASE_HEAD)?],
        }))
    }

    /// Put the current command of `progress` back at the start of `git-rebase-todo` as it failed in a way that lets it
    /// run again, as `pick_commits()` reschedules a command, and return the edit that points `REBASE_HEAD` at its
    /// commit, if it has one.
    pub fn reschedule(&self, progress: &Progress) -> Result<Vec<RefEdit>> {
        self.save_todo(progress, true)?;
        Ok(progress
            .list
            .lines
            .get(progress.current)
            .and_then(|line| line.item.instruction())
            .and_then(|instruction| instruction.commit)
            .map(refs::rebase_head)
            .into_iter()
            .collect())
    }

    /// Write the commands of `progress` from the current one on to `git-rebase-todo`, or from the one after it while
    /// appending the current one to `done` unless the command is rescheduled, as `save_todo()` does.
    fn save_todo(&self, progress: &Progress, reschedule: bool) -> Result<()> {
        let next = if reschedule {
            progress.current
        } else {
            progress.current + 1
        };
        let mut todo = BString::default();
        for line in progress.list.lines.iter().skip(next) {
            todo.extend_from_slice(&line.text);
        }
        self.write_file(name::TODO, &todo)?;
        if !reschedule && let Some(line) = progress.list.lines.get(progress.current) {
            file::append(&self.file(name::DONE), &line.text)?;
        }
        Ok(())
    }

    /// Write `count` with a line feed to the file `name`, as Git writes `msgnum` and `end`.
    fn write_count(&self, name: &str, count: usize) -> Result<()> {
        self.write_file(name, format!("{count}\n").as_bytes())
    }
}

/// What the commands record
impl Dir {
    /// Record that the command rewrote `commit`, as `record_in_rewritten()` does: it goes to `rewritten-pending`, which
    /// is flushed with `head` unless `next`, the next command that does something, is a `fixup` or `squash`.
    pub fn record_rewritten(&self, commit: ObjectId, next: Option<Command>, head: Option<ObjectId>) -> Result<()> {
        file::append(&self.file(name::REWRITTEN_PENDING), &hex_line(&commit))?;
        if !next.is_some_and(Command::is_fixup) {
            self.flush_rewritten_pending(head)?;
        }
        Ok(())
    }

    /// Append each commit of `rewritten-pending` with `head`, the commit made of it, to `rewritten-list` and remove
    /// `rewritten-pending`, as `flush_rewritten_pending()` does. Nothing happens if `head` is `None`, as Git can't
    /// read `HEAD` then, or if nothing is pending.
    pub fn flush_rewritten_pending(&self, head: Option<ObjectId>) -> Result<()> {
        let Some(head) = head else {
            return Ok(());
        };
        let Some(pending) = self
            .read_file(name::REWRITTEN_PENDING)?
            .filter(|pending| !pending.is_empty())
        else {
            return Ok(());
        };
        let mut out = BString::default();
        let mut rest = pending.as_slice();
        while !rest.is_empty() {
            let (line, next) = match rest.find_byte(b'\n') {
                Some(end) => (&rest[..end], &rest[end + 1..]),
                None => (rest, &[][..]),
            };
            out.extend_from_slice(line);
            out.push(b' ');
            out.extend_from_slice(&hex_line(&head));
            rest = next;
        }
        file::append(&self.file(name::REWRITTEN_LIST), &out)?;
        self.remove_file(name::REWRITTEN_PENDING)
    }

    /// Return the commits of `rewritten-list`, each with the commit that the rebase made of it.
    pub fn rewritten(&self) -> Result<Vec<(ObjectId, ObjectId)>> {
        let content = self.read_file(name::REWRITTEN_LIST)?.unwrap_or_default();
        content
            .lines()
            .map(|line| {
                line.split_once_str(" ")
                    .and_then(|(old, new)| Some((file::id(old, self.object_hash)?, file::id(new, self.object_hash)?)))
                    .ok_or_else(|| {
                        message!("invalid line in rewritten-list: '{}'", line.as_bstr())
                            .corrupted()
                            .into()
                    })
            })
            .collect()
    }

    /// Write the files of a command that stops the rebase as `stop` says, as `error_with_patch()`, `make_patch()` and
    /// `intend_to_amend()` write them, and return the edit that points `REBASE_HEAD` at the commit of the command.
    pub fn stop(&self, stop: &Stop<'_>) -> Result<Vec<RefEdit>> {
        let mut edits = Vec::new();
        if let Some(message) = stop.prepared_message
            && !self.has_file(name::MESSAGE)
        {
            self.write_file(name::MESSAGE, message)?;
        }
        if let Some(commit) = stop.commit {
            self.write_file(name::STOPPED_SHA, &hex_line(&commit))?;
            edits.push(refs::rebase_head(commit));
            if let Some(patch) = stop.patch {
                self.write_file(name::PATCH, patch)?;
            }
            if let Some(message) = stop.commit_message
                && !self.has_file(name::MESSAGE)
            {
                self.write_file(name::MESSAGE, &line(message))?;
            }
        }
        if let Some(amend) = &stop.amend {
            self.write_file(name::AMEND, &hex_line(amend))?;
        }
        Ok(edits)
    }

    /// Write the messages of a `fixup` or `squash` that couldn't apply its commit, as `error_failed_squash()` does before
    /// the rebase stops: `message` gets the content of `message-squash`, and so does `MERGE_MSG` in the git directory.
    pub fn write_failed_squash_messages(&self) -> Result<()> {
        let squash_message = self.read_file(name::SQUASH_MESSAGE)?.ok_or_else(|| {
            message!(
                "could not copy '{}' to '{}'",
                self.file(name::SQUASH_MESSAGE).display(),
                self.file(name::MESSAGE).display()
            )
            .not_found()
        })?;
        self.write_file(name::MESSAGE, &squash_message)?;
        let merge_message = self.git_dir.join("MERGE_MSG");
        file::remove(&merge_message)?;
        file::write(&merge_message, &squash_message)
    }

    /// Return the `fixup` and `squash` commands of the current chain from `current-fixups`, each with its commit.
    pub fn current_fixups(&self) -> Result<Vec<(Command, ObjectId)>> {
        let content = self.read_file(name::CURRENT_FIXUPS)?.unwrap_or_default();
        content
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                let parsed = line.split_once_str(" ").and_then(|(command, id)| {
                    let command = match command {
                        b"fixup" => Command::Fixup,
                        b"squash" => Command::Squash,
                        _ => return None,
                    };
                    Some((command, file::id(id, self.object_hash)?))
                });
                parsed.ok_or_else(|| {
                    message!("invalid line in current-fixups: '{}'", line.as_bstr())
                        .corrupted()
                        .into()
                })
            })
            .collect()
    }

    /// Add `command`, a `fixup` or `squash` of `commit`, to the current chain in `current-fixups`, as
    /// `update_squash_messages()` records it: the commands are separated by line feeds, and the last one has none.
    pub fn add_current_fixup(&self, command: Command, commit: ObjectId) -> Result<()> {
        let mut content = self
            .read_file(name::CURRENT_FIXUPS)?
            .map(BString::from)
            .unwrap_or_default();
        if !content.is_empty() {
            content.push(b'\n');
        }
        content.extend_from_slice(command.name().as_bytes());
        content.push(b' ');
        file::push_hex(&mut content, &commit);
        self.write_file(name::CURRENT_FIXUPS, &content)
    }

    /// Return the author of the commit that is being made, from `author-script`, or `None` if it doesn't exist.
    pub fn author_script(&self) -> Result<Option<AuthorScript>> {
        self.read_file(name::AUTHOR_SCRIPT)?
            .map(|content| AuthorScript::from_bytes(&content))
            .transpose()
    }

    /// Write `author` to `author-script`, as `write_author_script()` does.
    pub fn write_author_script(&self, author: &AuthorScript) -> Result<()> {
        self.write_file(name::AUTHOR_SCRIPT, &author.to_bytes())
    }

    /// Write `squash_onto`, the empty commit that a `reset [new root]` made, to `squash-onto` without a line feed, as
    /// `do_reset()` writes it.
    pub fn write_squash_onto(&self, squash_onto: ObjectId) -> Result<()> {
        let mut content = BString::default();
        file::push_hex(&mut content, &squash_onto);
        self.write_file(name::SQUASH_ONTO, &content)
    }

    /// Record `label`, whose reference a `label` command wrote, in `refs-to-delete`, as `do_label()` does once the
    /// reference of [`refs::label()`] is written.
    pub fn add_label(&self, label: &BStr) -> Result<()> {
        let name = refs::label_name(label)?;
        let mut content = self
            .read_file(name::REFS_TO_DELETE)?
            .map(BString::from)
            .unwrap_or_default();
        if !content.is_empty() && !content.ends_with(b"\n") {
            content.push(b'\n');
        }
        content.extend_from_slice(&line(name.as_bstr()));
        self.write_file(name::REFS_TO_DELETE, &content)
    }
}

/// The branches to update
impl Dir {
    /// Return the branches of `update-refs`, sorted by name, as `sequencer_get_update_refs_state()` reads them.
    pub fn update_refs(&self) -> Result<Vec<UpdateRef>> {
        let path = self.file(name::UPDATE_REFS);
        let Some(content) = file::read(&path)? else {
            return Ok(Vec::new());
        };
        let invalid = || message!("update-refs file at '{}' is invalid", path.display()).corrupted();
        let mut records: Vec<UpdateRef> = Vec::new();
        let mut lines = content.lines();
        while let Some(name) = lines.next() {
            let mut id = || {
                lines
                    .next()
                    .and_then(|hex| file::id_at_start(hex, self.object_hash))
                    .ok_or_else(invalid)
            };
            let (before, after) = (id()?, id()?);
            let record = UpdateRef {
                name: name.into(),
                before,
                after,
            };
            match records.binary_search_by(|existing| existing.name.cmp(&record.name)) {
                Ok(index) => records[index] = record,
                Err(index) => records.insert(index, record),
            }
        }
        Ok(records)
    }

    /// Write `records` to `update-refs` sorted by name, or remove it if there are none, as `write_update_refs_state()`
    /// does.
    pub fn write_update_refs(&self, records: &[UpdateRef]) -> Result<()> {
        if records.is_empty() {
            return self.remove_file(name::UPDATE_REFS);
        }
        let mut sorted: Vec<&UpdateRef> = records.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        let mut content = BString::default();
        for record in sorted {
            content.extend_from_slice(&line(&record.name));
            content.extend_from_slice(&hex_line(&record.before));
            content.extend_from_slice(&hex_line(&record.after));
        }
        self.write_file(name::UPDATE_REFS, &content)
    }

    /// Make `update-refs` agree with the `update-ref` commands of `list` after an edit of the todo list, as
    /// `todo_list_filter_update_refs()` does: a branch without a commit that no command names goes, and a branch that a
    /// command names is added with the commit that `current` returns for it, or the null id if it has none.
    pub fn filter_update_refs(&self, list: &List, current: &mut dyn FnMut(&BStr) -> Option<ObjectId>) -> Result<()> {
        let mut records = self.update_refs()?;
        let named: Vec<&BStr> = list
            .instructions()
            .filter(|instruction| instruction.command == Command::UpdateRef)
            .map(|instruction| instruction.arg.as_bstr())
            .collect();
        let before = records.len();
        records.retain(|record| !record.after.is_null() || named.contains(&record.name.as_bstr()));
        let mut updated = records.len() != before;
        for name in named {
            if !records.iter().any(|record| record.name == name) {
                records.push(UpdateRef {
                    name: name.into(),
                    before: current(name).unwrap_or_else(|| self.object_hash.null()),
                    after: self.object_hash.null(),
                });
                updated = true;
            }
        }
        if updated {
            self.write_update_refs(&records)?;
        }
        Ok(())
    }

    /// Record `head` as the commit for the branch `name` of an `update-ref` command, as `do_update_ref()` does.
    pub fn record_update_ref(&self, name: &BStr, head: ObjectId) -> Result<()> {
        let mut records = self.update_refs()?;
        if let Some(record) = records.iter_mut().find(|record| record.name == name) {
            record.after = head;
        }
        self.write_update_refs(&records)
    }
}

/// The steps of `git rebase`
impl Dir {
    /// Decide what `git rebase --continue` does with the staged changes of `changes` before it performs the next command
    /// of `progress`, as `commit_staged_changes()` does with `HEAD` at `head`.
    ///
    /// When the rebase stopped for amending, a `fixup` or `squash` chain is accounted for as Git does: `current-fixups`
    /// loses the command that stopped, or goes with `message-fixup` and `message-squash` if the chain ended by hand, and
    /// `message-squash` gets `head_message`, the message of `HEAD` from its subject on, if the chain goes on.
    pub fn prepare_continue(
        &self,
        state: &State,
        progress: &Progress,
        changes: Changes,
        head: Option<ObjectId>,
        head_message: Option<&[u8]>,
    ) -> Result<Continue> {
        if changes.unstaged {
            bail!("cannot rebase: You have unstaged changes.".validation());
        }
        let is_clean = !changes.uncommitted;
        if !is_clean && !self.has_file(name::MESSAGE) {
            let gpg_sign = gpg_sign_opt_quoted(&state.options);
            bail!(message!("{}", staged_changes_advice(gpg_sign.as_bstr())).validation());
        }
        let mut current_fixups = self.read_line(name::CURRENT_FIXUPS)?.unwrap_or_default();
        let mut fixup_count = if current_fixups.is_empty() {
            0
        } else {
            1 + current_fixups.find_iter(b"\n").count()
        };
        let (mut amend, mut edit_message, mut cleanup_message, mut final_fixup) = (false, true, false, false);
        if self.has_file(name::AMEND) {
            let head = head.ok_or_else(|| message!("cannot amend non-existing commit").validation())?;
            let path = self.file(name::AMEND);
            let text = self
                .read_line(name::AMEND)?
                .ok_or_else(|| message!("invalid file: '{}'", path.display()).corrupted())?;
            let to_amend = file::id_at_start(&text, self.object_hash)
                .ok_or_else(|| message!("invalid contents: '{}'", path.display()).corrupted())?;
            if !is_clean && head != to_amend {
                bail!(
                    "\nYou have uncommitted changes in your working tree. Please, commit them\nfirst and then run 'git rebase --continue' again."
                        .validation()
                );
            }
            let next_is_fixup = progress.peek_command(0).is_some_and(Command::is_fixup);
            if !is_clean || fixup_count == 0 {
                // This isn't the last command of a chain.
            } else if head != to_amend || !self.has_file(name::STOPPED_SHA) {
                // The chain ended by hand if no `fixup` or `squash` follows.
                if !next_is_fixup {
                    for name in [name::FIXUP_MESSAGE, name::SQUASH_MESSAGE, name::CURRENT_FIXUPS] {
                        self.remove_file(name)?;
                    }
                    current_fixups.clear();
                    fixup_count = 0;
                }
            } else {
                // The rebase stopped in a chain, at a command whose commit it skips.
                fixup_count -= 1;
                let kept = current_fixups.rfind_byte(b'\n').map_or(0, |end| end + 1);
                current_fixups.truncate(kept);
                self.write_file(name::CURRENT_FIXUPS, &current_fixups)?;
                if fixup_count > 0 && !next_is_fixup {
                    final_fixup = true;
                    if !current_fixups.starts_with(b"squash ") && current_fixups.find(b"\nsquash ").is_none() {
                        edit_message = false;
                        cleanup_message = true;
                    }
                } else if next_is_fixup {
                    let message = head_message.ok_or_else(|| message!("could not parse HEAD").not_found())?;
                    self.write_file(name::SQUASH_MESSAGE, message)?;
                }
            }
            amend = true;
        }
        let mut outcome = Continue {
            edits: Vec::new(),
            remove_merge_message: false,
            commit: None,
        };
        if is_clean {
            outcome.edits.push(refs::delete(refs::CHERRY_PICK_HEAD)?);
            outcome.remove_merge_message = true;
            if !final_fixup {
                return Ok(outcome);
            }
        }
        outcome.commit = Some(Commit {
            amend,
            edit_message,
            cleanup_message,
            message_file: (!final_fixup).then(|| self.file(name::MESSAGE)),
            final_fixup,
            remove_current_fixups: fixup_count > 0,
        });
        Ok(outcome)
    }

    /// Remove what `git rebase --continue` removes once it made `commit`, as `commit_staged_changes()` does: `amend`,
    /// `MERGE_HEAD` of the git directory, `message-fixup` and `message-squash` after the last command of a chain, and
    /// `current-fixups` if the chain had commands, and return the edit that removes `AUTO_MERGE`.
    pub fn committed(&self, commit: &Commit) -> Result<Vec<RefEdit>> {
        self.remove_file(name::AMEND)?;
        file::remove(&self.git_dir.join("MERGE_HEAD"))?;
        if commit.final_fixup {
            self.remove_file(name::FIXUP_MESSAGE)?;
            self.remove_file(name::SQUASH_MESSAGE)?;
        }
        if commit.remove_current_fixups {
            self.remove_file(name::CURRENT_FIXUPS)?;
        }
        Ok(vec![refs::delete(refs::AUTO_MERGE)?])
    }

    /// Record the commit at which the rebase stopped as rewritten, as `sequencer_continue()` does before it performs the
    /// next command of `progress`, if `stopped-sha` names one, with `HEAD` at `head`.
    pub fn continue_stopped(&self, progress: &Progress, head: Option<ObjectId>) -> Result<()> {
        if let Some(stopped) = self
            .read_line(name::STOPPED_SHA)?
            .and_then(|text| file::id_at_start(&text, self.object_hash))
        {
            self.record_rewritten(stopped, progress.peek_command(0), head)?;
        }
        Ok(())
    }

    /// Take the steps of `git rebase --skip` once the caller reset the index and the worktree to `HEAD` and cleared the
    /// recorded resolutions, as `builtin/rebase.c` does: the files of the stopped merge go, as `remove_branch_state()`
    /// removes them, and the returned edits remove its references. The rebase then continues as
    /// [`Dir::prepare_continue()`] says without staged changes.
    pub fn skip(&self) -> Result<Vec<RefEdit>> {
        self.remove_merge_files()?;
        Ok(refs::delete_branch_state())
    }

    /// Return the edits of `git rebase --abort` for `state`, as `builtin/rebase.c` makes them with the reference log
    /// action `reflog_action`: the branch of the rebase points at `orig-head` again and `HEAD` at the branch, or a
    /// detached `HEAD` points at `orig-head`, with the message `<action> (abort): returning to <branch or commit>`, and
    /// `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `AUTO_MERGE` and `REBASE_HEAD` go.
    ///
    /// The caller resets the index and the worktree to `orig-head` with the edits, then removes the files of the
    /// stopped merge with [`Dir::remove_merge_files()`] and the state with [`Dir::remove()`].
    pub fn abort(&self, state: &State, reflog_action: &BStr) -> Result<Vec<RefEdit>> {
        let mut target = BString::default();
        match &state.head_name {
            Some(name) => target.extend_from_slice(name.as_bstr()),
            None => file::push_hex(&mut target, &state.orig_head),
        }
        let text = format!("returning to {target}");
        let message = reflog_message(reflog_action, Some("abort"), Some(text.as_bytes()));
        let mut edits = match &state.head_name {
            Some(name) => vec![
                RefEdit::update(name.clone(), state.orig_head, PreviousValue::Any, message.clone()).with_deref(true),
                head_to_branch(name, message),
            ],
            None => vec![RefEdit::update(head_name(), state.orig_head, PreviousValue::Any, message).with_deref(true)],
        };
        edits.extend(refs::delete_branch_state());
        edits.push(refs::delete(refs::REBASE_HEAD)?);
        Ok(edits)
    }

    /// Take the steps of the end of the rebase of `state` with `HEAD` at `head`, as `pick_commits()` does once no command
    /// is left, with the reference log action `reflog_action`.
    ///
    /// The branch of the rebase gets `head` if it still points at `orig-head`, with the message
    /// `<action> (finish): <branch> onto <onto>`, and `HEAD` points at the branch again with
    /// `<action> (finish): returning to <branch>`. The pending rewritten commits are flushed with `head`. The caller
    /// applies the edits, runs the hooks with the rewritten commits, applies the edits of `update-refs`, and removes the
    /// state with [`Dir::remove()`].
    pub fn finish(&self, state: &State, head: ObjectId, reflog_action: &BStr) -> Result<Finish> {
        let mut branch_edits = Vec::new();
        if let Some(name) = &state.head_name {
            let mut text = BString::from(name.as_bstr().as_bytes());
            text.extend_from_slice(b" onto ");
            file::push_hex(&mut text, &state.onto);
            branch_edits.push(RefEdit::update(
                name.clone(),
                head,
                PreviousValue::MustExistAndMatch(Target::Object(state.orig_head)),
                reflog_message(reflog_action, Some("finish"), Some(text.as_bytes())),
            ));
            let mut text = BString::from("returning to ");
            text.extend_from_slice(name.as_bstr());
            branch_edits.push(head_to_branch(
                name,
                reflog_message(reflog_action, Some("finish"), Some(text.as_bytes())),
            ));
        }
        self.flush_rewritten_pending(Some(head))?;
        let rewritten = self
            .read_file(name::REWRITTEN_LIST)?
            .filter(|content| !content.is_empty());
        let mut update_ref_edits = Vec::new();
        for record in self.update_refs()? {
            let name = FullName::try_from(record.name.as_bstr())
                .or_raise(|| message!("could not update '{}'", record.name))?;
            update_ref_edits.push(refs::update_ref(name, record.before, record.after));
        }
        Ok(Finish {
            branch_edits,
            rewritten,
            update_ref_edits,
        })
    }
}

/// Return the reference log message `<action> (<sub_action>): <text>`, as `reflog_message()` makes it, where `action` is
/// `rebase` unless `GIT_REFLOG_ACTION` says otherwise. The part in parentheses or after the colon is left out if
/// `sub_action` or `text` are `None`.
pub fn reflog_message(action: &BStr, sub_action: Option<&str>, text: Option<&[u8]>) -> BString {
    let mut message = BString::from(action.as_bytes());
    if let Some(sub_action) = sub_action {
        message.extend_from_slice(b" (");
        message.extend_from_slice(sub_action.as_bytes());
        message.push(b')');
    }
    if let Some(text) = text {
        message.extend_from_slice(b": ");
        message.extend_from_slice(text);
    }
    message
}

/// Return the edit that points `HEAD` at the branch `name` with the reference log message `message`, as
/// `refs_update_symref()` does.
fn head_to_branch(name: &FullName, message: BString) -> RefEdit {
    RefEdit::update_with_log(
        head_name(),
        Target::Symbolic(name.clone()),
        PreviousValue::Any,
        LogChange {
            message,
            ..Default::default()
        },
    )
}

/// Return the name `HEAD`.
fn head_name() -> FullName {
    FullName::try_from("HEAD").expect("HEAD is a valid name")
}

/// Return `text` with a line feed after it, as `write_file()` completes a line.
fn line(text: &[u8]) -> BString {
    let mut out = BString::from(text);
    out.push(b'\n');
    out
}

/// Return the hexadecimal form of `id` with a line feed after it.
fn hex_line(id: &gix_hash::oid) -> BString {
    let mut out = BString::default();
    file::push_hex(&mut out, id);
    out.push(b'\n');
    out
}

/// Return `args` quoted for `split_cmdline()`, each in double quotes with a backslash before each double quote and
/// backslash, separated by spaces, as `quote_cmdline()` quotes them.
fn quote_cmdline(args: &[BString]) -> BString {
    let mut out = BString::default();
    for (index, arg) in args.iter().enumerate() {
        if index > 0 {
            out.push(b' ');
        }
        out.push(b'"');
        for byte in arg.iter() {
            if matches!(byte, b'"' | b'\\') {
                out.push(b'\\');
            }
            out.push(*byte);
        }
        out.push(b'"');
    }
    out
}

/// Split `cmdline` into arguments at unquoted whitespace, as `split_cmdline()` does: single and double quotes group,
/// and a backslash outside single quotes takes the next byte as it is. Return `None` for a trailing backslash or an
/// unclosed quote.
fn split_cmdline(cmdline: &[u8]) -> Option<Vec<BString>> {
    let mut args = vec![BString::default()];
    let mut quoted = None;
    let mut bytes = cmdline.iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        match quoted {
            None if crate::todo::is_space(byte) => {
                while bytes.next_if(|byte| crate::todo::is_space(*byte)).is_some() {}
                args.push(BString::default());
            }
            None if matches!(byte, b'\'' | b'"') => quoted = Some(byte),
            Some(quote) if quote == byte => quoted = None,
            _ => {
                let byte = if byte == b'\\' && quoted != Some(b'\'') {
                    bytes.next()?
                } else {
                    byte
                };
                args.last_mut()?.push(byte);
            }
        }
    }
    quoted.is_none().then_some(args)
}

/// Return the option `-S<key>` of `options` in single quotes as `gpg_sign_opt_quoted()` writes it for advice, or nothing.
fn gpg_sign_opt_quoted(options: &Options) -> BString {
    let Some(key) = &options.gpg_sign else {
        return BString::default();
    };
    let mut out = BString::from("'-S");
    for byte in key.iter() {
        if matches!(byte, b'\'' | b'!') {
            out.extend_from_slice(b"'\\");
            out.push(*byte);
            out.push(b'\'');
        } else {
            out.push(*byte);
        }
    }
    out.push(b'\'');
    out
}

/// Return the advice of `git rebase --continue` for staged changes without a message, as `staged_changes_advice` says.
fn staged_changes_advice(gpg_sign: &BStr) -> String {
    format!(
        "you have staged changes in your working tree\n\
If these changes are meant to be squashed into the previous commit, run:\n\
\n  git commit --amend {gpg_sign}\n\n\
If they are meant to go into a new commit, run:\n\n  git commit {gpg_sign}\n\n\
In both cases, once you're done, continue with:\n\n  git rebase --continue\n"
    )
}
