#!/usr/bin/env bash
set -eu -o pipefail

gix_testtools_require_symlinks

# Make the repository `$1` with the committed files `a` and `b` and an index that is a link to `$1-target/index`.
function make_repo() {
  git init -q "$1"
  (
    cd "$1"
    echo a >a
    echo b >b
    git add a b
    git commit -q -m one
  )
  mkdir "$1-target"
  mv "$1/.git/index" "$1-target/index"
  ln -s ../../"$1-target"/index "$1/.git/index"
}

# Remove `$2` from the index of the `git` repository and record in `git.locks` whether Git did, named `$1`.
function remove_with_lock() {
  if git -C git rm -q --cached "$2" 2>/dev/null; then
    echo "$1 removed"
  else
    echo "$1 refused"
  fi >>git.locks
}

make_repo gix
make_repo git

# Git writes the link's target and keeps the link.
git -C git rm -q --cached b
cp git-target/index git-index-without-b

# Git locks the link's target: a lock beside the target holds back the change, a lock beside the link doesn't.
touch git-target/index.lock
remove_with_lock beside-target a
rm git-target/index.lock
touch git/.git/index.lock
remove_with_lock beside-link a
rm git/.git/index.lock
