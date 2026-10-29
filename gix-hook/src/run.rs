//! Run hooks one after another, starting their programs through `gix-command`, as Git's `run_hooks_opt()` does.

use std::{
    ffi::{OsStr, OsString},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
};

use bstr::ByteSlice;
use gix_error::{ErrorExt, OptionExt, Result, ResultExt, message};

use crate::{
    Hook, Hooks, Invocation,
    invocation::{Checkout, Commit, MessageSource, PushUpdate, RefUpdate, Rewrite, Stdin, Stdout, TransactionState},
};

/// The characters that make Git run a configured command with the shell, as listed in its `prepare_shell_cmd()`.
const SHELL_CHARACTERS: &[u8] = b"|&;<>()$`\\\"' \t\n*?[#~=%";

/// The outcome of running the hooks of an event.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The hooks that were started or failed to start, in the order they ran.
    pub hooks: Vec<Ran>,
}

/// A hook that was started or failed to start.
#[derive(Debug)]
pub struct Ran {
    /// The hook.
    pub hook: Hook,
    /// How it ended.
    pub status: Status,
}

/// How a hook ended.
#[derive(Debug)]
pub enum Status {
    /// The hook exited with this status.
    Exited(ExitStatus),
    /// The hook couldn't be started. Git counts this as exit code 1 and starts no further hook of the event.
    NotStarted(gix_error::Error),
}

impl Status {
    /// The code Git takes from this status, as its `wait_or_whine()` does: the exit code of the hook, 128 plus the number
    /// of the signal that ended it, or 1 if it couldn't be started. On Windows, only the lowest 8 bits of the exit code
    /// are kept, as in Git for Windows.
    pub fn code(&self) -> i32 {
        match self {
            Status::Exited(status) => exit_code(*status),
            Status::NotStarted(_) => 1,
        }
    }
}

#[cfg(unix)]
fn exit_code(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| signal + 128))
        .unwrap_or(-1)
}

#[cfg(not(unix))]
fn exit_code(status: ExitStatus) -> i32 {
    status.code().map_or(-1, |code| code & 0xff)
}

impl Outcome {
    /// The status of the hooks as Git combines it, the bitwise OR of their [codes](Status::code()), which is `0` if all
    /// hooks succeeded or none ran.
    pub fn code(&self) -> i32 {
        self.hooks.iter().fold(0, |code, ran| code | ran.status.code())
    }

    /// Return `true` if all hooks succeeded or none ran, which makes Git's combined [code](Outcome::code()) `0`.
    pub fn success(&self) -> bool {
        self.code() == 0
    }
}

impl Hooks {
    /// Run the hooks of the event of `invocation` one after another, in the order [`Hooks::find()`] finds them, as
    /// Git's `run_hooks_opt()` runs them, and return how each ended.
    ///
    /// Each hook runs in the [directory hooks run in](Hooks::working_dir()), or the one the invocation names, with the
    /// environment of this process, the [variables Git exports](Hooks::environment()) and those of the invocation,
    /// with its arguments and standard input, its standard output where the invocation sends it, and the standard
    /// error of this process. Hooks run one after another, as Git runs them unless `hook.jobs` lets it run them at the
    /// same time.
    ///
    /// - A program in the hooks directory is started directly. On Unix, a program the system can't execute, like a
    ///   script without `#!` line, runs with `/bin/sh`, as Git runs it. On Windows, a program whose name ends in
    ///   `.exe` is started directly, and a script with a `#!` line runs through the interpreter it names, as in Git
    ///   for Windows: the last component of its path, without arguments, found in `PATH` with `.exe` appended. As the
    ///   directories of Git's programs come first in `PATH` unless the environment of the hook sets `PATH`, shell
    ///   scripts run with Git's `sh`. The script receives its path with slashes, as Git for Windows writes it. Other
    ///   files fail to start, as they do in Git for Windows.
    /// - A configured command with a character special to the shell runs with Git's shell as
    ///   `sh -c '<command> "$@"' '<command>'` followed by the arguments of the hook, or as
    ///   `sh -c '<command>' '<command>'` if there are none, as Git's `prepare_shell_cmd()` runs it. Other commands are
    ///   looked up in `PATH` unless they contain a slash, or on Windows a backslash, and start like the programs in the
    ///   hooks directory.
    ///
    /// A hook that fails doesn't stop the others. A hook that can't be started is [recorded](Status::NotStarted) and
    /// stops the hooks after it, as in Git. Errors are returned for what makes Git stop as well: if the file of
    /// [`Stdin::File`] can't be opened, if the input can't be written for another reason than the hook not reading
    /// all of it, or if waiting for the hook fails.
    ///
    /// ### Deviation
    ///
    /// Hooks in the hooks directory are started with their absolute path, which they see in `$0`, where Git names them
    /// as it names its Git directory, often relative to its working directory. Git also exports `GIT_EXEC_PATH` and puts
    /// the directory of its programs in front of `PATH`, while here, only on Windows, `PATH` starts with the
    /// directories of Git's programs, as `gix-command` sets it.
    pub fn run(&self, invocation: &Invocation<'_>) -> Result<Outcome> {
        let mut outcome = Outcome::default();
        for hook in self.find(invocation.event).hooks {
            let status = self.run_hook(&hook, invocation)?;
            let not_started = matches!(status, Status::NotStarted(_));
            outcome.hooks.push(Ran { hook, status });
            if not_started {
                break;
            }
        }
        Ok(outcome)
    }

    fn run_hook(&self, hook: &Hook, invocation: &Invocation<'_>) -> Result<Status> {
        let env: Vec<_> = self.environment.iter().chain(&invocation.env).collect();
        let working_dir = invocation.working_dir.unwrap_or(&self.working_dir);
        let program = match Program::of(hook, &search_path(&env), working_dir, !invocation.args.is_empty()) {
            Ok(program) => program,
            Err(err) => return Ok(Status::NotStarted(err)),
        };
        let mut started = start(&program, invocation, working_dir, &env)?;
        if started.as_ref().is_err_and(is_exec_format_error) {
            started = start(&program.with_shell(), invocation, working_dir, &env)?;
        }
        let mut child = match started {
            Ok(child) => child,
            Err(err) => {
                let context = message("Could not start a hook").with_program(&program.program);
                return Ok(Status::NotStarted(err.and_raise(context)));
            }
        };
        let written = match &invocation.stdin {
            Stdin::Bytes(bytes) => write_input(child.stdin.take(), bytes),
            Stdin::Null | Stdin::File(_) => Ok(()),
        };
        let status = child.wait();
        written?;
        let status = status.or_raise(|| message("Could not wait for a hook").with_program(&program.program))?;
        Ok(Status::Exited(status))
    }
}

/// Methods to run the client-side hooks Git runs, with the arguments, environment and standard input Git passes, as
/// the constructors of [`Invocation`] describe them.
impl Hooks {
    /// Run `pre-commit` as [`Invocation::pre_commit()`] describes it.
    pub fn pre_commit(&self, commit: Commit<'_>) -> Result<Outcome> {
        self.run(&Invocation::pre_commit(commit))
    }

    /// Run `prepare-commit-msg` as [`Invocation::prepare_commit_msg()`] describes it.
    pub fn prepare_commit_msg(
        &self,
        commit: Commit<'_>,
        message_file: &Path,
        source: Option<MessageSource<'_>>,
    ) -> Result<Outcome> {
        self.run(&Invocation::prepare_commit_msg(commit, message_file, source))
    }

    /// Run `commit-msg` as [`Invocation::commit_msg()`] describes it.
    pub fn commit_msg(&self, commit: Commit<'_>, message_file: &Path) -> Result<Outcome> {
        self.run(&Invocation::commit_msg(commit, message_file))
    }

    /// Run `post-commit` as [`Invocation::post_commit()`] describes it.
    pub fn post_commit(&self, commit: Commit<'_>) -> Result<Outcome> {
        self.run(&Invocation::post_commit(commit))
    }

    /// Run `pre-merge-commit` as [`Invocation::pre_merge_commit()`] describes it.
    pub fn pre_merge_commit(&self, commit: Commit<'_>) -> Result<Outcome> {
        self.run(&Invocation::pre_merge_commit(commit))
    }

    /// Run `post-merge` as [`Invocation::post_merge()`] describes it.
    pub fn post_merge(&self, squash: bool) -> Result<Outcome> {
        self.run(&Invocation::post_merge(squash))
    }

    /// Run `pre-rebase` as [`Invocation::pre_rebase()`] describes it.
    pub fn pre_rebase(&self, upstream: &OsStr, branch: Option<&OsStr>) -> Result<Outcome> {
        self.run(&Invocation::pre_rebase(upstream, branch))
    }

    /// Run `post-rewrite` as [`Invocation::post_rewrite()`] describes it.
    pub fn post_rewrite<'a>(
        &self,
        command: Rewrite,
        rewritten: impl IntoIterator<Item = (&'a gix_hash::oid, &'a gix_hash::oid)>,
    ) -> Result<Outcome> {
        self.run(&Invocation::post_rewrite(command, rewritten))
    }

    /// Run `post-checkout` as [`Invocation::post_checkout()`] describes it.
    pub fn post_checkout(&self, previous: &gix_hash::oid, new: &gix_hash::oid, checkout: Checkout) -> Result<Outcome> {
        self.run(&Invocation::post_checkout(previous, new, checkout))
    }

    /// Run `post-checkout` for `git clone` as [`Invocation::post_checkout_after_clone()`] describes it.
    pub fn post_checkout_after_clone(&self, new: &gix_hash::oid, git_dir: &Path) -> Result<Outcome> {
        self.run(&Invocation::post_checkout_after_clone(new, git_dir))
    }

    /// Run `post-checkout` for `git worktree add` as [`Invocation::post_checkout_in_new_worktree()`] describes it.
    pub fn post_checkout_in_new_worktree(&self, new: &gix_hash::oid, worktree: &Path) -> Result<Outcome> {
        self.run(&Invocation::post_checkout_in_new_worktree(new, worktree))
    }

    /// Run `pre-push` as [`Invocation::pre_push()`] describes it.
    pub fn pre_push<'a>(
        &self,
        remote: &OsStr,
        url: &OsStr,
        updates: impl IntoIterator<Item = PushUpdate<'a>>,
    ) -> Result<Outcome> {
        self.run(&Invocation::pre_push(remote, url, updates))
    }

    /// Run `reference-transaction` as [`Invocation::reference_transaction()`] describes it.
    pub fn reference_transaction<'a>(
        &self,
        state: TransactionState,
        updates: impl IntoIterator<Item = RefUpdate<'a>>,
    ) -> Result<Outcome> {
        self.run(&Invocation::reference_transaction(state, updates))
    }
}

/// How a hook is started: `program`, with `leading_args` in front of the arguments of the hook.
#[derive(Debug, Clone)]
struct Program {
    program: PathBuf,
    leading_args: Vec<OsString>,
}

impl Program {
    /// Decide how `hook` is started, as Git's `prepare_shell_cmd()` and `prepare_cmd()` or Git for Windows'
    /// `mingw_spawnvpe()` decide it, looking up programs in `search_path`, the `PATH` hooks receive. Relative paths
    /// are taken from `working_dir`, where the hook runs. `has_args` is `true` if the hook receives arguments.
    fn of(hook: &Hook, search_path: &OsStr, working_dir: &Path, has_args: bool) -> Result<Self> {
        let command = match hook {
            Hook::Program { path } => return resolve(path, search_path, working_dir),
            Hook::Command { command, .. } => command,
        };
        let command_line = gix_path::from_bstr(command.as_bstr())?.into_owned().into_os_string();
        if command.find_byteset(SHELL_CHARACTERS).is_none() {
            return resolve(Path::new(&command_line), search_path, working_dir);
        }
        let mut script = command_line.clone();
        if has_args {
            script.push(" \"$@\"");
        }
        let mut program = shell(search_path, working_dir)?;
        program
            .leading_args
            .extend([OsString::from("-c"), script, command_line]);
        Ok(program)
    }

    /// Return how to run this program with Git's shell, as Git does on Unix if the system can't execute it.
    fn with_shell(&self) -> Self {
        Program {
            program: gix_path::env::shell().into(),
            leading_args: std::iter::once(self.program.clone().into_os_string())
                .chain(self.leading_args.iter().cloned())
                .collect(),
        }
    }
}

/// Return how to start Git's shell, as Git's `prepare_shell_cmd()` names it: `/bin/sh` on Unix, and `sh` looked up
/// in `search_path` on Windows.
fn shell(search_path: &OsStr, working_dir: &Path) -> Result<Program> {
    if cfg!(unix) {
        Ok(Program {
            program: gix_path::env::shell().into(),
            leading_args: Vec::new(),
        })
    } else {
        resolve(Path::new("sh"), search_path, working_dir)
    }
}

/// Start `program` for a hook of `invocation`, and return the error of starting it inside, or outside an error that
/// stops Git.
fn start(
    program: &Program,
    invocation: &Invocation<'_>,
    working_dir: &Path,
    env: &[&(OsString, Option<OsString>)],
) -> Result<std::io::Result<Child>> {
    let stdin = match &invocation.stdin {
        Stdin::Null => Stdio::null(),
        Stdin::Bytes(_) => Stdio::piped(),
        Stdin::File(path) => Stdio::from(
            std::fs::File::open(path).or_raise(|| message("Could not open the input of hooks").with("path", *path))?,
        ),
    };
    let prepare = gix_command::prepare(program.program.clone())
        .args(program.leading_args.iter().cloned())
        .args(invocation.args.iter().cloned());
    let mut cmd = match Command::try_from(prepare) {
        Ok(cmd) => cmd,
        Err(err) => return Ok(Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, err))),
    };
    cmd.current_dir(working_dir)
        .stdin(stdin)
        .stdout(match invocation.stdout {
            Stdout::Stderr => Stdio::from(std::io::stderr()),
            Stdout::Inherit => Stdio::inherit(),
        })
        .stderr(Stdio::inherit());
    for (name, value) in env {
        match value {
            Some(value) => cmd.env(name, value),
            None => cmd.env_remove(name),
        };
    }
    Ok(cmd.spawn())
}

/// Write `bytes` to the standard input of a hook and close it, ignoring that the hook doesn't read all of it, as Git
/// does.
fn write_input(stdin: Option<ChildStdin>, bytes: &[u8]) -> Result<()> {
    let Some(mut stdin) = stdin else {
        return Ok(());
    };
    match stdin.write_all(bytes) {
        Err(err) if err.kind() != std::io::ErrorKind::BrokenPipe => {
            Err(err.and_raise(message("Could not write the input of a hook")))
        }
        _ => Ok(()),
    }
}

/// Return `true` if the system couldn't execute a program, as one without `#!` line that isn't a binary, which Git then
/// runs with the shell. `ENOEXEC` is 8 on the Unix systems Git supports.
fn is_exec_format_error(err: &std::io::Error) -> bool {
    cfg!(unix) && err.raw_os_error() == Some(8)
}

/// Return the `PATH` hooks receive: the last one `env` sets, or else that of this process with the directories of Git's
/// programs in front, as `gix-command` passes it to the programs it starts.
fn search_path(env: &[&(OsString, Option<OsString>)]) -> OsString {
    if let Some((_, value)) = env.iter().rev().find(|(name, _)| is_path_variable(name)) {
        return value.clone().unwrap_or_default();
    }
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let dirs = gix_path::env::git_program_dirs();
    if dirs.is_empty() {
        return inherited;
    }
    let mut path = std::env::join_paths(dirs).unwrap_or_default();
    if !inherited.is_empty() {
        // Only Windows has these directories, and it separates the entries of `PATH` with `;`.
        path.push(";");
        path.push(inherited);
    }
    path
}

fn is_path_variable(name: &OsStr) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case("PATH")
    } else {
        name == "PATH"
    }
}

fn not_found(program: &Path) -> gix_error::Message {
    message("Could not find the program of a hook in PATH")
        .not_found()
        .with_program(program)
}

/// Find the program to start for `command` as Git's `prepare_cmd()` does: look it up in `search_path` unless it
/// contains a slash.
#[cfg(unix)]
fn resolve(command: &Path, search_path: &OsStr, working_dir: &Path) -> Result<Program> {
    let program = if command.as_os_str().as_encoded_bytes().contains(&b'/') {
        working_dir.join(command)
    } else {
        locate_in_path(command, search_path, working_dir).ok_or_raise(|| not_found(command))?
    };
    Ok(Program {
        program,
        leading_args: Vec::new(),
    })
}

/// Find `command` in `search_path` as Git's `locate_in_PATH()` does: the first regular file its owner may execute.
/// Relative entries, and an empty one, which stands for the current directory, are taken from `working_dir`, the
/// directory the hook runs in, where Git works when it runs hooks.
#[cfg(unix)]
fn locate_in_path(command: &Path, search_path: &OsStr, working_dir: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    if search_path.is_empty() {
        return None;
    }
    std::env::split_paths(search_path)
        .map(|dir| working_dir.join(dir).join(command))
        .find(|path| {
            std::fs::metadata(path)
                .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o100 != 0)
        })
}

/// Find the program to start for `command` as Git for Windows' `mingw_spawnvpe()` does: look it up in `search_path`
/// unless it contains a slash or backslash, and if it's a script, start the interpreter its `#!` line names with the
/// path of the script, written with slashes.
#[cfg(not(unix))]
fn resolve(command: &Path, search_path: &OsStr, working_dir: &Path) -> Result<Program> {
    let has_separator = command
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .any(|byte| matches!(byte, b'/' | b'\\'));
    let program = if has_separator {
        working_dir.join(command)
    } else {
        lookup(command.as_os_str(), search_path, working_dir, false).ok_or_raise(|| not_found(command))?
    };
    Ok(match interpreter(&program) {
        Some(name) => Program {
            program: lookup(&name, search_path, working_dir, true).ok_or_raise(|| {
                message("Could not find the interpreter of a hook in PATH")
                    .not_found()
                    .with_program(&name)
            })?,
            leading_args: vec![with_slashes(program)],
        },
        None => Program {
            program,
            leading_args: Vec::new(),
        },
    })
}

/// Look up `name` in the non-empty entries of `search_path` as Git for Windows' `path_lookup()` does: in each, first as
/// `name` with `.exe` appended unless `name` ends in `.exe`, then as `name`, if that's not a directory and `exe_only`
/// is `false` or `name` ends in `.exe`. Relative entries are taken from `working_dir`, the directory the hook runs in,
/// where Git works when it runs hooks.
#[cfg(not(unix))]
fn lookup(name: &OsStr, search_path: &OsStr, working_dir: &Path, exe_only: bool) -> Option<PathBuf> {
    let is_exe = has_exe_extension(name);
    std::env::split_paths(search_path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .find_map(|dir| {
            let path = working_dir.join(dir).join(name);
            if !is_exe {
                let mut with_exe = path.clone().into_os_string();
                with_exe.push(".exe");
                let with_exe = PathBuf::from(with_exe);
                if with_exe.exists() {
                    return Some(with_exe);
                }
            }
            ((!exe_only || is_exe) && path.exists() && !path.is_dir()).then_some(path)
        })
}

#[cfg(not(unix))]
fn has_exe_extension(name: &OsStr) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

/// Return `path` with slashes instead of backslashes, as Git for Windows names files, so that a script finds the files
/// next to it through the path it receives as `$0`.
#[cfg(not(unix))]
fn with_slashes(path: PathBuf) -> OsString {
    match path.to_str() {
        Some(path) => path.replace('\\', "/").into(),
        None => path.into_os_string(),
    }
}

/// Return the name of the interpreter of the script at `program` as Git for Windows' `parse_interpreter()` does: the
/// last component of the path on its `#!` line, without arguments, or `None` if `program` ends in `.exe` or doesn't
/// have such a line, ended within its first 99 bytes.
#[cfg(not(unix))]
fn interpreter(program: &Path) -> Option<OsString> {
    use std::io::Read;

    if has_exe_extension(program.as_os_str()) {
        return None;
    }
    let mut buf = [0; 99];
    let len = std::fs::File::open(program).ok()?.read(&mut buf).ok()?;
    let buf = &buf[..len];
    if len < 4 || !buf.starts_with(b"#!") {
        return None;
    }
    let end = buf.iter().position(|byte| matches!(byte, b'\r' | b'\n' | 0))?;
    if buf[end] == 0 {
        return None;
    }
    let line = &buf[2..end];
    let start = line.rfind_byte(b'/').or_else(|| line.rfind_byte(b'\\'))? + 1;
    let name = &line[start..];
    let name = name.find_byte(b' ').map_or(name, |space| &name[..space]);
    gix_path::from_byte_slice(name)
        .ok()
        .map(|name| name.as_os_str().to_owned())
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, path::Path};

    /// Git looks up the programs of hooks where they run, so a relative entry of `PATH` is taken from the directory
    /// hooks run in, wherever this process works, and the program that is found there is the one that starts.
    #[test]
    fn relative_path_entries_are_taken_from_the_directory_hooks_run_in() -> gix_testtools::Result {
        let dir = gix_testtools::tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join("bin"))?;
        let program = dir.path().join("bin").join(if cfg!(windows) {
            "relative-hook.exe"
        } else {
            "relative-hook"
        });
        std::fs::write(&program, b"")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))?;
        }
        assert!(
            !Path::new("bin").exists(),
            "the current directory of the test has no `bin` directory to find the program in"
        );
        let found = super::resolve(Path::new("relative-hook"), OsStr::new("bin"), dir.path())?;
        assert_eq!(
            found.program, program,
            "the program is found in the `bin` directory of the directory hooks run in"
        );
        Ok(())
    }
}
