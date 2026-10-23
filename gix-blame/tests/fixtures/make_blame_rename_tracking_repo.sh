#!/usr/bin/env bash
set -eu -o pipefail

git init -q
git config --local diff.algorithm histogram

git config merge.ff false

git checkout -q -b main

seq 1 4 > before-rename.txt
git add before-rename.txt
git commit -q -m c1

mv before-rename.txt after-rename.txt
git add before-rename.txt after-rename.txt
git commit -q -m c2

seq 1 5 > after-rename.txt
git add after-rename.txt
git commit -q -m c3

git checkout -b different-branch
git reset --hard HEAD~2

seq 0 4 > before-rename.txt
git add before-rename.txt
git commit -q -m c10

mv before-rename.txt after-rename.txt
git add before-rename.txt after-rename.txt
git commit -q -m c11

git checkout main
git merge different-branch || true

git blame --porcelain after-rename.txt > .git/after-rename.baseline

echo -e "1\n2\n3\n4\n5\n" > change-and-rename.txt
git add change-and-rename.txt
git commit -q -m c2.1.1

echo -e "1\ntwo\n3\n4\n5\n" > change-and-rename.txt
git add change-and-rename.txt
git commit -q -m c2.1.2

git checkout -b branch-that-renames-file
git reset --hard HEAD~1

echo -e "1\n2\n3\nfour\n5\n" > change-and-rename.txt
git add change-and-rename.txt
git commit -q -m c2.2.1

mv change-and-rename.txt change-and-renamed.txt
git add change-and-rename.txt change-and-renamed.txt
git commit -q -m c2.2.2

echo -e "1\n2\n3\nfour\nfive\n" > change-and-renamed.txt
git add change-and-renamed.txt
git commit -q -m c2.2.2

git checkout main
git merge branch-that-renames-file || true
git add change-and-rename.txt
git commit --no-edit

git blame --porcelain change-and-renamed.txt > .git/change-and-renamed.baseline

# Git's blame follows a file that a commit added at its path to the file of the parent that the commit deleted and
# that is the most similar to it, and the added file is the only one paired with a deleted file. `closer.txt`, which
# the same commit added, is more similar to `source.txt` than `followed.txt` is, so `git diff -M` pairs `source.txt`
# with `closer.txt`, yet the blame of `followed.txt` follows it to `source.txt`.
seq 1 20 > source.txt
git add source.txt
git commit -q -m c20

git mv source.txt followed.txt
seq 1 20 | sed -e 's/^5$/x5/' -e 's/^10$/x10/' -e 's/^15$/x15/' > followed.txt
seq 1 20 | sed -e 's/^20$/y20/' > closer.txt
git add followed.txt closer.txt
git commit -q -m c21

echo extra >> followed.txt
git add followed.txt
git commit -q -m c22

git blame --porcelain followed.txt > .git/followed.baseline

# A file that replaced a directory is followed to the file of that directory that is the most similar to it.
mkdir dir-then-file
seq 101 110 > dir-then-file/a.txt
seq 201 203 > dir-then-file/b.txt
git add dir-then-file
git commit -q -m c30

git rm -q -r dir-then-file
seq 101 111 > dir-then-file
git add dir-then-file
git commit -q -m c31

git blame --porcelain dir-then-file > .git/dir-then-file.baseline
