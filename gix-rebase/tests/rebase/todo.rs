//! The todo lists that `git rebase -i` hands its editor, which `gix-rebase` makes from the commits of the rebase and
//! `gix-sequencer` writes with the help that Git appends to them.

use std::{collections::HashMap, path::Path};

use bstr::{BStr, BString, ByteSlice};
use gix_hash::{ObjectId, oid};
use gix_rebase::{
    fork_point, plan,
    script::{self, RebaseMerges},
    todo, walk,
};
use gix_sequencer::todo::{Format, MissingCommitsCheck, Start, append_help};
use gix_testtools::TestResult;
use pretty_assertions::assert_eq;

use crate::{Repo, file, id_in, revs, root};

/// The options of `git rebase -i` in a case, besides the commits that it names.
#[derive(Default)]
struct Options<'a> {
    /// `--keep-base`.
    keep_base: bool,
    /// `--reapply-cherry-picks`.
    reapply_cherry_picks: bool,
    /// `--no-keep-empty`.
    no_keep_empty: bool,
    /// `rebase.abbreviateCommands`.
    abbreviate_commands: bool,
    /// `core.commentChar`, if it isn't `#`.
    comment: Option<&'a str>,
    /// `rebase.missingCommitsCheck`.
    missing_commits: MissingCommitsCheck,
    /// `--rebase-merges`.
    rebase_merges: Option<RebaseMerges>,
    /// `--autosquash`.
    autosquash: bool,
    /// The commands of `--exec`.
    exec: &'a [&'a str],
    /// `--update-refs`, with the branch that is rebased.
    update_refs: Option<&'a str>,
    /// `--fork-point`, with the branch of the upstream.
    fork_point: Option<&'a str>,
    /// `--root`.
    root: bool,
}

/// The abbreviation of each commit, as Git abbreviates it.
fn shorts(root: &Path) -> TestResult<HashMap<ObjectId, BString>> {
    let mut shorts = HashMap::new();
    for line in std::fs::read(root.join("shorts"))?.lines() {
        let (id, short) = line.split_once_str(" ").ok_or("an id and its abbreviation")?;
        shorts.insert(ObjectId::from_hex(id)?, short.into());
    }
    Ok(shorts)
}

/// The path of the worktree other than the main one that has the branch `name` checked out in the repository with
/// `git_dir`, as `branch_checked_out()` finds it.
fn checked_out(git_dir: &Path, name: &gix_ref::FullNameRef) -> Option<BString> {
    let head = [b"ref: ".as_slice(), name.as_bstr().as_bytes()].concat();
    for entry in std::fs::read_dir(git_dir.join("worktrees")).ok()? {
        let dir = entry.ok()?.path();
        if std::fs::read(dir.join("HEAD")).ok()?.trim_end() == head.as_slice() {
            let gitdir = std::fs::read(dir.join("gitdir")).ok()?;
            let gitdir = gitdir.trim_end();
            return Some(gitdir.strip_suffix(b"/.git").unwrap_or(gitdir).into());
        }
    }
    None
}

/// Make the todo list that `git rebase -i` hands its editor in the case `case` with `options`, as `do_interactive_rebase()`
/// and `complete_action()` make it, and assert that it is the one that Git wrote, byte for byte.
fn assert_todo_as_git_wrote_it(case: &str, options: Options<'_>) -> TestResult {
    let root = root()?;
    let repo = Repo::at(&root, "repo")?;
    let revs = revs(&root, case)?;
    let shorts = shorts(&root)?;
    let mut short = |id: &oid| -> gix_error::Result<BString> {
        Ok(shorts
            .get(id)
            .cloned()
            .ok_or_else(|| gix_error::message!("{id} has no abbreviation"))?)
    };
    let head = revs["head"];
    let upstream = revs.get("upstream").copied();
    let refs = repo.refs()?;
    let mut commits = repo.commits();

    let restrict = match options.fork_point {
        Some(branch) => {
            let name = gix_ref::FullName::try_from(format!("refs/heads/{branch}").as_str())?;
            let entries = fork_point::reflog_entries(&repo.refs, name.as_ref())?;
            fork_point::fork_point(&mut commits, &entries, upstream.ok_or("an upstream")?, head)?
        }
        None => None,
    };
    let reapply_cherry_picks = options.reapply_cherry_picks || options.keep_base;
    let squash_onto = if options.root {
        Some(id_in(&root, case, "squash-onto")?)
    } else {
        None
    };
    let plan = plan::Plan::new(
        &mut commits,
        head,
        &plan::Options {
            upstream,
            onto: revs.get("onto").copied(),
            keep_base: options.keep_base,
            restrict,
            reapply_cherry_picks,
            squash_onto,
        },
    )?;
    let walk = plan.walk(
        &mut commits,
        walk::Options {
            reapply_cherry_picks,
            rebase_merges: options.rebase_merges.is_some(),
        },
    )?;

    let comment: &BStr = options.comment.unwrap_or("#").into();
    let decorations = script::Decorations::new(refs.iter().cloned());
    let mut oneline = script::default_oneline;
    let mut list = script::make(
        &mut script::Context {
            commits: &mut commits,
            oneline: &mut oneline,
            abbreviate: &mut short,
            decorations: &decorations,
        },
        &walk,
        &script::Options {
            keep_empty: !options.no_keep_empty,
            rebase_merges: options.rebase_merges,
            comment,
            ..Default::default()
        },
    )?;
    if let Some(branch) = options.update_refs {
        let head_ref = gix_ref::FullName::try_from(format!("refs/heads/{branch}").as_str())?;
        todo::add_update_refs(
            &mut list,
            &decorations,
            Some(head_ref.as_ref()),
            comment,
            &mut |name| checked_out(&repo.git_dir, name),
            &mut |name| {
                refs.iter()
                    .find(|(candidate, _)| candidate.as_bstr() == name.as_bstr())
                    .map(|(_, id)| *id)
            },
            gix_testtools::object_hash(),
        );
    }
    if options.autosquash {
        let by_short: HashMap<&BStr, ObjectId> = shorts.iter().map(|(id, short)| (short.as_bstr(), *id)).collect();
        todo::rearrange_squash(&mut list, &mut commits, &mut |name| by_short.get(name).copied())?;
    }
    let exec: Vec<BString> = options.exec.iter().map(|command| BString::from(*command)).collect();
    todo::add_exec_commands(&mut list, &exec);

    let mut actual = list.to_bytes(Format {
        abbreviate_commands: options.abbreviate_commands,
        shorten_ids: Some(&mut short),
    })?;
    let head_short = short(&head)?;
    let revisions: BString = match plan.upstream {
        Some(upstream) => format!("{}..{head_short}", short(&upstream)?).into(),
        None => head_short,
    };
    let onto = short(&plan.onto)?;
    append_help(
        &mut actual,
        list.count_commands(),
        Some(Start {
            revisions: revisions.as_bstr(),
            onto: onto.as_bstr(),
        }),
        comment,
        options.missing_commits,
    );
    let expected = file(&root, case, "todo")?;
    assert_eq!(actual.to_str_lossy(), expected.to_str_lossy(), "{case}");
    Ok(())
}

#[test]
fn linear() -> TestResult {
    assert_todo_as_git_wrote_it("linear", Options::default())
}

#[test]
fn reapply_cherry_picks() -> TestResult {
    assert_todo_as_git_wrote_it(
        "reapply",
        Options {
            reapply_cherry_picks: true,
            ..Default::default()
        },
    )
}

#[test]
fn no_keep_empty() -> TestResult {
    assert_todo_as_git_wrote_it(
        "no-keep-empty",
        Options {
            no_keep_empty: true,
            ..Default::default()
        },
    )
}

#[test]
fn abbreviate_commands() -> TestResult {
    assert_todo_as_git_wrote_it(
        "abbreviate",
        Options {
            abbreviate_commands: true,
            ..Default::default()
        },
    )
}

#[test]
fn comment_char() -> TestResult {
    assert_todo_as_git_wrote_it(
        "comment",
        Options {
            comment: Some(";"),
            ..Default::default()
        },
    )
}

#[test]
fn missing_commits_check_error() -> TestResult {
    assert_todo_as_git_wrote_it(
        "missing-commits-error",
        Options {
            missing_commits: MissingCommitsCheck::Error,
            ..Default::default()
        },
    )
}

#[test]
fn exec() -> TestResult {
    assert_todo_as_git_wrote_it(
        "exec",
        Options {
            exec: &["echo one", "echo two"],
            ..Default::default()
        },
    )
}

#[test]
fn keep_base() -> TestResult {
    assert_todo_as_git_wrote_it(
        "keep-base",
        Options {
            keep_base: true,
            ..Default::default()
        },
    )
}

#[test]
fn onto() -> TestResult {
    assert_todo_as_git_wrote_it("onto", Options::default())
}

#[test]
fn root_onto_an_empty_commit() -> TestResult {
    assert_todo_as_git_wrote_it(
        "root",
        Options {
            root: true,
            ..Default::default()
        },
    )
}

#[test]
fn rebase_merges() -> TestResult {
    assert_todo_as_git_wrote_it(
        "merges",
        Options {
            rebase_merges: Some(RebaseMerges::NoRebaseCousins),
            ..Default::default()
        },
    )
}

#[test]
fn rebase_merges_with_cousins() -> TestResult {
    assert_todo_as_git_wrote_it(
        "merges-cousins",
        Options {
            rebase_merges: Some(RebaseMerges::RebaseCousins),
            ..Default::default()
        },
    )
}

#[test]
fn merges_are_left_out_without_rebase_merges() -> TestResult {
    assert_todo_as_git_wrote_it("merges-linear", Options::default())
}

#[test]
fn autosquash() -> TestResult {
    assert_todo_as_git_wrote_it(
        "autosquash",
        Options {
            autosquash: true,
            ..Default::default()
        },
    )
}

#[test]
fn update_refs() -> TestResult {
    assert_todo_as_git_wrote_it(
        "update-refs",
        Options {
            update_refs: Some("stack"),
            ..Default::default()
        },
    )
}

#[test]
fn fork_point() -> TestResult {
    assert_todo_as_git_wrote_it(
        "fork-point",
        Options {
            fork_point: Some("upstream"),
            ..Default::default()
        },
    )
}

#[test]
fn without_fork_point() -> TestResult {
    assert_todo_as_git_wrote_it("no-fork-point", Options::default())
}
