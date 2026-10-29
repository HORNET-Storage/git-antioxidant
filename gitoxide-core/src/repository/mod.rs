use std::fmt::Formatter;
use std::path::PathBuf;

use gix::{
    Result,
    bstr::BString,
    error::{ResultExt, message},
};

#[cfg(feature = "archive")]
pub mod archive;
pub mod branch;
pub mod cat;
pub use cat::function::cat;
pub mod blame;
pub mod commit;
pub mod config;
mod credential;
pub use credential::function as credential;
pub mod attributes;
#[cfg(feature = "clean")]
pub mod clean;
#[cfg(feature = "clean")]
pub use clean::function::clean;
pub mod diff;
mod diffstat;
pub mod dirty;
pub mod dirwalk;
mod editor;
pub use editor::function as editor;
#[cfg(feature = "blocking-client")]
pub mod clone;
pub mod exclude;
#[cfg(feature = "blocking-client")]
pub mod fetch;
#[cfg(feature = "blocking-client")]
pub use clone::function::clone;
#[cfg(feature = "blocking-client")]
pub use fetch::function::fetch;
#[cfg(feature = "blocking-client")]
pub mod push;
#[cfg(feature = "blocking-client")]
pub use push::function::push;

pub mod commitgraph;
mod fsck;
pub use fsck::function as fsck;
pub mod index;
pub mod log;
pub mod mailmap;
mod merge_base;
pub use merge_base::merge_base;
pub mod merge;
pub mod odb;
pub mod rebase;
pub use rebase::function::rebase;
pub mod remote;
pub mod reset;
pub use reset::function::reset;
pub mod revision;
pub mod status;
pub mod submodule;
pub mod tag;
pub mod tree;
pub mod verify;
pub mod worktree;

pub fn init(directory: Option<PathBuf>) -> Result<gix::discover::repository::Path> {
    gix::create::into(
        directory.unwrap_or_default(),
        gix::create::Kind::WithWorktree,
        gix::create::Options::default(),
    )
    .or_raise(|| message("Repository initialization failed"))
}

/// Return the action that `GIT_REFLOG_ACTION` sets in front of the messages of reflog entries, as `git commit`,
/// `git reset`, `git merge` and `git rebase` read it from their environment.
fn reflog_action() -> Option<BString> {
    std::env::var_os("GIT_REFLOG_ACTION").and_then(|action| gix::path::os_string_into_bstring(action).ok())
}

/// Return whether Git gives the advice `key`, as `advice_enabled()` in Git's `advice.c` decides: unless `GIT_ADVICE`
/// turns all advice off or `advice.<key>` is `false`.
fn advice_enabled(repo: &gix::Repository, key: &str) -> bool {
    let enabled = std::env::var_os("GIT_ADVICE")
        .and_then(|value| gix::config::Boolean::try_from(value).ok())
        .is_none_or(|value| value.0);
    enabled && repo.config_snapshot().boolean(format!("advice.{key}").as_str()) != Some(false)
}

/// Write `advice` to `err` unless the advice `key` is disabled, as `advise_if_enabled()` in Git's `advice.c` writes it:
/// followed by how to disable it unless `advice.<key>` is set.
fn advise_if_enabled(repo: &gix::Repository, key: &str, advice: &str, err: &mut dyn std::io::Write) -> Result<()> {
    if !advice_enabled(repo, key) {
        return Ok(());
    }
    if repo
        .config_snapshot()
        .boolean(format!("advice.{key}").as_str())
        .is_some()
    {
        return advise(advice, err);
    }
    advise(
        &format!("{advice}\nDisable this message with \"git config set advice.{key} false\""),
        err,
    )
}

/// Write each line of `advice` to `err` after `hint: `, or as `hint:` if it is empty, as `vadvise()` in Git's `advice.c`
/// writes it.
fn advise(advice: &str, err: &mut dyn std::io::Write) -> Result<()> {
    for line in advice.split_terminator('\n') {
        if line.is_empty() {
            writeln!(err, "hint:").or_error()?;
        } else {
            writeln!(err, "hint: {line}").or_error()?;
        }
    }
    Ok(())
}

pub enum PathsOrPatterns {
    Paths(Box<dyn std::iter::Iterator<Item = BString>>),
    Patterns(Vec<BString>),
}

struct HexId<'a>(gix::Id<'a>, bool);

impl<'a> HexId<'a> {
    pub fn new(id: gix::Id<'a>, long_hex: bool) -> Self {
        HexId(id, long_hex)
    }
}

impl std::fmt::Display for HexId<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let HexId(id, long_hex) = self;
        if *long_hex {
            id.fmt(f)
        } else {
            id.shorten_or_id().fmt(f)
        }
    }
}
