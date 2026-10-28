#!/usr/bin/env bash
set -eu -o pipefail

# Record which remote references `git push --dry-run --porcelain` updates, and with which local sources, for
# push refspecs as `match_push_refs()` in Git's `remote.c` matches them, along with the errors Git reports.
#
# `base` pushes into `remote.git`. The two have branches and tags the other lacks, short names that are
# ambiguous on either side (`x` locally, `ambiguous` remotely), a remote name matched both inside and outside
# `refs/heads/` (`both`), and references outside `refs/heads/` and `refs/tags/` that only match weakly
# (`origin/main` locally, `upstream/main` remotely). `refs/remotes/origin/HEAD` is a symbolic reference to a
# remote-tracking branch, which can't name a destination. `detached` is a clone of `base` with a detached `HEAD`.
#
# Each pushing repository records what the matching needs to know about it: its references as `git push` lists
# them in `local-refs.list` (object, name, target of symbolic references), what `HEAD` points to in `head-symref`
# (empty if detached), and the objects the revisions used as sources name in `revisions.list`.

function record_local() {
  git for-each-ref --format='%(objectname) %(refname) %(symref)' > local-refs.list
  git symbolic-ref -q HEAD > head-symref || :
  local revision
  for revision in "$@"; do
    echo "$(git rev-parse "$revision") $revision"
  done > revisions.list
}

# Append what Git does with the refspecs given as arguments to `baseline.git`, followed by a `specs:` line.
function baseline() {
  {
    git -c advice.pushUnqualifiedRefName=false push --dry-run --porcelain ../remote.git "$@" 2>&1 || :
    echo specs: "$@"
  } >> baseline.git
}

git init -q base
(cd base
  git commit -q --allow-empty -m initial
  git tag -m annotated annotated-v0.0
  git tag v0.0
  git commit -q --allow-empty -m second
  git checkout -q -b f1 main~1
  git commit -q --allow-empty -m f1
  git checkout -q -b f2 main~1
  git commit -q --allow-empty -m f2
  git checkout -q -b sub/f3 main
  git branch x main
  git tag x main~1
  git update-ref refs/remotes/origin/main main
  git symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main
  git checkout -q main
)

git init -q --bare remote.git
# The names of loose SHA-256 objects received into deeply nested fixture directories exceed what Windows allows
# unless Git for Windows uses long paths. Other Git builds ignore the setting.
git -C remote.git config core.longPaths true
git -C base push -q ../remote.git \
  main~1:refs/heads/main f1:refs/heads/f1 main~1:refs/heads/remote-only \
  main~1:refs/heads/ambiguous main~1:refs/tags/ambiguous \
  main~1:refs/heads/both main~1:refs/remotes/both main~1:refs/remotes/upstream/main \
  refs/tags/v0.0:refs/tags/v0.0
# Relative paths are resolved from the top of the current repository, which may contain the fixture directory.
git -C base ls-remote ../remote.git > remote-refs.list

git clone -q base detached
git -C detached checkout -q --detach main

(cd base
  initial=$(git rev-parse main~1)
  record_local HEAD HEAD~1 main~1 "$initial"

  # Sources and destinations named fully, partially, or not at all
  baseline main
  baseline refs/heads/main
  baseline heads/main
  baseline f2
  baseline v0.0
  baseline annotated-v0.0
  baseline main:refs/heads/new
  baseline main:main
  baseline main:remote-only
  baseline main:both
  baseline main:upstream/main
  baseline origin/main
  baseline f1:refs/heads/f1
  baseline +main:refs/heads/f1
  baseline main:refs/heads/f1

  # Destinations that aren't full names are put below the namespace of their source if the remote lacks them
  baseline main:new
  baseline v0.0:new-tag
  baseline "$initial:by-id"

  # Sources that aren't local references
  baseline HEAD
  baseline @
  baseline HEAD:refs/heads/from-head
  baseline HEAD:from-head
  baseline HEAD~1:refs/heads/older
  baseline "$initial:refs/heads/by-id"
  baseline "$initial"
  baseline origin/HEAD

  # Deletions
  baseline :refs/heads/remote-only
  baseline :remote-only
  baseline +:remote-only
  baseline :refs/heads/does-not-exist
  baseline :does-not-exist

  # Ambiguity and conflicts
  baseline x
  baseline nonexistent
  baseline main:ambiguous
  baseline main:refs/heads/dup f1:refs/heads/dup
  baseline main nonexistent :does-not-exist
  baseline main:ambiguous origin/HEAD

  # Matching branches, patterns and negative refspecs
  baseline :
  baseline +:
  baseline 'refs/heads/*'
  baseline 'refs/heads/*:refs/heads/mirror/*'
  baseline 'refs/heads/f*:refs/heads/g*'
  baseline 'refs/tags/*:refs/tags/*'
  baseline main 'refs/heads/*:refs/heads/*'
  baseline 'refs/heads/*:refs/heads/*' main:refs/heads/f1
  baseline 'refs/heads/*:refs/heads/*' :
  baseline : 'refs/heads/f*:refs/heads/mirror/f*'
  baseline : '^refs/heads/main'
  baseline 'refs/heads/*:refs/heads/*' '^refs/heads/f1'
  baseline 'refs/heads/*:refs/heads/mirror/*' '^refs/heads/f1'
  baseline 'refs/heads/*:refs/heads/mirror/*' '^refs/heads/mirror/f*'
)

(cd detached
  record_local HEAD

  baseline main
  baseline HEAD
  baseline HEAD:refs/heads/from-detached
  baseline HEAD:from-detached
  baseline 'refs/remotes/origin/*:refs/heads/*'
)
