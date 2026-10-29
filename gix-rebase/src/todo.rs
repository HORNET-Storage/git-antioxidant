//! The changes that `complete_action()` makes to the todo list of a rebase before the user edits it: `update-ref`
//! commands for `--update-refs`, the order of `--autosquash` and `exec` commands for `--exec`, and the `pick` commands
//! that a rebase without `--force-rebase` skips as they leave their commits as they are.

use std::collections::HashMap;

use bstr::{BStr, BString, ByteSlice};
use gix_error::{Result, bail};
use gix_hash::ObjectId;
use gix_ref::FullNameRef;
use gix_sequencer::{
    rebase::UpdateRef,
    todo::{Command, Flags, Instruction, Item, Line, List},
};

use crate::{commits::Commits, script::Decorations};

/// Add an `update-ref` command after each command with a commit for each branch that points at the commit, as
/// `todo_list_add_update_ref_commands()` does for `--update-refs`, and return the branches with the commits they point
/// at, sorted by name, for `update-refs` in the state of the rebase.
///
/// The branch of `head_ref`, which the rebase updates anyway, is left out. A branch for which `checked_out` returns the
/// path of the worktree that has it checked out gets the comment `Ref <branch> checked out at '<path>'` instead. Each
/// command and comment is followed by an empty line, as Git ends their argument with a line feed. `current` returns
/// the commit that a branch points at, or `None` if it can't be read, which is recorded as the null id.
pub fn add_update_refs(
    list: &mut List,
    decorations: &Decorations,
    head_ref: Option<&FullNameRef>,
    comment: &BStr,
    checked_out: &mut dyn FnMut(&FullNameRef) -> Option<BString>,
    current: &mut dyn FnMut(&FullNameRef) -> Option<ObjectId>,
    object_hash: gix_hash::Kind,
) -> Vec<UpdateRef> {
    let mut records: Vec<UpdateRef> = Vec::new();
    let mut lines = Vec::with_capacity(list.lines.len());
    for line in std::mem::take(&mut list.lines) {
        let commit = line.item.instruction().and_then(|instruction| instruction.commit);
        lines.push(line);
        let Some(commit) = commit else { continue };
        for name in decorations.of(&commit) {
            let name_ref: &FullNameRef = name.as_ref();
            if head_ref == Some(name_ref) {
                continue;
            }
            let item = match checked_out(name_ref) {
                Some(path) => {
                    let mut text = BString::from(comment.as_bytes());
                    text.extend_from_slice(b" Ref ");
                    text.extend_from_slice(name.as_bstr());
                    text.extend_from_slice(b" checked out at '");
                    text.extend_from_slice(&path);
                    text.push(b'\'');
                    Item::Comment(text)
                }
                None => {
                    if !records.iter().any(|record| record.name == name.as_bstr()) {
                        records.push(UpdateRef {
                            name: name.as_bstr().into(),
                            before: current(name_ref).unwrap_or_else(|| object_hash.null()),
                            after: object_hash.null(),
                        });
                    }
                    Item::Command(Instruction::new(Command::UpdateRef, None, name.as_bstr()))
                }
            };
            lines.push(Line::new(item));
            lines.push(Line::new(Item::Comment(BString::default())));
        }
    }
    list.lines = lines;
    records.sort_by(|a, b| a.name.cmp(&b.name));
    records
}

/// Move each commit whose subject starts with `fixup! `, `amend! ` or `squash! ` right after the commit it names, as
/// `todo_list_rearrange_squash()` does for `--autosquash`, turning its `pick` into `fixup`, `fixup -C` or `squash`.
///
/// The commit named is the first earlier one with the rest of the subject as its subject, else the earlier commit that
/// `resolve` finds for a rest without spaces, else the first earlier one whose subject starts with the rest. Commits
/// that name the same commit follow it in their order. A list that has a `fixup` or `squash` already is an error.
pub fn rearrange_squash(
    list: &mut List,
    commits: &mut Commits<'_>,
    resolve: &mut dyn FnMut(&BStr) -> Option<ObjectId>,
) -> Result<()> {
    let count = list.lines.len();
    let mut next: Vec<Option<usize>> = vec![None; count];
    let mut tail: Vec<Option<usize>> = vec![None; count];
    let mut subjects: Vec<Option<BString>> = vec![None; count];
    let mut subject_to_index: HashMap<BString, usize> = HashMap::new();
    let mut commit_to_index: HashMap<ObjectId, usize> = HashMap::new();
    let mut rearranged = false;
    for index in 0..count {
        let Some(instruction) = list.lines[index].item.instruction() else {
            continue;
        };
        let Some(commit) = instruction.commit.filter(|_| instruction.command != Command::Drop) else {
            continue;
        };
        if instruction.command.is_fixup() {
            bail!("the script was already rearranged.".validation());
        }
        let subject = commits.get(&commit)?.subject();
        let mut target = None;
        if let Some(mut rest) = skip_fixupish(&subject) {
            loop {
                rest = rest.trim_start_with(|c| matches!(c, ' ' | '\t' | '\n' | '\r'));
                match skip_fixupish(rest) {
                    Some(after) => rest = after,
                    None => break,
                }
            }
            target = subject_to_index.get(rest.as_bstr()).copied();
            if target.is_none() && !rest.contains(&b' ') {
                target = resolve(rest.as_bstr()).and_then(|id| commit_to_index.get(&id).copied());
            }
            if target.is_none() {
                target = (0..index).find(|earlier| {
                    subjects[*earlier]
                        .as_ref()
                        .is_some_and(|subject| subject.starts_with(rest))
                });
            }
        }
        if let Some(target) = target {
            rearranged = true;
            if let Some(instruction) = list.lines[index].item.instruction_mut() {
                if subject.starts_with(b"fixup!") {
                    instruction.command = Command::Fixup;
                } else if subject.starts_with(b"amend!") {
                    instruction.command = Command::Fixup;
                    instruction.flags = Flags {
                        replace_fixup_message: true,
                        ..Flags::default()
                    };
                } else {
                    instruction.command = Command::Squash;
                }
            }
            let after = tail[target].unwrap_or(target);
            next[index] = next[after];
            next[after] = Some(index);
            tail[target] = Some(index);
        } else {
            subject_to_index.entry(subject.clone()).or_insert(index);
        }
        subjects[index] = Some(subject);
        commit_to_index.insert(commit, index);
    }
    if rearranged {
        let mut lines: Vec<Option<Line>> = std::mem::take(&mut list.lines).into_iter().map(Some).collect();
        let mut order = Vec::with_capacity(count);
        for (index, line) in lines.iter().enumerate() {
            if line
                .as_ref()
                .and_then(|line| line.item.command())
                .is_some_and(Command::is_fixup)
            {
                continue;
            }
            let mut current = Some(index);
            while let Some(index) = current {
                order.push(index);
                current = next[index];
            }
        }
        for index in order {
            if let Some(line) = lines[index].take() {
                list.lines.push(Line::new(line.item));
            }
        }
    }
    Ok(())
}

/// Return what follows `fixup! `, `amend! ` or `squash! ` at the start of `subject`, as `skip_fixupish()` does.
fn skip_fixupish(subject: &[u8]) -> Option<&[u8]> {
    [&b"fixup! "[..], b"amend! ", b"squash! "]
        .into_iter()
        .find_map(|prefix| subject.strip_prefix(prefix))
}

/// Add `commands` as `exec` commands after each `pick` and `merge` and the `fixup` and `squash` commands after it, as
/// `todo_list_add_exec_commands()` does for `--exec`.
pub fn add_exec_commands(list: &mut List, commands: &[BString]) {
    if commands.is_empty() {
        return;
    }
    let exec = || {
        commands
            .iter()
            .map(|command| Line::new(Item::Command(Instruction::new(Command::Exec, None, command.clone()))))
    };
    let mut lines = Vec::with_capacity(list.lines.len());
    let mut insert = false;
    for line in std::mem::take(&mut list.lines) {
        let command = line.item.command();
        if insert && !command.is_some_and(Command::is_fixup) {
            lines.extend(exec());
            insert = false;
        }
        lines.push(line);
        if matches!(command, Some(Command::Pick | Command::Merge)) {
            insert = true;
        }
    }
    if insert {
        lines.extend(exec());
    }
    list.lines = lines;
}

/// Return the number of lines at the start of `list` whose `pick` commands a rebase skips, as their commits have
/// their parent at `onto` or at the commit of the previous `pick` already, and the commit that the last of them
/// leaves `HEAD` at, as `skip_unnecessary_picks()` finds them. Lines that do nothing are skipped with them.
pub fn skip_unnecessary_picks(list: &List, onto: ObjectId, commits: &mut Commits<'_>) -> Result<(usize, ObjectId)> {
    let mut base = onto;
    let mut skipped = 0;
    for line in &list.lines {
        if line.item.is_noop() {
            skipped += 1;
            continue;
        }
        let Some(instruction) = line
            .item
            .instruction()
            .filter(|instruction| instruction.command == Command::Pick)
        else {
            break;
        };
        let Some(commit) = instruction.commit else {
            break;
        };
        let parents = &commits.get(&commit)?.parents;
        if parents.len() != 1 || parents[0] != base {
            break;
        }
        base = commit;
        skipped += 1;
    }
    Ok((skipped, base))
}
