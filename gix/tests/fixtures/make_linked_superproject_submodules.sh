#!/usr/bin/env bash
set -eu -o pipefail

git init -q module1
(cd module1
  touch this
  git add . && git commit -q -m c1
)

# Short names keep each linked superproject's private submodule Git directory within Git for
# Windows' limit for `$GIT_DIR`, 40 bytes below its `PATH_MAX` of 260, which `core.longpaths`
# doesn't lift.
git init -q host
git -C host submodule add ../module1 m1
git -C host/m1 worktree add ../../worktree-of-submodule
git -C host commit -q -m record-submodule

# Each linked superproject owns its submodule repository, while the main
# superproject's submodule stays populated. Freeze each lifecycle state so
# read-only tests never initialize or deinitialize a shared fixture.
for state in uninitialized initialized deinitialized; do
  git -C host worktree add --detach "../$state" HEAD
  if [[ "$state" != uninitialized ]]; then
    # Private worktree submodule object paths exceed Windows' traditional path limit.
    git -c core.longpaths=true -C "$state" submodule update --init --recursive
  fi
  if [[ "$state" == deinitialized ]]; then
    git -C "$state" submodule deinit -f m1
  fi
done
