#!/usr/bin/env bash
set -eu -o pipefail

# A repository whose ignore files per directory are called `.custom-ignore`, as `git ls-files` reads them if it's told so,
# next to a `.gitignore` file that isn't read then. `baseline` lists what Git ignores this way, and the ignore files are in
# the index, from which the tests read them as well.

git init -q
mkdir sub
echo ignored-by-root >.custom-ignore
echo ignored-by-sub >sub/.custom-ignore
echo ignored-by-gitignore >.gitignore
touch ignored-by-root ignored-by-sub ignored-by-gitignore kept
touch sub/ignored-by-root sub/ignored-by-sub sub/ignored-by-gitignore sub/kept
git add .custom-ignore sub/.custom-ignore .gitignore
git ls-files --others --ignored --exclude-per-directory=.custom-ignore >baseline
