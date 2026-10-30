#!/usr/bin/env bash
set -eu -o pipefail

# `git diff` runs an external command with an option, which makes it a command line for the shell. `received` holds what
# the command printed, which is what it received from Git but for the names of tempfiles, and the tests compare that with
# what the same command prints if gix-diff prepares it. `command` holds the command.

cat >command <<'EOF'
f() { printf '[%s]\n' "$0" "$#" "$1" "$2" "$4" "$5" "$7" "$8" "$GIT_DIFF_PATH_COUNTER" "$GIT_DIFF_PATH_TOTAL"; }; f --option
EOF

git init -q repo
cd repo
echo old >file
git add file
git commit -q -m old
echo new >file
git commit -q -a -m new
git -c diff.external="$(cat ../command)" diff HEAD~1 HEAD >../received
