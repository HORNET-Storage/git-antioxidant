use std::{
    borrow::Cow,
    io::{IsTerminal, Read, Write},
    path::Path,
};

use gix::{
    Result,
    bstr::{BStr, BString, ByteSlice},
    error::{OptionExt, ResultExt, bail, message},
    object::tree::diff::ChangeDetached,
    objs::tree::EntryMode,
    prelude::TreeDiffChangeExt,
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
fn cleanup_mode(value: &str) -> Result<gix::commit::message::Cleanup> {
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
fn print_summary(
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
    let new_tree = commit.tree()?;
    let mut options = gix::diff::Options::default();
    options.track_rewrites(Some(gix::diff::Rewrites::default()));
    let mut changes = repo.diff_tree_to_tree(&old_tree, &new_tree, options)?;
    changes.retain(|change| !change.entry_mode().is_tree());
    if changes.is_empty() {
        return Ok(());
    }
    changes.sort_by(|a, b| a.location().cmp(b.location()));

    let mut resource_cache = repo.diff_resource_cache_for_tree_diff()?;
    let (mut insertions, mut deletions) = (0_u64, 0_u64);
    for change in &changes {
        let (source_mode, _) = change.source_entry_mode_and_id();
        if !(change.entry_mode().is_blob_or_symlink() && source_mode.is_blob_or_symlink()) {
            continue;
        }
        if let Some(counts) = change.attach(repo, repo).diff(&mut resource_cache)?.line_counts()? {
            insertions += u64::from(counts.insertions);
            deletions += u64::from(counts.removals);
        }
        resource_cache.clear_resource_cache_keep_allocation();
    }
    let files = changes.len();
    write!(out, " {files} file{} changed", if files == 1 { "" } else { "s" }).or_error()?;
    if insertions != 0 || deletions == 0 {
        write!(
            out,
            ", {insertions} insertion{}(+)",
            if insertions == 1 { "" } else { "s" }
        )
        .or_error()?;
    }
    if deletions != 0 || insertions == 0 {
        write!(
            out,
            ", {deletions} deletion{}(-)",
            if deletions == 1 { "" } else { "s" }
        )
        .or_error()?;
    }
    writeln!(out).or_error()?;

    let quote_path = repo.config_snapshot().boolean("core.quotePath").unwrap_or(true);
    for change in &changes {
        out.write_all(&change_summary(change, quote_path)).or_error()?;
    }
    Ok(())
}

/// Return the lines that `diff_summary()` in Git's `diff.c` writes for `change` with `git diff --summary`, with paths
/// quoted as `core.quotePath` says with `quote_path`.
fn change_summary(change: &ChangeDetached, quote_path: bool) -> BString {
    let mut lines = BString::default();
    match change {
        ChangeDetached::Addition {
            location, entry_mode, ..
        } => {
            lines.extend_from_slice(format!(" create mode {:06o} ", entry_mode.value()).as_bytes());
            lines.extend_from_slice(&quote_c_style(location.as_bstr(), quote_path));
            lines.push(b'\n');
        }
        ChangeDetached::Deletion {
            location, entry_mode, ..
        } => {
            lines.extend_from_slice(format!(" delete mode {:06o} ", entry_mode.value()).as_bytes());
            lines.extend_from_slice(&quote_c_style(location.as_bstr(), quote_path));
            lines.push(b'\n');
        }
        ChangeDetached::Modification {
            location,
            previous_entry_mode,
            entry_mode,
            ..
        } => mode_change(
            &mut lines,
            *previous_entry_mode,
            *entry_mode,
            Some(location.as_bstr()),
            quote_path,
        ),
        ChangeDetached::Rewrite {
            source_location,
            source_entry_mode,
            location,
            entry_mode,
            diff,
            copy,
            ..
        } => {
            let similarity = diff.map_or(100, |diff| (diff.similarity * 100.0) as u32);
            lines.extend_from_slice(if *copy { " copy " } else { " rename " }.as_bytes());
            lines.extend_from_slice(&pprint_rename(
                source_location.as_bstr(),
                location.as_bstr(),
                quote_path,
            ));
            lines.extend_from_slice(format!(" ({similarity}%)\n").as_bytes());
            mode_change(&mut lines, *source_entry_mode, *entry_mode, None, quote_path);
        }
    }
    lines
}

/// Append the line that `show_mode_change()` in Git's `diff.c` writes if `old` and `new` differ to `lines`, followed by
/// `path` if it is given.
fn mode_change(lines: &mut BString, old: EntryMode, new: EntryMode, path: Option<&BStr>, quote_path: bool) {
    if old == new {
        return;
    }
    lines.extend_from_slice(format!(" mode change {:06o} => {:06o}", old.value(), new.value()).as_bytes());
    if let Some(path) = path {
        lines.push(b' ');
        lines.extend_from_slice(&quote_c_style(path, quote_path));
    }
    lines.push(b'\n');
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

/// Return `path` as `quote_c_style()` in Git's `quote.c` writes it: unchanged if it needs no quotes, and otherwise in
/// double quotes, with C escapes for `"`, `\` and the control characters that have them, and octal escapes for the other
/// control characters and, if `quote_path` is set as `core.quotePath` is by default, for the bytes above 0x7f.
fn quote_c_style(path: &BStr, quote_path: bool) -> Cow<'_, BStr> {
    let needs_quote = |byte: u8| byte < 0x20 || matches!(byte, b'"' | b'\\' | 0x7f) || (quote_path && byte >= 0x80);
    if !path.iter().any(|byte| needs_quote(*byte)) {
        return Cow::Borrowed(path);
    }
    let mut quoted = BString::from("\"");
    for &byte in path.iter() {
        if !needs_quote(byte) {
            quoted.push(byte);
            continue;
        }
        quoted.push(b'\\');
        match byte {
            0x07 => quoted.push(b'a'),
            0x08 => quoted.push(b'b'),
            b'\t' => quoted.push(b't'),
            b'\n' => quoted.push(b'n'),
            0x0b => quoted.push(b'v'),
            0x0c => quoted.push(b'f'),
            b'\r' => quoted.push(b'r'),
            b'"' | b'\\' => quoted.push(byte),
            _ => quoted.extend_from_slice(&[b'0' + (byte >> 6), b'0' + ((byte >> 3) & 7), b'0' + (byte & 7)]),
        }
    }
    quoted.push(b'"');
    Cow::Owned(quoted)
}

/// Return the source `a` and the destination `b` of a rename as `pprint_rename()` in Git's `diff.c` writes them: with
/// their common leading directories and trailing path components written once, as in `dir/{old => new}/file`, unless
/// one of them needs quotes.
fn pprint_rename(a: &BStr, b: &BStr, quote_path: bool) -> BString {
    let (quoted_a, quoted_b) = (quote_c_style(a, quote_path), quote_c_style(b, quote_path));
    if matches!(quoted_a, Cow::Owned(_)) || matches!(quoted_b, Cow::Owned(_)) {
        let mut names = quoted_a.into_owned();
        names.extend_from_slice(b" => ");
        names.extend_from_slice(&quoted_b);
        return names;
    }

    let mut prefix = 0;
    for (index, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        if x != y {
            break;
        }
        if *x == b'/' {
            prefix = index + 1;
        }
    }
    // Compare from the ends, which match as both strings end there, down to the slash of the common prefix if there is
    // one. The suffix starts at a slash.
    let byte_at = |path: &BStr, index: usize| path.get(index).copied().unwrap_or(0);
    let floor = prefix.saturating_sub(1);
    let (mut i, mut j) = (a.len(), b.len());
    let mut suffix = 0;
    while i >= floor && j >= floor && byte_at(a, i) == byte_at(b, j) {
        if byte_at(a, i) == b'/' {
            suffix = a.len() - i;
        }
        if i == 0 || j == 0 {
            break;
        }
        i -= 1;
        j -= 1;
    }

    let a_middle = a.len().saturating_sub(prefix + suffix);
    let b_middle = b.len().saturating_sub(prefix + suffix);
    let mut names = BString::default();
    if prefix + suffix > 0 {
        names.extend_from_slice(&a[..prefix]);
        names.push(b'{');
    }
    names.extend_from_slice(&a[prefix..prefix + a_middle]);
    names.extend_from_slice(b" => ");
    names.extend_from_slice(&b[prefix..prefix + b_middle]);
    if prefix + suffix > 0 {
        names.push(b'}');
        names.extend_from_slice(&a[a.len() - suffix..]);
    }
    names
}
