#!/usr/bin/env bash
set -eu -o pipefail

gix_testtools_require_symlinks

# Make a link at `$2` with the text `$1` that leads nowhere. Native Windows links are made for an existing target,
# so a file stands in for the target while the link is created.
function link_to_nothing() {
  local target
  target=$(dirname "$2")/$1
  mkdir -p "$(dirname "$target")"
  touch "$target"
  ln -s "$1" "$2"
  rm "$target"
}

# Make the repository `$1` whose HEAD is a link with the text `$2`, and record in `$1.git-verdict` whether Git
# accepts its `.git` directory as a repository.
function head_link_repo() {
  git init -q -b main "$1"
  git -C "$1" commit -q --allow-empty -m one
  git -C "$1" branch other
  rm "$1/.git/HEAD"
  if test -e "$1/.git/$2"; then
    ln -s "$2" "$1/.git/HEAD"
  else
    link_to_nothing "$2" "$1/.git/HEAD"
  fi
  if git --git-dir="$1/.git" rev-parse --git-dir >/dev/null 2>&1; then
    echo repository
  else
    echo "not a repository"
  fi >"$1.git-verdict"
}

head_link_repo branch refs/heads/main
# Git accepts a link whose text starts with refs/ even if it leads nowhere, which is how it names an unborn branch.
head_link_repo unborn refs/heads/unborn
# This text isn't a valid reference name, so Git reads HEAD through the link and finds a detached HEAD.
head_link_repo detached refs/heads/../heads/other
