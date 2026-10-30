#!/usr/bin/env bash
set -eu -o pipefail

# Each scenario is a repository `<name>` as it is before a rebase, in which the tests rebase with `gix`, and its copy
# `<name>-git`, in which `git rebase` rebased, with its standard error in `<name>-git.err` where it stopped or refused.
# Where Git stopped before it went on, `<name>-git-stopped` is the copy as Git left it at the stop.
#
# The hooks of the scenarios record how they ran in `.git/hook-records`, the editors what they received in
# `.git/editor-records`, and the commands of `exec` that they ran in `.git/exec-records`.

# Git for Windows can't check whether the revision range `<upstream>...<head>` of a rebase names a file once the path
# exceeds `MAX_PATH`, which the ids of SHA-256 reach in deep fixture directories, unless long paths are enabled.
export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.longpaths GIT_CONFIG_VALUE_0=true

# Fail if the command that follows succeeds.
function fails() {
  if "$@"; then
    echo "unexpectedly succeeded: $*" >&2
    exit 1
  fi
}

# Copy the repository `$1` to `$1-git`, in which Git acts.
function twin() {
  cp -R "$1" "$1-git"
}

# Keep a copy of `$1-git` as Git left it at a stop in `$1-git-stopped`.
function snapshot() {
  cp -R "$1-git" "$1-git-stopped"
}

# Commit the file `$2` of the repository `$1` with the content `$3` and the message `$4`.
function change() {
  echo "$3" >"$1/$2"
  git -C "$1" add "$2"
  git -C "$1" commit -q -m "$4"
}

# Install hooks into the repository `$1` that append how they ran to `.git/hook-records`: their arguments, the lines of
# the message they receive that aren't comments or empty, and their input. A hook fails if `.git/fail-<hook>` exists.
function record_hooks() {
  local hook
  mkdir -p "$1/.git/hooks"
  for hook in pre-rebase post-checkout pre-commit prepare-commit-msg commit-msg post-commit post-rewrite; do
    cat >"$1/.git/hooks/$hook" <<'EOF'
#!/bin/sh
hook=${0##*/}
{
  echo "hook: $hook"
  for arg in "$@"; do
    echo "argument: $arg"
  done
  case "$hook" in
    prepare-commit-msg|commit-msg)
      echo "message:"
      grep -v -e '^#' -e '^$' "$1" || :
      echo "end of message"
      ;;
    post-rewrite)
      echo "input:"
      cat
      echo "end of input"
      ;;
  esac
} >>.git/hook-records
if test -e ".git/fail-$hook"; then
  exit 1
fi
EOF
    chmod +x "$1/.git/hooks/$hook"
  done
}

# Configure the editors of the repository `$1`. The sequence editor records the commands of the todo list it receives
# and writes `.git/todo` over it if that exists. The editor of messages records the lines of the message it receives
# that aren't comments or empty, and writes `.git/message-<n>` over the message of its `n`th run if that exists.
function editors() {
  cat >"$1/.git/sequence-editor" <<'EOF'
#!/bin/sh
{
  echo "todo list:"
  grep -v -e '^#' -e '^$' "$1" || :
} >>.git/editor-records
if test -f .git/todo; then
  cat .git/todo >"$1"
fi
EOF
  cat >"$1/.git/editor" <<'EOF'
#!/bin/sh
n=$(( $(cat .git/editor-runs 2>/dev/null || echo 0) + 1 ))
echo "$n" >.git/editor-runs
{
  echo "message $n:"
  grep -v -e '^#' -e '^$' "$1" || :
} >>.git/editor-records
if test -f ".git/message-$n"; then
  cat ".git/message-$n" >"$1"
fi
EOF
  git -C "$1" config sequence.editor "sh .git/sequence-editor"
  git -C "$1" config core.editor "sh .git/editor"
}

# Prepare the repository `$1` for its rebases with the recording hooks and editors.
function prepare() {
  record_hooks "$1"
  editors "$1"
}

# Write the todo list that the sequence editor of the repository `$1` writes, with a line for each pair of a command
# and a revision that follows, and the revision as its full commit id.
function todo() {
  local repo=$1
  shift
  while test $# -gt 0; do
    if test "$1" = break; then
      echo break
      shift
    else
      echo "$1 $(git -C "$repo" rev-parse "$2")"
      shift 2
    fi
  done >"$repo/.git/todo"
}

# Create the repository `$1` whose `main` has the commit `base` with the files `a`, `b`, `c` and `d` and then
# `upstream`, which changes `a`, and whose `topic`, checked out, has `one` and `two` on top of `base`, which change `b`
# and `c`.
function history() {
  base "$1"
  git -C "$1" checkout -q -b topic
  change "$1" b b2 one
  change "$1" c c2 two
  git -C "$1" checkout -q main
  change "$1" a a2 upstream
  git -C "$1" checkout -q topic
}

# Create the repository `$1` with the commit `base` of the files `a`, `b`, `c` and `d` on `main`.
function base() {
  git init -q "$1"
  echo a >"$1/a"
  echo b >"$1/b"
  echo c >"$1/c"
  echo d >"$1/d"
  git -C "$1" add a b c d
  git -C "$1" commit -q -m base
}

# Create the repository `$1` like `history`, in which `topic` has `three`, `four` and `five` after `two`, which change
# `d` and add `e` and `f`.
function long_history() {
  history "$1"
  change "$1" d d2 three
  change "$1" e e four
  change "$1" f f five
}

# Create the repository `$1` like `history`, in which `one` changes `a` as well, so that it conflicts with `upstream`.
function conflicting() {
  base "$1"
  git -C "$1" checkout -q -b topic
  echo a1 >"$1/a"
  echo b2 >"$1/b"
  git -C "$1" commit -q -am one
  change "$1" c c2 two
  git -C "$1" checkout -q main
  change "$1" a a2 upstream
  git -C "$1" checkout -q topic
}

# Create the repository `$1` whose `topic`, checked out, has `one` on top of `main`.
function linear() {
  git init -q "$1"
  echo a >"$1/a"
  git -C "$1" add a
  git -C "$1" commit -q -m base
  git -C "$1" checkout -q -b topic
  change "$1" b b one
}

# Create the repository `$1` like `history`, in which `topic` has `one`, `two`, which changes `a` so that it conflicts
# with `upstream`, and `three`, which changes `c`.
function conflicting_chain() {
  base "$1"
  git -C "$1" checkout -q -b topic
  change "$1" b b2 one
  change "$1" a a1 two
  change "$1" c c2 three
  git -C "$1" checkout -q main
  change "$1" a a2 upstream
  git -C "$1" checkout -q topic
}

# The rebases of `topic` onto `main` without options and with `--onto`, and of a given branch.
history pick
prepare pick
twin pick
git -C pick-git rebase main

history onto
prepare onto
twin onto
git -C onto-git rebase --onto main topic~1

history branch-argument
git -C branch-argument checkout -q main
prepare branch-argument
twin branch-argument
git -C branch-argument-git rebase main topic

history reflog-action
prepare reflog-action
twin reflog-action
GIT_REFLOG_ACTION=custom git -C reflog-action-git rebase main

# The rebases of branches that are up to date, which Git only switches to, and of a branch behind `main`.
linear up-to-date
prepare up-to-date
twin up-to-date
git -C up-to-date-git rebase main

linear up-to-date-switch
git -C up-to-date-switch checkout -q main
prepare up-to-date-switch
twin up-to-date-switch
git -C up-to-date-switch-git rebase main topic

linear fast-forward
git -C fast-forward checkout -q main
git -C fast-forward merge -q --ff-only topic
change fast-forward c c upstream
git -C fast-forward checkout -q topic
prepare fast-forward
twin fast-forward
git -C fast-forward-git rebase main

# `--keep-base`, forced to replay the commits, and `--root` without and with `--onto`.
history keep-base
prepare keep-base
twin keep-base
git -C keep-base-git rebase --keep-base --force-rebase main

history root
prepare root
twin root
git -C root-git rebase --root

history root-onto
git -C root-onto checkout -q --orphan orphan
git -C root-onto rm -q -rf .
change root-onto x x orphan-one
change root-onto y y orphan-two
prepare root-onto
twin root-onto
git -C root-onto-git rebase --root --onto main

# The upstream of `topic` is `main`, which was rewritten after `topic` forked from it, so that the fork point of
# `topic` is a commit that `main` had before.
git init -q fork-point
change fork-point a a base
change fork-point m m1 m1
git -C fork-point checkout -q -b topic
change fork-point t t t1
git -C fork-point config branch.topic.remote .
git -C fork-point config branch.topic.merge refs/heads/main
git -C fork-point checkout -q main
git -C fork-point reset -q --hard HEAD~1
change fork-point m m1b "m1 rewritten"
git -C fork-point checkout -q topic
prepare fork-point
twin fork-point
git -C fork-point-git rebase

# Refusals: changes that aren't committed, a `pre-rebase` hook that fails, and a branch to rebase that another worktree
# has checked out, which is linked with relative paths so that the copies of the repository keep it.
history dirty
echo changed >dirty/b
prepare dirty
twin dirty
fails git -C dirty-git rebase main 2>dirty-git.err

history pre-rebase-refuses
prepare pre-rebase-refuses
touch pre-rebase-refuses/.git/fail-pre-rebase
twin pre-rebase-refuses
fails git -C pre-rebase-refuses-git rebase main 2>pre-rebase-refuses-git.err

history used-by-worktree
git -C used-by-worktree checkout -q main
git -C used-by-worktree worktree add -q other topic
# `--relative-paths` needs Git 2.48 or newer, so the links between the worktree and the repository are rewritten as
# relative paths, as `make_indirect_head_repos.sh` does.
printf 'gitdir: ../.git/worktrees/other\n' >used-by-worktree/other/.git
printf '../../../other/.git\n' >used-by-worktree/.git/worktrees/other/gitdir
prepare used-by-worktree
twin used-by-worktree
fails git -C used-by-worktree-git rebase main topic 2>used-by-worktree-git.err

# A conflict, at which Git stops, and what follows it: `--continue` after resolving it, `--skip`, `--abort` and
# `--quit`.
conflicting conflict
prepare conflict
twin conflict
fails git -C conflict-git rebase main 2>conflict-git.err

conflicting conflict-continue
prepare conflict-continue
twin conflict-continue
fails git -C conflict-continue-git rebase main 2>/dev/null
echo resolved >conflict-continue-git/a
git -C conflict-continue-git add a
git -C conflict-continue-git rebase --continue

conflicting conflict-skip
prepare conflict-skip
twin conflict-skip
fails git -C conflict-skip-git rebase main 2>/dev/null
git -C conflict-skip-git rebase --skip

conflicting conflict-abort
prepare conflict-abort
twin conflict-abort
fails git -C conflict-abort-git rebase main 2>/dev/null
git -C conflict-abort-git rebase --abort

conflicting conflict-quit
prepare conflict-quit
twin conflict-quit
fails git -C conflict-quit-git rebase main 2>/dev/null
git -C conflict-quit-git rebase --quit

# Interactive rebases with the todo lists that the sequence editor writes, and the messages that the editor writes.
history reword
prepare reword
todo reword reword topic~1 pick topic
echo "one reworded" >reword/.git/message-1
twin reword
git -C reword-git rebase -i main

# The worktree has a file whose path is the command line of the editor, which stays a command line for the shell.
history reword-editor-file
prepare reword-editor-file
todo reword-editor-file reword topic~1 pick topic
echo "one reworded" >reword-editor-file/.git/message-1
mkdir "reword-editor-file/sh .git"
printf '#!/bin/sh\necho "ran as a file" >>.git/editor-records\n' >"reword-editor-file/sh .git/editor"
chmod +x "reword-editor-file/sh .git/editor"
twin reword-editor-file
git -C reword-editor-file-git rebase -i main

# The `commit-msg` hook fails when `reword` amends its commit, so that the rebase stops after the commit was picked.
history reword-hook-fails
prepare reword-hook-fails
todo reword-hook-fails reword topic~1 pick topic
touch reword-hook-fails/.git/fail-commit-msg
twin reword-hook-fails
fails git -C reword-hook-fails-git rebase -i main 2>reword-hook-fails-git.err
snapshot reword-hook-fails
rm reword-hook-fails-git/.git/fail-commit-msg
git -C reword-hook-fails-git rebase --continue

# A fixup chain whose second commit conflicts, after which the rebase continues with the resolution or skips the
# commit. Where it stops at `fixup -c`, Git 2.56 records the command with `-c` in `current-fixups`, and older Git
# without it.
conflicting_chain fixup-conflict
prepare fixup-conflict
todo fixup-conflict pick topic~2 "fixup -c" topic~1 fixup topic
twin fixup-conflict
fails git -C fixup-conflict-git rebase -i main 2>fixup-conflict-git.err
snapshot fixup-conflict
echo resolved >fixup-conflict-git/a
git -C fixup-conflict-git add a
git -C fixup-conflict-git rebase --continue

conflicting_chain fixup-conflict-skip
prepare fixup-conflict-skip
todo fixup-conflict-skip pick topic~2 fixup topic~1 fixup topic
twin fixup-conflict-skip
fails git -C fixup-conflict-skip-git rebase -i main 2>/dev/null
git -C fixup-conflict-skip-git rebase --skip

long_history squash
prepare squash
todo squash pick topic~4 squash topic~3 fixup topic~2 pick topic~1 squash topic
echo "four and five" >squash/.git/message-2
twin squash
git -C squash-git rebase -i main

long_history fixup-message
prepare fixup-message
todo fixup-message pick topic~4 "fixup -C" topic~3 "fixup -c" topic~2 pick topic~1 fixup topic
twin fixup-message
git -C fixup-message-git rebase -i main

history edit
prepare edit
todo edit edit topic~1 pick topic
twin edit
git -C edit-git rebase -i main 2>/dev/null
snapshot edit
git -C edit-git rebase --continue

history break
prepare break
todo break pick topic~1 break pick topic
twin break
git -C break-git rebase -i main 2>/dev/null
snapshot break
git -C break-git rebase --continue

long_history drop
prepare drop
todo drop pick topic~4 drop topic~3 pick topic~2
twin drop
git -C drop-git rebase -i main

history missing-commits
git -C missing-commits config rebase.missingCommitsCheck error
prepare missing-commits
todo missing-commits pick topic
twin missing-commits
fails git -C missing-commits-git rebase -i main 2>missing-commits-git.err

history instruction-format
git -C instruction-format config rebase.instructionFormat "%s by %an"
git -C instruction-format config rebase.abbreviateCommands true
prepare instruction-format
twin instruction-format
git -C instruction-format-git rebase -i main

# `exec` lines of `--exec`, all of which succeed, or one of which fails once and is rescheduled.
history exec
prepare exec
twin exec
git -C exec-git rebase -i --exec "echo ran >>.git/exec-records" main

history exec-fails
git -C exec-fails config rebase.rescheduleFailedExec true
prepare exec-fails
twin exec-fails
fails git -C exec-fails-git rebase --exec "test -f .git/exec-ok || { touch .git/exec-ok; exit 1; }" main 2>exec-fails-git.err
snapshot exec-fails
git -C exec-fails-git rebase --continue

# `--autosquash` as `rebase.autoSquash` asks for it, and `--update-refs` as `rebase.updateRefs` asks for it.
history autosquash
git -C autosquash config rebase.autoSquash true
change autosquash b b3 "fixup! one"
change autosquash d d2 "squash! one"
prepare autosquash
twin autosquash
git -C autosquash-git rebase -i main

history update-refs
git -C update-refs config rebase.updateRefs true
git -C update-refs branch part topic~1
prepare update-refs
twin update-refs
git -C update-refs-git rebase main

# `--rebase-merges` of a branch with a merge.
history rebase-merges
git -C rebase-merges checkout -q -b side main~1
change rebase-merges e e side-one
git -C rebase-merges checkout -q topic
git -C rebase-merges merge -q --no-ff -m "Merge branch 'side' into topic" side
change rebase-merges d d2 three
prepare rebase-merges
twin rebase-merges
git -C rebase-merges-git rebase -i --rebase-merges main
