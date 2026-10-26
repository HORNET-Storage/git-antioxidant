#!/usr/bin/env bash
set -eu -o pipefail

# The cases whose results depend on the file system of the host: whether it has executable bits, and whether it ignores
# the case of file names. `git init` sets `core.filemode` and `core.ignorecase` accordingly. The results aren't archived
# but recorded on each host, by `record` as in `make_add.sh`.

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

# With `core.filemode`, files are added with the executable bits that they have. Without it, entries keep their modes,
# and new files are added as regular files.
(start_case executable-bits
  echo a > a
  echo x > x
  git add a x
  git update-index --chmod=+x x
  git commit -q -m base
  echo a2 > a
  chmod +x a
  echo x2 > x
  echo new > new
  chmod +x new
  record
)

# A tracked file that was renamed to another case of its name and changed. With `core.ignorecase`, its entry keeps the
# name. Without it, that entry is removed and the file is added with its new name.
(start_case case-rename
  echo A > A.txt
  commit base
  mv A.txt tmp.txt
  mv tmp.txt a.txt
  echo changed > a.txt
  record
)
