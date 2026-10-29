#!/usr/bin/env bash
set -eu -o pipefail

# Files change over the commits, and `git pack-objects` stores most of their versions as deltas of others in `packed/`.
# The tests compute each of these deltas from the same base and target and compare it with Git's.
#
# The files cover what the encoder does: text with lines removed, changed and added; a line of characters with
# ranges replaced and long runs inserted, which take several insert instructions; a file of more than 64 KiB, whose
# copies take several copy instructions; a file whose halves trade places; and two repetitive files, one repeating
# 16 bytes, whose blocks all have the same hash, and one repeating 17 bytes, whose blocks fill buckets beyond 64
# entries.
#
# `objects.list` lists the objects in the order in which `git pack-objects` reads them, and `default/` and `shallow/`
# hold packs made with other windows and depths, the latter also with a window memory. The search tests compare the
# bases that they find with those of the three packs, which are made with one thread so that Git's bases don't depend
# on the speed of threads. `dir/text` has the name of `text`, and `one.c` and `two.c` end alike, so that their objects
# sort together and are tried as bases of each other; several root trees have the same size, as do several commits, so
# that their order in the list decides between them. The first and the last version of `relatives` and of `families`
# have one text and the versions between them another, so that the last version, the smallest, is only a delta of the
# first one, the largest. In the order of the search, the first version of `relatives` comes 10 objects before the
# last one, the farthest that the window of `default/` reaches, and that of `families` 11 objects, one beyond it. The
# commits that change one of these files have trees that differ in one entry and messages that share a long text, as
# do the two tags, so that objects of every kind are stored as deltas.

# words <seed> <lines> writes lines of words that repeat as the words of source code do, and characters <seed> <count>
# writes a line of printable characters. Their numbers come from a generator whose products stay below 2^23, so that
# every awk computes them exactly.
words() {
  awk -v x="$1" -v lines="$2" 'BEGIN {
    n = split("let fn match object delta base window depth pack index = { } ( ) ; self return Some None", word, " ")
    for (line = 0; line < lines; line++) {
      x = (x * 75 + 74) % 65537
      count = x % 10 + 1
      text = ""
      for (i = 0; i < count; i++) {
        x = (x * 75 + 74) % 65537
        text = text word[x % n + 1] " "
      }
      print text
    }
  }'
}

characters() {
  awk -v x="$1" -v count="$2" 'BEGIN {
    for (i = 0; i < count; i++) {
      x = (x * 75 + 74) % 65537
      printf "%c", 33 + x % 94
    }
    print ""
  }'
}

# repeat <unit> <count> writes <unit> <count> times and a newline.
repeat() {
  awk -v unit="$1" -v count="$2" 'BEGIN { for (i = 0; i < count; i++) printf "%s", unit; print "" }'
}

# edit_lines <file> <n> removes, changes and adds a line at places that depend on <n>.
edit_lines() {
  awk -v n="$2" '
    NR == 7 * n + 3 { next }
    NR == 11 * n + 5 { print "changed in edit " n; next }
    { print }
    NR == 13 * n + 1 { print "added in edit " n }
  ' "$1" > "$1.new"
  mv "$1.new" "$1"
}

# splice <file> <offset> <removed> <seed> <inserted> replaces <removed> bytes at <offset> with <inserted> characters.
splice() {
  {
    head -c "$2" "$1"
    characters "$4" "$5" | tr -d '\n'
    tail -c +"$(($2 + $3 + 1))" "$1"
  } > "$1.new"
  mv "$1.new" "$1"
}

# swap_halves <file> exchanges the first and the second half of <file>.
swap_halves() {
  local size half
  size=$(wc -c < "$1")
  half=$((size / 2))
  {
    tail -c +"$((half + 1))" "$1"
    head -c "$half" "$1"
  } > "$1.new"
  mv "$1.new" "$1"
}

# family <file> <seed> <versions> commits <file> with a text, then <versions> versions of another text, each one line
# longer than the version before, and then the first half of the first text.
family() {
  local n=1
  words "$2" 300 > "$1"
  git add "$1"
  git commit -q -m "$1 1" -m "$(words 9 8)"
  while [ "$n" -le "$3" ]; do
    words $(($2 + 1)) $((200 + n)) > "$1"
    n=$((n + 1))
    git commit -q -am "$1 $n" -m "$(words 9 8)"
  done
  words "$2" 150 > "$1"
  git commit -q -am "$1 $((n + 1))" -m "$(words 9 8)"
}

git init -q
words 1 400 > text
characters 2 6000 > characters
words 3 5000 > large
words 4 300 > halves
repeat 0123456789abcdef 3000 > period-16
repeat 0123456789abcdefg 3000 > period-17
mkdir dir
words 5 400 > dir/text
words 6 200 > one.c
words 6 200 > two.c
edit_lines two.c 7
git add -A
git commit -q -m "first versions"

for n in 1 2 3 4 5 6; do
  edit_lines text "$n"
  splice characters $((n * 700)) 50 $((n + 10)) $((n * 60))
  edit_lines large "$n"
  if [ "$n" = 3 ]; then
    swap_halves halves
  else
    edit_lines halves "$n"
  fi
  splice period-16 $((n * 5000 + 3)) 5 $((n + 20)) 7
  splice period-17 $((n * 5000 + 5)) 0 $((n + 30)) $((n * 40))
  edit_lines dir/text $((n + 6))
  edit_lines one.c "$n"
  if [ $((n % 2)) = 0 ]; then
    edit_lines two.c $((n + 7))
  fi
  git add -A
  git commit -q -m "edit $n"
done

family families 7 10
family relatives 11 9
git tag -a v1 -m "tag 1" -m "$(words 10 8)" HEAD~1
git tag -a v2 -m "tag 2" -m "$(words 10 8)"

git rev-list --objects --all > objects.list
mkdir packed default shallow
git pack-objects -q --delta-base-offset --threads=1 --window=250 --depth=50 packed/objects < objects.list > /dev/null
git pack-objects -q --delta-base-offset --threads=1 --window=10 --depth=50 default/objects < objects.list > /dev/null
git pack-objects -q --delta-base-offset --threads=1 --window=50 --depth=3 --window-memory=64k shallow/objects \
  < objects.list > /dev/null

# `thin/` holds the packs that pushes send. `<name>.revs` lists the revisions that `pack_objects()` in Git's `send-pack.c`
# passes to `git pack-objects --revs --thin --stdout --delta-base-offset`: the objects to send, and after `^` those that
# the receiver has. `<name>.pack` is the pack that it writes with one thread. The `loose` packs are made from the loose
# objects of the repository, and the `packed` ones from `packed.git`, a bare clone with a pack whose deltas they copy.
# `tags.list` lists the objects of the tags, each tag and the object that it peels to, which Git writes first.
git for-each-ref --format='%(objectname) %(*objectname)' refs/tags > tags.list
git clone -q --bare --no-local . packed.git
mkdir thin

# thin_pack <repository> <name> <revision>... writes `thin/<name>.revs` and the pack of these revisions.
thin_pack() {
  local repo=$1 name=$2
  shift 2
  printf '%s\n' "$@" > "thin/$name.revs"
  git -C "$repo" -c pack.threads=1 pack-objects -q --revs --thin --stdout --delta-base-offset \
    < "thin/$name.revs" > "thin/$name.pack"
}

for kind in loose packed; do
  repo=.
  if [ "$kind" = packed ]; then
    repo=packed.git
  fi
  # The new versions of every file since `edit 3`, sent with the tag `v2` of the last commit.
  thin_pack "$repo" "$kind-edits" "$(git rev-parse v2)" "^$(git rev-parse ':/^edit 3')"
  # The last commit, to a receiver that has the tag `v1` of the commit before it.
  thin_pack "$repo" "$kind-last" "$(git rev-parse HEAD)" "^$(git rev-parse v1)"
  # Both tags and all they reach, to a receiver that has nothing, so that there are no preferred bases.
  thin_pack "$repo" "$kind-all" "$(git rev-parse v1)" "$(git rev-parse v2)"
done
