#!/usr/bin/env bash
set -eu -o pipefail

# Each case is a repository with an old and a new tree, between which `git diff -M` finds renames. The files are added
# to the index directly, so that symbolic links and submodules don't depend on the file system, and nothing is checked
# out. `trees` lists the ids of both trees, `<percentage>-<limit>.expected` records
# `git diff --name-status -z -M<percentage>%` with `diff.renameLimit=<limit>`, and `<percentage>-<limit>.warnings` what
# git printed to its standard error.
#
# The files consist of lines of 20 bytes, so that the similarity of two files follows from the lines they share: each
# line is a span of its own, and a file of 40 lines that shares 36 of them with another is 90% similar to it.

# start_case <name> creates the repository of a case and enters it. Each case leaves it again at its end.
start_case() {
  git init -q "$1"
  cd "$1"
}

# add <mode> <path> adds what the standard input provides as the file <path> of the index.
add() {
  local id
  id=$(git hash-object -w --stdin)
  git update-index --add --cacheinfo "$1,$id,$2"
}

# lines <family> <first> <last> prints the numbered lines <first> to <last> of a family, each of 20 bytes.
lines() {
  local number
  for ((number = $2; number <= $3; number++)); do
    printf '%-12s %06d\n' "$1" "$number"
  done
}

# crlf ends each line of the standard input with a carriage return and a line feed.
crlf() {
  local line
  while IFS= read -r line; do
    printf '%s\r\n' "$line"
  done
}

# old_tree writes the index as the old tree and empties it for the new one.
old_tree() {
  old=$(git write-tree)
  git read-tree --empty
}

# new_tree writes the index as the new tree.
new_tree() {
  echo "$old $(git write-tree)" > trees
}

# record <percentage> <limit> records what `git diff -M<percentage>%` reports with `diff.renameLimit=<limit>`, in the C
# locale, which keeps the warnings untranslated.
record() {
  local old new
  read -r old new < trees
  LC_ALL=C git -c diff.renameLimit="$2" diff --name-status -z -M"$1%" "$old" "$new" > "$1-$2.expected" 2> "$1-$2.warnings"
}

# add_fillers <directory> <count> adds <count> files with a line of their own to <directory>.
add_fillers() {
  local number
  for number in $(seq -f '%04g' 1 "$2"); do
    lines "fill-$number" 1 1 > ".git/fill-$number"
    echo ".git/fill-$number"
  done |
    git hash-object -w --no-filters --stdin-paths |
    {
      number=0
      while read -r id; do
        number=$((number + 1))
        printf '100644 %s\t%s/%04d\n' "$id" "$1" "$number"
      done
    } |
    git update-index --index-info
  rm .git/fill-*
}

# Identical files: `d/one.txt` takes `b/one.txt` for its file name although `a/other.txt` comes first, which `e/else.txt`
# takes next. A file replaces the tree `dir`, and a regular file becomes executable as it is renamed.
start_case identical
lines shared 1 10 | add 100644 a/other.txt
lines shared 1 10 | add 100644 b/one.txt
lines two 1 10 | add 100644 c/two.txt
lines file 1 10 | add 100644 dir/file.txt
lines run 1 10 | add 100644 run.sh
old_tree
lines shared 1 10 | add 100644 d/one.txt
lines file 1 10 | add 100644 dir
lines shared 1 10 | add 100644 e/else.txt
lines two 1 10 | add 100644 f/two.txt
lines run 1 10 | add 100755 run-it.sh
new_tree
record 50 1000
cd ..

# Symbolic links and submodules: the regular file `file-b` has the blob of the links, but only `link-a` pairs with one.
# The submodules point to the same commit, and `t` becomes a symbolic link with the same blob, a change of its type.
start_case links-and-submodules
submodule=$(git commit-tree "$(git mktree < /dev/null)" -m submodule)
printf target | add 100644 file-b
printf target | add 120000 link-a
git update-index --add --cacheinfo "160000,$submodule,sub-a"
printf abc | add 100644 t
old_tree
printf target | add 120000 link-c
printf target | add 120000 link-d
git update-index --add --cacheinfo "160000,$submodule,sub-b"
printf abc | add 120000 t
new_tree
record 50 1000
cd ..

# Empty files pair with each other like other identical files, preferring the same file name, but are similar to none.
start_case empty
add 100644 a/empty < /dev/null
add 100644 b/zero < /dev/null
add 100644 f/empty-too < /dev/null
old_tree
add 100644 c/empty < /dev/null
add 100644 d/nothing < /dev/null
lines full 1 10 | add 100644 e/full.txt
lines data 1 10 | add 100644 g/data
new_tree
record 50 1000
cd ..

# Similar files: `alpha2.txt` keeps 90% of `alpha.txt`, `beta2.txt` 70% of `beta.txt`, and `delta.txt` nothing of
# `gamma.txt`. They are recorded with minimums at and just above these similarities, and at 0%, which git takes as 50%.
start_case similar
lines alpha 1 40 | add 100644 alpha.txt
lines beta 1 40 | add 100644 beta.txt
lines gamma 1 40 | add 100644 gamma.txt
old_tree
{
  lines alpha 1 36
  lines alpha-new 1 4
} | add 100644 alpha2.txt
{
  lines beta 1 28
  lines beta-new 1 12
} | add 100644 beta2.txt
lines delta 1 40 | add 100644 delta.txt
new_tree
for percentage in 0 50 70 71 90 91 100; do
  record "$percentage" 1000
done
cd ..

# A unique file name: `new/data.txt` keeps 80% of `old/data.txt` and is paired with it by its file name before
# `new/copy.txt`, which keeps 95%, is compared.
start_case unique-file-name
lines data 1 40 | add 100644 old/data.txt
old_tree
{
  lines data 1 32
  lines data-new 1 8
} | add 100644 new/data.txt
{
  lines data 1 38
  lines copy-new 1 2
} | add 100644 new/copy.txt
new_tree
record 50 1000
cd ..

# A unique file name that is too different: `new/info.txt` keeps 60% of `old/info.txt`, less than the 75% that a file
# name needs at a minimum of 50%, so `new/other.txt`, which keeps 95%, takes it.
start_case unique-file-name-too-different
lines info 1 40 | add 100644 old/info.txt
old_tree
{
  lines info 1 24
  lines info-new 1 16
} | add 100644 new/info.txt
{
  lines info 1 38
  lines other-new 1 2
} | add 100644 new/other.txt
new_tree
record 50 1000
cd ..

# File names that become unique once identical files are paired: `a/name.txt` is renamed to `c/other.txt` unchanged,
# which leaves `b/name.txt` the only source named `name.txt`, and `m/list.txt` takes `j/other.txt` unchanged, which
# leaves `n/list.txt` the only destination named `list.txt`. Both pairs by file name keep 80% and come before the more
# similar copies.
start_case unique-after-exact
lines x 1 40 | add 100644 a/name.txt
lines name 1 40 | add 100644 b/name.txt
lines l2 1 40 | add 100644 j/other.txt
lines list 1 40 | add 100644 k/list.txt
old_tree
lines x 1 40 | add 100644 c/other.txt
{
  lines name 1 32
  lines name-new 1 8
} | add 100644 d/name.txt
{
  lines name 1 38
  lines name-copy 1 2
} | add 100644 e/copy.txt
lines l2 1 40 | add 100644 m/list.txt
{
  lines list 1 32
  lines list-new 1 8
} | add 100644 n/list.txt
{
  lines list 1 38
  lines list-copy 1 2
} | add 100644 p/copy.txt
new_tree
record 50 1000
cd ..

# A shared file name: both sources are named `Makefile`, as are both destinations, each of which keeps 90% of the
# source that isn't in the same place, so the estimates pair them.
start_case shared-file-names
lines make-a 1 40 | add 100644 a/Makefile
lines make-b 1 40 | add 100644 b/Makefile
old_tree
{
  lines make-b 1 36
  lines make-c 1 4
} | add 100644 c/Makefile
{
  lines make-a 1 36
  lines make-d 1 4
} | add 100644 d/Makefile
new_tree
record 50 1000
cd ..

# Four candidates per destination: `s<k>.txt` adds <2k> lines of its own to 40 shared lines, and `d<k+1>.txt` keeps all
# but the first of them and adds a line of its own. `d1.txt` has just the shared lines, so it is similar to every
# source, most of all to `s1.txt` to `s4.txt`, which are paired with the other destinations first. `s5.txt` would still
# be 80% similar to `d1.txt`, but it isn't among the four sources that `d1.txt` keeps, so it remains deleted.
start_case four-candidates
for k in 1 2 3 4 5; do
  {
    lines base 1 40
    lines "s$k" 1 $((2 * k))
  } | add 100644 "s$k.txt"
done
old_tree
lines base 1 40 | add 100644 d1.txt
for k in 1 2 3 4; do
  {
    lines base 1 40
    lines "s$k" 2 $((2 * k))
    lines "d$((k + 1))" 1 1
  } | add 100644 "d$((k + 1)).txt"
done
new_tree
record 50 1000
cd ..

# Equally similar sources: `d/file.txt` keeps 60% of both `a/x.txt` and `b/file.txt` and takes the one with its file
# name, while `g/three.txt` keeps 60% of both `e/one.txt` and `f/two.txt` and takes the first of them.
start_case ties
lines first 1 40 | add 100644 a/x.txt
lines first 1 40 | add 100644 b/file.txt
lines second 1 40 | add 100644 e/one.txt
lines second 1 40 | add 100644 f/two.txt
old_tree
{
  lines first 1 24
  lines first-new 1 16
} | add 100644 d/file.txt
{
  lines second 1 24
  lines second-new 1 16
} | add 100644 g/three.txt
new_tree
record 50 1000
cd ..

# The places of the candidates: `dst.txt` keeps 60% of `b.txt` to `e.txt` and nothing of `a.txt`. The four places for
# its candidates are taken by `a.txt` to `d.txt` before `e.txt` replaces the least similar, `a.txt`, so `e.txt` comes
# first among the equally similar sources and is paired.
start_case slot-order
lines unrelated 1 40 | add 100644 a.txt
for name in b c d e; do
  lines copy 1 40 | add 100644 "$name.txt"
done
old_tree
{
  lines copy 1 24
  lines copy-new 1 16
} | add 100644 dst.txt
new_tree
record 50 1000
cd ..

# Binary data: `text-crlf.txt` is `text.txt` with carriage returns, which don't count in text, while the NUL byte of
# `data.bin` makes its carriage returns count. `image-edited.bin` replaces 600 bytes of the middle of `image.bin`, whose
# only line feed ends it, so only the spans of 64 bytes that overlap the replaced bytes differ. Without that line feed,
# the result would depend on the version of git, as git counts the bytes after the last line feed only since 2.44.
start_case binary
lines text 1 40 | add 100644 text.txt
{
  printf '\0'
  lines data 1 40
} | add 100644 data.bin
{
  printf '\0'
  seq 100000 101000 | tr -d '\n'
  echo
} | add 100644 image.bin
old_tree
lines text 1 40 | crlf | add 100644 text-crlf.txt
{
  printf '\0'
  lines data 1 40 | crlf
} | add 100644 data-crlf.bin
{
  printf '\0'
  seq 100000 100500 | tr -d '\n'
  seq 200501 200600 | tr -d '\n'
  seq 100601 101000 | tr -d '\n'
  echo
} | add 100644 image-edited.bin
new_tree
record 50 1000
cd ..

# The rename limit: identical files and a unique file name pair two of the four sources and destinations before the
# limit is checked, which leaves 2 sources and 2 destinations, more than a limit of 1 allows but not a limit of 2.
start_case limit
lines exact 1 40 | add 100644 exact-a
lines name 1 40 | add 100644 same/name.txt
lines p 1 40 | add 100644 p.txt
lines q 1 40 | add 100644 q.txt
old_tree
lines exact 1 40 | add 100644 exact-b
{
  lines name 1 38
  lines name-new 1 2
} | add 100644 other/name.txt
{
  lines p 1 36
  lines p-new 1 4
} | add 100644 p2.txt
{
  lines q 1 36
  lines q-new 1 4
} | add 100644 q2.txt
new_tree
record 50 1
record 50 2
cd ..

# The product of sources and destinations: one source and four destinations are more than a limit of 1 allows, but
# within a limit of 2, as 1 * 4 isn't more than 2 * 2. A limit of 0 means that there is none.
start_case limit-product
lines r 1 40 | add 100644 r.txt
old_tree
{
  lines r 1 36
  lines r-new 1 4
} | add 100644 r1.txt
for name in z2 z3 z4; do
  lines "$name" 1 40 | add 100644 "$name.txt"
done
new_tree
for limit in 0 1 2; do
  record 50 "$limit"
done
cd ..

# Many identical sources: `new/f101` has the blob of `old/f001` to `old/f101`, but only the first 100 of them are
# compared with it, so `old/f101`, the one with its file name, isn't.
start_case many-identical
id=$(lines same 1 40 | git hash-object -w --stdin)
for number in $(seq -f '%03g' 1 101); do
  printf '100644 %s\told/f%s\n' "$id" "$number"
done | git update-index --index-info
old_tree
git update-index --add --cacheinfo "100644,$id,new/f101"
new_tree
record 50 1000
cd ..

# The table of sources: git's table of identical files returns them in the order of their locations, but when it grows
# as the last of them is added, as it does for 205 sources but not for 204, it returns them in reverse. Then
# `d3/same.txt` takes `d2/same.txt` instead of `d1/same.txt`, both of which have its file name.
for sources in 204 205; do
  start_case "table-growth-$sources"
  lines same 1 40 | add 100644 d1/same.txt
  lines same 1 40 | add 100644 d2/same.txt
  add_fillers fill $((sources - 2))
  old_tree
  lines same 1 40 | add 100644 d3/same.txt
  new_tree
  record 50 1000
  cd ..
done

# Modified files are no sources of renames: `copy.txt` has the old blob of `kept.txt`, which still exists.
start_case modifications
lines kept 1 40 | add 100644 kept.txt
old_tree
{
  lines kept 1 39
  lines kept-new 1 1
} | add 100644 kept.txt
lines kept 1 40 | add 100644 copy.txt
new_tree
record 50 1000
cd ..
