use std::{
    io::{IsTerminal, Read, Write},
    path::Path,
};

use gix::{
    Result,
    bstr::{BStr, BString, ByteSlice},
    error::{OptionExt, ResultExt, bail, message},
};

/// Note that this is a quick implementation of commit signature verification that ignores a lot of what
/// git does and can do, while focussing on the gist of it.
/// For this to go into `gix`, one will have to implement many more options and various validation programs.
pub fn verify(repo: gix::Repository, rev_spec: Option<&str>) -> Result<()> {
    let rev_spec = rev_spec.unwrap_or("HEAD");
    let commit = repo
        .rev_parse_single(format!("{rev_spec}^{{commit}}").as_str())?
        .object()?
        .into_commit();
    let outcome = commit
        .verify_signature()
        .or_raise(|| message("Could not verify commit signature"))?
        .ok_or_raise(|| message!("Commit at {rev_spec} is not signed"))?;
    std::io::stderr().write_all(&outcome.output).or_error()?;
    if !outcome.is_valid() {
        bail!("Commit at {rev_spec} has an invalid or untrusted signature");
    }
    Ok(())
}

/// Sign an unsigned commit using repository configuration and print its object ID.
pub fn sign(repo: gix::Repository, rev_spec: Option<&str>, mut out: impl std::io::Write) -> Result<()> {
    let rev_spec = rev_spec.unwrap_or("HEAD");
    let commit = repo
        .rev_parse_single(format!("{rev_spec}^{{commit}}").as_str())?
        .object()?
        .into_commit();
    if commit.signature()?.is_some() {
        gix::trace::info!("The commit {id} is already signed, did nothing", id = commit.id);
        writeln!(out, "{id}", id = commit.id).or_error()?;
        return Ok(());
    }

    let signed = commit.signed()?;
    writeln!(out, "{id}", id = signed.id).or_error()?;

    Ok(())
}

pub fn describe(
    mut repo: gix::Repository,
    rev_spec: Option<&str>,
    mut out: impl std::io::Write,
    mut err: impl std::io::Write,
    describe::Options {
        all_tags,
        all_refs,
        first_parent,
        always,
        statistics,
        max_candidates,
        long_format,
        dirty_suffix,
    }: describe::Options,
) -> Result<()> {
    repo.object_cache_size_if_unset(4 * 1024 * 1024);
    let commit = match rev_spec {
        Some(spec) => repo.rev_parse_single(spec)?.object()?.try_into_commit()?,
        None => repo.head_commit()?,
    };
    use gix::commit::describe::SelectRef::*;
    let select_ref = if all_refs {
        AllRefs
    } else if all_tags {
        AllTags
    } else {
        Default::default()
    };
    let resolution = commit
        .describe()
        .names(select_ref)
        .traverse_first_parent(first_parent)
        .id_as_fallback(always)
        .max_candidates(max_candidates)
        .try_resolve()?
        .ok_or_raise(|| message!("Did not find a single candidate ref for naming id '{}'", commit.id))?;

    if statistics {
        writeln!(err, "traversed {} commits", resolution.outcome.commits_seen).or_error()?;
    }

    let mut describe_id = resolution.format_with_dirty_suffix(dirty_suffix)?;
    describe_id.long(long_format);

    writeln!(out, "{describe_id}").or_error()?;
    Ok(())
}

pub mod describe {
    #[derive(Debug, Clone)]
    pub struct Options {
        pub all_tags: bool,
        pub all_refs: bool,
        pub first_parent: bool,
        pub always: bool,
        pub long_format: bool,
        pub statistics: bool,
        pub max_candidates: usize,
        pub dirty_suffix: Option<String>,
    }
}

/// Commit the index as `git commit` does without an editor, as `cmd_commit()` in Git's `builtin/commit.c` does, and print
/// the summary of the new commit as `print_commit_summary()` in its `sequencer.c` prints it.
///
/// The message is made of the paragraphs of `-m`, or read from the file of `-F`, or without either, it is that of a merge
/// in progress or of the amended commit, as `git commit --no-edit` takes it. `GIT_REFLOG_ACTION` replaces the action of
/// the reflog messages, as in Git.
pub fn index(
    repo: gix::Repository,
    mut out: impl std::io::Write,
    mut err: impl std::io::Write,
    index::Options {
        messages,
        file,
        amend,
        allow_empty,
        allow_empty_message,
        no_verify,
        cleanup,
    }: index::Options,
) -> Result<()> {
    if !messages.is_empty() && file.is_some() {
        bail!(gix::error::validation("options '-m' and '-F' cannot be used together"));
    }
    let cleanup = cleanup.as_deref().map(cleanup_mode).transpose()?;
    let message = match file {
        Some(file) => Some(read_message(&file, &mut err)?),
        None => (!messages.is_empty()).then(|| join_paragraphs(&messages)),
    };
    let outcome = repo.commit_index(gix::commit::index::Options {
        message,
        cleanup,
        amend,
        allow_empty,
        allow_empty_message,
        no_verify,
        reflog_action: super::reflog_action(),
        ..Default::default()
    })?;
    print_summary(&repo, &outcome, amend, &mut out)
}

/// Options for committing the index with [`index()`].
pub mod index {
    use std::path::PathBuf;

    use gix::bstr::BString;

    /// The options of `gix commit`, named after those of `git commit`.
    #[derive(Debug, Clone, Default)]
    pub struct Options {
        /// The paragraphs of the message, one for each `-m`, which are joined as `git commit` joins them.
        pub messages: Vec<BString>,
        /// The file to read the message from, as `-F` names it, or `-` for standard input.
        pub file: Option<PathBuf>,
        /// If `true`, replace the commit of `HEAD`, as `--amend` does.
        pub amend: bool,
        /// If `true`, allow a commit that records the tree of its parent, as `--allow-empty` does.
        pub allow_empty: bool,
        /// If `true`, allow a message that is empty after cleaning, as `--allow-empty-message` does.
        pub allow_empty_message: bool,
        /// If `true`, skip the `pre-commit` and `commit-msg` hooks, as `--no-verify` does.
        pub no_verify: bool,
        /// How to clean the message, as `--cleanup` names it, or `None` for `commit.cleanup`.
        pub cleanup: Option<String>,
    }
}

/// Parse `value` as `--cleanup` takes it, failing with Git's message as `get_cleanup_mode()` in its `sequencer.c` does.
pub(crate) fn cleanup_mode(value: &str) -> Result<gix::commit::message::Cleanup> {
    use gix::commit::message::Cleanup;
    Ok(match value {
        "default" => Cleanup::Default,
        "verbatim" => Cleanup::Verbatim,
        "whitespace" => Cleanup::Whitespace,
        "strip" => Cleanup::Strip,
        "scissors" => Cleanup::Scissors,
        _ => bail!(message!("Invalid cleanup mode {value}").validation()),
    })
}

/// Join the paragraphs of `-m` as `opt_parse_m()` in Git's `builtin/commit.c` joins them: each ends with a line feed and
/// follows the one before it after an empty line.
fn join_paragraphs(paragraphs: &[BString]) -> BString {
    let mut message = BString::default();
    for paragraph in paragraphs {
        if !message.is_empty() {
            message.push(b'\n');
        }
        message.extend_from_slice(paragraph);
        if message.last().is_some_and(|byte| *byte != b'\n') {
            message.push(b'\n');
        }
    }
    message
}

/// Read the message of `-F <file>`, or from standard input if `file` is `-`, as `prepare_to_commit()` in Git's
/// `builtin/commit.c` reads it.
fn read_message(file: &Path, err: &mut dyn std::io::Write) -> Result<BString> {
    if file.as_os_str() != "-" {
        return std::fs::read(file)
            .map(Into::into)
            .or_raise(|| message!("could not read log file '{}'", file.display()));
    }
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        writeln!(err, "(reading log message from standard input)").or_error()?;
    }
    let mut buf = Vec::new();
    stdin
        .read_to_end(&mut buf)
        .or_raise(|| message("could not read log from standard input"))?;
    Ok(buf.into())
}

/// Print the summary of the commit that `outcome` describes as `print_commit_summary()` in Git's `sequencer.c` prints it
/// after `git commit`: its branch, abbreviated id and subject, its author unless that is the committer, the date of the
/// author after `--amend`, and unless it is a merge, the short statistics and the summary of its changes against its
/// first parent, with renames found as `git diff -M` finds them.
pub(crate) fn print_summary(
    repo: &gix::Repository,
    outcome: &gix::commit::index::Outcome,
    amend: bool,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    let commit = repo.find_commit(outcome.commit_id)?;
    let decoded = commit.decode()?;
    let reference = outcome.reference.as_bstr();
    let branch = if reference == "HEAD" {
        "detached HEAD".into()
    } else {
        reference.strip_prefix(b"refs/heads/").unwrap_or(reference).as_bstr()
    };
    let root = if outcome.previous.is_none() {
        " (root-commit)"
    } else {
        ""
    };
    let mut header = BString::from("[");
    header.extend_from_slice(branch);
    header.extend_from_slice(format!("{root} {}] ", commit.id().shorten_or_id()).as_bytes());
    header.extend_from_slice(&subject(decoded.message));
    let author = decoded.author()?;
    let committer = decoded.committer()?;
    if (author.name, author.email) != (committer.name, committer.email) {
        header.extend_from_slice(b"\n Author: ");
        header.extend_from_slice(author.name);
        header.extend_from_slice(b" <");
        header.extend_from_slice(author.email);
        header.push(b'>');
    }
    if amend {
        header.extend_from_slice(b"\n Date: ");
        header.extend_from_slice(
            author
                .time()?
                .format_or_unix(gix::date::time::format::DEFAULT)
                .as_bytes(),
        );
    }
    header.push(b'\n');
    out.write_all(&header).or_error()?;

    // Like `log_tree_diff()`, show no changes for a merge without diff options.
    let mut parents = decoded.parents();
    let first_parent = parents.next();
    if parents.next().is_some() {
        return Ok(());
    }
    let old_tree = match first_parent {
        Some(parent_id) => repo.find_commit(parent_id)?.tree()?,
        None => repo.empty_tree(),
    };
    super::diffstat::write_shortstat_and_summary(repo, &old_tree, &commit.tree()?, out)
}

/// Return the subject of a commit with `message`, as `format_subject()` in Git's `pretty.c` makes it for `%s` and
/// `--oneline`: the lines of the first paragraph after leading blank lines, without their trailing whitespace and joined
/// by spaces.
pub(crate) fn subject(message: &BStr) -> BString {
    let mut subject = BString::default();
    let lines = message
        .lines_with_terminator()
        .map(trim_end_space)
        .skip_while(|line| line.is_empty())
        .take_while(|line| !line.is_empty());
    for line in lines {
        if !subject.is_empty() {
            subject.push(b' ');
        }
        subject.extend_from_slice(line);
    }
    subject
}

/// Return `line` without the whitespace that Git's `isspace()` sees at its end, as `is_blank_line()` in Git's `pretty.c`
/// measures it.
fn trim_end_space(line: &[u8]) -> &[u8] {
    let end = line
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        .map_or(0, |position| position + 1);
    &line[..end]
}
