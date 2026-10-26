#!/usr/bin/env bash
set -eu -o pipefail
export LC_ALL=C

# Each case is a repository whose packs store some versions of files whole, which `git repack` stores as deltas of each
# other, and others as deltas that it can copy. For each case and each variant of the configuration, the fixture records
# what `git repack -d -l --geometric=2`, the repack of `git maintenance`, does with a copy of the case:
#
# * `<case>.packs-before` and `<case>.loose-before`: the names of the pack indices and the loose objects before,
# * `<case>.mtimes`: the data file of each pack, the time at which it was modified as seconds since the epoch, and as
#   argument of `touch -t` in UTC. Each copy gets these times before it is repacked, as git lists the objects of the
#   pack modified last first, and as copying the case changes them.
# * `<case>.<variant>/git.pack` and `git.idx`: the pack that git wrote, and its index,
# * `<case>.<variant>/verify-pack`: what `git verify-pack -v` reports of that pack.
#
# Each loose object is in another fanout directory, as git lists the objects of a fanout directory in the order of the
# file system. With `pack.threads=1`, git searches deltas in a single thread, so that its deltas don't depend on the
# speed of threads, which they do in the variant `threads`. The packs depend on the zlib that git uses as well.

# init <name> creates the repository <name> without the files of git's template directory.
init() {
  git init -q --template= "$1"
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

# label <case> <label>, run in the repository of <case>, names the pack that the last command added to it.
label() {
  local new
  new=$(comm -13 <(cut -d ' ' -f 1 "../$1.labels" 2> /dev/null | sort) <(packs .))
  echo "$new $2" >> "../$1.labels"
}

# times <case> <label>... gives the packs with <label>s the modification times 2023-11-14 22:13:20 UTC and one minute
# later for each label after the first, in `<case>.mtimes`.
times() {
  local name=$1 label pack minute=13
  shift
  for label in "$@"; do
    pack=$(sed -n "s/ $label\$//p" "$name.labels")
    echo "${pack%.idx}.pack $((1700000000 + (minute - 13) * 60)) 2023111422$minute.20"
    minute=$((minute + 1))
  done > "$name.mtimes"
  rm "$name.labels"
}

# apply_times <repository> <case> gives the packs of <repository> the modification times of `<case>.mtimes`.
apply_times() {
  local pack seconds stamp
  while read -r pack seconds stamp; do
    TZ=UTC0 touch -m -t "$stamp" "$1/.git/objects/pack/$pack"
  done < "$2.mtimes"
}

# before <case> records the packs and the loose objects of <case>, which are each in another fanout directory.
before() {
  packs "$1" > "$1.packs-before"
  loose "$1" > "$1.loose-before"
  test -z "$(cut -c 1-2 "$1.loose-before" | uniq -d)"
}

# record <case> <variant> <git-option>... repacks a copy of <case> with git and <git-option>s, keeps the pack it wrote
# with what `git verify-pack -v` reports of it in `<case>.<variant>`, and removes the copy. The copy has a short name,
# as the paths of the temporary files of git in it must fit into the 260 characters that Windows allows by default.
record() {
  local name=$1 variant=$2 copy=copy index
  shift 2
  cp -R "$name" "$copy"
  apply_times "$copy" "$name"
  git -C "$copy" "$@" repack -q -d -l --geometric=2
  index=$(comm -13 "$name.packs-before" <(packs "$copy"))
  mkdir "$name.$variant"
  git -C "$copy" verify-pack -v ".git/objects/pack/$index" > "$name.$variant/verify-pack"
  mv "$copy/.git/objects/pack/${index%.idx}.pack" "$name.$variant/git.pack"
  mv "$copy/.git/objects/pack/$index" "$name.$variant/git.idx"
  rm -rf "$copy"
}

# words <seed> <lines> writes <lines> lines of words that are chosen by the generator of the seed <seed>.
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

# edit_lines <file> <n> removes, changes and adds a line of <file> at places that depend on <n>.
edit_lines() {
  awk -v n="$2" '
    NR == 7 * n + 3 { next }
    NR == 11 * n + 5 { print "changed in edit " n; next }
    { print }
    NR == 13 * n + 1 { print "added in edit " n }
  ' "$1" > "$1.new"
  mv "$1.new" "$1"
}

# The first pack holds the first commit with 200 small files that no other commit changes, and remains. Each commit
# after it edits four files, two of which have the same name in other directories. Three packs that are made with a
# window of 0 store the versions of three commits whole: the first of them also adds a file that the second renames,
# and the third copies it with a change. A pack made by `git repack` stores the versions of the next three commits as
# deltas, and one made by `git pack-objects` holds some of the objects of two of these commits again, also as deltas.
# Another pack holds the first and the last version of a file, and another the tag of an annotated tag whose name is
# gone, which tags a blob that nothing else refers to. The edits of the last commit are loose, as are another
# unreachable blob and another such tag. The edits have committer dates of other days than the first commit, but two of
# them have the same date, and the last one that of the first commit.
init versions
(
  cd versions
  edit() {
    edit_lines src/main.c "$1"
    edit_lines lib/main.c $(($1 + 3))
    edit_lines src/net/socket.c "$1"
    edit_lines docs/readme.txt "$1"
    git add -A
    GIT_COMMITTER_DATE="2000-01-$((10 + $2)) 00:00:00 +0000" git commit -q -m "edit $1" -m "$(words 9 8)"
  }
  mkdir -p docs src/net lib
  for n in $(seq 1 200); do
    words $((1000 + n)) 2 > "docs/page$n.md"
  done
  words 1 150 > src/main.c
  words 2 150 > lib/main.c
  words 3 100 > src/net/socket.c
  words 4 80 > docs/readme.txt
  git add -A
  git commit -q -m base -m "$(words 9 8)"
  git -c pack.threads=1 repack -q -a -d
  label versions R
  words 77 120 > docs/m.txt
  edit 1 1
  git -c pack.window=0 repack -q -d
  label versions W1
  git mv docs/m.txt lib/m.c
  edit 2 1
  git -c pack.window=0 repack -q -d
  label versions W2
  sed -e 's/fn/function/' lib/m.c > lib/p.c
  edit 3 3
  git -c pack.window=0 repack -q -d
  label versions W3
  edit 4 4
  edit 5 5
  edit 6 6
  git -c pack.threads=1 repack -q -d
  label versions D4
  git rev-list --objects HEAD~5..HEAD~3 | cut -d ' ' -f 1 |
    git -c pack.threads=1 pack-objects -q .git/objects/pack/pack > /dev/null
  label versions D5
  printf '%s\n' "$(git rev-parse HEAD~6:src/net/socket.c)" "$(git rev-parse HEAD:src/net/socket.c)" |
    git -c pack.threads=1 pack-objects -q .git/objects/pack/pack > /dev/null
  label versions X6
  { words 50 60; echo unreachable; } | git hash-object -w --stdin > /dev/null
  git tag -a unreachable-1 -m "tag 1" -m "$(words 10 8)"
  git rev-parse unreachable-1 | git -c pack.window=0 pack-objects -q .git/objects/pack/pack > /dev/null
  git tag -d unreachable-1 > /dev/null
  git prune-packed
  label versions U
  edit_lines src/main.c 7
  edit_lines lib/main.c 7
  git commit -q -am "edit 7" -m "$(words 9 8)"
  { words 50 60; echo unreachable too; } | git hash-object -w --stdin > /dev/null
  git tag -a unreachable-2 -m "tag 2" -m "$(words 10 8)"
  git tag -d unreachable-2 > /dev/null
)
times versions R W1 W2 W3 D4 X6 U D5
before versions
record versions default -c pack.threads=1
record versions window1 -c pack.threads=1 -c pack.window=1
record versions window-memory -c pack.threads=1 -c pack.windowMemory=8k
record versions depth2 -c pack.threads=1 -c pack.depth=2
record versions depth1 -c pack.threads=1 -c pack.depth=1
record versions threshold -c pack.threads=1 -c core.bigFileThreshold=2000
record versions threads -c pack.threads=4

# One pack stores 128 versions of a file in chains of deltas that are longer than the 50 deltas that `pack.depth`
# allows by default. Each version marks 5 more of the file's 700 lines than the one before it, so that git stores most
# versions as deltas of one of the next two. Another pack stores 128 versions of another file whole, each with 5 more
# lines than the one before it, with their trees and commits, so that both packs are combined.
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
  label chain chained
  for n in $(seq 1 128); do
    seq 1 "$((n * 5))" | sed 's/$/ other/' > other
    git add other
    git commit -q -m "other $n"
  done
  git -c pack.window=0 repack -q -d
  label chain other
)
times chain chained other
before chain
record chain default -c pack.threads=1
record chain depth3 -c pack.threads=1 -c pack.depth=3
record chain threads -c pack.threads=4
