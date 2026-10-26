#!/usr/bin/env bash
set -eu -o pipefail

# The cases with symbolic links, which aren't archived as archives can't represent them on all hosts, and which need a
# host that can create them. Each case is set up twice by the same function, as `<case>` and as `<case>-git`, and
# `git switch` switches `<case>-git` to `new` as in `make_switch.sh`. A copy of `<case>` would differ from its index
# in the stat information of its files, which git doesn't refresh for the files behind symbolic links.

# commit <message> commits all changes of the worktree.
commit() {
  git add --all
  git commit -q -m "$1"
}

# record <case> <setup> sets up the case twice with the function <setup>, and switches the second one.
record() {
  (git init -q "$1"
    cd "$1"
    "$2"
    git rev-parse 'HEAD^{tree}' > .git/old-tree
    git rev-parse 'new^{tree}' > .git/new-tree)
  (git init -q "$1-git"
    cd "$1-git"
    "$2"
    if (LC_ALL=C git -c advice.commitBeforeMerge=false switch -q new) > .git/error 2>&1; then
      rm .git/error
    fi)
}

# Symbolic links are switched like files: a link whose target changes, a file and a directory that become links, a link
# that becomes a file, and links that are added and removed.
set_up_symlinks() {
  mkdir d d2
  echo f > f
  echo x > d/x
  echo y > d2/y
  echo p > p
  ln -s f l
  ln -s f q
  ln -s f r
  commit old
  git switch -q -c new
  rm -r l p q r d2
  ln -s g l
  ln -s f p
  echo q > q
  ln -s d n
  ln -s d d2
  commit new
  git switch -q main
}
record symlinks set_up_symlinks

# A symbolic link replaced the directory of a file that the new tree changes, and leads to a directory with that file.
set_up_link_in_path() {
  mkdir d
  echo x > d/x
  commit old
  git switch -q -c new
  echo x2 > d/x
  commit new
  git switch -q main
  mv d e
  ln -s e d
}
record link-in-path set_up_link_in_path
