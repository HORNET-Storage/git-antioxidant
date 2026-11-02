#!/usr/bin/env bash
set -eu -o pipefail

# A commit leaves the whole cache tree of the index valid. Adding a changed file below `a` then invalidates the nodes
# of `a` and of the root, which keep no tree id, while `b` and `b/x` stay valid and keep the ids of their trees.
git init -q
mkdir -p a b/x
echo f > a/f
echo g > b/x/g
echo c > c
git add .
git commit -q -m "init"

echo changed > a/f
git add a/f

# Git's fsck checks the cache tree with the objects of its valid nodes, and accepts this index.
git fsck --no-progress --no-dangling
