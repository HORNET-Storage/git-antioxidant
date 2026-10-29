#!/usr/bin/env bash
set -eu -o pipefail

# Each scenario is a repository `<name>` on `main`, in which the tests merge `topic` with `gix` and apply the result to
# the index and the worktree, and its copy `<name>-git`, in which `git merge --no-commit topic` did the same or refused
# to, with its standard error in `<name>-git.err` then.

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
}

# Create the repository `$1` with the commit `base` of `a`, `dir/b`, `c` and `u`, on which `topic` adds a line to `a`,
# adds `t` and removes `c`, and `main` changes `dir/b` and adds `m`.
function diverged() {
  git init -q "$1"
  printf '1\n2\n3\n' >"$1/a"
  mkdir "$1/dir"
  echo b >"$1/dir/b"
  echo c >"$1/c"
  echo u >"$1/u"
  git -C "$1" add a dir/b c u
  git -C "$1" commit -q -m base
  git -C "$1" checkout -q -b topic
  printf '1\n2\n3\ntopic\n' >"$1/a"
  echo t >"$1/t"
  git -C "$1" rm -q c
  git -C "$1" add a t
  git -C "$1" commit -q -m topic
  git -C "$1" checkout -q main
  echo main >"$1/dir/b"
  echo m >"$1/m"
  git -C "$1" add dir/b m
  git -C "$1" commit -q -m main
}

# Create the repository `$1` like `diverged`, in which `main` also adds another line to `a` and changes `c`, which
# `topic` removes, so that both conflict.
function conflicting() {
  diverged "$1"
  printf '1\n2\n3\nmain\n' >"$1/a"
  echo "c on main" >"$1/c"
  git -C "$1" commit -q -am "main conflicts"
}

diverged clean
twin clean
git -C clean-git merge -q --no-commit --no-ff topic

conflicting conflict
twin conflict
fails git -C conflict-git merge -q --no-commit topic

diverged kept
echo "local change" >kept/dir/b
echo untracked >kept/untracked
twin kept
git -C kept-git merge -q --no-commit --no-ff topic

diverged refused-change
echo "local change" >refused-change/a
twin refused-change
fails git -C refused-change-git merge -q --no-commit --no-ff topic 2>refused-change-git.err

diverged refused-untracked
echo untracked >refused-untracked/t
twin refused-untracked
fails git -C refused-untracked-git merge -q --no-commit --no-ff topic 2>refused-untracked-git.err
