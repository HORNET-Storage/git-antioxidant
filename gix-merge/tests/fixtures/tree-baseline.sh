#!/usr/bin/env bash
set -eu -o pipefail

function tick () {
  if test -z "${tick+set}"
  then
    tick=1112911993
  else
    tick=$(($tick + 60))
  fi
  GIT_COMMITTER_DATE="$tick -0700"
  GIT_AUTHOR_DATE="$tick -0700"
  export GIT_COMMITTER_DATE GIT_AUTHOR_DATE
}

# Translate a historical SHA-1 object id (as hard-coded in the `git update-index
# --index-info` blocks below) to the equivalent id under the repository's
# current hash algorithm, so these baselines work under both SHA-1 and SHA-256.
function oid () {
  case "${GIT_DEFAULT_HASH:-sha1}" in
  sha1)
    printf '%s' "$1"
    return
    ;;
  sha256)
    ;;
  *)
    echo "oid(): unsupported object hash '${GIT_DEFAULT_HASH}', expected 'sha1' or 'sha256'" >&2
    return 1
    ;;
  esac
  case "$1" in
    092bfb9bdf74dd8cfd22e812151281ee9aa6f01a) echo d246bf89e9d0f356301a75184ab79e68cfcb0aac1a7acbf3d4de81400b5ee231 ;;
    09c277aa66897c58157f57a374eacc63a407dcab) echo cf8aedf347c947abf6c6e2c4ecaa2e086ea61ce81f52491c453b81c6455f305e ;;
    0a6a0ba83635bc00e7c79a4b5b6e50381385c1af) echo be45c0023fb9c5e05387235ef07be7e7e2034838216cd24cca06840cc01bc771 ;;
    19102815663d23f8b75a47e7a01965dcdc96468c) echo a65ca4376b51e98097fec3a009b7e5adce3255aecaa1a6da7b1cdc92b90e025e ;;
    1a2664a9924754c698e323f756f9f87f3f2fb337) echo cff0bb6fba5633bd804f0a15fed6329ede0a9dfc4a7f53e31581f3d1a2005e0f ;;
    257cc5642cb1a054f08cc83f2d943e56fd3ebe99) echo 47d6aca82756ff2e61e53520bfdf1faa6c86d933be4854eb34840c57d12e0c85 ;;
    4178ea6795c4c3e07b4e17e6a04aa49584b07ecd) echo 60becdf11d1fcac5bdafbebf1f3e5c7dec1463f8381bcc8bfb533262ec3b1fa8 ;;
    44065282f89b9bd6439ed2e4674721383fd987eb) echo 6632209c4f376d0d893dcb49cd85f0abf9fb1bb0f79c2c6c998adef4d95cac95 ;;
    45b983be36b73c0788dc9cbcb76cbb80fc7bb057) echo 96c18f0297e38d01f4b2dacddea4259aea6b2961eb0822bd2c0c3f6029030045 ;;
    4b5599c7c2ed4390417d9699bec86144a386873d) echo 54b65f98d2bdee38cacc335639121482f497d71b6c9874002b637173f2cc212b ;;
    542802a799ded74fa01c47ba2f8925e284a369e2) echo 863a67a88f3ae07c9c4784fbed16d4f8eabea50aa40923804f7435c45c4bcb1e ;;
    5716ca5987cbf97d6bb54920bea6adde242d87e6) echo a52e146ac2ab2d0efbb768ab8ebd1e98a6055764c81fe424fbae4522f5b4cb92 ;;
    61780798228d17af2d34fce4cfbdf35556832472) echo 9b69d308c97f2c5933fdd0e8ce04acce91c09cb969e36a1f86756fc5a5d3323a ;;
    64012489f118cb4011c8902b4a635f70dcb0c0ca) echo 24bba3b983b179b8ceb5b7af0995c745dbe07f36b75e593b28e000c7d26aa1e5 ;;
    65bc6a1e238f4bf05b28fd05240636e2cfb657e0) echo 315347e306b9d8222324e6f48a1f585074712267eae89d34ef607663046cb3e3 ;;
    78981922613b2afb6025042ff6bd878ac1994e85) echo f8625e43f9e04f24291f77cdbe4c71b3c2a3b0003f60419b3ed06a058d766c8b ;;
    8a1218a1024a212bb3db30becd860315f9f3ac52) echo 6841122c240e69074e82e38506c1fcf806c3ee2469673392a9ed7650d7a6d51d ;;
    9dc97bdc2426e68423360e3e5299280b2cf6b8ff) echo 7108d070fd9fde47d46504102c6935d9251cce816fdbdcc6c1d56710f11790d1 ;;
    a4ae6e4709228b5da6001cb9d1cfa7736851e2a6) echo c3ae6188533b9bea9ad1049892c6b5488fffe5cfc518b30ba21b64a124bc7afe ;;
    b414108e81e5091fe0974a1858b4d0d22b107f70) echo f5f52958fe19d6073227d52208826e0d189bfdb9d0c428984244075155e57c71 ;;
    d0549c3d3c96a464289f3b820b7d96aedc58924b) echo 7960eaf2c5f3ac192aa208323a78bf4b0f2fb21d79813af79a78674698689442 ;;
    df967b96a579e45a18b8251732d16804b2e56a55) echo abed979e3cd3667c5a295c2641f8319f950860c65cc168eebd1571c51bb4f6fc ;;
    e29fa63dae4ccf0788897a7025da868083178fdf) echo 577603f537e595c5324b5987646cb9073d159a4f0c9e73f820d2ee83c8926acf ;;
    e33f5e94470d3b5fa0220ff6a9cabb78a3f72fa3) echo 72a7259eef0664ef8dae2039c11c61ec4dd039f94eee87a21c6ee1c637b7cca0 ;;
    e69de29bb2d1d6434b8b29ae775ad8c2e48c5391) echo 473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813 ;;
    ea28dcd7f627a2a7bbd09daa679c452180617c9f) echo efd6b0b052df02752b7f371021660541dcb8f2b0665c19b672b031af7c086588 ;;
    f286e5cdd97ac6895438ea4548638bb98ac9bd6b) echo 33ba40b57f32a1383518de4f91d9eaf8bf5eb55e318f47215af5c367f9778b9b ;;
    f2ad6c76f0115a6ba5b00456a849810e7ec0af20) echo 2abe107e3b1b618efafa0df5e5f1118e5bf86694eb8c185741e67795ae314aa4 ;;
    f801a62deed900f8a80ff35e3339474ad6352a93) echo 970d16592cccbb8ce3846be7a74ecb94bc3bf8d01bec1eeb15a57380aae5d5a3 ;;
    f89a08d1e226b9a319210641b63b07dcf0bd705f) echo 43d8308eb726995b8b6a3f826252dd21dc35d01386931b8eea4125723e36cc83 ;;
    fa49b077972391ad58037050f2a75f74e3671e92) echo 6d5fd291bb0f67444e99ab492f1bf1fcdf5dca09dab24cf331e05111b4cfc1a3 ;;
    # gitlinks: submodule 'sub' commits (sub-root, sub-a, sub-b), deterministic from the fixed tick dates
    e835c0c403c8e494c0ca98f3d25d0b8464c18d38) echo ed2d65b7fe3c3f869a21cfec7f54867db6c8eb2cc9b0bf671522d87599704d70 ;;
    64466ebdff775ad618d9cc993cf52840e0af528c) echo df5d889015388598b724a741b53437e4dee9d67a5dbe0fa9f361fedd4c9ac6e8 ;;
    ea6eb701e03c2497915c25a851f3da8f8e362ca0) echo c79d38d6c8161bf2aec16032f82127c7ac237eeab69e1490422c93b5c314ce9c ;;
    *) echo "oid(): no SHA-256 mapping for '$1'" >&2; return 1 ;;
  esac
}

function write_lines () {
	printf "%s\n" "$@"
}

function seq () {
	case $# in
	1)	set 1 "$@" ;;
	2)	;;
	*)	{ echo "need 1 or 2 parameters: <end> or <start> <end>" 1>&2 && exit 2; } ;;
	esac
	local seq_counter=$1
	while test "$seq_counter" -le "$2"
	do
		echo "$seq_counter"
		seq_counter=$(( seq_counter + 1 ))
	done
}

function make_conflict_index() {
  local identifier=${1:?The first argument is the name of the parent directory along with the output name}
  cp .git/index .git/"${identifier}".index
}

function make_resolve_tree() {
  local resolve=${1:?Their 'ancestor' or 'ours'}
  local our_side=${2:-}
  local their_side=${3:-}

  local filename="resolve-${our_side}-${their_side}-with-${resolve}"
  git write-tree > ".git/${filename}.tree"
}

# Write a blob with the numbers <from> to <to>, one per line, the line at <position> replaced by
# <text> if given, and print its id.
function numbers_blob () {
  local from=${1:?the first number} to=${2:?the last number} position=${3:-0} text=${4:-}
  local number line=0
  for number in $(seq "$from" "$to"); do
    line=$((line + 1))
    if [ "$line" -eq "$position" ]; then
      echo "$text"
    else
      echo "$number"
    fi
  done | git hash-object -w --stdin
}

# Commit the index with the parents <parent>... and point the branch <name> at that commit.
function commit_index () {
  local name=${1:?the branch to point at the commit} message=${2:?the commit message}
  local parent parents=
  shift 2
  for parent in "$@"; do
    parents="$parents -p $parent"
  done
  git update-ref "refs/heads/$name" "$(git commit-tree $parents -m "$message" "$(git write-tree)")"
}

# Merge the merge-bases A1 and B1 crosswise: A merges B1 into A1 and keeps the tree of A1, and B merges
# A1 into B1 and keeps the tree of B1. Merging A and B then merges A1 and B1 into a virtual merge-base
# first, and the result shows how that merged merge-base resolved their conflicts.
function make_criss_cross_merges () {
  git update-ref refs/heads/A "$(git commit-tree -p A1 -p B1 -m "merge B1 into A1, keep the tree of A1" "A1^{tree}")"
  git update-ref refs/heads/B "$(git commit-tree -p B1 -p A1 -m "merge A1 into B1, keep the tree of B1" "B1^{tree}")"
}

function baseline () (
  local dir=${1:?the directory to enter}
  local output_name=${2:?the basename of the output of the merge}
  local our_committish=${3:?our side from which a commit can be derived}
  local their_committish=${4:?Their side from which a commit can be derived}
  local opt_deviation_message=${5:-}
  local one_side=${6:-}

  cd "$dir"
  local our_commit_id
  local their_commit_id

  local conflict_style="merge"
   if [[ "$output_name" == *-merge ]]; then
       conflict_style="merge"
   elif [[ "$output_name" == *-diff3 ]]; then
       conflict_style="diff3"
   fi
  local directory_renames="conflict"
  if [[ "$output_name" == *-dir-renames-true ]]; then
    directory_renames="true"
  elif [[ "$output_name" == *-dir-renames-false ]]; then
    directory_renames="false"
  fi
  local strategy_option=""
  if [[ "$output_name" == *-X-ours ]]; then
    strategy_option="ours"
  elif [[ "$output_name" == *-X-theirs ]]; then
    strategy_option="theirs"
  fi

  our_commit_id="$(git rev-parse "$our_committish")"
  their_commit_id="$(git rev-parse "$their_committish")"
  local maybe_expected_tree="$(git rev-parse expected^{tree})"
  local maybe_expected_reversed_tree="$(git rev-parse expected-reversed^{tree})"
  if [ "$maybe_expected_reversed_tree" == "expected-reversed^{tree}" ]; then
     maybe_expected_reversed_tree="$(git rev-parse expected^{tree} || :)"
  fi
  if [ -z "$opt_deviation_message" ]; then
    maybe_expected_tree="expected^{tree}"
    maybe_expected_reversed_tree="expected^{tree}"
  fi

  local merge_info="${output_name}.merge-info"
  git -c merge.conflictStyle=$conflict_style -c merge.directoryRenames=$directory_renames merge-tree -z --write-tree --allow-unrelated-histories ${strategy_option:+-X "$strategy_option"} "$our_committish" "$their_committish" > "$merge_info" || :
  echo "$dir" "$conflict_style" "$our_commit_id" "$our_committish" "$their_commit_id" "$their_committish" "$merge_info" "$maybe_expected_tree" "$opt_deviation_message" >> ../baseline.cases

  if [[ "$one_side" != "no-reverse" ]]; then
    local merge_info="${output_name}-reversed.merge-info"
    git -c merge.conflictStyle=$conflict_style -c merge.directoryRenames=$directory_renames merge-tree -z --write-tree --allow-unrelated-histories ${strategy_option:+-X "$strategy_option"} "$their_committish" "$our_committish" > "$merge_info" || :
    echo "$dir" "$conflict_style" "$their_commit_id" "$their_committish" "$our_commit_id" "$our_committish" "$merge_info" "$maybe_expected_reversed_tree" "$opt_deviation_message" >> ../baseline.cases
  fi
)


git init non-tree-to-tree
(cd non-tree-to-tree
  write_lines original 1 2 3 4 5 >a
  git add a && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 6 >a
  git commit -am "'A' changes 'a'"

  git checkout B
  rm a
  mkdir -p a/sub
  touch a/sub/b a/sub/c a/d a/e
  git add a && git commit -m "mv 'a' to 'a/sub/b', populate 'a/' with empty files"
)

git init deleted-file-added-dir
(cd deleted-file-added-dir
  echo original >to-be-deleted
  git add to-be-deleted && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  git rm to-be-deleted
  git commit -m "delete file"

  git checkout B
  git rm to-be-deleted
  mkdir to-be-deleted
  touch to-be-deleted/a
  git add to-be-deleted/a && git commit -m "replace file with directory"
)

git init deleted-file-added-gitlink-directory
(cd deleted-file-added-gitlink-directory
  write_lines original >a
  git add a
  git commit -m "file base"
  base=$(git rev-parse HEAD)

  git branch A
  git branch B

  # Both sides delete `a`; B additionally replaces it with a directory containing
  # a gitlink. The shared deletion and descendant addition are compatible even
  # though the gitlink takes a different structural merge path than a blob.
  git checkout A
  git rm a
  git commit -m "delete a"

  git checkout B
  git rm a
  git update-index --add --cacheinfo 160000,$base,a/a
  git commit -m "replace a with a gitlink directory"
)

git init tree-to-non-tree
(cd tree-to-non-tree
  mkdir -p a/sub
  write_lines original 1 2 3 4 5 >a/sub/b
  touch a/sub/c a/d a/e
  git add a && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 6 >a/sub/b
  git commit -am "'A' changes 'a/sub/b'"

  git checkout B
  rm -Rf a
  echo "new file" > a
  git add a && git commit -m "rm -Rf a/ && add non-empty 'a'"
)

git init non-tree-to-tree-with-rename
(cd non-tree-to-tree-with-rename
  write_lines original 1 2 3 4 5 >a
  git add a && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 6 >a
  git commit -am "'A' changes 'a'"

  git checkout B
  mv a tmp
  mkdir -p a/sub
  mv tmp a/sub/b
  touch a/sub/c a/d a/e
  git add a && git commit -m "mv 'a' to 'a/sub/b', populate 'a/' with empty files"
)

git init tree-to-non-tree-with-rename
(cd tree-to-non-tree-with-rename
  mkdir -p a/sub
  write_lines original 1 2 3 4 5 >a/sub/b
  touch a/sub/c a/d a/e
  git add a && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 6 >a/sub/b
  git commit -am "'A' changes 'a/sub/b'"

  git checkout B
  rm -Rf a
  touch a
  git add a && git commit -m "rm -Rf a/ && add empty 'a' (which is like a rename from an empty deleted file)"
  # And because it's so thrown off, it gets a completely different result if reversed.
  git branch expected-reversed

  rm .git/index
  git update-index --index-info <<EOF
100644 $(oid 44065282f89b9bd6439ed2e4674721383fd987eb) 1	a/sub/b
100644 $(oid b414108e81e5091fe0974a1858b4d0d22b107f70) 2	a/sub/b
100644 $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391) 3	a~B
EOF
  make_conflict_index tree-to-non-tree-with-rename-A-B

  rm .git/index
  git update-index --index-info <<EOF
100644 $(oid 44065282f89b9bd6439ed2e4674721383fd987eb) 1	a/sub/b
100644 $(oid b414108e81e5091fe0974a1858b4d0d22b107f70) 3	a/sub/b
100644 $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391) 2	a~B
EOF
  make_conflict_index tree-to-non-tree-with-rename-A-B-reversed
)

git init simple
(cd simple
  rm -Rf .git/hooks
  write_lines 1 2 3 4 5 >numbers
  echo hello >greeting
  echo foo >whatever
  git add numbers greeting whatever
  tick
  git commit -m initial

  git branch side1
  git branch side2
  git branch side3
  git branch side4

  git checkout side1
  write_lines 1 2 3 4 5 6 >numbers
  echo hi >greeting
  echo bar >whatever
  git add numbers greeting whatever
  tick
  git commit -m modify-stuff

  git checkout side2
  write_lines 0 1 2 3 4 5 >numbers
  echo yo >greeting
  git rm whatever
  mkdir whatever
  >whatever/empty
  git add numbers greeting whatever/empty
  tick
  git commit -m other-modifications

  git checkout side3
  git mv numbers sequence
  tick
  git commit -m rename-numbers

  git checkout side4
  write_lines 0 1 2 3 4 5 >numbers
  echo yo >greeting
  git add numbers greeting
  tick
  git commit -m other-content-modifications

  git switch --orphan unrelated
  >something-else
  git add something-else
  tick
  git commit -m first-commit

  git checkout -b tweak1 side1
  write_lines zero 1 2 3 4 5 6 >numbers
  git add numbers
  git mv numbers "Αυτά μου φαίνονται κινέζικα"
  git commit -m "Renamed numbers"
)

git init rename-delete
(cd rename-delete
  write_lines 1 2 3 4 5 >foo
  mkdir olddir
  for i in a b c; do echo $i >olddir/$i; done
  git add foo olddir
  git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 6 >foo
  git add foo
  git mv olddir newdir
  git commit -m "Modify foo, rename olddir to newdir"

  git checkout B
  write_lines 1 2 3 4 5 six >foo
  git add foo
  git mv foo olddir/bar
  git commit -m "Modify foo & rename foo -> olddir/bar"
)

git init rename-change-matrix
(cd rename-change-matrix
  # Each top-level directory is one independent rename interaction:
  #
  # * modify-source:
  #   A renames `file` to `renamed`; B modifies `file`.
  #   The modification follows the rename and the result is clean at `renamed`.
  # * delete-source:
  #   A renames `file` to `renamed`; B deletes `file`.
  #   This is a rename/delete conflict, with A's renamed file retained.
  # * add-destination:
  #   A renames `file` to `target`; B adds an unrelated `target`.
  #   Both additions occupy the rename destination, producing an add/add conflict.
  # * modify-destination:
  #   A replaces the existing `target` by renaming `source` onto it; B modifies the old `target`.
  #   The merge remains tied to the destination path: the base is the old `target`, A replaces all
  #   of it with `source`, and B appends to the old `target`. Those edits conflict, preserving the
  #   complete A and B versions inside conflict markers.
  # * delete-destination:
  #   A replaces the existing `target` by renaming `source` onto it; B deletes the old `target`.
  #   The deletion conflicts with A's replacement, which remains at `target`.
  # * different-renames:
  #   A and B both modify `file`, then rename it to different destinations.
  #   Both destinations contain the one merged result, with a content conflict for the two appends.
  #   Its configured marker size is 10; the rename/rename conflict adds one, yielding 11.
  # * directory-old:
  #   A renames the directory; B adds a child below its old name.
  #   The child follows the directory rename and the result is clean below `directory-renamed`.
  # * collide-destination:
  #   A modifies `file` and renames it to `target`; B modifies `file` and adds an unrelated `target`.
  #   The rename's content merge conflicts and then collides with B's `target`: its conflict markers are
  #   one character longer than those of the add/add conflict that contains them.
  #
  # Distinct repeated words keep rename similarity pairing local to each case.
  mkdir -p modify-source delete-source add-destination modify-destination delete-destination \
    different-renames directory-old collide-destination
  write_lines alpha alpha alpha alpha alpha >modify-source/file
  write_lines bravo bravo bravo bravo bravo >delete-source/file
  write_lines charlie charlie charlie charlie charlie >add-destination/file
  write_lines delta-source delta-source delta-source delta-source delta-source >modify-destination/source
  write_lines delta-target delta-target delta-target delta-target delta-target >modify-destination/target
  write_lines echo-source echo-source echo-source echo-source echo-source >delete-destination/source
  write_lines echo-target echo-target echo-target echo-target echo-target >delete-destination/target
  write_lines golf golf golf golf golf >different-renames/file
  write_lines juliet juliet juliet juliet juliet >directory-old/file
  write_lines hotel hotel hotel hotel hotel >collide-destination/file
  echo 'different-renames/* conflict-marker-size=10' >.gitattributes
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  git mv modify-source/file modify-source/renamed
  git mv delete-source/file delete-source/renamed
  git mv add-destination/file add-destination/target
  git rm modify-destination/target
  git mv modify-destination/source modify-destination/target
  git rm delete-destination/target
  git mv delete-destination/source delete-destination/target
  echo changed-by-A >>different-renames/file
  git mv different-renames/file different-renames/ours
  git mv directory-old directory-renamed
  echo changed-by-A >>collide-destination/file
  git mv collide-destination/file collide-destination/target
  git commit -am "rename one side of each matrix entry"

  git checkout B
  echo changed-by-B >>modify-source/file
  git rm delete-source/file
  echo added-by-B >add-destination/target
  echo changed-by-B >>modify-destination/target
  git rm delete-destination/target
  echo changed-by-B >>different-renames/file
  git mv different-renames/file different-renames/theirs
  echo added-by-B >directory-old/added
  echo changed-by-B >>collide-destination/file
  echo added-by-B >collide-destination/target
  git add . && git commit -m "apply the other change in each matrix entry"

)

git init same-rename-with-content
(cd same-rename-with-content
  # Both sides modify the same source and rename it to the same destination.
  # The two appends conflict, but the rename itself agrees. The result should therefore be
  # exactly one content merge at `target`, never a second merge of an already-merged blob.
  # Git puts the base and each side's version at stages 1 to 3 of `target`, and none at `file`.
  write_lines foxtrot foxtrot foxtrot foxtrot foxtrot >file
  git add file && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  echo changed-by-A >>file
  git mv file target
  git commit -am "A renames and changes file"

  git checkout B
  echo changed-by-B >>file
  git mv file target
  git commit -am "B renames and changes file"
)

git init same-rename-and-file-to-directory
(cd same-rename-and-file-to-directory
  # Both sides perform the same compound operation:
  #
  # * rename `source` to `moved`;
  # * replace the old `source` path with a directory;
  # * add different content at `source/child`.
  #
  # The identical rewrite must be applied only once. Applying it again after merging the
  # children would recursively remove `source` and silently discard both payloads.
  write_lines source source source source source >source
  git add source && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  git mv source moved
  mkdir source
  echo changed-by-A >source/child
  git add . && git commit -m "A renames source and adds a child at its old path"

  git checkout B
  git mv source moved
  mkdir source
  echo changed-by-B >source/child
  git add . && git commit -m "B renames source and adds a child at its old path"
)

git init renames-to-same-destination
(cd renames-to-same-destination
  # The sides rename different source files onto the same destination:
  #
  # * A: `one` -> `target`
  # * B: `two` -> `target`
  #
  # Neither rename should silently win. A normal merge removes both sources and records the
  # two destination contents as an add/add conflict. Forced ancestor resolution keeps `one`
  # and `two`; forced ours applies only the rename belonging to the selected first side.
  write_lines one one one one one >one
  write_lines two two two two two >two
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  git mv one target
  git commit -m "rename one to target"

  git checkout B
  git mv two target
  git commit -m "rename two to target"
)

git init identical-renames-to-same-destination
(cd identical-renames-to-same-destination
  write_lines same >one
  cp one two
  git add .
  git commit -m "two identical files"

  git branch A
  git branch B

  # Both sides rename a different source to `target`, but the entries have the
  # same mode and object ID. There is nothing to content-merge and the two
  # operations collapse cleanly to the shared destination in either direction.
  git checkout A
  git mv one target
  git commit -m "rename one to target"

  git checkout B
  git mv two target
  git commit -m "rename two to target"
)

git init identical-renames-to-same-destination-with-mode-change
(cd identical-renames-to-same-destination-with-mode-change
  write_lines same >one
  cp one two
  git add .
  git commit -m "two identical files"

  git branch A
  git branch B

  # The destinations have the same blob ID but differ in executable mode. Without a common
  # ancestor entry at `target`, the executable bits conflict as in an add/add, and `target`
  # keeps our mode.
  git checkout A
  git mv one target
  chmod +x target
  # For this to work on windows, we need explicit executable bit handling.
  git update-index --chmod=+x target
  git commit -m "rename one to executable target"

  git checkout B
  git mv two target
  git commit -m "rename two to target"
)

git init deleted-file-added-dir-with-rename
(cd deleted-file-added-dir-with-rename
  # Regression for a deletion that is processed but not applied:
  #
  # * A deletes the file `x`.
  # * B renames that file to `renamed`, then creates the directory entry `x/a`.
  #
  # With ancestor conflict resolution, A's deletion must not count as applied merely because
  # the rename/delete conflict was handled. The ancestor file `x` remains, so `x/a` cannot
  # turn it into a directory.
  echo base >x
  git add x
  git commit -m "add x"

  git branch A
  git branch B

  git checkout A
  git rm x
  git commit -m "delete x"

  git checkout B
  git mv x renamed
  mkdir x
  echo added >x/a
  git add x/a
  git commit -m "rename x and add x/a"
)

git init deleted-file-replaced-by-renamed-dir
(cd deleted-file-replaced-by-renamed-dir
  # Regression for a file that one side replaces with a directory filled only by renames:
  #
  # * A deletes the file `a/b` and moves `d/g` and `d/h` into the new directory `a/b/`.
  # * B only edits the unrelated file `u`.
  #
  # Rename tracking reports A's renames before the unpaired deletion of `a/b`, so the deletion
  # reaches the tree editor after `a/b/` exists and must not remove that directory.
  mkdir a d
  echo original >a/b
  seq 1 10 >d/g
  seq 11 20 >d/h
  seq 21 30 >u
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  git rm a/b
  mkdir -p a/b
  git mv d/g a/b/g
  git mv d/h a/b/h
  git commit -m "replace the file a/b with the moved directory d"

  git checkout B
  seq 21 31 >u
  git commit -am "edit unrelated file"
)

git init renamed-file-replaced-by-renamed-dir
(cd renamed-file-replaced-by-renamed-dir
  # Like `deleted-file-replaced-by-renamed-dir`, but A moves the file `a/b` to `c` instead of
  # deleting it, and B edits `a/b`. B's edit follows the rename to `c`, and removing the rename
  # source `a/b` must not remove the directory `a/b/` that A's other rename created.
  mkdir a d
  seq 1 10 >a/b
  seq 11 20 >d/g
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  git mv a/b c
  mkdir -p a/b
  git mv d/g a/b/g
  git commit -m "move the file a/b away and d/g into a/b/"

  git checkout B
  seq 1 11 >a/b
  git commit -am "edit a/b"
)

git init rename-add
(cd rename-add
		write_lines original 1 2 3 4 5 >foo
		git add foo
		git commit -m "original"

		git branch A
		git branch B

		git checkout A
		write_lines 1 2 3 4 5 >foo
		echo "different file" >bar
		git add foo bar
		git commit -m "Modify foo, add bar"

		git checkout B
		write_lines original 1 2 3 4 5 6 >foo
		git add foo
		git mv foo bar
		git commit -m "rename foo to bar"
)

git init rename-add-exe-bit-conflict
(cd rename-add-exe-bit-conflict
		touch a b
		chmod +x a
    git add --chmod=+x a
		git add b
		git commit -m "original"

		git branch A
		git branch B

		git checkout A
		chmod -x a
    git update-index --chmod=-x a
		git commit -m "-x a"

		git checkout B
		git mv --force b a
		chmod +x a
    git update-index --chmod=+x a
		git commit -m "mv b a; chmod +x a"
)

git init rename-add-symlink
(cd rename-add-symlink
  write_lines original 1 2 3 4 5 >foo
  git add foo
  git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 >foo
  ln -s foo bar
  git add foo bar
  git commit -m "Modify foo, add symlink bar"

  git checkout B
  write_lines original 1 2 3 4 5 6 >foo
  git add foo
  git mv foo bar
  git commit -m "rename foo to bar"
)

git init rename-add-same-symlink
(cd rename-add-same-symlink
  touch target
  ln -s target link
  git add .
  git commit -m "original"

  git branch A
  git branch B

  git checkout A
  git mv link link-new
  git commit -m "rename link to link-new"

  git checkout B
  ln -s target link-new
  git add link-new
  git commit -m "create link-new"
)

git init rename-rename-plus-content
(cd rename-rename-plus-content
  write_lines 1 2 3 4 5 >foo
  git add foo
  git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 six >foo
  git add foo
  git mv foo bar
  git commit -m "Modify foo + rename to bar"

  git checkout B
  write_lines 1 2 3 4 5 6 >foo
  git add foo
  git mv foo baz
  git commit -m "Modify foo + rename to baz"
)

git init rename-add-delete
(
  cd rename-add-delete
  echo "original file" >foo
  git add foo
  git commit -m "original"

  git branch A
  git branch B

  git checkout A
  git rm foo
  echo "different file" >bar
  git add bar
  git commit -m "Remove foo, add bar"

  git checkout B
  git mv foo bar
  git commit -m "rename foo to bar"
)

git init rename-rename-delete-delete
(
  cd rename-rename-delete-delete
  echo foo >foo
  echo bar >bar
  git add foo bar
  git commit -m O

  git branch A
  git branch B

  git checkout A
  git mv foo baz
  git rm bar
  git commit -m "Rename foo, remove bar"

  git checkout B
  git mv bar baz
  git rm foo
  git commit -m "Rename bar, remove foo"
)

git init super-1
(cd super-1
  seq 11 19 >one
  seq 31 39 >three
  seq 51 59 >five
  git add .
  tick
  git commit -m "O"

  git branch A
  git branch B

  git checkout A
  seq 10 19 >one
  echo 40        >>three
  git add one three
  git mv  one   two
  git mv  three four
  git mv  five  six
  tick
  git commit -m "A"

  git checkout B
  echo 20    >>one
  echo forty >>three
  echo 60    >>five
  git add one three five
  git mv  one   six
  git mv  three two
  git mv  five  four
  tick
  git commit -m "B"
)

git init super-2
(cd super-2
  write_lines 1 2 3 4 5 >foo
  mkdir olddir
  for i in a b c; do echo $i >olddir/$i || exit 1; done
  git add foo olddir
  git commit -m "original"

  git branch A
  git branch B

  git checkout A
  git rm foo
  git mv olddir newdir
  mkdir newdir/bar
  >newdir/bar/file
  git add newdir/bar/file
  git commit -m "rm foo, olddir/ -> newdir/, + newdir/bar/file"

  git checkout B
  write_lines 1 2 3 4 5 6 >foo
  git add foo
  git mv foo olddir/bar
  git commit -m "Modify foo & rename foo -> olddir/bar"
)

git init rename-within-rename
(cd rename-within-rename
  mkdir a && write_lines original 1 2 3 4 5 >a/x.f
  mkdir a/sub && write_lines original 1 2 3 4 5 >a/sub/y.f
  touch a/w a/sub/z
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 >a/x.f
  write_lines 1 2 3 4 5 >a/sub/y.f
  git mv a a-renamed
  git commit -am "changed all content, renamed a -> a-renamed"

  git checkout B
  write_lines original 1 2 3 4 5 6 >a/x.f
  write_lines original 1 2 3 4 5 6 >a/sub/y.f
  git mv a/sub a/sub-renamed
  git commit -am "changed all content, renamed a/sub -> a/sub-renamed"
)

git init rename-within-rename-2
(cd rename-within-rename-2
  mkdir a && write_lines original 1 2 3 4 5 >a/x.f
  mkdir a/sub && write_lines original 1 2 3 4 5 >a/sub/y.f
  touch a/w a/sub/z
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 >a/x.f
  write_lines 1 2 3 4 5 >a/sub/y.f
  git mv a/sub a/sub-renamed
  git mv a a-renamed
  git commit -am "changed all content, renamed a -> a-renamed, a/sub -> a/sub-renamed"

  git checkout B
  write_lines original 1 2 3 4 5 6 >a/x.f
  write_lines original 1 2 3 4 5 6 >a/sub/y.f
  git mv a/sub a/sub-renamed
  git commit -am "changed all content, renamed a/sub -> a/sub-renamed"
)

git init conflicting-rename
(cd conflicting-rename
  mkdir a && write_lines original 1 2 3 4 5 >a/x.f
  mkdir a/sub && write_lines original 1 2 3 4 5 >a/sub/y.f
  touch a/w a/sub/z
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 >a/x.f
  write_lines 1 2 3 4 5 >a/sub/y.f
  git mv a a-renamed
  git commit -am "changed all content, renamed a -> a-renamed"

  git checkout B
  write_lines original 1 2 3 4 5 6 >a/x.f
  write_lines original 1 2 3 4 5 6 >a/sub/y.f
  git mv a a-different
  git commit -am "changed all content, renamed a -> a-different"
)

git init conflicting-rename-2
(cd conflicting-rename-2
  mkdir a && write_lines original 1 2 3 4 5 >a/x.f
  mkdir a/sub && write_lines original 1 2 3 4 5 >a/sub/y.f
  touch a/w a/sub/z
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 >a/x.f
  write_lines 1 2 3 4 5 >a/sub/y.f
  git mv a/sub a/sub-renamed
  git commit -am "changed all content, renamed a/sub -> a/sub-renamed"

  git checkout B
  write_lines original 1 2 3 4 5 6 >a/x.f
  write_lines original 1 2 3 4 5 6 >a/sub/y.f
  git mv a/sub a/sub-different
  git commit -am "changed all content, renamed a/sub -> a/sub-different"
)

git init conflicting-rename-complex
(cd conflicting-rename-complex
  mkdir a && write_lines original 1 2 3 4 5 >a/x.f
  mkdir a/sub && write_lines original 1 2 3 4 5 >a/sub/y.f
  touch a/w a/sub/z
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 >a/x.f
  write_lines 1 2 3 4 5 >a/sub/y.f
  git mv a a-renamed
  git commit -am "changed all content, renamed a -> a-renamed"

  git checkout B
  write_lines original 1 2 3 4 5 6 >a/sub/y.f
  git mv a/sub tmp
  git rm -r a
  git mv tmp a
  git commit -am "change something in subdirectory, then overwrite directory with subdirectory"
)

git init same-rename-different-mode
(cd same-rename-different-mode
  mkdir a && write_lines original 1 2 3 4 5 >a/x.f
  touch a/w
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 >a/x.f
  chmod +x a/x.f a/w
  git update-index --chmod=+x a/x.f a/w
  git mv a a-renamed
  git commit -am "changed a/xf, add +x everywhere, renamed a -> a-renamed"

  git checkout B
  write_lines original 1 2 3 4 5 6 >a/x.f
  git mv a a-renamed
  git commit -am "changed all content, renamed a -> a-renamed"
)

git init remove-executable-mode
(cd remove-executable-mode
  touch w
  chmod +x w
  git add --chmod=+x w
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  chmod -x w
  git update-index --chmod=-x w
  git commit -am "remove executable bit from w"

  git checkout B
  write_lines 1 2 3 4 5  >w
  git commit -am "unrelated change to w"
)

git init renamed-symlink-with-conflict
(cd renamed-symlink-with-conflict
  mkdir a && write_lines original 1 2 3 4 5 >a/x.f
  ln -s a/x.f link
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 >a/x.f
  git mv link link-renamed
  git commit -am "changed a/x.f, renamed link -> link-renamed"

  git checkout B
  write_lines original 1 2 3 4 5 6 >a/x.f
  git mv link link-different
  git commit -am "change content, renamed link -> link-different"
)

git init added-file-changed-content-and-mode
(cd added-file-changed-content-and-mode
  mkdir a && write_lines original 1 2 3 4 5 >a/x.f
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 >new
  git add .
  git commit -m "add 'new' with content A"

  git checkout B
  write_lines original 1 2 3 4 5 6 >new
  chmod +x new
  git add --chmod=+x new
  git commit -m "add new with content B and +x"
)

git init executable-bit-merges
(cd executable-bit-merges
  # Each top-level directory is one independent merge of executable bits that Git resolves
  # cleanly, as the mode that only one side changed wins:
  #
  # * rename-clears-bit:
  #   A renames executable `f` to `g` and clears its executable bit; B appends to `f`.
  #   `g` gets B's content and A's mode.
  # * modify-clears-bit:
  #   A renames executable `f` to `g`; B appends to `f` and clears its executable bit.
  #   `g` gets B's content and B's mode.
  # * rename-changes-and-clears-bit:
  #   A renames executable `f` to `g`, prepends to it and clears its executable bit; B appends
  #   to `f`. `g` gets both changes and A's mode.
  # * same-rename:
  #   Both rename executable `f` to `g`; A clears its executable bit and B appends to it.
  #   `g` gets B's content and A's mode.
  # * same-change-sets-bit:
  #   Both change `f` to the same content, and A also sets its executable bit, which `f` keeps.
  # * same-change-clears-bit:
  #   Both change executable `f` to the same content, and A also clears its executable bit, which
  #   stays cleared.
  #
  # Distinct repeated words keep rename similarity pairing local to each case.
  mkdir rename-clears-bit modify-clears-bit rename-changes-and-clears-bit same-rename \
    same-change-sets-bit same-change-clears-bit
  write_lines kilo kilo kilo kilo kilo >rename-clears-bit/f
  write_lines lima lima lima lima lima >modify-clears-bit/f
  write_lines mike mike mike mike mike >rename-changes-and-clears-bit/f
  write_lines november november november november november >same-rename/f
  write_lines oscar >same-change-sets-bit/f
  write_lines papa >same-change-clears-bit/f
  chmod +x rename-clears-bit/f modify-clears-bit/f rename-changes-and-clears-bit/f same-rename/f \
    same-change-clears-bit/f
  git add .
  # For this to work on windows, we need explicit executable bit handling.
  git update-index --chmod=+x rename-clears-bit/f modify-clears-bit/f rename-changes-and-clears-bit/f \
    same-rename/f same-change-clears-bit/f
  git commit -m "base"

  git branch A
  git branch B

  git checkout A
  git mv rename-clears-bit/f rename-clears-bit/g
  git mv modify-clears-bit/f modify-clears-bit/g
  git mv rename-changes-and-clears-bit/f rename-changes-and-clears-bit/g
  write_lines changed-by-A mike mike mike mike mike >rename-changes-and-clears-bit/g
  git mv same-rename/f same-rename/g
  write_lines oscar-changed >same-change-sets-bit/f
  write_lines papa-changed >same-change-clears-bit/f
  chmod +x same-change-sets-bit/f
  chmod -x rename-clears-bit/g rename-changes-and-clears-bit/g same-rename/g same-change-clears-bit/f
  git add .
  git update-index --chmod=+x same-change-sets-bit/f
  git update-index --chmod=-x rename-clears-bit/g rename-changes-and-clears-bit/g same-rename/g \
    same-change-clears-bit/f
  git commit -m "A renames files and changes executable bits"

  git checkout B
  echo changed-by-B >>rename-clears-bit/f
  echo changed-by-B >>modify-clears-bit/f
  echo changed-by-B >>rename-changes-and-clears-bit/f
  echo changed-by-B >>same-rename/f
  write_lines oscar-changed >same-change-sets-bit/f
  write_lines papa-changed >same-change-clears-bit/f
  chmod -x modify-clears-bit/f
  git add .
  git update-index --chmod=-x modify-clears-bit/f
  git mv same-rename/f same-rename/g
  git commit -m "B changes content, renames one file and clears one executable bit"
)

git init executable-bit-conflicts
(cd executable-bit-conflicts
  # Each top-level directory is one independent merge that Git reports as conflicting, and whose
  # result depends on how the executable bits merge:
  #
  # * different-renames:
  #   A renames executable `f` to `g` and clears its executable bit; B appends to `f` and renames
  #   it to `h`. Both destinations of the rename/rename conflict get the merged content and A's
  #   mode, as only A changed the mode.
  # * added-with-different-modes:
  #   Both add the same content at `f`, but only B makes it executable. Without an ancestor entry,
  #   the executable bits conflict and `f` keeps our mode.
  # * symlink-replaced-with-different-modes:
  #   Both replace the symlink `l` with the same content, but only B makes it executable. The
  #   symlink ancestor has no executable bit, so the executable bits conflict and `l` keeps our
  #   mode.
  # * rename-and-addition-with-different-modes:
  #   A renames `source` to `target`; B adds the same content at `target` and makes it
  #   executable. The rename destination and the addition have no ancestor entry, so the
  #   executable bits conflict and `target` keeps our mode.
  #
  # Distinct repeated words keep rename similarity pairing local to each case.
  mkdir different-renames added-with-different-modes symlink-replaced-with-different-modes \
    rename-and-addition-with-different-modes
  write_lines quebec quebec quebec quebec quebec >different-renames/f
  write_lines romeo >added-with-different-modes/existing
  write_lines sierra >symlink-replaced-with-different-modes/existing
  write_lines tango tango tango tango tango >rename-and-addition-with-different-modes/source
  chmod +x different-renames/f
  git add .
  git update-index --chmod=+x different-renames/f
  # The symlink exists only in the index, which works without symlink support as well.
  git update-index --add --cacheinfo \
    120000,"$(printf existing | git hash-object -w --stdin)",symlink-replaced-with-different-modes/l
  git commit -m "base"

  git branch A
  git branch B

  git checkout A
  git mv different-renames/f different-renames/g
  write_lines uniform >added-with-different-modes/f
  rm -f symlink-replaced-with-different-modes/l
  write_lines victor >symlink-replaced-with-different-modes/l
  git rm --cached -q symlink-replaced-with-different-modes/l
  git mv rename-and-addition-with-different-modes/source rename-and-addition-with-different-modes/target
  chmod -x different-renames/g
  git add .
  git update-index --chmod=-x different-renames/g
  git commit -m "A renames files, adds a file and replaces the symlink"

  git checkout B
  echo changed-by-B >>different-renames/f
  git add different-renames/f
  git mv different-renames/f different-renames/h
  write_lines uniform >added-with-different-modes/f
  rm -f symlink-replaced-with-different-modes/l
  write_lines victor >symlink-replaced-with-different-modes/l
  git rm --cached -q symlink-replaced-with-different-modes/l
  write_lines tango tango tango tango tango >rename-and-addition-with-different-modes/target
  chmod +x added-with-different-modes/f symlink-replaced-with-different-modes/l \
    rename-and-addition-with-different-modes/target
  git add .
  git update-index --chmod=+x added-with-different-modes/f symlink-replaced-with-different-modes/l \
    rename-and-addition-with-different-modes/target
  git commit -m "B renames a file, adds executable files and replaces the symlink with one"
)

git init type-change-and-renamed
(cd type-change-and-renamed
  mkdir a && >a/x.f
  ln -s a/x.f link
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  rm link && echo not-link > link
  git commit -am "link type-changed, file changed"

  git checkout B
  git mv link link-renamed
  git commit -am "just renamed the link"
)

git init change-and-delete
(cd change-and-delete
  mkdir a && write_lines original 1 2 3 4 5 >a/x.f
  ln -s a/x.f link
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 3 4 5 6 >a/x.f
  rm link && echo not-link > link
  git commit -am "link type-changed, file changed"

  git checkout B
  git rm link a/x.f
  git commit -am "delete everything"
)

git init submodule-both-modify
(cd submodule-both-modify
	mkdir sub
	(cd sub
	 git init
	 echo original > file
	 git add file
	 tick
	 git commit -m sub-root
	)
	git add sub
	tick
	git commit -m root

	git branch expected

	git checkout -b A main
	(cd sub
	 echo A > file
	 git add file
	 tick
	 git commit -m sub-a
	)
	git add sub
	tick
	git commit -m a
	git branch -f expected

	git checkout -b B main
	(cd sub
	 echo B > file
	 git add file
	 tick
	 git commit -m sub-b
	)
	git add sub
	tick
	git commit -m b
	git branch expected-reversed

	# The tree merger retains Git's conflict stages and provisional ours entry without inspecting submodule history.
	rm .git/index
	git update-index --index-info <<EOF
160000 $(oid e835c0c403c8e494c0ca98f3d25d0b8464c18d38) 1	sub
160000 $(oid 64466ebdff775ad618d9cc993cf52840e0af528c) 2	sub
160000 $(oid ea6eb701e03c2497915c25a851f3da8f8e362ca0) 3	sub
EOF
  make_conflict_index submodule-both-modify-A-B

	rm .git/index
	git update-index --index-info <<EOF
160000 $(oid e835c0c403c8e494c0ca98f3d25d0b8464c18d38) 1	sub
160000 $(oid ea6eb701e03c2497915c25a851f3da8f8e362ca0) 2	sub
160000 $(oid 64466ebdff775ad618d9cc993cf52840e0af528c) 3	sub
EOF
  make_conflict_index submodule-both-modify-A-B-reversed
)

git init gitlink-replaced-by-files
(cd gitlink-replaced-by-files
  git commit --allow-empty -m "seed commit"
  seed=$(git rev-parse HEAD)
  git update-index --add --cacheinfo 160000,$seed,item
  git commit -m "gitlink base"

  git branch A
  git branch B

  # Both sides replace the gitlink with regular files. Their contents must be
  # merged as an add/add pair with an empty blob ancestor; the commit named by
  # the base entry is not a valid blob-merge resource.
  git checkout A
  git rm item
  write_lines changed-by-A >item
  git add item
  git commit -m "replace gitlink with A's file"

  git checkout B
  git rm item
  write_lines changed-by-B >item
  git add item
  git commit -m "replace gitlink with B's file"
)

git init both-modify-union-attr
(cd both-modify-union-attr
  mkdir a && write_lines original 1 2 3 4 5 >a/x.f
  echo "a/* merge=union" >.gitattributes
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  write_lines A 1 2 3 4 5 6 >a/x.f
  git commit -am "change file"

  git checkout B
  write_lines B 1 2 3 4 5 7 >a/x.f
  git commit -am "change file differently"
)

git init both-modify-binary
(cd both-modify-binary
  mkdir a && printf '\x00 binary' >a/x.f
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  printf '\x00 A' >a/x.f
  git commit -am "change binary file"

  git checkout B
  printf '\x00 B' >a/x.f
  git commit -am "change binary file differently"
)

git init both-modify-file-with-binary-attr
(cd both-modify-file-with-binary-attr
  mkdir a && echo 'not binary' >a/x.f
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  echo 'A binary' >a/x.f
  git commit -am "change pseudo-binary file"

  git checkout B
  echo 'B binary' >a/x.f
  git commit -am "change pseudo-binary file differently"
)

git init big-file-merge
(cd big-file-merge
  git config --local core.bigFileThreshold 100
  mkdir a && write_lines original 1 2 3 4 5 >a/x.f
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  seq 37 >a/x.f
  git commit -am "turn normal file into big one (102 bytes)"
  git branch expected

  git checkout B
  write_lines 1 2 3 4 5 6 >a/x.f
  git commit -am "a normal but conflicting file change"
)

git init attribute-path-modify-rename
(cd attribute-path-modify-rename
  # A modifies `source`; B renames it to `destination` and modifies it too. Git merges the content
  # at the rename's destination, whose attributes give conflict markers of 13 characters.
  write_lines 'source* conflict-marker-size=10' 'destination* conflict-marker-size=13' >.gitattributes
  write_lines kilo-1 kilo-2 kilo-3 kilo-4 kilo-5 >source
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  write_lines kilo-1 kilo-2 changed-by-A kilo-4 kilo-5 >source
  git commit -am "A modifies source"

  git checkout B
  git mv source destination
  write_lines kilo-1 kilo-2 changed-by-B kilo-4 kilo-5 >destination
  git commit -am "B renames source to destination and modifies it"
)

git init attribute-path-rename-rename-1to2
(cd attribute-path-rename-rename-1to2
  # A renames `source` to `destination-one` and B renames it to `destination-two`, both modifying it.
  # Git merges the content at the source, whose attributes give conflict markers of 10 characters, one
  # longer for the rename/rename conflict, and writes the result to both destinations.
  write_lines 'source* conflict-marker-size=10' 'destination* conflict-marker-size=13' >.gitattributes
  write_lines lima-1 lima-2 lima-3 lima-4 lima-5 >source
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  git mv source destination-one
  write_lines lima-1 lima-2 changed-by-A lima-4 lima-5 >destination-one
  git commit -am "A renames source to destination-one and modifies it"

  git checkout B
  git mv source destination-two
  write_lines lima-1 lima-2 changed-by-B lima-4 lima-5 >destination-two
  git commit -am "B renames source to destination-two and modifies it"
)

git init attribute-path-rename-collision
(cd attribute-path-rename-collision
  # A modifies `source` and adds an unrelated `destination`; B renames `source` to `destination` and
  # modifies it. Git merges the rename's content at its source, with conflict markers of 10 characters
  # made one longer by the collision, then the add/add at `destination` with its markers of 13 characters.
  write_lines 'source* conflict-marker-size=10' 'destination* conflict-marker-size=13' >.gitattributes
  write_lines mike-1 mike-2 mike-3 mike-4 mike-5 >source
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  write_lines mike-1 mike-2 changed-by-A mike-4 mike-5 >source
  write_lines november-1 november-2 november-3 november-4 november-5 >destination
  git add . && git commit -m "A modifies source and adds destination"

  git checkout B
  git mv source destination
  write_lines mike-1 mike-2 changed-by-B mike-4 mike-5 >destination
  git commit -am "B renames source to destination and modifies it"
)

git init attribute-path-rename-rename-2to1
(cd attribute-path-rename-rename-2to1
  # A renames `source-one` to `destination` and B renames `source-two` there; each side modifies its
  # rename and the other side's source. Git merges each rename with the other side's modification at
  # the rename's source, with conflict markers of 10 characters made one longer by the collision, then
  # the add/add at `destination` with its markers of 13 characters.
  write_lines 'source* conflict-marker-size=10' 'destination* conflict-marker-size=13' >.gitattributes
  write_lines oscar-1 oscar-2 oscar-3 oscar-4 oscar-5 >source-one
  write_lines papa-1 papa-2 papa-3 papa-4 papa-5 >source-two
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  git mv source-one destination
  write_lines oscar-1 oscar-2 changed-by-A oscar-4 oscar-5 >destination
  write_lines papa-1 papa-2 changed-by-A papa-4 papa-5 >source-two
  git commit -am "A renames source-one to destination, modifying it and source-two"

  git checkout B
  git mv source-two destination
  write_lines papa-1 papa-2 changed-by-B papa-4 papa-5 >destination
  write_lines oscar-1 oscar-2 changed-by-B oscar-4 oscar-5 >source-one
  git commit -am "B renames source-two to destination, modifying it and source-one"
)

git init attribute-path-directory-rename-rename
(cd attribute-path-directory-rename-rename
  # A renames the directory `old` to `new` and modifies `new/file`; B renames `old/file` to `old/moved`
  # and modifies it, which A's directory rename moves to `new/moved`. Git merges this rename to two
  # destinations at the source `old/file`, with conflict markers of 10 characters made one longer by the
  # rename/rename conflict.
  write_lines 'old/* conflict-marker-size=10' 'new/* conflict-marker-size=13' >.gitattributes
  mkdir old
  write_lines quebec-1 quebec-2 quebec-3 quebec-4 quebec-5 >old/file
  write_lines romeo-1 romeo-2 romeo-3 romeo-4 romeo-5 >old/other
  write_lines sierra-1 sierra-2 sierra-3 sierra-4 sierra-5 >old/third
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  git mv old new
  write_lines quebec-1 quebec-2 changed-by-A quebec-4 quebec-5 >new/file
  git commit -am "A renames old to new and modifies new/file"

  git checkout B
  git mv old/file old/moved
  write_lines quebec-1 quebec-2 changed-by-B quebec-4 quebec-5 >old/moved
  git commit -am "B renames old/file to old/moved and modifies it"
)

git init marker-size-values
(cd marker-size-values
  # Each file has its own `conflict-marker-size`. Git reads a decimal `int`, perhaps signed and after leading
  # whitespace, and uses it if it is positive; other values, and the attribute set or unset, keep 7 characters.
  write_lines 'm300 conflict-marker-size=300' 'mplus conflict-marker-size=+10' 'm010 conflict-marker-size=010' \
    'm0 conflict-marker-size=0' 'mneg conflict-marker-size=-3' 'mabc conflict-marker-size=abc' \
    'mbig conflict-marker-size=2147483648' 'mset conflict-marker-size' 'munset -conflict-marker-size' \
    'm255 conflict-marker-size=255' 'm256 conflict-marker-size=256' 'm1000 conflict-marker-size=1000' >.gitattributes
  printf 'mvt conflict-marker-size=\v12\nmff conflict-marker-size=\f12\n' >>.gitattributes
  files='m300 mplus m010 m0 mneg mabc mbig mset munset m255 m256 m1000 mvt mff'
  for f in $files; do write_lines $f-1 $f-2 $f-3 $f-4 $f-5 >$f; done
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  for f in $files; do write_lines $f-1 $f-2 changed-by-A $f-4 $f-5 >$f; done
  git commit -am "A changes the middle line of each file"

  git checkout B
  for f in $files; do write_lines $f-1 $f-2 changed-by-B $f-4 $f-5 >$f; done
  git commit -am "B changes the middle line of each file"
)

git init marker-size-rename-collision
(cd marker-size-rename-collision
  # As in attribute-path-rename-collision, with markers of 255 characters at both paths. The content merge at the
  # source has markers of 256 characters inside the markers of 255 characters of the add/add at `destination`.
  write_lines 'source conflict-marker-size=255' 'destination conflict-marker-size=255' >.gitattributes
  write_lines tango-1 tango-2 tango-3 tango-4 tango-5 >source
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  write_lines tango-1 tango-2 changed-by-A tango-4 tango-5 >source
  write_lines uniform-1 uniform-2 uniform-3 uniform-4 uniform-5 >destination
  git add . && git commit -m "A modifies source and adds destination"

  git checkout B
  git mv source destination
  write_lines tango-1 tango-2 changed-by-B tango-4 tango-5 >destination
  git commit -am "B renames source to destination and modifies it"
)

git init binary-content-merge-drivers
(cd binary-content-merge-drivers
  # A and B change each file differently. All files but `lines` hold binary data, which Git merges as binary whatever
  # driver the attributes name: the conflict keeps our version unless `-X ours` or `-X theirs` chooses a side, and the
  # union driver keeps its conflict even then. `-X ours` and `-X theirs` also resolve the conflict of `lines`.
  write_lines 'union merge=union' 'text merge=text' 'binary merge=binary' 'unset -merge' >.gitattributes
  files='union text binary unset unspecified'
  for f in $files; do printf 'base\0%s\n' $f >$f; done
  write_lines 1 2 3 >lines
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  for f in $files; do printf 'A\0%s\n' $f >$f; done
  write_lines 1 A 3 >lines
  git commit -am "A changes every file"

  git checkout B
  for f in $files; do printf 'B\0%s\n' $f >$f; done
  write_lines 1 B 3 >lines
  git commit -am "B changes every file"
)

git init recursive-merge-driver
(cd recursive-merge-driver
  # A1 and B1 change `r` and `u` differently, and A and B merge them crosswise, A keeping the files of A1 and B those of
  # the base, so merging A and B first merges A1 and B1 into a virtual merge-base. The merge driver `custom` of `r` names
  # `binary` as its driver for merges of merge-bases, and `u` holds binary data and has the union driver. Merging the
  # merge-bases, Git keeps the base's version of both, as of all binary data there, so only A changed them, and the
  # merge has the files of A1. `custom` has no command, which Git would need to merge `r` in any other merge.
  write_lines 'r merge=custom' 'u merge=union' >.gitattributes
  git config merge.custom.recursive binary
  write_lines base >r
  printf 'base\0u\n' >u
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  write_lines A1 >r
  printf 'A1\0u\n' >u
  git commit -am "A1 changes r and u" && git tag A1

  git checkout B
  write_lines B1 >r
  printf 'B1\0u\n' >u
  git commit -am "B1 changes r and u" && git tag B1

  git update-ref refs/heads/A "$(git commit-tree -p A1 -p B1 -m "merge B1 into A1, keep the files of A1" "A1^{tree}")"
  git update-ref refs/heads/B "$(git commit-tree -p B1 -p A1 -m "merge A1 into B1, keep the files of the base" "main^{tree}")"
)

git init no-merge-base
(cd no-merge-base
  git checkout -b A
  echo "A" >content && git add . && git commit -m "content A"

  git checkout --orphan B
  echo "B" >content && git add . && git commit -m "content B"

  git checkout -b expectation
)

git init multiple-merge-bases
(cd multiple-merge-bases
  write_lines 1 2 3 4 5 >content
  git add . && git commit -m "initial"

  git branch A
  git branch B

  git checkout A
  write_lines 0 1 2 3 4 5 >content
  git commit -am "change in A" && git tag A1

  git checkout B
  write_lines 1 2 3 4 5 6 >content
  git commit -am "change in B" && git tag B1

  git checkout A
  git merge B1

  git checkout B
  git merge A1

  git checkout A
  write_lines 0 1 2 3 4 5 A >content
  git commit -am "conflicting in A"

  git checkout B
  git rm content
  write_lines 0 2 3 4 5 six >renamed
  git commit -m "rename in B"
)

git init multiple-merge-bases-with-renamed-dir
(cd multiple-merge-bases-with-renamed-dir
  # `deleted-file-replaced-by-renamed-dir` with two merge bases: building the virtual merge base
  # replays that shape, and only if it keeps `a/b/` do A's and B's edits inside it merge cleanly.
  mkdir a d
  echo original >a/b
  seq 1 10 >d/g
  seq 11 20 >d/h
  seq 21 30 >u
  git add . && git commit -m "initial"

  git branch A
  git branch B

  git checkout A
  git rm a/b
  mkdir -p a/b
  git mv d/g a/b/g
  git mv d/h a/b/h
  git commit -m "replace the file a/b with the moved directory d" && git tag A1

  git checkout B
  seq 21 31 >u
  git commit -am "edit unrelated file" && git tag B1

  git checkout A
  git merge B1

  git checkout B
  git merge A1

  git checkout A
  seq 1 11 >a/b/g
  git commit -am "edit a/b/g in A"

  git checkout B
  seq 11 21 >a/b/h
  git commit -am "edit a/b/h in B"
)

git init multiple-merge-bases-with-conflicting-content
(cd multiple-merge-bases-with-conflicting-content
  # A and B change the same line differently, and each branch merges the change of the other while
  # keeping its own line. Merging the two merge-bases conflicts on that line, and as Git keeps the
  # conflict markers in the merged merge-base, the final merge conflicts on the lines A and B kept.
  write_lines 1 2 3 4 5 >content
  git add . && tick && git commit -m "initial"

  git branch A
  git branch B

  git checkout A
  write_lines 1 2 A1 4 5 >content
  tick && git commit -am "change in A" && git tag A1

  git checkout B
  write_lines 1 2 B1 4 5 >content
  tick && git commit -am "change in B" && git tag B1

  git checkout A
  git merge B1 || :
  write_lines 1 2 A1 4 5 >content
  tick && git commit -am "merge B1 and keep the line of A1"

  git checkout B
  git merge A1 || :
  write_lines 1 2 B1 4 5 >content
  tick && git commit -am "merge A1 and keep the line of B1"
)

git init three-merge-bases-with-conflicting-content
(cd three-merge-bases-with-conflicting-content
  # X, Y and Z change the same line differently, and A and B each merge all three, so A and B have
  # three merge-bases. Both merges of two merge-bases conflict one level of recursion below the final
  # merge, so the conflict markers of both are two characters longer than the default.
  write_lines 1 2 3 4 5 >content
  git add . && tick && git commit -m "initial"

  for side in X Y Z; do
    git checkout -b "$side" main
    write_lines 1 2 "$side" 4 5 >content
    tick && git commit -am "change in $side"
  done

  git checkout -b A X
  git merge Y || :
  write_lines 1 2 A 4 5 >content
  tick && git commit -am "merge Y into X"
  git merge Z || :
  write_lines 1 2 A 4 5 >content
  tick && git commit -am "merge Z"

  git checkout -b B Z
  git merge X || :
  write_lines 1 2 B 4 5 >content
  tick && git commit -am "merge X into Z"
  git merge Y || :
  write_lines 1 2 B 4 5 >content
  tick && git commit -am "merge Y"
)

git init multiple-merge-bases-with-unmergeable-entries
(cd multiple-merge-bases-with-unmergeable-entries
  # Each directory holds one conflict between A1 and B1, which A and B merge crosswise, so merging A
  # and B merges A1 and B1 into a virtual merge-base first. As merge-ort does one level of recursion
  # below the merge, that merged merge-base keeps the base version of each entry that can't be
  # merged: of a modified file that the other side deleted, of entries of distinct types, of
  # submodules, binary files and symlinks, and no entry where the base has none, except that a
  # binary file both sides added becomes empty. Conflicting text keeps its markers, and of the
  # executable bits that both sides set differently, that of 'Temporary merge branch 1' applies.
  # The final merge then merges the changes of A and B against the merged merge-base.
  #
  # Every entry is written to the index directly, as for `distinct-types`. Each directory has its
  # own sibling, and no two directories share content.
  empty_tree=$(git mktree </dev/null)
  for dir in modify-vs-delete delete-vs-modify link-vs-modify modify-vs-link added-file-vs-added-link \
    added-link-vs-added-file added-link-vs-added-gitlink added-gitlink-vs-added-link modified-gitlinks \
    added-gitlinks modified-binary-files added-binary-files modified-links added-links \
    added-executable-vs-added-file added-file-vs-added-executable modified-text added-text; do
    git update-index --add --cacheinfo 100644,"$(write_lines "$dir" | git hash-object -w --stdin)","$dir"/sibling
  done
  git update-index --add --index-info <<EOF
100644 $(numbers_blob 101 120)	modify-vs-delete/f
100644 $(numbers_blob 201 220)	delete-vs-modify/f
100644 $(numbers_blob 301 320)	link-vs-modify/f
100644 $(numbers_blob 401 420)	modify-vs-link/f
160000 $(git commit-tree -m "modified-gitlinks base" "$empty_tree")	modified-gitlinks/f
100644 $(printf 'base\0modified-binary-files\n' | git hash-object -w --stdin)	modified-binary-files/f
120000 $(printf 'modified-links base' | git hash-object -w --stdin)	modified-links/f
100644 $(numbers_blob 1701 1720)	modified-text/f
EOF
  commit_index main "base"

  git update-index --force-remove delete-vs-modify/f
  git update-index --add --index-info <<EOF
100644 $(numbers_blob 101 120 10 A110)	modify-vs-delete/f
120000 $(printf 'link-vs-modify A1' | git hash-object -w --stdin)	link-vs-modify/f
100644 $(numbers_blob 401 420 10 A410)	modify-vs-link/f
100644 $(numbers_blob 501 520)	added-file-vs-added-link/f
120000 $(printf 'added-link-vs-added-file A1' | git hash-object -w --stdin)	added-link-vs-added-file/f
120000 $(printf 'added-link-vs-added-gitlink A1' | git hash-object -w --stdin)	added-link-vs-added-gitlink/f
160000 $(git commit-tree -m "added-gitlink-vs-added-link A1" "$empty_tree")	added-gitlink-vs-added-link/f
160000 $(git commit-tree -m "modified-gitlinks A1" "$empty_tree")	modified-gitlinks/f
160000 $(git commit-tree -m "added-gitlinks A1" "$empty_tree")	added-gitlinks/f
100644 $(printf 'A1\0modified-binary-files\n' | git hash-object -w --stdin)	modified-binary-files/f
100644 $(printf 'A1\0added-binary-files\n' | git hash-object -w --stdin)	added-binary-files/f
120000 $(printf 'modified-links A1' | git hash-object -w --stdin)	modified-links/f
120000 $(printf 'added-links A1' | git hash-object -w --stdin)	added-links/f
100755 $(numbers_blob 1501 1520)	added-executable-vs-added-file/f
100644 $(numbers_blob 1601 1620)	added-file-vs-added-executable/f
100644 $(numbers_blob 1701 1720 10 A1710)	modified-text/f
100644 $( (seq 1801 1820; echo A1) | git hash-object -w --stdin)	added-text/f
EOF
  commit_index A1 "A1 changes the entry of every directory" main

  git read-tree main
  git update-index --force-remove modify-vs-delete/f
  git update-index --add --index-info <<EOF
100644 $(numbers_blob 201 220 10 B210)	delete-vs-modify/f
100644 $(numbers_blob 301 320 10 B310)	link-vs-modify/f
120000 $(printf 'modify-vs-link B1' | git hash-object -w --stdin)	modify-vs-link/f
120000 $(printf 'added-file-vs-added-link B1' | git hash-object -w --stdin)	added-file-vs-added-link/f
100644 $(numbers_blob 601 620)	added-link-vs-added-file/f
160000 $(git commit-tree -m "added-link-vs-added-gitlink B1" "$empty_tree")	added-link-vs-added-gitlink/f
120000 $(printf 'added-gitlink-vs-added-link B1' | git hash-object -w --stdin)	added-gitlink-vs-added-link/f
160000 $(git commit-tree -m "modified-gitlinks B1" "$empty_tree")	modified-gitlinks/f
160000 $(git commit-tree -m "added-gitlinks B1" "$empty_tree")	added-gitlinks/f
100644 $(printf 'B1\0modified-binary-files\n' | git hash-object -w --stdin)	modified-binary-files/f
100644 $(printf 'B1\0added-binary-files\n' | git hash-object -w --stdin)	added-binary-files/f
120000 $(printf 'modified-links B1' | git hash-object -w --stdin)	modified-links/f
120000 $(printf 'added-links B1' | git hash-object -w --stdin)	added-links/f
100644 $(numbers_blob 1501 1520)	added-executable-vs-added-file/f
100755 $(numbers_blob 1601 1620)	added-file-vs-added-executable/f
100644 $(numbers_blob 1701 1720 10 B1710)	modified-text/f
100644 $( (seq 1801 1820; echo B1) | git hash-object -w --stdin)	added-text/f
EOF
  commit_index B1 "B1 changes the entry of every directory" main
  make_criss_cross_merges
)

git init multiple-merge-bases-with-renames
(cd multiple-merge-bases-with-renames
  # Each directory holds one conflict between A1 and B1 that involves a rename, merged crosswise as in
  # `multiple-merge-bases-with-unmergeable-entries`. Merging A1 and B1, merge-ort removes the sources
  # of the renames as it does one level of recursion higher. A rename whose source the other side
  # deleted leaves the base version of the source at the destination, unless the other side added an
  # entry other than a directory there, which is then merged with the renamed one. A directory there
  # that the other side added keeps its path while the base version moves to a unique path beside it,
  # and one that the other side deleted leaves the base version in its place. Entries of distinct types
  # that meet at a rename's destination leave none there, and of a source that the other side changed
  # to another type, the destination gets the base version while the source keeps the change.
  #
  # Each rename source has content that no other change of its side adds, so rename detection pairs
  # it only with its own destination.
  for dir in rename-vs-delete delete-vs-rename rename-and-modify-vs-delete delete-vs-rename-and-modify \
    renames-to-different-paths renames-to-different-paths-with-changes renames-to-different-paths-with-conflict \
    renames-to-same-path rename-vs-add add-vs-rename rename-vs-added-link added-link-vs-rename link-vs-rename \
    rename-vs-link renames-of-file-and-link-to-same-path renames-of-link-and-file-to-same-path \
    rename-vs-rename-and-add rename-and-add-vs-rename rename-vs-delete-and-add delete-and-add-vs-rename \
    rename-vs-delete-and-add-directory delete-and-add-directory-vs-rename rename-over-directory-vs-delete \
    delete-vs-rename-over-directory; do
    git update-index --add --cacheinfo 100644,"$(write_lines "$dir" | git hash-object -w --stdin)","$dir"/sibling
  done
  git update-index --add --index-info <<EOF
100644 $(numbers_blob 101 120)	rename-vs-delete/f
100644 $(numbers_blob 201 220)	delete-vs-rename/f
100644 $(numbers_blob 301 320)	rename-and-modify-vs-delete/f
100644 $(numbers_blob 401 420)	delete-vs-rename-and-modify/f
100644 $(numbers_blob 501 520)	renames-to-different-paths/f
100644 $(numbers_blob 601 620)	renames-to-different-paths-with-changes/f
100644 $(numbers_blob 701 720)	renames-to-different-paths-with-conflict/f
100644 $(numbers_blob 801 820)	renames-to-same-path/f1
100644 $(numbers_blob 851 870)	renames-to-same-path/f2
100644 $(numbers_blob 901 920)	rename-vs-add/f
100644 $(numbers_blob 1001 1020)	add-vs-rename/f
100644 $(numbers_blob 1101 1120)	rename-vs-added-link/f
100644 $(numbers_blob 1201 1220)	added-link-vs-rename/f
100644 $(numbers_blob 1301 1320)	link-vs-rename/f
100644 $(numbers_blob 1401 1420)	rename-vs-link/f
100644 $(numbers_blob 1501 1520)	renames-of-file-and-link-to-same-path/f
120000 $(printf 'renames-of-file-and-link-to-same-path' | git hash-object -w --stdin)	renames-of-file-and-link-to-same-path/l
100644 $(numbers_blob 1601 1620)	renames-of-link-and-file-to-same-path/f
120000 $(printf 'renames-of-link-and-file-to-same-path' | git hash-object -w --stdin)	renames-of-link-and-file-to-same-path/l
100644 $(numbers_blob 1701 1720)	rename-vs-rename-and-add/f
100644 $(numbers_blob 1801 1820)	rename-and-add-vs-rename/f
100644 $(numbers_blob 1901 1920)	rename-vs-delete-and-add/f
100644 $(numbers_blob 2001 2020)	delete-and-add-vs-rename/f
100644 $(numbers_blob 2101 2120)	rename-vs-delete-and-add-directory/f
100644 $(numbers_blob 2201 2220)	delete-and-add-directory-vs-rename/f
100644 $(numbers_blob 2301 2320)	rename-over-directory-vs-delete/f
100644 $(write_lines 'rename-over-directory-vs-delete base' | git hash-object -w --stdin)	rename-over-directory-vs-delete/g/x
100644 $(numbers_blob 2401 2420)	delete-vs-rename-over-directory/f
100644 $(write_lines 'delete-vs-rename-over-directory base' | git hash-object -w --stdin)	delete-vs-rename-over-directory/g/x
EOF
  commit_index main "base"

  git update-index --force-remove rename-vs-delete/f delete-vs-rename/f rename-and-modify-vs-delete/f \
    delete-vs-rename-and-modify/f renames-to-different-paths/f renames-to-different-paths-with-changes/f \
    renames-to-different-paths-with-conflict/f renames-to-same-path/f1 rename-vs-add/f rename-vs-added-link/f \
    rename-vs-link/f renames-of-file-and-link-to-same-path/f renames-of-link-and-file-to-same-path/l \
    rename-vs-rename-and-add/f rename-and-add-vs-rename/f rename-vs-delete-and-add/f delete-and-add-vs-rename/f \
    rename-vs-delete-and-add-directory/f delete-and-add-directory-vs-rename/f rename-over-directory-vs-delete/f \
    rename-over-directory-vs-delete/g/x delete-vs-rename-over-directory/f delete-vs-rename-over-directory/g/x
  git update-index --add --index-info <<EOF
100644 $(numbers_blob 101 120)	rename-vs-delete/g
100644 $(numbers_blob 301 320 10 A310)	rename-and-modify-vs-delete/g
100644 $(numbers_blob 501 520)	renames-to-different-paths/g
100644 $(numbers_blob 601 620 1 A601)	renames-to-different-paths-with-changes/g
100644 $(numbers_blob 701 720 10 A710)	renames-to-different-paths-with-conflict/g
100644 $(numbers_blob 801 820)	renames-to-same-path/g
100644 $(numbers_blob 901 920)	rename-vs-add/g
100644 $(numbers_blob 1051 1070)	add-vs-rename/g
100644 $(numbers_blob 1101 1120)	rename-vs-added-link/g
120000 $(printf 'added-link-vs-rename A1' | git hash-object -w --stdin)	added-link-vs-rename/g
120000 $(printf 'link-vs-rename A1' | git hash-object -w --stdin)	link-vs-rename/f
100644 $(numbers_blob 1401 1420)	rename-vs-link/g
100644 $(numbers_blob 1501 1520)	renames-of-file-and-link-to-same-path/g
120000 $(printf 'renames-of-link-and-file-to-same-path' | git hash-object -w --stdin)	renames-of-link-and-file-to-same-path/g
100644 $(numbers_blob 1701 1720)	rename-vs-rename-and-add/g
100644 $(numbers_blob 1801 1820)	rename-and-add-vs-rename/h
100644 $(numbers_blob 1851 1870)	rename-and-add-vs-rename/g
100644 $(numbers_blob 1901 1920 10 A1910)	rename-vs-delete-and-add/g
100644 $(numbers_blob 2051 2070)	delete-and-add-vs-rename/g
100644 $(numbers_blob 2101 2120 10 A2110)	rename-vs-delete-and-add-directory/g
100644 $(write_lines 'delete-and-add-directory-vs-rename A1' | git hash-object -w --stdin)	delete-and-add-directory-vs-rename/g/x
100644 $(numbers_blob 2301 2320 10 A2310)	rename-over-directory-vs-delete/g
EOF
  commit_index A1 "A1 renames, deletes and adds" main

  git read-tree main
  git update-index --force-remove rename-vs-delete/f delete-vs-rename/f rename-and-modify-vs-delete/f \
    delete-vs-rename-and-modify/f renames-to-different-paths/f renames-to-different-paths-with-changes/f \
    renames-to-different-paths-with-conflict/f renames-to-same-path/f2 add-vs-rename/f added-link-vs-rename/f \
    link-vs-rename/f renames-of-file-and-link-to-same-path/l renames-of-link-and-file-to-same-path/f \
    rename-vs-rename-and-add/f rename-and-add-vs-rename/f rename-vs-delete-and-add/f delete-and-add-vs-rename/f \
    rename-vs-delete-and-add-directory/f delete-and-add-directory-vs-rename/f rename-over-directory-vs-delete/f \
    rename-over-directory-vs-delete/g/x delete-vs-rename-over-directory/f delete-vs-rename-over-directory/g/x
  git update-index --add --index-info <<EOF
100644 $(numbers_blob 201 220)	delete-vs-rename/g
100644 $(numbers_blob 401 420 10 B410)	delete-vs-rename-and-modify/g
100644 $(numbers_blob 501 520)	renames-to-different-paths/h
100644 $(numbers_blob 601 620 20 B620)	renames-to-different-paths-with-changes/h
100644 $(numbers_blob 701 720 10 B710)	renames-to-different-paths-with-conflict/h
100644 $(numbers_blob 851 870)	renames-to-same-path/g
100644 $(numbers_blob 951 970)	rename-vs-add/g
100644 $(numbers_blob 1001 1020)	add-vs-rename/g
120000 $(printf 'rename-vs-added-link B1' | git hash-object -w --stdin)	rename-vs-added-link/g
100644 $(numbers_blob 1201 1220)	added-link-vs-rename/g
100644 $(numbers_blob 1301 1320)	link-vs-rename/g
120000 $(printf 'rename-vs-link B1' | git hash-object -w --stdin)	rename-vs-link/f
120000 $(printf 'renames-of-file-and-link-to-same-path' | git hash-object -w --stdin)	renames-of-file-and-link-to-same-path/g
100644 $(numbers_blob 1601 1620)	renames-of-link-and-file-to-same-path/g
100644 $(numbers_blob 1701 1720)	rename-vs-rename-and-add/h
100644 $(numbers_blob 1751 1770)	rename-vs-rename-and-add/g
100644 $(numbers_blob 1801 1820)	rename-and-add-vs-rename/g
100644 $(numbers_blob 1951 1970)	rename-vs-delete-and-add/g
100644 $(numbers_blob 2001 2020 10 B2010)	delete-and-add-vs-rename/g
100644 $(write_lines 'rename-vs-delete-and-add-directory B1' | git hash-object -w --stdin)	rename-vs-delete-and-add-directory/g/x
100644 $(numbers_blob 2201 2220 10 B2210)	delete-and-add-directory-vs-rename/g
100644 $(numbers_blob 2401 2420 10 B2410)	delete-vs-rename-over-directory/g
EOF
  commit_index B1 "B1 renames, deletes and adds" main
  make_criss_cross_merges
)

git init multiple-merge-bases-with-file-directory-conflicts
(cd multiple-merge-bases-with-file-directory-conflicts
  # Each directory holds a file and a directory at the same path in A1 and B1, merged crosswise as in
  # `multiple-merge-bases-with-unmergeable-entries`. Merging A1 and B1, merge-ort moves the file to a
  # unique path named after its side, such as `d~Temporary merge branch 2`, as it does one level of
  # recursion higher, while the directory keeps the path. A file there that one side modified and the
  # other deleted keeps its base version. In the final merge, the side that kept the file has it at the
  # original path again: a rename from the unique path, which the other side deleted. merge-ort stages
  # the version of the merged merge-base as the base of that rename/delete conflict, at the path of the
  # renamed file.
  for dir in modify-vs-directory directory-vs-modify added-file-vs-added-directory added-directory-vs-added-file \
    modify-in-directory-vs-file file-vs-modify-in-directory modify-and-rename-vs-directory \
    directory-vs-modify-and-rename; do
    git update-index --add --cacheinfo 100644,"$(write_lines "$dir" | git hash-object -w --stdin)","$dir"/sibling
  done
  git update-index --add --index-info <<EOF
100644 $(numbers_blob 101 120)	modify-vs-directory/d
100644 $(numbers_blob 201 220)	directory-vs-modify/d
100644 $(numbers_blob 501 520)	modify-in-directory-vs-file/d/x
100644 $(numbers_blob 601 620)	file-vs-modify-in-directory/d/x
100644 $(numbers_blob 701 720)	modify-and-rename-vs-directory/d
100644 $(numbers_blob 801 820)	directory-vs-modify-and-rename/d
EOF
  commit_index main "base"

  git update-index --force-remove directory-vs-modify/d file-vs-modify-in-directory/d/x \
    modify-and-rename-vs-directory/d directory-vs-modify-and-rename/d
  git update-index --add --index-info <<EOF
100644 $(numbers_blob 101 120 10 A110)	modify-vs-directory/d
100644 $(write_lines 'directory-vs-modify A1' | git hash-object -w --stdin)	directory-vs-modify/d/x
100644 $(numbers_blob 301 320)	added-file-vs-added-directory/d
100644 $(write_lines 'added-directory-vs-added-file A1' | git hash-object -w --stdin)	added-directory-vs-added-file/d/x
100644 $(numbers_blob 501 520 10 A510)	modify-in-directory-vs-file/d/x
100644 $(numbers_blob 651 670)	file-vs-modify-in-directory/d
100644 $(numbers_blob 701 720 10 A710)	modify-and-rename-vs-directory/e
100644 $(write_lines 'directory-vs-modify-and-rename A1' | git hash-object -w --stdin)	directory-vs-modify-and-rename/d/x
EOF
  commit_index A1 "A1 replaces files and directories" main

  git read-tree main
  git update-index --force-remove modify-vs-directory/d modify-in-directory-vs-file/d/x \
    modify-and-rename-vs-directory/d directory-vs-modify-and-rename/d
  git update-index --add --index-info <<EOF
100644 $(write_lines 'modify-vs-directory B1' | git hash-object -w --stdin)	modify-vs-directory/d/x
100644 $(numbers_blob 201 220 10 B210)	directory-vs-modify/d
100644 $(write_lines 'added-file-vs-added-directory B1' | git hash-object -w --stdin)	added-file-vs-added-directory/d/x
100644 $(numbers_blob 401 420)	added-directory-vs-added-file/d
100644 $(numbers_blob 551 570)	modify-in-directory-vs-file/d
100644 $(numbers_blob 601 620 10 B610)	file-vs-modify-in-directory/d/x
100644 $(write_lines 'modify-and-rename-vs-directory B1' | git hash-object -w --stdin)	modify-and-rename-vs-directory/d/x
100644 $(numbers_blob 801 820 10 B810)	directory-vs-modify-and-rename/e
EOF
  commit_index B1 "B1 replaces files and directories" main
  make_criss_cross_merges
)

git init rename-and-modification
(cd rename-and-modification
  mkdir a && write_lines original 1 2 3 4 5 >a/x.f
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  git mv a/x.f x.f
  git commit -am "move a/x.f to the top-level"

  git checkout B
  write_lines 1 2 3 4 5 6 >a/x.f
  git commit -am "changed a/x.f"
)

git init symlink-modification
(cd symlink-modification
  touch a b o
  ln -s o link
  git add . && git commit -m "original"

  git branch A
  git branch B

  git checkout A
  rm link && ln -s a link
  git commit -am "set link to point to 'a'"

  git checkout B
  rm link && ln -s b link
  git commit -am "set link to point to 'b'"
)

git init symlink-addition
(cd symlink-addition
  touch a b
  git add . && git commit -m "original without symlink"

  git branch A
  git branch B

  git checkout A
  ln -s a link && git add .
  git commit -m "new link to point to 'a'"

  git checkout B
  ln -s b link && git add .
  git commit -m "new link to point to 'b'"
)

git init added-file-vs-added-directory
(cd added-file-vs-added-directory
  git commit --allow-empty -m "empty base"

  git branch A
  git branch B

  # A adds a file at `e`, while B adds a file below the directory `e`. Resolving
  # this tree/non-tree pair removes B's `e/e` path-tree leaf; its now-empty `e`
  # parent must not remain visible as a change during the inverse scheduling pass.
  git checkout A
  write_lines file >e
  git add e
  git commit -m "add file e"

  git checkout B
  mkdir e
  write_lines nested >e/e
  git add e/e
  git commit -m "add directory e"
)

git init added-symlink-blocks-gitlink-directory
(cd added-symlink-blocks-gitlink-directory
  git commit --allow-empty -m "empty base"
  base="$(git rev-parse HEAD)"

  git branch A
  git branch B

  # A adds a non-tree at `d`, while B adds a directory at the same path. The nested
  # gitlink is deliberate: tree/non-tree handling must not accidentally route this
  # structural conflict through the blob or submodule merge cases.
  git checkout A
  ln -s target d
  git add d
  git commit -m "add symlink d"

  git checkout B
  git update-index --index-info <<EOF
160000 commit $base	d/a/d
EOF
  git commit -m "add nested gitlink below d"
)

git init gitlink-vs-renamed-symlink-directory-with-siblings
(cd gitlink-vs-renamed-symlink-directory-with-siblings
  git commit --allow-empty -m "gitlink target"
  root="$(git rev-parse HEAD)"
  mkdir -p a/a
  ln -s target a/a/a
  git add a/a/a
  git update-index --add --cacheinfo 160000,$root,h
  git commit -m "symlink and gitlink base"

  git branch A
  git branch B

  # A replaces the symlink-containing directory with a regular child and keeps
  # the gitlink at `h`. B instead removes that gitlink, moves the symlink below
  # a new `h` directory, and adds a sibling below it. Resolving the resulting
  # type mismatch may consume the sibling's structural path before cleanup.
  git checkout A
  git rm a/a/a
  mkdir a
  write_lines changed >a/b
  git add a/b
  git commit -m "replace the symlink directory"

  git checkout B
  git mv a/a/a moved-link
  git update-index --force-remove h
  mkdir -p h/b
  git mv moved-link h/a
  write_lines sibling >h/b/a
  git add .
  git commit -m "replace the gitlink with a renamed symlink and sibling"
)

git init same-source-rewrites-after-consumed-path
(cd same-source-rewrites-after-consumed-path
  mkdir -p a/a
  printf payload >a/a/a
  printf payload >b
  git add .
  git commit -m "identical files at nested and root paths"

  git branch A
  git branch B

  # Every non-directory entry deliberately has the same object ID. This gives
  # rewrite detection several equally valid source/destination pairings. A
  # removes `a/a/a` and reuses its payload at unrelated paths.
  git checkout A
  git rm a/a/a
  mkdir -p e/a g/e
  ln -s payload e/a/g
  printf payload >g/e/a
  git add .
  git commit -m "remove the nested source and add identical entries"

  # B retains `a/a/a`, adds an executable sibling, replaces `b` with a nested
  # copy, and adds another copy. Resolving one ambiguous rewrite can consume
  # the shared source before a later same-source rewrite cleans it up.
  git checkout B
  mkdir -p a/e h/e
  printf payload >a/e/a
  chmod +x a/e/a
  git rm b
  mkdir -p b/b
  printf payload >b/b/f
  printf payload >h/e/a
  git add .
  git update-index --chmod=+x a/e/a
  git commit -m "retain and multiply the identical payload"
)

git init rename-delete-after-consumed-path
(cd rename-delete-after-consumed-path
  mkdir -p a h/d f e/f
  write_lines shared >a/a
  chmod +x a/a
  write_lines four >b
  write_lines seven >e/f/a
  chmod +x e/f/a
  write_lines shared >f/a
  write_lines shared >h/d/a
  git add .
  git update-index --chmod=+x a/a e/f/a
  git commit -m "base with repeated rename candidates"

  git branch A
  git branch B

  # A moves `h/d/a` below `a`, turns `f/a` into `f`, and replaces `e/`.
  # The repeated `shared` payload deliberately gives rename detection several
  # possible sources, matching the scheduling ambiguity found by the fuzzer.
  git checkout A
  git rm -r a e
  mkdir -p a/a a/d
  write_lines four >a/a/a
  git mv h/d/a a/d/a
  mv f/a moved
  rmdir f
  mv moved f
  write_lines five >d
  write_lines shared >e
  git add -A
  git commit -m "rename repeated payloads and replace directories"

  # B deletes A's rename source and replaces `b`, `e`, and `f` with opposite
  # file/directory shapes. Resolving another rename/delete pair can consume the
  # path node for `h/d/a` before that pending deletion follows a directory rename.
  git checkout B
  git rm -r a b e f h
  mkdir -p b/a
  write_lines shared >b/a/a
  write_lines four >e
  write_lines four >f
  git add .
  git commit -m "delete rename sources and replace directories"
)

git init modified-file-vs-gitlink-directory
(cd modified-file-vs-gitlink-directory
  ln -s target a
  git add a
  git commit -m "symlink base"
  base="$(git rev-parse HEAD)"

  git branch A
  git branch B

  # A replaces the symlink with an executable file while B replaces it with a
  # directory. The nested gitlink makes its addition sort before the base-file
  # deletion, exercising merge scheduling independently of diff order.
  git checkout A
  rm a
  write_lines modified >a
  chmod +x a
  git add --chmod=+x a
  git commit -m "replace a with an executable"

  git checkout B
  git rm a
  git update-index --index-info <<EOF
160000 commit $base	a/h
EOF
  git commit -m "replace a with a gitlink directory"
)

git init relocated-addition-blocked-by-rename
(cd relocated-addition-blocked-by-rename
  mkdir -p c
  write_lines base >c/c
  git add .
  git commit -m "file in c"

  git branch A
  git branch B

  # A's exact file rename also implies the directory rename `c` -> `a`. B adds
  # `c/a/c`, so directory-rename handling relocates it to `a/a/c`, where A's
  # renamed file at `a/a` blocks the required directory.
  git checkout A
  mkdir -p a
  git mv c/c a/a
  rmdir c
  git commit -m "rename c/c to a/a"

  git checkout B
  mkdir -p c/a
  write_lines added >c/a/c
  git add .
  git commit -m "add below renamed directory"
)

git init nested-rename-blocks-relocated-addition
(cd nested-rename-blocks-relocated-addition
  mkdir a
  write_lines base >a/a
  git add .
  git commit -m "file in a"

  git branch A
  git branch B

  # Both sides rename the same file. A puts it where its containing directory
  # used to be, while B nests it another level below that directory.
  git checkout A
  git mv a/a moved
  rmdir a
  git mv moved a
  git commit -m "move a/a to a"

  git checkout B
  git mv a/a moved
  mkdir -p a/a
  git mv moved a/a/a
  git commit -m "move a/a to a/a/a"
)

git init directory-rename-vs-directory-to-file
(cd directory-rename-vs-directory-to-file
  mkdir a
  write_lines same >a/a
  git add .
  git commit -m "file in a"

  git branch A
  git branch B

  # A renames the containing directory. B moves its only file to the directory's
  # former path, replacing the directory with that file. This is a different-renames
  # conflict whose tree/non-tree handling must update the rename side's path tree.
  git checkout A
  git mv a e
  git commit -m "rename a to e"

  git checkout B
  git mv a/a moved
  rmdir a
  git mv moved a
  git commit -m "replace a directory with its file"
)

git init directory-rename-vs-renamed-file-replacement
(cd directory-rename-vs-renamed-file-replacement
  mkdir -p h/h
  write_lines nested >h/h/a
  write_lines outside >a
  git add .
  git commit -m "directory and outside file"

  git branch A
  git branch B

  # A moves the directory away. B deletes its contents and moves an unrelated
  # file onto the vacated directory path. Unlike the contained-file variant
  # above, the replacement rename has a distinct source, so it can meet the
  # structural directory rewrite before the nested rename/delete pair does.
  git checkout A
  git mv h/h f
  rmdir h
  git commit -m "rename the directory"

  git checkout B
  git rm h/h/a
  mkdir -p h
  git mv a h/h
  git commit -m "replace the directory with a renamed file"
)

git init unrelated-renames-overlapping-destinations
(cd unrelated-renames-overlapping-destinations
  mkdir -p a/a h/b
  write_lines first >a/a/a
  write_lines second >h/b/a
  git add .
  git commit -m "two files in separate directories"

  git branch A
  git branch B

  # A's directory rename places `h/b/a` below `c`. B independently renames
  # `a/a/a` to the non-tree `c` and renames `h/b/a` elsewhere. The two rename
  # destinations therefore overlap even though their source files are unrelated.
  git checkout A
  git mv h c
  git commit -m "rename h to c"

  git checkout B
  git mv h/b/a moved-h
  git mv a/a/a moved-a
  rmdir a/a
  mkdir -p a
  git mv moved-h a/a
  git mv moved-a c
  git commit -m "rename both files to crossing destinations"
)

git init renamed-file-inside-renamed-directory
(cd renamed-file-inside-renamed-directory
  mkdir -p a/a h/b
  write_lines first >a/a/a
  write_lines second >h/b/a
  git add .
  git commit -m "two files in separate directories"

  git branch A
  git branch B

  # A renames `h` to `c`. B replaces the contents of `h` with a file renamed
  # from elsewhere while moving the original `h/b/a` to `a/a`. Directory-rename
  # handling must therefore defer and relocate a rewrite, not just an addition.
  git checkout A
  git mv h c
  git commit -m "rename h to c"

  git checkout B
  git mv h/b/a moved-h
  git mv a/a/a moved-a
  rmdir a/a h/b h
  mkdir -p a h
  git mv moved-h a/a
  git mv moved-a h/h
  git commit -m "replace a renamed directory with an outside file"
)

git init unrelated-renames-to-same-path-with-type-mismatch
(cd unrelated-renames-to-same-path-with-type-mismatch
  write_lines payload >file-source
  ln -s payload link-source
  git add .
  git commit -m "regular file and symlink"

  git branch A
  git branch B

  # Both sides rename a different base entry to `target`. A's entry is a
  # symlink while B's is a regular file, so their destination cannot be
  # content-merged. Git keeps the symlink at `target` and relocates the regular
  # file to the side-qualified `target~B`, independently of merge direction.
  git checkout A
  git mv link-source target
  git commit -m "rename the symlink to target"

  git checkout B
  git mv file-source target
  git commit -m "rename the regular file to target"
)

git init renamed-file-vs-file-to-directory-with-siblings
(cd renamed-file-vs-file-to-directory-with-siblings
  write_lines base >a
  git add a
  git commit -m "base file"

  git branch A
  git branch B

  # A renames the base file away while B replaces its old path with a directory
  # containing two children. Both children encounter the same rename/delete
  # conflict; handling the first must not make the second try to remove an
  # already-pruned rename-destination node.
  git checkout A
  git mv a h
  git commit -m "rename the file"

  git checkout B
  git rm a
  mkdir a
  write_lines first >a/a
  write_lines second >a/c
  git add .
  git commit -m "replace the file with two children"
)

git init identical-additions-with-different-relations
(cd identical-additions-with-different-relations
  # A's earlier directory deletion shifts the relation ID assigned to the shared
  # addition. The relation is diff-local, so both additions still have one effect.
  mkdir -p a-shift
  echo shift >a-shift/file
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  git rm -r a-shift
  mkdir -p b-target
  echo shared-addition >b-target/file
  git add . && git commit -m "delete a directory and add the shared tree"

  git checkout B
  mkdir -p b-target
  echo shared-addition >b-target/file
  git add . && git commit -m "add the shared tree"
)

git init identical-deletions-with-different-relations
(cd identical-deletions-with-different-relations
  # A deletes an earlier directory as well, so the shared deletion receives a
  # different relation ID on each side despite removing the same tree.
  mkdir -p a-shift b-target
  echo shift >a-shift/file
  echo shared-deletion >b-target/file
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  git rm -r a-shift b-target
  git commit -m "delete the earlier and shared trees"

  git checkout B
  git rm -r b-target
  git commit -m "delete the shared tree"
)

git init identical-rewrites-with-different-relations
(cd identical-rewrites-with-different-relations
  # Moving the same file into a new directory gives each rewrite a destination
  # relation. A's earlier deletion shifts that ID without changing the rename.
  mkdir -p a-shift
  echo shift >a-shift/file
  echo shared-rewrite >b-source
  git add . && git commit -m "base"

  git branch A
  git branch B

  git checkout A
  git rm -r a-shift
  mkdir c-target
  git mv b-source c-target/file
  mkdir b-source
  echo from-A >b-source/A-only
  git add . && git commit -m "delete an earlier tree and rename the shared file"

  git checkout B
  mkdir c-target
  git mv b-source c-target/file
  mkdir b-source
  echo from-B >b-source/B-only
  git add . && git commit -m "rename the shared file"
)

git init type-change-to-symlink
(cd type-change-to-symlink
  touch a b link
  git add . && git commit -m "original without symlink"

  git branch A
  git branch B

  git checkout A
  git rm link
  ln -s a link && git add .
  git commit -m "new link to point to 'a'"

  git checkout B
  git rm link
  ln -s b link && git add .
  git commit -m "new link to point to 'b'"
)

git init distinct-types
(cd distinct-types
  # Each top-level directory records different types for its entry `item` on the two sides: a
  # regular file, a symlink, a gitlink or a directory. As in merge-ort's process_entry(), the
  # regular file moves to the side-qualified `item~<side>` while the other entry stays at `item`,
  # both move if neither is a regular file, and a directory stays where it is. An entry keeps the
  # base stage only if it has the base's type.
  #
  # Both sides change the base's `item`:
  # * link-to-file: A replaces the symlink with a regular file, B changes the symlink.
  # * file-to-link: A replaces the regular file with a symlink, B changes the file.
  # * file-to-gitlink: A replaces the regular file with a gitlink, B changes the file.
  # * link-to-gitlink: A replaces the symlink with a gitlink, B changes the symlink, so both
  #   move and nothing remains at `item`.
  # * gitlink-to-file: A changes the gitlink, B replaces it with a regular file.
  # * three-types: A replaces the symlink with a regular file and B with a gitlink, so neither
  #   entry keeps the base stage.
  #
  # Both sides add `item`, A's type named first: added-file-vs-link, added-file-vs-gitlink,
  # added-link-vs-gitlink, added-directory-vs-gitlink and added-directory-vs-link.
  #
  # A renames `source` to `item` while B adds `item`: renamed-file-vs-added-gitlink,
  # renamed-link-vs-added-gitlink and renamed-gitlink-vs-added-file.
  #
  # A renames `file-source` or `link-source` and B renames `gitlink-source` to `item`:
  # renamed-file-vs-renamed-gitlink and renamed-link-vs-renamed-gitlink.
  #
  # The script writes every entry to the index directly: that needs no symlink support, and no
  # file has to be written where a checked-out gitlink left a directory. Each rename source has an
  # object no other change of its side adds, so exact rename detection pairs it only with its own
  # destination.
  empty_tree=$(git mktree </dev/null)
  added_target=$(git commit-tree -m "added gitlink target" "$empty_tree")
  base_target=$(git commit-tree -m "base gitlink target" "$empty_tree")
  changed_target=$(git commit-tree -p "$base_target" -m "changed gitlink target" "$empty_tree")
  renamed_target=$(git commit-tree -m "renamed gitlink target" "$empty_tree")
  file_rename_target=$(git commit-tree -m "gitlink renamed onto a renamed file" "$empty_tree")
  link_rename_target=$(git commit-tree -m "gitlink renamed onto a renamed symlink" "$empty_tree")
  for dir in link-to-file file-to-link file-to-gitlink link-to-gitlink gitlink-to-file three-types \
    added-file-vs-link added-file-vs-gitlink added-link-vs-gitlink added-directory-vs-gitlink \
    added-directory-vs-link renamed-file-vs-added-gitlink renamed-link-vs-added-gitlink \
    renamed-gitlink-vs-added-file renamed-file-vs-renamed-gitlink renamed-link-vs-renamed-gitlink; do
    git update-index --add --cacheinfo 100644,"$(write_lines "$dir" | git hash-object -w --stdin)","$dir"/sibling
  done
  git update-index --add --index-info <<EOF
120000 $(printf target-one | git hash-object -w --stdin)	link-to-file/item
100644 $(write_lines kilo kilo kilo kilo kilo | git hash-object -w --stdin)	file-to-link/item
100644 $(write_lines lima lima lima lima lima | git hash-object -w --stdin)	file-to-gitlink/item
120000 $(printf target-two | git hash-object -w --stdin)	link-to-gitlink/item
160000 $base_target	gitlink-to-file/item
120000 $(printf target-three | git hash-object -w --stdin)	three-types/item
100644 $(write_lines sierra sierra sierra sierra sierra | git hash-object -w --stdin)	renamed-file-vs-added-gitlink/source
120000 $(printf target-seven | git hash-object -w --stdin)	renamed-link-vs-added-gitlink/source
160000 $renamed_target	renamed-gitlink-vs-added-file/source
100644 $(write_lines xray xray xray xray xray | git hash-object -w --stdin)	renamed-file-vs-renamed-gitlink/file-source
160000 $file_rename_target	renamed-file-vs-renamed-gitlink/gitlink-source
120000 $(printf target-nine | git hash-object -w --stdin)	renamed-link-vs-renamed-gitlink/link-source
160000 $link_rename_target	renamed-link-vs-renamed-gitlink/gitlink-source
EOF
  git commit -m "base"

  git branch A
  git branch B

  git checkout A
  git update-index --force-remove renamed-file-vs-added-gitlink/source renamed-link-vs-added-gitlink/source \
    renamed-gitlink-vs-added-file/source renamed-file-vs-renamed-gitlink/file-source \
    renamed-link-vs-renamed-gitlink/link-source
  git update-index --add --index-info <<EOF
100644 $(write_lines mike mike mike mike mike | git hash-object -w --stdin)	link-to-file/item
120000 $(printf target-four | git hash-object -w --stdin)	file-to-link/item
160000 $added_target	file-to-gitlink/item
160000 $added_target	link-to-gitlink/item
160000 $changed_target	gitlink-to-file/item
100644 $(write_lines november november november november november | git hash-object -w --stdin)	three-types/item
100644 $(write_lines oscar oscar oscar oscar oscar | git hash-object -w --stdin)	added-file-vs-link/item
100644 $(write_lines papa papa papa papa papa | git hash-object -w --stdin)	added-file-vs-gitlink/item
120000 $(printf target-five | git hash-object -w --stdin)	added-link-vs-gitlink/item
100644 $(write_lines tango tango tango tango tango | git hash-object -w --stdin)	added-directory-vs-gitlink/item/file
100644 $(write_lines uniform uniform uniform uniform uniform | git hash-object -w --stdin)	added-directory-vs-link/item/file
100644 $(write_lines sierra sierra sierra sierra sierra | git hash-object -w --stdin)	renamed-file-vs-added-gitlink/item
120000 $(printf target-seven | git hash-object -w --stdin)	renamed-link-vs-added-gitlink/item
160000 $renamed_target	renamed-gitlink-vs-added-file/item
100644 $(write_lines xray xray xray xray xray | git hash-object -w --stdin)	renamed-file-vs-renamed-gitlink/item
120000 $(printf target-nine | git hash-object -w --stdin)	renamed-link-vs-renamed-gitlink/item
EOF
  git commit -m "A changes types, adds entries and renames"

  git checkout B
  git update-index --force-remove renamed-file-vs-renamed-gitlink/gitlink-source \
    renamed-link-vs-renamed-gitlink/gitlink-source
  git update-index --add --index-info <<EOF
120000 $(printf target-one-changed | git hash-object -w --stdin)	link-to-file/item
100644 $(write_lines kilo kilo kilo kilo kilo quebec | git hash-object -w --stdin)	file-to-link/item
100644 $(write_lines lima lima lima lima lima romeo | git hash-object -w --stdin)	file-to-gitlink/item
120000 $(printf target-two-changed | git hash-object -w --stdin)	link-to-gitlink/item
100644 $(write_lines whiskey whiskey whiskey whiskey whiskey | git hash-object -w --stdin)	gitlink-to-file/item
160000 $added_target	three-types/item
120000 $(printf target-six | git hash-object -w --stdin)	added-file-vs-link/item
160000 $added_target	added-file-vs-gitlink/item
160000 $added_target	added-link-vs-gitlink/item
160000 $added_target	added-directory-vs-gitlink/item
120000 $(printf target-eight | git hash-object -w --stdin)	added-directory-vs-link/item
160000 $added_target	renamed-file-vs-added-gitlink/item
160000 $added_target	renamed-link-vs-added-gitlink/item
100644 $(write_lines victor victor victor victor victor | git hash-object -w --stdin)	renamed-gitlink-vs-added-file/item
160000 $file_rename_target	renamed-file-vs-renamed-gitlink/item
160000 $link_rename_target	renamed-link-vs-renamed-gitlink/item
EOF
  git commit -m "B changes and adds entries of other types and renames gitlinks"
)

git init conflicts-when-ours-is-processed-first
(cd conflicts-when-ours-is-processed-first
  write_lines same >x
  write_lines same >y
  seq 10 >m
  write_lines one two three >d
  write_lines four five six >e
  write_lines renamed >r
  git add .
  git commit -m "base with two identical files"

  git branch A
  git branch B

  # A renames the two identical files, which gives rename detection two equally valid pairings. Merges then
  # process the changes of the same side first in both directions, so in one of them the conflicts below are
  # found while the changes of ours are processed against those of theirs. A also changes `m`, deletes `d`
  # and renames `e` and `r`.
  git checkout A
  git mv x x2
  git mv y y2
  write_lines 1 2 3 4 five-A 6 7 8 9 10 >m
  git rm d
  git mv e e2
  git mv r r-A
  git add m
  git commit -m "rename the identical files, e and r, change m and delete d"

  # B renames `m` with a change of its own, renames `d` and `r` and deletes `e`.
  git checkout B
  git mv m m2
  write_lines 1 2 3 4 five-B 6 7 8 9 10 >m2
  git mv d d2
  git rm e
  git mv r r-B
  git add m2
  git commit -m "rename and change m, rename d and r and delete e"
)

git init directory-rename-moves
(cd directory-rename-moves
  # A renames `old` to `new`. B adds `old/added` and `old/sub/added` and renames `src/moved`, which A changes,
  # into `old`. A's directory rename moves all three to `new`: with Git's default `merge.directoryRenames=conflict`
  # each move is a file location conflict, with `true` it is clean, and with `false` the files stay in `old`. The
  # renamed file keeps A's change.
  mkdir -p old/sub src
  seq 1 10 >old/1
  seq 11 20 >old/2
  seq 21 30 >old/3
  seq 31 40 >old/sub/4
  seq 41 50 >src/moved
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  git mv old new
  seq 41 51 >src/moved
  git commit -am "rename old/ to new/, change src/moved"

  git checkout B
  seq 51 60 >old/added
  seq 61 70 >old/sub/added
  git add old/added old/sub/added
  git mv src/moved old/moved
  git commit -m "add old/added and old/sub/added, move src/moved into old/"
)

git init directory-rename-split
(cd directory-rename-split
  # A moves two files of `dir` to `one` and the other two to `two`. As neither directory received more of them,
  # `dir` has no directory rename and B's `dir/new` stays where it is. Git reports this as a conflict without
  # conflicting stages, unless `merge.directoryRenames` is `false`.
  mkdir dir
  seq 1 10 >dir/1
  seq 11 20 >dir/2
  seq 21 30 >dir/3
  seq 31 40 >dir/4
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  mkdir one two
  git mv dir/1 dir/2 one/
  git mv dir/3 dir/4 two/
  git commit -m "split dir/ into one/ and two/"

  git checkout B
  seq 41 50 >dir/new
  git add dir/new
  git commit -m "add dir/new"
)

git init directory-rename-file-in-way
(cd directory-rename-file-in-way
  # A renames `old` to `new`, and B adds `old/x` as well as `new/x`. B's own `new/x` is in the way of moving
  # `old/x` there, so `old/x` stays where it is. Git reports this as a conflict without conflicting stages, unless
  # `merge.directoryRenames` is `false`.
  mkdir old
  seq 1 10 >old/1
  seq 11 20 >old/2
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  git mv old new
  git commit -m "rename old/ to new/"

  git checkout B
  mkdir new
  seq 21 30 >old/x
  seq 31 40 >new/x
  git add old/x new/x
  git commit -m "add old/x and new/x"
)

git init directory-rename-collision
(cd directory-rename-collision
  # A moves the files of `a` and `b` into `c`, and B adds `a/x` and `b/x`. A's directory renames would move both
  # to `c/x`, so neither is moved. Git reports one conflict for both, without conflicting stages, unless
  # `merge.directoryRenames` is `false`.
  mkdir a b
  seq 1 10 >a/1
  seq 11 20 >a/2
  seq 21 30 >b/3
  seq 31 40 >b/4
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  mkdir c
  git mv a/1 a/2 b/3 b/4 c/
  git commit -m "merge a/ and b/ into c/"

  git checkout B
  seq 41 50 >a/x
  seq 51 60 >b/x
  git add a/x b/x
  git commit -m "add a/x and b/x"
)

git init directory-rename-into-renamed-directory
(cd directory-rename-into-renamed-directory
  # A renames `e` to `f` and adds `d/new`, while B moves the files of `d` into `e`. A's directory rename moves
  # B's renamed files on to `f`, where Git records the merge-base and both sides of each with its default
  # `merge.directoryRenames=conflict`. B's directory rename of `d` to `e` isn't applied to A's `d/new`, as A
  # renamed `e` itself. With `merge.directoryRenames=false`, B's files stay in `e`.
  mkdir d e
  seq 1 10 >d/1
  seq 11 20 >d/2
  seq 21 30 >e/3
  seq 31 40 >e/4
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  git mv e f
  seq 41 50 >d/new
  git add d/new
  git commit -m "rename e/ to f/, add d/new"

  git checkout B
  git mv d/1 d/2 e/
  git commit -m "move the files of d/ into e/"
)

git init directory-rename-to-same-destination
(cd directory-rename-to-same-destination
  # A renames `old` to `new` and `file` to `new/file`, and B renames `file` to `old/file`. A's directory rename
  # moves B's rename to `new/file`, where both sides renamed `file`, and their changes are merged there. With Git's
  # default `merge.directoryRenames=conflict`, the move is a file location conflict, and Git records the merge-base
  # and both sides at `new/file`. With `merge.directoryRenames=false`, `file` is renamed to two destinations.
  mkdir old
  seq 1 10 >old/1
  seq 11 20 >old/2
  seq 21 30 >file
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  git mv old new
  git mv file new/file
  write_lines 20 $(seq 21 30) >new/file
  git commit -am "rename old/ to new/ and file to new/file, change its start"

  git checkout B
  git mv file old/file
  write_lines $(seq 21 30) 31 >old/file
  git commit -am "rename file to old/file, change its end"
)

git init identical-rename-destinations-in-walk-order
(cd identical-rename-destinations-in-walk-order
  # A renames `src/f` to `r/f` and adds an identical `p/f`, and B changes `src/f`. Of identical destinations, the
  # merge's rename detection pairs `src/f` with the one it finds first, which then takes B's change. Directories
  # that only one side changed, like A's new `p`, are visited after all others, so `src/f` is paired with `r/f` in
  # `r`, which both sides changed, and `p/f` stays as A added it.
  mkdir r src
  seq 1 10 >r/kept
  seq 11 20 >src/f
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  mkdir p
  git mv src/f r/f
  cp r/f p/f
  git add p/f
  git commit -m "rename src/f to r/f and add an identical p/f"

  git checkout B
  seq 11 21 >src/f
  seq 21 30 >r/other
  git add r/other
  git commit -am "change src/f, add r/other"
)

git init identical-rename-destinations-in-deferred-order
(cd identical-rename-destinations-in-deferred-order
  # Like `identical-rename-destinations-in-walk-order`, but both destinations are in directories that only A
  # added, `a` and `b`. These are visited in the order in which Git's hash map lists their paths, which puts `b`
  # first, so `src/f` is paired with `b/f`, which takes B's change.
  mkdir src
  seq 11 20 >src/f
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  mkdir a b
  git mv src/f a/f
  cp a/f b/f
  git add b/f
  git commit -m "rename src/f to a/f and add an identical b/f"

  git checkout B
  seq 11 21 >src/f
  git commit -am "change src/f"
)

git init rename-to-self-modified
(cd rename-to-self-modified
  # A renames `x/f` and `x/g` into `x/y` and changes both, while B moves the files of `x/y` into `x` and changes
  # `x/f` and `x/g`. B's directory rename moves A's renames back to their sources, which leaves them changes of `x/f`
  # and `x/g` that are merged with B's changes there. The conflict markers in `x/g` name A's version by the location
  # A gave it, `x/y/g`. With Git's default `merge.directoryRenames=conflict`, both files are file location conflicts
  # with all three stages.
  mkdir -p x/y
  seq 1 10 >x/y/1
  seq 11 20 >x/y/2
  seq 21 30 >x/f
  seq 31 40 >x/g
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  git mv x/f x/y/f
  git mv x/g x/y/g
  write_lines 20 $(seq 21 30) >x/y/f
  write_lines 31 32 33 34 "35 A" 36 37 38 39 40 >x/y/g
  git commit -am "rename x/f and x/g into x/y, change both"

  git checkout B
  git mv x/y/1 x/y/2 x/
  write_lines $(seq 21 30) 31 >x/f
  write_lines 31 32 33 34 "35 B" 36 37 38 39 40 >x/g
  git commit -am "move the files of x/y into x, change x/f and x/g"
)

git init rename-to-self-deleted
(cd rename-to-self-deleted
  # A renames `x/f` into `x/y` and changes it, while B moves the files of `x/y` into `x` and deletes `x/f`. B's
  # directory rename moves A's rename back to `x/f`, where it is a change that conflicts with B's deletion, so Git
  # records the merge-base and A's version there. With `merge.directoryRenames=false`, the rename stays and
  # conflicts with the deletion at `x/y/f`.
  mkdir -p x/y
  seq 1 10 >x/y/1
  seq 11 20 >x/y/2
  seq 21 30 >x/f
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  git mv x/f x/y/f
  write_lines 20 $(seq 21 30) >x/y/f
  git commit -am "rename x/f into x/y and change it"

  git checkout B
  git mv x/y/1 x/y/2 x/
  git rm x/f
  git commit -m "move the files of x/y into x, delete x/f"
)

git init rename-to-self-renamed
(cd rename-to-self-renamed
  # A renames `x/f` into `x/y` and changes it, while B moves the files of `x/y` into `x` and renames `x/f` to `x/h`
  # with another change. With A as ours, B's directory rename moves A's rename back to `x/f`, which leaves B's rename
  # as the only one: both changes are merged at `x/h`, and the merge is clean even with Git's default
  # `merge.directoryRenames=conflict`. With B as ours, Git still pairs both renames of `x/f`, B's to `x/h` and A's,
  # which the directory rename made a rename to `x/f`, and records them as renames to two destinations.
  mkdir -p x/y
  seq 1 10 >x/y/1
  seq 11 20 >x/y/2
  seq 21 30 >x/f
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  git mv x/f x/y/f
  write_lines 20 $(seq 21 30) >x/y/f
  git commit -am "rename x/f into x/y and change it"

  git checkout B
  git mv x/y/1 x/y/2 x/
  git mv x/f x/h
  write_lines $(seq 21 30) 31 >x/h
  git commit -am "move the files of x/y into x, rename x/f to x/h and change it"
)

git init rename-to-self-kept
(cd rename-to-self-kept
  # A renames `x/f` into `x/y` and changes it, while B moves the files of `x/y` into `x` and keeps `x/f`. As B didn't
  # change `x/f`, the merge doesn't look for where A moved it, and A's `x/y/f` is an addition. B's directory rename
  # would move it to `x/f`, which the merge-base has, so `x/y/f` stays where it is. With `merge.directoryRenames` set
  # to `conflict` or `true`, Git reports this as a conflict without stages.
  mkdir -p x/y
  seq 1 10 >x/y/1
  seq 11 20 >x/y/2
  seq 21 30 >x/f
  git add . && git commit -m "init"

  git branch A
  git branch B

  git checkout A
  git mv x/f x/y/f
  write_lines 20 $(seq 21 30) >x/y/f
  git commit -am "rename x/f into x/y and change it"

  git checkout B
  git mv x/y/1 x/y/2 x/
  git commit -m "move the files of x/y into x"
)



baseline non-tree-to-tree A-B A B
baseline deleted-file-added-dir A-B A B
baseline deleted-file-added-gitlink-directory A-B A B
baseline tree-to-non-tree A-B A B
baseline tree-to-non-tree-with-rename A-B A B
baseline non-tree-to-tree-with-rename A-B A B
baseline rename-add-same-symlink A-B A B
baseline rename-add-exe-bit-conflict A-B A B
baseline remove-executable-mode A-B A B
baseline simple side-1-3-without-conflict side1 side3
baseline simple fast-forward side1 main
baseline simple no-change main main
baseline simple side-1-3-without-conflict-diff3 side1 side3
baseline simple side-1-2-various-conflicts side1 side2
baseline simple side-1-2-various-conflicts-diff3 side1 side2
baseline simple single-content-conflict side1 side4
baseline simple single-content-conflict-diff3 side1 side4
baseline simple tweak1-side2 tweak1 side2
baseline simple tweak1-side2-diff3 tweak1 side2
baseline simple side-1-unrelated side1 unrelated
baseline simple side-1-unrelated-diff3 side1 unrelated
baseline rename-delete A-B A B
baseline rename-delete A-similar A A
baseline rename-delete B-similar B B
baseline rename-change-matrix A-B A B
baseline same-rename-with-content A-B A B
baseline same-rename-and-file-to-directory A-B A B
baseline renames-to-same-destination A-B A B
baseline identical-renames-to-same-destination A-B A B
baseline identical-renames-to-same-destination-with-mode-change A-B A B
baseline deleted-file-added-dir-with-rename A-B A B
baseline deleted-file-replaced-by-renamed-dir A-B A B
baseline renamed-file-replaced-by-renamed-dir A-B A B
baseline rename-add A-B A B
baseline rename-add A-B-diff3 A B
baseline rename-add-symlink A-B A B
baseline rename-add-symlink A-B-diff3 A B
baseline rename-rename-plus-content A-B A B
baseline rename-rename-plus-content A-B-diff3 A B
baseline rename-add-delete A-B A B
baseline rename-rename-delete-delete A-B A B
baseline super-1 A-B A B
baseline super-1 A-B-diff3 A B
baseline super-2 A-B A B
baseline super-2 A-B-diff3 A B

baseline rename-within-rename A-B A B
baseline rename-within-rename-2 A-B A B
baseline conflicting-rename A-B A B
baseline conflicting-rename-2 A-B A B
baseline conflicting-rename-complex A-B A B

baseline same-rename-different-mode A-B A B
baseline renamed-symlink-with-conflict A-B A B
baseline added-file-changed-content-and-mode A-B A B
baseline executable-bit-merges A-B A B
baseline executable-bit-conflicts A-B A B

baseline type-change-and-renamed A-B A B
baseline change-and-delete A-B A B
baseline submodule-both-modify A-B A B "The tree-only merge cannot inspect submodule reachability, so it keeps ours as the provisional entry and reports SubmoduleMerge."
baseline gitlink-replaced-by-files A-B A B
baseline both-modify-union-attr A-B A B
baseline both-modify-union-attr A-B-diff3 A B
baseline both-modify-binary A-B A B
baseline both-modify-binary A-B A B
baseline both-modify-file-with-binary-attr A-B A B
baseline big-file-merge A-B A B "Git actually ignores core.bigFileThreshold during merging and tries a normal merge (or binary one) anyway. We don't ignore it and treat big files like binary files" \
                                no-reverse
baseline attribute-path-modify-rename A-B A B
baseline attribute-path-rename-rename-1to2 A-B A B
baseline attribute-path-rename-collision A-B A B
baseline attribute-path-rename-rename-2to1 A-B A B
baseline attribute-path-directory-rename-rename A-B A B
baseline marker-size-values A-B A B
baseline marker-size-values A-B-diff3 A B
baseline marker-size-rename-collision A-B A B
baseline binary-content-merge-drivers A-B A B
baseline binary-content-merge-drivers A-B-X-ours A B
baseline binary-content-merge-drivers A-B-X-theirs A B
baseline no-merge-base A-B A B
baseline no-merge-base A-B-diff3 A B

baseline multiple-merge-bases A-B A B
baseline multiple-merge-bases A-B-diff3 A B
baseline multiple-merge-bases-with-renamed-dir A-B A B
baseline multiple-merge-bases-with-conflicting-content A-B A B
baseline multiple-merge-bases-with-conflicting-content A-B-diff3 A B
baseline three-merge-bases-with-conflicting-content A-B A B
baseline three-merge-bases-with-conflicting-content A-B-diff3 A B
baseline multiple-merge-bases-with-unmergeable-entries A-B A B
baseline multiple-merge-bases-with-unmergeable-entries A-B-diff3 A B
baseline multiple-merge-bases-with-renames A-B A B
baseline multiple-merge-bases-with-renames A-B-diff3 A B
baseline multiple-merge-bases-with-file-directory-conflicts A-B A B
baseline multiple-merge-bases-with-file-directory-conflicts A-B-diff3 A B
baseline recursive-merge-driver A-B A B

baseline rename-and-modification A-B A B
baseline symlink-modification A-B A B
baseline symlink-addition A-B A B
baseline added-file-vs-added-directory A-B A B
baseline added-symlink-blocks-gitlink-directory A-B A B
baseline gitlink-vs-renamed-symlink-directory-with-siblings A-B A B
baseline same-source-rewrites-after-consumed-path A-B A B
baseline rename-delete-after-consumed-path A-B A B
baseline modified-file-vs-gitlink-directory A-B A B
baseline relocated-addition-blocked-by-rename A-B A B
baseline nested-rename-blocks-relocated-addition A-B A B
baseline directory-rename-vs-directory-to-file A-B A B
baseline directory-rename-vs-renamed-file-replacement A-B A B
baseline unrelated-renames-overlapping-destinations A-B A B
baseline renamed-file-inside-renamed-directory A-B A B
baseline unrelated-renames-to-same-path-with-type-mismatch A-B A B
baseline renamed-file-vs-file-to-directory-with-siblings A-B A B
baseline identical-additions-with-different-relations A-B A B
baseline identical-deletions-with-different-relations A-B A B
baseline identical-rewrites-with-different-relations A-B A B
baseline type-change-to-symlink A-B A B
baseline distinct-types A-B A B
baseline conflicts-when-ours-is-processed-first A-B A B

baseline directory-rename-moves A-B A B
baseline directory-rename-moves A-B-dir-renames-true A B
baseline directory-rename-moves A-B-dir-renames-false A B
baseline directory-rename-split A-B A B
baseline directory-rename-split A-B-dir-renames-true A B
baseline directory-rename-split A-B-dir-renames-false A B
baseline directory-rename-file-in-way A-B A B
baseline directory-rename-file-in-way A-B-dir-renames-true A B
baseline directory-rename-file-in-way A-B-dir-renames-false A B
baseline directory-rename-collision A-B A B
baseline directory-rename-collision A-B-dir-renames-true A B
baseline directory-rename-collision A-B-dir-renames-false A B
baseline directory-rename-into-renamed-directory A-B A B
baseline directory-rename-into-renamed-directory A-B-dir-renames-true A B
baseline directory-rename-into-renamed-directory A-B-dir-renames-false A B
baseline directory-rename-to-same-destination A-B A B
baseline directory-rename-to-same-destination A-B-dir-renames-true A B
baseline directory-rename-to-same-destination A-B-dir-renames-false A B
baseline identical-rename-destinations-in-walk-order A-B A B
baseline identical-rename-destinations-in-deferred-order A-B A B
baseline rename-to-self-modified A-B A B
baseline rename-to-self-modified A-B-dir-renames-true A B
baseline rename-to-self-modified A-B-dir-renames-false A B
baseline rename-to-self-deleted A-B A B
baseline rename-to-self-deleted A-B-dir-renames-true A B
baseline rename-to-self-deleted A-B-dir-renames-false A B
baseline rename-to-self-renamed A-B A B
baseline rename-to-self-renamed A-B-dir-renames-true A B
baseline rename-to-self-renamed A-B-dir-renames-false A B
baseline rename-to-self-kept A-B A B
baseline rename-to-self-kept A-B-dir-renames-true A B
baseline rename-to-self-kept A-B-dir-renames-false A B

##
## Only once the tree-merges were performed can we refer to their objects
## when making tree-conflict resolution expectations. It's important
## to get these right.
##
(cd added-file-vs-added-directory
  # The ancestor is empty, so choosing it keeps neither addition.
  git read-tree main
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # Choosing ours keeps precisely the side selected by the merge direction.
  git read-tree A
  make_resolve_tree ours A B
  git read-tree B
  make_resolve_tree ours B A
)

(cd deleted-file-added-gitlink-directory
  # Both operations are compatible, so forced conflict resolution changes nothing:
  # the shared deletion applies and B's directory remains in both directions.
  git read-tree B
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A
  make_resolve_tree ours A B
  make_resolve_tree ours B A
)

(cd added-symlink-blocks-gitlink-directory
  # The ancestor is empty, so choosing it keeps neither addition in either direction.
  git read-tree main
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # Choosing ours keeps precisely the side named first: either A's symlink or B's
  # directory containing the nested gitlink.
  git read-tree A
  make_resolve_tree ours A B
  git read-tree B
  make_resolve_tree ours B A
)

(cd modified-file-vs-gitlink-directory
  # Ancestor resolution restores the original symlink in both directions.
  git read-tree main
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # Choosing ours keeps precisely the selected replacement.
  git read-tree A
  make_resolve_tree ours A B
  git read-tree B
  make_resolve_tree ours B A
)

(cd relocated-addition-blocked-by-rename
  # The explicit file rename prevents the inferred directory rename from relocating
  # B's addition through a non-tree. The changes are therefore compatible, and forced
  # conflict resolution has nothing to discard in either direction.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  make_resolve_tree ancestor A B
  make_resolve_tree ours A B

  IFS= read -r -d '' merged_reversed_tree_id <A-B-reversed.merge-info
  git read-tree "$merged_reversed_tree_id"
  make_resolve_tree ancestor B A
  make_resolve_tree ours B A
)

(cd directory-rename-vs-directory-to-file
  # Ancestor resolution applies neither rename and restores `a/a`.
  git read-tree main
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # Choosing ours keeps precisely the directory rename or directory-to-file
  # replacement selected by the merge direction.
  git read-tree A
  make_resolve_tree ours A B
  git read-tree B
  make_resolve_tree ours B A
)

(cd directory-rename-vs-renamed-file-replacement
  # The rename/delete of `h/h/a` is the only conflict; B's rename of `a` to `h/h` is clean.
  # FIXME: ancestor resolution keeps `h/h/a`, but B's clean rename onto `h/h` then replaces it,
  # so the ancestor's `h/h/a` is lost and the result is B's tree.
  git read-tree B
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # With A as ours, A's rename of `h/h/a` to `f/a` stays beside B's `h/h`: Git's merged tree.
  # With B as ours, B's deletion of `h/h/a` leaves B's tree.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  make_resolve_tree ours A B
  git read-tree B
  make_resolve_tree ours B A
)

(cd unrelated-renames-overlapping-destinations
  # A renames `h/b/a` to `c/b/a`, while B renames it to `a/a` and renames `a/a/a` to `c`.
  # Ancestor rejects the rename/rename and keeps `h/b/a`, so no directory of A's remains at
  # `c` and B's rename of `a/a/a` applies.
  git read-tree --empty
  git update-index --add --cacheinfo "100644,$(git rev-parse B:c),c"
  git update-index --add --cacheinfo "100644,$(git rev-parse main:h/b/a),h/b/a"
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # FIXME: with A as ours, B's rename of `a/a/a` to `c` is rejected in favor of A's `c/b/a`,
  # but `a/a/a` doesn't stay: only `c/b/a` remains.
  git read-tree --empty
  git update-index --add --cacheinfo "100644,$(git rev-parse A:c/b/a),c/b/a"
  make_resolve_tree ours A B

  # With B as ours, B's tree remains.
  git read-tree B
  make_resolve_tree ours B A
)

(cd renamed-file-inside-renamed-directory
  # Ancestor rejects the divergent rename of `h/b/a`, but the separate file
  # rename is cleanly relocated through A's directory rename to `c/h`.
  git read-tree main
  git update-index --force-remove a/a/a
  git update-index --add --cacheinfo "100644,$(git rev-parse main:a/a/a),c/h"
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # Choosing A still retains the independently relocated file from B.
  git read-tree A
  git update-index --force-remove a/a/a
  git update-index --add --cacheinfo "100644,$(git rev-parse main:a/a/a),c/h"
  make_resolve_tree ours A B

  # Choosing B keeps B's divergent rename, while the other file still follows
  # the independently resolved directory relocation.
  git read-tree B
  git update-index --force-remove h/h
  git update-index --add --cacheinfo "100644,$(git rev-parse main:a/a/a),c/h"
  make_resolve_tree ours B A
)

(cd unrelated-renames-to-same-path-with-type-mismatch
  # Neither side changes the other's rename source, so the merge detects no renames: each side
  # deletes its source cleanly and both add an entry of another type at `target`. Ancestor
  # resolution keeps neither addition.
  git read-tree --empty
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # Choosing ours keeps only the addition of the side named first.
  git read-tree A
  git update-index --force-remove file-source
  make_resolve_tree ours A B
  git read-tree B
  git update-index --force-remove link-source
  make_resolve_tree ours B A
)

(cd renamed-file-vs-file-to-directory-with-siblings
  # Ancestor resolution rejects the rename and the file-to-directory
  # replacement, restoring the base file.
  git read-tree main
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # Choosing A keeps its rename while still accepting B's non-conflicting
  # children at the now-vacant source path.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  make_resolve_tree ours A B

  # Choosing B rejects A's rename and keeps B's replacement directory.
  git read-tree B
  make_resolve_tree ours B A
)

(cd gitlink-vs-renamed-symlink-directory-with-siblings
  # The rename/delete of `a/a/a` is the only conflict. Ancestor keeps the symlink at its base
  # path, and the other changes of both sides apply.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  git update-index --force-remove h/a
  git update-index --add --cacheinfo "120000,$(git rev-parse main:a/a/a),a/a/a"
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # With A as ours, A's deletion of the symlink wins. With B as ours, B's rename does, which
  # is Git's merged tree.
  git read-tree "$merged_tree_id"
  git update-index --force-remove h/a
  make_resolve_tree ours A B
  git read-tree "$merged_tree_id"
  make_resolve_tree ours B A
)

(cd distinct-types
  # Neither side changes a path that the other side renames away, so the merge detects no
  # renames: each rename is a deletion of its source, which doesn't conflict, and an addition
  # at `item`. Every other change conflicts. Choosing the ancestor keeps the base entries and
  # none of the changes, apart from the deletions of those seven sources.
  git read-tree main
  git update-index --force-remove renamed-file-vs-added-gitlink/source renamed-link-vs-added-gitlink/source \
    renamed-gitlink-vs-added-file/source renamed-file-vs-renamed-gitlink/file-source \
    renamed-file-vs-renamed-gitlink/gitlink-source renamed-link-vs-renamed-gitlink/link-source \
    renamed-link-vs-renamed-gitlink/gitlink-source
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # Choosing ours keeps our side of every conflict and, of their changes, only those deletions.
  # With A as ours, that is A's tree without B's two gitlink sources. With B as ours, it is B's
  # tree without A's five sources.
  git read-tree A
  git update-index --force-remove renamed-file-vs-renamed-gitlink/gitlink-source \
    renamed-link-vs-renamed-gitlink/gitlink-source
  make_resolve_tree ours A B
  git read-tree B
  git update-index --force-remove renamed-file-vs-added-gitlink/source renamed-link-vs-added-gitlink/source \
    renamed-gitlink-vs-added-file/source renamed-file-vs-renamed-gitlink/file-source \
    renamed-link-vs-renamed-gitlink/link-source
  make_resolve_tree ours B A
)

(cd conflicts-when-ours-is-processed-first
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  IFS= read -r -d '' merged_reversed_tree_id <A-B-reversed.merge-info

  # Choosing the ancestor rejects the rename/delete and rename/rename conflicts and keeps `d`, `e` and `r`. The
  # content of `m` is still merged at B's destination, with conflict markers, and A's renames of the identical
  # files apply.
  git read-tree "$merged_tree_id"
  git update-index --force-remove d2 e2 r-A r-B
  git update-index --add --cacheinfo "100644,$(git rev-parse main:d),d"
  git update-index --add --cacheinfo "100644,$(git rev-parse main:e),e"
  git update-index --add --cacheinfo "100644,$(git rev-parse main:r),r"
  make_resolve_tree ancestor A B
  git read-tree "$merged_reversed_tree_id"
  git update-index --force-remove d2 e2 r-A r-B
  git update-index --add --cacheinfo "100644,$(git rev-parse main:d),d"
  git update-index --add --cacheinfo "100644,$(git rev-parse main:e),e"
  git update-index --add --cacheinfo "100644,$(git rev-parse main:r),r"
  make_resolve_tree ancestor B A

  # Choosing ours keeps A's deletion of `d` and its renames of `e` and `r` with A as ours, and B's renames of `d`
  # and `r` and its deletion of `e` with B as ours. `m` is merged at B's destination with the content of ours
  # where both sides changed it.
  git read-tree A
  git update-index --force-remove m
  git update-index --add --cacheinfo "100644,$(git rev-parse A:m),m2"
  make_resolve_tree ours A B
  git read-tree B
  git update-index --force-remove x y
  git update-index --add --cacheinfo "100644,$(git rev-parse A:x2),x2"
  git update-index --add --cacheinfo "100644,$(git rev-parse A:y2),y2"
  make_resolve_tree ours B A
)

(cd same-rename-with-content
  # Both sides rename `file` to `target`, which is no conflict for the ancestor to resolve. Choosing the ancestor
  # leaves Git's merged tree, with the conflict markers of the content merge at `target`.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  make_resolve_tree ancestor A B
  IFS= read -r -d '' merged_reversed_tree_id <A-B-reversed.merge-info
  git read-tree "$merged_reversed_tree_id"
  make_resolve_tree ancestor B A
)

# Forced resolution leaves the trees of these merges as they are: the files that directory renames moved stay where
# they were moved, the other directory rename conflicts change no file, and the remaining merges are clean.
for dir in directory-rename-moves directory-rename-split directory-rename-file-in-way directory-rename-collision \
  directory-rename-into-renamed-directory directory-rename-to-same-destination \
  identical-rename-destinations-in-walk-order identical-rename-destinations-in-deferred-order; do
  (cd "$dir"
    IFS= read -r -d '' merged_tree_id <A-B.merge-info
    git read-tree "$merged_tree_id"
    make_resolve_tree ancestor A B
    make_resolve_tree ours A B
    IFS= read -r -d '' merged_reversed_tree_id <A-B-reversed.merge-info
    git read-tree "$merged_reversed_tree_id"
    make_resolve_tree ancestor B A
    make_resolve_tree ours B A
  )
done

(cd simple
  rm .git/index
  # 'whatever' is tree-conflict, 'greeting' is content conflict with markers
  git update-index --index-info <<EOF
100644 $(oid 45b983be36b73c0788dc9cbcb76cbb80fc7bb057) 0	greeting
100644 $(oid 09c277aa66897c58157f57a374eacc63a407dcab) 0	numbers
100644 $(oid 5716ca5987cbf97d6bb54920bea6adde242d87e6) 0	whatever
EOF
  make_resolve_tree ours side1 side2

  rm .git/index
  git update-index --index-info <<EOF
100644 $(oid 092bfb9bdf74dd8cfd22e812151281ee9aa6f01a) 0	greeting
100644 $(oid 09c277aa66897c58157f57a374eacc63a407dcab) 0	numbers
100644 $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391) 0	whatever/empty
EOF
  make_resolve_tree ours side2 side1

  rm .git/index
  git update-index --index-info <<EOF
100644 $(oid 9dc97bdc2426e68423360e3e5299280b2cf6b8ff) 0	greeting
100644 $(oid 09c277aa66897c58157f57a374eacc63a407dcab) 0	numbers
100644 $(oid 257cc5642cb1a054f08cc83f2d943e56fd3ebe99) 0	whatever
EOF
  make_resolve_tree ancestor side1 side2

  rm .git/index
  git update-index --index-info <<EOF
100644 $(oid 1a2664a9924754c698e323f756f9f87f3f2fb337) 0	greeting
100644 $(oid 09c277aa66897c58157f57a374eacc63a407dcab) 0	numbers
100644 $(oid 257cc5642cb1a054f08cc83f2d943e56fd3ebe99) 0	whatever
EOF
  make_resolve_tree ancestor side2 side1

  rm .git/index
  git update-index --index-info <<EOF
100644 $(oid a4ae6e4709228b5da6001cb9d1cfa7736851e2a6) 0	greeting
100644 $(oid 257cc5642cb1a054f08cc83f2d943e56fd3ebe99) 0	whatever
100644 $(oid 542802a799ded74fa01c47ba2f8925e284a369e2) 0	Αυτά μου φαίνονται κινέζικα
EOF
  make_resolve_tree ancestor tweak1 side2

  rm .git/index
  git update-index --index-info <<EOF
100644 $(oid 45b983be36b73c0788dc9cbcb76cbb80fc7bb057) 0	greeting
100644 $(oid 5716ca5987cbf97d6bb54920bea6adde242d87e6) 0	whatever
100644 $(oid 65bc6a1e238f4bf05b28fd05240636e2cfb657e0) 0	Αυτά μου φαίνονται κινέζικα
EOF
  make_resolve_tree ours tweak1 side2

  rm .git/index
  git update-index --index-info <<EOF
100644 blob $(oid ea28dcd7f627a2a7bbd09daa679c452180617c9f)	greeting
100644 blob $(oid 257cc5642cb1a054f08cc83f2d943e56fd3ebe99)	whatever
100644 blob $(oid f801a62deed900f8a80ff35e3339474ad6352a93)	Αυτά μου φαίνονται κινέζικα
EOF
  make_resolve_tree ancestor side2 tweak1

  rm .git/index
  git update-index --index-info <<EOF
100644 blob $(oid 092bfb9bdf74dd8cfd22e812151281ee9aa6f01a)	greeting
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	whatever/empty
100644 blob $(oid 09c277aa66897c58157f57a374eacc63a407dcab)	Αυτά μου φαίνονται κινέζικα
EOF
  make_resolve_tree ours side2 tweak1
)

(cd rename-add-symlink
  rm .git/index
  # the symlink of 'bar' from A
  git update-index --index-info <<EOF
120000 blob $(oid 19102815663d23f8b75a47e7a01965dcdc96468c)	bar
EOF
  make_resolve_tree ours A B

  rm .git/index
  # the merged form of 'bar' from B, not replaced by symlink
  git update-index --index-info <<EOF
100644 blob $(oid b414108e81e5091fe0974a1858b4d0d22b107f70)	bar
EOF
  make_resolve_tree ours B A

  rm .git/index
  # foo is renamed to bar, type clash means neither A nor B can be added - empty tree
  # It is not able to 'get foo back', it can't track that currently.
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A
)

(cd deleted-file-added-dir-with-rename
  rm .git/index
  git update-index --index-info <<EOF
100644 blob $(oid df967b96a579e45a18b8251732d16804b2e56a55)	x
EOF
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A
)

(cd same-rename-and-file-to-directory
  # Forced ours keeps the agreed rename and resolves the added child's content in favor
  # of the current side. Use each complete side tree as the directional expectation.
  git read-tree A
  make_resolve_tree ours A B

  git read-tree B
  make_resolve_tree ours B A
)

(cd renames-to-same-destination
  # Neither side changes the other's rename source, so the merge detects no renames: each side
  # deletes its source cleanly and both add `target`, a content conflict that leaves no tree
  # conflict to resolve. Ancestor resolution keeps Git's merged tree with its conflict markers.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  make_resolve_tree ancestor A B
  IFS= read -r -d '' merged_reversed_tree_id <A-B-reversed.merge-info
  git read-tree "$merged_reversed_tree_id"
  make_resolve_tree ancestor B A

  # Ours resolves the content conflict with our side, so only our `target` remains.
  git read-tree --empty
  git update-index --add --cacheinfo "100644,$(git rev-parse A:target),target"
  make_resolve_tree ours A B
  git read-tree --empty
  git update-index --add --cacheinfo "100644,$(git rev-parse B:target),target"
  make_resolve_tree ours B A
)

(cd identical-renames-to-same-destination
  # Identical entries make the two renames compatible rather than a tree
  # conflict, so conflict-resolution policy must not alter the clean result.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A
  make_resolve_tree ours A B
  make_resolve_tree ours B A
)

(cd identical-renames-to-same-destination-with-mode-change
  # Neither side changes the other's rename source, so the merge detects no renames: both
  # sources are deleted cleanly and both sides add `target` with the same content but different
  # executable bits. That mode conflict is no tree conflict and stays unresolved with every
  # policy, so ancestor resolution keeps Git's merged tree, and no ours tree is recorded: the
  # test expects ours to resolve every conflict.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  make_resolve_tree ancestor A B
  IFS= read -r -d '' merged_reversed_tree_id <A-B-reversed.merge-info
  git read-tree "$merged_reversed_tree_id"
  make_resolve_tree ancestor B A
)

(cd rename-rename-plus-content
  rm .git/index
  # both sides rename 'foo' into something else.
  git update-index --index-info <<EOF
100644 blob $(oid 8a1218a1024a212bb3db30becd860315f9f3ac52)	foo
EOF
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  rm .git/index
  # 'bar' is the name in 'A', and there is a merge with the content from 'B'
  # which we auto-resolve.
  git update-index --index-info <<EOF
100644 blob $(oid d0549c3d3c96a464289f3b820b7d96aedc58924b)	bar
EOF
  make_resolve_tree ours A B

  rm .git/index
  # 'baz' is the name in 'B', and there is a merge with the content from 'A'
  # which we auto-resolve.
  git update-index --index-info <<EOF
100644 blob $(oid b414108e81e5091fe0974a1858b4d0d22b107f70)	baz
EOF
  make_resolve_tree ours B A
)

(cd rename-add-delete
  # A deletes `foo` and adds `bar`, B renames `foo` to `bar`: a rename/delete of `foo`. Ancestor
  # rejects B's rename and keeps `foo` at its base version, and A's `bar` then has no conflict.
  git read-tree A
  git update-index --add --cacheinfo "100644,$(git rev-parse main:foo),foo"
  make_resolve_tree ancestor A B
  # In the reversed merge gix reports only the add/add content conflict at `bar`, not Git's
  # rename/delete, so there is no tree conflict to resolve and B A isn't recorded.

  # Ours keeps A's deletion of `foo` and A's `bar`: A's tree.
  rm .git/index
  git update-index --index-info <<EOF
100644 blob $(oid f286e5cdd97ac6895438ea4548638bb98ac9bd6b)	bar
EOF
  make_resolve_tree ours A B
)

(cd rename-rename-delete-delete
  rm .git/index
  # 'A' deletes 'bar' and 'B' turns 'bar' into 'baz'. 'A' renames 'foo' into 'bar', and
  # 'B' deletes 'foo'. 'ancestor' resolves to avoid any edits, leaving the state from 'main'.
  git update-index --index-info <<EOF
100644 blob $(oid 5716ca5987cbf97d6bb54920bea6adde242d87e6)	bar
100644 blob $(oid 257cc5642cb1a054f08cc83f2d943e56fd3ebe99)	foo
EOF
  make_resolve_tree ancestor A B
  # this works in reverse as well (this time).
  make_resolve_tree ancestor B A

  rm .git/index
  # As 'ours' is a deletion of 'foo', it goes through, but we also acknowledge 'theirs'
  # as it gives better results, so end up with `baz`.
  git update-index --index-info <<EOF
100644 blob $(oid 257cc5642cb1a054f08cc83f2d943e56fd3ebe99)	baz
EOF
  make_resolve_tree ours A B

  rm .git/index
  # Here we end up in exactly the same spot as if we'd do a normal merge,
  # which ends `baz` in a conflict. However, with content-merges set to 'ours'
  # it ends up like it should, giving a good result.
  git update-index --index-info <<EOF
  100644 blob $(oid 5716ca5987cbf97d6bb54920bea6adde242d87e6)	baz
EOF
  make_resolve_tree ours B A
)

(cd super-1
  # Each of the ancestor files are renamed in a conflicting way, and here
  # with ancestor choice, nothing happens, making this equivalent to `main`
  git checkout main
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  rm .git/index
  # We do indeed perform the renames like this, and the content is merges as well as possible,
  # (here) configured to content-merge with 'ours' as well where needed.
  git update-index --index-info <<EOF
100644 blob $(oid 4b5599c7c2ed4390417d9699bec86144a386873d)	four
100644 blob $(oid 64012489f118cb4011c8902b4a635f70dcb0c0ca)	six
100644 blob $(oid e33f5e94470d3b5fa0220ff6a9cabb78a3f72fa3)	two
EOF
  make_resolve_tree ours A B

  rm .git/index
  # The same, but from the other side.
  git update-index --index-info <<EOF
  100644 blob $(oid 64012489f118cb4011c8902b4a635f70dcb0c0ca)	four
  100644 blob $(oid e33f5e94470d3b5fa0220ff6a9cabb78a3f72fa3)	six
  100644 blob $(oid 4178ea6795c4c3e07b4e17e6a04aa49584b07ecd)	two
EOF
  make_resolve_tree ours B A
)

(cd super-2
  rm .git/index
  # 'B' changes foo, and moves it into 'olddir/bar', but `A' deleted 'foo', and adds 'newdir/bar/file'
  # after renaming 'olddir' to 'newdir'.
  # As `B` only has a single change that gets dropped when it clashes with the deletion of 'foo',
  # all other changes of 'A' can just be applied without any conflict whatsoever.
  git update-index --index-info <<EOF
100644 blob $(oid 8a1218a1024a212bb3db30becd860315f9f3ac52)	foo
100644 blob $(oid 78981922613b2afb6025042ff6bd878ac1994e85)	newdir/a
100644 blob $(oid 61780798228d17af2d34fce4cfbdf35556832472)	newdir/b
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	newdir/bar/file
100644 blob $(oid f2ad6c76f0115a6ba5b00456a849810e7ec0af20)	newdir/c
EOF
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  rm .git/index
  # Similar to the ancestor version, but now we choose 'ours', so the rename of 'foo' gets
  # dropped and it just gets deleted. Everything else is then 'A'.
  git update-index --index-info <<EOF
100644 blob $(oid 78981922613b2afb6025042ff6bd878ac1994e85)	newdir/a
100644 blob $(oid 61780798228d17af2d34fce4cfbdf35556832472)	newdir/b
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	newdir/bar/file
100644 blob $(oid f2ad6c76f0115a6ba5b00456a849810e7ec0af20)	newdir/c
EOF
  make_resolve_tree ours A B

  rm .git/index
  # 'B' changes 'foo' and moves it to 'olddir/bar', which gets tracked to be
  # 'newdir/bar' and is taken verbatim. The clash that it finds it
  # resolves in 'B's favor, leaving only 'newdir/bar'.
  git update-index --index-info <<EOF
100644 blob $(oid 78981922613b2afb6025042ff6bd878ac1994e85)	newdir/a
100644 blob $(oid 61780798228d17af2d34fce4cfbdf35556832472)	newdir/b
100644 blob $(oid b414108e81e5091fe0974a1858b4d0d22b107f70)	newdir/bar
100644 blob $(oid f2ad6c76f0115a6ba5b00456a849810e7ec0af20)	newdir/c
EOF
  make_resolve_tree ours B A
)

(cd conflicting-rename
  # A renames `a` to `a-renamed`, B renames it to `a-different`, and both change `x.f` and
  # `sub/y.f` mergeably. Each of the two files is renamed differently by the two sides, the
  # only tree conflicts. Empty files are never renamed in merges, so `w` and `sub/z` are
  # deleted and added again on both sides without conflict.
  # Ancestor keeps both files at their base versions and none of their destinations.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  git update-index --force-remove a-renamed/x.f a-renamed/sub/y.f a-different/x.f a-different/sub/y.f
  git update-index --add --cacheinfo "100644,$(git rev-parse main:a/x.f),a/x.f"
  git update-index --add --cacheinfo "100644,$(git rev-parse main:a/sub/y.f),a/sub/y.f"
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # Ours keeps the merged contents at our destinations only.
  git read-tree "$merged_tree_id"
  git update-index --force-remove a-different/x.f a-different/sub/y.f
  make_resolve_tree ours A B
  IFS= read -r -d '' merged_reversed_tree_id <A-B-reversed.merge-info
  git read-tree "$merged_reversed_tree_id"
  git update-index --force-remove a-renamed/x.f a-renamed/sub/y.f
  make_resolve_tree ours B A
)

(cd conflicting-rename-2
  # Like `conflicting-rename`, but only `a/sub` is renamed, to `a/sub-renamed` by A and to
  # `a/sub-different` by B, so `a/sub/y.f` is the only tree conflict.
  # Ancestor keeps it at its base version and neither destination.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  git update-index --force-remove a/sub-renamed/y.f a/sub-different/y.f
  git update-index --add --cacheinfo "100644,$(git rev-parse main:a/sub/y.f),a/sub/y.f"
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # Ours keeps the merged content at our destination only.
  git read-tree "$merged_tree_id"
  git update-index --force-remove a/sub-different/y.f
  make_resolve_tree ours A B
  IFS= read -r -d '' merged_reversed_tree_id <A-B-reversed.merge-info
  git read-tree "$merged_reversed_tree_id"
  git update-index --force-remove a/sub-renamed/y.f
  make_resolve_tree ours B A
)

(cd conflicting-rename-complex
  # A renames `a` to `a-renamed`, while B moves `a/sub` up to `a` and deletes the rest of `a`.
  # A's directory rename moves B's `a/y.f` and `a/z` to `a-renamed`. The tree conflicts are
  # `x.f`, which A renames and B deletes, and `sub/y.f`, which each side renames differently.
  # Ancestor keeps both files at their base versions and none of their destinations, while
  # B's `a/z`, which only A's directory rename moved, stays at `a-renamed/z`.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  git update-index --force-remove a-renamed/sub/y.f a-renamed/x.f a-renamed/y.f
  git update-index --add --cacheinfo "100644,$(git rev-parse main:a/x.f),a/x.f"
  git update-index --add --cacheinfo "100644,$(git rev-parse main:a/sub/y.f),a/sub/y.f"
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # With A as ours, A's renames win with their merged contents, while B's `a/z` stays where
  # A's directory rename moved it.
  rm .git/index
  git update-index --index-info <<EOF
  100644 blob $(oid b414108e81e5091fe0974a1858b4d0d22b107f70)	a-renamed/sub/y.f
  100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a-renamed/sub/z
  100644 blob $(oid 8a1218a1024a212bb3db30becd860315f9f3ac52)	a-renamed/x.f
  100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a-renamed/z
  100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a-renamed/w
EOF
  make_resolve_tree ours A B

  # With B as ours, B's deletion of `x.f` and B's rename of `sub/y.f` win.
  git read-tree "$merged_tree_id"
  git update-index --force-remove a-renamed/sub/y.f a-renamed/x.f
  make_resolve_tree ours B A
)

(cd renamed-symlink-with-conflict
  rm .git/index
  # 'A' changes 'a/x.f' and renames the 'link', while 'B' also changes 'a/x.f' in a mergable fashion,
  # while renaming 'link' to something else which is where the conflict comes from.
  # Choosing the 'ancestor' means to not rename 'link' at all, while merging the file.
  git update-index --index-info <<EOF
100644 blob $(oid b414108e81e5091fe0974a1858b4d0d22b107f70)	a/x.f
120000 blob $(oid e29fa63dae4ccf0788897a7025da868083178fdf)	link
EOF
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  rm .git/index
  # Here we choose the name of 'link' in 'A'.
  git update-index --index-info <<EOF
100644 blob $(oid b414108e81e5091fe0974a1858b4d0d22b107f70)	a/x.f
120000 blob $(oid e29fa63dae4ccf0788897a7025da868083178fdf)	link-renamed
EOF
  make_resolve_tree ours A B

  rm .git/index
  # Here we choose the name of 'link' in 'B'.
  git update-index --index-info <<EOF
100644 blob $(oid b414108e81e5091fe0974a1858b4d0d22b107f70)	a/x.f
120000 blob $(oid e29fa63dae4ccf0788897a7025da868083178fdf)	link-different
EOF
  make_resolve_tree ours B A
)

(cd type-change-and-renamed
  rm .git/index
  # 'A' changes `link` to a file, while 'B' keeps the link, but renames it.
  # 'ancestor' just keeps the original version of 'link'
  git update-index --index-info <<EOF
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a/x.f
120000 blob $(oid e29fa63dae4ccf0788897a7025da868083178fdf)	link
EOF
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  rm .git/index
  # 'A' changes the type of 'link' to be a file, and that's what's used here.
  git update-index --index-info <<EOF
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a/x.f
100644 blob $(oid f89a08d1e226b9a319210641b63b07dcf0bd705f)	link
EOF
  make_resolve_tree ours A B

  rm .git/index
  # 'B' renames the link, and that is picked up as well.
  git update-index --index-info <<EOF
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a/x.f
120000 blob $(oid e29fa63dae4ccf0788897a7025da868083178fdf)	link-renamed
EOF
  make_resolve_tree ours B A
)

(cd change-and-delete
  rm .git/index
  # 'A' changes 'link' to be a file, and changes the file, while 'B' deletes everything,
  # causing each file to be irreconcilable.
  # 'ancestor' keeps everything as is.
  git update-index --index-info <<EOF
100644 blob $(oid 44065282f89b9bd6439ed2e4674721383fd987eb)	a/x.f
120000 blob $(oid e29fa63dae4ccf0788897a7025da868083178fdf)	link
EOF
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  rm .git/index
  # 'A' changes everything, and that's the change we keep.
  git update-index --index-info <<EOF
100644 blob $(oid b414108e81e5091fe0974a1858b4d0d22b107f70)	a/x.f
100644 blob $(oid f89a08d1e226b9a319210641b63b07dcf0bd705f)	link
EOF
  make_resolve_tree ours A B

  rm .git/index
  # 'B' deletes everything, which is what we keep.
  make_resolve_tree ours B A
)

(cd submodule-both-modify
  rm .git/index
  # There is only one submodule. 'A' and 'B' change it in a fast-forwardable manner,
  # but we can't handle this at all yet, and thus have to consider it irreconcilable.
  # The 'ancestor' resolution just keeps what was.
  git update-index --index-info <<EOF
160000 commit $(oid e835c0c403c8e494c0ca98f3d25d0b8464c18d38)	sub
EOF
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  rm .git/index
  # Otherwise it's the state of 'A'.
  git update-index --index-info <<EOF
160000 commit $(oid 64466ebdff775ad618d9cc993cf52840e0af528c)	sub
EOF
  make_resolve_tree ours A B

  rm .git/index
  # Otherwise it's the state of 'B'.
  git update-index --index-info <<EOF
160000 commit $(oid ea6eb701e03c2497915c25a851f3da8f8e362ca0)	sub
EOF
  make_resolve_tree ours B A
)

(cd gitlink-replaced-by-files
  # Resolving tree conflicts with the ancestor does not resolve the content
  # conflict, so retain Git's directional conflict-marker trees.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  make_resolve_tree ancestor A B
  IFS= read -r -d '' merged_reversed_tree_id <A-B-reversed.merge-info
  git read-tree "$merged_reversed_tree_id"
  make_resolve_tree ancestor B A

  # ResolveWith::Ours also configures the blob merge to keep the current side.
  git read-tree A
  make_resolve_tree ours A B
  git read-tree B
  make_resolve_tree ours B A
)

(cd multiple-merge-bases
  rm .git/index
  # 'A' modifies and 'B' deletes the single file in the tree.
  # 'ancestor' keeps the original, which is already the result of the merge of
  # the merge-bases.
  git update-index --index-info <<EOF
100644 blob $(oid 09c277aa66897c58157f57a374eacc63a407dcab)	content
EOF
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  rm .git/index
  # 'A' keeps the modified version.
  git update-index --index-info <<EOF
100644 blob $(oid 0a6a0ba83635bc00e7c79a4b5b6e50381385c1af)	content
EOF
  make_resolve_tree ours A B

  rm .git/index
  # 'B' applies the deletion to get an empty tree.
  make_resolve_tree ours B A
)

(cd non-tree-to-tree
  rm .git/index
  # 'A' changes the single file 'a', while 'B' replaces it with a directory structure 'a',
  # without a rename though.
  # We manage to pick the 'ancestor', just a single file, while discarding all follow-up changes.
  git update-index --index-info <<EOF
100644 blob $(oid 44065282f89b9bd6439ed2e4674721383fd987eb)	a
EOF
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  rm .git/index
  # Picks 'A' which is just a single, modified (and mergable) file.
  git update-index --index-info <<EOF
100644 blob $(oid b414108e81e5091fe0974a1858b4d0d22b107f70)	a
EOF
  make_resolve_tree ours A B

  rm .git/index
  # Picks 'B' which is a whole directory tree.
  git update-index --index-info <<EOF
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a/d
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a/e
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a/sub/b
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a/sub/c
EOF
  make_resolve_tree ours B A
)

(cd tree-to-non-tree
  rm .git/index
  # 'A' modifies a nested file 'a/sub/b', while 'B' replaces 'a/' with file 'a'.
  # Ignore *their* changes for 'ancestor' resolution, and the modification,
  # but apply all others which are deletions.
  git update-index --index-info <<EOF
100644 blob $(oid 44065282f89b9bd6439ed2e4674721383fd987eb)	a/sub/b
EOF
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A
  # *ours* is the same as ancestor, we want to keep the tree and changes, but it only
  # applies to the one modification that protects the change.
  rm .git/index
  git update-index --index-info <<EOF
100644 blob $(oid b414108e81e5091fe0974a1858b4d0d22b107f70)	a/sub/b
EOF
  make_resolve_tree ours A B

  rm .git/index
  # Now *ours* is the single file which replaces a tree.
  git update-index --index-info <<EOF
100644 blob $(oid fa49b077972391ad58037050f2a75f74e3671e92)	a
EOF
  make_resolve_tree ours B A
)

(cd tree-to-non-tree-with-rename
  rm .git/index
  # 'A' modifies a nested file 'a/sub/b', while 'B' replaces 'a/' with file 'a'.
  # I let it pass as it's an edge-case to some extent.
  git update-index --index-info <<EOF
100644 blob $(oid 44065282f89b9bd6439ed2e4674721383fd987eb)	a/sub/b
EOF
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  rm .git/index
  # Thanks to the rename, this version keeps one additional file, 'a/e'
  git update-index --index-info <<EOF
  100644 blob $(oid b414108e81e5091fe0974a1858b4d0d22b107f70)	a/sub/b
EOF
  make_resolve_tree ours A B

  rm .git/index
  # Now *ours* is the single file which replaces a tree.
  git update-index --index-info <<EOF
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a
EOF
  make_resolve_tree ours B A
)

(cd non-tree-to-tree-with-rename
  # 'A' changes the single file 'a', while 'B' replaces it with a directory structure 'a'.
  # The rename now sends this off-course, as it removes the previously 'protective' entry
  # and thus makes all changes from 'B' succeed without us detecting any problem with that.
  # Also, here we don't actually have irreconcilable tree-changes because of that.
  rm .git/index
  git update-index --index-info <<EOF
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a/d
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a/e
100644 blob $(oid b414108e81e5091fe0974a1858b4d0d22b107f70)	a/sub/b
100644 blob $(oid e69de29bb2d1d6434b8b29ae775ad8c2e48c5391)	a/sub/c
EOF
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # Thanks to the rename, this time there isn't even an irreconcilable conflict.
  # This is deactivated as the test-suite can't handle this (one) special case.
  # The state really is the one from the ancestors, as there are no irreconcilable
  # tree changes, it's all the same. But the test expects to see changes when 'ours'
  # is chosen so we can't easily run this here.
  #  make_resolve_tree ours A B
  #  make_resolve_tree ours B A
)

(cd rename-within-rename
  # A renames `a` to `a-renamed`, B renames `a/sub` to `a/sub-renamed`, and both change all
  # contents mergeably. B adds no file directly in `a`, so A's directory rename doesn't move
  # B's `a/sub-renamed`, and `a/sub/y.f`, which each side renames differently, is the only
  # tree conflict. Empty files are never renamed in merges, so `w` and both `z` are deleted
  # and added again without conflict.
  # Ancestor keeps `a/sub/y.f` at its base version and neither destination.
  IFS= read -r -d '' merged_tree_id <A-B.merge-info
  git read-tree "$merged_tree_id"
  git update-index --force-remove a-renamed/sub/y.f a/sub-renamed/y.f
  git update-index --add --cacheinfo "100644,$(git rev-parse main:a/sub/y.f),a/sub/y.f"
  make_resolve_tree ancestor A B
  make_resolve_tree ancestor B A

  # Ours keeps the merged content at our destination only.
  git read-tree "$merged_tree_id"
  git update-index --force-remove a/sub-renamed/y.f
  make_resolve_tree ours A B
  IFS= read -r -d '' merged_reversed_tree_id <A-B-reversed.merge-info
  git read-tree "$merged_reversed_tree_id"
  git update-index --force-remove a-renamed/sub/y.f
  make_resolve_tree ours B A
)
