//! Find and run the hooks of a Git repository, as Git does.
//!
//! Git runs hooks at certain points of its commands: the program named after the event, like `pre-commit`, in the hooks
//! directory, and the commands the configuration names for the event. This crate finds them as Git 2.55 does in
//! `hook.c`, and runs them as its `run_hooks_opt()` does.
//!
//! - [`Hooks`] knows where a repository keeps its hooks and which hooks its configuration names, and
//!   [`Hooks::find()`] lists the hooks of an event in Git's order: the [configured hooks](config::Configured) first,
//!   then the program in the hooks directory.
//! - [`Invocation`] is what Git passes to the hooks of an event: their arguments, environment variables and standard
//!   input, where their standard output goes and where they run. It has a constructor for each client-side hook Git
//!   runs, with Git's arguments and standard input, whose documentation says what Git does when the hook fails. Whether
//!   a failing hook stops an operation is the caller's decision.
//! - With the `run` feature, `Hooks::run()` runs the hooks of an [`Invocation`] one after another, starting their
//!   programs through `gix-command`, and [`Hooks`] has a method to run each of these client-side hooks.
//!
//! The caller supplies the paths of the repository, the configuration values and the environment that hooks receive.
//!
//! ## Feature Flags
//!
//! - `run`: run hooks, starting their programs through `gix-command`. Without it, hooks are found and invocations
//!   prepared, but no program is started.
//! - `sha1`, `sha256`: enable these hashes in `gix-hash`, one of which is needed unless another crate enables it.
#![deny(missing_docs)]
#![forbid(unsafe_code)]

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use bstr::BString;

pub mod config;
mod find;
pub mod invocation;
pub use invocation::Invocation;
#[cfg(feature = "run")]
pub mod run;

/// The hook events that Git 2.55 knows, as `githooks(5)` describes them, in the order of Git's `hook_name_list`.
///
/// Git refuses configured hooks named like one of these, see [`config::Configured`].
pub const KNOWN_EVENTS: &[&str] = &[
    "applypatch-msg",
    "commit-msg",
    "fsmonitor-watchman",
    "p4-changelist",
    "p4-post-changelist",
    "p4-pre-submit",
    "p4-prepare-changelist",
    "post-applypatch",
    "post-checkout",
    "post-commit",
    "post-index-change",
    "post-merge",
    "post-receive",
    "post-rewrite",
    "post-update",
    "pre-applypatch",
    "pre-auto-gc",
    "pre-commit",
    "pre-merge-commit",
    "pre-push",
    "pre-rebase",
    "pre-receive",
    "prepare-commit-msg",
    "proc-receive",
    "push-to-checkout",
    "reference-transaction",
    "sendemail-validate",
    "update",
];

/// Return `true` if `name` is one of the [`KNOWN_EVENTS`].
pub fn is_known_event(name: impl AsRef<[u8]>) -> bool {
    let name = name.as_ref();
    KNOWN_EVENTS.iter().any(|event| event.as_bytes() == name)
}

/// Where a repository keeps the directories its hooks depend on, as known from opening it.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// The Git directory of the worktree the hooks run for: the `.git` directory of the main worktree, the directory
    /// in `.git/worktrees` of a linked worktree, or the repository itself if it's bare.
    pub git_dir: &'a Path,
    /// The Git directory that all worktrees share, whose `hooks` directory holds the hooks unless `core.hooksPath` is
    /// set. It's `git_dir`, except in a linked worktree, where it's the Git directory of the main worktree.
    pub common_dir: &'a Path,
    /// The root of the worktree, or `None` if the repository is bare.
    pub worktree: Option<&'a Path>,
}

/// The configuration values that decide which hooks run.
#[derive(Clone, Default)]
pub struct Config<'a> {
    /// The value of `core.hooksPath`, or `None` if it isn't set.
    pub hooks_path: Option<gix_config_value::Path>,
    /// How to interpolate `hooks_path`, which Git interpolates like other paths of its configuration.
    pub interpolate: gix_config_value::path::interpolate::Context<'a>,
    /// The hooks that `hook.*` values of the configuration name.
    pub configured: config::Configured,
}

/// The hooks of a repository, to find and run as Git does.
///
/// Create it with [`Hooks::new()`].
#[derive(Debug, Clone)]
pub struct Hooks {
    hooks_dir: PathBuf,
    working_dir: PathBuf,
    environment: Vec<(OsString, Option<OsString>)>,
    configured: config::Configured,
}

/// A hook that runs for an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hook {
    /// The program named after the event in the hooks directory, which Git calls a traditional hook.
    Program {
        /// Where the program is.
        path: PathBuf,
    },
    /// A command that the configuration names for the event with `hook.<name>.command`.
    Command {
        /// The `<name>` of `hook.<name>.command`.
        name: BString,
        /// The command: the path of a program, or a shell command line.
        command: BString,
    },
}

/// A hook of an event that doesn't run, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    /// The file named after the event in the hooks directory isn't executable, and Git ignores it.
    ///
    /// Unless `advice.ignoredHook` is false, Git tells the user once for each hook name:
    /// `The '<path>' hook was ignored because it's not set as executable.` Only Unix has executable files; on
    /// Windows, Git starts any file named after the event.
    NotExecutable {
        /// The file that isn't executable.
        path: PathBuf,
    },
    /// The configured hook is disabled with `hook.<name>.enabled`.
    Disabled {
        /// The `<name>` of the hook.
        name: BString,
    },
    /// The configured hooks of the event are disabled with `hook.<event>.enabled`, which leaves the program in the
    /// hooks directory alone.
    EventDisabled {
        /// The `<name>` of the hook.
        name: BString,
    },
}

/// The hooks of an event, as [`Hooks::find()`] finds them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    /// The hooks that run, in Git's order: the configured hooks first, then the program in the hooks directory.
    pub hooks: Vec<Hook>,
    /// The hooks that don't run.
    pub skipped: Vec<Skipped>,
}
