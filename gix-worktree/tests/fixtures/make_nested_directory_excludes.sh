#!/usr/bin/env bash
set -eu -o pipefail

git init -q

# Directories that patterns exclude, below directories that patterns exclude. Git reports the pattern of the outermost
# excluded directory for every path below it, and doesn't read the ignore files inside that directory. With precious
# files, which Git doesn't have, `$precious/` declares the directory of that name precious.
mkdir -p tld/sub/deeper tld/sub/precious other/sub keep/sub
cat <<EOF >.gitignore
tld/
sub/
!keep/
EOF

cat <<'EOF' >tld/.gitignore
deeper/
$precious/
EOF

git check-ignore -vn --stdin 2>&1 <<EOF >git-check-ignore.baseline || :
tld/sub/file
tld/sub/deeper/file
tld/sub/precious/file
tld/file
other/sub/file
keep/sub/file
other/file
EOF
