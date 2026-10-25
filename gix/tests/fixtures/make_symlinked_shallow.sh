#!/usr/bin/env bash
set -eu -o pipefail

gix_testtools_require_symlinks

git init -q -b main remote
(
  cd remote
  for n in 1 2 3 4; do
    echo "$n" >file
    git add file
    git commit -q -m "c$n"
  done
)

# The clones' remote URL holds a path that gix can open as well, which Git for Windows' shell gives with `cygpath`.
remote_path=$PWD/remote
if command -v cygpath >/dev/null 2>&1; then
  remote_path=$(cygpath -am -- "$remote_path")
fi

# Clone `remote` one commit deep into `$1`, with a `shallow` file that is a link to `$1-target/shallow`.
function make_clone() {
  git clone -q --depth 1 "file://$remote_path" "$1"
  mkdir "$1-target"
  mv "$1/.git/shallow" "$1-target/shallow"
  ln -s ../../"$1-target"/shallow "$1/.git/shallow"
}

# Deepen the `git` clone to `$2` commits and record in `git.locks` whether Git did, named `$1`.
function deepen_with_lock() {
  if git -C git fetch -q --depth "$2" 2>/dev/null; then
    echo "$1 deepened"
  else
    echo "$1 refused"
  fi >>git.locks
}

make_clone gix
make_clone git

# Git writes the new shallow boundary to the link's target and keeps the link.
git -C git fetch -q --depth 2
cp git-target/shallow git-depth-2

# Git locks the link's target: a lock beside the target holds back the fetch, a lock beside the link doesn't.
touch git-target/shallow.lock
deepen_with_lock beside-target 3
rm git-target/shallow.lock
touch git/.git/shallow.lock
deepen_with_lock beside-link 3
rm git/.git/shallow.lock
cp git-target/shallow git-depth-3

# Unshallowing removes the link itself and leaves its target as it was.
git -C git fetch -q --unshallow
