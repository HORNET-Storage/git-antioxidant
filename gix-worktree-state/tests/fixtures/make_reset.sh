#!/usr/bin/env bash
set -eu -o pipefail

# Each case is a repository whose index and worktree differ from the checked-out commit of `main` as its comment says,
# and whose branch `new` has the tree to reset to. `record` copies the case to `<case>-<mode>` for each mode that it is
# given and resets the copy to `new` with `git reset --<mode>`, and `record_paths` copies it to `<case>-paths` and resets
# the entries of the paths that it is given with `git reset new -- <path>...`. That way `<case>` remains as it was, and
# each copy holds the index and the worktree that git left. If git fails, its output goes to `.git/error` of the copy, in
# the C locale. As `git reset` stops at the first path that it refuses and only names that one, each mode of a case
# refuses at most one path. The trees of `main` and `new` go to `.git/old-tree` and `.git/new-tree`, the paths of
# `record_paths` go to `.git/pathspec`, one per line, and the directory in which git ran goes to `.git/cwd` if `cwd`
# names it.
#
# `record` refreshes the index of each copy before git resets it. The copy changes the stat information of the files,
# which `git reset --merge` and `git reset --keep` take for changes of the files otherwise, as they don't refresh the
# index themselves.
#
# The cases only use regular files and directories, so that their results are the same on all hosts. Those whose names
# start with `dev-` are where this crate deviates from Git, and aren't reset by git.

# start_case <name> creates the repository of a case and enters it.
start_case() {
  git init -q "$1"
  cd "$1"
}

# commit <message> commits all changes of the worktree.
commit() {
  git add --all
  git commit -q -m "$1"
}

# write <file>... writes the path of each file into it.
write() {
  local file
  for file in "$@"; do
    echo "$file" > "$file"
  done
}

# record <mode>... resets copies of the case to `new` in each mode as described above, from the directory `$cwd` of the
# worktree if it is set, which is the current working directory that git protects.
record() {
  local name mode copy
  name=$(basename "$PWD")
  git rev-parse 'HEAD^{tree}' > .git/old-tree
  git rev-parse 'new^{tree}' > .git/new-tree
  if test -n "${cwd:-}"; then
    echo "$cwd" > .git/cwd
  fi
  for mode in "$@"; do
    copy="../$name-$mode"
    cp -R . "$copy"
    # This fails if files changed, after it refreshed the index nonetheless.
    (cd "$copy" && git update-index -q --unmerged --refresh) || :
    if (cd "$copy/${cwd:-.}" && LC_ALL=C git reset -q "--$mode" new) > "$copy/.git/error" 2>&1; then
      rm "$copy/.git/error"
    fi
  done
}

# record_paths <path>... resets the entries of the paths in a copy of the case to those of `new`, as described above.
record_paths() {
  local copy
  copy="../$(basename "$PWD")-paths"
  git rev-parse 'new^{tree}' > .git/new-tree
  printf '%s\n' "$@" > .git/pathspec
  cp -R . "$copy"
  if (cd "$copy" && LC_ALL=C git reset -q new -- "$@") > "$copy/.git/error" 2>&1; then
    rm "$copy/.git/error"
  fi
}

# row <worktree> <index> <head> <new> makes the file `f` hold <head> in `HEAD`, <new> in `new`, <index> in the index and
# <worktree> in the worktree, as a row of the tables in the discussion of `git reset --help` names them.
row() {
  echo "$3" > f
  commit head
  git branch new
  if test "$4" != "$3"; then
    git switch -q new
    echo "$4" > f
    commit new
    git switch -q main
  fi
  echo "$2" > f
  git add f
  echo "$1" > f
}

# conflict <new> stops a merge that conflicts in the file `f`, which holds `A` in `HEAD` and the conflict markers in the
# worktree, and makes `new` a branch whose file holds <new>. The state of the merge is removed, so that only the stages
# of the index tell of the conflict.
conflict() {
  echo base > f
  commit base
  git switch -q -c theirs
  echo theirs > f
  commit theirs
  git switch -q main
  echo A > f
  commit ours
  git branch new
  if test "$1" != A; then
    git switch -q new
    echo "$1" > f
    commit new
    git switch -q main
  fi
  git merge -q theirs > /dev/null 2>&1 || :
  rm -f .git/MERGE_HEAD .git/MERGE_MSG .git/MERGE_MODE .git/AUTO_MERGE
}

# Every kind of change in a clean worktree: changed, added and removed files, a new directory, one that the reset leaves
# empty, and files and directories that replace each other.
(start_case clean
  mkdir -p d gone to-file
  write a b d/c d/e gone/x to-dir to-file/f
  commit old
  git switch -q -c new
  echo a2 > a
  git rm -q d/e gone/x to-dir to-file/f
  mkdir new-dir to-dir
  write d/g new-dir/n to-dir/y to-file
  commit new
  git switch -q main
  record hard mixed merge keep
)

# The rows of the tables in the discussion of `git reset --help` for a file without conflicts, whose contents in the
# worktree, the index, `HEAD` and `new` the names of the cases spell.
(start_case abcd
  row A B C D
  record hard mixed merge keep
)

(start_case abcc
  row A B C C
  record hard mixed merge keep
)

(start_case bbcd
  row B B C D
  record hard mixed merge keep
)

(start_case bbcc
  row B B C C
  record hard mixed merge keep
)

(start_case bccd
  row B C C D
  record hard mixed merge keep
)

(start_case bccc
  row B C C C
  record hard mixed merge keep
)

# The rows of the tables in the discussion of `git reset --help` for a file with a conflict, whose file has the conflict
# markers, and whose contents in `HEAD` and `new` the names of the cases spell.
(start_case xuab
  conflict B
  record hard mixed merge keep
)

(start_case xuaa
  conflict A
  record hard mixed merge keep
)

# A conflict whose file was replaced by a directory with an untracked file.
(start_case xu-dir
  conflict B
  rm f
  mkdir f
  echo untracked > f/untracked
  record hard mixed merge keep
)

# An untracked file at the path of a file that only `new` has.
(start_case untracked
  write a
  commit old
  git switch -q -c new
  write added
  commit new
  git switch -q main
  echo untracked > added
  record hard mixed merge keep
)

# An untracked file at the path of a directory of `new`.
(start_case way
  write a
  commit old
  git switch -q -c new
  mkdir d
  write d/x
  commit new
  git switch -q main
  echo untracked > d
  record hard mixed merge keep
)

# An untracked file at the path of a file that only `HEAD` has, whose entry was removed from the index.
(start_case removed
  write a removed
  commit old
  git switch -q -c new
  git rm -q removed
  commit new
  git switch -q main
  git rm -q --cached removed
  record hard mixed merge keep
)

# A directory with an untracked file at the path of a file of `new`.
(start_case dirs
  mkdir d
  write a d/x
  commit old
  git switch -q -c new
  git rm -q d/x
  echo d > d
  commit new
  git switch -q main
  echo untracked > d/untracked
  record hard mixed merge keep
)

# Ignored files in the way of files of `new` are overwritten: a file at the path of a file, and a directory with only
# ignored files at the path of a file.
(start_case ignored
  printf '%s\n' ignored '*.log' > .gitignore
  commit old
  git switch -q -c new
  echo tracked > ignored
  echo logs > logs
  git add -f ignored
  git add logs
  git commit -q -m new
  git switch -q main
  echo untracked > ignored
  mkdir logs
  echo log > logs/a.log
  record hard mixed merge keep
)

# Files that are gone: one that `new` changes, one that it removes, one that it keeps, and the directory of a file that
# it changes.
(start_case gone
  mkdir d
  write a b c d/x
  commit old
  git switch -q -c new
  echo a2 > a
  echo x2 > d/x
  git rm -q b
  commit new
  git switch -q main
  rm -r a b c d
  record hard mixed merge keep
)

# Local changes at paths that `new` doesn't change: a changed file, a file that is gone, and a file replaced by a
# directory with an untracked file in it.
(start_case restore
  write a g m t
  commit old
  git branch new
  echo m2 > m
  rm g t
  mkdir t
  echo untracked > t/untracked
  record hard mixed merge keep
)

# Staged changes at paths that `new` doesn't change: a change, a new file and a deletion.
(start_case staged
  write a changed deleted
  commit old
  git switch -q -c new
  echo a2 > a
  commit new
  git switch -q main
  echo changed2 > changed
  echo added > added
  git add changed added
  git rm -q deleted
  record hard mixed merge keep
)

# The current working directory is in the way of a file of `new`.
(start_case cwd
  mkdir d
  write a d/x
  commit old
  git switch -q -c new
  git rm -q d/x
  echo d > d
  commit new
  git switch -q main
  cwd=d record hard mixed merge keep
)

# The entries of some paths are reset: a changed file, a directory with a changed, a removed and an added file, and the
# file of a directory of `new` whose entry replaced the directory. The entries of the other paths stay, and so does the
# worktree.
(start_case paths
  mkdir d p
  write a b d/x d/y p/q
  commit old
  git switch -q -c new
  echo a2 > a
  echo b2 > b
  echo x2 > d/x
  git rm -q d/y
  write d/z
  commit new
  git switch -q main
  echo a3 > a
  echo b3 > b
  git add a b
  git rm -q -r p
  echo p > p
  git add p
  record_paths a d p/q
)

# The current working directory is in a directory that a file of `new` replaces. Git removes it, or fails to remove it
# on a host that can't remove the current directory of a process.
(start_case dev-cwd
  mkdir -p d/sub
  write a d/sub/f
  commit old
  git switch -q -c new
  git rm -q -r d
  echo d > d
  commit new
  git switch -q main
  cwd=d/sub record
)
