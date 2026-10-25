use std::{collections::HashMap, ops::Range};

use bstr::BStr;
use imara_diff::{Algorithm, Diff, Hunk, Interner, NoSliderHeuristic, Token};

use crate::blob::builtin_driver::text::{ConflictStyle, Labels};

/// Split `input` into lines that keep their line endings.
pub fn tokens(input: &[u8]) -> imara_diff::sources::ByteLines<'_> {
    imara_diff::sources::byte_lines(input)
}

/// Return the changes that turn `before` into `after` according to `algorithm`, where every token of both
/// is less than `num_tokens`.
///
/// Like Git's merges, slide ambiguous changes as far down as possible without the indentation heuristic
/// of `git diff`, as their position decides whether the changes of both sides overlap.
pub fn diff(algorithm: Algorithm, before: &[Token], after: &[Token], num_tokens: u32) -> Vec<Hunk> {
    let mut diff = Diff::default();
    diff.compute_with(algorithm, before, after, num_tokens);
    diff.postprocess_with(before, after, NoSliderHeuristic);
    diff.hunks().collect()
}

/// `len` consecutive lines from line `start` on.
///
/// Both are signed like the line numbers in Git's merge, whose computations pass through negative offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lines {
    pub start: i64,
    pub len: i64,
}

impl Lines {
    /// The lines in `range`.
    fn in_range(range: &Range<u32>) -> Self {
        Lines::between(range.start.into(), range.end.into())
    }

    /// The lines from line `start` up to, but not including, line `end`.
    fn between(start: i64, end: i64) -> Self {
        Lines {
            start,
            len: end - start,
        }
    }

    /// All lines of `tokens`.
    fn all(tokens: &[Token]) -> Self {
        Lines {
            start: 0,
            len: tokens.len() as i64,
        }
    }

    /// The line after the last one.
    fn end(self) -> i64 {
        self.start + self.len
    }

    /// Return the tokens of these lines in `tokens`, or `None` if they aren't all within it.
    fn of(self, tokens: &[Token]) -> Option<&[Token]> {
        tokens.get(usize::try_from(self.start).ok()?..usize::try_from(self.end()).ok()?)
    }
}

/// What the merge result contains for a [`Region`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The lines of both sides between conflict markers, as both sides changed the region differently.
    Conflict,
    /// The lines of the current side.
    Current,
    /// The lines of the other side.
    Other,
    /// The lines of the current side followed by the lines of the other side.
    Both,
    /// The lines of the current side, which are the same as those of the other side.
    Identical,
}

/// Lines of the ancestor that one or both sides changed, and the lines of each side in their place.
///
/// The lines of a side that didn't change the ancestor lines are its copy of them.
#[derive(Debug, Clone, Copy)]
pub struct Region {
    pub mode: Mode,
    pub ancestor: Lines,
    pub current: Lines,
    pub other: Lines,
}

/// Combine the changes that turn `ancestor` into `current` and into `other` into the regions of the merge
/// result, as Git's `xdl_do_merge()` does.
///
/// A change that at least one unchanged ancestor line separates from the changes of the other side becomes
/// a region with the lines of its side. Other changes conflict, unless both sides replaced the same ancestor
/// lines with the same lines, which leaves nothing to merge. A region that starts within or right after the
/// previous one on either side extends that one, which then conflicts if their modes differ.
pub fn combine_changes(
    current_changes: &[Hunk],
    other_changes: &[Hunk],
    ancestor: &[Token],
    current: &[Token],
    other: &[Token],
) -> Vec<Region> {
    let mut regions = Vec::new();
    let (mut current_idx, mut other_idx) = (0, 0);
    while let (Some(current_change), Some(other_change)) =
        (current_changes.get(current_idx), other_changes.get(other_idx))
    {
        let (current_ancestor, current_lines) = (
            Lines::in_range(&current_change.before),
            Lines::in_range(&current_change.after),
        );
        let (other_ancestor, other_lines) = (
            Lines::in_range(&other_change.before),
            Lines::in_range(&other_change.after),
        );
        if current_ancestor.end() < other_ancestor.start {
            append(
                &mut regions,
                Region {
                    mode: Mode::Current,
                    ancestor: current_ancestor,
                    current: current_lines,
                    other: Lines {
                        start: other_lines.start - other_ancestor.start + current_ancestor.start,
                        len: current_ancestor.len,
                    },
                },
            );
            current_idx += 1;
            continue;
        }
        if other_ancestor.end() < current_ancestor.start {
            append(
                &mut regions,
                Region {
                    mode: Mode::Other,
                    ancestor: other_ancestor,
                    current: Lines {
                        start: current_lines.start - current_ancestor.start + other_ancestor.start,
                        len: other_ancestor.len,
                    },
                    other: other_lines,
                },
            );
            other_idx += 1;
            continue;
        }
        if current_ancestor != other_ancestor
            || current_lines.len != other_lines.len
            || current_lines.of(current) != other_lines.of(other)
        {
            append(
                &mut regions,
                conflict(current_ancestor, current_lines, other_ancestor, other_lines),
            );
        }
        if current_ancestor.end() >= other_ancestor.end() {
            other_idx += 1;
        }
        if other_ancestor.end() >= current_ancestor.end() {
            current_idx += 1;
        }
    }

    let ancestor_len = ancestor.len() as i64;
    for change in &current_changes[current_idx..] {
        let ancestor_lines = Lines::in_range(&change.before);
        append(
            &mut regions,
            Region {
                mode: Mode::Current,
                ancestor: ancestor_lines,
                current: Lines::in_range(&change.after),
                other: Lines {
                    start: ancestor_lines.start + other.len() as i64 - ancestor_len,
                    len: ancestor_lines.len,
                },
            },
        );
    }
    for change in &other_changes[other_idx..] {
        let ancestor_lines = Lines::in_range(&change.before);
        append(
            &mut regions,
            Region {
                mode: Mode::Other,
                ancestor: ancestor_lines,
                current: Lines {
                    start: ancestor_lines.start + current.len() as i64 - ancestor_len,
                    len: ancestor_lines.len,
                },
                other: Lines::in_range(&change.after),
            },
        );
    }
    regions
}

/// Return the conflict of the overlapping changes of `current_lines` in place of `current_ancestor` and of
/// `other_lines` in place of `other_ancestor`, as Git's `xdl_do_merge()` computes it: in the ancestor, it
/// reaches from the start of the earlier change to the end of the later one, and each side's lines extend by
/// the ancestor lines that only the change of the other side covers.
fn conflict(current_ancestor: Lines, current_lines: Lines, other_ancestor: Lines, other_lines: Lines) -> Region {
    let start_offset = current_ancestor.start - other_ancestor.start;
    let (mut ancestor_start, mut current_start, mut other_start) =
        (current_ancestor.start, current_lines.start, other_lines.start);
    if start_offset > 0 {
        ancestor_start -= start_offset;
        current_start -= start_offset;
    } else {
        other_start += start_offset;
    }

    let end_offset = current_ancestor.end() - other_ancestor.end();
    let (mut ancestor_end, mut current_end, mut other_end) =
        (current_ancestor.end(), current_lines.end(), other_lines.end());
    if end_offset < 0 {
        ancestor_end -= end_offset;
        current_end -= end_offset;
    } else {
        other_end += end_offset;
    }

    Region {
        mode: Mode::Conflict,
        ancestor: Lines::between(ancestor_start, ancestor_end),
        current: Lines::between(current_start, current_end),
        other: Lines::between(other_start, other_end),
    }
}

/// Add `region` to `regions`, or extend the last of them by it if it starts within or right after that one on
/// either side, as Git's `xdl_append_merge()` does.
fn append(regions: &mut Vec<Region>, region: Region) {
    if let Some(last) = regions.last_mut()
        && (region.current.start <= last.current.end() || region.other.start <= last.other.end())
    {
        if region.mode != last.mode {
            last.mode = Mode::Conflict;
        }
        last.ancestor.len = region.ancestor.end() - last.ancestor.start;
        last.current.len = region.current.end() - last.current.start;
        last.other.len = region.other.end() - last.other.start;
    } else {
        regions.push(region);
    }
}

/// Narrow each conflict of `regions` to the lines in which its sides differ, as Git's `xdl_refine_conflicts()`
/// does: the lines of the current side are diffed against those of the other side with `algorithm`, and each
/// change becomes a conflict of its own. A conflict without changes becomes [`Mode::Identical`].
pub fn refine_conflicts(regions: Vec<Region>, current: &[Token], other: &[Token], algorithm: Algorithm) -> Vec<Region> {
    let mut refined = Vec::with_capacity(regions.len());
    for region in regions {
        if region.mode != Mode::Conflict || region.current.len == 0 || region.other.len == 0 {
            refined.push(region);
            continue;
        }
        let (Some(current_tokens), Some(other_tokens)) = (region.current.of(current), region.other.of(other)) else {
            refined.push(region);
            continue;
        };
        let (current_tokens, other_tokens, num_tokens) = renumber(current_tokens, other_tokens);
        let changes = diff(algorithm, &current_tokens, &other_tokens, num_tokens);
        if changes.is_empty() {
            refined.push(Region {
                mode: Mode::Identical,
                ..region
            });
            continue;
        }
        refined.extend(changes.iter().map(|change| {
            let (current_lines, other_lines) = (Lines::in_range(&change.before), Lines::in_range(&change.after));
            Region {
                mode: Mode::Conflict,
                ancestor: region.ancestor,
                current: Lines {
                    start: region.current.start + current_lines.start,
                    len: current_lines.len,
                },
                other: Lines {
                    start: region.other.start + other_lines.start,
                    len: other_lines.len,
                },
            }
        }));
    }
    refined
}

/// Return `a` and `b` with their tokens renumbered from zero in the order in which they first appear, along
/// with the number of distinct tokens, so that diffing them takes time in proportion to their length rather
/// than to the number of tokens in all files.
fn renumber(a: &[Token], b: &[Token]) -> (Vec<Token>, Vec<Token>, u32) {
    let mut numbers = HashMap::with_capacity(a.len() + b.len());
    let mut renumber = |tokens: &[Token]| -> Vec<Token> {
        tokens
            .iter()
            .map(|token| {
                let next = Token(numbers.len() as u32);
                *numbers.entry(*token).or_insert(next)
            })
            .collect()
    };
    let (a, b) = (renumber(a), renumber(b));
    let num_tokens = numbers.len() as u32;
    (a, b, num_tokens)
}

/// Join each conflict of `regions` with the next one if at most three lines of the current side separate them,
/// as Git's `xdl_simplify_non_conflicts()` does in merges: the joined conflict takes up no more lines than
/// the two conflicts with the lines between them.
pub fn join_close_conflicts(regions: Vec<Region>) -> Vec<Region> {
    let mut joined: Vec<Region> = Vec::with_capacity(regions.len());
    for region in regions {
        if let Some(last) = joined.last_mut()
            && last.mode == Mode::Conflict
            && region.mode == Mode::Conflict
            && region.current.start - last.current.end() <= 3
        {
            last.current.len = region.current.end() - last.current.start;
            last.other.len = region.other.end() - last.other.start;
            continue;
        }
        joined.push(region);
    }
    joined
}

/// Remove the lines that both sides of each conflict of `regions` start or end with from the conflict, keeping
/// all of its ancestor lines, as Git's `xdl_refine_zdiff3_conflicts()` does.
pub fn trim_conflicts(regions: &mut [Region], current: &[Token], other: &[Token]) {
    let same_line = |current_line: i64, other_line: i64| {
        let line = |tokens: &[Token], line: i64| usize::try_from(line).ok().and_then(|line| tokens.get(line).copied());
        matches!((line(current, current_line), line(other, other_line)), (Some(a), Some(b)) if a == b)
    };
    for region in regions.iter_mut().filter(|region| region.mode == Mode::Conflict) {
        while region.current.len > 0 && region.other.len > 0 && same_line(region.current.start, region.other.start) {
            region.current.start += 1;
            region.current.len -= 1;
            region.other.start += 1;
            region.other.len -= 1;
        }
        while region.current.len > 0
            && region.other.len > 0
            && same_line(region.current.end() - 1, region.other.end() - 1)
        {
            region.current.len -= 1;
            region.other.len -= 1;
        }
    }
}

/// How to write the conflicts of a merge result.
#[derive(Debug, Clone, Copy)]
pub struct Markers<'a> {
    /// Whether to show the ancestor's lines of each conflict.
    pub style: ConflictStyle,
    /// The number of characters of each marker.
    pub size: u32,
    /// The names that follow the markers.
    pub labels: Labels<'a>,
}

/// The lines of the three files of a merge, from which the merge result is written.
pub struct Writer<'a, 'data> {
    pub interner: &'a Interner<&'data [u8]>,
    pub ancestor: &'a [Token],
    pub current: &'a [Token],
    pub other: &'a [Token],
}

impl Writer<'_, '_> {
    /// Write all of `tokens` to `out`.
    pub fn write_all(&self, out: &mut Vec<u8>, tokens: &[Token]) {
        self.write(out, tokens, Lines::all(tokens), None);
    }

    /// Write the merge result of `regions` to `out`, as Git's `xdl_fill_merge_buffer()` does: conflicts take
    /// the mode `resolve_with` if it is set, and are otherwise written with `markers`.
    pub fn write_merge(&self, out: &mut Vec<u8>, regions: &[Region], resolve_with: Option<Mode>, markers: Markers<'_>) {
        // The current side's lines before this one are written, or replaced by those of the other side.
        let mut current_written = 0;
        for region in regions {
            let mode = match region.mode {
                Mode::Conflict => resolve_with.unwrap_or(Mode::Conflict),
                mode => mode,
            };
            match mode {
                Mode::Conflict => {
                    self.write(
                        out,
                        self.current,
                        Lines::between(current_written, region.current.start),
                        None,
                    );
                    self.write_conflict(out, region, markers);
                }
                Mode::Current | Mode::Other | Mode::Both => {
                    self.write(
                        out,
                        self.current,
                        Lines::between(current_written, region.current.start),
                        None,
                    );
                    if mode != Mode::Other {
                        let line_ending = (mode == Mode::Both).then(|| self.line_ending(region));
                        self.write(out, self.current, region.current, line_ending);
                    }
                    if mode != Mode::Current {
                        self.write(out, self.other, region.other, None);
                    }
                }
                Mode::Identical => continue,
            }
            current_written = region.current.end();
        }
        self.write(
            out,
            self.current,
            Lines::between(current_written, Lines::all(self.current).end()),
            None,
        );
    }

    /// Write the lines of both sides of the conflict `region` between `markers`, with its ancestor lines in the
    /// `diff3` styles, as Git's `fill_conflict_hunk()` does.
    fn write_conflict(&self, out: &mut Vec<u8>, region: &Region, markers: Markers<'_>) {
        let line_ending = self.line_ending(region);
        write_marker(out, b'<', markers.size, markers.labels.current, line_ending);
        self.write(out, self.current, region.current, Some(line_ending));
        if matches!(markers.style, ConflictStyle::Diff3 | ConflictStyle::ZealousDiff3) {
            write_marker(out, b'|', markers.size, markers.labels.ancestor, line_ending);
            self.write(out, self.ancestor, region.ancestor, Some(line_ending));
        }
        write_marker(out, b'=', markers.size, None, line_ending);
        self.write(out, self.other, region.other, Some(line_ending));
        write_marker(out, b'>', markers.size, markers.labels.other, line_ending);
    }

    /// Write `lines` of `tokens` to `out`, followed by `line_ending` if it is set and the last of them has none,
    /// as Git's `xdl_recs_copy()` does.
    fn write(&self, out: &mut Vec<u8>, tokens: &[Token], lines: Lines, line_ending: Option<&[u8]>) {
        let tokens = lines.of(tokens).unwrap_or_default();
        for token in tokens {
            out.extend_from_slice(self.interner[*token]);
        }
        if let (Some(line_ending), Some(last)) = (line_ending, tokens.last())
            && !self.interner[*last].ends_with(b"\n")
        {
            out.extend_from_slice(line_ending);
        }
    }

    /// Return the line ending of the conflict markers of `region` and of the line ending added to the last line
    /// of each of its sides, as Git's `is_cr_needed()` decides: CRLF only if the first line of the ancestor ends
    /// with CRLF and neither side's line before the region, or first line, ends with LF alone.
    fn line_ending(&self, region: &Region) -> &'static [u8] {
        let mut crlf = ends_with_crlf(self.interner, self.current, (region.current.start - 1).max(0));
        if crlf != Some(false) {
            crlf = ends_with_crlf(self.interner, self.other, (region.other.start - 1).max(0));
        }
        if crlf != Some(false) {
            crlf = ends_with_crlf(self.interner, self.ancestor, 0);
        }
        if crlf == Some(true) { b"\r\n" } else { b"\n" }
    }
}

/// Return whether line `line` of `lines` ends with CRLF, or `None` if that can't be told, as Git's
/// `is_eol_crlf()` does: a last line without a line ending is judged by the line before it.
fn ends_with_crlf(interner: &Interner<&[u8]>, lines: &[Token], line: i64) -> Option<bool> {
    let line = usize::try_from(line).ok()?;
    let token = *lines.get(line)?;
    if line + 1 < lines.len() || interner[token].ends_with(b"\n") {
        return Some(interner[token].ends_with(b"\r\n"));
    }
    let previous = *lines.get(line.checked_sub(1)?)?;
    Some(interner[previous].ends_with(b"\r\n"))
}

/// Write a conflict marker of `size` times `marker`, then a space and `label` unless it is empty, as Git does.
fn write_marker(out: &mut Vec<u8>, marker: u8, size: u32, label: Option<&BStr>, line_ending: &[u8]) {
    out.extend(std::iter::repeat_n(marker, size as usize));
    if let Some(label) = label.filter(|label| !label.is_empty()) {
        out.push(b' ');
        out.extend_from_slice(label);
    }
    out.extend_from_slice(line_ending);
}
