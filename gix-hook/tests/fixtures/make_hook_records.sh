#!/usr/bin/env bash
set -eu -o pipefail

# Git runs hooks that record how it started them, and the tests run the same hooks through `gix-hook` to compare their
# records with those Git made.
#
# `record-hook` is that hook. Started as `<hook>`, or with `HOOK_NAME=<hook>`, it appends to `$HOOK_RECORDS/<hook>` its
# arguments, the directory it runs in, those of the variables Git exports for hooks that are set, and its standard
# input. It appends `<hook>` to `$HOOK_RECORDS/order`, prints `stdout of <hook>` to its standard output, and exits with
# `$HOOK_EXIT`, or 0. Paths below `$HOOK_BASE`, this directory, are recorded as `<base>/...`, whether Git or Git for
# Windows' `sh` wrote them, so that the records of Git and of the tests can be compared.
#
# Each scenario records into `records/<scenario>`, where `out` and `err` hold what the commands printed, and `status`
# the exit status of a command whose hooks don't all succeed. `oids/<name>` holds the objects the tests pass to hooks.
# The scenarios that differ between Unix and Windows are `no-shebang` and `not-executable`, which is why the fixture
# isn't archived.

root=$PWD
export HOOK_BASE=$root
mkdir records oids bin

cat >record-hook <<'EOF'
#!/bin/sh
name=${0##*/}
name=${HOOK_NAME:-${name##*\\}}
base=$(cd "$HOOK_BASE" && pwd -P)
native_base=$(cd "$HOOK_BASE" && { pwd -W 2>/dev/null || pwd -P; })
relative() {
  case $1 in
  "$native_base" | "$native_base"/*) printf '<base>%s\n' "${1#"$native_base"}" ;;
  "$base" | "$base"/*) printf '<base>%s\n' "${1#"$base"}" ;;
  *) printf '%s\n' "$1" ;;
  esac
}
{
  printf 'hook: %s\n' "$name"
  for arg in "$@"; do
    printf 'arg: %s\n' "$arg"
  done
  printf 'cwd: %s\n' "$(relative "$(pwd -P)")"
  for var in GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_EDITOR GIT_IMPLICIT_WORK_TREE GIT_PREFIX; do
    if eval "test -n \"\${$var+set}\""; then
      eval "value=\$$var"
      printf '%s: %s\n' "$var" "$(relative "$value")"
    fi
  done
  echo "stdin:"
  cat
  echo "end"
} >>"$HOOK_RECORDS/$name"
printf '%s\n' "$name" >>"$HOOK_RECORDS/order"
printf 'stdout of %s\n' "$name"
exit "${HOOK_EXIT:-0}"
EOF
chmod +x record-hook
# Found in `PATH` by a configured hook.
cp record-hook bin/record-path

# Install `record-hook` as the hooks named by all but the first argument in the hooks directory $1.
install_hooks() {
  local dir=$1
  shift
  mkdir -p "$dir"
  for hook in "$@"; do
    cp "$root/record-hook" "$dir/$hook"
  done
}

# Remove all hooks from the hooks directory $1.
remove_hooks() {
  rm -rf "$1"
  mkdir "$1"
}

# Run all but the first argument as command, recording its hooks in `records/$1`. If the command fails, what it wrote
# to its standard error is shown as well.
record() {
  local dir=$root/records/$1
  shift
  mkdir -p "$dir"
  local status=0
  HOOK_RECORDS=$dir "$@" >>"$dir/out" 2>>"$dir/err" || status=$?
  if [ "$status" != 0 ]; then
    cat "$dir/err" >&2
  fi
  return "$status"
}

# Write the object that $2 names in the repository of the current directory to `oids/$1`.
oid() {
  git rev-parse "$2" >"$root/oids/$1"
}

git init -q repo
(cd repo
  # `git rebase` makes Git for Windows look for a file named `<onto>...<head>`, and `git push` writes objects to a
  # temporary directory of the remote, which fail as names too long for Windows in this directory with SHA-256 objects,
  # unless long paths are allowed. Other platforms ignore this setting.
  git config core.longpaths true
  echo one >file
  git add file
  git commit -q -m one
  for branch in topic squashed rebased linked; do
    git branch "$branch"
  done
  git checkout -q topic
  echo topic >topic
  git add topic
  git commit -q -m topic
  git checkout -q squashed
  echo squashed >squashed
  git add squashed
  git commit -q -m squashed
  git checkout -q rebased
  echo r1 >r1
  git add r1
  git commit -q -m r1
  echo r2 >r2
  git add r2
  git commit -q -m r2
  git checkout -q main
  echo two >>file
  git add file
  git commit -q -m two
)
git clone -q --bare -c core.longpaths=true repo remote.git
git -C repo remote add origin ../remote.git

# `git commit` with a message: `pre-commit`, `prepare-commit-msg`, `commit-msg` and `post-commit`, without editor.
install_hooks repo/.git/hooks pre-commit prepare-commit-msg commit-msg post-commit
(cd repo
  echo three >>file
  git add file
  record commit git commit -q -m three
)
remove_hooks repo/.git/hooks

# `git commit --amend --no-edit`: the message of `HEAD` is reused, and `post-rewrite` learns the rewritten commit,
# which differs from it by the content of `file`.
install_hooks repo/.git/hooks prepare-commit-msg post-rewrite
(cd repo
  oid amend-old HEAD
  echo amended >>file
  git add file
  record amend git commit -q --amend --no-edit
  oid amend-new HEAD
)
remove_hooks repo/.git/hooks

# `git merge` making a merge commit, with a message and without editor.
install_hooks repo/.git/hooks pre-merge-commit prepare-commit-msg commit-msg post-merge
(cd repo && record merge git merge -q --no-ff -m "merge topic" topic)
remove_hooks repo/.git/hooks

# `git merge --squash`, which doesn't commit.
install_hooks repo/.git/hooks post-merge
(cd repo && record squash git merge -q --squash squashed)
remove_hooks repo/.git/hooks
git -C repo commit -q -m squashed

# `git checkout` of branches, then of a file.
install_hooks repo/.git/hooks post-checkout
(cd repo
  oid main HEAD
  oid topic topic
  record checkout git checkout -q topic
  record checkout git checkout -q main
  echo changed >file
  record checkout git checkout -q -- file
)
remove_hooks repo/.git/hooks

# `git rebase <upstream>` of the current branch, which checks out the commit it starts from and rewrites two commits.
(cd repo
  git checkout -q rebased
  oid rebase-old-1 HEAD~1
  oid rebase-old-2 HEAD
  oid rebase-onto main
)
install_hooks repo/.git/hooks pre-rebase post-checkout post-rewrite
(cd repo && record rebase git rebase -q main)
remove_hooks repo/.git/hooks
(cd repo
  oid rebase-new-1 HEAD~1
  oid rebase-new-2 HEAD
  git checkout -q main
)

# `pre-rebase` with the branch given, and with `--root`. Both are forced, as Git doesn't run the hook if the branch is
# up to date.
install_hooks repo/.git/hooks pre-rebase
(cd repo
  record pre-rebase git rebase -q -f main rebased
  record pre-rebase git rebase -q -f --root rebased
  git checkout -q main
)
remove_hooks repo/.git/hooks

# `git push` of an update, then of a deletion. The standard output of `pre-push` is that of Git.
install_hooks repo/.git/hooks pre-push
(cd remote.git
  oid push-old-main main
  oid push-old-topic topic
)
(cd repo
  oid push-main main
  record push git push -q origin main
  record push git push -q origin :refs/heads/topic
)
remove_hooks repo/.git/hooks

# Reference transactions: a creation, an update, a symbolic reference, an aborted transaction and a deletion.
install_hooks repo/.git/hooks reference-transaction
(cd repo
  record reference-transaction git update-ref refs/heads/created main
  record reference-transaction git update-ref refs/heads/created topic main
  record reference-transaction git symbolic-ref refs/heads/symbolic refs/heads/main
  printf 'start\nupdate refs/heads/aborted %s\nprepare\nabort\n' "$(cat "$root/oids/main")" |
    record reference-transaction git update-ref --stdin
  record reference-transaction git update-ref -d refs/heads/created
)
remove_hooks repo/.git/hooks

# A bare repository: Git exports `GIT_DIR=.` and `GIT_IMPLICIT_WORK_TREE=0`, and hooks run in the repository.
install_hooks remote.git/hooks reference-transaction
(cd remote.git && oid bare main)
record bare git -C remote.git update-ref refs/heads/bare main
remove_hooks remote.git/hooks

# `git worktree add`, then a commit in the linked worktree, which shares the hooks of the main worktree, and for which
# Git exports the Git directory of the linked worktree as `GIT_DIR`.
install_hooks repo/.git/hooks post-checkout pre-commit
(cd repo && oid linked linked)
record worktree-add git -C repo worktree add -q ../linked linked
(cd linked
  echo linked >linked-file
  git add linked-file
  record linked git commit -q -m linked
)
remove_hooks repo/.git/hooks

# `git clone`, with the hook installed by the template, exports the real path of the new Git directory as `GIT_DIR`.
install_hooks template/hooks post-checkout
(cd repo && oid clone-head HEAD)
record clone git clone -q -c core.longpaths=true --template=template repo cloned

# A relative `core.hooksPath` is relative to the root of the worktree, where the hook runs, even if Git is started in
# a subdirectory, which Git exports as `GIT_PREFIX`.
git init -q hooks-path
(cd hooks-path
  git config core.hooksPath custom-hooks
  install_hooks custom-hooks pre-commit
  mkdir sub
  echo sub >sub/file
  git add sub/file
  cd sub
  record hooks-path git commit -q -m sub
)

# An optional `core.hooksPath` that doesn't exist leaves the hooks directory of the repository.
git init -q optional-hooks-path
(cd optional-hooks-path
  git config core.hooksPath ':(optional)missing-hooks'
  install_hooks .git/hooks pre-commit
  echo file >file
  git add file
  record optional-hooks-path git commit -q -m optional
)

# Configured hooks run before the hook of the hooks directory, in the order of the configuration: a shell command,
# a command found in `PATH`, and a disabled hook. The configured hooks of `post-commit` are disabled as event.
# `git hook list` shows how Git lists them, and `config` the configuration of hooks as Git reads it.
git init -q configured
(cd configured
  git config hook.one.event pre-commit
  git config hook.one.command 'HOOK_NAME=one "$HOOK_RECORDER"'
  git config hook.path.event pre-commit
  git config hook.path.command record-path
  git config hook.off.event pre-commit
  git config hook.off.command 'HOOK_NAME=off "$HOOK_RECORDER"'
  git config hook.off.enabled false
  git config hook.notified.event post-commit
  git config hook.notified.command 'HOOK_NAME=notified "$HOOK_RECORDER"'
  git config hook.post-commit.enabled false
  install_hooks .git/hooks pre-commit post-commit
  mkdir -p "$root/records/configured"
  git config --get-regexp '^hook\.' >"$root/records/configured/config"
  git hook list pre-commit >"$root/records/configured/list-pre-commit"
  git hook list post-commit >"$root/records/configured/list-post-commit"
  echo file >file
  git add file
  export HOOK_RECORDER=$root/record-hook PATH=$root/bin:$PATH
  record configured git commit -q -m configured
)

# How Git reads the configuration of hooks, as `git hook list` shows it: adding a hook to an event again moves it to
# the end, with the last hook of the event taking its place, an empty event removes a hook from all events, `enabled`
# can be set back, a disabled hook needs no command, and `hook.<event>.enabled` doesn't disable an event that is also
# the name of a hook, which `parallel` set to true makes it.
git init -q config-rules
(cd config-rules
  cat >>.git/config <<'EOF'
[hook "a"]
	command = true a
	event = pre-commit
[hook "b"]
	command = true b
	event = pre-commit
[hook "c"]
	command = true c
	event = pre-commit
	event = post-commit
[hook "d"]
	command = true d
	event = pre-commit
[hook "b"]
	event = pre-commit
[hook "c"]
	event =
[hook "d"]
	enabled = false
	enabled = true
[hook "e"]
	event = pre-commit
	enabled = false
[hook "pushed"]
	command = true pushed
	event = pre-push
[hook "pre-push"]
	parallel = true
	enabled = false
EOF
  mkdir -p "$root/records/config-rules"
  git config --get-regexp '^hook\.' >"$root/records/config-rules/config"
  for event in pre-commit post-commit pre-push; do
    git hook list "$event" >"$root/records/config-rules/list-$event" 2>/dev/null || true
  done
)

# Configurations of hooks that Git refuses, with the message it gives in `<case>`, and the configuration in
# `<case>-config`: a hook with event but without command, a hook named like an event, and a key without value.
mkdir -p records/config-errors
git init -q config-error-missing-command
git -C config-error-missing-command config hook.x.event pre-commit
git init -q config-error-known-name
git -C config-error-known-name config hook.pre-commit.command true
git -C config-error-known-name config hook.pre-commit.event pre-commit
git init -q config-error-missing-value
printf '[hook "x"]\n\tcommand\n' >>config-error-missing-value/.git/config
for case in missing-command known-name missing-value; do
  (cd "config-error-$case"
    git config --get-regexp '^hook\.' >"$root/records/config-errors/$case-config"
    if git hook list pre-commit 2>"$root/records/config-errors/$case"; then false; fi
  )
done

# `git hook run` with arguments and input from a file: hooks that fail don't stop the others, and their exit codes
# are combined, but a hook that can't be started stops the hooks after it.
git init -q exit-codes
(cd exit-codes
  git config hook.first.event pre-commit
  git config hook.first.command 'HOOK_NAME=first HOOK_EXIT=1 "$HOOK_RECORDER"'
  git config hook.second.event pre-commit
  git config hook.second.command 'HOOK_NAME=second HOOK_EXIT=2 "$HOOK_RECORDER"'
  git config hook.missing.event pre-commit
  git config hook.missing.command missing-program-of-a-hook
  mkdir -p "$root/records/exit-codes"
  git config --get-regexp '^hook\.' >"$root/records/exit-codes/config"
  install_hooks .git/hooks pre-commit
  echo input >input-of-hooks
  export HOOK_RECORDER=$root/record-hook
  status=0
  record exit-codes git hook run --to-stdin=input-of-hooks pre-commit -- "first argument" second || status=$?
  echo "$status" >"$root/records/exit-codes/status"
)

# A hook without `#!` line runs with the shell on Unix, but fails to start in Git for Windows.
git init -q no-shebang
(cd no-shebang
  sed 1d "$root/record-hook" >.git/hooks/pre-commit
  chmod +x .git/hooks/pre-commit
  mkdir -p "$root/records/no-shebang"
  status=0
  record no-shebang git hook run pre-commit || status=$?
  echo "$status" >"$root/records/no-shebang/status"
)

# A hook that isn't executable is ignored on Unix, with advice, but Git for Windows runs it.
git init -q not-executable
(cd not-executable
  install_hooks .git/hooks pre-commit
  chmod -x .git/hooks/pre-commit
  mkdir -p "$root/records/not-executable"
  record not-executable git hook run --ignore-missing pre-commit
)

# The tests run the hooks of each scenario again, where Git found them.
install_hooks repo/.git/hooks pre-commit prepare-commit-msg commit-msg post-commit post-rewrite pre-merge-commit \
  post-merge post-checkout pre-rebase pre-push reference-transaction
install_hooks remote.git/hooks reference-transaction
