//! Parsing todo lists as `todo_list_parse_insn_buffer()` and `parse_insn_line()` do.

use bstr::{BStr, BString, ByteSlice};
use gix_error::Result;
use gix_hash::ObjectId;

use super::{Command, Flags, Instruction, Item, Line, List};

/// What a name of a commit in a todo list names, as `repo_get_oid()` and `lookup_commit_reference()` find it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    /// A commit, or an annotated tag that leads to one.
    Commit {
        /// The id of the commit.
        id: ObjectId,
        /// Whether the commit has more than one parent.
        is_merge: bool,
    },
    /// An object that isn't a commit, and isn't a tag that leads to one.
    NotACommit {
        /// The id of the object that the name, or the tags it names, lead to.
        id: ObjectId,
        /// The kind of that object.
        kind: gix_object::Kind,
    },
    /// Nothing, as the name names no object.
    Nothing,
}

/// Find what the names of commits in a todo list name, as `parse_insn_line()` does with `repo_get_oid()`, which accepts
/// any name of a revision.
pub trait Resolve {
    /// Return what `name` names.
    fn resolve(&mut self, name: &BStr) -> Result<Resolved>;
}

impl<F> Resolve for F
where
    F: FnMut(&BStr) -> Result<Resolved>,
{
    fn resolve(&mut self, name: &BStr) -> Result<Resolved> {
        self(name)
    }
}

/// A [`Resolve`] for names that are full hexadecimal object ids, the names that Git writes into the todo lists of a
/// rebase and into its `done`, which looks them up in an object database and follows annotated tags.
///
/// Other names, like abbreviated ids or names of references, name nothing.
pub struct ById<T> {
    objects: T,
    object_hash: gix_hash::Kind,
    buf: Vec<u8>,
}

impl<T> ById<T>
where
    T: gix_object::Find,
{
    /// Look up the ids of `object_hash` that names spell out in `objects`.
    pub fn new(objects: T, object_hash: gix_hash::Kind) -> Self {
        ById {
            objects,
            object_hash,
            buf: Vec::new(),
        }
    }
}

impl<T> Resolve for ById<T>
where
    T: gix_object::Find,
{
    fn resolve(&mut self, name: &BStr) -> Result<Resolved> {
        let Some(mut id) = crate::file::id(name, self.object_hash) else {
            return Ok(Resolved::Nothing);
        };
        loop {
            let Some(object) = self.objects.try_find(&id, &mut self.buf)? else {
                return Ok(Resolved::Nothing);
            };
            match object.kind {
                gix_object::Kind::Commit => {
                    let parents = gix_object::CommitRefIter::from_bytes(object.data, self.object_hash)
                        .parent_ids()
                        .count();
                    return Ok(Resolved::Commit {
                        id,
                        is_merge: parents > 1,
                    });
                }
                gix_object::Kind::Tag => {
                    id = gix_object::TagRefIter::from_bytes(object.data, self.object_hash).target_id()?;
                }
                kind => return Ok(Resolved::NotACommit { id, kind }),
            }
        }
    }
}

/// How [`List::parse()`] reads a list.
#[derive(Debug, Clone, Copy)]
pub struct ParseOptions<'a> {
    /// The string that starts comments, the value of `core.commentString` or `core.commentChar`, `#` by default.
    pub comment: &'a BStr,
    /// Whether the list belongs to a rebase, in which `pick`, `reword`, `edit`, `fixup` and `squash` refuse merge
    /// commits, as `parse_insn_line()` checks if `is_rebase_i()`. The lists of cherry-pick and revert sequences take them.
    pub rebase: bool,
    /// Whether `done` exists in the directory of the rebase, so that a commit was picked before and the list may start
    /// with `fixup` or `squash`, as `todo_list_parse_insn_buffer()` checks with `file_exists(rebase_path_done())`.
    pub after_done: bool,
}

impl Default for ParseOptions<'_> {
    /// The options for the list of a rebase that didn't pick a commit yet, with `#` as comment string.
    fn default() -> Self {
        ParseOptions {
            comment: super::default_comment(),
            rebase: true,
            after_done: false,
        }
    }
}

/// Why Git rejects a line of a todo list, with the message that `parse_insn_line()` and
/// `todo_list_parse_insn_buffer()` print for it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reason {
    /// The line starts with no command: `invalid command '<name>'`.
    InvalidCommand {
        /// The word that starts the line.
        name: BString,
    },
    /// `break` and `noop` take no argument: `<command> does not accept arguments: '<arguments>'`.
    UnexpectedArguments {
        /// The command.
        command: Command,
        /// What follows it up to the end of the list, with the lines after it, as Git prints the rest of the list.
        arguments: BString,
    },
    /// The command needs an argument but none follows: `missing arguments for <command>`.
    MissingArguments {
        /// The command.
        command: Command,
    },
    /// The name of a label is `#` or isn't a valid reference name: `'<label>' is not a valid label`.
    InvalidLabel {
        /// The name.
        label: BString,
    },
    /// The reference of `update-ref` isn't a valid reference name: `'<name>' is not a valid refname`.
    InvalidRefName {
        /// The name.
        name: BString,
    },
    /// The reference of `update-ref` has only one component:
    /// `update-ref requires a fully qualified refname e.g. refs/heads/<name>`.
    UnqualifiedRefName {
        /// The name.
        name: BString,
    },
    /// The name of the commit names nothing: `could not parse '<name>'`.
    UnknownCommit {
        /// The name.
        name: BString,
    },
    /// The name of the commit names another kind of object: `object <id> is a <kind>, not a commit`.
    NotACommit {
        /// The id of the object.
        id: ObjectId,
        /// Its kind.
        kind: gix_object::Kind,
    },
    /// A rebase can't pick, reword or edit a merge commit: `'<command>' does not accept merge commits`.
    MergeCommit {
        /// The command.
        command: Command,
    },
    /// A rebase can't meld a merge commit into another: `cannot squash merge commit into another commit`.
    SquashMergeCommit,
    /// A `fixup` or `squash` comes before any command that leaves a commit to meld into, while nothing was picked before:
    /// `cannot '<command>' without a previous commit`. Unlike the other reasons, it leaves the line valid.
    FixupWithoutPreviousCommit {
        /// The command.
        command: Command,
    },
}

impl Reason {
    /// Return `true` if Git keeps the line as invalid for this reason, so that it is written back as it was, which is the
    /// case for all reasons but [`Reason::FixupWithoutPreviousCommit`].
    pub fn invalidates_line(&self) -> bool {
        !matches!(self, Reason::FixupWithoutPreviousCommit { .. })
    }

    /// Return the advice that Git gives after the message, if `advice.rebaseTodoError` isn't turned off, as
    /// `check_merge_commit_insn()` gives it for merge commits that `pick`, `reword` and `edit` refuse.
    pub fn advice(&self) -> Option<&'static str> {
        match self {
            Reason::MergeCommit { command: Command::Pick } => Some(
                "'pick' does not take a merge commit. If you wanted to\nreplay the merge, use 'merge -C' on the commit.",
            ),
            Reason::MergeCommit {
                command: Command::Reword,
            } => Some(concat!(
                "'reword' does not take a merge commit. If you wanted to\n",
                "replay the merge and reword the commit message, use\n",
                "'merge -c' on the commit"
            )),
            Reason::MergeCommit { command: Command::Edit } => Some(concat!(
                "'edit' does not take a merge commit. If you wanted to\n",
                "replay the merge, use 'merge -C' on the commit, and then\n",
                "'break' to give the control back to you so that you can\n",
                "do 'git commit --amend && git rebase --continue'."
            )),
            _ => None,
        }
    }
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reason::InvalidCommand { name } => write!(f, "invalid command '{name}'"),
            Reason::UnexpectedArguments { command, arguments } => {
                write!(f, "{command} does not accept arguments: '{arguments}'")
            }
            Reason::MissingArguments { command } => write!(f, "missing arguments for {command}"),
            Reason::InvalidLabel { label } => write!(f, "'{label}' is not a valid label"),
            Reason::InvalidRefName { name } => write!(f, "'{name}' is not a valid refname"),
            Reason::UnqualifiedRefName { name } => {
                write!(
                    f,
                    "update-ref requires a fully qualified refname e.g. refs/heads/{name}"
                )
            }
            Reason::UnknownCommit { name } => write!(f, "could not parse '{name}'"),
            Reason::NotACommit { id, kind } => write!(f, "object {id} is a {kind}, not a commit"),
            Reason::MergeCommit { command } => write!(f, "'{command}' does not accept merge commits"),
            Reason::SquashMergeCommit => f.write_str("cannot squash merge commit into another commit"),
            Reason::FixupWithoutPreviousCommit { command } => {
                write!(f, "cannot '{command}' without a previous commit")
            }
        }
    }
}

/// A line of a todo list that Git rejects, or a `fixup` or `squash` without a previous commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineError {
    /// The number of the line in the list, starting at 1.
    pub number: usize,
    /// The line, without its line ending.
    pub line: BString,
    /// Why Git rejects it.
    pub reason: Reason,
}

impl std::fmt::Display for LineError {
    /// Write `invalid line <number>: <line>` for a line that Git keeps as invalid, which Git prints after the message of
    /// the [reason](Reason), or the message of the reason otherwise.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.reason.invalidates_line() {
            write!(f, "invalid line {}: {}", self.number, self.line)
        } else {
            self.reason.fmt(f)
        }
    }
}

impl std::error::Error for LineError {}

impl List {
    /// Parse the todo list `input` as `todo_list_parse_insn_buffer()` does with `options`, finding the commits it names
    /// with `resolve`, and return the list with the errors that Git reports about its lines, in order.
    ///
    /// Each line keeps its text as it was. A line that Git rejects becomes an [invalid item](Item::Invalid) and has an
    /// error, and a `fixup` or `squash` that comes before any commit to meld into has an error but stays a command, as
    /// Git reports both but keeps the list. Git uses such a list only once the errors are fixed.
    ///
    /// Lines end with a line feed, and a carriage return before it is ignored, except that it belongs to the name of a
    /// commit that nothing follows, as Git takes it.
    pub fn parse(input: &[u8], options: ParseOptions<'_>, resolve: &mut dyn Resolve) -> Result<(List, Vec<LineError>)> {
        let mut lines = Vec::new();
        let mut errors = Vec::new();
        let mut fixup_okay = options.after_done;
        let mut offset = 0;
        for (index, text) in super::lines(input).enumerate() {
            let number = index + 1;
            let line = super::strip_line_ending(text);
            let line_with_carriage_return = text.strip_suffix(b"\n").unwrap_or(text);
            let rest_of_input = &input[offset..];
            offset += text.len();
            let item = match parse_line(line, line_with_carriage_return, rest_of_input, &options, resolve)? {
                Ok(item) => item,
                Err(reason) => {
                    errors.push(LineError {
                        number,
                        line: line.into(),
                        reason,
                    });
                    Item::Invalid(line.into())
                }
            };
            if !fixup_okay {
                match item.command() {
                    Some(command) if command.is_fixup() => errors.push(LineError {
                        number,
                        line: line.into(),
                        reason: Reason::FixupWithoutPreviousCommit { command },
                    }),
                    _ if !item.is_noop() => fixup_okay = true,
                    _ => {}
                }
            }
            lines.push(Line {
                item,
                text: text.into(),
            });
        }
        Ok((List { lines }, errors))
    }
}

/// Parse `line`, which has no line ending, as `parse_insn_line()` does. `line_with_carriage_return` is `line` with the
/// carriage return that preceded its line feed, if there was one, and `rest_of_input` is the list from the start of
/// `line` on.
fn parse_line(
    line: &[u8],
    line_with_carriage_return: &[u8],
    rest_of_input: &[u8],
    options: &ParseOptions<'_>,
    resolve: &mut dyn Resolve,
) -> Result<std::result::Result<Item, Reason>> {
    let rest = trim_spaces_and_tabs(line);
    if rest.is_empty() || rest.first() == Some(&b'\r') || rest.starts_with(options.comment) {
        return Ok(Ok(Item::Comment(rest.into())));
    }
    let Some((command, after_command)) = Command::ALL
        .iter()
        .find_map(|&command| match_command(command, rest).map(|after| (command, after)))
    else {
        let end = rest
            .iter()
            .position(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
            .unwrap_or(rest.len());
        return Ok(Err(Reason::InvalidCommand {
            name: rest[..end].into(),
        }));
    };
    let arguments = trim_spaces_and_tabs(after_command);
    if matches!(command, Command::Noop | Command::Break) {
        if !arguments.is_empty() {
            // Git prints the arguments from the buffer of the whole list, which goes on past the end of the line.
            return Ok(Err(Reason::UnexpectedArguments {
                command,
                arguments: rest_of_input[line.len() - arguments.len()..].into(),
            }));
        }
        return Ok(Ok(Item::Command(Instruction::new(command, None, BString::default()))));
    }
    if arguments.len() == after_command.len() {
        return Ok(Err(Reason::MissingArguments { command }));
    }
    if matches!(
        command,
        Command::Exec | Command::Label | Command::Reset | Command::UpdateRef
    ) {
        let arg = arguments.as_bstr();
        match command {
            Command::Label if arg == "#" || gix_validate::reference::name_partial(arg).is_err() => {
                return Ok(Err(Reason::InvalidLabel { label: arg.into() }));
            }
            Command::UpdateRef if gix_validate::reference::name_partial(arg).is_err() => {
                return Ok(Err(Reason::InvalidRefName { name: arg.into() }));
            }
            Command::UpdateRef if arg.find_byte(b'/').is_none() => {
                return Ok(Err(Reason::UnqualifiedRefName { name: arg.into() }));
            }
            _ => {}
        }
        return Ok(Ok(Item::Command(Instruction::new(command, None, arg))));
    }

    let mut flags = Flags::default();
    let mut name_and_rest = arguments;
    match command {
        Command::Fixup => {
            if let Some(rest) = name_and_rest.strip_prefix(b"-C") {
                name_and_rest = trim_spaces_and_tabs(rest);
                flags.replace_fixup_message = true;
            } else if let Some(rest) = name_and_rest.strip_prefix(b"-c") {
                name_and_rest = trim_spaces_and_tabs(rest);
                flags.edit_fixup_message = true;
            }
        }
        Command::Merge => {
            if let Some(rest) = name_and_rest.strip_prefix(b"-C") {
                name_and_rest = trim_spaces_and_tabs(rest);
            } else if let Some(rest) = name_and_rest.strip_prefix(b"-c") {
                name_and_rest = trim_spaces_and_tabs(rest);
                flags.edit_merge_message = true;
            } else {
                flags.edit_merge_message = true;
                return Ok(Ok(Item::Command(Instruction {
                    command,
                    flags,
                    commit: None,
                    arg: name_and_rest.into(),
                })));
            }
        }
        _ => {}
    }

    // Git looks for the end of the name in the buffer of the whole list, past the end of the line without its
    // carriage return, so the name of a commit that ends a line takes that carriage return.
    let name_start = line.len() - name_and_rest.len();
    let name_scope = &line_with_carriage_return[name_start..];
    let name_len = name_scope
        .iter()
        .position(|byte| matches!(byte, b' ' | b'\t'))
        .unwrap_or(name_scope.len());
    let name = &name_scope[..name_len];
    let arg = trim_spaces_and_tabs(line.get(name_start + name_len..).unwrap_or_default());
    Ok(match resolve.resolve(name.as_bstr())? {
        Resolved::Nothing => Err(Reason::UnknownCommit { name: name.into() }),
        Resolved::NotACommit { id, kind } => Err(Reason::NotACommit { id, kind }),
        Resolved::Commit { id, is_merge } => {
            if options.rebase && is_merge {
                match command {
                    Command::Fixup | Command::Squash => return Ok(Err(Reason::SquashMergeCommit)),
                    Command::Merge | Command::Drop => {}
                    _ => return Ok(Err(Reason::MergeCommit { command })),
                }
            }
            Ok(Item::Command(Instruction {
                command,
                flags,
                commit: Some(id),
                arg: arg.into(),
            }))
        }
    })
}

/// Return what follows `command` at the start of `rest`, if `rest` starts with its name or its letter followed by
/// whitespace or nothing, as `is_command()` does. The letter is tried only where the name doesn't start `rest`.
fn match_command(command: Command, rest: &[u8]) -> Option<&[u8]> {
    let after = match rest.strip_prefix(command.name().as_bytes()) {
        Some(after) => after,
        None => match command.abbreviation() {
            Some(letter) if rest.first() == Some(&letter) => &rest[1..],
            _ => return None,
        },
    };
    matches!(after.first(), None | Some(b' ' | b'\t' | b'\n' | b'\r')).then_some(after)
}

/// Return `bytes` without the spaces and tabs at its start, as `strspn(bytes, " \t")` skips them.
fn trim_spaces_and_tabs(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(bytes.len());
    &bytes[start..]
}
