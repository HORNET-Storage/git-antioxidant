#!/usr/bin/env bash
set -eu -o pipefail

# Include sections whose names are spelled with other cases, and the values that Git reads with the files that they
# include into `expected`. Section names ignore case, while the condition of `includeIf` doesn't, so `GITDIR:` matches
# nothing.
git init -q repo
for value in one two three wrong; do
  printf '[answer]\n\tvalue = %s\n' "$value" >"repo/.git/$value"
done
cat >>repo/.git/config <<'EOF'
[Include]
	PaTh = one
[INCLUDEIF "gitdir:**"]
	path = two
[includeif "gitdir:**"]
	path = three
[INCLUDEIF "GITDIR:**"]
	path = wrong
EOF
git -C repo config --get-all answer.value >expected
