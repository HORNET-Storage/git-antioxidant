#!/usr/bin/env bash
set -eu -o pipefail

# Commits whose tree names a commit, a tag or a blob, as only a corrupt repository has them, with their ids in the files
# `commit`, `tag` and `blob`. Git refuses to take what they name for their tree, as `parse_commit_buffer()` looks the tree
# up as a tree.

git init -q
git checkout -q -b main
git commit -q --allow-empty -m base
git tag -a -m tag tag
blob=$(printf 'blob\n' | git hash-object -w --stdin)
for kind in commit tag blob; do
  case $kind in
    commit) target=$(git rev-parse HEAD) ;;
    tag) target=$(git rev-parse tag) ;;
    blob) target=$blob ;;
  esac
  printf 'tree %s\nauthor foo <foo@example.com> 1234 +0000\ncommitter foo <foo@example.com> 1234 +0000\n\nwrong tree\n' "$target" |
    git hash-object --literally -t commit -w --stdin >"$kind"
  if git rev-parse --verify --quiet "$(cat "$kind")^{tree}" >/dev/null 2>&1; then
    echo "Git takes the $kind that a commit names as its tree for a tree" >&2
    exit 1
  fi
done
