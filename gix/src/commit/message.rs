//! Clean the message of a commit as `git commit` does, as `cleanup_message()` in `sequencer.c`, `strbuf_stripspace()` in
//! `strbuf.c` and `wt_status_locate_end()` in `wt-status.c` clean it.
//!
//! Like Git, this counts a space, a tab, a carriage return and a line feed as whitespace.

use crate::{
    Result,
    bstr::{BStr, BString, ByteSlice},
    error::bail,
};

/// The scissors line, which follows the comment prefix and a space. Without an editor, `git commit` cuts no message at
/// it, but `git merge` writes it into `MERGE_MSG` above its comments with `commit.cleanup=scissors`.
pub const CUT_LINE: &str = "------------------------ >8 ------------------------\n";

/// The comment prefixes that `core.commentChar=auto` chooses from, in the order Git tries them.
const AUTO_CANDIDATES: &[u8] = b"#;@!$%^&|:";

/// How the message of a commit is cleaned, as `--cleanup` and `commit.cleanup` name it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cleanup {
    /// `default`: [`Strip`](Cleanup::Strip) if the message was edited in an editor, and
    /// [`Whitespace`](Cleanup::Whitespace) otherwise.
    #[default]
    Default,
    /// `verbatim`: the message stays as it is.
    Verbatim,
    /// `whitespace`: trailing whitespace is removed from each line, leading and trailing empty lines are removed, and
    /// consecutive empty lines become one.
    Whitespace,
    /// `strip`: like [`Whitespace`](Cleanup::Whitespace), and the lines that start with the comment prefix are removed.
    Strip,
    /// `scissors`: like [`Whitespace`](Cleanup::Whitespace), and if the message was edited in an editor, the scissors
    /// line and everything below it are removed as well.
    Scissors,
}

/// What cleaning does with a message, as Git's `enum commit_msg_cleanup_mode` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// The message stays as it is: `COMMIT_MSG_CLEANUP_NONE`.
    None,
    /// Whitespace is cleaned up: `COMMIT_MSG_CLEANUP_SPACE`.
    Space,
    /// Whitespace is cleaned up and comment lines are removed: `COMMIT_MSG_CLEANUP_ALL`.
    All,
    /// The message is cut at the scissors line, then whitespace is cleaned up: `COMMIT_MSG_CLEANUP_SCISSORS`.
    Scissors,
}

impl Cleanup {
    /// Return what this cleanup does with a message, which was edited in an editor if `edited` is `true`, as
    /// `get_cleanup_mode()` in `sequencer.c` decides it.
    pub fn mode(self, edited: bool) -> Mode {
        match self {
            Cleanup::Default if edited => Mode::All,
            Cleanup::Scissors if edited => Mode::Scissors,
            Cleanup::Default | Cleanup::Scissors | Cleanup::Whitespace => Mode::Space,
            Cleanup::Verbatim => Mode::None,
            Cleanup::Strip => Mode::All,
        }
    }
}

/// The prefix of the comment lines of commit messages, as `core.commentChar` and `core.commentString` set it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CommentPrefix {
    /// Lines that start with these bytes are comments. Git's default is `#`.
    Fixed(BString),
    /// `auto`: the prefix is chosen for each message, as [`CommentPrefix::for_message()`] chooses it.
    Auto,
}

impl Default for CommentPrefix {
    fn default() -> Self {
        CommentPrefix::Fixed("#".into())
    }
}

impl CommentPrefix {
    /// Return the prefix of the comment lines of `message`: the fixed prefix, or with `auto`, `#` if `message` contains
    /// no `#`, and otherwise the first of `#;@!$%^&|:` that starts neither `message` nor a line of it, as
    /// `adjust_comment_line_char()` in `builtin/commit.c` chooses it for the message that `git commit` prepared.
    ///
    /// ### Errors
    ///
    /// With `auto`, a message whose lines start with each of these characters is refused, as Git refuses it.
    pub fn for_message(&self, message: &BStr) -> Result<BString> {
        match self {
            CommentPrefix::Fixed(prefix) => Ok(prefix.clone()),
            CommentPrefix::Auto => auto_prefix(message),
        }
    }
}

/// Choose the comment prefix of `auto` for `message`, as `adjust_comment_line_char()` does: Git looks at the first byte of
/// `message` and at the byte after each carriage return and line feed, up to the first NUL byte.
fn auto_prefix(message: &BStr) -> Result<BString> {
    if !message.contains(&b'#') {
        return Ok("#".into());
    }
    let text = message.find_byte(0).map_or(message, |nul| &message[..nul]);
    let mut used = [false; AUTO_CANDIDATES.len()];
    let mut mark = |byte: u8| {
        if let Some(index) = AUTO_CANDIDATES.iter().position(|candidate| *candidate == byte) {
            used[index] = true;
        }
    };
    if let Some(first) = text.first() {
        mark(*first);
    }
    for pair in text.windows(2) {
        if matches!(pair[0], b'\n' | b'\r') {
            mark(pair[1]);
        }
    }
    match AUTO_CANDIDATES.iter().zip(used).find(|(_, used)| !used) {
        Some((candidate, _)) => Ok(BString::from(vec![*candidate])),
        None => bail!(crate::error::validation(
            "Unable to select a comment character that is not used in the current commit message"
        )),
    }
}

/// Return `message` cleaned as `mode` says, with `comment_prefix` as the prefix of comment lines, as `cleanup_message()` in
/// `sequencer.c` cleans the message of a commit that `git commit` makes without `--verbose`.
pub fn clean(message: &BStr, mode: Mode, comment_prefix: &BStr) -> BString {
    let message = match mode {
        Mode::Scissors => &message[..scissors_position(message, comment_prefix)],
        Mode::None | Mode::Space | Mode::All => message,
    };
    match mode {
        Mode::None => message.to_owned(),
        Mode::All => strip_space(message, Some(comment_prefix)),
        Mode::Space | Mode::Scissors => strip_space(message, None),
    }
}

/// Return the length of `message` before its scissors line, which is `comment_prefix`, a space and [`CUT_LINE`] at the start
/// of a line, or the length of `message` if it has none, as `wt_status_locate_end()` in `wt-status.c` finds it.
pub fn scissors_position(message: &BStr, comment_prefix: &BStr) -> usize {
    let mut line = BString::from("\n");
    line.extend_from_slice(comment_prefix);
    line.push(b' ');
    line.extend_from_slice(CUT_LINE.as_bytes());
    if message.starts_with(&line[1..]) {
        0
    } else {
        message.find(&line).map_or(message.len(), |position| position + 1)
    }
}

/// Return `message` with the trailing whitespace of each line removed, without leading and trailing empty lines, with
/// consecutive empty lines turned into one and each line ending in a line feed, and without the lines that start with
/// `comment_prefix` if it is given, as `strbuf_stripspace()` in `strbuf.c` does.
pub fn strip_space(message: &BStr, comment_prefix: Option<&BStr>) -> BString {
    let mut out = BString::default();
    let mut empty_lines = 0;
    for line in message.lines_with_terminator() {
        if comment_prefix.is_some_and(|prefix| line.starts_with(prefix)) {
            continue;
        }
        let line = trim_trailing_space(line);
        if line.is_empty() {
            empty_lines += 1;
            continue;
        }
        if empty_lines > 0 && !out.is_empty() {
            out.push(b'\n');
        }
        empty_lines = 0;
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out
}

/// Return `true` if `message`, cleaned as `mode` says, has nothing but whitespace and lines that start with
/// `Signed-off-by: `, which makes `git commit` refuse it unless `--allow-empty-message` is given, as `message_is_empty()`
/// in `sequencer.c` decides it. A message kept [verbatim](Mode::None) is only empty if it has no byte at all.
pub fn is_empty(message: &BStr, mode: Mode) -> bool {
    if mode == Mode::None && !message.is_empty() {
        return false;
    }
    message
        .split(|byte| *byte == b'\n')
        .all(|line| line.starts_with(b"Signed-off-by: ") || line.iter().all(|byte| is_space(*byte)))
}

/// Return the message of the reflog entry that records `message` after `action`: the action, a colon and the first line of
/// `message`, normalized as `update_head_with_reflog()` in `sequencer.c` and the references of Git write it.
#[cfg(feature = "index")]
pub(crate) fn reflog_message(action: &BStr, message: &BStr) -> BString {
    let mut raw = BString::from(action);
    raw.extend_from_slice(b": ");
    raw.extend_from_slice(message.lines_with_terminator().next().unwrap_or_default());
    normalize_reflog_message(raw.as_bstr())
}

/// Return `message` as Git writes it into a reflog, as `copy_reflog_msg()` in `refs.c` does: without leading and trailing
/// whitespace, and with each run of whitespace turned into a single space.
#[cfg(feature = "index")]
pub(crate) fn normalize_reflog_message(message: &BStr) -> BString {
    let mut out = BString::default();
    let mut was_space = true;
    for &byte in message.iter().take_while(|byte| **byte != 0) {
        let space = is_space(byte);
        if was_space && space {
            continue;
        }
        was_space = space;
        out.push(if space { b' ' } else { byte });
    }
    while out.last().is_some_and(|byte| is_space(*byte)) {
        out.pop();
    }
    out
}

/// Return `true` for the bytes that Git's `isspace()` counts as whitespace.
fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

/// Return `line` without its trailing whitespace, including its line feed.
fn trim_trailing_space(line: &[u8]) -> &[u8] {
    let end = line
        .iter()
        .rposition(|byte| !is_space(*byte))
        .map_or(0, |position| position + 1);
    &line[..end]
}
