#!/usr/bin/env bash
set -eu -o pipefail

# `base` has the history A-B-C-D on `main` and B-E-F on `side`, whose merge-base is B, and a commit-graph.
# The other repositories are shallow copies of it. Git reads the commits that a `shallow` file lists as having
# no parents, and reads no commit-graph in a shallow repository, so each records what Git finds there:
# * `kept` became shallow by fetching with a depth after a complete clone. It keeps the objects of B and A, and
#   its commit-graph, written before it became shallow, records B as the parent of C and E, its shallow commits.
# * `graph-only` is a shallow clone with a copy of the commit-graph of `base`, which records the parents of its
#   shallow commits C and E although the clone lacks their objects.
# * `depth-3` is a shallow clone whose `shallow` file lists B, the merge-base.

function commit() {
  local name=$1 file=$2 seconds=$3
  echo "$name" >"$file"
  git add "$file"
  GIT_AUTHOR_DATE="@$seconds +0000" GIT_COMMITTER_DATE="@$seconds +0000" git commit -q -m "$name"
}

function baseline() {
  git merge-base --all main side >merge-base.baseline || :
  git rev-list main side >rev-list.baseline
}

git init -q -b main base
(
  cd base
  commit A file 100
  commit B file 200
  git branch side
  commit C file 300
  commit D file 400
  git checkout -q side
  commit E other 250
  commit F other 350
  git checkout -q main
  git commit-graph write --reachable
  baseline
)

git clone -q --no-local base kept
(
  cd kept
  git branch -q side origin/side
  git commit-graph write --reachable
  git fetch -q --depth=2 origin main side
  baseline
)

git clone -q --no-local --depth=2 --no-single-branch base graph-only
(
  cd graph-only
  git branch -q side origin/side
  cp ../base/.git/objects/info/commit-graph .git/objects/info/
  baseline
)

git clone -q --no-local --depth=3 --no-single-branch base depth-3
(
  cd depth-3
  git branch -q side origin/side
  baseline
)
