#!/usr/bin/env bash
set -eu -o pipefail

# `core.bigFileThreshold` is 100 bytes, and every version of `clean` and `conflict` has at least 160 bytes. A and B
# change different lines of `clean` and the same line of `conflict`. Git ignores `core.bigFileThreshold` when it
# merges, so `clean` merges cleanly and `conflict` gets conflict markers. `merge-tree.baseline` is the merged tree
# and the conflicted files, as `git merge-tree` prints them.

# write_file <file> <number> <text> writes the lines `line 01` to `line 20`, with <text> as line <number>.
function write_file() {
  local file=$1 number=$2 text=$3 i
  for i in $(seq 1 20); do
    if [ "$i" -eq "$number" ]; then
      echo "$text"
    else
      printf 'line %02d\n' "$i"
    fi
  done >"$file"
}

git init -q -b main
git config core.bigFileThreshold 100
write_file clean 0 ''
write_file conflict 0 ''
git add clean conflict
git commit -q -m base

git checkout -q -b A
write_file clean 2 'changed by A'
write_file conflict 10 'changed by A'
git commit -q -am A

git checkout -q -b B main
write_file clean 19 'changed by B'
write_file conflict 10 'changed by B'
git commit -q -am B

git checkout -q main
status=0
git merge-tree --write-tree --name-only A B >merge-tree.baseline || status=$?
test "$status" -eq 1
