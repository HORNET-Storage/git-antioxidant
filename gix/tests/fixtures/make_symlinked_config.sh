#!/usr/bin/env bash
set -eu -o pipefail

gix_testtools_require_symlinks

# Create a repository at `$1` with branches a, b and c and a section for each in its configuration. Its `.git/config`
# is a link to `$1-target/config`, which only its owner may read and write.
function make_repo() {
  git init -q -b main "$1"
  (
    cd "$1"
    git commit -q --allow-empty -m c1
    for branch in a b c; do
      git branch "$branch"
      git config "branch.$branch.remote" origin
    done
  )
  mkdir "$1-target"
  mv "$1/.git/config" "$1-target/config"
  chmod 600 "$1-target/config"
  ln -s ../../"$1-target"/config "$1/.git/config"
}

# Delete branch `$2` in the `git` repository and record in `git.locks`, named `$1`, whether its section is still in the
# configuration. Git deletes the branch even if it can't update the configuration.
function delete_with_lock() {
  git -C git branch -q -D "$2" 2>/dev/null || true
  if git -C git config --get "branch.$2.remote" >/dev/null; then
    echo "$1 kept"
  else
    echo "$1 removed"
  fi >>git.locks
}

make_repo gix
make_repo git

# Git removes the section of a from the link's target and keeps the link and the target's mode.
git -C git branch -q -D a
cp git-target/config git-without-a

# Git locks the link's target: a lock beside the target holds back the update, a lock beside the link doesn't.
touch git-target/config.lock
delete_with_lock beside-target b
rm git-target/config.lock
touch git/.git/config.lock
delete_with_lock beside-link c
rm git/.git/config.lock
