#!/usr/bin/env bash
set -eu -o pipefail

# A file whose lines come from two root commits: `main` and the orphan branch `unrelated` each add
# `file-from-two-root-commits.txt` with lines of their own, and the merge `c20` joins both histories and adds a line.
#
# The committer dates are what makes the fixture special: `c10`, the root of `unrelated`, is newer than `c1`, the root
# of `main`, so blame, which takes commits from the newest on, reaches the root commit `c10` while `c1` is still queued.
# The baseline of `git blame --porcelain` credits each root commit with its own lines.

git init -q
git config --local diff.algorithm histogram

git checkout -q -b main

echo -e "line 1 from the first root\nline 2 from the first root" > file-from-two-root-commits.txt
git add file-from-two-root-commits.txt
# The first root commit is the older one.
GIT_COMMITTER_DATE="2000-01-02 00:00:00 +0000" git commit -q -m c1

git checkout -q --orphan unrelated
git rm -q --cached file-from-two-root-commits.txt
echo -e "line 1 from the second root\nline 2 from the second root" > file-from-two-root-commits.txt
git add file-from-two-root-commits.txt
# The second root commit is newer than the first.
GIT_COMMITTER_DATE="2000-01-03 00:00:00 +0000" git commit -q -m c10

git checkout -q main
# Both roots add the file, so the merge stops with a conflict, which the next commit resolves.
git merge -q --allow-unrelated-histories unrelated || true
echo -e "line 1 from the first root\nline 2 from the first root\nline added in the merge\nline 1 from the second root\nline 2 from the second root" > file-from-two-root-commits.txt
git add file-from-two-root-commits.txt
GIT_COMMITTER_DATE="2000-01-04 00:00:00 +0000" git commit -q -m c20

git blame --porcelain file-from-two-root-commits.txt > .git/file-from-two-root-commits.baseline
