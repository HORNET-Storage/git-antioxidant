#!/usr/bin/env bash
set -eu -o pipefail

# Since version 2.44, git's similarity estimate counts the bytes after the last line feed of a file as a span of their
# own, which earlier versions ignore. `old.txt` and `new.txt` share only their last line of 40 bytes, which no line
# feed ends, so 40 of the 60 bytes of `new.txt` are from `old.txt`, which makes it 66% similar.
#
# As in `make_renames.sh`, the case is a repository with an old and a new tree, whose files are added to the index
# directly. `trees` lists the ids of both trees, `50-1000.expected` records `git diff --name-status -z -M50%` with
# `diff.renameLimit=1000`, and `50-1000.warnings` what git printed to its standard error.

git init -q unterminated-last-line
cd unterminated-last-line

# add <path> adds what the standard input provides as the regular file <path> of the index.
add() {
  local id
  id=$(git hash-object -w --stdin)
  git update-index --add --cacheinfo "100644,$id,$1"
}

{
  printf '%-19s\n' 'the first line'
  printf '%-40s' 'the last line, which no line feed ends'
} | add old.txt
old=$(git write-tree)
git read-tree --empty
{
  printf '%-19s\n' 'a new first line'
  printf '%-40s' 'the last line, which no line feed ends'
} | add new.txt
new=$(git write-tree)
echo "$old $new" > trees
LC_ALL=C git -c diff.renameLimit=1000 diff --name-status -z -M50% "$old" "$new" > 50-1000.expected 2> 50-1000.warnings
