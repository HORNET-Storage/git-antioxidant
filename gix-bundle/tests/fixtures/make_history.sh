#!/usr/bin/env bash
set -eu -o pipefail

# `source` has a merge, a file that is removed and then added again unchanged, commits with a two-line subject, with
# surrounding whitespace and with an empty message, a gitlink, annotated tags, one of which tags another, references to
# a tree and to a blob, and a second root commit. The message of `D` has a NUL byte, where git stops reading it. That of
# `G` starts with blank lines, and a line of whitespace ends its subject. The subject has a tab, a form feed and a
# no-break space, which git keeps, and its first line ends with a space and a carriage return, which git removes.
# `git repack` puts all of its objects into one pack, where the versions of `large` are stored as deltas of each other.
# Only `refs/heads/empty` is committed afterwards: its tree is the empty tree, which git provides without storing it.
# No file is named like a tag, which would make revisions ambiguous on case-insensitive file systems.
git init -q source
(
  cd source
  seq 1 2000 > large
  mkdir -p dir/sub
  echo a1 > top
  echo b1 > dir/b
  echo c1 > dir/sub/c
  git add -A
  git commit -q -m A
  git tag A

  git checkout -q -b side
  echo s1 > s
  git add s
  git commit -q -m '  S1  with  spaces  '
  git tag S1

  git checkout -q main
  echo a2 > top
  { seq 1 999; echo changed; seq 1001 2000; } > large
  git add top large
  git commit -q -m $'B subject\nsecond line of subject\n\nbody'
  git tag B
  git merge -q --no-ff -m 'Merge side' side
  git tag M

  git rm -q dir/b
  # git refuses to write a commit with a NUL byte in its message, but reads one.
  D=$(
    printf 'tree %s\nparent %s\nauthor %s\ncommitter %s\n\nD\0ignored after a NUL byte\n' "$(git write-tree)" \
      "$(git rev-parse HEAD)" "$(git var GIT_AUTHOR_IDENT)" "$(git var GIT_COMMITTER_IDENT)" |
      git hash-object -t commit -w --stdin --literally
  )
  git update-ref HEAD "$D"
  git tag D
  echo b1 > dir/b
  git add dir/b
  git commit -q -m E
  git tag E
  git commit -q --allow-empty --allow-empty-message -m ''
  git tag F

  echo a3 > top
  { seq 1 1999; echo changed again; } > large
  git add top large
  # The gitlink names a commit of another repository, which is neither stored here nor carried by bundles.
  module_commit=$(
    printf 'tree %s\nauthor m <m@example.com> 0 +0000\ncommitter m <m@example.com> 0 +0000\n\nmodule\n' \
      "$(git hash-object -t tree --stdin < /dev/null)" | git hash-object -t commit --stdin
  )
  git update-index --add --cacheinfo "160000,$module_commit,module"
  printf '\n \t\nG\tsubject\f \r\nsecond line\302\240\n \t\nbody\n' | git commit -q --cleanup=verbatim -F -
  git tag G
  echo a4 > top
  git add top
  git commit -q -m H
  git tag H

  git tag -a -m 'tag v1' v1 B
  git -c advice.nestedTag=false tag -a -m 'tag of tag' v1-outer v1
  git tag tree-tag 'A^{tree}'
  git tag blob-tag A:top

  git checkout -q --orphan lone
  git rm -q -r -f --cached .
  echo lone > lone
  git add lone
  git commit -q -m Lone
  git tag lone-root
  git tag -a -m 'tag of lone' lone-tag lone-root
  git checkout -q -f main

  git repack -q -a -d -f
  git update-ref refs/heads/empty "$(git commit-tree -m 'empty tree' "$(git hash-object -t tree --stdin < /dev/null)")"
)

# Objects the tests name, as `<id> <revision>`.
for revision in A B H lone-root A:dir A:dir/b; do
  echo "$(git -C source rev-parse "$revision") $revision"
done > source.revs

# bundle_case <name> <receiver> <references> [<hidden commits>] records what `git bundle create` makes of the
# references for repositories that have the hidden commits with their history, like `<receiver>`: `<name>.bundle`, unless
# git refuses to write it, the references as `<id> <name>` and the hidden commits in `<name>.refs` and `<name>.hidden`,
# and the objects that `git rev-list --objects` lists for both in `<name>.objects`. `version` selects the version of the
# bundle format. Each case is added to `cases` as `<name> <receiver> <version>`, with `default` for the version that
# git chooses.
bundle_case() {
  local name=$1 receiver=$2 references=$3 hidden=${4:-} reference commit
  for reference in $references; do
    echo "$(git -C source rev-parse "$reference") $(git -C source rev-parse --symbolic-full-name "$reference")"
  done > "$name.refs"
  for commit in $hidden; do
    git -C source rev-parse "$commit^{commit}"
  done > "$name.hidden"
  # shellcheck disable=SC2086 # The lists are meant to be split into arguments.
  git -C source rev-list --objects $references --not $hidden | cut -d ' ' -f 1 > "$name.objects"
  # shellcheck disable=SC2086
  git -C source bundle create -q ${version:+--version=$version} "../$name.bundle" $references --not $hidden 2> /dev/null || true
  echo "$name $receiver ${version:-default}" >> cases
}

bundle_case full empty main
bundle_case incremental-B has-B main B
bundle_case incremental-M has-M main M
bundle_case incremental-D has-D main D
bundle_case incremental-S1 has-S1 main S1
bundle_case incremental-F has-F main F
bundle_case incremental-G has-G main G
bundle_case tag-at-boundary has-B 'main v1' B
bundle_case tag-of-tag has-A v1-outer A
version=3 bundle_case v3 has-M main M
bundle_case tree-ref empty refs/tags/tree-tag
bundle_case blob-ref empty refs/tags/blob-tag
bundle_case two-heads has-A 'main side' A
bundle_case empty-tree empty refs/heads/empty
bundle_case lone empty lone-tag

# References into the history of the hidden commits, for which git writes other bundles than `gix-bundle`: git leaves
# out references to hidden commits, which it also refuses as the only reference, and doesn't require the commits that
# tags of other references point to.
bundle_case tag-in-history has-M 'main v1' M
bundle_case tag-unrelated has-lone-root 'main lone-tag' lone-root
bundle_case ref-at-boundary has-S1 'main side' side
bundle_case ref-is-hidden has-H main main
bundle_case ref-in-history has-D 'E A' D

# Repositories that receive the bundles: one without objects and one with the history of each hidden commit.
# `<receiver>.tips` lists what their references point to.
git init -q empty
for tag in A B D F G H M S1 lone-root; do
  git init -q "has-$tag"
  git -C "has-$tag" fetch -q --no-tags ../source "refs/tags/$tag:refs/tags/$tag"
done
for receiver in empty has-A has-B has-D has-F has-G has-H has-M has-S1 has-lone-root; do
  git -C "$receiver" rev-parse --all > "$receiver.tips"
done

# A repository whose only commit lacks its blob.
git init -q broken
(
  cd broken
  echo lost > lost
  git add lost
  git commit -q -m 'lost blob'
  blob=$(git rev-parse HEAD:lost)
  rm -f ".git/objects/${blob:0:2}/${blob:2}"
)
git -C broken rev-parse --all > broken.tips
