#!/usr/bin/env bash
set -eu -o pipefail

# `git push --porcelain` in many situations, to compare pushing with `gix` with.
#
# Each case is a directory with `before/remote.git`, a bare repository, and `before/local`, whose remote `origin` is
# `../remote.git`, both prepared as the case needs. `git push --porcelain` ran in `local` of a copy of `before`, with
# the arguments in `args`, one per line, and the case directory records:
#
# - `porcelain` and `stderr`: what it printed, and `exit-code`: its exit code.
# - `remote-refs`: the references of the remote afterwards, as `<object> <name>` lines.
# - `tracking-refs`: the remote-tracking references of `local` afterwards, as `<object> <name> <message>` lines with
#   the message of their newest reflog entry.
# - `pre-push`: the name of the remote and the input that the `pre-push` hook of `local` received, if it has the hook.
# - `push-options`: the number and the values of the push options that the `pre-receive` hook of the remote received,
#   if it has the hook.
#
# `before` stays as it was, so pushing from it with the same arguments has to lead to the same records. Relative URLs
# like `../remote.git` are those of the remotes of the case, as seen from `local`. The remotes set `daemon.receivepack`,
# so that `git daemon` lets the tests push to them over `git://` as well.

function tick () {
  if test -z "${tick+set}"; then
    tick=1112911993
  else
    tick=$((tick + 60))
  fi
  GIT_COMMITTER_DATE="$tick -0700"
  GIT_AUTHOR_DATE="$tick -0700"
  export GIT_COMMITTER_DATE GIT_AUTHOR_DATE
}

GIT_AUTHOR_NAME='A U Thor'
GIT_AUTHOR_EMAIL=author@example.com
GIT_COMMITTER_NAME='C O Mitter'
GIT_COMMITTER_EMAIL=committer@example.com
export GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL

# Git for Windows needs `core.longPaths` for the objects written deep below the directory of the fixture. Git passes
# the configuration of its environment to the programs it starts, but not to those that work in a remote on the same
# machine, like `git receive-pack`, so the remotes have it in their configuration as well. Other systems ignore it.
export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.longPaths GIT_CONFIG_VALUE_0=true

# Commit the file `$2.t` with the message `$2` in the repository `$1`.
function commit () {
  tick
  echo "$2" > "$1/$2.t"
  git -C "$1" add "$2.t"
  git -C "$1" commit -q -m "$2"
}

# Create the annotated tag `$2` of `HEAD` in the repository `$1`.
function annotated_tag () {
  tick
  git -C "$1" tag -a -m "$2" "$2"
}

# The repositories each case starts from: the bare `remote.git`, and `local`, which committed `A` and `B` on `main` and
# pushed them to its remote `origin`, so that `main` tracks `origin/main`. Fetching doesn't create
# `refs/remotes/origin/HEAD`, which leaves the remote-tracking references to those that pushing changes.
git init -q --bare template/remote.git
git -C template/remote.git config daemon.receivepack true
git -C template/remote.git config core.longPaths true
git init -q template/local
git -C template/local remote add origin ../remote.git
git -C template/local config remote.origin.followRemoteHEAD never
commit template/local A
commit template/local B
git -C template/local push -q -u origin main

# Start the case `$1` with a copy of the template as `before`, whose repositories are `$local_repo` and `$remote_repo`.
function start () {
  mkdir "$1"
  cp -R template "$1/before"
  local_repo="$1/before/local"
  remote_repo="$1/before/remote.git"
}

# Commit `$2` on `main` of the remote of the case `$1` from another repository, so that its `local` doesn't have it.
function commit_elsewhere () {
  git clone -q "$1/before/remote.git" "$1/elsewhere"
  commit "$1/elsewhere" "$2"
  git -C "$1/elsewhere" push -q origin main
  rm -rf "$1/elsewhere"
}

# Start the case `$1` with `main` of `local` diverged from `main` of the remote: the remote has the commit `X` on top of
# `B`, which `local` fetched, and `local` has the commit `C` on top of `B`.
function start_diverged () {
  start "$1"
  commit_elsewhere "$1" X
  git -C "$local_repo" fetch -q origin
  commit "$local_repo" C
}

# Install a `pre-push` hook in the repository `$1` that writes the name of the remote and its input to `pre-push` in the
# worktree, where hooks run, and exits with `$2`.
function pre_push_hook () {
  mkdir -p "$1/.git/hooks"
  cat > "$1/.git/hooks/pre-push" <<EOF
#!/bin/sh
{ echo "\$1"; cat; } > pre-push
exit $2
EOF
  chmod +x "$1/.git/hooks/pre-push"
}

# Let the remote `$1` accept push options, and install a `pre-receive` hook that writes the number and the values of
# the push options it receives to `push-options` in the remote, where hooks of a bare repository run.
function push_options_hook () {
  git -C "$1" config receive.advertisePushOptions true
  mkdir -p "$1/hooks"
  cat > "$1/hooks/pre-receive" <<'EOF'
#!/bin/sh
{
  echo "${GIT_PUSH_OPTION_COUNT-none}"
  i=0
  while test "$i" -lt "${GIT_PUSH_OPTION_COUNT-0}"; do
    eval "echo \"\$GIT_PUSH_OPTION_$i\""
    i=$((i + 1))
  done
} > push-options
cat > /dev/null
EOF
  chmod +x "$1/hooks/pre-receive"
}

# Print the remote-tracking references of the repository `$1` with the message of their newest reflog entry.
function tracking_refs () {
  local id name
  git -C "$1" for-each-ref --format='%(objectname) %(refname)' refs/remotes |
    while read -r id name; do
      echo "$id $name $(git -C "$1" reflog show -n1 --format=%gs "$name" -- 2>/dev/null < /dev/null || :)"
    done
}

# Push with `git push --porcelain` and the remaining arguments from `local` in a copy of `before` of the case `$1`, and
# record what it printed and what it changed.
function record () {
  local dir=$1 arg status=0
  shift
  : > "$dir/args"
  for arg in "$@"; do
    printf '%s\n' "$arg" >> "$dir/args"
  done
  cp -R "$dir/before" "$dir/after"
  git -C "$dir/after/local" push --porcelain "$@" > "$dir/porcelain" 2> "$dir/stderr" || status=$?
  echo "$status" > "$dir/exit-code"
  git -C "$dir/after/remote.git" for-each-ref --format='%(objectname) %(refname)' > "$dir/remote-refs"
  tracking_refs "$dir/after/local" > "$dir/tracking-refs"
  if test -f "$dir/after/local/pre-push"; then
    cp "$dir/after/local/pre-push" "$dir/pre-push"
  fi
  if test -f "$dir/after/remote.git/push-options"; then
    cp "$dir/after/remote.git/push-options" "$dir/push-options"
  fi
  rm -rf "$dir/after"
}

# A fast-forward of `main`.
start fast-forward
commit "$local_repo" C
record fast-forward origin main

# A branch the remote doesn't have yet.
start new-branch
git -C "$local_repo" checkout -q -b topic
commit "$local_repo" C
git -C "$local_repo" checkout -q main
record new-branch origin topic

# The remote has a commit `local` doesn't have, so it can't tell if the update is a fast-forward: `fetch first`.
start fetch-first
commit_elsewhere fetch-first X
commit "$local_repo" C
record fetch-first origin main

# Updates that aren't fast-forwards: rejected unless forced by `--force` or `+`, in which case they are reported as
# `forced update`.
start_diverged non-fast-forward
record non-fast-forward origin main
start_diverged force
record force --force origin main
start_diverged force-refspec
record force-refspec origin +main

# Deleting a remote branch with `--delete` and with a refspec without source, which deletes its remote-tracking branch
# as well.
for dir in delete delete-refspec; do
  start $dir
  git -C "$local_repo" push -q origin main:refs/heads/topic
done
record delete --delete origin topic
record delete-refspec origin :refs/heads/topic

# `--delete` and `--tags` can't be combined.
start delete-with-tags
record delete-with-tags --delete --tags origin

# `--tags` pushes lightweight and annotated tags, and `tag <tag>` stands for `refs/tags/<tag>`.
start tags
git -C "$local_repo" tag light main~1
annotated_tag "$local_repo" v1
record tags --tags origin
start tag-shorthand
annotated_tag "$local_repo" v1
record tag-shorthand origin tag v1

# A tag the remote has with another object is only changed by force: `already exists`.
for dir in tag-exists tag-force; do
  start $dir
  git -C "$local_repo" tag t main~1
  git -C "$local_repo" push -q origin t
  git -C "$local_repo" tag -f t main
done
record tag-exists --tags origin
record tag-force --force --tags origin

# `--follow-tags`, or `push.followTags`, also pushes the annotated tags the remote doesn't have that point into what it
# has after the push: `old`, which it didn't have although it has `B`, and `new`, but neither the lightweight `light`
# nor `unpushed-tag`, which points to a commit that isn't pushed.
for dir in follow-tags follow-tags-config; do
  start $dir
  annotated_tag "$local_repo" old
  commit "$local_repo" C
  annotated_tag "$local_repo" new
  git -C "$local_repo" tag light
  git -C "$local_repo" checkout -q -b unpushed
  commit "$local_repo" D
  annotated_tag "$local_repo" unpushed-tag
  git -C "$local_repo" checkout -q main
done
git -C follow-tags-config/before/local config push.followTags true
record follow-tags --follow-tags origin main
record follow-tags-config origin main

# `--force-with-lease` forces an update if the remote reference points to what its remote-tracking reference points to,
# and rejects it as `stale info` if not, unless `--force` is given as well.
start_diverged lease
record lease --force-with-lease origin main
for dir in lease-stale lease-stale-forced; do
  start $dir
  commit_elsewhere $dir X
  commit "$local_repo" C
done
record lease-stale --force-with-lease origin main
record lease-stale-forced --force --force-with-lease origin main

# `--force-with-lease=<ref>:<expect>` expects the given object, here spelled as full id, and an empty `<expect>` that the
# remote reference doesn't exist.
start_diverged lease-value
record lease-value "--force-with-lease=main:$(git -C "$local_repo" rev-parse origin/main)" origin main
start_diverged lease-value-stale
record lease-value-stale "--force-with-lease=main:$(git -C "$local_repo" rev-parse main~1)" origin main
start lease-new
git -C "$local_repo" push -q origin main:refs/heads/topic
git -C "$local_repo" checkout -q -b topic
commit "$local_repo" C
git -C "$local_repo" branch new
record lease-new --force-with-lease=new: --force-with-lease=topic: origin new topic

# The remote is up to date, which still updates the outdated remote-tracking branch.
start up-to-date
git -C "$local_repo" update-ref refs/remotes/origin/main main~1
record up-to-date origin main

# `--atomic` updates all references or none, and fails if the remote doesn't support it.
start atomic
commit "$local_repo" C
git -C "$local_repo" branch topic
record atomic --atomic origin main topic
start_diverged atomic-rejected
git -C "$local_repo" branch topic
record atomic-rejected --atomic origin main topic
start atomic-unsupported
git -C "$remote_repo" config receive.advertiseAtomic false
commit "$local_repo" C
record atomic-unsupported --atomic origin main

# Without refspecs, `push.default` decides what is pushed, and when it's `simple`, as by default, or `upstream`, the
# upstream of the current branch matters.
start default-simple
commit "$local_repo" C
record default-simple
start default-simple-no-upstream
git -C "$local_repo" checkout -q -b feature
commit "$local_repo" F
record default-simple-no-upstream
start default-simple-mismatch
git -C "$local_repo" checkout -q -b feature --track origin/main
commit "$local_repo" F
record default-simple-mismatch
start default-current
git -C "$local_repo" config push.default current
git -C "$local_repo" checkout -q -b feature
commit "$local_repo" F
record default-current
for dir in default-upstream default-upstream-mapped; do
  start $dir
  git -C "$local_repo" config push.default upstream
  git -C "$local_repo" checkout -q -b feature --track origin/main
  commit "$local_repo" F
done
record default-upstream
record default-upstream-mapped origin feature
start default-nothing
git -C "$local_repo" config push.default nothing
record default-nothing
start default-matching
git -C "$local_repo" config push.default matching
git -C "$local_repo" push -q origin main:refs/heads/side
commit "$local_repo" C
git -C "$local_repo" branch topic
record default-matching
start detached
git -C "$local_repo" checkout -q --detach
record detached

# The push refspecs of the remote are used without refspecs, and map the branches that refspecs name.
for dir in remote-push-refspec push-refspec-mapping; do
  start $dir
  git -C "$local_repo" config remote.origin.push refs/heads/main:refs/heads/published
done
record remote-push-refspec
record push-refspec-mapping origin main

# `--dry-run` does everything but send.
start dry-run
commit "$local_repo" C
git -C "$local_repo" push -q origin main:refs/heads/topic
commit "$local_repo" D
record dry-run --dry-run origin main :refs/heads/topic

# Push options reach the hooks of the remote, from `--push-option` or else from `push.pushOption`, where an empty value
# clears those before it. A remote that doesn't support them fails the push.
start push-options
push_options_hook "$remote_repo"
commit "$local_repo" C
record push-options --push-option=first -o second origin main
start push-options-config
push_options_hook "$remote_repo"
git -C "$local_repo" config push.pushOption ignored
git -C "$local_repo" config --add push.pushOption ''
git -C "$local_repo" config --add push.pushOption from-config
commit "$local_repo" C
record push-options-config origin main
start push-options-unsupported
commit "$local_repo" C
record push-options-unsupported -o first origin main

# The `pre-push` hook receives what is pushed but not what is up to date, here `same`, and a failing hook stops the
# push, unless `--no-verify` skips it.
start pre-push
git -C "$local_repo" push -q origin main:refs/heads/topic main:refs/heads/same
pre_push_hook "$local_repo" 0
commit "$local_repo" C
record pre-push origin main :refs/heads/topic main~1:refs/heads/same HEAD:refs/heads/new
for dir in pre-push-fails no-verify; do
  start $dir
  pre_push_hook "$local_repo" 1
  commit "$local_repo" C
done
record pre-push-fails origin main
record no-verify --no-verify origin main

# The remote refuses to delete branches, but takes the update of `main`.
start remote-rejected
git -C "$local_repo" push -q origin main:refs/heads/topic
git -C "$remote_repo" config receive.denyDeletes true
commit "$local_repo" C
record remote-rejected origin main :refs/heads/topic

# A remote without references receives all objects.
mkdir -p empty-remote/before
git init -q --bare empty-remote/before/remote.git
git -C empty-remote/before/remote.git config daemon.receivepack true
git -C empty-remote/before/remote.git config core.longPaths true
git init -q empty-remote/before/local
git -C empty-remote/before/local remote add origin ../remote.git
commit empty-remote/before/local A
record empty-remote origin main

# `local` keeps its objects in a pack, where the smaller version of `big` is likely stored as delta of the version the
# remote has, which may be sent as is in a thin pack, but not with `--no-thin`.
for dir in packed packed-no-thin; do
  start $dir
  seq 1 3000 | sed 's/^/line /' > "$local_repo/big"
  git -C "$local_repo" add big
  tick
  git -C "$local_repo" commit -q -m big
  git -C "$local_repo" push -q origin main
  sed '100,120d' "$local_repo/big" > "$local_repo/big.tmp"
  mv "$local_repo/big.tmp" "$local_repo/big"
  git -C "$local_repo" add big
  tick
  git -C "$local_repo" commit -q -m smaller
  git -C "$local_repo" repack -q -a -d
done
record packed origin main
record packed-no-thin --no-thin origin main

# An object named by a revision instead of a reference, which the remote has already, so no object is sent.
start object-source
commit "$local_repo" C
record object-source origin HEAD~1:refs/heads/older

# `remote.<name>.pushurl` is pushed to instead of `remote.<name>.url`.
start push-url
git -C "$local_repo" config remote.origin.url ../nowhere.git
git -C "$local_repo" config remote.origin.pushurl ../remote.git
commit "$local_repo" C
record push-url origin main

# A URL instead of a remote name: since Git 2.56, the remote-tracking references of the configured remote that pushes
# to that URL, through `remote.<name>.pushurl` if it has one, are updated, unless another configured remote pushes
# there as well. Older Git updates none. `url-ambiguous` and `url-unknown` update none with any Git.
start url-remote
commit "$local_repo" C
record url-remote ../remote.git main

start url-push-url
git -C "$local_repo" config remote.origin.url ../nowhere.git
git -C "$local_repo" config remote.origin.pushurl ../remote.git
commit "$local_repo" C
record url-push-url ../remote.git main

start url-ambiguous
git -C "$local_repo" remote add other ../remote.git
git -C "$local_repo" config remote.other.followRemoteHEAD never
git -C "$local_repo" fetch -q other
commit "$local_repo" C
record url-ambiguous ../remote.git main

start url-unknown
git -C "$local_repo" config remote.origin.pushurl ../nowhere.git
commit "$local_repo" C
record url-unknown ../remote.git main

# Without a remote, `remote.pushDefault` or `branch.<name>.pushRemote` choose the remote over `branch.<name>.remote`.
for dir in push-default-remote branch-push-remote; do
  start $dir
  git -C "$local_repo" remote set-url origin ../nowhere.git
  git -C "$local_repo" remote add pub ../remote.git
  git -C "$local_repo" config remote.pub.followRemoteHEAD never
  git -C "$local_repo" fetch -q pub
  commit "$local_repo" C
done
git -C push-default-remote/before/local config remote.pushDefault pub
git -C branch-push-remote/before/local config branch.main.pushRemote pub
record push-default-remote
record branch-push-remote

# The source of a refspec has to name one local reference or an object.
start src-not-found
record src-not-found origin nonexistent
start src-ambiguous
git -C "$local_repo" branch dup
git -C "$local_repo" tag dup
record src-ambiguous origin dup

# Patterns push all the references they match, except to the remote references that a negative refspec excludes:
# `git push` matches negative refspecs against the names of the remote references, so excluding the source
# `refs/heads/topic` changes nothing, while excluding `refs/heads/mirror/topic` leaves out the update of that reference.
for dir in glob-refspec negative-refspec negative-refspec-source; do
  start $dir
  git -C "$local_repo" branch topic
  commit "$local_repo" C
done
record glob-refspec origin 'refs/heads/*:refs/heads/mirror/*'
record negative-refspec origin 'refs/heads/*:refs/heads/mirror/*' '^refs/heads/mirror/topic'
record negative-refspec-source origin 'refs/heads/*:refs/heads/mirror/*' '^refs/heads/topic'

rm -rf template
