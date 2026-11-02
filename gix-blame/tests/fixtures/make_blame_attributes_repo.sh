#!/usr/bin/env bash
set -eu -o pipefail

git init -q
git config --local diff.algorithm histogram

# Git's blame compares the versions of a file line by line as they are stored, whatever their attributes and data.
# `-diff` makes the versions of `renamed.bin` binary to `git diff`, but not the versions of `first.txt` and `second.txt`,
# which the commits before the last rename changed, and the NUL byte of `with-nul.dat` makes `git diff` take it for
# binary data. The name `nul` with any extension would name a device on Windows.
echo '*.bin -diff' > .gitattributes
seq 1 2 > first.txt
printf 'a\nb\0c\nd\n' > with-nul.dat
git add .gitattributes first.txt with-nul.dat
git commit -q -m c1

seq 1 3 > first.txt
printf 'a\nb\0c\nx\nd\n' > with-nul.dat
git add first.txt with-nul.dat
git commit -q -m c2

git mv first.txt second.txt
seq 1 4 > second.txt
git add second.txt
git commit -q -m c3

git mv second.txt renamed.bin
git commit -q -m c4

seq 0 5 > renamed.bin
printf 'z\na\nb\0c\nx\nd\n' > with-nul.dat
git add renamed.bin with-nul.dat
git commit -q -m c5

git blame --porcelain renamed.bin > .git/renamed.baseline
git blame --porcelain with-nul.dat > .git/with-nul.baseline

# Uncommitted lines, which Git's blame finds by comparing the worktree files with the versions of `HEAD` the same way.
echo 6 >> renamed.bin
printf 'y\n' >> with-nul.dat

git blame --porcelain renamed.bin > .git/renamed-uncommitted.baseline
git blame --porcelain with-nul.dat > .git/with-nul-uncommitted.baseline
