#!/usr/bin/env bash
set -eu -o pipefail

git init -q
git config --local diff.algorithm histogram

# Each version of a file is converted with the attributes of its path in its commit, as Git's blame converts it.
# `-diff` makes the versions of `renamed.bin` binary to the diff of `gix-blame`, but not the versions of `first.txt`
# and `second.txt`, which the commits before the last rename changed.
echo '*.bin -diff' > .gitattributes
seq 1 2 > first.txt
git add .gitattributes first.txt
git commit -q -m c1

seq 1 3 > first.txt
git add first.txt
git commit -q -m c2

git mv first.txt second.txt
seq 1 4 > second.txt
git add second.txt
git commit -q -m c3

git mv second.txt renamed.bin
git commit -q -m c4

git blame --porcelain renamed.bin > .git/renamed.baseline
