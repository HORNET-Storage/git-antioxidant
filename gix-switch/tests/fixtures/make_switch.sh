#!/usr/bin/env bash
set -eu -o pipefail

# Each case is a repository whose index and worktree differ from the checked-out commit of `main` as its comment says,
# and whose branch `new` has the tree to switch to. `record` copies the case to `<case>-git` and switches the copy to
# `new` with `git switch`, which refreshes the index and then merges the two trees into the index and the worktree with
# Git's two-way merge. That way `<case>` remains as it was, and `<case>-git` holds the index and the worktree that git
# left. If git fails, its output goes to `.git/error` of the copy, in the C locale and without advice, so that its
# messages only list the refused paths. The trees of `main` and `new` go to `.git/old-tree` and `.git/new-tree`, and
# the directory in which git ran goes to `.git/cwd` if it isn't the worktree.
#
# The cases on a branch without commits run `git read-tree -m -u` from the empty tree to `new` instead, as `git merge`
# does on such a branch. They have no `.git/old-tree`, and as `git read-tree` stops at the first path that it refuses,
# each of them refuses at most one.
#
# The cases only use regular files, directories, repositories and submodules, so that their results are the same on all
# hosts. Those whose names start with `dev-` are where this crate deviates from Git, whose results also depend on the
# host for some of them. Case names are short so that the SHA-256 object paths of the repositories nested in a case
# stay within Windows' traditional path limit, which Git for Windows keeps by default.

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

# nested_repository <path> creates a repository at <path> with one commit.
nested_repository() {
  mkdir -p "$1"
  (cd "$1" && git init -q && echo inner > inner && git add inner && git commit -q -m inner)
}

# record [<directory>] switches a copy of the case to `new` as described above, from <directory> of its worktree if
# given, which is the current working directory that git protects.
record() {
  local copy
  copy="../$(basename "$PWD")-git"
  git rev-parse 'HEAD^{tree}' > .git/old-tree
  git rev-parse 'new^{tree}' > .git/new-tree
  if test -n "${1:-}"; then
    echo "$1" > .git/cwd
  fi
  cp -R . "$copy"
  if (cd "$copy/${1:-.}" && LC_ALL=C git -c advice.commitBeforeMerge=false switch -q new) > "$copy/.git/error" 2>&1; then
    rm "$copy/.git/error"
  fi
}

# record_unborn switches a copy of the case from the empty tree to `new` with `git read-tree -m -u`, as described above.
record_unborn() {
  local copy empty_tree
  copy="../$(basename "$PWD")-git"
  empty_tree=$(git hash-object -t tree /dev/null)
  git rev-parse 'new^{tree}' > .git/new-tree
  cp -R . "$copy"
  if (cd "$copy" && LC_ALL=C git read-tree -m -u "$empty_tree" new) > "$copy/.git/error" 2>&1; then
    rm "$copy/.git/error"
  fi
}

# Every kind of change in a clean worktree: changed, added and removed files, a new directory, and one that the switch
# leaves empty.
(start_case clean
  mkdir -p d/sub gone
  write a b d/c d/e d/sub/f gone/x
  commit old
  git switch -q -c new
  echo a2 > a
  git rm -q d/e gone/x
  mkdir new-dir
  write d/g new-dir/n
  commit new
  git switch -q main
  record
)

# A file becomes a directory, and a directory with a subdirectory becomes a file, as do untracked empty directories.
(start_case to-dir
  mkdir -p d/sub
  write p d/x d/sub/y
  commit old
  git switch -q -c new
  git rm -q -r p d
  mkdir p
  write p/q
  echo d > d
  echo e > e
  commit new
  git switch -q main
  mkdir -p e/f
  record
)

# Changes at paths that the trees don't change stay: a staged and an unstaged change, a staged new file, a staged
# deletion and an untracked file. So do the staged changes that the new tree makes as well: a change, a new file and a
# deletion.
(start_case kept
  write a c deleted removed staged unstaged
  commit old
  git switch -q -c new
  echo a2 > a
  echo c2 > c
  write same
  git rm -q removed
  commit new
  git switch -q main
  echo c2 > c
  echo staged2 > staged
  echo added > added
  write same
  git add c staged added same
  echo unstaged2 > unstaged
  git rm -q deleted removed
  echo untracked > untracked
  record
)

# Ignored files in the way of the new tree are overwritten or removed: a file at the path of a new file, a file at the
# path of a new directory, and a directory with only ignored files at the path of a new file. An ignored file at the
# path of a file that only the old tree has stays.
(start_case ignored
  printf '%s\n' ignored in-the-way '*.log' > .gitignore
  echo old > old.log
  git add .gitignore
  git add -f old.log
  git commit -q -m old
  git switch -q -c new
  git rm -q old.log
  mkdir in-the-way
  echo tracked > ignored
  echo x > in-the-way/x
  echo logs > logs
  git add -f ignored in-the-way/x
  git add logs
  git commit -q -m new
  git switch -q main
  git rm -q --cached old.log
  echo untracked > ignored
  echo untracked > in-the-way
  mkdir logs
  echo log > logs/a.log
  record
)

# Local changes at paths that the trees change are refused. Staged changes that differ from both trees: a change, a
# deletion, a change of a file that the new tree removes, and a new file that the new tree adds with other content.
# Unstaged changes of files that the new tree changes or removes, even one whose file has the content of the new tree,
# and of files whose directory became a file. Git lists the staged ones before the unstaged ones, under the same
# message.
(start_case local
  mkdir dir
  write deleted dir/x dir/y dropped matching removed staged unstaged
  commit old
  git switch -q -c new
  echo added2 > added
  echo deleted2 > deleted
  echo x2 > dir/x
  echo matching2 > matching
  echo staged2 > staged
  echo unstaged2 > unstaged
  git rm -q dir/y dropped removed
  commit new
  git switch -q main
  git rm -q deleted
  echo added3 > added
  echo dropped3 > dropped
  echo staged3 > staged
  git add added dropped staged
  echo matching2 > matching
  echo unstaged3 > unstaged
  echo removed3 > removed
  rm -r dir
  echo dir > dir
  record
)

# Untracked files that the switch would overwrite or remove are refused: one at the path of a new file, one at the path
# of a directory of the new tree, and one at the path of a file that only the old tree has.
(start_case untracked
  write a removed
  commit old
  git switch -q -c new
  git rm -q removed
  mkdir d
  write added d/x
  commit new
  git switch -q main
  git rm -q --cached removed
  echo untracked > added
  echo untracked > d
  record
)

# Directories whose files would be lost are refused: the directory of a changed file and others with untracked files or
# a repository in them, which new files replace, and directories in the place of tracked files, which the changed or
# removed entries of those files would lose.
(start_case dirs
  mkdir changed untracked
  write changed/x untracked/x replaced emptied filled
  commit old
  git switch -q -c new
  git rm -q -r changed untracked emptied filled
  write changed untracked new-dir repo
  echo replaced2 > replaced
  commit new
  git switch -q main
  echo changed2 > changed/x
  echo y > untracked/y
  mkdir new-dir
  echo f > new-dir/f
  nested_repository repo
  rm replaced emptied filled
  mkdir replaced emptied filled
  echo f > replaced/f
  echo f > filled/f
  record
)

# Files that are gone aren't changes: a file that the new tree changes, one that it removes, and the directory of a file
# that it changes.
(start_case gone
  mkdir d
  write a b d/x
  commit old
  git switch -q -c new
  echo a2 > a
  echo x2 > d/x
  git rm -q b
  commit new
  git switch -q main
  rm -r a b d
  record
)

# The directories that the removed files leave empty are removed, but not those that keep untracked files, nor empty
# untracked directories.
(start_case empty
  mkdir -p d e/f keep
  write a d/x e/f/g keep/k
  commit old
  git switch -q -c new
  git rm -q d/x e/f/g keep/k
  commit new
  git switch -q main
  mkdir untracked
  echo untracked > keep/untracked
  record
)

# The current working directory stays although the switch leaves it empty.
(start_case cwd
  mkdir d
  write a d/x
  commit old
  git switch -q -c new
  git rm -q d/x
  commit new
  git switch -q main
  record d
)

# The current working directory isn't replaced by a file of the new tree.
(start_case cwd-file
  mkdir d
  write a d/x
  commit old
  git switch -q -c new
  git rm -q d/x
  echo d > d
  commit new
  git switch -q main
  record d
)

# The current working directory isn't removed at the path of a file that only the old tree has.
(start_case cwd-old
  write a p
  commit old
  git switch -q -c new
  git rm -q p
  commit new
  git switch -q main
  git rm -q p
  mkdir p
  record p
)

# Submodules keep their worktrees: one whose commit changes, and one that is removed but has files, which git can't
# remove. A removed one whose directory is empty goes, a file replaces another one whose directory is empty, and a new
# one gets an empty directory.
(start_case gitlinks
  echo a > a
  nested_repository changed
  nested_repository removed
  git add --no-warn-embedded-repo a changed removed
  inner=$(git -C changed rev-parse HEAD)
  git update-index --add --cacheinfo "160000,$inner,emptied" --cacheinfo "160000,$inner,filed"
  git commit -q -m old
  git switch -q -c new
  (cd changed && echo two > two && git add two && git commit -q -m two)
  git add changed
  git rm -q --cached removed emptied filed
  echo filed > filed
  git add filed
  git update-index --add --cacheinfo "160000,$inner,added"
  git commit -q -m new
  git switch -q main
  record
)

# A conflict is refused before anything else is checked.
(start_case unmerged
  write a b
  commit base
  git branch new
  git switch -q -c theirs
  echo theirs > a
  commit theirs
  git switch -q main
  echo ours > a
  commit ours
  git merge -q theirs > /dev/null 2>&1 || :
  rm -f .git/MERGE_HEAD .git/MERGE_MSG .git/MERGE_MODE .git/AUTO_MERGE
  record
)

# The files of changed entries are checked out with the attributes of the new tree, and the other files aren't touched.
(start_case eol
  printf 'one\ntwo\n' > a.txt
  printf 'one\ntwo\n' > kept.txt
  commit old
  git switch -q -c new
  echo '*.txt eol=crlf' > .gitattributes
  printf 'one\ntwo\nthree\n' > a.txt
  commit new
  git switch -q main
  record
)

# Filters apply when files are compared with their entries and when they are checked out.
(start_case ident
  echo '*.c ident' > .gitattributes
  echo '$Id$' > f.c
  commit old
  git switch -q -c new
  printf '$Id$\nchanged\n' > f.c
  commit new
  git switch -q main
  record
)

# On a branch without commits, the entries of the new tree are added, and untracked files that aren't in their way stay.
(start_case unborn
  git switch -q -c new
  mkdir d
  write a d/x
  commit new
  git switch -q --orphan main
  echo draft > draft
  record_unborn
)

# On a branch without commits, an untracked file in the way of an entry of the new tree is refused.
(start_case unborn-way
  git switch -q -c new
  write a
  commit new
  git switch -q --orphan main
  echo untracked > a
  record_unborn
)

# On a branch without commits, staged files stay if the new tree has the same entries or none at their paths.
(start_case unborn-kept
  git switch -q -c new
  write a c
  commit new
  git switch -q --orphan main
  write a b
  git add a b
  record_unborn
)

# On a branch without commits, a staged file that differs from the entry of the new tree is refused.
(start_case unborn-staged
  git switch -q -c new
  write a
  commit new
  git switch -q --orphan main
  echo other > a
  git add a
  record_unborn
)

# A staged change of a file in a directory that a new file replaces. Git loses it.
(start_case dev-staged
  mkdir d
  write a d/x
  commit old
  git switch -q -c new
  git rm -q d/x
  echo d > d
  commit new
  git switch -q main
  echo staged > d/x
  git add d/x
  record
)

# A staged new file in a directory that a new file replaces. Git loses it.
(start_case dev-added
  mkdir d
  write a d/x
  commit old
  git switch -q -c new
  git rm -q d/x
  echo d > d
  commit new
  git switch -q main
  echo y > d/y
  git add d/y
  record
)

# A staged new file whose file is gone, at the path of a directory of the new tree. Git drops its entry.
(start_case dev-gone
  write a
  commit old
  git switch -q -c new
  mkdir d
  write d/x
  commit new
  git switch -q main
  echo d > d
  git add d
  rm d
  record
)

# A staged new file at the path of a directory of the new tree. Git refuses it as an untracked file, but loses it on a
# file system that ignores case.
(start_case dev-file
  write a
  commit old
  git switch -q -c new
  mkdir d
  write d/x
  commit new
  git switch -q main
  echo d > d
  git add d
  record
)

# An ignored directory with a repository in it, at the path of a new file. Git removes it along with the repository.
(start_case dev-repo
  echo 'p/' > .gitignore
  echo a > a
  commit old
  git switch -q -c new
  echo p > p
  commit new
  git switch -q main
  nested_repository p
  record
)

# A new file replaces the worktree of a submodule, which has files. Git removes the worktree along with its repository.
(start_case dev-sub
  echo a > a
  commit base
  git switch -q -c new
  echo file > sub
  commit new
  git switch -q main
  nested_repository sub
  git add --no-warn-embedded-repo sub
  git commit -q -m old
  record
)

# A file at the path of a submodule whose commit changes. Git replaces it with the directory of the submodule.
(start_case dev-gitlink
  echo a > a
  git add a
  git update-index --add --cacheinfo "160000,$(echo one | git hash-object --stdin),s"
  git commit -q -m old
  git switch -q -c new
  git update-index --cacheinfo "160000,$(echo two | git hash-object --stdin),s"
  git commit -q -m new
  git switch -q main
  rmdir s
  echo file > s
  record
)

# The current working directory is in a directory that a new file replaces. Git removes it, or fails to remove it on a
# host that can't remove the current directory of a process.
(start_case dev-cwd
  mkdir -p d/sub
  write a d/sub/f
  commit old
  git switch -q -c new
  git rm -q -r d
  echo d > d
  commit new
  git switch -q main
  record d/sub
)

# A staged file in a directory at the path of a file that only the old tree has. Git removes the entry and the file.
(start_case dev-old
  write a p
  commit old
  git switch -q -c new
  git rm -q p
  commit new
  git switch -q main
  git rm -q p
  mkdir p
  echo x > p/x
  git add p/x
  record
)
