#!/usr/bin/env bash
set -eu -o pipefail

# Each scenario is a repository `<name>` as it is before a reset, in which the tests reset with `gix`, and its copy
# `<name>-git`, in which `git reset` did the same or refused to, with its standard error in `<name>-git.err` then.

# Fail if the command that follows succeeds.
function fails() {
  if "$@"; then
    echo "unexpectedly succeeded: $*" >&2
    exit 1
  fi
}

# Copy the repository `$1` to `$1-git`, in which Git acts.
function twin() {
  cp -R "$1" "$1-git"
  # The copies of the files don't have the stat information of the index anymore, which Git compares before it
  # resets without refreshing the index first.
  if test -d "$1-git/.git"; then
    git -C "$1-git" update-index -q --unmerged --refresh
  fi
}

# Create the repository `$1` whose `main` has the commits `first` with `a`, `dir/b` and `c`, `second`, which changes
# `a` and removes `c`, and `third`, which changes `dir/b` and adds `d`.
function history() {
  git init -q "$1"
  echo a >"$1/a"
  mkdir "$1/dir"
  echo b >"$1/dir/b"
  echo c >"$1/c"
  git -C "$1" add a dir/b c
  git -C "$1" commit -q -m first
  echo a2 >"$1/a"
  git -C "$1" rm -q c
  git -C "$1" commit -q -am second
  echo b3 >"$1/dir/b"
  echo d >"$1/d"
  git -C "$1" add dir/b d
  git -C "$1" commit -q -m third
}

# Create the repository `$1` like `history`, with a change of `a` in the index, a change of `dir/b` in the worktree and
# the untracked file `untracked`.
function changed() {
  history "$1"
  echo staged >"$1/a"
  git -C "$1" add a
  echo unstaged >"$1/dir/b"
  echo untracked >"$1/untracked"
}

# Create the repository `$1` like `history`, with `topic`, which changes `a` differently than `main`, merged into `main`
# with a conflict.
function conflicted() {
  history "$1"
  git -C "$1" checkout -q -b topic HEAD~1
  echo topic >"$1/a"
  git -C "$1" commit -q -am topic
  git -C "$1" checkout -q main
  echo main >"$1/a"
  git -C "$1" commit -q -am "main changes a"
  fails git -C "$1" merge -q --no-commit topic
}

changed soft
twin soft
git -C soft-git reset -q --soft HEAD~1

changed mixed
twin mixed
git -C mixed-git reset -q HEAD~1

changed mixed-head
twin mixed-head
git -C mixed-head-git reset -q

changed hard
echo "untracked c" >hard/c
twin hard
git -C hard-git reset -q --hard HEAD~2

history merge
echo unstaged >merge/a
echo untracked >merge/untracked
twin merge
git -C merge-git reset -q --merge HEAD~1

history merge-refused
echo unstaged >merge-refused/dir/b
twin merge-refused
fails git -C merge-refused-git reset -q --merge HEAD~1 2>merge-refused-git.err

history keep
echo unstaged >keep/a
echo untracked >keep/untracked
twin keep
git -C keep-git reset -q --keep HEAD~1

history keep-refused
echo unstaged >keep-refused/dir/b
twin keep-refused
fails git -C keep-refused-git reset -q --keep HEAD~1 2>keep-refused-git.err

conflicted soft-in-merge
twin soft-in-merge
fails git -C soft-in-merge-git reset -q --soft HEAD~1 2>soft-in-merge-git.err

conflicted keep-in-merge
twin keep-in-merge
fails git -C keep-in-merge-git reset -q --keep HEAD~1 2>keep-in-merge-git.err

conflicted hard-in-merge
twin hard-in-merge
git -C hard-in-merge-git reset -q --hard

conflicted abort-merge
twin abort-merge
git -C abort-merge-git reset -q --merge

changed paths
twin paths
git -C paths-git reset -q HEAD~2 -- a dir d

changed paths-head
twin paths-head
git -C paths-head-git reset -q -- a

git init -q unborn
echo a >unborn/a
git -C unborn add a
twin unborn
git -C unborn-git reset -q

git init -q unborn-hard
echo a >unborn-hard/a
git -C unborn-hard add a
echo untracked >unborn-hard/untracked
twin unborn-hard
git -C unborn-hard-git reset -q --hard

git init -q unborn-keep
echo a >unborn-keep/a
git -C unborn-keep add a
twin unborn-keep
fails git -C unborn-keep-git reset -q --keep 2>unborn-keep-git.err

history detached
git -C detached checkout -q --detach
twin detached
git -C detached-git reset -q --hard HEAD~1

history reflog-action
twin reflog-action
GIT_REFLOG_ACTION=custom git -C reflog-action-git reset -q --hard HEAD~1

history bare-source
git clone -q --bare bare-source bare-soft
twin bare-soft
git -C bare-soft-git reset -q --soft HEAD~1

git clone -q --bare bare-source bare-mixed
twin bare-mixed
fails git -C bare-mixed-git reset -q HEAD~1 2>bare-mixed-git.err
