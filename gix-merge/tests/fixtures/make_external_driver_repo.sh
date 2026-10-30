#!/usr/bin/env bash
set -eu -o pipefail

# `git merge` runs a merge driver in a repository whose directory has a name that makes a shell run a command, which
# would leave a file `injected`. The driver answers with what it received, so the merged `with space` holds what Git gave
# it, which the tests compare with what the same driver receives from gix-merge. `driver-command` holds the driver, and
# `ancestor-label` the name Git gives to the common ancestor.

cat >driver-command <<'EOF'
received=$(printf '[%s]\n' '%%A' '100%' '%Z' '%%%' '%%%L' %L %P %S %X %Y "$(basename "$PWD")" "$(cat %O %A %B)" "$(case %O%A%B in (*/*) echo paths;; (*) echo names;; esac)") && printf '%s\n' "$received" >%A # 100%
EOF

git init -q 'repo $(touch injected)'
cd 'repo $(touch injected)'
git config merge.custom.driver "$(cat ../driver-command)"
echo '* merge=custom' >.gitattributes
echo base >'with space'
git add .
git commit -q -m base

echo ours >'with space'
git commit -q -a -m ours
git checkout -q -b feature HEAD~1
echo theirs >'with space'
git commit -q -a -m theirs
git checkout -q -

git merge -q --no-edit feature
git rev-parse --short HEAD~2 >../ancestor-label
