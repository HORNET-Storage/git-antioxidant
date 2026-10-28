use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use gix_error::{Result, ResultExt, message};

use crate::{Config, Context, Found, Hook, Hooks};

impl Hooks {
    /// Prepare to find the hooks of the repository at `ctx`, with the hooks directory and the configured hooks of
    /// `config`.
    ///
    /// The hooks directory is `core.hooksPath`, interpolated as Git interpolates paths of its configuration, or else the
    /// `hooks` directory of the common Git directory, as Git's `repo_git_path()` makes it. A relative `core.hooksPath` is
    /// relative to the [directory hooks run in](Hooks::working_dir()), where Git works when it looks for hooks. If
    /// `core.hooksPath` is marked `:(optional)` and doesn't exist, the `hooks` directory is used, as Git's
    /// `git_config_pathname()` ignores such a value. Relative paths of `ctx` are made absolute with the current
    /// working directory of this process.
    ///
    /// The [environment](Hooks::environment()) of the hooks is the one Git exports for the repository.
    pub fn new(ctx: Context<'_>, config: Config<'_>) -> Result<Self> {
        let git_dir = absolute(ctx.git_dir)?;
        let worktree = ctx.worktree.map(absolute).transpose()?;
        let working_dir = worktree.clone().unwrap_or_else(|| git_dir.clone());
        let hooks_dir = match config.hooks_path {
            Some(value) => {
                let is_optional = value.is_optional;
                let path = value
                    .interpolate(config.interpolate)
                    .or_raise(|| message("Could not interpolate the value of core.hooksPath"))?;
                let path = working_dir.join(path);
                if is_optional && is_missing(&path) {
                    absolute(ctx.common_dir)?.join("hooks")
                } else {
                    path
                }
            }
            None => absolute(ctx.common_dir)?.join("hooks"),
        };
        let environment = exported_environment(&git_dir, worktree.as_deref())?;
        Ok(Hooks {
            hooks_dir,
            working_dir,
            environment,
            configured: config.configured,
        })
    }

    /// The directory in which the programs named after events are.
    pub fn hooks_dir(&self) -> &Path {
        &self.hooks_dir
    }

    /// The directory hooks run in: the root of the worktree, or the Git directory of a bare repository.
    pub fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    /// The environment variables Git exports for the hooks of the repository, each with its value, or `None` if it's
    /// removed. Hooks receive them in addition to the environment of this process and before those of an
    /// [`Invocation`](crate::Invocation).
    ///
    /// Git's `setup_git_directory()` exports `GIT_DIR` unless it found the `.git` directory of the worktree: the real
    /// path of the Git directory if the `.git` of the worktree is a file naming it, as in a linked worktree, with `/` as
    /// separator on Windows as well, and `.` in a bare repository, where it also exports `GIT_IMPLICIT_WORK_TREE=0`.
    /// It also exports `GIT_PREFIX`, the directory Git was started in relative to the root of the worktree, which is
    /// empty here. These are the values Git exports when it is started in the directory hooks run in.
    pub fn environment(&self) -> &[(OsString, Option<OsString>)] {
        &self.environment
    }

    /// Find the hooks of `event` in Git's order, as Git's `list_hooks()` does: the hooks the configuration names for
    /// the event, then the program named after the event in the hooks directory.
    ///
    /// Like Git's `find_hook()`, this looks for the program each time, so programs added or removed since are noticed.
    /// On Unix, a file without permission to execute it is [skipped](crate::Skipped::NotExecutable). On Windows,
    /// which has no such permission, any file named after the event is found, or else one named after it with `.exe`
    /// appended.
    pub fn find(&self, event: &str) -> Found {
        let mut found = Found::default();
        self.configured.find(event, &mut found);
        match find_program(&self.hooks_dir, event) {
            Program::Found(path) => found.hooks.push(Hook::Program { path }),
            #[cfg(unix)]
            Program::NotExecutable(path) => found.skipped.push(crate::Skipped::NotExecutable { path }),
            Program::Missing => {}
        }
        found
    }
}

/// What is named after an event in the hooks directory.
enum Program {
    Found(PathBuf),
    /// Only Unix has files that may not be executed.
    #[cfg(unix)]
    NotExecutable(PathBuf),
    Missing,
}

/// Find the program named `event` in `hooks_dir` as Git's `find_hook()` does with `access(X_OK)`.
///
/// `access()` tells whether this process may execute the file; any execute permission counts here instead. Like Git,
/// this finds a directory that may be searched, which then fails to start.
#[cfg(unix)]
fn find_program(hooks_dir: &Path, event: &str) -> Program {
    use std::os::unix::fs::PermissionsExt;

    let path = hooks_dir.join(event);
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.permissions().mode() & 0o111 != 0 => Program::Found(path),
        Ok(_) => Program::NotExecutable(path),
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => Program::NotExecutable(path),
        Err(_) => Program::Missing,
    }
}

/// Find the program named `event` in `hooks_dir` as Git for Windows' `find_hook()` does: its `access()` only checks
/// that the file exists, and then the name with `.exe` appended is tried.
#[cfg(not(unix))]
fn find_program(hooks_dir: &Path, event: &str) -> Program {
    let path = hooks_dir.join(event);
    if path.exists() {
        return Program::Found(path);
    }
    let path = hooks_dir.join(format!("{event}.exe"));
    if path.exists() {
        Program::Found(path)
    } else {
        Program::Missing
    }
}

/// Return the variables Git exports for hooks after it found the repository from the directory hooks run in.
fn exported_environment(git_dir: &Path, worktree: Option<&Path>) -> Result<Vec<(OsString, Option<OsString>)>> {
    let prefix = ("GIT_PREFIX".into(), Some(OsString::new()));
    let Some(worktree) = worktree else {
        return Ok(vec![
            ("GIT_DIR".into(), Some(".".into())),
            ("GIT_IMPLICIT_WORK_TREE".into(), Some("0".into())),
            prefix,
        ]);
    };
    let real_git_dir = gix_path::realpath(git_dir)?;
    if gix_path::realpath(worktree.join(".git"))? == real_git_dir {
        return Ok(vec![prefix]);
    }
    let value = gix_path::to_unix_separators_on_windows(gix_path::into_bstr(real_git_dir)?);
    let value = gix_path::from_bstr(value)?.into_owned().into_os_string();
    Ok(vec![("GIT_DIR".into(), Some(value)), prefix])
}

/// Return `true` if nothing is at `path`, as Git's `is_missing_file()` does.
fn is_missing(path: &Path) -> bool {
    matches!(std::fs::symlink_metadata(path), Err(err) if err.kind() == std::io::ErrorKind::NotFound)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).or_raise(|| message("Could not make a path absolute").with("path", path))
}
