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

# Make the repository `$1`, whose branches `main` and `other` are also reached through links: the text of
# `text-main` names `refs/heads/main`, while `path-main` and `path-other` lead to the files of the branches.
function make_repo() {
  git init -q -b main "$1"
  (
    cd "$1"
    git commit -q --allow-empty -m one
    git commit -q --allow-empty -m two
    git branch other HEAD~1
    cd .git/refs/heads
    link_to_nothing refs/heads/main text-main
    ln -s main path-main
    ln -s ../../refs/heads/other path-other
  )
}

make_repo gix
make_repo git
one=$(git -C git rev-parse other)
two=$(git -C git rev-parse main)
# Git updates the branch that a link names, replaces a link that it reads through, and deletes only the link.
git -C git update-ref refs/heads/text-main "$one"
git -C git update-ref refs/heads/path-main "$two"
git -C git update-ref -d refs/heads/path-other
