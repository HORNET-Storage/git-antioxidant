#!/usr/bin/env bash
set -eu -o pipefail

gix_testtools_require_symlinks

# Make a link at `$2` with the text `$1` that leads nowhere. Native Windows links are made for an existing target,
# so a file stands in for the target while the link is created.
function link_to_nothing() {
  local target
  target=$(dirname "$2")/$1
  mkdir -p "$(dirname "$target")"
  touch "$target"
  ln -s "$1" "$2"
  rm "$target"
}

# Print the references of the current repository as Git lists them.
function listing() {
  git for-each-ref --format='%(refname) %(objectname) %(symref)'
}

# Print each reference named by an argument as Git reads it: the object it resolves to and the reference it names
# if it is symbolic, with `-` for neither.
function reads() {
  local name oid target
  for name in "$@"; do
    oid=$(git rev-parse --verify --quiet "$name" 2>/dev/null) || oid=-
    target=$(git symbolic-ref --quiet "$name" 2>/dev/null) || target=-
    echo "$name $oid $target"
  done
}

# Git reads a link in the reference store as a symbolic reference if its text is a valid reference name in refs/,
# wherever that text leads as a path, and reads other links through. It lists what the links lead to: a file is a
# reference, a directory is walked, and a link that leads nowhere is skipped.
git init -q -b main repo
(
  cd repo
  git commit -q --allow-empty -m one
  git commit -q --allow-empty -m two
  one=$(git rev-parse HEAD~1)
  two=$(git rev-parse HEAD)
  git branch other "$one"
  git branch both "$two"
  # A linked directory is ordered as a directory, between these two branches.
  git branch dir-link-2 "$one"
  git branch dir-link0 "$two"
  rm .git/HEAD
  ln -s refs/heads/main .git/HEAD
  (
    cd .git/refs/heads
    # As paths, texts like refs/heads/both lead to files in refs/heads/refs/heads, which hold other objects than the
    # branches that the texts name.
    mkdir -p refs/heads
    echo "$one" >refs/heads/both
    echo "$one" >refs/heads/nowhere-ref
    echo "$two" >refs/heads/bad..name
    link_to_nothing refs/heads/main text-main
    link_to_nothing refs/heads/missing text-to-missing
    ln -s refs/heads/both text-both
    ln -s refs/heads/nowhere-ref text-and-path
    # This text starts like a reference name in refs/, but isn't a valid one.
    ln -s refs/heads/bad..name text-invalid
    ln -s main path-main
    ln -s ../../refs/heads/other path-other
    link_to_nothing nowhere dangling
    mkdir -p ../../../outside/sub
    echo "$one" >../../../outside/sub/x
    ln -s ../../../outside dir-link
  )
  listing >../repo.listing
  reads HEAD refs/heads/main refs/heads/other refs/heads/both \
    refs/heads/text-main refs/heads/text-to-missing refs/heads/text-both refs/heads/text-and-path refs/heads/text-invalid \
    refs/heads/path-main refs/heads/path-other refs/heads/dangling refs/heads/dir-link refs/heads/dir-link/sub/x \
    refs/heads/refs/heads/both refs/heads/refs/heads/nowhere-ref refs/heads/dir-link-2 refs/heads/dir-link0 \
    >../repo.reads
)

# Loose references below a linked directory are merged with packed references in name order.
git init -q -b main packed
(
  cd packed
  git commit -q --allow-empty -m one
  git commit -q --allow-empty -m two
  one=$(git rev-parse HEAD~1)
  git branch dir-link-2 "$one"
  git branch dir-link0 "$one"
  git pack-refs --all --prune
  # This loose reference replaces the packed one of the same name.
  git update-ref refs/heads/dir-link-2 HEAD
  mkdir -p outside/sub
  echo "$one" >outside/sub/x
  ln -s ../../../outside .git/refs/heads/dir-link
  listing >../packed.listing
)

# A link to a directory that contains it leads back into the walk.
git init -q -b main loop
git -C loop commit -q --allow-empty -m one
ln -s .. loop/.git/refs/heads/loop
