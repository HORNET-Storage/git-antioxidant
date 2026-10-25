#!/usr/bin/env bash
set -eu -o pipefail

gix_testtools_require_symlinks

# Git orders a link to a directory as that directory: common-2, common/, common0.
mkdir -p links/target/sub
touch links/target/sub/file links/common-2 links/common0
ln -s target links/common
# Create the dangling link while its target exists for native-strict Git Bash on Windows.
printf "contents" >links/removed
ln -s removed links/dangling
rm links/removed
# A link to the directory that contains it leads back into the walk.
ln -s . links/loop
