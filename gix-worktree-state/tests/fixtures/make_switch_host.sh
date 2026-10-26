#!/usr/bin/env bash
set -eu -o pipefail

# The cases whose results depend on the file system of the host: whether it has executable bits, and whether it ignores
# the case of file names. `git init` sets `core.filemode` and `core.ignorecase` accordingly. The results aren't archived
# but recorded on each host, by `record` as in `make_switch.sh`. The branch `new` of the cases with names that differ in
# case is made without its worktree, which can't hold both names on a file system that ignores case.

# start_case <name> creates the repository of a case and enters it.
start_case() {
  git init -q "$1"
  cd "$1"
}

# new_branch <ls-tree lines> creates the branch `new` with a commit on top of `HEAD` whose tree has these entries.
new_branch() {
  local tree
  tree=$(printf '%s' "$1" | git mktree)
  git branch new "$(git commit-tree -p HEAD -m new "$tree")"
}

# record switches a copy of the case to `new` as `make_switch.sh` describes it.
record() {
  local copy
  copy="../$(basename "$PWD")-git"
  git rev-parse 'HEAD^{tree}' > .git/old-tree
  git rev-parse 'new^{tree}' > .git/new-tree
  cp -R . "$copy"
  if (cd "$copy" && LC_ALL=C git -c advice.commitBeforeMerge=false switch -q new) > "$copy/.git/error" 2>&1; then
    rm "$copy/.git/error"
  fi
}

# The executable bits of files change with the modes of their entries, if the file system has them. The modes are set
# in the index as well, as `git add` doesn't take them from a file system without executable bits.
(start_case exec
  echo x > x
  echo y > y
  chmod +x y
  git add x y
  git update-index --chmod=+x y
  git commit -q -m old
  git switch -q -c new
  chmod +x x
  chmod -x y
  git update-index --chmod=+x x
  git update-index --chmod=-x y
  git commit -q -m new
  git switch -q main
  record
)

# A file whose executable bit changed is an unstaged change, which is refused if the new tree changes its entry and the
# file system has executable bits.
(start_case exec-local
  echo z > z
  git add z
  git commit -q -m old
  git switch -q -c new
  echo z2 > z
  git commit -q -am new
  git switch -q main
  chmod +x z
  record
)

# On a file system that ignores case, the file of an entry that the switch removes is in the way of an entry whose path
# only differs in case, which replaces it.
(start_case case-rename
  echo a > a
  git add a
  git commit -q -m old
  new_branch "100644 blob $(echo A | git hash-object -w --stdin)	A"
  record
)

# On a file system that ignores case, the file of an entry that stays is in the way of an entry whose path only differs
# in case. Git takes it for the file of the new entry and replaces it.
(start_case icase-kept
  echo a > a
  git add a
  git commit -q -m old
  new_branch "$(git ls-tree HEAD)
100644 blob $(echo A | git hash-object -w --stdin)	A"
  record
)
