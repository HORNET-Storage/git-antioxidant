#!/usr/bin/env bash
set -eu -o pipefail

# Each case is a repository with an old and a new tree, between which `git diff -C` finds copies and renames. The files
# are added to the index directly, and nothing is checked out. `trees` lists the ids of both trees,
# `C-<percentage>-<limit>.expected` records `git diff --name-status -z -C<percentage>%` with
# `diff.renameLimit=<limit>`, `CC-<percentage>-<limit>.expected` the same with `--find-copies-harder`, and the files
# ending in `.warnings` instead what git printed to its standard error.
#
# The files consist of lines of 20 bytes, so that the similarity of two files follows from the lines they share: each
# line is a span of its own, and a file of 40 lines that shares 36 of them with another is 90% similar to it.

# start_case <name> creates the repository of a case and enters it. Each case leaves it again at its end.
start_case() {
  git init -q "$1"
  cd "$1"
}

# add <mode> <path> adds what the standard input provides as the file <path> of the index, replacing what it had there.
add() {
  local id
  id=$(git hash-object -w --stdin)
  git update-index --add --cacheinfo "$1,$id,$2"
}

# remove <path> removes the file <path> from the index.
remove() {
  git update-index --force-remove "$1"
}

# lines <family> <first> <last> prints the numbered lines <first> to <last> of a family, each of 20 bytes.
lines() {
  local number
  for ((number = $2; number <= $3; number++)); do
    printf '%-12s %06d\n' "$1" "$number"
  done
}

# old_tree writes the index as the old tree, and keeps it to be changed into the new one.
old_tree() {
  old=$(git write-tree)
}

# new_tree writes the index as the new tree.
new_tree() {
  echo "$old $(git write-tree)" > trees
}

# record <C|CC> <percentage> <limit> records what `git diff -C<percentage>%` reports with `diff.renameLimit=<limit>`, with
# `--find-copies-harder` for `CC`, in the C locale, which keeps the warnings untranslated.
record() {
  local old new harder=
  read -r old new < trees
  if [ "$1" = CC ]; then
    harder=--find-copies-harder
  fi
  LC_ALL=C git -c diff.renameLimit="$3" diff --name-status -z -C"$2%" $harder "$old" "$new" \
    > "$1-$2-$3.expected" 2> "$1-$2-$3.warnings"
}

# A modified file is a source of copies as it was before: `copy.txt` has its old blob, `similar.txt` keeps 90% of it.
start_case modified-source
lines kept 1 40 | add 100644 kept.txt
lines other 1 40 | add 100644 other.txt
old_tree
{
  lines kept 1 39
  lines kept-new 1 1
} | add 100644 kept.txt
lines kept 1 40 | add 100644 copy.txt
{
  lines kept 1 36
  lines similar-new 1 4
} | add 100644 similar.txt
new_tree
record C 50 1000
cd ..

# The destinations of a deleted file: `a.txt` and `b.txt` are identical to `orig.txt`, `c.txt` keeps 90% of it. The
# last of them in the order of their locations is the rename, though it is only similar, and the others are copies.
start_case deleted-source
lines orig 1 40 | add 100644 orig.txt
old_tree
remove orig.txt
lines orig 1 40 | add 100644 a.txt
lines orig 1 40 | add 100644 b.txt
{
  lines orig 1 36
  lines c-new 1 4
} | add 100644 c.txt
new_tree
record C 50 1000
cd ..

# Deleted files are paired first: `dst.txt` keeps 95% of the old `kept.txt` but only 90% of `gone.txt`, which is deleted,
# so it is the rename of `gone.txt`.
start_case renames-first
{
  lines shared 1 36
  lines gone 1 4
} | add 100644 gone.txt
{
  lines shared 1 38
  lines kept 1 2
} | add 100644 kept.txt
old_tree
remove gone.txt
lines kept-new 1 40 | add 100644 kept.txt
{
  lines shared 1 38
  lines dst 1 2
} | add 100644 dst.txt
new_tree
record C 50 1000
cd ..

# Files that didn't change are sources only with `--find-copies-harder`: `exact.txt` has the blob of `same.txt`, and
# `copy.txt` keeps 90% of it.
start_case unchanged-source
lines same 1 40 | add 100644 same.txt
lines mod 1 40 | add 100644 mod.txt
old_tree
{
  lines mod 1 39
  lines mod-new 1 1
} | add 100644 mod.txt
lines same 1 40 | add 100644 exact.txt
{
  lines same 1 36
  lines copy-new 1 4
} | add 100644 copy.txt
new_tree
record C 50 1000
record CC 50 1000
cd ..

# The limit with `--find-copies-harder`: `dst.txt` keeps 90% of the old `mod.txt`, and `dst2.txt` 90% of `u1.txt`, which
# didn't change, like `u2.txt` and `u3.txt`. The 4 sources and 2 destinations are within a limit of 3, but too many for
# a limit of 2 unless the files that didn't change are left out, and too many for a limit of 1 even then.
start_case harder-limit
lines mod 1 40 | add 100644 mod.txt
for name in u1 u2 u3; do
  lines "$name" 1 40 | add 100644 "$name.txt"
done
old_tree
{
  lines mod 1 39
  lines mod-new 1 1
} | add 100644 mod.txt
{
  lines mod 1 36
  lines dst-new 1 4
} | add 100644 dst.txt
{
  lines u1 1 36
  lines dst2-new 1 4
} | add 100644 dst2.txt
new_tree
for limit in 1 2 3; do
  record CC 50 "$limit"
done
cd ..

# Identical sources: a deleted file that is still unpaired counts as much as the file name of the destination, and of
# the sources that count the most, the first in the order of their locations is taken. `x/name.txt` takes the deleted
# `a/other.txt` before the modified `b/name.txt`, and `y/zzz.txt`, which neither has the file name of, takes
# `a/other.txt` again as its rename. `b/thing.txt` takes the modified `c/thing.txt` with its file name before the deleted
# `d/else.txt`, which remains deleted, but `g/eee.txt` takes the deleted `f/ddd.txt` over the modified `e/ccc.txt`.
start_case identical-sources
lines same 1 40 | add 100644 a/other.txt
lines same 1 40 | add 100644 b/name.txt
lines more 1 40 | add 100644 c/thing.txt
lines more 1 40 | add 100644 d/else.txt
lines third 1 40 | add 100644 e/ccc.txt
lines third 1 40 | add 100644 f/ddd.txt
lines same 1 40 | add 100644 u/name.txt
old_tree
remove a/other.txt
lines changed 1 40 | add 100644 b/name.txt
lines changed-too 1 40 | add 100644 c/thing.txt
remove d/else.txt
lines same 1 40 | add 100644 x/name.txt
lines same 1 40 | add 100644 y/zzz.txt
lines changed-three 1 40 | add 100644 e/ccc.txt
remove f/ddd.txt
lines more 1 40 | add 100644 b/thing.txt
lines third 1 40 | add 100644 g/eee.txt
new_tree
record C 50 1000
record CC 50 1000
cd ..

# The limit applies after identical files are paired: `exact.txt` has the old blob of `m1.txt`, and `sim.txt` keeps 90%
# of the old `m2.txt`, which leaves 2 sources and 2 destinations, more than a limit of 1 allows but not a limit of 2.
start_case limit
lines m1 1 40 | add 100644 m1.txt
lines m2 1 40 | add 100644 m2.txt
old_tree
{
  lines m1 1 39
  lines m1-new 1 1
} | add 100644 m1.txt
{
  lines m2 1 39
  lines m2-new 1 1
} | add 100644 m2.txt
lines m1 1 40 | add 100644 exact.txt
{
  lines m2 1 36
  lines sim-new 1 4
} | add 100644 sim.txt
lines other 1 40 | add 100644 other.txt
new_tree
record C 50 1
record C 50 2
cd ..
