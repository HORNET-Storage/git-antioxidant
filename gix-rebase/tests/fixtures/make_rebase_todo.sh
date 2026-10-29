#!/usr/bin/env bash
set -eu -o pipefail

# Each case is a directory with the todo list that `git rebase -i` hands its editor in `todo`, which `gix-rebase` makes
# byte for byte with the same options. The editor copies the list and empties it, so that Git stops with "nothing to
# do" and leaves the repository as it was. The commits that a case names are in its `revs` as `<name> <id>`, and
# `shorts` maps the id of each commit to its abbreviation as Git abbreviates it, as `<id> <abbreviation>`.
#
# Each case holds what `git rebase` printed in `rebase-output`, which tells for the cases `ff-*` whether it found the
# branch up to date, and `fork-point/fork-point` holds the fork point that `git merge-base --fork-point` found. For the
# commits of the repository `patches`, `patch-ids` holds what `git patch-id --stable` printed for each, and
# `prerequisite-patch-ids` the patch IDs that `git format-patch --base` printed for all but the first, oldest first.

# Git for Windows can't check whether the range `<id>...<id>` of a rebase names a file when the directory and the range
# with SHA-256 ids make a path longer than Windows allows, unless it uses long paths.
export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.longpaths GIT_CONFIG_VALUE_0=true

root=$PWD
cat > "$root/copy-todo" <<'EOF'
#!/bin/sh
cp "$1" "$TODO_COPY"
: > "$1"
EOF
chmod +x "$root/copy-todo"

# commit <file> [<message>] commits <file> with its name as content.
commit() {
  echo "$1" > "$1"
  git add "$1"
  git commit -q -m "${2:-$1}"
}

# record <case> <name>=<revision>... creates the directory of <case> with the ids of the revisions in `revs`.
record() {
  local case=$1
  shift
  mkdir -p "$root/$case"
  : > "$root/$case/revs"
  for pair in "$@"; do
    echo "${pair%%=*} $(git rev-parse "${pair#*=}")" >> "$root/$case/revs"
  done
}

# todo <case> <git option>... -- <rebase argument>... records the todo list of `git rebase -i` for <case>.
todo() {
  local case=$1
  shift
  local options=()
  while [ "$1" != -- ]; do
    options+=("$1")
    shift
  done
  shift
  if TODO_COPY="$root/$case/todo" GIT_SEQUENCE_EDITOR="$root/copy-todo" \
      git ${options[@]+"${options[@]}"} rebase -i "$@" > "$root/$case/rebase-output" 2>&1; then
    echo "the rebase of $case has to find nothing to do" >&2
    exit 1
  fi
  if ! test -f "$root/$case/todo"; then
    cat "$root/$case/rebase-output" >&2
    exit 1
  fi
  test ! -d .git/rebase-merge
}

git init -q repo
cd repo
git config core.abbrev 7
commit base

# `topic` has a commit with the patch of `u2` on `main`, an empty commit, and a commit with a body.
git switch -q -c topic
commit t1
echo cherry > cherry
git add cherry
git commit -q -m t2
git commit -q --allow-empty -m 't3 is empty'
commit t4 't4

with a body'
git switch -q main
commit u1
echo cherry > cherry
git add cherry
git commit -q -m u2

# `side` forks from `base` with a commit of its own.
git switch -q -c side main~2
commit s1

# `merges` merges the branch `feature`, and `u1` of `main`, which the rebase doesn't replay.
git switch -q -c merges main~2
commit r1
git switch -q -c feature
commit f1
commit f2
git switch -q merges
commit r2
git merge -q --no-ff -m "Merge branch 'feature'" feature
git merge -q --no-ff -m 'Merge pull request #1 from somewhere/else' main~1
commit r4

# `squashes` has commits that `--autosquash` moves, which name their commit by its subject, by the start of its subject
# or by its abbreviated id.
git switch -q -c squashes main~2
commit a1 'first subject'
commit a2 'second subject'
commit a3 'fixup! first subject'
commit a4 'squash! second'
commit a5 "fixup! $(git rev-parse --short HEAD~2)"
commit a6 'amend! first subject

first subject again'
commit a7 'fixup! fixup! second subject'

# `stack` has branches on its commits for `--update-refs`, one of which is checked out in another worktree.
git switch -q -c stack main~2
commit k1
git branch stack-1
commit k2
git branch stack-2
git branch stack-3
commit k3
git worktree add -q ../elsewhere stack-2

# `forked` forked from `upstream` at `m1`, which `upstream` no longer has, as `--fork-point` finds in its reflog.
git switch -q -c upstream main~2
commit m1
git switch -q -c forked
commit n1
git switch -q upstream
git reset -q --hard HEAD~1
commit m2

git switch -q topic
record linear upstream=main head=topic
todo linear -- main
record reapply upstream=main head=topic
todo reapply -- --reapply-cherry-picks main
record no-keep-empty upstream=main head=topic
todo no-keep-empty -- --no-keep-empty main
record abbreviate upstream=main head=topic
todo abbreviate -c rebase.abbreviateCommands=true -- main
record comment upstream=main head=topic
todo comment -c core.commentChar=';' -- main
record missing-commits-error upstream=main head=topic
todo missing-commits-error -c rebase.missingCommitsCheck=error -- main
record exec upstream=main head=topic
todo exec -- --exec 'echo one' --exec 'echo two' main
record keep-base upstream=main head=topic
todo keep-base -- --keep-base main
record onto upstream=main onto=side head=topic
todo onto -- --onto side main

git switch -q side
record root head=side
git commit-tree "$(git hash-object -t tree /dev/null)" < /dev/null > "$root/root/squash-onto"
todo root -- --root

git switch -q merges
record merges upstream=main head=merges
todo merges -- --rebase-merges main
record merges-cousins upstream=main head=merges
todo merges-cousins -- --rebase-merges=rebase-cousins main
record merges-linear upstream=main head=merges
todo merges-linear -- main

git switch -q squashes
record autosquash upstream=main head=squashes
todo autosquash -- --autosquash main

git switch -q stack
record update-refs upstream=main head=stack
todo update-refs -- --update-refs main

git switch -q forked
record fork-point upstream=upstream head=forked
todo fork-point -- --fork-point upstream
git merge-base --fork-point upstream forked > "$root/fork-point/fork-point"
record no-fork-point upstream=upstream head=forked
todo no-fork-point -- upstream

git log --all --format='%H %h' > "$root/shorts"
squash_onto=$(cat "$root/root/squash-onto")
echo "$squash_onto $(git rev-parse --short "$squash_onto")" >> "$root/shorts"

# `ahead` is ahead of `main`, `diverged` forked from it before its last commit, and `merged` merges another branch
# of `main`.
cd "$root"
git init -q ff
cd ff
commit base
commit x1
git switch -q -c ahead
commit a1
git switch -q -c diverged main~1
commit d1
git switch -q -c merged ahead
git switch -q -c other main
commit o1
git switch -q merged
git merge -q --no-ff -m 'merge other' other
git switch -q main

# fast_forward <case> <branch> <rebase argument>... records what `git rebase` prints as it rebases a copy of <branch>.
fast_forward() {
  local case=$1 branch=$2
  shift 2
  git switch -q -c "try-$case" "$branch"
  git rebase "$@" > "$root/$case/rebase-output" 2>&1
  git switch -q main
}

record ff-ahead upstream=main head=ahead
fast_forward ff-ahead ahead main
record ff-diverged upstream=main head=diverged
fast_forward ff-diverged diverged main
record ff-onto upstream=main onto=main~1 head=ahead
fast_forward ff-onto ahead --onto main~1 main
record ff-keep-base upstream=main head=diverged
fast_forward ff-keep-base diverged --keep-base main
record ff-merged upstream=main head=merged
fast_forward ff-merged merged main
record ff-behind upstream=main head=main~1
fast_forward ff-behind main~1 main

cd "$root"
git init -q patches
cd patches

# numbers [<number>...] writes 30 numbered lines to `lines`, with the lines of the given numbers changed.
numbers() {
  local i
  : > lines
  for ((i = 1; i <= 30; i++)); do
    case " $* " in
      *" $i "*) echo "changed line $i" >> lines;;
      *) echo "line $i" >> lines;;
    esac
  done
}

numbers
echo text > text
printf 'no line feed' > no-line-feed
git add lines text no-line-feed
git commit -q -m root

# Changes four lines apart make one hunk, and a change far from them another. Changes six lines apart still make one
# hunk, and changes seven lines apart two.
numbers 3 8 25
git commit -q -am 'two hunks'
numbers 3 8 25 10 17
git commit -q -am 'six lines apart'
numbers 3 8 25 10 17 20 28
git commit -q -am 'seven lines apart'

printf '\ttext with  spaces \n' > text
git commit -q -am 'spaces'
printf 'still no line feed' > no-line-feed
git commit -q -am 'no line feed'
printf 'a line feed\n' > no-line-feed
git commit -q -am 'a line feed'

printf 'binary\0content\n' > binary
git add binary
git commit -q -m 'add binary'
printf 'binary\0changed\n' > binary
git commit -q -am 'change binary'

git rm -q text
git commit -q -m 'delete text'
echo spaced > 'with space'
git add 'with space'
git commit -q -m 'add with space'
echo run > tool
git add tool
git update-index --chmod=+x tool
git commit -q -m 'add executable'
echo 'run more' > tool
git add tool
git update-index --chmod=-x tool
git commit -q -m 'change content and mode'
git update-index --add --cacheinfo "160000,$(git rev-parse HEAD),sub"
git commit -q -m 'add submodule'

# A line added to equal lines could be added anywhere among them.
printf 'x\nx\nx\n' > same
git add same
git commit -q -m 'add same'
printf 'x\nx\nx\nx\n' > same
git add same
git commit -q -m 'add another x'

# The diff of an empty new file and of a change of a mode alone has no lines.
: > empty
git add empty
git commit -q -m 'add empty'
git update-index --chmod=+x empty
git commit -q -m 'change mode'

numbers 3 8 25 10 17 20 28 1
echo 'spaced more' > 'with space'
git add lines 'with space'
git commit -q -m 'several files'

git rev-list --reverse HEAD | while read -r commit; do
  git -c diff.algorithm=myers -c diff.indentHeuristic=false show --format='commit %H' --no-renames --full-index \
    --no-ext-diff --no-textconv --no-color -U3 "$commit" | git patch-id --stable
done > "$root/patch-ids"

# `git format-patch --base` prints the patch IDs of the commits between the base and the patch as `commit_patch_id()`
# computes them, which the cherry-picks of a rebase use too.
commit last
git format-patch --stdout --base="$(git rev-list --max-parents=0 HEAD)" HEAD~1 |
  sed -n 's/^prerequisite-patch-id: //p' > "$root/prerequisite-patch-ids"
