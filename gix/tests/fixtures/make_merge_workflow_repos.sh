#!/usr/bin/env bash
set -eu -o pipefail

# Each scenario is a repository `<name>` as it is before `git merge`, in which the tests merge with `gix`, and its copy
# `<name>-git`, in which `git merge` merged, stopped or refused to merge, with its standard error in `<name>-git.err`
# when it refused.

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
# that `git merge` and `git commit` set for them, and the message they receive. A hook fails if `.git/fail-<hook>` exists.
function record_hooks() {
  local hook
  mkdir -p "$1/.git/hooks"
  for hook in pre-commit pre-merge-commit prepare-commit-msg commit-msg post-commit post-merge; do
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
  echo "GIT_REFLOG_ACTION: ${GIT_REFLOG_ACTION-unset}"
  env | grep '^GITHEAD_' | sort
  case "$hook" in
    prepare-commit-msg|commit-msg)
      echo "message:"
      cat "$1"
      echo "end of message"
      ;;
  esac
} >>.git/hook-records
if test -e ".git/fail-$hook"; then
  exit 1
fi
EOF
    chmod +x "$1/.git/hooks/$hook"
  done
}

# Install the recording hooks into the repository `$1` and copy it for Git.
function prepared() {
  record_hooks "$1"
  twin "$1"
}

# Create the repository `$1` whose `main` has the commit `base` of `a` and `dir/b`.
function base() {
  git init -q "$1"
  printf '1\n2\n3\n' >"$1/a"
  mkdir "$1/dir"
  echo b >"$1/dir/b"
  git -C "$1" add a dir/b
  git -C "$1" commit -q -m base
}

# Create the repository `$1` like `base`, on which `topic` adds a line to `a` and adds `topic-file`, so that `main` can
# fast-forward to `topic`.
function ahead() {
  base "$1"
  git -C "$1" checkout -q -b topic
  printf '1\n2\n3\ntopic\n' >"$1/a"
  echo topic >"$1/topic-file"
  git -C "$1" add a topic-file
  git -C "$1" commit -q -m topic
  git -C "$1" checkout -q main
}

# Create the repository `$1` like `ahead`, in which `main` changes `dir/b` and adds `main-file` as well.
function diverged() {
  ahead "$1"
  echo main >"$1/dir/b"
  echo main >"$1/main-file"
  git -C "$1" add dir/b main-file
  git -C "$1" commit -q -m main
}

# Create the repository `$1` like `diverged`, in which `main` also adds another line to `a`, which conflicts.
function conflicting() {
  diverged "$1"
  printf '1\n2\n3\nmain\n' >"$1/a"
  git -C "$1" commit -q -am "main conflicts"
}

# Create the repository `$1` like `base`, with the branch `other` of an unrelated history that adds `other-file`.
function unrelated() {
  base "$1"
  git -C "$1" checkout -q --orphan other
  git -C "$1" rm -rfq .
  echo other >"$1/other-file"
  git -C "$1" add other-file
  git -C "$1" commit -q -m other
  git -C "$1" checkout -q main
}

# The names of the scenarios are short, as Git for Windows fails to open objects whose paths are too long, which those
# of SHA-256 in deep directories are.

ahead fast-forward
prepared fast-forward
git -C fast-forward-git merge -q topic

ahead ff-detached
git -C ff-detached checkout -q --detach main
prepared ff-detached
git -C ff-detached-git merge -q topic

ahead ff-refused
echo untracked >ff-refused/topic-file
prepared ff-refused
fails git -C ff-refused-git merge -q topic 2>ff-refused-git.err

ahead no-ff
prepared no-ff
git -C no-ff-git merge -q --no-ff topic

ahead ff-config
git -C ff-config config merge.ff false
prepared ff-config
git -C ff-config-git merge -q topic

diverged ff-only
prepared ff-only
fails git -C ff-only-git merge -q --ff-only topic 2>ff-only-git.err

diverged merge
prepared merge
git -C merge-git merge -q topic

diverged into-feature
git -C into-feature checkout -q -b feature
prepared into-feature
git -C into-feature-git merge -q topic

diverged merge-detached
git -C merge-detached checkout -q --detach main
prepared merge-detached
git -C merge-detached-git merge -q topic

diverged merge-commit-id
prepared merge-commit-id
git -C merge-commit-id-git merge -q "$(git -C merge-commit-id rev-parse topic)"

diverged merge-tag
git -C merge-tag tag -a -m "version one" v1 topic
prepared merge-tag
git -C merge-tag-git merge -q v1

diverged suppress-dest
git -C suppress-dest config merge.suppressDest ""
prepared suppress-dest
git -C suppress-dest-git merge -q topic

diverged message
prepared message
git -C message-git merge -q -m "merged with a message" topic

diverged no-commit
prepared no-commit
git -C no-commit-git merge -q --no-commit topic

ahead no-commit-no-ff
prepared no-commit-no-ff
git -C no-commit-no-ff-git merge -q --no-commit --no-ff topic

diverged squash
prepared squash
git -C squash-git merge -q --squash topic

# `topic` merges `side` and goes on, so that the message of the squash lists a merge.
ahead squash-ff
git -C squash-ff checkout -q -b side
echo side >squash-ff/side-file
git -C squash-ff add side-file
git -C squash-ff commit -q -m side
git -C squash-ff checkout -q topic
git -C squash-ff merge -q --no-ff -m "merge side" side
echo more >squash-ff/topic-file
git -C squash-ff commit -q -am "more topic"
git -C squash-ff checkout -q main
prepared squash-ff
git -C squash-ff-git merge -q --squash topic

conflicting conflict
prepared conflict
fails git -C conflict-git merge -q topic

conflicting continue
fails git -C continue merge -q topic
echo resolved >continue/a
git -C continue add a
git -C continue config commit.status false
prepared continue
GIT_EDITOR=: git -C continue-git merge --continue

conflicting abort
fails git -C abort merge -q topic
prepared abort
# `git reset --merge` doesn't refresh the index, whose stat information the copy outdated.
git -C abort-git status >/dev/null
git -C abort-git merge --abort

conflicting quit
fails git -C quit merge -q topic
prepared quit
git -C quit-git merge --quit

diverged local-changes
echo "local change" >local-changes/a
prepared local-changes
fails git -C local-changes-git merge -q topic 2>local-changes-git.err

diverged staged-changes
echo staged >staged-changes/main-file
git -C staged-changes add main-file
prepared staged-changes
fails git -C staged-changes-git merge -q topic 2>staged-changes-git.err

diverged untracked
echo untracked >untracked/topic-file
prepared untracked
fails git -C untracked-git merge -q topic 2>untracked-git.err

base up-to-date
git -C up-to-date branch topic
echo main >up-to-date/main-file
git -C up-to-date add main-file
git -C up-to-date commit -q -m main
prepared up-to-date
git -C up-to-date-git merge -q topic

unrelated unrelated
prepared unrelated
fails git -C unrelated-git merge -q other 2>unrelated-git.err

unrelated allow-unrelated
prepared allow-unrelated
git -C allow-unrelated-git merge -q --allow-unrelated-histories other

diverged hook-fails
record_hooks hook-fails
touch hook-fails/.git/fail-pre-merge-commit
twin hook-fails
fails git -C hook-fails-git merge -q topic 2>hook-fails-git.err

diverged no-verify
record_hooks no-verify
touch no-verify/.git/fail-pre-merge-commit no-verify/.git/fail-commit-msg
twin no-verify
git -C no-verify-git merge -q --no-verify topic

# `main` is unborn, with an empty index, while `topic` has a commit.
git init -q unborn
echo a >unborn/a
git -C unborn add a
git -C unborn commit -q -m topic
git -C unborn branch -m topic
git -C unborn checkout -q --orphan main
git -C unborn rm -rfq .
prepared unborn
git -C unborn-git merge -q topic
