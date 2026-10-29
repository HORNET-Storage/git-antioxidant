//! The state of a cherry-pick or revert sequence in `.git/sequencer`, read and written as `sequencer.c` does, and the
//! steps of the sequence through its todo list.
//!
//! A sequence keeps four files: `todo` holds the commands from the current one on, `head` the commit at which `HEAD`
//! was when the sequence started, `opts` its [options](Options) as Git configuration, and `abort-safety` the commit at
//! which `HEAD` was after the last command, which tells whether `--abort` may move `HEAD` back.
//!
//! The caller performs the commands, makes the commits and resets the worktree, and applies the reference edits that
//! the functions here return.

use std::path::{Path, PathBuf};

use bstr::{BStr, BString, ByteSlice};
use gix_error::{ErrorExt, Result, ResultExt, bail, message};
use gix_hash::ObjectId;
use gix_ref::transaction::RefEdit;

use crate::{
    file, refs,
    todo::{Command, Instruction, Item, Line, List, ParseOptions, Resolve},
};

/// The name of the directory of the state of a sequence in the git directory.
pub const DIR: &str = "sequencer";

/// The names of the files in the directory of a sequence, as `sequencer.c` names them.
pub mod name {
    /// The commands that the sequence has yet to perform, starting with the current one.
    pub const TODO: &str = "todo";
    /// The commit at which `HEAD` was when the sequence started.
    pub const HEAD: &str = "head";
    /// The options of the sequence, as Git configuration.
    pub const OPTS: &str = "opts";
    /// The commit at which `HEAD` was after the last command.
    pub const ABORT_SAFETY: &str = "abort-safety";
}

/// What a sequence does with its commits, as `enum replay_action` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// `git cherry-pick`, whose commands are `pick`.
    CherryPick,
    /// `git revert`, whose commands are `revert`.
    Revert,
}

impl Action {
    /// Return the name of the command that runs the sequence, as `action_name()` does.
    pub fn name(self) -> &'static str {
        match self {
            Action::CherryPick => "cherry-pick",
            Action::Revert => "revert",
        }
    }

    /// Return the command of the todo list of the sequence.
    pub fn command(self) -> Command {
        match self {
            Action::CherryPick => Command::Pick,
            Action::Revert => Command::Revert,
        }
    }

    /// Return the name of the reference to the commit that the stopped command applies, `CHERRY_PICK_HEAD` or
    /// `REVERT_HEAD`.
    pub fn head_ref(self) -> &'static str {
        match self {
            Action::CherryPick => refs::CHERRY_PICK_HEAD,
            Action::Revert => refs::REVERT_HEAD,
        }
    }
}

/// How the message of a commit is cleaned up, as `enum commit_msg_cleanup_mode` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cleanup {
    /// Remove leading and trailing empty lines and trailing whitespace, `whitespace`.
    Whitespace,
    /// Keep the message as it is, `verbatim`.
    Verbatim,
    /// Remove what follows the scissors line, and clean up like `whitespace`, `scissors`.
    Scissors,
    /// Clean up like `whitespace` and remove comments, `strip`.
    Strip,
}

impl Cleanup {
    /// Return the mode that `value` names as `get_cleanup_mode()` reads `options.default-msg-cleanup`, for which
    /// `default` is `strip`, or `None` for an invalid mode.
    pub fn from_config_value(value: &BStr) -> Option<Self> {
        Some(match value.as_bytes() {
            b"default" | b"strip" => Cleanup::Strip,
            b"verbatim" => Cleanup::Verbatim,
            b"whitespace" => Cleanup::Whitespace,
            b"scissors" => Cleanup::Scissors,
            _ => return None,
        })
    }

    /// Return the name of the mode, as `describe_cleanup_mode()` does.
    pub fn name(self) -> &'static str {
        match self {
            Cleanup::Whitespace => "whitespace",
            Cleanup::Verbatim => "verbatim",
            Cleanup::Scissors => "scissors",
            Cleanup::Strip => "strip",
        }
    }
}

/// The options of a sequence that `opts` keeps, as `save_opts()` writes them and `populate_opts_cb()` reads them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Options {
    /// `--no-commit`, as `options.no-commit`.
    pub no_commit: bool,
    /// `--edit` if `true` and `--no-edit` if `false`, as `options.edit`.
    pub edit: Option<bool>,
    /// `--allow-empty`, as `options.allow-empty`.
    pub allow_empty: bool,
    /// `--allow-empty-message`, as `options.allow-empty-message`.
    pub allow_empty_message: bool,
    /// `--empty=drop`, as `options.drop-redundant-commits`.
    pub drop_redundant_commits: bool,
    /// `--empty=keep` or `--keep-redundant-commits`, as `options.keep-redundant-commits`.
    pub keep_redundant_commits: bool,
    /// `--signoff`, as `options.signoff`.
    pub signoff: bool,
    /// `-x`, as `options.record-origin`.
    pub record_origin: bool,
    /// `--ff`, as `options.allow-ff`.
    pub allow_ff: bool,
    /// The parent of `--mainline` for merge commits, as `options.mainline`.
    pub mainline: Option<i32>,
    /// The merge strategy of `--strategy`, as `options.strategy`.
    pub strategy: Option<BString>,
    /// The key of `--gpg-sign`, which is empty for the default key, as `options.gpg-sign`.
    pub gpg_sign: Option<BString>,
    /// The options of `--strategy-option` for the merge strategy, as the values of `options.strategy-option`.
    pub strategy_options: Vec<BString>,
    /// `--rerere-autoupdate` if `true` and `--no-rerere-autoupdate` if `false`, as `options.allow-rerere-auto`.
    pub rerere_autoupdate: Option<bool>,
    /// The mode of `--cleanup`, as `options.default-msg-cleanup`.
    pub default_message_cleanup: Option<Cleanup>,
}

impl Options {
    /// Parse `content`, the content of `opts`, as `read_populate_opts()` reads it with `populate_opts_cb()`: each key
    /// must be one of the section `options`, booleans take only the words that Git takes for them, and `mainline` takes
    /// an integer.
    pub fn from_bytes(content: &[u8]) -> Result<Self> {
        let config = gix_config::File::from_bytes_no_includes(
            content,
            gix_config::file::Metadata::api(),
            gix_config::file::init::Options::default(),
        )
        .or_raise(|| message!("malformed options sheet"))?;
        let mut options = Options::default();
        for section in config.sections() {
            let header = section.header();
            let mut prefix = header.name().to_ascii_lowercase();
            if let Some(subsection) = header.subsection_name() {
                prefix.push(b'.');
                prefix.extend_from_slice(subsection);
            }
            for (key, value) in section.body() {
                let mut full_key = BString::from(prefix.clone());
                full_key.push(b'.');
                full_key.extend_from_slice(key.to_ascii_lowercase().as_bytes());
                options.set(full_key.as_bstr(), value.as_bstr())?;
            }
        }
        Ok(options)
    }

    /// Set the option of `key`, a key of the configuration in lower case, to `value`, as `populate_opts_cb()` does.
    fn set(&mut self, key: &BStr, value: &BStr) -> Result<()> {
        let invalid = || message!("invalid value for '{key}': '{value}'").corrupted();
        let boolean = || parse_maybe_bool_text(value).ok_or_else(invalid);
        match key.as_bytes() {
            b"options.no-commit" => self.no_commit = boolean()?,
            b"options.edit" => self.edit = Some(boolean()?),
            b"options.allow-empty" => self.allow_empty = boolean()?,
            b"options.allow-empty-message" => self.allow_empty_message = boolean()?,
            b"options.drop-redundant-commits" => self.drop_redundant_commits = boolean()?,
            b"options.keep-redundant-commits" => self.keep_redundant_commits = boolean()?,
            b"options.signoff" => self.signoff = boolean()?,
            b"options.record-origin" => self.record_origin = boolean()?,
            b"options.allow-ff" => self.allow_ff = boolean()?,
            b"options.mainline" => {
                let mainline = gix_config::Integer::try_from(value)
                    .ok()
                    .and_then(|integer| integer.to_decimal())
                    .and_then(|integer| i32::try_from(integer).ok())
                    .ok_or_else(|| message!("bad numeric config value '{value}' for '{key}'").corrupted())?;
                self.mainline = Some(mainline);
            }
            b"options.strategy" => self.strategy = Some(value.into()),
            b"options.gpg-sign" => self.gpg_sign = Some(value.into()),
            b"options.strategy-option" => self.strategy_options.push(value.into()),
            b"options.allow-rerere-auto" => self.rerere_autoupdate = Some(boolean()?),
            b"options.default-msg-cleanup" => {
                let cleanup = Cleanup::from_config_value(value)
                    .ok_or_else(|| message!("Invalid cleanup mode {value}").corrupted())?;
                self.default_message_cleanup = Some(cleanup);
            }
            _ => bail!(message!("invalid key: {key}").corrupted()),
        }
        Ok(())
    }

    /// Return the content of `opts` for these options as `save_opts()` writes it, each option that is set in the
    /// section `options`, or nothing if none is set, as Git then writes no file.
    pub fn to_bytes(&self) -> BString {
        let mut values: Vec<(&str, BString)> = Vec::new();
        if self.no_commit {
            values.push(("no-commit", "true".into()));
        }
        if let Some(edit) = self.edit {
            values.push(("edit", if edit { "true" } else { "false" }.into()));
        }
        for (enabled, key) in [
            (self.allow_empty, "allow-empty"),
            (self.allow_empty_message, "allow-empty-message"),
            (self.drop_redundant_commits, "drop-redundant-commits"),
            (self.keep_redundant_commits, "keep-redundant-commits"),
            (self.signoff, "signoff"),
            (self.record_origin, "record-origin"),
            (self.allow_ff, "allow-ff"),
        ] {
            if enabled {
                values.push((key, "true".into()));
            }
        }
        if let Some(mainline) = self.mainline.filter(|mainline| *mainline != 0) {
            values.push(("mainline", mainline.to_string().into()));
        }
        if let Some(strategy) = &self.strategy {
            values.push(("strategy", strategy.clone()));
        }
        if let Some(key) = &self.gpg_sign {
            values.push(("gpg-sign", key.clone()));
        }
        for option in &self.strategy_options {
            values.push(("strategy-option", option.clone()));
        }
        if let Some(rerere_autoupdate) = self.rerere_autoupdate {
            values.push((
                "allow-rerere-auto",
                if rerere_autoupdate { "true" } else { "false" }.into(),
            ));
        }
        if let Some(cleanup) = self.default_message_cleanup {
            values.push(("default-msg-cleanup", cleanup.name().into()));
        }
        let mut out = BString::default();
        if values.is_empty() {
            return out;
        }
        out.extend_from_slice(b"[options]\n");
        for (key, value) in values {
            write_pair(&mut out, key, &value);
        }
        out
    }
}

/// A todo list that a sequence performs, as `struct todo_list` holds it while `pick_commits()` goes through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// The commands of the sequence, from the one that was current when the list was read on.
    pub list: List,
    /// The index of the current command in `list`.
    pub current: usize,
}

/// What `git cherry-pick --continue` or `git revert --continue` does, as `sequencer_continue()` decides it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Continue {
    /// No sequence is in progress, only a single cherry-pick or revert that stopped: commit it, as
    /// `continue_single_pick()` does with `git commit`, which opens the editor as `edit` says or, if it is `None`, if
    /// standard input is a terminal.
    Single {
        /// Whether the commit opens the editor for its message, which it cleans up with `--cleanup=strip` otherwise.
        edit: Option<bool>,
    },
    /// Perform the commands of `progress` from its current one on, after committing the stopped command first as
    /// `continue_single_pick()` does if `commit` is `true`. The caller fails with `error_dirty_index()` if the index
    /// differs from `HEAD` then.
    Sequence {
        /// Whether the stopped command is to be committed first.
        commit: bool,
        /// Whether that commit opens the editor, as for [`Continue::Single`].
        edit: Option<bool>,
        /// The commands to perform, with the stopped one done.
        progress: Progress,
    },
}

/// What `git cherry-pick --abort` or `git revert --abort` does, as `sequencer_rollback()` decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rollback {
    /// No sequence is in progress, only a single cherry-pick or revert: reset with `git reset --merge` to `HEAD`, as
    /// `rollback_single_pick()` does.
    Single,
    /// Reset with `git reset --merge` to the commit at which the sequence started, then remove the state with
    /// [`Dir::remove()`].
    ResetTo(ObjectId),
    /// `HEAD` moved since the last command, so it stays where it is, as Git warns with `You seem to have moved HEAD.
    /// Not rewinding, check your HEAD!`, and only the state is removed with [`Dir::remove()`].
    Moved,
}

/// The state directory of a sequence, `sequencer` in a git directory.
#[derive(Debug, Clone)]
pub struct Dir {
    git_dir: PathBuf,
    path: PathBuf,
    object_hash: gix_hash::Kind,
}

/// Lifecycle and files
impl Dir {
    /// Return the state directory of a sequence in `git_dir`, the git directory of a worktree, in which objects have ids
    /// of `object_hash`.
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

    /// Return `true` if a sequence is in progress, as the directory exists.
    pub fn is_in_progress(&self) -> bool {
        self.path.is_dir()
    }

    /// Remove the state of the sequence, as `sequencer_remove_state()` does when it ends or quits.
    pub fn remove(&self) -> Result<()> {
        file::remove_dir(&self.path)
    }

    /// Return the action of the sequence in progress from the first command of `todo`, as
    /// `sequencer_get_last_command()` does, or `None` if there is no `todo` or it doesn't start with `pick` or `revert`.
    pub fn last_action(&self) -> Result<Option<Action>> {
        let Some(content) = file::read(&self.file(name::TODO))? else {
            return Ok(None);
        };
        let start = content
            .iter()
            .position(|byte| !crate::todo::is_space(*byte))
            .unwrap_or(content.len());
        let line = &content[start..];
        for action in [Action::CherryPick, Action::Revert] {
            let command = action.command();
            let rest = line.strip_prefix(command.name().as_bytes()).or_else(|| {
                let letter = command.abbreviation()?;
                line.strip_prefix(&[letter][..])
            });
            if rest.is_some_and(|rest| matches!(rest.first(), Some(b' ' | b'\t'))) {
                return Ok(Some(action));
            }
        }
        Ok(None)
    }
}

/// The state
impl Dir {
    /// Start a sequence of `action` with `options` and `HEAD` at `head`, as `sequencer_pick_revisions()` does once it has
    /// the commits: fail if a sequence is in progress, as `create_seq_dir()` does with the advice for `picking`, whether
    /// `CHERRY_PICK_HEAD` or `REVERT_HEAD` exists, then write `head`, `opts` if an option is set, and `abort-safety`.
    /// `todo` is written when the first command begins.
    pub fn start(&self, action: Action, head: Option<ObjectId>, options: &Options, picking: bool) -> Result<()> {
        if let Some(in_progress) = self.last_action()? {
            let skip = if picking { "--skip | " } else { "" };
            bail!(
                message!(
                    "{} is already in progress\nhint: try \"git {} (--continue | {skip}--abort | --quit)\"",
                    in_progress.name(),
                    in_progress.name()
                )
                .conflict()
            );
        }
        if action == Action::Revert && head.is_none() {
            bail!("can't revert as initial commit".validation());
        }
        std::fs::create_dir(&self.path)
            .or_raise(|| message!("could not create sequencer directory '{}'", self.path.display()))?;
        let head = head.unwrap_or_else(|| self.object_hash.null());
        file::write(&self.file(name::HEAD), &hex_line(&head))?;
        let opts = options.to_bytes();
        if !opts.is_empty() {
            file::write(&self.file(name::OPTS), &opts)?;
        }
        self.update_abort_safety(Some(head).filter(|head| !head.is_null()))
    }

    /// Return the commit at which `HEAD` was when the sequence started, from `head`, as `sequencer_rollback()` reads it.
    pub fn head(&self) -> Result<ObjectId> {
        let path = self.file(name::HEAD);
        let content = file::read(&path)?.ok_or_else(|| message!("cannot open '{}'", path.display()).not_found())?;
        let Some(line) = content.lines_with_terminator().next() else {
            bail!(message!("cannot read '{}': unexpected end of file", path.display()).corrupted());
        };
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        file::id(line, self.object_hash).ok_or_else(|| {
            message!("stored pre-cherry-pick HEAD file '{}' is corrupt", path.display())
                .corrupted()
                .into()
        })
    }

    /// Return the options of the sequence from `opts`, or the default options if it doesn't exist, as
    /// `read_populate_opts()` does.
    pub fn options(&self) -> Result<Options> {
        let path = self.file(name::OPTS);
        match file::read(&path)? {
            Some(content) => {
                Options::from_bytes(&content).or_raise(|| message!("malformed options sheet: '{}'", path.display()))
            }
            None => Ok(Options::default()),
        }
    }

    /// Write `head`, the commit at which `HEAD` is after a command, to `abort-safety`, or nothing for an unborn `HEAD`,
    /// as `update_abort_safety_file()` does if a sequence is in progress.
    pub fn update_abort_safety(&self, head: Option<ObjectId>) -> Result<()> {
        if !file::exists(&self.path) {
            return Ok(());
        }
        let content = head.map(|head| hex_line(&head)).unwrap_or_default();
        file::write(&self.file(name::ABORT_SAFETY), &content)
    }

    /// Return `true` if `head`, the commit at which `HEAD` is, is still the commit of `abort-safety`, so that `--abort`
    /// and `--skip` may move `HEAD`, as `rollback_is_safe()` tells. A missing `abort-safety` stands for an unborn `HEAD`.
    pub fn rollback_is_safe(&self, head: Option<ObjectId>) -> Result<bool> {
        let path = self.file(name::ABORT_SAFETY);
        let expected = match file::read(&path)? {
            Some(content) => Some(
                file::id_at_start(content.trim(), self.object_hash)
                    .ok_or_else(|| message!("could not parse {}", path.display()).corrupted())?,
            ),
            None => None,
        };
        Ok(expected == head)
    }
}

/// The todo list
impl Dir {
    /// Read the todo list of a sequence of `action` with comments that start with `comment`, finding its commits with
    /// `resolve`, as `read_populate_todo()` does: each command must be the command of `action`.
    pub fn progress(&self, action: Action, comment: &BStr, resolve: &mut dyn Resolve) -> Result<Progress> {
        let path = self.file(name::TODO);
        let content = file::read(&path)?
            .ok_or_else(|| message!("could not open '{}' for reading", path.display()).not_found())?;
        let options = ParseOptions {
            comment,
            rebase: false,
            after_done: file::exists(&self.git_dir.join(crate::rebase::DIR).join(crate::rebase::name::DONE)),
        };
        let (list, errors) = List::parse(&content, options, resolve)?;
        if !errors.is_empty() {
            return Err(message!("unusable instruction sheet: '{}'", path.display())
                .corrupted()
                .raise_all(errors)
                .into());
        }
        if list.is_empty() {
            bail!("no commits parsed.".corrupted());
        }
        if list.items().any(|item| item.command() != Some(action.command())) {
            match action {
                Action::CherryPick => bail!("cannot cherry-pick during a revert.".conflict()),
                Action::Revert => bail!("cannot revert during a cherry-pick.".conflict()),
            }
        }
        Ok(Progress { list, current: 0 })
    }

    /// Begin the current command of `progress`, as each round of `pick_commits()` does for a sequence, writing the
    /// commands from it on to `todo`, and return its index, or `None` if no command is left. The caller writes
    /// `abort-safety` with [`Dir::update_abort_safety()`] after it performed the command, and removes the state with
    /// [`Dir::remove()`] once no command is left.
    pub fn begin(&self, progress: &Progress) -> Result<Option<usize>> {
        if progress.current >= progress.list.lines.len() {
            return Ok(None);
        }
        let mut todo = BString::default();
        for line in progress.list.lines.iter().skip(progress.current) {
            todo.extend_from_slice(&line.text);
        }
        file::write(&self.file(name::TODO), &todo)?;
        Ok(Some(progress.current))
    }
}

/// The steps of a sequence
impl Dir {
    /// Decide what `--continue` does for a sequence of `action`, as `sequencer_continue()` does, where `picking` tells
    /// whether `CHERRY_PICK_HEAD` or `REVERT_HEAD` exists and the todo list is read as [`Dir::progress()`] reads it.
    pub fn prepare_continue(
        &self,
        action: Action,
        picking: bool,
        comment: &BStr,
        resolve: &mut dyn Resolve,
    ) -> Result<Continue> {
        let options = self.options()?;
        if !file::exists(&self.file(name::TODO)) {
            if !picking {
                bail!("no cherry-pick or revert in progress".validation());
            }
            return Ok(Continue::Single { edit: options.edit });
        }
        let mut progress = self.progress(action, comment, resolve)?;
        progress.current += 1;
        Ok(Continue::Sequence {
            commit: picking,
            edit: options.edit,
            progress,
        })
    }

    /// Decide whether `--skip` may skip the stopped command of a sequence of `action`, as `sequencer_skip()` does with
    /// `HEAD` at `head`, where `picking` tells whether the reference of [`Action::head_ref()`] exists.
    ///
    /// If it may, the caller resets with `git reset --merge` to `HEAD`, as `skip_single_pick()` does, and continues
    /// with [`Dir::prepare_continue()`] if a sequence is in progress.
    pub fn skip(&self, action: Action, picking: bool, head: Option<ObjectId>) -> Result<()> {
        if !picking {
            if self.last_action()? != Some(action) {
                match action {
                    Action::CherryPick => bail!("no cherry-pick in progress".validation()),
                    Action::Revert => bail!("no revert in progress".validation()),
                }
            }
            if !self.rollback_is_safe(head)? {
                bail!(
                    message!(
                        "there is nothing to skip\nhint: have you committed already?\nhint: try \"git {} --continue\"",
                        action.name()
                    )
                    .validation()
                );
            }
        }
        Ok(())
    }

    /// Decide what `--abort` does, as `sequencer_rollback()` does with `HEAD` at `head`, where `picking` tells whether
    /// `CHERRY_PICK_HEAD` or `REVERT_HEAD` exists.
    pub fn rollback(&self, picking: bool, head: Option<ObjectId>) -> Result<Rollback> {
        if !file::exists(&self.file(name::HEAD)) {
            if !picking {
                bail!("no cherry-pick or revert in progress".validation());
            }
            return match head {
                None => bail!("cannot resolve HEAD".validation()),
                Some(head) if head.is_null() => bail!("cannot abort from a branch yet to be born".validation()),
                Some(_) => Ok(Rollback::Single),
            };
        }
        let start = self.head()?;
        if start.is_null() {
            bail!("cannot abort from a branch yet to be born".validation());
        }
        Ok(if self.rollback_is_safe(head)? {
            Rollback::ResetTo(start)
        } else {
            Rollback::Moved
        })
    }

    /// Return the edits that `git commit` applies when it commits a stopped cherry-pick or revert, as
    /// `sequencer_post_commit_cleanup()` makes them, and remove the state of the sequence if its stopped command was
    /// its last. `cherry_pick_head` and `revert_head` tell whether those references exist.
    pub fn post_commit_cleanup(&self, cherry_pick_head: bool, revert_head: bool) -> Result<Vec<RefEdit>> {
        let mut edits = Vec::new();
        for (exists, name) in [
            (cherry_pick_head, refs::CHERRY_PICK_HEAD),
            (revert_head, refs::REVERT_HEAD),
        ] {
            if exists {
                edits.push(refs::delete(name)?);
            }
        }
        edits.push(refs::delete(refs::AUTO_MERGE)?);
        if (cherry_pick_head || revert_head) && self.has_finished_the_last_pick()? {
            self.remove()?;
        }
        Ok(edits)
    }

    /// Return `true` if `todo` holds only the stopped command, as `have_finished_the_last_pick()` tells.
    fn has_finished_the_last_pick(&self) -> Result<bool> {
        let Some(content) = file::read(&self.file(name::TODO))? else {
            return Ok(false);
        };
        Ok(content.find_byte(b'\n').is_none_or(|end| end + 1 == content.len()))
    }
}

/// Return the line of `todo` for `commit` in a sequence of `action`, as `walk_revs_populate_todo()` writes it: the
/// command, `short_id`, the abbreviated id of the commit, and `subject`, the subject of the commit, separated by spaces.
pub fn todo_line(action: Action, commit: ObjectId, short_id: &BStr, subject: &BStr) -> Line {
    let mut text = BString::from(action.command().name());
    text.push(b' ');
    text.extend_from_slice(short_id);
    text.push(b' ');
    text.extend_from_slice(subject);
    text.push(b'\n');
    Line {
        item: Item::Command(Instruction::new(action.command(), Some(commit), subject)),
        text,
    }
}

/// Return the meaning of `value` as a boolean as `git_parse_maybe_bool_text()` reads it, or `None` if it isn't one.
fn parse_maybe_bool_text(value: &BStr) -> Option<bool> {
    let value = value.to_ascii_lowercase();
    match value.as_slice() {
        b"true" | b"yes" | b"on" => Some(true),
        b"false" | b"no" | b"off" | b"" => Some(false),
        _ => None,
    }
}

/// Append `key = value` to `out` as a line of a section, as `write_pair()` writes it: a value with a space at either
/// end or with `;`, `#` or a carriage return is quoted, and line feeds, tabs, double quotes and backslashes are
/// escaped.
fn write_pair(out: &mut BString, key: &str, value: &[u8]) {
    let quote = value.first() == Some(&b' ')
        || value.last() == Some(&b' ')
        || value.iter().any(|byte| matches!(byte, b';' | b'#' | b'\r'));
    out.push(b'\t');
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(b" = ");
    if quote {
        out.push(b'"');
    }
    for byte in value {
        match byte {
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'"' | b'\\' => {
                out.push(b'\\');
                out.push(*byte);
            }
            _ => out.push(*byte),
        }
    }
    if quote {
        out.push(b'"');
    }
    out.push(b'\n');
}

/// Return the hexadecimal form of `id` with a line feed after it.
fn hex_line(id: &gix_hash::oid) -> BString {
    let mut out = BString::default();
    file::push_hex(&mut out, id);
    out.push(b'\n');
    out
}
