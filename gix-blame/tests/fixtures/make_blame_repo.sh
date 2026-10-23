#!/usr/bin/env bash
set -eu -o pipefail

git init -q
git config --local diff.algorithm histogram

git config merge.ff false

git checkout -q -b main

echo "line 1" >> simple.txt
git add simple.txt
git commit -q -m c1

echo -e "line 1\nline 2\nline 3" >> multiline-hunks.txt
git add multiline-hunks.txt
git commit -q -m c1.1

echo -e "line 1\nline 2" > changed-lines.txt
echo -e "line 1\nline 2\nline 3\nline 4\nline 5\nline 6" >> changed-line-between-unchanged-lines.txt
git add changed-lines.txt
git add changed-line-between-unchanged-lines.txt
git commit -q -m c1.2

echo "line 2" >> added-lines.txt
echo "line 2" >> added-lines-around.txt
echo -e "line 1\nline 2" > coalesce-adjacent-hunks.txt
git add added-lines.txt
git add added-lines-around.txt
git add coalesce-adjacent-hunks.txt
git commit -q -m c1.3

echo "line 1 in renamed file" >> before-rename.txt
echo "line 1 in file renamed twice" >> before-first-rename.txt
echo -e "line 1 in renamed and rewritten file\nline 2\nline 3\nline 4\nline 5\nline 6" >> before-rewrite.txt
echo -e "line 1 in file moved to sub-directory" > before-move-to-sub-directory.txt
git add before-rename.txt
git add before-first-rename.txt
git add before-rewrite.txt
git add before-move-to-sub-directory.txt
git commit -q -m c1.4

echo "line 2" >> simple.txt
git add simple.txt
git commit -q -m c2

echo -e "line 4\nline 5\nline 6" >> multiline-hunks.txt
git add multiline-hunks.txt
git commit -q -m c2.1

echo -e "line 1\nline 2\nline 3\nline 4\nline 5\nline 6" >> deleted-lines.txt
echo -e "line 1\nline 2\nline 3\nline 4\nline 5\nline 6" >> deleted-lines-multiple-hunks.txt
git add deleted-lines.txt
git add deleted-lines-multiple-hunks.txt
git commit -q -m c2.2

echo -e "line 1\nline 2\nline 3" > added-line-before-changed-line.txt
git add added-line-before-changed-line.txt
git commit -q -m c2.3

echo -e "line 1\nline 2" > same-line-changed-twice.txt
echo -e "line 1\nline in between\nline 2" > coalesce-adjacent-hunks.txt
git add same-line-changed-twice.txt
git add coalesce-adjacent-hunks.txt
git commit -q -m c2.4

mkdir sub-directory
echo -e "line 1\nline 2" > sub-directory/sub-directory.txt
mv before-move-to-sub-directory.txt sub-directory/after-move-to-sub-directory.txt
git add sub-directory/sub-directory.txt
git add before-move-to-sub-directory.txt sub-directory/after-move-to-sub-directory.txt
git commit -q -m c2.5

echo "line 3" >> simple.txt
git add simple.txt
git commit -q -m c3

echo -e "line 3\nline 4" > deleted-lines.txt
echo -e "line 2\nline 4" > deleted-lines-multiple-hunks.txt
git add deleted-lines.txt
git add deleted-lines-multiple-hunks.txt
git commit -q -m c3.1

echo -e "line 3\nline 4" > changed-lines.txt
echo -e "line 1\nline 2\nline 3 changed\nline 4\nline 5\nline 6" > changed-line-between-unchanged-lines.txt
git add changed-lines.txt
git add changed-line-between-unchanged-lines.txt
git commit -q -m c3.2

echo -e "line 2\nline 3" > added-line-before-changed-line.txt
echo -e "line 1\nline 2" > coalesce-adjacent-hunks.txt
git add added-line-before-changed-line.txt
git add coalesce-adjacent-hunks.txt
git commit -q -m c3.3

echo -e "line 1\nline 2 changed" > same-line-changed-twice.txt
git add same-line-changed-twice.txt
git commit -q -m c3.4

mv before-rename.txt after-rename.txt
mv before-first-rename.txt before-second-rename.txt
rm before-rewrite.txt
echo -e "line 1 in renamed and rewritten file\nline 2 changed\nline 3 changed\nline 4\nline 5\nline 6" >> after-rewrite.txt
git add before-rename.txt after-rename.txt
git add before-first-rename.txt before-second-rename.txt
git add before-rewrite.txt after-rewrite.txt
git commit -q -m c3.5

echo "line 4" >> simple.txt
git add simple.txt
git commit -q -m c4

echo -e "line 7\nline 8\nline 9" >> multiline-hunks.txt
git add multiline-hunks.txt
git commit -q -m c4.1

echo -e "line 1\nline 3\nline 2\nline 4" > switched-lines.txt
git add switched-lines.txt
git commit -q -m c4.2

echo -e "line 2 changed\nline 3" > added-line-before-changed-line.txt
git add added-line-before-changed-line.txt
git commit -q -m c4.3

echo -e "line 1\nline 2 changed a second time" > same-line-changed-twice.txt
git add same-line-changed-twice.txt
git commit -q -m c4.4

echo -e "  line 1\n\n  line 2\n\n  line 3" > empty-lines-histogram.txt
cp empty-lines-histogram.txt empty-lines-myers.txt
git add empty-lines-histogram.txt empty-lines-myers.txt
git commit -q -m c4.5

echo -e "line 0\nline 2\nline 3" > sub-directory/sub-directory.txt
git add sub-directory/sub-directory.txt
git commit -q -m c4.6

echo -e "line 0\nline 1\nline 2" > added-lines.txt
echo -e "line 0\nline 1\nline 2\nline 3" > added-lines-around.txt
git add added-lines.txt
git add added-lines-around.txt
git commit -q -m c5

echo -e "line 4" > deleted-lines.txt
git add deleted-lines.txt
git commit -q -m c5.1

echo -e "line 1\nline 2\nline 3\nline 4" > switched-lines.txt
git add switched-lines.txt
git commit -q -m c5.2

echo -e "line 1\nline 2 changed\nline 3" > added-line-before-changed-line.txt
git add added-line-before-changed-line.txt
git commit -q -m c5.3

echo -e "  line 1\n\n  line in between\n\n  line 2\n\n  line in between\n\n  line 3" > empty-lines-histogram.txt
cp empty-lines-histogram.txt empty-lines-myers.txt
git add empty-lines-histogram.txt empty-lines-myers.txt
git commit -q -m c5.4

mv before-second-rename.txt after-second-rename.txt
git add before-second-rename.txt after-second-rename.txt
git commit -q -m c5.5

# Git's blame leaves out most of the end that both versions of a file share before diffing them: whole blocks of
# 1024 bytes, less the rest of the line the cut falls in. Each file below is a run of equal lines that gets one more
# such line, which a diff could place anywhere in the run. Git places it at the end of what it diffs, so it is blamed
# for line 491 of 1002 and line 20 of 702 rather than for the last line. With lines of two bytes the cut falls at the
# start of a line, with lines of three bytes within one.
echo "line 1" > shared-end-cut-at-line-start.txt
printf 'r\n%.0s' $(seq 1000) >> shared-end-cut-at-line-start.txt
echo "line 1" > shared-end-cut-within-line.txt
printf 'rr\n%.0s' $(seq 700) >> shared-end-cut-within-line.txt
git add shared-end-cut-at-line-start.txt shared-end-cut-within-line.txt
git commit -q -m c5.6

echo "r" >> shared-end-cut-at-line-start.txt
echo "rr" >> shared-end-cut-within-line.txt
git add shared-end-cut-at-line-start.txt shared-end-cut-within-line.txt
git commit -q -m c5.7

# The commit history created by the commits above this line is linear, it only
# contains commits that have exactly one parent.
# Below this line, there’s also commits that have more than one parent.

echo -e "line 1 original\nline 2\n line 3" > resolved-conflict.txt
git add resolved-conflict.txt
git commit -q -m c6

echo -e "line 1 changed\nline 2\n line 3" > resolved-conflict.txt
git add resolved-conflict.txt
git commit -q -m c7

git checkout -b different-branch-to-create-a-conflict
git reset --hard HEAD~1

echo -e "line 1 changed in a different way\nline 2\n line 3" > resolved-conflict.txt
git add resolved-conflict.txt
git commit -q -m c8

git checkout main
git merge different-branch-to-create-a-conflict || true

echo -e "line 1 conflict resolved\nline 2\n line 3" > resolved-conflict.txt
git add resolved-conflict.txt
git commit -q -m c9

echo -e "line 1\nline 2\n line 3" > file-in-one-chain-of-ancestors.txt
git add file-in-one-chain-of-ancestors.txt
git commit -q -m c10

git checkout -b different-branch-that-does-not-contain-file
git reset --hard HEAD~1

echo -e "line 4\nline 5\n line 6" > different-file-in-another-chain-of-ancestors.txt
git add different-file-in-another-chain-of-ancestors.txt
git commit -q -m c11

git checkout main
git merge different-branch-that-does-not-contain-file || true

echo -e "line 1\nline 2\n line 3" > file-only-changed-in-branch.txt
git add file-only-changed-in-branch.txt
git commit -q -m c12

git checkout -b branch-that-has-one-commit

echo -e "line 1 changed\nline 2\n line 3" > file-only-changed-in-branch.txt
git add file-only-changed-in-branch.txt
git commit -q -m c13

git checkout main
git merge branch-that-has-one-commit || true

echo -e "line 1\nline 2\n line 3" > file-changed-in-two-branches.txt
git add file-changed-in-two-branches.txt
git commit -q -m c14

git checkout -b branch-that-has-one-of-the-changes

echo -e "line 1\nline 2\n line 3 changed" > file-changed-in-two-branches.txt
git add file-changed-in-two-branches.txt
git commit -q -m c14.1

git checkout main
echo -e "line 1 changed\nline 2\n line 3" > file-changed-in-two-branches.txt
git add file-changed-in-two-branches.txt
git commit -q -m c14.2

git merge branch-that-has-one-of-the-changes || true

# This is to verify that, even though commits `c15`, `c15.1` and `merge` are
# not chronologically ordered (`c15.1` has a `CommitDate` that is before
# `c15`’s `CommitDate`), the resulting blame is still correct.
#
# ---c15------c15.2
#     \        \
#      c15.1----merge
echo -e "line 1\nline 2\n line 3" > file-topo-order-different-than-date-order.txt
git add file-topo-order-different-than-date-order.txt
git commit -q -m c15

git checkout -b branch-that-has-earlier-commit

echo -e "line 1\nline 2\n line 3 changed" > file-topo-order-different-than-date-order.txt
git add file-topo-order-different-than-date-order.txt
# `GIT_COMMITTER_DATE` is set to "2000-01-02 00:00:00 +0000" in
# `tests/tools/src/lib.rs` and is the default `CommitDate` used in
# `make_blame_repo.sh`.
GIT_COMMITTER_DATE="1999-12-31 00:00:00 +0000" git commit -q -m c15.1

git checkout main
echo -e "line 1 changed\nline 2\n line 3" > file-topo-order-different-than-date-order.txt
git add file-topo-order-different-than-date-order.txt
git commit -q -m c15.2

git merge branch-that-has-earlier-commit || true

seq 1 4 > untracked-lines.txt
git add untracked-lines.txt
git commit -q -m c16

# Git's blame finds a merge's file in every parent before it passes blame to any of them, and gives all of it to the
# first parent with the same content: under the same name or, when no parent has it there, under the name that rename
# tracking pairs with the file. Here the first parent renamed the file and added a first and a last line, the second
# parent added the same first line under the old name, and the merge has the second parent's content under the new
# name, so Git blames the first line on c17.1 rather than on c17.2.
echo -e "line 2\nline 3\nline 4\nline 5" > before-rename-in-first-parent.txt
git add before-rename-in-first-parent.txt
git commit -q -m c17

git checkout -b branch-that-keeps-the-old-name
echo -e "line 1\nline 2\nline 3\nline 4\nline 5" > before-rename-in-first-parent.txt
git add before-rename-in-first-parent.txt
git commit -q -m c17.1

git checkout main
git mv before-rename-in-first-parent.txt after-rename-in-first-parent.txt
echo -e "line 1\nline 2\nline 3\nline 4\nline 5\nline 6" > after-rename-in-first-parent.txt
git add after-rename-in-first-parent.txt
git commit -q -m c17.2

git merge -q -s ours --no-commit branch-that-keeps-the-old-name
echo -e "line 1\nline 2\nline 3\nline 4\nline 5" > after-rename-in-first-parent.txt
git add after-rename-in-first-parent.txt
git commit -q -m c17.3

# Only the executable bit of this file changes: both versions are regular files, so Git's blame gives all lines to the
# parent, which has the same blob.
seq 1 3 > executable-bit-changed.txt
git add executable-bit-changed.txt
git commit -q -m c18

chmod +x executable-bit-changed.txt
git update-index --chmod=+x executable-bit-changed.txt
git commit -q -m c18.1

seq 5 7 >> untracked-lines.txt

seq 1 4 >> untracked-file.txt
git add --intent-to-add untracked-file.txt

git blame --porcelain simple.txt > .git/simple.baseline
git blame --porcelain -L 1,2 simple.txt > .git/simple-lines-1-2.baseline
git blame --porcelain -L 1,2 -L 4 simple.txt > .git/simple-lines-multiple-1-2-and-4.baseline
git blame --porcelain --since 2025-01-31 simple.txt > .git/simple-since.baseline
git blame --porcelain multiline-hunks.txt > .git/multiline-hunks.baseline
git blame --porcelain deleted-lines.txt > .git/deleted-lines.baseline
git blame --porcelain deleted-lines-multiple-hunks.txt > .git/deleted-lines-multiple-hunks.baseline
git blame --porcelain changed-lines.txt > .git/changed-lines.baseline
git blame --porcelain changed-line-between-unchanged-lines.txt > .git/changed-line-between-unchanged-lines.baseline
git blame --porcelain added-lines.txt > .git/added-lines.baseline
git blame --porcelain added-lines-around.txt > .git/added-lines-around.baseline
git blame --porcelain switched-lines.txt > .git/switched-lines.baseline
git blame --porcelain added-line-before-changed-line.txt > .git/added-line-before-changed-line.baseline
git blame --porcelain same-line-changed-twice.txt > .git/same-line-changed-twice.baseline
git blame --porcelain coalesce-adjacent-hunks.txt > .git/coalesce-adjacent-hunks.baseline
git blame --porcelain shared-end-cut-at-line-start.txt > .git/shared-end-cut-at-line-start.baseline
git blame --porcelain shared-end-cut-within-line.txt > .git/shared-end-cut-within-line.baseline
git blame --porcelain untracked-lines.txt > .git/untracked-lines.baseline
git blame --porcelain -L 3,7 untracked-lines.txt > .git/untracked-lines-ranges.baseline
git blame --porcelain untracked-file.txt > .git/untracked-file.baseline

mkdir .git/sub-directory
git blame --porcelain sub-directory/sub-directory.txt > .git/sub-directory/sub-directory.baseline

git blame --porcelain after-rename.txt > .git/after-rename.baseline
git blame --porcelain after-second-rename.txt > .git/after-second-rename.baseline
git blame --porcelain after-rewrite.txt > .git/after-rewrite.baseline
git blame --porcelain sub-directory/after-move-to-sub-directory.txt > .git/sub-directory/after-move-to-sub-directory.baseline

git blame --porcelain resolved-conflict.txt > .git/resolved-conflict.baseline
git blame --porcelain file-in-one-chain-of-ancestors.txt > .git/file-in-one-chain-of-ancestors.baseline
git blame --porcelain different-file-in-another-chain-of-ancestors.txt > .git/different-file-in-another-chain-of-ancestors.baseline
git blame --porcelain file-only-changed-in-branch.txt > .git/file-only-changed-in-branch.baseline
git blame --porcelain file-changed-in-two-branches.txt > .git/file-changed-in-two-branches.baseline
git blame --porcelain file-topo-order-different-than-date-order.txt > .git/file-topo-order-different-than-date-order.baseline
git blame --porcelain after-rename-in-first-parent.txt > .git/after-rename-in-first-parent.baseline
git blame --porcelain executable-bit-changed.txt > .git/executable-bit-changed.baseline

git blame --porcelain empty-lines-histogram.txt > .git/empty-lines-histogram.baseline

git config --local diff.algorithm myers

git blame --porcelain empty-lines-myers.txt > .git/empty-lines-myers.baseline
