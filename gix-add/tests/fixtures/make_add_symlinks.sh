#!/usr/bin/env bash
set -eu -o pipefail

# The cases with symbolic links, which aren't archived as archives can't represent them on all hosts, and which need a
# host that can create them. `record` runs `git add --all` as in `make_add.sh`.

# start_case <name> creates the repository of a case and enters it. Each case leaves it again at its end.
start_case() {
  git init -q "$1"
  cd "$1"
}

# commit <message> commits all changes of the worktree.
commit() {
  git add --all
  git commit -q -m "$1"
}

# record runs `git add --all` as `make_add.sh` describes it.
record() {
  cp .git/index .git/index.after
  mkdir .git/git-objects
  if GIT_INDEX_FILE=.git/index.after GIT_OBJECT_DIRECTORY=.git/git-objects GIT_ALTERNATE_OBJECT_DIRECTORIES=.git/objects \
    LC_ALL=C git add --all 2> .git/error; then
    rm .git/error
  fi
}

# Symbolic links are added with their targets, without following those that lead to directories.
(start_case symlinks
  echo target > target
  mkdir dir
  echo file > dir/file
  commit base
  ln -s target link
  ln -s dir dir-link
  record
)

# A symbolic link that replaced a tracked file is added in its place.
(start_case file-to-symlink
  echo a > a
  echo target > target
  commit base
  rm a
  ln -s target a
  record
)

# A symbolic link that replaced a tracked directory is added, and the entries of the directory are removed although the
# link leads to a directory with files of the same names.
(start_case directory-to-symlink
  mkdir x elsewhere
  echo y > x/y
  echo y > elsewhere/y
  commit base
  rm -r x
  ln -s elsewhere x
  record
)
