#!/usr/bin/env bash
set -eu -o pipefail

gix_testtools_require_symlinks

# Make the repository `$1` with a detached `HEAD` and the branches `a`, `b`, `c` and `main` packed into a `packed-refs`
# that is a link to `$1-target/packed-refs`.
function make_repo() {
  git init -q -b main "$1"
  (
    cd "$1"
    git commit -q --allow-empty -m one
    git checkout -q --detach
    git branch a
    git branch b
    git branch c
    git pack-refs --all
  )
  mkdir "$1-target"
  mv "$1/.git/packed-refs" "$1-target/packed-refs"
  ln -s ../../"$1-target"/packed-refs "$1/.git/packed-refs"
}

# Delete the branch `$2` in the `git` repository and record in `git.locks` whether Git did, named `$1`.
function delete_with_lock() {
  if git -C git update-ref -d "refs/heads/$2" 2>/dev/null; then
    echo "$1 deleted"
  else
    echo "$1 refused"
  fi >>git.locks
}

make_repo gix
make_repo git

# Git removes a packed branch from the link's target and keeps the link.
git -C git update-ref -d refs/heads/b
cp git-target/packed-refs git-without-b

# Git locks the link's target: a lock beside the target holds back a deletion, a lock beside the link doesn't.
touch git-target/packed-refs.lock
delete_with_lock beside-target c
rm git-target/packed-refs.lock
touch git/.git/packed-refs.lock
delete_with_lock beside-link c
rm git/.git/packed-refs.lock
cp git-target/packed-refs git-without-c

# Deleting the last packed branches leaves only the header line in the link's target.
git -C git update-ref -d refs/heads/a
git -C git update-ref -d refs/heads/main
