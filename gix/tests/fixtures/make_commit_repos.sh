#!/usr/bin/env bash
set -eu -o pipefail

# Each scenario is a repository `<name>` as it is before a commit, in which the tests commit with `gix`, and its copy
# `<name>-git`, in which `git commit` made the same commit or refused to make it.

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

# Install hooks into the repository `$1` that append how they ran to `.git/hook-records`: their arguments, the variables
# that Git sets for them, and the message or input they receive. `commit-msg` adds a trailer to the message if
# `.git/extend-message` exists, and a hook fails if `.git/fail-<hook>` exists.
function record_hooks() {
  local hook
  mkdir -p "$1/.git/hooks"
  for hook in pre-commit prepare-commit-msg commit-msg post-commit post-rewrite; do
    cat >"$1/.git/hooks/$hook" <<'EOF'
#!/bin/sh
hook=${0##*/}
{
  echo "hook: $hook"
  for arg in "$@"; do
    echo "argument: $arg"
  done
  echo "GIT_INDEX_FILE: ${GIT_INDEX_FILE-unset}"
  echo "GIT_EDITOR: ${GIT_EDITOR-unset}"
  echo "GIT_AUTHOR_NAME: ${GIT_AUTHOR_NAME-unset}"
  echo "GIT_AUTHOR_EMAIL: ${GIT_AUTHOR_EMAIL-unset}"
  echo "GIT_AUTHOR_DATE: ${GIT_AUTHOR_DATE-unset}"
  case "$hook" in
    prepare-commit-msg|commit-msg)
      echo "message:"
      cat "$1"
      echo "end of message"
      ;;
    post-rewrite)
      echo "input:"
      cat
      echo "end of input"
      ;;
  esac
} >>.git/hook-records
if test "$hook" = commit-msg && test -e .git/extend-message; then
  printf '\nHook-Trailer: commit-msg\n' >>"$1"
fi
if test -e ".git/fail-$hook"; then
  exit 1
fi
EOF
    chmod +x "$1/.git/hooks/$hook"
  done
}

# Create the repository `$1` whose `main` has the commit `base` with the files `a` and `dir/b`.
function base() {
  git init -q "$1"
  echo a >"$1/a"
  mkdir "$1/dir"
  echo b >"$1/dir/b"
  git -C "$1" add a dir/b
  git -C "$1" commit -q -m base
}

# Create the repository `$1` like `base`, with the change of `a` to `$1` in its index and the recording hooks.
function hooked() {
  base "$1"
  echo "$1" >"$1/a"
  git -C "$1" add a
  record_hooks "$1"
}

# Create the repository `$1` like `base`, in which `topic` adds `topic-file` and `main` adds `main-file`.
function diverged() {
  base "$1"
  git -C "$1" checkout -q -b topic
  echo topic >"$1/topic-file"
  git -C "$1" add topic-file
  git -C "$1" commit -q -m topic
  git -C "$1" checkout -q main
  echo main >"$1/main-file"
  git -C "$1" add main-file
  git -C "$1" commit -q -m main
}

# Create the repository `$1` like `base`, in which `topic` and `main` change `a` differently.
function conflicting() {
  base "$1"
  git -C "$1" checkout -q -b topic
  echo topic >"$1/a"
  git -C "$1" commit -q -am topic
  git -C "$1" checkout -q main
  echo main >"$1/a"
  git -C "$1" commit -q -am main
}

# Create the repository `$1` like `base`, whose second commit by another author, with a message of two paragraphs, is
# amended with the change of `a` in the index, and the recording hooks.
function amendable() {
  base "$1"
  echo to-be-amended >"$1/a"
  git -C "$1" add a
  GIT_AUTHOR_NAME=other GIT_AUTHOR_EMAIL=other@example.com GIT_AUTHOR_DATE="2001-01-01 00:00:00 +0100" \
    git -C "$1" commit -q -m "to be amended" -m "with a body"
  echo amended >"$1/a"
  git -C "$1" add a
  record_hooks "$1"
}

# The message of the scenarios of cleanups: blank lines around it, whitespace at the end of lines, comment lines of
# several comment characters and a scissors line.
printf '\n\nsubject   \n\n   \nbody\t\n# hash comment\n; semicolon comment\n// slash comment\n\n\n# ------------------------ >8 ------------------------\nbelow the scissors\n' >message

git init -q initial
echo a >initial/a
mkdir initial/dir
echo b >initial/dir/b
git -C initial add a dir/b
record_hooks initial
twin initial
git -C initial-git commit -q -m initial

base subsequent
echo changed >subsequent/a
echo new >subsequent/new
git -C subsequent add a new
record_hooks subsequent
twin subsequent
git -C subsequent-git commit -q -m "subsequent

with a body"

base detached
git -C detached checkout -q --detach
echo detached >detached/a
git -C detached add a
twin detached
git -C detached-git commit -q -m detached

base reflog-action
echo changed >reflog-action/a
git -C reflog-action add a
twin reflog-action
GIT_REFLOG_ACTION=custom git -C reflog-action-git commit -q -m "custom action"

for mode in default strip whitespace verbatim scissors; do
  base "cleanup-$mode"
  echo "$mode" >"cleanup-$mode/a"
  git -C "cleanup-$mode" add a
  twin "cleanup-$mode"
  git -C "cleanup-$mode-git" commit -q --cleanup="$mode" -F ../message
done

# Create the repository `$1` like `base` with a change of `a` in its index, `commit.cleanup=strip` and the
# configuration that follows, and commit the message of the cleanups in `$1-git`.
function commented() {
  local name=$1
  shift
  base "$name"
  git -C "$name" config commit.cleanup strip
  while test $# -gt 0; do
    git -C "$name" config "$1" "$2"
    shift 2
  done
  echo "$name" >"$name/a"
  git -C "$name" add a
  twin "$name"
  git -C "$name-git" commit -q -F ../message
}

commented cleanup-config
commented comment-char core.commentChar ';'
commented comment-string core.commentString '//'
commented comment-auto core.commentChar auto
commented comment-last core.commentString '//' core.commentChar ';'

amendable amend
twin amend
git -C amend-git commit -q --amend --no-edit

amendable amend-message
twin amend-message
git -C amend-message-git commit -q --amend -m "amended message"

diverged merge
git -C merge merge -q --no-commit --no-ff topic
record_hooks merge
twin merge
git -C merge-git commit -q --no-edit

conflicting merge-resolved
fails git -C merge-resolved merge -q --no-commit topic
echo resolved >merge-resolved/a
git -C merge-resolved add a
record_hooks merge-resolved
twin merge-resolved
git -C merge-resolved-git commit -q --no-edit

diverged squash
git -C squash merge -q --squash topic
record_hooks squash
twin squash
git -C squash-git commit -q --no-edit

for hook in pre-commit prepare-commit-msg commit-msg post-commit; do
  hooked "$hook-fails"
  touch "$hook-fails/.git/fail-$hook"
  twin "$hook-fails"
done
fails git -C pre-commit-fails-git commit -q -m "pre-commit fails"
fails git -C prepare-commit-msg-fails-git commit -q -m "prepare-commit-msg fails"
fails git -C commit-msg-fails-git commit -q -m "commit-msg fails"
git -C post-commit-fails-git commit -q -m "post-commit fails"

hooked no-verify
touch no-verify/.git/fail-pre-commit no-verify/.git/fail-commit-msg
twin no-verify
git -C no-verify-git commit -q --no-verify -m "no verify"

hooked hook-edits-message
touch hook-edits-message/.git/extend-message
twin hook-edits-message
git -C hook-edits-message-git commit -q -m "edited by commit-msg"

base nothing-to-commit
record_hooks nothing-to-commit
twin nothing-to-commit
fails git -C nothing-to-commit-git commit -q -m nothing

base allow-empty
twin allow-empty
git -C allow-empty-git commit -q --allow-empty -m empty

hooked empty-message
twin empty-message
fails git -C empty-message-git commit -q -m ""

base allow-empty-message
echo changed >allow-empty-message/a
git -C allow-empty-message add a
twin allow-empty-message
git -C allow-empty-message-git commit -q --allow-empty-message -m ""

conflicting unmerged
fails git -C unmerged merge -q --no-commit topic
record_hooks unmerged
twin unmerged
fails git -C unmerged-git commit -q --no-edit

git init -q nothing-to-amend
echo a >nothing-to-amend/a
git -C nothing-to-amend add a
twin nothing-to-amend
fails git -C nothing-to-amend-git commit -q --amend -m amended

diverged amend-in-merge
git -C amend-in-merge merge -q --no-commit --no-ff topic
twin amend-in-merge
fails git -C amend-in-merge-git commit -q --amend --no-edit
