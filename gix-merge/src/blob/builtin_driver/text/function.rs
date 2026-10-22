use crate::blob::{
    Resolution,
    builtin_driver::text::{
        Conflict, ConflictStyle, Labels, Merge, Options,
        utils::{
            Markers, Mode, Writer, combine_changes, diff, join_close_conflicts, refine_conflicts, tokens,
            trim_conflicts,
        },
    },
};

impl<'input, 'data> Merge<'input, 'data> {
    /// Prepare merge state for `current`, `ancestor`, and `other` using `diff_algorithm`.
    ///
    /// This computes the changes of both sides once so they can be rendered multiple times with
    /// [`Merge::run()`], which is useful when experimenting with multiple conflict styles
    /// for the same input triplet.
    ///
    /// The returned [`Merge`] keeps a reference to the provided `input`, which guarantees
    /// that subsequent calls to [`Merge::run()`] use the exact same interner state.
    pub fn new(
        input: &'input mut imara_diff::InternedInput<&'data [u8]>,
        current: &'data [u8],
        ancestor: &'data [u8],
        other: &'data [u8],
        diff_algorithm: imara_diff::Algorithm,
    ) -> Merge<'input, 'data> {
        input.update_before(tokens(ancestor));
        input.update_after(tokens(current));
        let current_changes = diff(diff_algorithm, &input.before, &input.after, input.interner.num_tokens());

        let current_tokens = std::mem::take(&mut input.after);
        input.update_after(tokens(other));
        let other_changes = diff(diff_algorithm, &input.before, &input.after, input.interner.num_tokens());

        Merge {
            input,
            current_tokens,
            current_changes,
            other_changes,
            diff_algorithm,
        }
    }

    /// Merge `current` and `other` with `ancestor` as base using `conflict` as strategy, as Git's
    /// `xdl_merge()` does at the level that merges use, `XDL_MERGE_ZEALOUS`.
    ///
    /// Use `labels` to annotate conflict sections.
    ///
    /// Place the merged result in `out` (cleared before use) and return the resolution.
    pub fn run(&self, out: &mut Vec<u8>, labels: Labels<'_>, conflict: Conflict) -> Resolution {
        out.clear();
        let input = self.input;
        let writer = Writer {
            interner: &input.interner,
            ancestor: &input.before,
            current: &self.current_tokens,
            other: &input.after,
        };
        if self.current_changes.is_empty() {
            writer.write_all(out, writer.other);
            return Resolution::Complete;
        }
        if self.other_changes.is_empty() {
            writer.write_all(out, writer.current);
            return Resolution::Complete;
        }

        // Conflict resolutions have no style of their own and resolve the conflicts of the default style,
        // as in Git without `merge.conflictStyle`.
        let (style, marker_size, resolve_with) = match conflict {
            Conflict::Keep { style, marker_size } => (style, marker_size.get(), None),
            Conflict::ResolveWithOurs => (ConflictStyle::Merge, Conflict::DEFAULT_MARKER_SIZE, Some(Mode::Current)),
            Conflict::ResolveWithTheirs => (ConflictStyle::Merge, Conflict::DEFAULT_MARKER_SIZE, Some(Mode::Other)),
            Conflict::ResolveWithUnion => (ConflictStyle::Merge, Conflict::DEFAULT_MARKER_SIZE, Some(Mode::Both)),
        };
        let mut regions = combine_changes(
            &self.current_changes,
            &self.other_changes,
            writer.ancestor,
            writer.current,
            writer.other,
        );
        // `diff3` shows each conflict with its ancestor lines, so its sides stay whole, and `zdiff3` only trims
        // the lines that both sides start or end with.
        match style {
            ConflictStyle::Merge => {
                regions = join_close_conflicts(refine_conflicts(
                    regions,
                    writer.current,
                    writer.other,
                    self.diff_algorithm,
                ));
            }
            ConflictStyle::Diff3 => {}
            ConflictStyle::ZealousDiff3 => trim_conflicts(&mut regions, writer.current, writer.other),
        }
        let has_conflicts = regions.iter().any(|region| region.mode == Mode::Conflict);
        writer.write_merge(
            out,
            &regions,
            resolve_with,
            Markers {
                style,
                size: marker_size,
                labels,
            },
        );
        match (has_conflicts, resolve_with) {
            (false, _) => Resolution::Complete,
            (true, None) => Resolution::Conflict,
            (true, Some(_)) => Resolution::CompleteWithAutoResolvedConflict,
        }
    }
}

/// Merge `current` and `other` with `ancestor` as base according to `opts`.
///
/// Use `labels` to annotate conflict sections.
///
/// `input` is for reusing memory for lists of tokens, but note that it grows indefinitely
/// while tokens for `current`, `ancestor` and `other` are added.
/// Place the merged result in `out` (cleared before use) and return the resolution.
///
/// # Important
///
/// *The caller* is responsible for clearing `input`, otherwise tokens will accumulate.
/// This idea is to save time if the input is known to be very similar.
pub fn merge<'a>(
    out: &mut Vec<u8>,
    input: &mut imara_diff::InternedInput<&'a [u8]>,
    labels: Labels<'_>,
    current: &'a [u8],
    ancestor: &'a [u8],
    other: &'a [u8],
    Options {
        diff_algorithm,
        conflict,
    }: Options,
) -> Resolution {
    out.clear();
    let merge = Merge::new(input, current, ancestor, other, diff_algorithm);
    merge.run(out, labels, conflict)
}
