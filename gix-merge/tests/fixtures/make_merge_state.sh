#!/usr/bin/env bash
set -eu -o pipefail

# Each case is a repository in which `git merge` stopped before its commit and left the state of the merge in `.git`:
# `MERGE_HEAD`, `MERGE_MSG` and `MERGE_MODE`, which `gix-merge` reads and writes byte for byte, and with conflicts
# `AUTO_MERGE`. The ids of the commits that `MERGE_HEAD` has to list go to `.git/heads`, one per line, as
# `git rev-parse` prints them. Its branches start from a commit `base` on `main` with the files `file` and `dir/file`.

# start <name> creates the repository <name> with the commit `base` on `main`, and enters it.
start() {
  git init -q "$1"
  cd "$1"
  printf '%s\n' one two three > file
  mkdir dir
  echo base > dir/file
  git add --all
  git commit -q -m base
}

# add_on <branch> <file> creates <branch> from `main` if it doesn't exist, and commits <file> on it with its name as
# content. `main` is checked out afterwards.
add_on() {
  git switch -q "$1" 2>/dev/null || git switch -q -c "$1" main
  echo "$2" > "$2"
  git add "$2"
  git commit -q -m "$2"
  git switch -q main
}

# conflicting_side changes `file` and `dir/file` on `side` and on `main` differently, so that merging `side` into
# `main` conflicts in both.
conflicting_side() {
  git switch -q -c side
  printf '%s\n' one side three > file
  echo side > dir/file
  git commit -q -am side
  git switch -q main
  printf '%s\n' one main three > file
  echo main > dir/file
  git commit -q -am main
}

# merge_with_conflicts <option>... merges `side` into `main` with the given configuration, which has to stop with
# conflicts.
merge_with_conflicts() {
  if git "$@" merge -q side >/dev/null 2>&1; then
    echo "merging 'side' has to stop with conflicts" >&2
    exit 1
  fi
}

# A merge without conflicts that `--no-commit` stops before its commit. `MERGE_MODE` is empty.
(start no-commit
  add_on side side
  add_on main main
  git rev-parse side > .git/heads
  git merge -q --no-commit side 2>/dev/null
)

# A merge that could fast-forward, but `--no-ff` makes it a merge commit, which `--no-commit` stops before. `MERGE_MODE`
# holds `no-ff`.
(start no-ff
  add_on side side
  git rev-parse side > .git/heads
  git merge -q --no-ff --no-commit side 2>/dev/null
)

# A merge with conflicts in `dir/file` and `file`, whose message ends with the hint that lists them.
(start conflict
  conflicting_side
  git rev-parse side > .git/heads
  merge_with_conflicts
)

# A merge with conflicts while `commit.cleanup` is `scissors`, whose hint starts with the scissors line.
(start scissors
  conflicting_side
  git rev-parse side > .git/heads
  merge_with_conflicts -c commit.cleanup=scissors
)

# A merge with conflicts while `core.commentChar` is `;`, which starts the lines of the hint.
(start comment
  conflicting_side
  git rev-parse side > .git/heads
  merge_with_conflicts -c core.commentChar=';'
)

# The merge of an annotated tag, which `MERGE_HEAD` lists with the id of the tag, and whose message has the message of
# the tag.
(start tag
  add_on side side
  git tag -a -m 'the message of the tag' tag side
  add_on main main
  git rev-parse tag > .git/heads
  git merge -q --no-commit tag 2>/dev/null
)

# The merge of two branches at once, which `MERGE_HEAD` lists on a line each.
(start octopus
  add_on side side
  add_on other other
  add_on main main
  git rev-parse side other > .git/heads
  git merge -q --no-commit side other 2>/dev/null
)

# A merge with conflicts that `git rerere` records in `MERGE_RR`. `abort-git` is a copy that `git merge --abort` aborted,
# which removes the state of the merge along with `AUTO_MERGE`.
(start abort
  git config rerere.enabled true
  conflicting_side
  merge_with_conflicts
  cp -R . ../abort-git
  cd ../abort-git
  git merge --abort
)
