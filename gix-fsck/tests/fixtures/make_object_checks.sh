#!/usr/bin/env bash
set -eu -o pipefail

# Objects with the problems that Git's object checks find, and Git's verdicts on them, which depend on the platform
# Git runs on and on its version (the tests expect Git 2.55 or later):
#
# * `objects.git` holds the objects that Git can parse, listed in `objects.list` as `<id> <type> <case>` lines.
#   `fsck.err` and `fsck-strict.err` are what `git fsck` and `git fsck --strict` print about them.
# * `hash-object/<case>.commit` and `hash-object/<case>.tag` are commits and tags, among them some that Git can't
#   parse and therefore doesn't check in a repository, which `git hash-object` checks before it writes them.
#   `<case>.err` and `<case>.exit` are what it printed and its exit code.
# * `index-pack/<case>/objects` lists objects of `objects.git` that `git index-pack --fsck-objects` checks in a pack
#   in this order, with the options in `options` and, if `threshold` exists, blobs larger than its number of bytes
#   read as large blobs. `stdout`, `stderr` and `exit` are what it printed and its exit code.

git init -q --bare objects.git
case $(git -C objects.git rev-parse --show-object-format) in
  sha1) hex_len=40 ;;
  sha256) hex_len=64 ;;
  *) echo "unknown object format" >&2; exit 1 ;;
esac
mkdir hash-object index-pack
: > objects.list

# A made-up object ID that repeats the character $1. Git takes an object ID for the type it reads it as first, so each
# digit stands for objects of one type: 1 for commits, 2 and 3 for blobs that trees name `.gitmodules` and
# `.gitattributes`, and 0 for the null ID, which a tree names as a blob.
fake_id() {
  printf "%${hex_len}s" "" | tr ' ' "$1"
}

# Write the object of type $2 that standard input holds into `objects.git` as the object of case $1, and print its ID.
object() {
  local id
  id=$(git -C objects.git hash-object --literally -w -t "$2" --stdin)
  echo "$id $2 $1" >> objects.list
  echo "$id"
}

# The ID of the object of case $1.
case_id() {
  local id
  id=$(awk -v name="$1" '$3 == name { print $1; exit }' objects.list)
  if [ -z "$id" ]; then
    echo "no object for the case $1" >&2
    return 1
  fi
  echo "$id"
}

# The binary form of the hexadecimal object ID $1.
raw_id() {
  # shellcheck disable=SC2059
  printf "$(printf '%s' "$1" | sed 's/../\\x&/g')"
}

# A tree entry with the mode $1, the name $2 with the escapes of `printf %b`, and the object ID $3.
entry() {
  printf '%s %b\0' "$1" "$2"
  raw_id "$3"
}

# Write the tree of case $1, whose entries follow as triples of mode, name and object ID, in that order, and print its ID.
tree_case() {
  local name=$1
  shift
  while [ $# -gt 0 ]; do
    entry "$1" "$2" "$3"
    shift 3
  done | object "$name" tree
}

# Write the commit or tag of case $1 and type $2, which the `printf` format $4 and the arguments after it describe, for
# `git hash-object`, and into `objects.git` if $3 is `parses`, which means that Git can parse it.
header_case() {
  local name=$1 type=$2 parses=$3
  shift 3
  # shellcheck disable=SC2059
  printf "$@" > "hash-object/$name.$type"
  if [ "$parses" = parses ]; then
    object "$name" "$type" < "hash-object/$name.$type" > /dev/null
  fi
}

# Write a blob of case $1 with the content $3 and a tree of case $1-tree that names it $2, both with the escapes of
# `printf %b`.
named_blob() {
  local blob
  blob=$(printf '%b' "$3" | object "$1" blob)
  tree_case "$1-tree" 100644 "$2" "$blob" > /dev/null
}

# $1 times the character $2.
repeat() {
  printf "%${1}s" "" | tr ' ' "$2"
}

blob=$(printf 'a\n' | object blob blob)
tree=$(tree_case tree 100644 a "$blob")
missing_commit=$(fake_id 1)
missing_gitmodules=$(fake_id 2)
missing_gitattributes=$(fake_id 3)
null_id=$(fake_id 0)
not_hex=$(fake_id g)
ident='A U Thor <author@example.com> 1112911993 -0700'

# Commits.
commit_case() {
  header_case "commit-$1" commit "${@:2}"
}
commit_case valid parses 'tree %s\nauthor %s\ncommitter %s\n\nmessage\n' "$tree" "$ident" "$ident"
commit_case with-parents parses 'tree %s\nparent %s\nparent %s\nauthor %s\ncommitter %s\n\nmessage\n' \
  "$tree" "$missing_commit" "$missing_commit" "$ident" "$ident"
commit_case uppercase-ids parses 'tree %s\nauthor %s\ncommitter %s\n\nmessage\n' \
  "$(printf '%s' "$tree" | tr a-f A-F)" "$ident" "$ident"
commit_case without-message parses 'tree %s\nauthor %s\ncommitter %s\n' "$tree" "$ident" "$ident"
commit_case extra-headers parses 'tree %s\nauthor %s\ncommitter %s\nencoding ISO-8859-1\nx-header a\n continued\n\nmessage\n' \
  "$tree" "$ident" "$ident"
commit_case nul-in-header parses 'tree %s\nauthor %s\ncommitter %s\nx-header a\0b\n\nmessage\n' "$tree" "$ident" "$ident"
commit_case unterminated-header parses 'tree %s\nauthor %s\ncommitter %s' "$tree" "$ident" "$ident"
commit_case nul-in-message parses 'tree %s\nauthor %s\ncommitter %s\n\nmes\0sage\n' "$tree" "$ident" "$ident"
commit_case missing-author parses 'tree %s\ncommitter %s\n\nmessage\n' "$tree" "$ident"
commit_case author-after-committer parses 'tree %s\ncommitter %s\nauthor %s\n\nmessage\n' "$tree" "$ident" "$ident"
commit_case multiple-authors parses 'tree %s\nauthor %s\nauthor %s\ncommitter %s\n\nmessage\n' \
  "$tree" "$ident" "$ident" "$ident"
commit_case missing-committer parses 'tree %s\nauthor %s\n\nmessage\n' "$tree" "$ident"
commit_case bad-committer parses 'tree %s\nauthor %s\ncommitter %s\n\nmessage\n' "$tree" "$ident" 'C O Mitter 1112911993 -0700'
# Commits that Git can't parse.
commit_case missing-tree fails 'author %s\ncommitter %s\n\nmessage\n' "$ident" "$ident"
commit_case only-tree fails 'tree %s\n' "$tree"
commit_case bad-tree-id fails 'tree %s\nauthor %s\ncommitter %s\n\nmessage\n' "$not_hex" "$ident" "$ident"
commit_case short-tree-id fails 'tree %s\nauthor %s\ncommitter %s\n\nmessage\n' "${tree%?}" "$ident" "$ident"
commit_case tree-id-followed-by-text fails 'tree %sx\nauthor %s\ncommitter %s\n\nmessage\n' "$tree" "$ident" "$ident"
commit_case bad-parent-id fails 'tree %s\nparent %s\nauthor %s\ncommitter %s\n\nmessage\n' \
  "$tree" "$not_hex" "$ident" "$ident"
commit_case parent-id-followed-by-text fails 'tree %s\nparent %sx\nauthor %s\ncommitter %s\n\nmessage\n' \
  "$tree" "$missing_commit" "$ident" "$ident"

# Author lines, each with one problem or none.
author_case() {
  commit_case "author-$1" parses 'tree %s\nauthor %s\ncommitter %s\n\nmessage\n' "$tree" "$2" "$ident"
}
author_case missing-name '<author@example.com> 1112911993 -0700'
author_case missing-email 'A U Thor 1112911993 -0700'
author_case bad-name 'A U >Thor <author@example.com> 1112911993 -0700'
author_case missing-space-before-email 'A U Thor<author@example.com> 1112911993 -0700'
author_case unclosed-email 'A U Thor <author@example.com 1112911993 -0700'
author_case email-with-lt 'A U Thor <author<@example.com> 1112911993 -0700'
author_case missing-space-before-date 'A U Thor <author@example.com>1112911993 -0700'
author_case blanks-before-date "A U Thor <author@example.com> $(printf '\t') 1112911993 -0700"
author_case missing-date 'A U Thor <author@example.com> -0700'
author_case date-followed-by-text 'A U Thor <author@example.com> 1112911993x -0700'
author_case zero-date 'A U Thor <author@example.com> 0 +0000'
author_case zero-padded-date 'A U Thor <author@example.com> 01112911993 -0700'
# Dates from 2^31 to 2^63 - 1 are left out, as Git with a 32-bit `time_t` refuses them.
author_case date-2-pow-63 'A U Thor <author@example.com> 9223372036854775808 -0700'
author_case largest-date 'A U Thor <author@example.com> 18446744073709551615 -0700'
author_case date-2-pow-64 'A U Thor <author@example.com> 18446744073709551616 -0700'
author_case date-of-23-digits 'A U Thor <author@example.com> 12345678901234567890123 -0700'
author_case date-of-24-digits 'A U Thor <author@example.com> 123456789012345678901234 -0700'
author_case short-time-zone 'A U Thor <author@example.com> 1112911993 -070'
author_case long-time-zone 'A U Thor <author@example.com> 1112911993 -07000'
author_case unsigned-time-zone 'A U Thor <author@example.com> 1112911993 0700'
author_case time-zone-with-letter 'A U Thor <author@example.com> 1112911993 -07a0'
author_case time-zone-followed-by-text 'A U Thor <author@example.com> 1112911993 -0700x'
author_case two-blanks-before-time-zone 'A U Thor <author@example.com> 1112911993  -0700'
author_case missing-time-zone 'A U Thor <author@example.com> 1112911993'
author_case empty-time-zone 'A U Thor <author@example.com> 1112911993 '
author_case empty-name ' <author@example.com> 1112911993 -0700'
author_case empty-email 'A U Thor <> 1112911993 -0700'

# Tags.
tag_case() {
  header_case "tag-$1" tag "${@:2}"
}
tag_case valid parses 'object %s\ntype commit\ntag v1\ntagger %s\n\nmessage\n' "$missing_commit" "$ident"
tag_case of-tree parses 'object %s\ntype tree\ntag v1\ntagger %s\n\nmessage\n' "$tree" "$ident"
tag_case of-blob parses 'object %s\ntype blob\ntag v1\ntagger %s\n\nmessage\n' "$blob" "$ident"
tag_case of-tag parses 'object %s\ntype tag\ntag v2\ntagger %s\n\nmessage\n' "$(case_id tag-valid)" "$ident"
tag_case without-message parses 'object %s\ntype commit\ntag v1\ntagger %s\n' "$missing_commit" "$ident"
tag_case nul-in-header parses 'object %s\ntype commit\ntag v1\ntagger %s\0\n\nmessage\n' "$missing_commit" "$ident"
tag_case unterminated-header parses 'object %s\ntype commit\ntag v1\ntagger %s' "$missing_commit" "$ident"
tag_case missing-tagger parses 'object %s\ntype commit\ntag v1\n\nmessage\n' "$missing_commit"
tag_case bad-tagger parses 'object %s\ntype commit\ntag v1\ntagger %s\n\nmessage\n' \
  "$missing_commit" 'A U Thor <author@example.com> 1112911993x -0700'
tag_case signed parses 'object %s\ntype commit\ntag v1\ntagger %s\ngpgsig -----BEGIN PGP SIGNATURE-----\n abc\n -----END PGP SIGNATURE-----\n\nmessage\n' \
  "$missing_commit" "$ident"
tag_case signed-sha256 parses 'object %s\ntype commit\ntag v1\ntagger %s\ngpgsig-sha256 -----BEGIN PGP SIGNATURE-----\n abc\n -----END PGP SIGNATURE-----\n\nmessage\n' \
  "$missing_commit" "$ident"
tag_case extra-header parses 'object %s\ntype commit\ntag v1\ntagger %s\nx-header a\n\nmessage\n' "$missing_commit" "$ident"
tag_case signed-with-extra-header parses 'object %s\ntype commit\ntag v1\ntagger %s\ngpgsig a\n b\nx-header a\n\nmessage\n' \
  "$missing_commit" "$ident"
tag_case bad-tagger-and-extra-header parses 'object %s\ntype commit\ntag v1\ntagger %s\nx-header a\n\nmessage\n' \
  "$missing_commit" 'A U Thor <author@example.com> 1112911993x -0700'
tag_case missing-tagger-and-extra-header parses 'object %s\ntype commit\ntag v1\nx-header a\n\nmessage\n' "$missing_commit"
# Tags that Git can't parse.
tag_case missing-object fails 'type commit\ntag v1\ntagger %s\n\nmessage\n' "$ident"
tag_case only-object fails 'object %s\n' "$missing_commit"
tag_case bad-object-id fails 'object %s\ntype commit\ntag v1\ntagger %s\n\nmessage\n' "$not_hex" "$ident"
tag_case object-id-followed-by-text fails 'object %sx\ntype commit\ntag v1\ntagger %s\n\nmessage\n' \
  "$missing_commit" "$ident"
tag_case missing-type fails 'object %s\ntag v1\ntagger %s\n\nmessage\n' "$missing_commit" "$ident"
tag_case bad-type fails 'object %s\ntype Commit\ntag v1\ntagger %s\n\nmessage\n' "$missing_commit" "$ident"
tag_case missing-tag-entry fails 'object %s\ntype commit\ntagger %s\n\nmessage\n' "$missing_commit" "$ident"

# Tag names, which must be valid below `refs/tags/`.
tag_name_case() {
  tag_case "name-$1" parses 'object %s\ntype commit\ntag %b\ntagger %s\n\nmessage\n' "$missing_commit" "$2" "$ident"
}
tag_name_case leading-dash '-v1'
tag_name_case nested 'a/b'
tag_name_case head 'HEAD'
tag_name_case at '@'
tag_name_case high-byte '\xff'
tag_name_case empty ''
tag_name_case double-dot 'v..1'
tag_name_case lock 'v1.lock'
tag_name_case component-lock 'a.lock/b'
tag_name_case blank 'v 1'
tag_name_case tilde 'v~1'
tag_name_case caret 'v^1'
tag_name_case colon 'v:1'
tag_name_case question-mark 'v?1'
tag_name_case asterisk 'v*1'
tag_name_case bracket 'v[1'
tag_name_case backslash 'v\\1'
tag_name_case at-brace 'v@{1'
tag_name_case leading-dot '.v1'
tag_name_case component-with-leading-dot 'a/.b'
tag_name_case trailing-dot 'v1.'
tag_name_case trailing-slash 'v1/'
tag_name_case double-slash 'a//b'
tag_name_case delete-character '\x7f'
tag_name_case control-character '\x01'

# Trees.
tree_case tree-modes 100644 a "$blob" 100755 b "$blob" 120000 c "$blob" 40000 d "$tree" 160000 e "$missing_commit" > /dev/null
printf '' | object tree-empty tree > /dev/null
tree_case tree-zero-padded-tree-mode 040000 d "$tree" > /dev/null
tree_case tree-zero-padded-file-mode 0100644 a "$blob" > /dev/null
tree_case tree-group-writable 100664 a "$blob" > /dev/null
tree_case tree-bad-mode 100600 a "$blob" > /dev/null
tree_case tree-unknown-type 170000 a "$blob" > /dev/null
tree_case tree-null-id 100644 a "$null_id" > /dev/null
tree_case tree-full-path 100644 a/b "$blob" > /dev/null
tree_case tree-dot 100644 . "$blob" > /dev/null
tree_case tree-dotdot 100644 .. "$blob" > /dev/null
tree_case tree-many-findings 100644 . "$blob" 100644 a/b "$blob" 100600 b "$blob" 100644 c "$null_id" > /dev/null
tree_case tree-not-sorted 100644 b "$blob" 100644 a "$blob" > /dev/null
tree_case tree-duplicate 100644 a "$blob" 100644 a "$blob" > /dev/null
tree_case tree-duplicate-file-and-tree 100644 a "$blob" 40000 a "$tree" > /dev/null
tree_case tree-duplicate-file-and-later-tree 100644 foo "$blob" 100644 foo.bar "$blob" 40000 foo "$tree" > /dev/null
tree_case tree-tree-after-file-with-suffix 100644 foo.bar "$blob" 40000 foo "$tree" > /dev/null
tree_case tree-tree-before-file-with-suffix 40000 foo "$tree" 100644 foo.bar "$blob" > /dev/null
for len in 9 16 17 1024 1025 4096 4097; do
  tree_case "tree-name-of-$len" 100644 "$(repeat "$len" n)" "$blob" > /dev/null
done

# Names that file systems may take for `.git`.
dotgit_case() {
  tree_case "tree-dotgit-$1" 100644 "$2" "$blob" > /dev/null
}
dotgit_case plain .git
dotgit_case upper .GIT
dotgit_case short git~1
dotgit_case short-upper GIT~1
dotgit_case other-short git~2
dotgit_case trailing-dot .git.
dotgit_case trailing-blank '.git\x20'
dotgit_case trailing-dot-and-blanks '.git. .'
dotgit_case stream '.git::$INDEX_ALLOCATION'
dotgit_case named-stream '.git:a'
dotgit_case longer .gitx
dotgit_case zero-width-joiner '.g\xe2\x80\x8dit'
dotgit_case zero-width-joiner-at-end '.git\xe2\x80\x8d'
dotgit_case byte-order-mark-before '\xef\xbb\xbf.git'
dotgit_case combining-diaeresis '.gi\xcc\x88t'
dotgit_case invalid-utf8-at-end '.git\xff'
dotgit_case invalid-utf8-inside '.g\xffit'
dotgit_case after-backslash 'a\\.git'
dotgit_case short-after-backslash 'a\\git~1'
dotgit_case before-backslash '.git\\a'
dotgit_case zero-width-non-joiner-before-backslash '.g\xe2\x80\x8cit\\a'

# Trees that Git can't parse.
{ printf '100644 \0'; raw_id "$blob"; } | object tree-empty-name tree > /dev/null
{ printf ' 100644 a\0'; raw_id "$blob"; } | object tree-mode-after-blank tree > /dev/null
{ printf '100648 a\0'; raw_id "$blob"; } | object tree-mode-with-8 tree > /dev/null
{ printf 'a\0'; raw_id "$blob"; } | object tree-missing-mode tree > /dev/null
{ printf '100644a\0'; raw_id "$blob"; } | object tree-missing-blank tree > /dev/null
{ printf '100644 a'; raw_id "$blob"; } | object tree-missing-nul tree > /dev/null
{ printf '100644 a\0'; raw_id "$blob" | head -c 10; } | object tree-truncated-id tree > /dev/null
{ entry 100664 a "$blob"; printf '100644 b'; } | object tree-bad-entry-after-group-writable tree > /dev/null
{ entry 100644 . "$blob"; printf 'x'; } | object tree-bad-entry-after-dot tree > /dev/null

# `.gitmodules` blobs, each named `.gitmodules` by a tree of its own.
gitmodules() {
  named_blob "gitmodules-$1" .gitmodules "$2"
}
gitmodules valid '[submodule "a"]\n\tpath = a\n\turl = https://example.com/a\n'
gitmodules name-dotdot '[submodule "../a"]\n\tpath = a\n\turl = https://example.com/a\n'
gitmodules name-dotdot-inside '[submodule "a/../b"]\n\tpath = a\n'
gitmodules name-dotdot-at-end '[submodule "a/.."]\n\tpath = a\n'
gitmodules name-backslash-dotdot '[submodule "a\\\\..\\\\b"]\n\tpath = a\n'
gitmodules name-with-dots '[submodule "a..b"]\n\tpath = a\n'
gitmodules name-empty '[submodule ""]\n\tpath = a\n'
gitmodules url-option '[submodule "a"]\n\turl = -upload-pack=x\n'
gitmodules url-relative '[submodule "a"]\n\turl = ../a\n'
gitmodules url-dot-relative '[submodule "a"]\n\turl = ./a\n'
gitmodules url-relative-newline '[submodule "a"]\n\turl = ./%0a\n'
gitmodules url-dotdot-colon '[submodule "a"]\n\turl = ../:example.com/a\n'
gitmodules url-dotdot-slash '[submodule "a"]\n\turl = .././/example.com/a\n'
gitmodules url-dotdot-backslash-colon '[submodule "a"]\n\turl = ..\\\\:example.com/a\n'
gitmodules url-git '[submodule "a"]\n\turl = git://example.com/a\n'
gitmodules url-git-newline '[submodule "a"]\n\turl = git://example.com/%0a\n'
gitmodules url-https-newline '[submodule "a"]\n\turl = https://example.com/%0a\n'
gitmodules url-https-bad-escape '[submodule "a"]\n\turl = https://example.com/%zz\n'
gitmodules url-https-port-65536 '[submodule "a"]\n\turl = https://example.com:65536/a\n'
gitmodules url-https-port-0 '[submodule "a"]\n\turl = https://example.com:0/a\n'
gitmodules url-http-padded-port '[submodule "a"]\n\turl = http://example.com:00080/a\n'
gitmodules url-https-without-host '[submodule "a"]\n\turl = https:///a\n'
gitmodules url-https-transport-prefix '[submodule "a"]\n\turl = https::https://example.com/a\n'
gitmodules url-http-transport-prefix-newline '[submodule "a"]\n\turl = http::http://example.com/%0a\n'
gitmodules url-dot-segments '[submodule "a"]\n\turl = https://user@example.com:8080/a/../b/./c?q=%41\n'
gitmodules url-dotdot-beyond-root '[submodule "a"]\n\turl = https://example.com/../../a\n'
gitmodules url-userinfo-newline '[submodule "a"]\n\turl = https://us%0aer@example.com/a\n'
gitmodules url-ssh '[submodule "a"]\n\turl = ssh://example.com/a\n'
gitmodules url-scp '[submodule "a"]\n\turl = example.com:a\n'
gitmodules url-ftp-blank-in-host '[submodule "a"]\n\turl = ftp://exa mple.com/a\n'
gitmodules url-ipv6 '[submodule "a"]\n\turl = https://[::1]:8080/a\n'
gitmodules url-quoted '[submodule "a"]\n\turl = "-a"\n'
gitmodules url-comment '[submodule "a"]\n\turl = -a ; comment\n'
gitmodules url-long "[submodule \"a\"]\n\turl = -$(repeat 5000 a)\n"
gitmodules path-option '[submodule "a"]\n\tpath = -a\n'
gitmodules path-with-tab '[submodule "a"]\n\tpath = "-a\\tb"\n'
gitmodules update-command '[submodule "a"]\n\tupdate = !rm -rf a\n'
gitmodules update-none '[submodule "a"]\n\tupdate = none\n'
gitmodules update-merge '[submodule "a"]\n\tupdate = merge\n'
gitmodules without-subsection '[submodule]\n\turl = -a\n'
gitmodules other-section '[core]\n\turl = -a\n'
gitmodules old-syntax '[submodule.Foo]\n\turl = -a\n'
gitmodules key-case '[submodule "a"]\n\tURL = -a\n'
gitmodules section-case '[SubModule "a"]\n\turl = -a\n'
gitmodules variable-without-value '[submodule "../a"]\n\tpath\n'
gitmodules nul-in-subsection '[submodule "a\0b"]\n\turl = -a\n'
gitmodules nul-in-value '[submodule "a"]\n\turl = -a\0b\n'
gitmodules parse-error '[submodule "a"\n'
gitmodules parse-error-after-variable '[submodule "../a"]\n\tpath = a\n[broken\n'
gitmodules byte-order-mark '\xef\xbb\xbf[submodule "../a"]\n\tpath = a\n'
gitmodules byte-ff-before-error '[submodule "../a"]\n\tpath = a\n\xff[broken\n'
gitmodules byte-ff-in-value '[submodule "a"]\n\turl = -a\xffb\n'
gitmodules crlf '[submodule "../a"]\r\n\tpath = a\r\n'
gitmodules continued-line '[submodule "a"]\n\turl = -a\\\nb\n'
gitmodules unknown-escape '[submodule "../a"]\n\turl = a\\qb\n'
gitmodules unclosed-quote '[submodule "a"]\n\turl = "-a\n'

# Names that file systems may take for `.gitmodules`, each naming a blob of its own.
gitmodules_named() {
  named_blob "gitmodules-named-$1" "$2" "[submodule \"../$1\"]\\n\\tpath = a\\n"
}
gitmodules_named upper .GITMODULES
gitmodules_named trailing-blank '.gitmodules '
gitmodules_named trailing-dots .gitmodules..
gitmodules_named stream '.gitmodules:$DATA'
gitmodules_named short gitmod~1
gitmodules_named short-upper GITMOD~4
gitmodules_named short-5 gitmod~5
gitmodules_named hashed-short gi7eba~9
gitmodules_named hashed-short-upper GI7EBA~1
gitmodules_named zero-width-non-joiner '.gitmodul\xe2\x80\x8ces'
gitmodules_named after-backslash 'x\\.gitmodules'
gitmodules_named before-backslash '.gitmodules\\a'
gitmodules_named longer .gitmodulesx
gitmodules_named without-dot gitmodules

# `.gitattributes` blobs, each named by a tree of its own.
long_line=$(repeat 2048 x)
named_blob gitattributes-long-line .gitattributes "* text\n$long_line\n"
named_blob gitattributes-longest-line .gitattributes "* text\n${long_line%?}\n"
named_blob gitattributes-long-line-after-nul .gitattributes "* text\0\n$long_line\n"
named_blob gitattributes-long-line-with-cr .gitattributes "* text\r\n${long_line%?}\r\n"
named_blob gitattributes-upper .GITATTRIBUTES "# upper\n$long_line\n"
named_blob gitattributes-short gitatt~1 "# short\n$long_line\n"
named_blob gitattributes-hashed-short gi7d29~1 "# hashed short\n$long_line\n"
named_blob gitattributes-after-backslash 'x\\.gitattributes' "# after backslash\n$long_line\n"

# Symbolic links and files that trees name like files Git reads.
tree_case gitmodules-symlink 120000 .gitmodules "$blob" > /dev/null
tree_case gitmodules-symlink-after-backslash 120000 'x\\.gitmodules' "$blob" > /dev/null
tree_case gitattributes-symlink 120000 .gitattributes "$blob" > /dev/null
tree_case gitignore-symlink 120000 .gitignore "$blob" > /dev/null
tree_case gitignore-symlink-upper 120000 .GITIGNORE "$blob" > /dev/null
tree_case gitignore-symlink-short 120000 gitign~1 "$blob" > /dev/null
tree_case gitignore-symlink-hashed-short 120000 gi250a~1 "$blob" > /dev/null
tree_case gitignore-file 100644 .gitignore "$blob" > /dev/null
tree_case mailmap-symlink 120000 .mailmap "$blob" > /dev/null
tree_case mailmap-symlink-short 120000 mailma~1 "$blob" > /dev/null
tree_case mailmap-symlink-hashed-short 120000 maba30~1 "$blob" > /dev/null
tree_case mailmap-file 100644 .mailmap "$blob" > /dev/null

# Blobs named `.gitmodules` or `.gitattributes` that are missing or aren't blobs, and a blob named both.
tree_case gitmodules-missing 100644 .gitmodules "$missing_gitmodules" > /dev/null
tree_case gitattributes-missing 100644 .gitattributes "$missing_gitattributes" > /dev/null
tree_case gitmodules-tree 40000 .gitmodules "$(tree_case gitmodules-target-tree 100644 b "$blob")" > /dev/null
commit_case gitattributes-target parses 'tree %s\nauthor %s\ncommitter %s\n\ngitattributes\n' "$tree" "$ident" "$ident"
tree_case gitattributes-commit 160000 .gitattributes "$(case_id commit-gitattributes-target)" > /dev/null
both=$(printf '#%s\n[submodule "../both"]\n\tpath = a\n' "$long_line" | object gitattributes-and-gitmodules blob)
tree_case gitattributes-and-gitmodules-tree 100644 .gitattributes "$both" 100644 .gitmodules "$both" > /dev/null

# Blobs for packs read with `core.bigFileThreshold` at 16 bytes.
named_blob large-gitmodules .gitmodules '[submodule "../large"]\n\tpath = a\n'
named_blob large-gitattributes .gitattributes '* text\n*.bin binary\n'
printf 'a blob larger than 16 bytes\n' | object large-blob blob > /dev/null
named_blob gitmodules-of-16-bytes .gitmodules '[submodule "a"]\n'
named_blob gitmodules-of-17-bytes .gitmodules '[submodule "ab"]\n'

# Check a pack of the objects of the cases after $3, in that order, with `git index-pack --fsck-objects` as the case
# $1, with the options $3 and with blobs larger than $2 bytes read as large blobs unless $2 is `-`.
pack_case() {
  local threshold=$2 options=$3 dir=index-pack/$1 status=0
  # Git for Windows needs long paths for the name of the pack with SHA-256 object IDs in the fixture's directory.
  local config=(-c core.longPaths=true)
  shift 3
  mkdir -p "$dir"
  for object_case in "$@"; do
    case_id "$object_case"
  done > "$dir/objects"
  printf '%s' "$options" > "$dir/options"
  if [ "$threshold" != - ]; then
    printf '%s' "$threshold" > "$dir/threshold"
    config+=(-c "core.bigFileThreshold=$threshold")
  fi
  git -C objects.git pack-objects -q --stdout --window=0 < "$dir/objects" > "$dir/pack"
  git init -q --bare "$dir/repo"
  git -C "$dir/repo" "${config[@]}" index-pack --stdin "--fsck-objects${options:+=$options}" \
    < "$dir/pack" > "$dir/stdout" 2> "$dir/stderr" || status=$?
  echo "$status" > "$dir/exit"
  rm -rf "$dir/repo" "$dir/pack"
}

# Write the skip list $2 for the case $1, which its options name as `../skiplist`.
skip_list() {
  mkdir -p "index-pack/$1"
  printf '%b' "$2" > "index-pack/$1/skiplist"
}

pack_case valid - '' blob tree commit-valid tag-valid gitmodules-valid-tree gitmodules-valid
pack_case warning-in-strict-mode - '' tree-dot
pack_case info - '' tree-bad-mode tag-missing-tagger
pack_case group-writable - '' tree-group-writable
pack_case stops-at-first-failure - '' blob tree-dot tree-dotdot
pack_case many-findings - '' tree-many-findings
pack_case commit - '' commit-missing-author
pack_case tagger-failure-passes-with-extra-header - '' tag-bad-tagger-and-extra-header
pack_case extra-header-as-error - 'extraHeaderEntry=error' tag-extra-header
pack_case ignored - 'hasDot=ignore' tree-dot
pack_case warned - 'hasDot=warn' tree-dot
pack_case info-as-error - 'badFilemode=error' tree-bad-mode
pack_case names-in-any-case - 'HASDOT=ignore' tree-dot
pack_case colon-after-name - 'hasDot:ignore' tree-dot
pack_case several-options - 'hasDot=ignore,hasDotdot=ignore|nullSha1=ignore fullPathname=ignore,,badFilemode=ignore' \
  tree-many-findings tree-dot tree-dotdot
pack_case fatal-as-error - 'nulInHeader=error' commit-nul-in-header
pack_case name-limit - 'largePathname=warn:0x10' tree-name-of-16 tree-name-of-17
pack_case name-limit-in-kib - 'largePathname=error:1k' tree-name-of-1024 tree-name-of-1025
pack_case name-limit-negative - 'largePathname=warn:-1' tree
pack_case name-limit-octal-after-tab - "largePathname=warn:$(printf '\t')010" tree-name-of-9
pack_case default-name-limit - '' tree-name-of-4096 tree-name-of-4097
pack_case unknown-name - 'fooBar=error' tree-dot
pack_case unknown-severity - 'badTreeSha1=fatal' tree-dot
pack_case severity-in-upper-case - 'missingTaggerEntry=IGNORE' tree-dot
pack_case demoted-fatal - 'nulInHeader=warn' tree-dot
pack_case unparsable-name-limit - 'largePathname=warn:12x' tree-dot
pack_case name-limit-overflow - 'largePathname=warn:9223372036854775807k' tree-dot
pack_case limit-for-other-name - 'hasDot=warn:5' tree-dot
pack_case missing-equals - 'hasDot' tree-dot
pack_case skiplist-without-path - 'skipList' tree-dot
skip_list skipped-tree "# skipped objects\n\n  $(case_id tree-dot)  # with a comment\r\n"
pack_case skipped-tree - 'skiplist=../skiplist' tree-dot
skip_list skipped-commit "$(case_id commit-nul-in-header)\n"
pack_case skipped-commit - 'skiplist=../skiplist' commit-nul-in-header
skip_list skipped-tree-still-queues "$(case_id gitmodules-name-dotdot-tree)\n"
pack_case skipped-tree-still-queues - 'skiplist=../skiplist' gitmodules-name-dotdot-tree gitmodules-name-dotdot
skip_list skipped-gitmodules "$(case_id gitmodules-name-dotdot)\n"
pack_case skipped-gitmodules - 'skiplist=../skiplist' gitmodules-name-dotdot-tree gitmodules-name-dotdot
skip_list skiplist-in-upper-case "$(case_id tree-dot | tr a-f A-F)\n"
pack_case skiplist-in-upper-case - 'skiplist=../skiplist' tree-dot
skip_list skiplist-with-nul-after-id "$(case_id tree-dot)\0junk\n"
pack_case skiplist-with-nul-after-id - 'skiplist=../skiplist' tree-dot
skip_list skiplist-with-invalid-line 'xyz\n'
pack_case skiplist-with-invalid-line - 'skiplist=../skiplist' tree-dot
skip_list skiplist-with-nul-in-invalid-line 'xy\0z\n'
pack_case skiplist-with-nul-in-invalid-line - 'skiplist=../skiplist' tree-dot
pack_case gitmodules-after-tree - '' gitmodules-name-dotdot-tree gitmodules-name-dotdot
pack_case gitmodules-before-tree - '' gitmodules-name-dotdot gitmodules-name-dotdot-tree
pack_case gitmodules-parse-error - '' gitmodules-parse-error-tree gitmodules-parse-error
pack_case gitmodules-long-url - '' gitmodules-url-long-tree gitmodules-url-long
pack_case gitmodules-missing - '' gitmodules-missing
pack_case gitattributes-missing - '' gitattributes-missing
pack_case gitmodules-tree - '' gitmodules-tree gitmodules-target-tree
pack_case gitmodules-tree-not-in-pack - '' gitmodules-tree
pack_case gitattributes-commit - '' gitattributes-commit commit-gitattributes-target
pack_case gitattributes-long-line - '' gitattributes-long-line-tree gitattributes-long-line
pack_case gitattributes-and-gitmodules - '' gitattributes-and-gitmodules-tree gitattributes-and-gitmodules
pack_case large-gitmodules 16 '' large-gitmodules-tree large-gitmodules
pack_case large-gitmodules-before-tree 16 '' large-gitmodules large-gitmodules-tree
pack_case large-gitattributes 16 '' large-gitattributes-tree large-gitattributes
pack_case large-blob 16 '' large-blob
pack_case large-gitmodules-warned-and-gitattributes 16 'gitmodulesLarge=warn' \
  gitattributes-and-gitmodules-tree gitattributes-and-gitmodules
pack_case gitmodules-of-threshold-size 16 '' gitmodules-of-16-bytes-tree gitmodules-of-16-bytes
pack_case gitmodules-above-threshold-size 16 '' gitmodules-of-17-bytes-tree gitmodules-of-17-bytes

git -C objects.git fsck --no-dangling --no-progress > /dev/null 2> fsck.err || true
git -C objects.git fsck --strict --no-dangling --no-progress > /dev/null 2> fsck-strict.err || true

for file in hash-object/*.commit hash-object/*.tag; do
  status=0
  git -C objects.git hash-object -t "${file##*.}" --stdin < "$file" > /dev/null 2> "${file%.*}.err" || status=$?
  echo "$status" > "${file%.*}.exit"
done
