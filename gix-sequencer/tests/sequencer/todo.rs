//! Todo lists as Git writes them, which `gix-sequencer` parses and writes back byte for byte, and lines that Git rejects,
//! which it rejects with the errors of Git.

use std::fmt::Write;

use bstr::{BStr, BString, ByteSlice};
use gix_hash::ObjectId;
use gix_sequencer::todo::{
    ById, Command, Format, Item, List, MissingCommitsCheck, ParseOptions, Reason, Resolved, append_help,
    default_comment, strip_space,
};
use gix_testtools::TestResult;
use pretty_assertions::assert_eq;

use crate::{Case, hash};

#[test]
fn lists_that_git_wrote_are_written_back_as_they_were() -> TestResult {
    for (case, file) in [
        ("edit", "rebase-merge/git-rebase-todo.backup"),
        ("edit", "rebase-merge/git-rebase-todo"),
        ("edit", "rebase-merge/done"),
        ("continued", "rebase-merge/done"),
        ("conflict", "rebase-merge/done"),
        ("exec-rescheduled", "rebase-merge/git-rebase-todo"),
        ("label", "rebase-merge/done"),
        ("update-ref", "rebase-merge/done"),
        ("dropped", "rebase-merge/git-rebase-todo.backup"),
    ] {
        let case = Case::new(case)?;
        let content = case.file(file)?;
        let options = ParseOptions {
            after_done: true,
            ..Default::default()
        };
        let (list, errors) = List::parse(&content, options, &mut ById::new(&case.objects, hash()))?;
        assert!(errors.is_empty(), "{file}: {errors:?}");
        assert_eq!(list.text(), content, "{file} as it was read");
        assert_eq!(list.to_bytes(Format::default())?, content, "{file} written anew");
    }
    Ok(())
}

#[test]
fn the_list_of_a_sequence_is_written_back_with_its_abbreviations() -> TestResult {
    let case = Case::new("cherry-pick")?;
    let shorts = case.shorts()?;
    let content = case.file("sequencer/todo")?;
    let mut resolve = |name: &BStr| -> gix_error::Result<Resolved> {
        Ok(match shorts.iter().find(|(_, short)| short.as_bstr() == name) {
            Some((commit, _)) => Resolved::Commit {
                id: case.rev(commit),
                is_merge: false,
            },
            None => Resolved::Nothing,
        })
    };
    let options = ParseOptions {
        rebase: false,
        ..Default::default()
    };
    let (list, errors) = List::parse(&content, options, &mut resolve)?;
    assert!(errors.is_empty(), "{errors:?}");
    let mut shorten = |id: &gix_hash::oid| -> gix_error::Result<BString> {
        Ok(shorts
            .iter()
            .find(|(commit, _)| case.rev(commit) == id)
            .map(|(_, short)| short.clone())
            .ok_or_else(|| gix_error::message!("{id} has no abbreviation"))?)
    };
    let written = list.to_bytes(Format {
        abbreviate_commands: false,
        shorten_ids: Some(&mut shorten),
    })?;
    assert_eq!(written, content);
    Ok(())
}

#[test]
fn the_list_for_the_editor_of_a_rebase_in_progress_has_the_help_of_git() -> TestResult {
    let case = Case::new("invalid")?;
    let shorts = case.shorts()?;
    let content = case.file("edit-todo-list")?;
    let a = case.rev("a");
    let mut resolve = |name: &BStr| -> gix_error::Result<Resolved> {
        Ok(if name == shorts["a"].as_bstr() {
            Resolved::Commit { id: a, is_merge: false }
        } else {
            Resolved::Nothing
        })
    };
    let options = ParseOptions {
        after_done: true,
        ..Default::default()
    };
    let (list, errors) = List::parse(&content, options, &mut resolve)?;
    assert!(errors.is_empty(), "{errors:?}");
    let commands = List::from_items(list.items().filter(|item| item.command().is_some()).cloned());
    let mut written = commands.to_bytes(Format {
        abbreviate_commands: false,
        shorten_ids: Some(&mut |_| Ok(shorts["a"].clone())),
    })?;
    append_help(
        &mut written,
        commands.count_commands(),
        None,
        default_comment(),
        MissingCommitsCheck::Ignore,
    );
    assert_eq!(written, content);
    Ok(())
}

#[test]
fn invalid_lines_are_rejected_with_the_errors_of_git() -> TestResult {
    let case = Case::new("invalid")?;
    let content = strip_space(&case.file("todo")?, Some(default_comment()));
    let options = ParseOptions {
        after_done: true,
        ..Default::default()
    };
    let (list, errors) = List::parse(&content, options, &mut ById::new(&case.objects, hash()))?;
    assert_eq!(errors.len(), 11, "all lines but the first");
    let mut expected = String::new();
    for error in &errors {
        writeln!(expected, "error: {}", error.reason)?;
        if error.reason.invalidates_line() {
            writeln!(expected, "error: {error}")?;
        }
    }
    expected.push_str("You can fix this with 'git rebase --edit-todo' and then run 'git rebase --continue'.\n");
    expected.push_str("Or you can abort the rebase with 'git rebase --abort'.\n");
    let git_says = case.file("errors")?;
    let without_hints: Vec<&[u8]> = git_says
        .lines_with_terminator()
        .filter(|line| !line.starts_with(b"hint: "))
        .collect();
    assert_eq!(without_hints.concat().to_str_lossy(), expected);
    for advice in errors.iter().filter_map(|error| error.reason.advice()) {
        for line in advice.lines() {
            assert!(git_says.contains_str(format!("hint: {line}\n")), "{line}");
        }
    }
    assert_eq!(list.text(), content, "the lines are kept as they were");
    assert_eq!(list.to_bytes(Format::default())?, content);
    Ok(())
}

/// A resolver that finds `id` for the name `A` and a merge commit for the name `M`.
fn resolve_a_and_m(id: ObjectId) -> impl FnMut(&BStr) -> gix_error::Result<Resolved> {
    move |name: &BStr| {
        Ok(match name.as_bytes() {
            b"A" => Resolved::Commit { id, is_merge: false },
            b"M" => Resolved::Commit { id, is_merge: true },
            _ => Resolved::Nothing,
        })
    }
}

#[test]
fn commands_and_their_letters_parse_alike() -> TestResult {
    let id = hash().null();
    let input = b"p A # a\nr A\ne A\ns A\nf A\nf -C A\nfixup -c A\nx make test\nb\nd A\nl one\nt one\nm -C M one # merge\nu refs/heads/x\nmerge one\n";
    let (list, errors) = List::parse(input, ParseOptions::default(), &mut resolve_a_and_m(id))?;
    assert!(errors.is_empty(), "{errors:?}");
    let commands: Vec<_> = list.instructions().map(|instruction| instruction.command).collect();
    assert_eq!(
        commands,
        [
            Command::Pick,
            Command::Reword,
            Command::Edit,
            Command::Squash,
            Command::Fixup,
            Command::Fixup,
            Command::Fixup,
            Command::Exec,
            Command::Break,
            Command::Drop,
            Command::Label,
            Command::Reset,
            Command::Merge,
            Command::UpdateRef,
            Command::Merge,
        ]
    );
    let flags: Vec<_> = list.instructions().map(|instruction| instruction.flags).collect();
    assert!(flags[5].replace_fixup_message && !flags[5].edit_fixup_message);
    assert!(flags[6].edit_fixup_message);
    assert!(!flags[12].edit_merge_message, "merge -C uses the message as it is");
    assert!(flags[14].edit_merge_message, "merge without a commit edits its message");

    let full = list.to_bytes(Format::default())?;
    let full_text = full.to_str()?;
    assert!(
        full_text.starts_with(&format!("pick {id} # a\nreword {id}\n")),
        "{full_text}"
    );
    assert!(full_text.contains(&format!("fixup -C {id}\nfixup -c {id}\nexec make test\nbreak\n")));
    assert!(full_text.ends_with(&format!(
        "merge -C {id} one # merge\nupdate-ref refs/heads/x\nmerge one\n"
    )));
    let abbreviated = list.to_bytes(Format {
        abbreviate_commands: true,
        shorten_ids: None,
    })?;
    assert!(abbreviated.starts_with(format!("p {id} # a\nr {id}\ne {id}\ns {id}\nf {id}\nf -C {id}\n").as_bytes()));
    Ok(())
}

#[test]
fn fixups_need_a_commit_before_them() -> TestResult {
    let id = hash().null();
    let input = b"# start\nfixup A\npick A\nsquash A\n";
    let (list, errors) = List::parse(input, ParseOptions::default(), &mut resolve_a_and_m(id))?;
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].number, 2);
    assert_eq!(errors[0].to_string(), "cannot 'fixup' without a previous commit");
    assert!(!errors[0].reason.invalidates_line());
    assert!(
        matches!(list.lines[1].item, Item::Command(_)),
        "the fixup stays a command"
    );

    let after_done = ParseOptions {
        after_done: true,
        ..Default::default()
    };
    let (_, errors) = List::parse(input, after_done, &mut resolve_a_and_m(id))?;
    assert!(errors.is_empty(), "a commit was picked before");
    Ok(())
}

#[test]
fn merge_commits_are_refused_where_git_refuses_them() -> TestResult {
    let id = hash().null();
    let input = b"pick M\nreword M\nedit M\nsquash M\ndrop M\nmerge -C M one\n";
    let (_, errors) = List::parse(input, ParseOptions::default(), &mut resolve_a_and_m(id))?;
    let reasons: Vec<_> = errors.iter().map(|error| error.reason.clone()).collect();
    assert_eq!(
        reasons,
        [
            Reason::MergeCommit { command: Command::Pick },
            Reason::MergeCommit {
                command: Command::Reword
            },
            Reason::MergeCommit { command: Command::Edit },
            Reason::SquashMergeCommit,
        ]
    );
    let sequence = ParseOptions {
        rebase: false,
        ..Default::default()
    };
    let (_, errors) = List::parse(b"pick M\n", sequence, &mut resolve_a_and_m(id))?;
    assert!(errors.is_empty(), "a cherry-pick sequence takes merge commits");
    Ok(())
}

#[test]
fn comments_start_with_the_comment_string() -> TestResult {
    let id = hash().null();
    let semicolon: &BStr = ";".into();
    let options = ParseOptions {
        comment: semicolon,
        ..Default::default()
    };
    let (list, errors) = List::parse(
        b"; a comment\n\t; another\n\npick A\n# not one\n",
        options,
        &mut resolve_a_and_m(id),
    )?;
    assert!(matches!(&list.lines[0].item, Item::Comment(text) if text == "; a comment"));
    assert!(matches!(&list.lines[1].item, Item::Comment(text) if text == "; another"));
    assert!(matches!(&list.lines[2].item, Item::Comment(text) if text.is_empty()));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].reason.to_string(), "invalid command '#'");
    assert_eq!(
        strip_space(b"\n  \n; gone\npick A  \n\n\n\nexec x\n\n", Some(semicolon)),
        "pick A\n\nexec x\n"
    );
    Ok(())
}

#[test]
fn missing_commits_check_takes_the_values_of_git() {
    for (value, expected) in [
        ("ignore", Some(MissingCommitsCheck::Ignore)),
        ("WARN", Some(MissingCommitsCheck::Warn)),
        ("Error", Some(MissingCommitsCheck::Error)),
        ("nonsense", None),
    ] {
        assert_eq!(
            MissingCommitsCheck::from_config_value(value.into()),
            expected,
            "{value}"
        );
    }
}
