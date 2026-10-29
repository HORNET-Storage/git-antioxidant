//! The statistics and the summary of the changes between two trees, as `diff_flush()` in Git's `diff.c` writes them: for
//! `--stat --summary`, or for `--compact-summary` if `merge.stat` asks for it, once `git merge` moved `HEAD`, and for
//! `--shortstat --summary` after `git commit`.

use std::borrow::Cow;

use gix::{
    ObjectId, Result,
    bstr::{BStr, BString, ByteSlice},
    error::ResultExt,
    hash::oid,
    object::tree::diff::ChangeDetached,
    objs::tree::EntryMode,
    prelude::TreeDiffChangeExt,
};

/// How `git merge` shows the changes of a merge, as `merge.stat` chooses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    /// `--stat --summary`, the default: a line with a graph for each file and the totals, followed by a line for each
    /// file that was created, deleted or renamed or whose mode changed.
    StatAndSummary,
    /// `--compact-summary`, which `merge.stat=compact` asks for: the statistics, with the creations, deletions and changes
    /// of the mode noted after the names of the files.
    CompactSummary,
}

impl Format {
    /// Return how `merge.stat`, or `merge.diffstat` if it isn't set, asks `git merge` in `repo` to show the changes, as
    /// `git_merge_config()` in Git's `builtin/merge.c` reads them, or `None` if they aren't shown: `false` turns them off,
    /// `compact` asks for [`CompactSummary`](Format::CompactSummary), and any other value for the default.
    pub(crate) fn configured(repo: &gix::Repository) -> Option<Self> {
        let config = repo.config_snapshot();
        for key in ["merge.stat", "merge.diffstat"] {
            match config.boolean(key) {
                Some(false) => return None,
                Some(true) => return Some(Format::StatAndSummary),
                None => match config.string(key) {
                    Some(value) if value == "compact" => return Some(Format::CompactSummary),
                    Some(_) => return Some(Format::StatAndSummary),
                    None => {}
                },
            }
        }
        Some(Format::StatAndSummary)
    }
}

/// A file of the statistics, as `struct diffstat_file` in Git's `diff.c` holds it.
struct File {
    /// The name as `fill_print_name()` makes it.
    name: BString,
    /// Whether the file is binary, for which the sizes of its blobs are shown instead of its lines.
    binary: bool,
    /// The added lines, or the size of the new blob of a binary file.
    added: u64,
    /// The deleted lines, or the size of the old blob of a binary file.
    deleted: u64,
}

/// The mode and the object of a side of a change.
type Side<'a> = Option<(EntryMode, &'a oid)>;

/// Write the statistics of the changes from the tree of `old_commit_id` to that of `new_commit_id` to `out` in `format`,
/// as `finish()` in Git's `builtin/merge.c` writes them: renames are found as `-M` finds them, paths are quoted as
/// `core.quotePath` says, and the lines are as wide as `COLUMNS`, or 80 columns, and as `diff.statNameWidth` and
/// `diff.statGraphWidth` allow.
pub(crate) fn write(
    repo: &gix::Repository,
    old_commit_id: ObjectId,
    new_commit_id: ObjectId,
    format: Format,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    let old_tree = repo.find_commit(old_commit_id)?.tree()?;
    let new_tree = repo.find_commit(new_commit_id)?.tree()?;
    let changes = changes(repo, &old_tree, &new_tree)?;
    let quote_path = repo.config_snapshot().boolean("core.quotePath").unwrap_or(true);
    let files = files(repo, &changes, quote_path, format == Format::CompactSummary)?;
    write_stats(&files, &Widths::configured(repo), out)?;
    if format == Format::StatAndSummary {
        write_summary(&changes, quote_path, out)?;
    }
    Ok(())
}

/// Write the short statistics and the summary of the changes from `old_tree` to `new_tree` to `out`, as
/// `print_commit_summary()` in Git's `sequencer.c` has them written with `--shortstat --summary` after a commit: the
/// numbers of the changed files, of the inserted lines and of the deleted lines, and a line for each file that was
/// created, deleted or renamed or whose mode changed. Renames are found as `-M` finds them, and paths are quoted as
/// `core.quotePath` says.
pub(crate) fn write_shortstat_and_summary(
    repo: &gix::Repository,
    old_tree: &gix::Tree<'_>,
    new_tree: &gix::Tree<'_>,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    let changes = changes(repo, old_tree, new_tree)?;
    let quote_path = repo.config_snapshot().boolean("core.quotePath").unwrap_or(true);
    let files = files(repo, &changes, quote_path, false)?;
    if !files.is_empty() {
        write_totals(&files, out)?;
    }
    write_summary(&changes, quote_path, out)
}

/// Return the changes of the files from `old_tree` to `new_tree` in the order of their paths, with renames found as `-M`
/// finds them.
fn changes(repo: &gix::Repository, old_tree: &gix::Tree<'_>, new_tree: &gix::Tree<'_>) -> Result<Vec<ChangeDetached>> {
    let mut options = gix::diff::Options::default();
    options.track_rewrites(Some(gix::diff::Rewrites::default()));
    let mut changes = repo.diff_tree_to_tree(old_tree, new_tree, options)?;
    changes.retain(|change| !change.entry_mode().is_tree());
    changes.sort_by(|a, b| a.location().cmp(b.location()));
    Ok(changes)
}

/// Return the files of `changes` with what Git counts for them, named as `fill_print_name()` in Git's `diff.c` names
/// them, with paths quoted as `quote_path` says, and followed by what `--compact-summary` notes if `compact` is set.
fn files(repo: &gix::Repository, changes: &[ChangeDetached], quote_path: bool, compact: bool) -> Result<Vec<File>> {
    let mut resource_cache = repo.diff_resource_cache_for_tree_diff()?;
    let mut files = Vec::with_capacity(changes.len());
    for change in changes {
        let mut name = print_name(change, quote_path);
        if compact && let Some(comment) = compact_summary(change) {
            name.extend_from_slice(format!(" ({comment})").as_bytes());
        }
        if let Some(file) = count(repo, change, name, &mut resource_cache)? {
            files.push(file);
        }
        resource_cache.clear_resource_cache_keep_allocation();
    }
    Ok(files)
}

/// Write the lines of `--summary` for `changes` to `out`, with paths quoted as `quote_path` says.
fn write_summary(changes: &[ChangeDetached], quote_path: bool, out: &mut dyn std::io::Write) -> Result<()> {
    for change in changes {
        out.write_all(&summary(change, quote_path)).or_error()?;
    }
    Ok(())
}

/// Return the file of `change`, named `name`, with what `builtin_diffstat()` in Git's `diff.c` counts for it: the added
/// and deleted lines, a submodule having the line `Subproject commit <id>`, or the sizes of the blobs of a binary file.
/// Return `None` for a modification that kept its mode and whose lines didn't change, which Git leaves out.
fn count(
    repo: &gix::Repository,
    change: &ChangeDetached,
    name: BString,
    resource_cache: &mut gix::diff::blob::Platform,
) -> Result<Option<File>> {
    let (old, new) = sides(change);
    if [old, new].into_iter().flatten().any(|(mode, _)| mode.is_commit()) {
        return Ok(Some(File {
            name,
            binary: false,
            added: line_count(repo, new)?,
            deleted: line_count(repo, old)?,
        }));
    }
    let Some(counts) = change.attach(repo, repo).diff(resource_cache)?.line_counts()? else {
        let unchanged = old.map(|(_, id)| id) == new.map(|(_, id)| id);
        let (added, deleted) = if unchanged {
            (0, 0)
        } else {
            (size(repo, new)?, size(repo, old)?)
        };
        return Ok(Some(File {
            name,
            binary: true,
            added,
            deleted,
        }));
    };
    let (added, deleted) = (u64::from(counts.insertions), u64::from(counts.removals));
    let mode_kept = matches!((old, new), (Some((old_mode, _)), Some((new_mode, _))) if old_mode == new_mode);
    if matches!(change, ChangeDetached::Modification { .. }) && mode_kept && added == 0 && deleted == 0 {
        return Ok(None);
    }
    Ok(Some(File {
        name,
        binary: false,
        added,
        deleted,
    }))
}

/// Return the old side of `change` and its new side, each `None` if it doesn't exist.
fn sides(change: &ChangeDetached) -> (Side<'_>, Side<'_>) {
    match change {
        ChangeDetached::Addition { entry_mode, id, .. } => (None, Some((*entry_mode, id.as_ref()))),
        ChangeDetached::Deletion { entry_mode, id, .. } => (Some((*entry_mode, id.as_ref())), None),
        ChangeDetached::Modification {
            previous_entry_mode,
            previous_id,
            entry_mode,
            id,
            ..
        } => (
            Some((*previous_entry_mode, previous_id.as_ref())),
            Some((*entry_mode, id.as_ref())),
        ),
        ChangeDetached::Rewrite {
            source_entry_mode,
            source_id,
            entry_mode,
            id,
            ..
        } => (
            Some((*source_entry_mode, source_id.as_ref())),
            Some((*entry_mode, id.as_ref())),
        ),
    }
}

/// Return the number of lines of `side` of a change that involves a submodule, as `count_lines()` in Git's `diff.c` counts
/// those of the text that Git compares for it: one for the line `Subproject commit <id>` of a submodule, and those of a
/// blob, with a last line that lacks its line feed.
fn line_count(repo: &gix::Repository, side: Side<'_>) -> Result<u64> {
    Ok(match side {
        None => 0,
        Some((mode, _)) if mode.is_commit() => 1,
        Some((_, blob_id)) => {
            let blob = repo.find_blob(blob_id)?;
            let lines = blob.data.iter().filter(|byte| **byte == b'\n').count() as u64;
            lines + u64::from(blob.data.last().is_some_and(|byte| *byte != b'\n'))
        }
    })
}

/// Return the size of the blob of `side`, or 0 if it doesn't exist, as `diff_filespec_size()` in Git's `diff.c` returns it.
fn size(repo: &gix::Repository, side: Side<'_>) -> Result<u64> {
    Ok(match side {
        None => 0,
        Some((_, blob_id)) => repo.find_header(blob_id)?.size(),
    })
}

/// Return the name of the file of `change` as `fill_print_name()` in Git's `diff.c` makes it: both paths of a rename as
/// `pprint_rename()` writes them, and otherwise the path, quoted as `core.quotePath` says with `quote_path`.
fn print_name(change: &ChangeDetached, quote_path: bool) -> BString {
    match change {
        ChangeDetached::Rewrite {
            source_location,
            location,
            ..
        } => pprint_rename(source_location.as_bstr(), location.as_bstr(), quote_path),
        _ => quote_c_style(change.location(), quote_path).into_owned(),
    }
}

/// Return what `--compact-summary` notes after the name of the file of `change`, as `get_compact_summary()` in Git's
/// `diff.c` chooses it: whether the file is new or gone, or how its mode changed.
fn compact_summary(change: &ChangeDetached) -> Option<&'static str> {
    let (old_mode, new_mode) = match change {
        ChangeDetached::Addition { entry_mode, .. } => {
            return Some(if entry_mode.is_link() {
                "new +l"
            } else if permissions(*entry_mode) == 0o755 {
                "new +x"
            } else {
                "new"
            });
        }
        ChangeDetached::Deletion { .. } => return Some("gone"),
        ChangeDetached::Modification {
            previous_entry_mode,
            entry_mode,
            ..
        } => (*previous_entry_mode, *entry_mode),
        ChangeDetached::Rewrite {
            source_entry_mode,
            entry_mode,
            ..
        } => (*source_entry_mode, *entry_mode),
    };
    if old_mode.is_link() && !new_mode.is_link() {
        Some("mode -l")
    } else if !old_mode.is_link() && new_mode.is_link() {
        Some("mode +l")
    } else if permissions(old_mode) == 0o644 && permissions(new_mode) == 0o755 {
        Some("mode +x")
    } else if permissions(old_mode) == 0o755 && permissions(new_mode) == 0o644 {
        Some("mode -x")
    } else {
        None
    }
}

/// Return the permission bits of `mode`.
fn permissions(mode: EntryMode) -> u16 {
    mode.value() & 0o777
}

/// Return the lines that `diff_summary()` in Git's `diff.c` writes for `change` with `--summary`: the creation or the
/// deletion of a file with its mode, a rename with its similarity, and a change of the mode.
fn summary(change: &ChangeDetached, quote_path: bool) -> BString {
    let mut lines = BString::default();
    match change {
        ChangeDetached::Addition {
            location, entry_mode, ..
        } => file_mode_name(&mut lines, "create", *entry_mode, location.as_bstr(), quote_path),
        ChangeDetached::Deletion {
            location, entry_mode, ..
        } => file_mode_name(&mut lines, "delete", *entry_mode, location.as_bstr(), quote_path),
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

/// Append the line that `show_file_mode_name()` in Git's `diff.c` writes for a file at `path` with `mode` that was created
/// or deleted, as `action` says, to `lines`.
fn file_mode_name(lines: &mut BString, action: &str, mode: EntryMode, path: &BStr, quote_path: bool) {
    lines.extend_from_slice(format!(" {action} mode {:06o} ", mode.value()).as_bytes());
    lines.extend_from_slice(&quote_c_style(path, quote_path));
    lines.push(b'\n');
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

/// The widths that the statistics may take, as `show_stats()` in Git's `diff.c` finds them after
/// `init_diffstat_widths()`.
struct Widths {
    /// The width of a line: `COLUMNS` if it is a positive number, and otherwise 80, as `term_columns()` in Git's `pager.c`
    /// returns it for an output that isn't a terminal.
    line: i64,
    /// The widest name, `diff.statNameWidth`, or 0 for no limit.
    name: i64,
    /// The widest graph, `diff.statGraphWidth`, or 0 for no limit.
    graph: i64,
}

impl Widths {
    /// Return the widths that the environment and the configuration of `repo` allow.
    fn configured(repo: &gix::Repository) -> Self {
        let config = repo.config_snapshot();
        Widths {
            line: std::env::var("COLUMNS")
                .ok()
                .and_then(|columns| columns.parse().ok())
                .filter(|columns| *columns > 0)
                .unwrap_or(80),
            name: config.integer("diff.statNameWidth").unwrap_or(0),
            graph: config.integer("diff.statGraphWidth").unwrap_or(0),
        }
    }
}

/// Write a line for each of `files` and their totals to `out` within `widths`, as `show_stats()` in Git's `diff.c` writes
/// them: the name, padded to the widest one or shortened from its start, the number of changed lines and a graph of `+`
/// and `-` scaled to fit, or the sizes of the blobs of a binary file, and then the number of files, insertions and
/// deletions.
fn write_stats(files: &[File], widths: &Widths, out: &mut dyn std::io::Write) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    let (mut max_change, mut max_len, mut number_width, mut bin_width) = (0_i64, 0_i64, 0_i64, 0_i64);
    for file in files {
        max_len = max_len.max(display_width(file.name.as_bstr()));
        if file.binary {
            // `Bin <deleted> -> <added> bytes`, whose count is aligned with `Bin`.
            bin_width = bin_width.max(14 + decimal_width(file.added) + decimal_width(file.deleted));
            number_width = 3;
        } else {
            max_change = max_change.max((file.added + file.deleted) as i64);
        }
    }
    number_width = number_width.max(decimal_width(max_change as u64));
    // Leave at least 6 columns to the graph and 10 to the names.
    let width = widths.line.max(16 + 6 + number_width);
    let mut graph_width = if max_change + 4 > bin_width {
        max_change
    } else {
        bin_width - 4
    };
    if widths.graph != 0 && widths.graph < graph_width {
        graph_width = widths.graph;
    }
    let mut name_width = if widths.name > 0 && widths.name < max_len {
        widths.name
    } else {
        max_len
    };
    if name_width + number_width + 6 + graph_width > width {
        if graph_width > width * 3 / 8 - number_width - 6 {
            graph_width = (width * 3 / 8 - number_width - 6).max(6);
        }
        if widths.graph != 0 && graph_width > widths.graph {
            graph_width = widths.graph;
        }
        if name_width > width - number_width - 6 - graph_width {
            name_width = width - number_width - 6 - graph_width;
        } else {
            graph_width = width - number_width - 6 - name_width;
        }
    }

    let number_width = number_width as usize;
    for file in files {
        let mut name = file.name.as_bstr();
        let mut prefix = "";
        let mut len = name_width;
        let name_len = display_width(name);
        if name_width < name_len {
            prefix = "...";
            len = (len - 3).max(0);
            let start = name
                .char_indices()
                .nth((name_len - len) as usize)
                .map_or(name.len(), |(start, _, _)| start);
            name = name[start..].as_bstr();
            if let Some(slash) = name.find_byte(b'/') {
                name = name[slash..].as_bstr();
            }
        }
        let padding = (len - display_width(name)).max(0) as usize;
        let mut line = BString::from(format!(" {prefix}"));
        line.extend_from_slice(name);
        line.extend_from_slice(format!("{:padding$} | ", "").as_bytes());
        if file.binary {
            line.extend_from_slice(format!("{:>number_width$}", "Bin").as_bytes());
            if file.added != 0 || file.deleted != 0 {
                line.extend_from_slice(format!(" {} -> {} bytes", file.deleted, file.added).as_bytes());
            }
        } else {
            let total = file.added + file.deleted;
            let (add, del) = scale(file.added as i64, file.deleted as i64, graph_width, max_change);
            let separator = if total == 0 { "" } else { " " };
            line.extend_from_slice(format!("{total:>number_width$}{separator}").as_bytes());
            line.extend_from_slice("+".repeat(add as usize).as_bytes());
            line.extend_from_slice("-".repeat(del as usize).as_bytes());
        }
        line.push(b'\n');
        out.write_all(&line).or_error()?;
    }

    write_totals(files, out)
}

/// Write the numbers of `files`, of their inserted lines and of their deleted lines to `out`, as
/// `print_stat_summary_inserts_deletes()` in Git's `diff.c` writes them for `show_stats()` and `show_shortstat()`, which
/// leave out the sizes of binary files.
fn write_totals(files: &[File], out: &mut dyn std::io::Write) -> Result<()> {
    let (insertions, deletions) = files
        .iter()
        .filter(|file| !file.binary)
        .fold((0_u64, 0_u64), |(insertions, deletions), file| {
            (insertions + file.added, deletions + file.deleted)
        });
    let changed = files.len();
    write!(out, " {changed} file{} changed", if changed == 1 { "" } else { "s" }).or_error()?;
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
    writeln!(out).or_error()
}

/// Return the characters of the graph of a file with `added` and `deleted` lines that is `graph_width` wide for the
/// largest change `max_change`, as `show_stats()` in Git's `diff.c` scales them if the graph would be wider: the number
/// of `+` and that of `-`, of which there is at least one each if any line was added or deleted.
fn scale(added: i64, deleted: i64, graph_width: i64, max_change: i64) -> (i64, i64) {
    if graph_width > max_change {
        return (added, deleted);
    }
    let mut total = scale_linear(added + deleted, graph_width, max_change);
    if total < 2 && added != 0 && deleted != 0 {
        total = 2;
    }
    if added < deleted {
        let add = scale_linear(added, graph_width, max_change);
        (add, total - add)
    } else {
        let del = scale_linear(deleted, graph_width, max_change);
        (total - del, del)
    }
}

/// Scale `it` to `width` columns for the largest change `max_change` as `scale_linear()` in Git's `diff.c` does, which
/// scales to one column less and adds one, so that a change of a line shows a character.
fn scale_linear(it: i64, width: i64, max_change: i64) -> i64 {
    if it == 0 { 0 } else { 1 + it * (width - 1) / max_change }
}

/// Return the number of decimal digits of `number`, as `decimal_width()` in Git's `pager.c` counts them.
fn decimal_width(mut number: u64) -> i64 {
    let mut width = 1;
    while number >= 10 {
        number /= 10;
        width += 1;
    }
    width
}

/// Return the number of columns that `name` takes, a column for each character.
fn display_width(name: &BStr) -> i64 {
    name.chars().count() as i64
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
