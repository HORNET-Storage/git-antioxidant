use bstr::ByteSlice;

pub(crate) mod pipeline;
mod platform;
mod slider;
mod textconv;
mod unified_diff;

pub(crate) fn new_attributes_stack(worktree_root: impl Into<std::path::PathBuf>) -> gix_worktree::Stack {
    gix_worktree::Stack::new(
        worktree_root,
        gix_worktree::stack::State::AttributesStack(gix_worktree::stack::state::Attributes::new(
            Default::default(),
            None,
            gix_worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
            Default::default(),
        )),
        gix_worktree::glob::pattern::Case::Sensitive,
        Vec::new(),
        Vec::new(),
    )
}

/// Drops the file headers of Git's diff in `content` and writes its hunk headers as gix-diff writes them.
pub(crate) fn skip_header_and_fold_to_unidiff(content: &[u8]) -> String {
    let mut lines = content.lines();

    assert!(lines.next().expect("diff header").starts_with(b"diff --git "));
    assert!(lines.next().expect("index header").starts_with(b"index "));
    assert!(lines.next().expect("--- header").starts_with(b"--- "));
    assert!(lines.next().expect("+++ header").starts_with(b"+++ "));

    let mut out = String::new();
    for line in lines {
        if line.starts_with(b"\\") {
            continue;
        }
        let line = line.to_str().expect("baseline diff is valid utf-8");
        if let Some((ranges, _section)) = line.strip_prefix("@@ ").and_then(|line| line.split_once(" @@")) {
            let (before, after) = ranges.split_once(' ').expect("a hunk header has two ranges");
            out.push_str("@@ ");
            out.push_str(&unidiff_range(before, '-'));
            out.push(' ');
            out.push_str(&unidiff_range(after, '+'));
            out.push_str(" @@");
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Writes `range`, which starts with `sign`, as gix-diff writes it: Git leaves out a length of one and
/// starts an empty range at the line before it.
fn unidiff_range(range: &str, sign: char) -> String {
    let range = range.strip_prefix(sign).expect("a range starts with its sign");
    let (start, len) = range.split_once(',').unwrap_or((range, "1"));
    let start: u32 = start.parse().expect("the start of a range is a number");
    let len: u32 = len.parse().expect("the length of a range is a number");
    let start = if len == 0 { start + 1 } else { start };
    format!("{sign}{start},{len}")
}

#[test]
fn git_hunk_sections_are_omitted_from_unified_diff_baselines() {
    let git = br#"diff --git a/before b/after
index 0000000..1111111 100644
--- a/before
+++ b/after
@@ -1,3 +1,3 @@ fn example() {
 fn example() {
-    before();
+    after();
 }
"#;
    let expected = r#"@@ -1,3 +1,3 @@
 fn example() {
-    before();
+    after();
 }
"#;

    assert_eq!(
        skip_header_and_fold_to_unidiff(git),
        expected,
        "Git's optional hunk section isn't part of gix-diff's unified diff output"
    );
}

#[test]
fn git_ranges_are_written_as_gix_diff_writes_them() {
    let git = br#"diff --git a/before b/after
index 0000000..1111111 100644
--- a/before
+++ b/after
@@ -1 +0,0 @@
-a
@@ -2,0 +2 @@
+c
"#;
    let expected = r#"@@ -1,1 +1,0 @@
-a
@@ -3,0 +2,1 @@
+c
"#;

    assert_eq!(
        skip_header_and_fold_to_unidiff(git),
        expected,
        "a range of one line keeps its length, and an empty range starts at the line after the one Git names"
    );
}
