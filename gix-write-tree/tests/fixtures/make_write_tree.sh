#!/usr/bin/env bash
set -eu -o pipefail

# Each case is a repository whose index `git write-tree` turns into trees. Entries are mostly added to the index directly,
# so that symbolic links, submodules and missing objects don't depend on the file system.
#
# `record` runs `git write-tree` on `index.after`, a copy of the index, with an object directory of its own whose
# alternate is the repository's. That way `.git/index` and `.git/objects` remain as they were before, `git-trees/` holds
# exactly the trees that git wrote, and `index.after` the cache tree that git computed. The id of the root tree goes to
# `tree`, or git's standard error to `error` if it refused the index.

# start_case <name> creates the repository of a case and enters it. Each case leaves it again at its end.
start_case() {
  git init -q "$1"
  cd "$1"
}

# add <mode> <path> adds what the standard input provides as the entry <path> of the index.
add() {
  local id
  id=$(git hash-object -w --stdin)
  git update-index --add --cacheinfo "$1,$id,$2"
}

# unwritten_id <content> prints the id of a blob with <content> without writing it.
unwritten_id() {
  echo "$1" | git hash-object --stdin
}

# record [<option>...] runs `git write-tree <option>...` as described above, in the C locale, which keeps its messages
# untranslated.
record() {
  cp .git/index index.after
  mkdir git-trees
  if GIT_INDEX_FILE=index.after GIT_OBJECT_DIRECTORY=git-trees GIT_ALTERNATE_OBJECT_DIRECTORIES=.git/objects \
    LC_ALL=C git write-tree "$@" > tree 2> error; then
    rm error
  else
    rm tree
  fi
}

# An empty index is the empty tree. `git read-tree --empty` records it in the cache tree of the index it writes, and as
# git has the empty tree whether or not it was written, `git write-tree` takes it from there without writing it. A new
# repository has no index until one is written.
(start_case empty
  git read-tree --empty
  record
)

# An index that only `git update-index` changed has no cache tree, so git writes the empty tree of an index whose
# entries were all removed like any other tree.
(start_case emptied
  echo removed | add 100644 removed
  git update-index --force-remove removed
  record
)

# Files, an executable, a symbolic link and a submodule whose commit isn't in the repository, in the root and in nested
# directories. The names show that the index sorts its entries as trees sort theirs: the file `a.txt` comes before the
# directory `a`, which comes before `a0`, and the directory `a-b` before `a`. Git writes cache-tree nodes ordered by the
# length of their names, which puts `z` before `yy`.
(start_case kinds
  echo text | add 100644 a.txt
  echo nested | add 100644 a/b
  echo deeper | add 100644 a/c/d
  echo dash | add 100644 a-b/c
  echo zero | add 100644 a0
  echo run | add 100755 run
  printf target | add 120000 link
  git update-index --add --cacheinfo "160000,$(unwritten_id submodule),sub"
  echo short | add 100644 z/file
  echo long | add 100644 yy/file
  record
)

# Entries that were added with intent to add aren't part of any tree, and neither are directories that contain nothing
# else. The cache-tree nodes of their directories and of all directories above are invalid.
(start_case intent-to-add
  echo kept | add 100644 kept
  echo kept | add 100644 mixed/kept
  mkdir -p only/deeper mixed
  echo new > only/deeper/new
  echo new > mixed/new
  echo new > top-new
  git add --intent-to-add only/deeper/new mixed/new top-new
  record
)

# missing_entries adds entries whose blob isn't in the repository, in the root and in a directory. Git looks at the
# entries of a directory after those of its subdirectories, so it refuses `dir/file` although `a-file` comes first.
missing_entries() {
  echo present | add 100644 present
  git update-index --add --cacheinfo "100644,$(unwritten_id missing),a-file"
  git update-index --add --cacheinfo "100644,$(unwritten_id missing),dir/file"
}

(start_case missing
  missing_entries
  record
)

(start_case missing-ok
  missing_entries
  record --missing-ok
)

# Four conflicting paths with three stages each are twelve unmerged entries, of which git lists the first ten.
(start_case unmerged
  echo fine | add 100644 fine
  for path in conflict-1 conflict-2 conflict-3 conflict-4; do
    for stage in 1 2 3; do
      printf '100644 %s %s\t%s\n' "$(echo "$path $stage" | git hash-object -w --stdin)" "$stage" "$path"
    done
  done | git update-index --index-info
  record
)

# A sparse index records each directory outside of the cone of a sparse checkout as one entry with the id of its tree.
(start_case sparse
  mkdir -p in/deep out/deep
  echo 1 > in/file
  echo 2 > in/deep/file
  echo 3 > out/file
  echo 4 > out/deep/file
  echo 5 > top
  git add .
  git commit -q -m initial
  git sparse-checkout init --cone --sparse-index
  git sparse-checkout set in
  git ls-files --sparse --stage > sparse-entries
  record
)

# cache_tree_entries adds entries in directories at several depths and records their cache tree, with the trees that it
# names, by writing them. `index.cached` keeps that index.
cache_tree_entries() {
  echo 1 | add 100644 a/b/c/file
  echo 2 | add 100644 a/b/other
  echo 3 | add 100644 a/sibling/file
  echo 4 | add 100644 x/y
  git write-tree > /dev/null
  cp .git/index index.cached
}

# Changing an entry invalidates the cache-tree nodes of the directories that lead to it, so that git writes their trees
# again and keeps the others. As git doesn't look at the entries of a valid node, it doesn't notice that the blob of
# `x/y` was removed from the repository.
(start_case cache-tree
  cache_tree_entries
  echo changed | add 100644 a/b/c/file
  x_blob_id=$(git rev-parse :x/y)
  rm ".git/objects/${x_blob_id:0:2}/${x_blob_id:2}"
  record
)

# Replacing a directory with a file removes the cache-tree node of the directory.
(start_case replace-directory
  cache_tree_entries
  git update-index --force-remove a/sibling/file
  echo file | add 100644 a/sibling
  record
)
