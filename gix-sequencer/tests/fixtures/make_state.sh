#!/usr/bin/env bash
set -eu -o pipefail

# Each case is a repository whose rebase, cherry-pick sequence or revert sequence stopped, with the state that Git left in
# `.git/rebase-merge` or `.git/sequencer`, or whose rebase ended. `.git/revs` holds the commits that the case names as
# `<name> <id>`, `.git/shorts` their abbreviations as `<name> <abbreviation>`, `.git/todo` the todo list that the user
# made of the list of `git rebase -i`, with full ids, and `.git/output` what Git printed.

root=$PWD

# Git for Windows can't check whether the range `<id>...<id>` of a rebase names a file when the directory and the range
# with SHA-256 ids make a path longer than Windows allows, unless it uses long paths.
export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.longpaths GIT_CONFIG_VALUE_0=true

# The editor of the todo list, which replaces it with the list in `$TODO`.
cat > "$root/write-todo" <<'EOF'
#!/bin/sh
cp "$TODO" "$1"
EOF
# The editor that copies the list it gets to `$COPY` before it replaces it.
cat > "$root/copy-and-write-todo" <<'EOF'
#!/bin/sh
cp "$1" "$COPY"
cp "$TODO" "$1"
EOF
chmod +x "$root/write-todo" "$root/copy-and-write-todo"

# commit <file> [<content> [<message>]] commits <file> with <content>, or with its name as content and message.
commit() {
  echo "${2:-$1}" > "$1"
  git add "$1"
  git commit -q -m "${3:-$1}"
}

# repo <case> makes the repository of <case> and enters it, on `topic`: `main` has `base` and `m`, which adds the file `c`,
# and `topic` forks from `base` with `a`, `b` and `c`, which add their files.
repo() {
  git init -q "$root/$1"
  cd "$root/$1"
  commit base
  git switch -q -c topic
  commit a
  commit b
  commit c
  git switch -q main
  commit c 'c of main' m
  git switch -q topic
}

# record <name>=<revision>... records the ids of the revisions in `.git/revs`.
record() {
  local pair
  for pair in "$@"; do
    echo "${pair%%=*} $(git rev-parse "${pair#*=}")" >> .git/revs
  done
}

# shorts <name>=<revision>... records the abbreviations of the revisions in `.git/shorts`.
shorts() {
  local pair
  for pair in "$@"; do
    echo "${pair%%=*} $(git rev-parse --short "${pair#*=}")" >> .git/shorts
  done
}

# line <command> <revision> prints the line of <command> for <revision> as Git writes it with the full id.
line() {
  echo "$1 $(git rev-parse "$2") # $(git log -1 --format=%s "$2")"
}

# rebase <rebase argument>... runs `git rebase -i` with `.git/todo` as the list that the user made, and expects it to
# stop.
rebase() {
  TODO="$PWD/.git/todo" GIT_SEQUENCE_EDITOR="$root/write-todo" git rebase -i "$@" > .git/output 2>&1 || true
  if ! test -d .git/rebase-merge; then
    cat .git/output >&2
    exit 1
  fi
}

# `edit` stops at `b` for amending, after it skipped `a`, whose parent is `onto` already, and fast-forwarded to `b`.
repo edit
{ line pick topic~2; line edit topic~1; line pick topic; } > .git/todo
record onto=main~1 orig-head=topic a=topic~2 b=topic~1 c=topic
rebase main~1

# `continued` stops at `b` too and continues to a `break`.
repo continued
{ line pick topic~2; line edit topic~1; echo break; line pick topic; } > .git/todo
record onto=main~1 orig-head=topic a=topic~2 b=topic~1 c=topic
rebase main~1
git rebase --continue >> .git/output 2>&1

# `break` stops at the `break` that its list starts with, with options that the state keeps.
repo break
{ echo break; line pick topic~2; line pick topic~1; line pick topic; } > .git/todo
record onto=main~1 orig-head=topic
rebase -q --ignore-date --empty=drop --no-reschedule-failed-exec main~1

# `options` stops at a `break` too, with the other options that the state keeps.
repo options
{ echo break; line pick topic~2; line pick topic~1; line pick topic; } > .git/todo
record onto=main~1 orig-head=topic
rebase -v --signoff --strategy=ort -X patience -X ignore-space-change --rerere-autoupdate --gpg-sign=key \
  --committer-date-is-author-date --empty=keep --reschedule-failed-exec \
  --trailer 'Reviewed-by: R <r@example.com>' --trailer 'Acked-by: A <a@example.com>' main~1

# `conflict` stops at `c`, which adds the file that `m` adds on `main`, after it picked `a` and `b` onto `main`.
repo conflict
{ line pick topic~2; line pick topic~1; line pick topic; } > .git/todo
record onto=main orig-head=topic a=topic~2 b=topic~1 c=topic
rebase main
record new-a=HEAD~1 new-b=HEAD

# `squash` stops at `fixup c`, which conflicts, in the chain that `squash b` started after `pick a` onto `main`.
repo squash
{ line pick topic~2; line squash topic~1; line fixup topic; } > .git/todo
record onto=main orig-head=topic a=topic~2 b=topic~1 c=topic
GIT_EDITOR=true rebase main
record head=HEAD

# `exec` stops as its `exec false` failed, after it skipped `a`, and `exec-rescheduled` puts the `exec` back.
repo exec
{ line pick topic~2; echo 'exec false'; line pick topic~1; } > .git/todo
record onto=main~1 orig-head=topic
rebase main~1
repo exec-rescheduled
{ line pick topic~2; echo 'exec false'; line pick topic~1; } > .git/todo
record onto=main~1 orig-head=topic
rebase --reschedule-failed-exec main~1

# `label` labels `onto` before it stops at a `break`.
repo label
{ echo 'label onto'; echo break; line pick topic~2; } > .git/todo
record onto=main~1 orig-head=topic
rebase main~1

# `update-ref` records `HEAD` for `side`, which the edit of the list added, before it stops at a `break`.
repo update-ref
git branch side main~1
{ line pick topic~2; echo 'update-ref refs/heads/side'; echo break; line pick topic~1; } > .git/todo
record onto=main~1 orig-head=topic a=topic~2 side=side
rebase main~1

# `finished` picks `a` and `b` onto `main` and ends, and `aborted` stops at a `break` and is aborted.
repo finished
{ line pick topic~2; line pick topic~1; } > .git/todo
record onto=main orig-head=topic
TODO="$PWD/.git/todo" GIT_SEQUENCE_EDITOR="$root/write-todo" git rebase -i main > .git/output 2>&1
test ! -d .git/rebase-merge
record head=topic
repo aborted
{ echo break; line pick topic~2; } > .git/todo
record onto=main~1 orig-head=topic
rebase main~1
git rebase --abort >> .git/output 2>&1
test ! -d .git/rebase-merge

# `dropped` doesn't start, as the list that the user made drops `b` and `c` while `rebase.missingCommitsCheck` is
# `error`.
repo dropped
line pick topic~2 > .git/todo
record b=topic~1 c=topic
shorts b=topic~1 c=topic
TODO="$PWD/.git/todo" GIT_SEQUENCE_EDITOR="$root/write-todo" git -c rebase.missingCommitsCheck=error rebase -i main~1 \
  > .git/output 2>&1 || true
test -f .git/rebase-merge/dropped

# `invalid` stops at a `break`, then `git rebase --edit-todo` copies the list that the editor gets to
# `.git/edit-todo-list` and gets a list with lines that Git rejects, which it reports in `.git/errors`.
repo invalid
git switch -q -c merged main~1
git merge -q --no-ff -m merged topic~1
git switch -q topic
record a=topic~2
shorts a=topic~2
{ echo break; line pick topic~2; } > .git/todo
rebase main~1
{
  line pick topic~1
  echo "bogus $(git rev-parse topic)"
  echo 'pick 0123456789abcdef0123'
  echo "pick $(git rev-parse 'topic^{tree}')"
  echo 'label #'
  echo 'update-ref onlyone'
  echo 'update-ref refs/heads/bad..name'
  echo "fixup $(git rev-parse merged)"
  echo "pick $(git rev-parse merged)"
  echo 'exec'
  echo 'noop now'
  echo 'break now'
} > .git/todo
COPY="$PWD/.git/edit-todo-list" TODO="$PWD/.git/todo" GIT_SEQUENCE_EDITOR="$root/copy-and-write-todo" \
  git rebase --edit-todo > /dev/null 2> .git/errors || true

# `cherry-pick` stops at `c`, which adds the file that `m` adds on `main`, after it picked `a` and `b` onto `main`, with
# the options that `opts` keeps.
repo cherry-pick
git switch -q main
record head=main a=topic~2 b=topic~1 c=topic
shorts a=topic~2 b=topic~1 c=topic
git cherry-pick -x --signoff --no-edit --allow-empty --allow-empty-message --keep-redundant-commits --strategy=ort \
  -X patience -X ignore-space-change --rerere-autoupdate --cleanup=scissors topic~2 topic~1 topic > .git/output 2>&1 ||
  true
test -d .git/sequencer
record new-a=HEAD~1 new-b=HEAD
# A revert of two commits can't start a sequence then, which Git reports in `.git/in-progress`.
git revert --no-edit HEAD HEAD~1 > /dev/null 2> .git/in-progress || true

# `revert` stops at `f1`, whose change of `f` `f2` changed again, after it reverted `g`.
git init -q "$root/revert"
cd "$root/revert"
commit f 0 base
commit f 1 f1
commit f 2 f2
commit g
record head=main g=main f1=main~2
shorts g=main f1=main~2
git revert --no-edit --signoff --cleanup=verbatim main main~2 > .git/output 2>&1 || true
test -d .git/sequencer
record new-g=HEAD
