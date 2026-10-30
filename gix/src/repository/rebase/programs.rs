//! The programs that a rebase starts through `gix-command`: the editor of the todo list as `launch_sequence_editor()`
//! starts it, the editor of commit messages as `git commit` starts it with `launch_editor()`, and the commands of `exec`
//! lines as `do_exec()` runs them. They start as [`Repository::editor_command()`] starts the editor, in the root of the
//! worktree, where Git runs them.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
};

use super::Sequencer;
use crate::{
    Repository, Result,
    bstr::BStr,
    config::tree::Sequence,
    error::{bail, message},
    rebase::{Error, Programs},
};

/// What an editor did with the file it edited.
pub(super) enum Edited {
    /// The editor succeeded, or none ran as it is `:` or programs may not start.
    Done,
    /// The editor couldn't start or failed, with Git's message.
    Failed(String),
}

/// What the command of an `exec` line did, as `do_exec()` finds it.
pub(super) struct Executed {
    /// Whether the command succeeded, which it didn't if it couldn't start.
    pub success: bool,
    /// Whether the command left changes in the index or the worktree.
    pub dirty: bool,
}

impl Sequencer<'_> {
    /// Edit the message of a commit in the file at `path` with the editor of `git commit`, with the variables of `env`,
    /// as `launch_editor()` starts it, unless programs may not start.
    pub(super) fn edit_message(&self, path: &Path, env: &[(OsString, OsString)]) -> Result<Edited> {
        match self.programs {
            Programs::Deny => Ok(Edited::Done),
            Programs::Allow => edit(self.repo, self.repo.editor()?, path, env),
        }
    }

    /// Run `command_line` with the shell in the root of the worktree as `do_exec()` runs the command of an `exec` line,
    /// without `GIT_CHERRY_PICK_HELP`, and tell what it did.
    pub(super) fn exec(&self, command_line: &BStr) -> Result<Executed> {
        let command = gix_path::from_bstr(command_line)?.into_owned().into_os_string();
        let prepare = gix_command::prepare(command)
            .with_shell()
            .with_context(self.repo.command_context()?)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let mut command = std::process::Command::try_from(prepare)?;
        command
            .current_dir(worktree_root(self.repo)?)
            .env_remove("GIT_CHERRY_PICK_HELP");
        let success = command.status().is_ok_and(|status| status.success());
        let changes = self.worktree_changes()?;
        Ok(Executed {
            success,
            dirty: changes.unstaged || changes.uncommitted,
        })
    }
}

/// Fail unless the commands of `exec` lines may start in `repo` with `programs`: programs have to be allowed, and the
/// Git directory has to be fully trusted, as the todo list in it names the commands, which another owner of the
/// repository could have written, and as hooks only run in such repositories as well.
pub(super) fn allow_exec(repo: &Repository, programs: Programs) -> Result<()> {
    match programs {
        Programs::Deny => bail!(Error::ProgramsDenied),
        Programs::Allow if repo.git_dir_trust() != gix_sec::Trust::Full => bail!(Error::ProgramsUntrusted),
        Programs::Allow => Ok(()),
    }
}

/// Return the editor of the todo list of `repo` as `git_sequence_editor()` selects it: `sequence.editor`, which
/// `GIT_SEQUENCE_EDITOR` overrides, or else the [editor](Repository::editor()).
pub(super) fn sequence_editor(repo: &Repository) -> Result<Option<OsString>> {
    match repo.config_snapshot().program(Sequence::EDITOR)? {
        Some(editor) => Ok(Some(editor)),
        None => repo.editor(),
    }
}

/// Edit the file at `path` with `editor` and the variables of `env`, as `launch_specified_editor()` does: an editor of
/// `:` does nothing, and other editors start with the absolute path of the file as argument.
pub(super) fn edit(
    repo: &Repository,
    editor: Option<OsString>,
    path: &Path,
    env: &[(OsString, OsString)],
) -> Result<Edited> {
    let Some(editor) = editor else {
        return Ok(Edited::Failed("Terminal is dumb, but EDITOR unset".into()));
    };
    if editor == ":" {
        return Ok(Edited::Done);
    }
    let mut prepare = repo
        .prepare_editor(&editor)?
        .arg(repo.current_dir().join(path).into_os_string());
    for (name, value) in env {
        prepare = prepare.env(name, value);
    }
    let mut command = std::process::Command::try_from(prepare)?;
    command.current_dir(worktree_root(repo)?);
    let editor = editor.to_string_lossy().into_owned();
    Ok(match command.status() {
        Ok(status) if status.success() => Edited::Done,
        Ok(_) => Edited::Failed(format!("there was a problem with the editor '{editor}'")),
        Err(_) => Edited::Failed(format!("unable to start editor '{editor}'")),
    })
}

/// Return the root of the worktree of `repo`, where Git runs the programs of a rebase.
pub(super) fn worktree_root(repo: &Repository) -> Result<PathBuf> {
    let worktree = repo
        .workdir()
        .ok_or_else(|| message("The repository has no worktree").validation_error())?;
    Ok(repo.current_dir().join(worktree))
}
