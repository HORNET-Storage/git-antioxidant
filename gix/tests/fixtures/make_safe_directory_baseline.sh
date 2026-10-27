#!/usr/bin/env bash
set -eu -o pipefail

# Repositories found in all the ways Git finds them, and for each case the `safe.directory` values and the directory
# from which Git looks for the repository, along with whether Git opens it. `GIT_TEST_ASSUME_DIFFERENT_OWNER` makes Git
# consider no repository owned by the current user, so `safe.directory` decides. As the values are absolute paths of
# this host, the results are recorded on each host instead of being archived.
#
# `baseline` has a line `<case> <open|refused> <start>` for each case, and `cases/<case>.config` is the global
# configuration of a case. `cases/<case>.env` lists additional environment variables as `NAME=value`, and
# `cases/<case>.cli` the values Git receives with `-c`. `cases/<case>.err` is what Git printed.

# Keep Git Bash from turning arguments that look like Unix paths into Windows paths.
export MSYS_NO_PATHCONV=1

# The root of the fixture, resolved, and spelled like Git: with a drive letter and `/` on Windows.
root=$(pwd -P)
on_windows=
if root_on_windows=$(pwd -W 2>/dev/null); then
  root=$root_on_windows
  on_windows=1
fi

git init -q repo
git -C repo commit -q --allow-empty -m initial
mkdir repo/sub
git -C repo worktree add -q ../wt
git init -q --bare bare.git
git init -q cw
mkdir cw-worktree
git -C cw config core.worktree "$root/cw-worktree"
git init -q local
git -C local config safe.directory "$root/local"
mkdir home

mkdir cases
case_env=()
case_cli=()
# record <case> <start> [<value>...] records whether Git opens the repository it finds from <start>, a directory relative
# to the root, with each <value> of `safe.directory` added to the global configuration of the case. The configuration
# may have been started already, and `case_env` and `case_cli` are passed to Git as well.
record() {
  local name=$1 start=$2
  shift 2
  local config=cases/$name.config value
  [ -e "$config" ] || : >"$config"
  for value in "$@"; do
    git config -f "$config" --add safe.directory "$value"
  done
  printf '%s\n' "${case_env[@]+"${case_env[@]}"}" >"cases/$name.env"
  printf '%s\n' "${case_cli[@]+"${case_cli[@]}"}" >"cases/$name.cli"
  local cli=()
  for value in "${case_cli[@]+"${case_cli[@]}"}"; do
    cli+=(-c "$value")
  done
  local verdict=refused
  if (cd "$start" && env GIT_TEST_ASSUME_DIFFERENT_OWNER=1 GIT_CONFIG_GLOBAL="$root/$config" \
    "${case_env[@]+"${case_env[@]}"}" git "${cli[@]+"${cli[@]}"}" rev-parse --git-dir) >/dev/null 2>"cases/$name.err"; then
    verdict=open
  fi
  echo "$name $verdict $start" >>baseline
  case_env=()
  case_cli=()
}

# The values in the order they appear, as in Git's t0033-safe-directory.sh.
record unset repo
record match repo "$root/repo"
record no-match repo "$root/other"
record match-then-reset repo "$root/repo" ""
record reset-then-match repo "" "$root/repo"
record match-then-other repo "$root/repo" "$root/other"
record asterisk repo "*"
record asterisk-then-reset repo "*" ""
record missing-then-match repo "$root/missing" "$root/repo"

# Values ending in `/*` allow the directories within, but not the directory itself.
record glob-parent repo "$root/*"
record glob-repository-itself repo "$root/repo/*"
record glob-without-slash repo "$root*"
record glob-unmatching repo "${root}no/*"
record glob-with-trailing-slash repo "$root/*/"
record glob-in-the-middle repo "$root/*/repo"
record glob-of-missing-directory repo "$root/missing/*"

# Values are resolved, but must be absolute and exist, and so must the repository.
record trailing-slash repo "$root/repo/"
record dot-component repo "$root/./repo"
record dot-dot-component repo "$root/repo/sub/.."
record relative repo repo
record missing repo "$root/missing"

# The directory that Git compares is where the `.git` directory was found, or the git directory if Git starts in it.
record from-subdirectory repo/sub "$root/repo"
record subdirectory-from-subdirectory repo/sub "$root/repo/sub"
record git-dir-from-worktree repo "$root/repo/.git"
record worktree-from-git-dir repo/.git "$root/repo"
record git-dir-from-git-dir repo/.git "$root/repo/.git"
record glob-from-git-dir repo/.git "$root/repo/*"
record git-dir-from-refs repo/.git/refs "$root/repo/.git"

# `.` is the current directory, which must be the directory Git compares.
record dot repo .
record dot-from-subdirectory repo/sub .
record dot-from-git-dir repo/.git .
record dot-from-refs repo/.git/refs .

# Paths are interpolated, and `:(optional)` paths are skipped if they are missing.
record optional repo ":(optional)$root/repo"
record optional-missing repo ":(optional)$root/missing"
record optional-missing-then-match repo ":(optional)$root/missing" "$root/repo"
case_env=("HOME=$root")
record home repo "~/repo"
case_env=("HOME=$root/repo")
record home-alone repo "~"
record unknown-user-then-match repo "~no-such-user-of-gix/repo" "$root/repo"
record match-then-unknown-user repo "$root/repo" "~no-such-user-of-gix/repo"
record prefix-with-absolute-path repo "%(prefix)/$root/repo"
record prefix-with-relative-path repo "%(prefix)/repo"

# Bare repositories, linked worktrees, and worktrees configured with `core.worktree`.
record bare-unset bare.git
record bare bare.git "$root/bare.git"
record bare-glob bare.git "$root/*"
record linked-worktree wt "$root/wt"
record linked-worktree-main-worktree wt "$root/repo"
record linked-worktree-git-dir wt "$root/repo/.git/worktrees/wt"
record core-worktree cw "$root/cw"
record core-worktree-configured cw "$root/cw-worktree"

# Values are only read from protected configuration, which is everything but the repository's own configuration.
record local-configuration local
case_env=(GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=safe.directory "GIT_CONFIG_VALUE_0=$root/repo")
record environment repo
case_cli=("safe.directory=$root/repo")
record command-line repo
git config -f cases/system-scope.system safe.directory "$root/repo"
case_env=(GIT_CONFIG_NOSYSTEM=0 "GIT_CONFIG_SYSTEM=$root/cases/system-scope.system")
record system-scope repo
git config -f cases/system-scope-disabled.system safe.directory "$root/repo"
case_env=(GIT_CONFIG_NOSYSTEM=1 "GIT_CONFIG_SYSTEM=$root/cases/system-scope-disabled.system")
record system-scope-disabled repo
git config -f cases/included.include safe.directory "$root/repo"
git config -f cases/include.config include.path "$root/cases/included.include"
record include repo
git config -f cases/include-if-gitdir.config "includeIf.gitdir:$root/repo/.path" "$root/cases/included.include"
record include-if-gitdir repo
git config -f cases/include-if-onbranch.config includeIf.onbranch:main.path "$root/cases/included.include"
record include-if-onbranch repo

# On Windows, Git resolves existing paths with the file system, which ignores the case of letters and accepts both
# separators. Values ending in `/*` aren't valid names, so their spelling is compared as is.
if [ -n "$on_windows" ]; then
  root_with_backslashes=${root//\//\\}
  lowercase_drive=$(printf '%s' "${root:0:1}" | tr '[:upper:]' '[:lower:]')${root:1}
  record backslashes repo "$root_with_backslashes\\repo"
  record lowercase-drive repo "$lowercase_drive/repo"
  record uppercase-name repo "$root/REPO"
  record glob-with-backslash repo "$root_with_backslashes\\*"
  record glob-with-lowercase-drive repo "$lowercase_drive/*"
fi
