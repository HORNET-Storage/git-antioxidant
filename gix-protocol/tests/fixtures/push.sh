#!/usr/bin/env bash
set -eu -o pipefail

# Record pushes of `git push` into bare repositories: what it sends to `git receive-pack` and receives from it, what
# `git push --porcelain` reports about each reference, and the references of the remote afterwards.
#
# Each case `<case>` has a directory with
#  - `request`: what `git push` sent, the commands followed by push options and the pack,
#  - `response`: what `git receive-pack` sent, the advertisement of its references followed by the status report,
#  - `porcelain`: the output of `git push --porcelain`,
#  - `refs-after`: the references of the remote after the push, as `<object> <name>`,
# along with `<case>.git`, the remote `git push` pushed into, and `<case>-before.git`, a copy of the remote before the
# push at the same depth, so the relative path to its alternate object database remains valid.

# Both sides identify with this agent.
export GIT_USER_AGENT=git/gix-fixture

# Run `git receive-pack` while keeping copies of its input and output in the directory named by `RECORD_DIR`.
cat > record-receive-pack <<'EOF'
#!/usr/bin/env bash
set -eu -o pipefail
tee "$RECORD_DIR/request" | git receive-pack "$@" | tee "$RECORD_DIR/response"
EOF
chmod +x record-receive-pack

git init -q local
(cd local
  git commit -q --allow-empty -m c1
  git tag -m annotated v1
  git commit -q --allow-empty -m c2
  git checkout -q -b side main~1
  git commit -q --allow-empty -m s1
  git checkout -q main
  git commit -q --allow-empty -m c3
)

# The names of loose SHA-256 objects received into deeply nested fixture directories exceed what Windows allows unless
# Git for Windows uses long paths. Other Git builds ignore the setting.
git init -q --bare empty.git
git -C empty.git config core.longPaths true
git init -q --bare remote.git
git -C remote.git config core.longPaths true
git -C local push -q ../remote.git main~1:refs/heads/main main~2:refs/heads/doomed main~2:refs/heads/stays
git init -q --bare with-alternates.git
git -C with-alternates.git config core.longPaths true
echo ../../remote.git/objects > with-alternates.git/objects/info/alternates

# Prepare the remote of case `$1` from the repository `$2`, with configuration given as further pairs of arguments.
function remote() {
  local name=$1 template=$2
  shift 2
  cp -R "$template" "$name.git"
  while [ $# -gt 0 ]; do
    git -C "$name.git" config "$1" "$2"
    shift 2
  done
  cp -R "$name.git" "$name-before.git"
}

# Push from `local` into the remote of case `$1` with the further arguments of `git push`, and record the exchange.
function record() {
  local name=$1
  shift
  mkdir "$name"
  RECORD_DIR="$PWD/$name" git -C local push --porcelain --receive-pack="$PWD/record-receive-pack" \
    "../$name.git" "$@" > "$name/porcelain" 2> "$name/stderr" || :
  git -C "$name.git" for-each-ref --format='%(objectname) %(refname)' > "$name/refs-after"
}

# An empty remote advertises its capabilities with `capabilities^{}`.
remote create empty.git
record create main refs/tags/v1

remote update remote.git
record update main side:refs/heads/new :refs/heads/doomed

remote delete-only remote.git
record delete-only :refs/heads/doomed :refs/heads/stays

remote rejected remote.git receive.denyNonFastForwards true
record rejected +main~2:refs/heads/main side:refs/heads/new

remote atomic remote.git receive.denyNonFastForwards true
record atomic --atomic +main~2:refs/heads/main side:refs/heads/new

remote push-options remote.git receive.advertisePushOptions true
record push-options --push-option=first --push-option="second option" main

# The remote advertises the objects of the references of its alternate object database with `.have`.
remote alternates with-alternates.git
record alternates main
