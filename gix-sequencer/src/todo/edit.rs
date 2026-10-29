//! What Git adds to a todo list for the editor, and what it checks after the edit, as `rebase-interactive.c` does.

use bstr::{BStr, BString, ByteSlice};
use gix_error::Result;

use super::{Instruction, List};

/// The help that Git appends to the list for the editor, which `append_todo_help()` translates before it comments its
/// lines out.
const COMMANDS_HELP: &str = concat!(
    "\nCommands:\n",
    "p, pick <commit> = use commit\n",
    "r, reword <commit> = use commit, but edit the commit message\n",
    "e, edit <commit> = use commit, but stop for amending\n",
    "s, squash <commit> = use commit, but meld into previous commit\n",
    "f, fixup [-C | -c] <commit> = like \"squash\" but keep only the previous\n",
    "                   commit's log message, unless -C is used, in which case\n",
    "                   keep only this commit's message; -c is same as -C but\n",
    "                   opens the editor\n",
    "x, exec <command> = run command (the rest of the line) using shell\n",
    "b, break = stop here (continue rebase later with 'git rebase --continue')\n",
    "d, drop <commit> = remove commit\n",
    "l, label <label> = label current HEAD with a name\n",
    "t, reset <label> = reset HEAD to a label\n",
    "m, merge [-C <commit> | -c <commit>] <label> [# <oneline>]\n",
    "        create a merge commit using the original merge commit's\n",
    "        message (or the oneline, if no original merge commit was\n",
    "        specified); use -c <commit> to reword the commit message\n",
    "u, update-ref <ref> = track a placeholder for the <ref> to be updated\n",
    "                      to this position in the new commits. The <ref> is\n",
    "                      updated at the end of the rebase\n",
    "\n",
    "These lines can be re-ordered; they are executed from top to bottom.\n"
);

/// The advice that Git prints when the edited list can't be used, `edit_todo_list_advice`.
const EDIT_TODO_LIST_ADVICE: &str = concat!(
    "You can fix this with 'git rebase --edit-todo' and then run 'git rebase --continue'.\n",
    "Or you can abort the rebase with 'git rebase --abort'.\n"
);

/// What the first line of the help names when a rebase starts: the commits that it replays, and the commit it replays
/// them onto.
#[derive(Debug, Clone, Copy)]
pub struct Start<'a> {
    /// The commits, like `5fd91cf..b361970`, as `get_revision_ranges()` names them with abbreviated ids: the upstream and
    /// the original `HEAD` separated by `..`, or only the original `HEAD` if there is no upstream as with `--root`.
    pub revisions: &'a BStr,
    /// The abbreviated id of the commit that the rebase replays the commits onto.
    pub onto: &'a BStr,
}

/// Append the help that Git appends to a todo list for the editor to `out`, as `append_todo_help()` does, with its lines
/// commented out by `comment`, the value of `core.commentString` or `core.commentChar`.
///
/// `command_count` is the number of [commands](List::count_commands()) of the list. `start` names the commits of a
/// rebase that starts, which the help then begins with, or is `None` if the list of a rebase in progress is edited, which
/// the help then ends with. With `missing_commits` at [`MissingCommitsCheck::Error`], the help tells not to remove lines.
pub fn append_help(
    out: &mut BString,
    command_count: usize,
    start: Option<Start<'_>>,
    comment: &BStr,
    missing_commits: MissingCommitsCheck,
) {
    if let Some(Start { revisions, onto }) = start {
        out.push(b'\n');
        let commands = if command_count == 1 { "command" } else { "commands" };
        add_commented_lines(
            out,
            comment,
            format!("Rebase {revisions} onto {onto} ({command_count} {commands})").as_bytes(),
        );
    }
    add_commented_lines(out, comment, COMMANDS_HELP.as_bytes());
    add_commented_lines(
        out,
        comment,
        if missing_commits == MissingCommitsCheck::Error {
            b"\nDo not remove any line. Use 'drop' explicitly to remove a commit.\n"
        } else {
            b"\nIf you remove a line here THAT COMMIT WILL BE LOST.\n"
        },
    );
    add_commented_lines(
        out,
        comment,
        if start.is_none() {
            concat!(
                "\nYou are editing the todo file of an ongoing interactive rebase.\n",
                "To continue rebase after editing, run:\n",
                "    git rebase --continue\n\n"
            )
            .as_bytes()
        } else {
            b"\nHowever, if you remove everything, the rebase will be aborted.\n\n"
        },
    );
}

/// Append each line of `text` to `out` behind `prefix`, separated from it by a space unless the line is empty or starts
/// with a tab, and end `out` with a line feed, as `strbuf_add_commented_lines()` does.
pub(crate) fn add_commented_lines(out: &mut BString, prefix: &BStr, text: &[u8]) {
    for line in text.lines_with_terminator() {
        out.extend_from_slice(prefix);
        if !matches!(line.first(), Some(b'\n' | b'\t')) {
            out.push(b' ');
        }
        out.extend_from_slice(line);
    }
    if !out.is_empty() && !out.ends_with(b"\n") {
        out.push(b'\n');
    }
}

/// Return `text` with the spaces at the end of its lines removed, without empty lines at its start and end, with runs
/// of empty lines turned into one, and with a line feed at its end, as `strbuf_stripspace()` does. With `comment`, the
/// value of `core.commentString` or `core.commentChar`, lines that start with it are removed too.
///
/// Git strips an edited todo list this way before it parses it, with the comment string.
pub fn strip_space(text: &[u8], comment: Option<&BStr>) -> BString {
    let mut out = BString::default();
    let mut empties = 0;
    for line in text.lines_with_terminator() {
        if comment.is_some_and(|comment| line.starts_with(comment)) {
            continue;
        }
        let end = line
            .iter()
            .rposition(|byte| !is_space(*byte))
            .map_or(0, |last| last + 1);
        if end == 0 {
            empties += 1;
            continue;
        }
        if empties > 0 && !out.is_empty() {
            out.push(b'\n');
        }
        empties = 0;
        out.extend_from_slice(&line[..end]);
        out.push(b'\n');
    }
    out
}

/// Return `true` for the bytes that Git counts as space: space, tab, line feed and carriage return.
pub(crate) fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

/// How Git reacts to commits that an edit of a todo list dropped without `drop`, as `rebase.missingCommitsCheck` says.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum MissingCommitsCheck {
    /// Don't check, the default.
    #[default]
    Ignore,
    /// Warn about the dropped commits, and use the list.
    Warn,
    /// Warn about the dropped commits, and refuse the list until they are added or dropped with `drop`.
    Error,
}

impl MissingCommitsCheck {
    /// Parse `value` of `rebase.missingCommitsCheck` as `get_missing_commit_check_level()` does: `ignore`, `warn` or
    /// `error`, ignoring case. For another value, Git warns
    /// `unrecognized setting <value> for option rebase.missingCommitsCheck. Ignoring.` and ignores it, as `None` here
    /// tells.
    pub fn from_config_value(value: &BStr) -> Option<Self> {
        Some(if value.eq_ignore_ascii_case(b"ignore") {
            MissingCommitsCheck::Ignore
        } else if value.eq_ignore_ascii_case(b"warn") {
            MissingCommitsCheck::Warn
        } else if value.eq_ignore_ascii_case(b"error") {
            MissingCommitsCheck::Error
        } else {
            return None;
        })
    }
}

/// Return the instructions of `old` whose commits no item of `new` names, from its last line to its first, which is
/// from the newer commits to the older ones, as `todo_list_check()` finds the commits that the edit of `old` into `new`
/// dropped by mistake.
///
/// Each commit is listed once. Commits that `new` drops with `drop` aren't missing.
pub fn missing_commits<'a>(old: &'a List, new: &List) -> Vec<&'a Instruction> {
    let mut seen: std::collections::HashSet<_> = new.instructions().filter_map(|item| item.commit).collect();
    let mut missing = Vec::new();
    for instruction in old.lines.iter().rev().filter_map(|line| line.item.instruction()) {
        if let Some(commit) = instruction.commit
            && seen.insert(commit)
        {
            missing.push(instruction);
        }
    }
    missing
}

/// Return the warning that Git prints about `missing` commits, as found by [`missing_commits()`], with their ids
/// shortened by `shorten_id` as `todo_list_check()` abbreviates them.
pub fn missing_commits_report(
    missing: &[&Instruction],
    shorten_id: &mut dyn FnMut(&gix_hash::oid) -> Result<BString>,
) -> Result<BString> {
    let mut out =
        BString::from("Warning: some commits may have been dropped accidentally.\nDropped commits (newer to older):\n");
    for instruction in missing {
        out.extend_from_slice(b" - ");
        if let Some(commit) = &instruction.commit {
            out.extend_from_slice(&shorten_id(commit)?);
        }
        out.push(b' ');
        out.extend_from_slice(&instruction.arg);
        out.push(b'\n');
    }
    out.extend_from_slice(
        concat!(
            "To avoid this message, use \"drop\" to explicitly remove a commit.\n\n",
            "Use 'git config rebase.missingCommitsCheck' to change the level of warnings.\n",
            "The possible behaviours are: ignore, warn, error.\n\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(EDIT_TODO_LIST_ADVICE.as_bytes());
    Ok(out)
}
