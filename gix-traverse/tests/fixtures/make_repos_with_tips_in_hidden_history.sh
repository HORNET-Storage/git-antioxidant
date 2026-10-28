#!/usr/bin/env bash
set -eu -o pipefail

# Each repository has the linear history `old`, `c1`, …, `hidden`, `tip`, and a tag named like the message of each commit.
# A traversal from `tip` and `old` that hides `hidden` must return `tip` alone. As `tip` is a child of `hidden`, every
# commit below `hidden` is reachable from a visible tip and from the hidden tip, and the traversal can tell that `hidden`
# reaches `old` only by following these commits down to `old`. Each repository has a commit-graph, whose generation
# numbers order commits by ancestry. Without it, commits are ordered by commit time, which puts `old` first here.

function commit_at() {
  local message=${1:?first argument is the commit message}
  local timestamp=${2:?second argument is the timestamp}
  GIT_COMMITTER_DATE="$timestamp +0000" GIT_AUTHOR_DATE="$timestamp +0000" git commit -q --allow-empty -m "$message"
  git tag "$message"
}

# All commits have the same time.
(git init -q same_time && cd same_time
  for name in old c1 hidden tip; do
    commit_at "$name" 1000000000
  done
  git commit-graph write --no-progress --reachable
)

# `old` is newer than every commit that follows it, as if the clock of its committer had been ahead.
(git init -q newer_old && cd newer_old
  commit_at old 1000000500
  commit_at c1 1000000100
  commit_at c2 1000000200
  commit_at hidden 1000000300
  commit_at tip 1000000400
  git commit-graph write --no-progress --reachable
)

# Record Git's results beside each repository, so that the tests don't run Git.
for repo in same_time newer_old; do
  (cd "$repo"
    git log --all --format='%H %s' >commit-names.baseline
    git rev-list hidden >rev-list-hidden.baseline
    git rev-list tip old --not hidden >rev-list.baseline
  )
done
