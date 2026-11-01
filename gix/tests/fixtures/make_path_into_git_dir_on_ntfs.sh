#!/usr/bin/env bash
set -eu -o pipefail

# The branch `evil` has a file whose name leads into `.git` on NTFS, which Git refuses to read into an index on all
# platforms.

git init -q
git checkout -q -b main
git commit -q --allow-empty -m base

blob=$(echo content | git hash-object -w --stdin)
tree=$(printf '100644 blob %s\t.git\\config\n100644 blob %s\tok\n' "$blob" "$blob" | git mktree)
git update-ref refs/heads/evil "$(git commit-tree -m evil "$tree")"

if git read-tree evil 2>/dev/null; then
  echo "Git reads a path that leads into .git" >&2
  exit 1
fi
