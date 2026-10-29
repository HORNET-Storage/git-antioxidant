use gix::bstr::BString;

/// The options of `gix reset`, named after those of `git reset`.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// How the index and the worktree change, as `--soft`, `--mixed`, `--hard`, `--merge` and `--keep` say, or `None` for
    /// Git's default, which is `--mixed`.
    pub mode: Option<gix::reset::Mode>,
    /// The arguments before `--`: the commit or tree-ish to reset to, and the paths that follow it unless `--` separates
    /// them, as `git reset` tells them apart.
    pub args: Vec<BString>,
    /// The paths after `--`, whose entries of the index are reset.
    pub paths: Vec<BString>,
}

pub(crate) mod function {
    use std::path::Path;

    use gix::{
        Result,
        bstr::{BStr, BString, ByteSlice},
        error::{ResultExt, bail, message},
        reset::Mode,
        status::index_worktree::Item,
    };
    use gix_status::index_as_worktree::{Change, EntryStatus};

    use super::Options;

    /// The hint of Git's messages about arguments that are neither clearly revisions nor clearly paths.
    const DOUBLE_DASH_HINT: &str =
        "Use '--' to separate paths from revisions, like this:\n'git <command> [<revision>...] -- [<file>...]'";

    /// Reset as `git reset` does with the arguments of `options`, as `cmd_reset()` in Git's `builtin/reset.c` does, and
    /// print what it prints: `HEAD is now at <commit> <subject>` after a hard reset, and after a mixed reset, the entries
    /// whose files differ from them, after `Unstaged changes after reset:`.
    ///
    /// Without paths, `HEAD` moves to the commit, `HEAD` if none is given, and the index and the worktree change as the
    /// mode says. With paths, only their entries of the index are reset to those of the tree-ish. `GIT_REFLOG_ACTION`
    /// replaces the action of the reflog messages, as in Git.
    pub fn reset(
        repo: gix::Repository,
        mut out: impl std::io::Write,
        mut err: impl std::io::Write,
        Options { mode, args, paths }: Options,
    ) -> Result<()> {
        let (revision, paths) = parse_args(&repo, args, paths)?;
        let unborn = revision == "HEAD" && repo.head()?.is_unborn();
        let target = if unborn {
            None
        } else {
            let object = repo.rev_parse_single(revision.as_bstr()).or_raise(|| {
                if paths.is_empty() {
                    message!("Failed to resolve '{revision}' as a valid revision.")
                } else {
                    message!("Failed to resolve '{revision}' as a valid tree.")
                }
            })?;
            let object = object.object()?;
            let id = if paths.is_empty() {
                object.peel_to_commit().map(|commit| commit.id)
            } else {
                object.peel_to_tree().map(|tree| tree.id)
            }
            .or_raise(|| message!("Could not parse object '{revision}'."))?;
            Some(id)
        };

        if !paths.is_empty() {
            match mode {
                None => {}
                Some(Mode::Mixed) => writeln!(
                    err,
                    "warning: --mixed with paths is deprecated; use 'git reset -- <paths>' instead."
                )
                .or_error()?,
                Some(mode) => bail!(message!("Cannot do {} reset with paths.", mode.as_str()).validation()),
            }
            repo.reset_paths(target, &paths)?;
            return print_unstaged_changes(&repo, &mut out);
        }

        let mode = mode.unwrap_or(Mode::Mixed);
        let outcome = repo.reset(
            target,
            mode,
            gix::reset::Options {
                revision: Some(revision.clone()),
                reflog_action: crate::repository::reflog_action(),
            },
            &gix::interrupt::IS_INTERRUPTED,
        )?;
        for unremoved in &outcome.changes.unremoved {
            writeln!(
                err,
                "warning: unable to unlink '{}': {}",
                unremoved.path, unremoved.error
            )
            .or_error()?;
        }
        if let Some(error) = outcome.changes.checkout_error {
            return Err(error).or_raise(|| message!("Could not check out all files of revision '{revision}'"));
        }
        match (mode, outcome.commit_id) {
            (Mode::Mixed, _) => print_unstaged_changes(&repo, &mut out),
            (Mode::Hard, Some(commit_id)) => print_new_head(&repo, commit_id, &mut out),
            _ => Ok(()),
        }
    }

    /// Tell the revision and the paths in `args` and `paths` apart as `parse_args()` in Git's `builtin/reset.c` does, and
    /// return the revision, which is `HEAD` if none is given or it is `@`, and the paths.
    ///
    /// With `--`, the revision is the only argument before it. Without, the first argument is the revision if it names an
    /// object and isn't also a file, a path if it is a file or looks like a pathspec, and refused otherwise, and the
    /// remaining arguments are paths.
    fn parse_args(repo: &gix::Repository, args: Vec<BString>, paths: Vec<BString>) -> Result<(BString, Vec<BString>)> {
        let (revision, mut pathspecs) = match args.first() {
            Some(first) if paths.is_empty() || args.len() > 1 => {
                if repo.rev_parse_single(first.as_bstr()).is_ok() {
                    verify_non_filename(repo, first.as_bstr())?;
                    (Some(first.clone()), args[1..].to_vec())
                } else {
                    verify_filename(repo, first.as_bstr())?;
                    (None, args)
                }
            }
            first => (first.cloned(), Vec::new()),
        };
        pathspecs.extend(paths);
        let revision = revision
            .filter(|revision| revision != "@")
            .unwrap_or_else(|| "HEAD".into());
        Ok((revision, pathspecs))
    }

    /// Fail as `verify_non_filename()` in Git's `setup.c` does if `arg`, which names an object, is also a file.
    fn verify_non_filename(repo: &gix::Repository, arg: &BStr) -> Result<()> {
        if names_file(repo, arg)? {
            bail!(message!("ambiguous argument '{arg}': both revision and filename\n{DOUBLE_DASH_HINT}").validation());
        }
        Ok(())
    }

    /// Fail as `verify_filename()` in Git's `setup.c` does if `arg`, which names no object, neither is a file nor looks
    /// like a pathspec.
    fn verify_filename(repo: &gix::Repository, arg: &BStr) -> Result<()> {
        if looks_like_pathspec(arg) || names_file(repo, arg)? {
            return Ok(());
        }
        bail!(
            message!(
                "ambiguous argument '{arg}': unknown revision or path not in the working tree.\n{DOUBLE_DASH_HINT}"
            )
            .validation()
        )
    }

    /// Return `true` if `arg` has a glob character that isn't escaped or starts with the magic `:(`, as
    /// `looks_like_pathspec()` in Git's `setup.c` decides.
    fn looks_like_pathspec(arg: &BStr) -> bool {
        let mut escaped = false;
        for &byte in arg.iter() {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if matches!(byte, b'*' | b'?' | b'[') {
                return true;
            }
        }
        arg.starts_with(b":(")
    }

    /// Return `true` if `arg` names a file or directory, relative to the current directory in the worktree or, after `:/`,
    /// to its root, as `check_filename()` in Git's `setup.c` checks it. Outside a worktree, nothing is a file.
    fn names_file(repo: &gix::Repository, arg: &BStr) -> Result<bool> {
        let Some(workdir) = repo.workdir() else {
            return Ok(false);
        };
        let (base, name) = if let Some(name) = arg.strip_prefix(b":/") {
            (workdir.to_owned(), name)
        } else {
            let base = workdir.join(repo.prefix()?.unwrap_or(Path::new("")));
            (
                base,
                arg.strip_prefix(b":!")
                    .or_else(|| arg.strip_prefix(b":^"))
                    .unwrap_or(arg),
            )
        };
        if name.is_empty() && arg.starts_with(b":") {
            return Ok(true);
        }
        let path = base.join(gix::path::from_bstr(name.as_bstr())?);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => Ok(true),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(false)
            }
            Err(err) => Err(err).or_raise(|| message!("failed to stat '{}'", path.display())),
        }
    }

    /// Print `HEAD is now at <commit> <subject>` for `commit_id`, as `print_new_head_line()` in Git's `builtin/reset.c` does.
    fn print_new_head(repo: &gix::Repository, commit_id: gix::ObjectId, out: &mut dyn std::io::Write) -> Result<()> {
        let commit = repo.find_commit(commit_id)?;
        let mut line = BString::from(format!("HEAD is now at {}", commit.id().shorten_or_id()));
        let subject = crate::repository::commit::subject(commit.decode()?.message);
        if !subject.is_empty() {
            line.push(b' ');
            line.extend_from_slice(&subject);
        }
        line.push(b'\n');
        out.write_all(&line).or_error()
    }

    /// Print the entries whose files differ from them, in the order of the index, after `Unstaged changes after reset:`,
    /// as `refresh_index()` in Git's `read-cache.c` prints them after a mixed reset, and write the information about the
    /// files that didn't change into the index, as Git refreshes it. Nothing is printed without a worktree.
    fn print_unstaged_changes(repo: &gix::Repository, out: &mut dyn std::io::Write) -> Result<()> {
        if repo.workdir().is_none() {
            return Ok(());
        }
        let mut iter = repo
            .status(gix::progress::Discard)?
            .untracked_files(gix::status::UntrackedFiles::None)
            .index_worktree_submodules(gix::status::Submodule::Given {
                ignore: gix::submodule::config::Ignore::Dirty,
                check_dirty: false,
            })
            .into_index_worktree_iter(Vec::new())?;
        let mut changes = Vec::new();
        for item in iter.by_ref() {
            let Item::Modification { rela_path, status, .. } = item? else {
                continue;
            };
            let kind = match status {
                EntryStatus::Conflict { .. } => 'U',
                EntryStatus::Change(Change::Removed) => 'D',
                EntryStatus::Change(Change::Type { .. }) => 'T',
                EntryStatus::Change(Change::Modification { .. } | Change::SubmoduleModification(_)) => 'M',
                EntryStatus::IntentToAdd => 'A',
                EntryStatus::NeedsUpdate(_) => continue,
            };
            changes.push((rela_path, kind));
        }
        if let Some(outcome) = iter.outcome_mut()
            && outcome.has_changes()
        {
            outcome.write_changes().transpose()?;
        }

        changes.sort();
        if !changes.is_empty() {
            writeln!(out, "Unstaged changes after reset:").or_error()?;
        }
        for (path, kind) in changes {
            let mut line = BString::from(format!("{kind}\t"));
            line.extend_from_slice(&path);
            line.push(b'\n');
            out.write_all(&line).or_error()?;
        }
        Ok(())
    }
}
