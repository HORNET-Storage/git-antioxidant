#!/usr/bin/env bash
set -eu -o pipefail

git init -q

git checkout -q -b main
git commit -q --allow-empty -m c1

current_commit="$(git rev-parse HEAD)"

# Write the references unsorted in one go, as creating 150,000 loose references takes many minutes on Windows.
# Git sorts them when reading, and `git pack-refs` below writes them sorted along with the others.
awk -v commit="$current_commit" 'BEGIN {
  for (level = 1; level <= 1000; level++)
    for (refname = 1; refname <= 150; refname++)
      printf "%s refs/heads/%04d/%d\n", commit, level, refname
}' > .git/packed-refs

git tag t1
git tag dt1

git pack-refs --all
