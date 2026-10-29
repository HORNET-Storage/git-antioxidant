//! The todo list of an interactive rebase and of a cherry-pick or revert sequence, as `sequencer.c` parses and writes it.
//!
//! Each line of a todo list holds a [command](Command) with its commit and argument, a comment, or nothing:
//!
//! ```text
//! pick 2840bbf4785053a89258960b12681e52f6694635 # topic 1
//! exec make test
//! # a comment
//! ```
//!
//! [`List::parse()`] reads a list as `todo_list_parse_insn_buffer()` and `parse_insn_line()` do, keeping each line as it
//! was written, and [`List::to_bytes()`] writes the list as `todo_list_to_strbuf()` does. For the editor,
//! [`append_help()`] appends the help that `append_todo_help()` appends, [`strip_space()`] removes the comments and blank
//! lines of the edited list as `strbuf_stripspace()` does, and [`missing_commits()`] finds the commits that the edit
//! dropped, as `todo_list_check()` does for `rebase.missingCommitsCheck`.

use bstr::{BStr, BString, ByteSlice};
use gix_error::Result;
use gix_hash::ObjectId;

mod edit;
pub(crate) use edit::is_space;
pub use edit::{MissingCommitsCheck, Start, append_help, missing_commits, missing_commits_report, strip_space};

mod parse;
pub use parse::{ById, LineError, ParseOptions, Reason, Resolve, Resolved};

/// A command of a todo list, in the order of `enum todo_command` in `sequencer.h`, which is also the order in which
/// `parse_insn_line()` tries their names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Command {
    /// `pick`, `p`: use the commit.
    Pick,
    /// `revert`: revert the commit, as a revert sequence does. It has no one-letter form.
    Revert,
    /// `edit`, `e`: use the commit, but stop for amending it.
    Edit,
    /// `reword`, `r`: use the commit, but edit its message.
    Reword,
    /// `fixup`, `f`: meld the commit into the previous one, keeping only the message of the previous one unless
    /// [`Flags::replace_fixup_message`] or [`Flags::edit_fixup_message`] say otherwise.
    Fixup,
    /// `squash`, `s`: meld the commit into the previous one, combining their messages.
    Squash,
    /// `exec`, `x`: run the rest of the line with the shell.
    Exec,
    /// `break`, `b`: stop here, to continue later.
    Break,
    /// `label`, `l`: label the current `HEAD` with a name.
    Label,
    /// `reset`, `t`: reset `HEAD` to a label.
    Reset,
    /// `merge`, `m`: create a merge commit.
    Merge,
    /// `update-ref`, `u`: track a placeholder for a reference that is updated to this position at the end of the rebase.
    UpdateRef,
    /// `noop`: do nothing. It has no one-letter form.
    Noop,
    /// `drop`, `d`: remove the commit.
    Drop,
}

impl Command {
    /// All commands, in the order in which `parse_insn_line()` tries their names.
    pub const ALL: [Command; 14] = [
        Command::Pick,
        Command::Revert,
        Command::Edit,
        Command::Reword,
        Command::Fixup,
        Command::Squash,
        Command::Exec,
        Command::Break,
        Command::Label,
        Command::Reset,
        Command::Merge,
        Command::UpdateRef,
        Command::Noop,
        Command::Drop,
    ];

    /// Return the name of the command, as `command_to_string()` does.
    pub fn name(self) -> &'static str {
        match self {
            Command::Pick => "pick",
            Command::Revert => "revert",
            Command::Edit => "edit",
            Command::Reword => "reword",
            Command::Fixup => "fixup",
            Command::Squash => "squash",
            Command::Exec => "exec",
            Command::Break => "break",
            Command::Label => "label",
            Command::Reset => "reset",
            Command::Merge => "merge",
            Command::UpdateRef => "update-ref",
            Command::Noop => "noop",
            Command::Drop => "drop",
        }
    }

    /// Return the letter that names the command where `rebase.abbreviateCommands` is set, as `command_to_char()` does, or
    /// `None` for `revert` and `noop`, which have none.
    pub fn abbreviation(self) -> Option<u8> {
        Some(match self {
            Command::Pick => b'p',
            Command::Edit => b'e',
            Command::Reword => b'r',
            Command::Fixup => b'f',
            Command::Squash => b's',
            Command::Exec => b'x',
            Command::Break => b'b',
            Command::Label => b'l',
            Command::Reset => b't',
            Command::Merge => b'm',
            Command::UpdateRef => b'u',
            Command::Drop => b'd',
            Command::Revert | Command::Noop => return None,
        })
    }

    /// Return `true` for `fixup` and `squash`, which meld a commit into the previous one, as `is_fixup()` does.
    pub fn is_fixup(self) -> bool {
        matches!(self, Command::Fixup | Command::Squash)
    }

    /// Return `true` for `noop` and `drop`, which do nothing but are counted, as `is_noop()` does for commands.
    pub fn is_noop(self) -> bool {
        matches!(self, Command::Noop | Command::Drop)
    }

    /// Return `true` for the commands that create a commit that isn't a merge, as `is_pick_or_similar()` does.
    pub fn is_pick_or_similar(self) -> bool {
        matches!(
            self,
            Command::Pick | Command::Revert | Command::Edit | Command::Reword | Command::Fixup | Command::Squash
        )
    }

    /// Return `true` for the commands that take a commit, which `parse_insn_line()` resolves, as all commands do but
    /// `exec`, `break`, `label`, `reset`, `update-ref` and `noop`. `merge` takes one only with `-C` or `-c`.
    pub fn takes_commit(self) -> bool {
        !matches!(
            self,
            Command::Exec | Command::Break | Command::Label | Command::Reset | Command::UpdateRef | Command::Noop
        )
    }
}

impl std::fmt::Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The options of `fixup` and `merge` that decide which commit message they use, as the flags of `struct todo_item` hold
/// them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Flags {
    /// `merge -c <commit>`, or `merge` without a commit: open the editor for the message of the merge commit, as
    /// `TODO_EDIT_MERGE_MSG` says. `merge -C <commit>` uses the message of the commit as it is.
    pub edit_merge_message: bool,
    /// `fixup -C`: use the message of this commit instead of the message of the previous one, as
    /// `TODO_REPLACE_FIXUP_MSG` says.
    pub replace_fixup_message: bool,
    /// `fixup -c`: use the message of this commit like `-C`, but open the editor for it, as `TODO_EDIT_FIXUP_MSG` says.
    pub edit_fixup_message: bool,
}

/// A command of a todo list with its commit and its argument, as `struct todo_item` holds it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Instruction {
    /// What to do.
    pub command: Command,
    /// The options of `fixup` and `merge`.
    pub flags: Flags,
    /// The commit that the command applies to, or the original merge commit of `merge -C` and `merge -c`. It is `None`
    /// for commands that take no commit, and for `merge` without `-C` or `-c`.
    pub commit: Option<ObjectId>,
    /// The rest of the line: what follows the commit, like the `# <subject>` that Git writes, the command of `exec`, the
    /// label of `label` and `reset`, the reference of `update-ref`, or the labels of `merge`. It is empty for `break` and
    /// `noop`, and holds no line ending.
    pub arg: BString,
}

impl Instruction {
    /// Create an instruction to apply `command` to `commit` with `arg`, without flags.
    pub fn new(command: Command, commit: Option<ObjectId>, arg: impl Into<BString>) -> Self {
        Instruction {
            command,
            flags: Flags::default(),
            commit,
            arg: arg.into(),
        }
    }
}

/// What a line of a todo list holds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Item {
    /// A command.
    Command(Instruction),
    /// A comment, which starts with the comment string after leading spaces and tabs, or an empty line, held without its
    /// leading spaces and tabs and without its line ending.
    Comment(BString),
    /// A line that Git can't parse, held as it was without its line ending, so that writing the list keeps it for the user
    /// to fix.
    Invalid(BString),
}

impl Item {
    /// Return the instruction of this item, if it is a command.
    pub fn instruction(&self) -> Option<&Instruction> {
        match self {
            Item::Command(instruction) => Some(instruction),
            Item::Comment(_) | Item::Invalid(_) => None,
        }
    }

    /// Return the instruction of this item for changing it, if it is a command.
    pub fn instruction_mut(&mut self) -> Option<&mut Instruction> {
        match self {
            Item::Command(instruction) => Some(instruction),
            Item::Comment(_) | Item::Invalid(_) => None,
        }
    }

    /// Return the command of this item, if it is one.
    pub fn command(&self) -> Option<Command> {
        self.instruction().map(|instruction| instruction.command)
    }

    /// Return `true` if this item is counted as a command, as `count_commands()` counts all items but comments, which
    /// includes the lines that Git can't parse.
    pub fn is_counted(&self) -> bool {
        !matches!(self, Item::Comment(_))
    }

    /// Return `true` if this item does nothing, as `is_noop()` tells for comments, lines that can't be parsed, `noop`
    /// and `drop`.
    pub fn is_noop(&self) -> bool {
        self.command().is_none_or(Command::is_noop)
    }

    /// Append this item to `out` as a line with a line feed in the way `todo_list_to_strbuf()` writes it with `format`.
    pub fn write_to(&self, out: &mut BString, format: &mut Format<'_>) -> Result<()> {
        let instruction = match self {
            Item::Comment(text) | Item::Invalid(text) => {
                out.extend_from_slice(text);
                out.push(b'\n');
                return Ok(());
            }
            Item::Command(instruction) => instruction,
        };
        match instruction.command.abbreviation() {
            Some(letter) if format.abbreviate_commands => out.push(letter),
            _ => out.extend_from_slice(instruction.command.name().as_bytes()),
        }
        if let Some(commit) = &instruction.commit {
            match instruction.command {
                Command::Fixup if instruction.flags.edit_fixup_message => out.extend_from_slice(b" -c"),
                Command::Fixup if instruction.flags.replace_fixup_message => out.extend_from_slice(b" -C"),
                Command::Merge if instruction.flags.edit_merge_message => out.extend_from_slice(b" -c"),
                Command::Merge => out.extend_from_slice(b" -C"),
                _ => {}
            }
            out.push(b' ');
            match format.shorten_ids.as_mut() {
                Some(shorten) => out.extend_from_slice(&shorten(commit)?),
                None => crate::file::push_hex(out, commit),
            }
        }
        if !instruction.arg.is_empty() {
            out.push(b' ');
            out.extend_from_slice(&instruction.arg);
        }
        out.push(b'\n');
        Ok(())
    }
}

/// A function that returns the name of a commit for a todo list, like the shortest unique abbreviation of its id.
pub type ShortenId<'a> = dyn FnMut(&gix_hash::oid) -> Result<BString> + 'a;

/// How [`Item::write_to()`] and [`List::to_bytes()`] write commands and commits, as the flags of
/// `todo_list_to_strbuf()` say.
#[derive(Default)]
pub struct Format<'a> {
    /// Write the letter of each command that has one, as `TODO_LIST_ABBREVIATE_CMDS` does where
    /// `rebase.abbreviateCommands` is set.
    pub abbreviate_commands: bool,
    /// Write each commit with the name that this function returns, as `TODO_LIST_SHORTEN_IDS` makes Git write the
    /// shortest unique abbreviation of the commit for the editor. The full hexadecimal id is written if it is `None`.
    pub shorten_ids: Option<&'a mut ShortenId<'a>>,
}

/// A line of a todo list: what it holds, and how the list holds it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Line {
    /// What the line holds.
    pub item: Item,
    /// The line as the file of the list holds it, with its line ending, which the last line of a file may lack.
    ///
    /// Git writes these bytes when it saves its progress through a list it read (`save_todo()`), and
    /// [`List::to_bytes()`] writes `item` anew. A line that is created from an item holds the item as
    /// `todo_list_to_strbuf()` writes it without abbreviations.
    pub text: BString,
}

impl Line {
    /// Create a line that holds `item` as `todo_list_to_strbuf()` writes it with full commands and commits.
    pub fn new(item: Item) -> Self {
        let mut text = BString::default();
        item.write_to(&mut text, &mut Format::default())
            .expect("writing full ids calls no function that could fail");
        Line { item, text }
    }
}

/// A todo list.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct List {
    /// The lines of the list, in order.
    pub lines: Vec<Line>,
}

impl List {
    /// Create a list of `items`, each held on a line as `todo_list_to_strbuf()` writes it with full commands and commits.
    pub fn from_items(items: impl IntoIterator<Item = Item>) -> Self {
        List {
            lines: items.into_iter().map(Line::new).collect(),
        }
    }

    /// Return the items of the list, in order.
    pub fn items(&self) -> impl Iterator<Item = &Item> + '_ {
        self.lines.iter().map(|line| &line.item)
    }

    /// Return the instructions of the list, in order, without its comments and invalid lines.
    pub fn instructions(&self) -> impl Iterator<Item = &Instruction> + '_ {
        self.items().filter_map(Item::instruction)
    }

    /// Return the number of commands of the list, as `count_commands()` counts them: all lines but comments and empty
    /// lines, which includes the lines that Git can't parse.
    pub fn count_commands(&self) -> usize {
        self.items().filter(|item| item.is_counted()).count()
    }

    /// Return `true` if the list has no lines.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Return the list written as `todo_list_to_strbuf()` writes it with `format`: each item on a line ending with a line
    /// feed.
    pub fn to_bytes(&self, mut format: Format<'_>) -> Result<BString> {
        let mut out = BString::default();
        for line in &self.lines {
            line.item.write_to(&mut out, &mut format)?;
        }
        Ok(out)
    }

    /// Return the lines of the list as they are held, which are the bytes that `save_todo()` writes to the file of the
    /// list, starting at its current command.
    pub fn text(&self) -> BString {
        let mut out = BString::default();
        for line in &self.lines {
            out.extend_from_slice(&line.text);
        }
        out
    }

    /// Return the command of the first item from `index` on that doesn't do nothing, as `peek_command()` does, or `None` if
    /// there is none.
    pub fn peek_command(&self, index: usize) -> Option<Command> {
        self.lines
            .get(index..)
            .into_iter()
            .flatten()
            .find(|line| !line.item.is_noop())
            .and_then(|line| line.item.command())
    }
}

/// Split `input` into its lines with their line endings, the last of which may lack one.
fn lines(input: &[u8]) -> impl Iterator<Item = &[u8]> + '_ {
    input.lines_with_terminator()
}

/// Return `line` without its line feed and a carriage return before it, as `todo_list_parse_insn_buffer()` strips them.
fn strip_line_ending(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    if line.is_empty() {
        line
    } else {
        line.strip_suffix(b"\r").unwrap_or(line)
    }
}

/// Return the comment string `#`, which Git uses unless `core.commentChar` or `core.commentString` say otherwise.
pub fn default_comment() -> &'static BStr {
    "#".into()
}
