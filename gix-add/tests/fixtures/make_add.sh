#!/usr/bin/env bash
set -eu -o pipefail

# Each case is a repository whose worktree differs from its index, and `record` keeps what `git add --all` makes of it.
# The cases only use regular files and directories, so that their results are the same on all hosts.
#
# `record` runs `git add --all` on `.git/index.after`, a copy of the index, with an object directory of its own,
# `.git/git-objects`, whose alternate is the repository's. That way `.git/index` and `.git/objects` remain as they were
# before, `.git/git-objects` holds exactly the blobs that git wrote, and `.git/index.after` the entries that git left.
# If git fails, which it also does after it changed the index in some cases, its standard error goes to `.git/error`.
# All of these are in `.git`, which `git add` doesn't look at.
#
# Case names are short so that the SHA-256 object paths of the repositories nested in a case stay within Windows'
# traditional path limit, which Git for Windows keeps by default.

# start_case <name> creates the repository of a case and enters it. Each case leaves it again at its end.
start_case() {
  git init -q "$1"
  cd "$1"
}

# commit <message> commits all changes of the worktree.
commit() {
  git add --all
  git commit -q -m "$1"
}

# record [<pathspec>...] runs `git add --all <pathspec>...` as described above, in the C locale, which keeps its messages
# untranslated.
record() {
  if test -e .git/index; then
    cp .git/index .git/index.after
  fi
  mkdir .git/git-objects
  if GIT_INDEX_FILE=.git/index.after GIT_OBJECT_DIRECTORY=.git/git-objects GIT_ALTERNATE_OBJECT_DIRECTORIES=.git/objects \
    LC_ALL=C git add --all "$@" 2> .git/error; then
    rm .git/error
  fi
}

# nested_repository <path> creates a repository at <path> with one commit.
nested_repository() {
  mkdir -p "$1"
  (cd "$1" && git init -q && echo inner > inner && git add inner && git commit -q -m inner)
}

# conflict <theirs> leaves the merge of a branch whose `a` is <theirs>, or which deleted `a` if <theirs> is empty, with a
# branch whose `a` is `ours`. The merge conflicts in `a`, which the worktree has as the merge left it.
conflict() {
  echo base > a
  echo kept > kept
  commit base
  git switch -q -c theirs
  if test -n "$1"; then
    echo "$1" > a
  else
    git rm -q a
  fi
  commit theirs
  git switch -q main
  echo ours > a
  commit ours
  if git merge -q theirs > /dev/null; then
    echo 'the merge was expected to conflict' >&2
    exit 1
  fi
}

# A new repository has no index until one is written. Files in the root and in nested directories are added, the
# ignored one isn't.
(start_case new-repository
  printf '*.log\n' > .gitignore
  echo a > a
  mkdir -p dir/deeper
  echo b > dir/b
  echo c > dir/deeper/c
  echo log > ignored.log
  record
)

# Modified, deleted and new files in the root and in a directory, and an ignored file.
(start_case changes
  printf '*.log\n' > .gitignore
  echo a > a
  echo b > b
  mkdir dir
  echo c > dir/c
  echo d > dir/d
  commit base
  echo a2 > a
  rm b dir/d
  echo new > new
  echo new > dir/new
  echo log > ignored.log
  record
)

# A pathspec limits the changes to its paths, the directory `dir` here.
(start_case pathspec
  echo a > a
  mkdir dir
  echo c > dir/c
  echo d > dir/d
  commit base
  echo a2 > a
  echo c2 > dir/c
  rm dir/d
  echo new > new
  echo new > dir/new
  record dir
)

# Without magic, the `*` of a pathspec also matches slashes, so that `*.txt` matches text files in all directories.
(start_case pathspec-wildcard
  echo a > a.txt
  echo b > b.md
  mkdir dir
  echo c > dir/c.txt
  commit base
  echo a2 > a.txt
  echo b2 > b.md
  echo c2 > dir/c.txt
  echo new > new.txt
  echo new > dir/new.md
  record '*.txt'
)

# A pathspec can select paths by their attributes.
(start_case pathspec-attributes
  printf '*.dat track\n' > .gitattributes
  echo a > a.dat
  echo b > b.txt
  commit base
  echo a2 > a.dat
  echo b2 > b.txt
  echo new > new.dat
  echo new > new.txt
  record ':(attr:track)'
)

# An ignored file that a pathspec names isn't added. Git reports it and fails after it added the other changes.
(start_case pathspec-ignored-file
  printf '*.log\n' > .gitignore
  echo a > a
  commit base
  echo a2 > a
  echo log > named.log
  record a named.log
)

# A directory that is a repository is added as the commit that its `HEAD` points to.
(start_case nested-repository
  echo a > a
  commit base
  nested_repository inner
  record
)

# A repository whose `HEAD` doesn't point to a commit can't be added, so git fails without changing the index.
(start_case nested-repository-without-commit
  echo a > a
  commit base
  mkdir inner
  (cd inner && git init -q)
  echo new > new
  record
)

# Repositories in ignored directories are left out like everything else in them.
(start_case ignored-repository
  printf 'ignored/\n' > .gitignore
  echo a > a
  commit base
  nested_repository ignored/inner
  record
)

# Only directories that contain `.git` are repositories, so the files of a bare repository are added like any other.
# Its empty directories are left out.
(start_case bare-repository
  echo a > a
  commit base
  mkdir -p inner.git/objects inner.git/refs
  echo 'ref: refs/heads/main' > inner.git/HEAD
  printf '[core]\n\tbare = true\n' > inner.git/config
  record
)

# A `.git` file that doesn't name a repository doesn't make its directory one. The directory's other files are added,
# while the `.git` file isn't.
(start_case invalid-dot-git-file
  echo a > a
  commit base
  mkdir sub
  echo 'gitdir: /nonexistent' > sub/.git
  echo s > sub/s
  record
)

# The file in the worktree resolves a conflict, replacing its stages.
(start_case conflict-resolved
  conflict theirs
  echo resolved > a
  record
)

# The stages of a conflict whose file was deleted are removed.
(start_case conflict-deleted
  conflict theirs
  rm a
  record
)

# A file that was modified on our side and deleted on theirs is resolved with the file that the merge left.
(start_case conflict-modified-deleted
  conflict ''
  record
)

# Submodules are added as the commits that their `HEAD`s point to, whatever else changed in them. A submodule that isn't
# checked out keeps its entry, and that of a submodule whose directory is gone is removed.
git init -q submodule-source
(cd submodule-source && echo source > source && git add source && git commit -q -m source)
(start_case submodules
  echo a > a
  commit base
  for name in moved not-checked-out removed; do
    git submodule add -q ../submodule-source "$name"
  done
  git commit -q -m submodules
  (cd moved && echo more > more && git add more && git commit -q -m more && echo dirty > dirty)
  git submodule deinit -q -f not-checked-out
  rm -r removed
  record
)

# Entries that were added with intent to add are added for real, or removed if their files are gone.
(start_case intent-to-add
  echo a > a
  commit base
  echo n > n
  echo m > m
  git add --intent-to-add n m
  rm m
  record
)

# A directory that replaced a tracked file is added, and the entry of the file is removed.
(start_case file-to-directory
  echo a > a
  commit base
  rm a
  mkdir a
  echo b > a/b
  record
)

# A repository that replaced a tracked file is added in its place, as the commit that its `HEAD` points to.
(start_case file-to-repository
  echo a > a
  echo kept > kept
  commit base
  rm a
  nested_repository a
  record
)

# A file that replaced a tracked directory is added, and the entries of the directory are removed.
(start_case directory-to-file
  mkdir d
  echo x > d/x
  echo y > d/y
  commit base
  rm -r d
  echo f > d
  record
)

# Tracked files are added although they are ignored, unlike new files that are ignored.
(start_case tracked-and-ignored
  echo t > t.log
  git add t.log
  printf '*.log\n' > .gitignore
  commit base
  echo t2 > t.log
  echo new > new.log
  record
)

# Directories without files aren't added.
(start_case empty-directories
  echo a > a
  commit base
  mkdir -p empty/deeper
  record
)

# Files are ignored by the patterns of `.git/info/exclude`, of `.gitignore` files in their directories, where a later
# pattern can negate an earlier one, and of the file that `core.excludesFile` names, here relative to the worktree, where
# git runs.
(start_case excludes
  echo a > a
  commit base
  mkdir -p .git/info nested
  echo from-info >> .git/info/exclude
  printf '*.tmp\n!keep.tmp\n' > nested/.gitignore
  git config core.excludesFile .git/custom-excludes
  echo from-custom > .git/custom-excludes
  touch from-info from-custom nested/ignored.tmp nested/keep.tmp nested/plain
  record
)

# Files in ignored directories aren't added.
(start_case ignored-directory
  printf 'build/\n' > .gitignore
  echo a > a
  commit base
  mkdir build
  echo out > build/out
  record
)

# A file whose modification time changed while its content didn't keeps its entry.
(start_case modification-time
  echo a > a
  mkdir dir
  echo b > dir/b
  commit base
  touch -d '2001-01-01 00:00:00' a
  record
)

# A change in one directory leaves the trees of the others valid in the cache tree that the commit recorded.
(start_case cache-tree
  mkdir -p changed/deeper unchanged
  echo 1 > changed/deeper/file
  echo 2 > changed/file
  echo 3 > unchanged/file
  echo 4 > top
  commit base
  echo 1b > changed/deeper/file
  record
)

# Without symbolic links, a symbolic link is checked out as a file that contains its target, and the content of that
# file is added as the target of the link.
(start_case symlink-entry-without-symlinks
  git config core.symlinks false
  echo target > target
  git add target
  git update-index --add --cacheinfo "120000,$(printf target | git hash-object -w --stdin),link"
  git commit -q -m base
  printf elsewhere > link
  record
)

# Without `core.filemode`, the executable bits of files don't matter: entries keep their modes, and new files are added
# as regular files.
(start_case filemode-false
  git config core.filemode false
  echo a > a
  echo x > x
  git add a x
  git update-index --chmod=+x x
  git commit -q -m base
  echo x2 > x
  echo new > new
  record
)

# With `core.autocrlf`, the CRLF line endings of text files become LF, while files that are binary or have a lone CR
# are added as they are.
(start_case autocrlf
  echo base > base
  commit base
  git config core.autocrlf true
  printf 'one\r\ntwo\r\n' > crlf.txt
  printf 'one\ntwo\n' > lf.txt
  printf 'mixed\r\nlf\n' > mixed.txt
  printf 'bin\0ary\r\n' > binary.dat
  printf 'lone\rcr\n' > lone-cr.txt
  record
)

# With `core.autocrlf`, a file whose blob in the index has CRLF line endings keeps them.
(start_case autocrlf-with-crlf-in-index
  printf 'one\r\n' > keep.txt
  commit base
  git config core.autocrlf true
  printf 'one\r\ntwo\r\n' > keep.txt
  record
)

# The `text=auto` attribute converts the line endings of text files without `core.autocrlf`.
(start_case text-auto
  printf '* text=auto\n' > .gitattributes
  commit base
  printf 'one\r\ntwo\r\n' > crlf.txt
  record
)

# A sparse index records each directory outside of the cone of a sparse checkout as one entry.
(start_case sparse-index
  mkdir in out
  echo in > in/file
  echo out > out/file
  echo top > top
  commit base
  git sparse-checkout set --cone --sparse-index in
)

# Without a sparse index, the entries outside of a sparse checkout are marked as such.
(start_case sparse-checkout
  mkdir in out
  echo in > in/file
  echo out > out/file
  echo top > top
  commit base
  git sparse-checkout set --cone --no-sparse-index in
)

# The modification times of these files are set by the tests, which copy the case for that.
(start_case racy
  echo old > old
  echo future > future
  mkdir outside
  echo kept > outside/kept
  commit base
)
