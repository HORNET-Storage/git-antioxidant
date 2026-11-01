#!/usr/bin/env bash
set -eu -o pipefail

# Trees to walk with `core.maxTreeDepth`, as `t6700-tree-depth.sh` of Git walks them.
#
# - `main` and `side` change the deepest directory of `main~1`, the root of both, in different files, and other
#   directories as well, so that each depth has more than one tree. `side` is checked out.
# - The tag `big` has a file below more directories than any default of Git allows.
# - The file `self-containing-tree` has the id of a tree with a tree that contains itself, as only a corrupt repository
#   has it: a copy of a tree under the id that its entry names.

git init -q
git checkout -q -b main

mkdir -p a/a/a/a/a b c/d
echo one > a/a/a/a/a/file
echo one > a/a/a/a/a/other
echo one > b/file
echo one > c/d/file
git add .
git commit -q -m base

echo main > a/a/a/a/a/file
echo main > b/file
echo main > c/d/file
git commit -q -am main

git checkout -q -b side HEAD~1
echo side > a/a/a/a/a/other
git commit -q -am side

{
  echo "commit refs/tags/big"
  echo "committer foo <foo@example.com> 1234 -0000"
  echo "data <<EOF"
  echo "the commit message"
  echo "EOF"
  printf 'M 100644 inline '
  printf 'a/%.0s' $(seq 1 4100)
  echo "file"
  echo "data <<EOF"
  echo "the file contents"
  echo "EOF"
  echo
} | git fast-import --quiet

empty_tree=$(git hash-object -t tree /dev/null)
self=${empty_tree//?/b}
tree=$(printf '040000 tree %s\tself\n' "$self" | git mktree --missing)
mkdir -p .git/objects/bb
cp ".git/objects/${tree:0:2}/${tree:2}" ".git/objects/bb/${self:2}"
printf '040000 tree %s\tinner\n' "$self" | git mktree > self-containing-tree
