#!/usr/bin/env bash
set -eu -o pipefail

# A tag that leads back to itself, as only a corrupt repository has it: a copy of a tag under the id that the tag names as
# its target, to which `refs/tags/loop` points. Git refuses to follow it, as it verifies the ids of the objects it reads.

git init -q
git checkout -q -b main
git commit -q --allow-empty -m base

empty_tree=$(git hash-object -t tree /dev/null)
id=${empty_tree//?/a}
tag=$(printf 'object %s\ntype tag\ntag loop\ntagger foo <foo@example.com> 1234 +0000\n\nthe message\n' "$id" |
  git hash-object -t tag -w --stdin)
mkdir -p .git/objects/aa
cp ".git/objects/${tag:0:2}/${tag:2}" ".git/objects/aa/${id:2}"
echo "$id" > .git/refs/tags/loop

if git rev-parse --verify --quiet 'refs/tags/loop^{commit}' >/dev/null 2>&1; then
  echo "Git follows a tag that leads back to itself" >&2
  exit 1
fi
