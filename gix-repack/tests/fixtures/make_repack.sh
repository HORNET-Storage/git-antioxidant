#!/usr/bin/env bash
set -eu -o pipefail
export LC_ALL=C

# Each case is a repository whose packs and loose objects the tests repack. For each case, the fixture records what
# `git repack -d -l --geometric=2`, the repack of `git maintenance`, does with a copy of it:
#
# * `<case>.packs-before` and `<case>.packs-after`: the names of the pack indices before and after,
# * `<case>.new-objects`: the objects of the packs that git wrote,
# * `<case>.loose-after`: the loose objects that remained,
# * `<case>.midx-after`: the files of multi-pack-indices that remained, as paths in the pack directory.
#
# Every list is sorted. `decisions` records whether `git maintenance run --auto` repacked copies of some cases.

# init <name> creates the repository <name> without the files of git's template directory.
init() {
  git init -q --template= "$1"
}

# base <count> commits the files `file1` to `file<count>`: a commit, its tree and <count> blobs.
base() {
  local n
  for n in $(seq 1 "$1"); do
    echo "file $n" > "file$n"
  done
  git add -A
  git commit -q -m base
}

# change <file> <content> commits <content> as the new content of <file>: a commit, its tree and a blob.
change() {
  echo "$2" > "$1"
  git commit -q -am "$2"
}

# pack writes the loose objects of the commits since the last pack into a new pack, and removes them.
pack() {
  git repack -q -d
}

# packs <repository> lists the names of the pack indices of <repository>.
packs() {
  local index
  for index in "$1"/.git/objects/pack/*.idx; do
    if [ -e "$index" ]; then
      basename "$index"
    fi
  done | sort
}

# loose <repository> lists the loose objects of <repository>.
loose() {
  (cd "$1/.git/objects" && find . -path './[0-9a-f][0-9a-f]/*' -type f) | sed -e 's|^\./||' -e 's|/||' | sort
}

# multi_pack_indices <repository> lists the files of the multi-pack-indices of <repository>.
multi_pack_indices() {
  (cd "$1/.git/objects/pack" && find . -name 'multi-pack-index*' -type f) | sed 's|^\./||' | sort
}

# record <case> repacks a copy of <case> with git, records the result and removes the copy.
record() {
  local name=$1 index
  packs "$name" > "$name.packs-before"
  cp -R "$name" "$name-git"
  git -C "$name-git" repack -q -d -l --geometric=2
  packs "$name-git" > "$name.packs-after"
  for index in $(comm -13 "$name.packs-before" "$name.packs-after"); do
    git -C "$name-git" show-index < "$name-git/.git/objects/pack/$index" | cut -d ' ' -f 2
  done | sort > "$name.new-objects"
  loose "$name-git" > "$name.loose-after"
  multi_pack_indices "$name-git" > "$name.midx-after"
  rm -rf "$name-git"
}

# decide <case> <limit> records in `decisions` whether `git maintenance run --auto` runs the `geometric-repack` task in a
# copy of <case> with `maintenance.geometric-repack.auto` set to <limit>.
decide() {
  local name=$1 limit=$2 trace
  cp -R "$name" "$name-maintenance"
  trace=$(GIT_TRACE=1 git -C "$name-maintenance" -c "maintenance.geometric-repack.auto=$limit" maintenance run --auto \
    --no-detach --quiet --task=geometric-repack 2>&1)
  rm -rf "$name-maintenance"
  case $trace in
  *"run_command: git repack"*) echo "$name $limit repack" ;;
  *) echo "$name $limit skip" ;;
  esac >> decisions
}

# Of the four packs that aren't kept, the three smallest are combined: one of them holds only a blob that the largest
# pack holds as well, and a kept pack holds the blob of another. The loose objects are those of a commit, a blob that
# nothing refers to, and the tree of the first commit, which the largest pack holds as well. A multi-pack-index lists
# every pack.
init geometric
(
  cd geometric
  base 40
  tree=$(git rev-parse 'HEAD^{tree}')
  cp ".git/objects/${tree:0:2}/${tree:2}" ../tree.loose
  pack
  change file1 one
  pack
  change file2 two
  pack
  git rev-parse HEAD~2:file3 | git pack-objects -q .git/objects/pack/pack > /dev/null
  kept=$(git rev-parse HEAD:file2 | git pack-objects -q .git/objects/pack/pack)
  touch ".git/objects/pack/pack-$kept.keep"
  change file3 three
  echo unreachable | git hash-object -w --stdin > /dev/null
  mkdir -p ".git/objects/${tree:0:2}"
  cp ../tree.loose ".git/objects/${tree:0:2}/${tree:2}"
  rm -f ../tree.loose
  git multi-pack-index write
)
record geometric

# The two smaller packs are combined, and the multi-pack-index lists only the largest pack.
init midx-retained
(
  cd midx-retained
  base 10
  pack
  git multi-pack-index write
  change file1 one
  pack
  change file2 two
  pack
)
record midx-retained

# The same with an incremental multi-pack-index.
init midx-incremental
(
  cd midx-incremental
  base 10
  pack
  git multi-pack-index write --incremental
  change file1 one
  pack
  change file2 two
  pack
)
record midx-incremental

# One pack stores 128 versions of a file in chains of deltas that are longer than the 50 deltas that `pack.depth`
# allows by default, and another pack holds as many objects, so that both are combined. Each version marks 5 more of
# the file's 700 lines than the one before it, so that git stores most versions as deltas of one of the next two.
# `chain.depth-before` is the length of the longest chain of deltas.
init chain
(
  cd chain
  for n in $(seq 1 128); do
    seq 1 700 | awk -v n="$n" '{ print ($1 <= 5 * n ? $1 " marked" : $1) }' > chained
    git add chained
    git commit -q -m "chained $n"
  done
  # With one thread, the search for deltas isn't split into parts, between which chains would end.
  git repack -q -a -d -f --depth=4095 --window=250 --threads=1
  for n in $(seq 1 128); do
    seq 1 "$((n * 5))" | sed 's/$/ other/' > other
    git add other
    git commit -q -m "other $n"
  done
  pack
  git verify-pack -v .git/objects/pack/*.idx | sed -n 's/^chain length = \([0-9]*\):.*/\1/p' | sort -n | tail -n 1 \
    > ../chain.depth-before
)
record chain

# The packs form a geometric progression already, so only the loose objects of a commit are packed.
init loose-only
(
  cd loose-only
  base 10
  pack
  change file1 one
  pack
  change file2 two
)
record loose-only

# The packs form a geometric progression, and there are no loose objects.
init nothing
(
  cd nothing
  base 10
  pack
  change file1 one
  pack
)
record nothing

# A kept pack holds every object of the two packs that are combined, so no pack is written.
init kept-only
(
  cd kept-only
  base 10
  pack
  change file1 one
  pack
  change file2 two
  pack
  kept=$(git rev-list --objects HEAD~2..HEAD | cut -d ' ' -f 1 | git pack-objects -q .git/objects/pack/pack)
  touch ".git/objects/pack/pack-$kept.keep"
)
record kept-only

# A cruft pack holds a blob that nothing refers to, which one of the combined packs holds as well.
init cruft
(
  cd cruft
  base 20
  cruft=$(echo cruft | git hash-object -w --stdin)
  git repack -q -d --cruft
  change file1 one
  pack
  change file2 two
  { git rev-list --objects HEAD~..HEAD | cut -d ' ' -f 1; echo "$cruft"; } | git pack-objects -q .git/objects/pack/pack \
    > /dev/null
  git prune-packed
)
record cruft

# A promisor pack holds a blob of one of the combined packs, which goes into the new pack nevertheless.
init promisor
(
  cd promisor
  base 20
  pack
  change file1 one
  pack
  change file2 two
  pack
  promisor=$(git rev-parse HEAD~:file1 | git pack-objects -q .git/objects/pack/pack)
  touch ".git/objects/pack/pack-$promisor.promisor"
)
record promisor

# Two promisor packs hold a blob each of the only other pack. Git combines them into a new promisor pack.
init promisors
(
  cd promisors
  base 10
  pack
  for file in file1 file2; do
    promisor=$(git rev-parse "HEAD:$file" | git pack-objects -q .git/objects/pack/pack)
    touch ".git/objects/pack/pack-$promisor.promisor"
  done
)
record promisors

# The smaller of two packs holds the first two of the three objects of the larger one, so that the pack that combines
# them is the larger one again.
init same-name
(
  cd same-name
  echo content > file
  git add file
  git commit -q -m one
  pack
  git show-index < ".git/objects/pack/$(packs .)" | sort -n | sed -n '1,2p' | cut -d ' ' -f 2 |
    git pack-objects -q .git/objects/pack/pack > /dev/null
)
record same-name

# The packs form a geometric progression, and the fanout directory `17` holds three loose blobs.
init auto
(
  cd auto
  base 10
  pack
  change file1 one
  pack
  mkdir ../candidates
  for n in $(seq 1 5000); do
    echo "loose $n" > "../candidates/$n"
  done
  seq -f '../candidates/%g' 1 5000 | git hash-object --stdin-paths | grep -n '^17' | sed -n '1,3p' | cut -d : -f 1 |
    while read -r n; do
      git hash-object -w "../candidates/$n" > /dev/null
    done
  rm -r ../candidates
  test "$(find .git/objects/17 -type f | wc -l)" -eq 3
)

decide auto 0
decide auto -1
decide auto 512
decide auto 513
decide midx-retained 1000000
decide nothing 1000000
