#!/usr/bin/env bash
set -eu -o pipefail

# `source` has three commits. The first is tagged `base`, the second `v1` with an annotated tag. `large` changes in one
# line in the second commit, so that the bundles of `base..main` store it as a delta of `base:large`, which only a
# repository that has `base` can resolve.
git init -q source
(
  cd source
  seq 1 2000 > large
  echo a > a
  git add large a
  git commit -q -m first
  git tag base

  { seq 1 999; echo changed; seq 1001 2000; } > large
  mkdir dir
  echo c > dir/c
  git add large dir
  git commit -q -m second
  git tag -a -m "version 1" v1

  echo d > d
  git add d
  git commit -q -m third
)
git -C source rev-parse base 'base^{tree}' base:large main v1 > source.revs

# Bundles as git writes them.
git -C source bundle create -q ../full.bundle main
git -C source bundle create -q ../incremental.bundle base..main
git -C source bundle create -q ../with-tag.bundle base..main v1
git -C source bundle create -q --version=3 ../incremental-v3.bundle base..main
git -C source bundle create -q ../filtered.bundle --filter=blob:none main

# Bundles that git doesn't write: one that continues after its pack, and one whose pack lacks the tree of its commit.
{ cat incremental.bundle; printf 'junk'; } > junk.bundle
object_format=$(git -C source rev-parse --show-object-format)
commit=$(git -C source commit-tree -m 'packed without its tree' 'main^{tree}')
{
  if [ "$object_format" = sha1 ]; then
    printf '# v2 git bundle\n'
  else
    printf '# v3 git bundle\n@object-format=%s\n' "$object_format"
  fi
  printf '%s refs/heads/main\n\n' "$commit"
  echo "$commit" | git -C source pack-objects -q --stdout
} > missing-tree.bundle

# The references of each bundle as `git bundle list-heads` reads them, and all objects reachable from them.
bundles="full incremental with-tag incremental-v3 filtered junk missing-tree"
for bundle in $bundles; do
  git -C source bundle list-heads "../$bundle.bundle" > "$bundle.heads"
  cut -d ' ' -f 1 "$bundle.heads" | git -C source rev-list --objects --stdin > "$bundle.objects"
done

# Repositories that receive the bundles: one with the history of `base`, one without objects, and one with the commit
# `base` but none of the objects it references. `<receiver>.tips` lists what their references point to.
git init -q has-base
git -C has-base fetch -q --no-tags ../source refs/tags/base:refs/tags/base
git init -q empty
git init -q unconnected
git -C source cat-file commit base | git -C unconnected hash-object -t commit -w --stdin > /dev/null
receivers="has-base empty unconnected"
for receiver in $receivers; do
  git -C "$receiver" rev-parse --all > "$receiver.tips"
done

# Whether `git bundle verify` and `git fetch` accept each bundle in each receiver, as `<receiver> <bundle> <verify> <fetch>`.
for receiver in $receivers; do
  for bundle in $bundles; do
    if git -C "$receiver" bundle verify -q "../$bundle.bundle" > /dev/null 2>&1; then verify=ok; else verify=fail; fi
    cp -R "$receiver" fetch-check
    # Fetching from a bundle always keeps a pack, whose path with a SHA-256 name exceeds Windows' traditional path limit
    # in the fixture directory.
    if git -c core.longpaths=true -C fetch-check fetch -q "../$bundle.bundle" 'refs/*:refs/fetched/*' > /dev/null 2>&1; then
      fetch=ok
    else
      fetch=fail
    fi
    rm -rf fetch-check
    echo "$receiver $bundle $verify $fetch" >> git-results
  done
done
