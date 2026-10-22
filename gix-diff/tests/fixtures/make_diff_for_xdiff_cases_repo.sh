#!/usr/bin/env bash
set -eu -o pipefail

# Pairs of files on which Git's diff algorithms take one particular decision, with Git's diffs of
# each pair as baselines: `<case>_before-<case>_after.<algorithm>.baseline` with the indent
# heuristic and `<case>_before-<case>_after.<algorithm>.no-indent.baseline` without it, for the
# `myers`, `minimal` and `histogram` algorithms.

# `git diff --no-index` still reads the configuration and attributes of the repository it runs in,
# so the diffs are made in a repository of their own.
git init -q
mkdir assets

# Writes the files of case $1, with $2 before the change and $3 after it, given with backslash escapes.
function pair() {
  local name="${1:?the name of the case}"
  printf '%b' "${2:?the file before the change}" >"assets/${name}_before.blob"
  printf '%b' "${3:?the file after the change}" >"assets/${name}_after.blob"
}

# Prints line $2 $1 times.
function lines() {
  local count="${1:?how often to print the line}" i
  for ((i = 0; i < count; i++)); do
    printf '%s\n' "$2"
  done
}

# Writes case $1: $3 lines drawn from $4 distinct lines by a linear congruential generator seeded
# with $2, and a copy in which, in every second run of $5 lines, $6 percent of the lines are
# deleted, replaced or followed by an inserted line.
function pseudo_random_pair() {
  local name="${1:?the name of the case}" x="${2:?the seed}" count="${3:?the number of lines}"
  local symbols="${4:?the number of distinct lines}" run="${5:?the length of a run}"
  local percent="${6:?the share of changed lines in a changed run}" i line
  {
    for ((i = 0; i < count; i++)); do
      x=$(( (x * 75 + 74) % 65537 ))
      line="l$(( x % symbols ))"
      printf '%s\n' "$line" >&3
      if (( (i / run) % 2 == 1 )); then
        x=$(( (x * 75 + 74) % 65537 ))
        if (( x % 100 < percent )); then
          x=$(( (x * 75 + 74) % 65537 ))
          case $(( x % 3 )) in
          0)
            continue
            ;;
          1)
            x=$(( (x * 75 + 74) % 65537 ))
            printf 's%d\n' $(( x % symbols ))
            continue
            ;;
          *)
            printf '%s\n' "$line"
            x=$(( (x * 75 + 74) % 65537 ))
            printf 'i%d\n' $(( x % symbols ))
            continue
            ;;
          esac
        fi
      fi
      printf '%s\n' "$line"
    done
  } 3>"assets/${name}_before.blob" >"assets/${name}_after.blob"
}

# Git's histogram diff extends matches around lines that occur rarely in the old file and looks for
# them in the whole files, including the lines that both files start or end with, so such a line
# can join a match elsewhere in the other file.
pair whole_file_a 'b\nc\nc\n' 'c\nb\nc\n'
pair whole_file_b '\na\n\n' '\n\na\n'
pair whole_file_c 't\no\ni\ng\nc\n' 't\ng\nc\nt\no\ni\n'
pair whole_file_d '    }\n\na;\n\n\n' '\n\n}\n\n'
pair whole_file_e '\n\n\n    b;\n' '\na;\n\n\n'

# Hunks that can slide are moved in the old file first, then in the new file, as Git's
# `xdl_change_compact()` does. In this pair, the order decides where the hunks end up.
pair compaction_order 'a\na\nb\nb\na\n' 'b\na\na\n'

# Before Myers' algorithm runs, Git leaves out the lines that both files start or end with, the
# lines that the other file lacks and frequent lines among those. It counts how often lines occur,
# and how often is frequent, over the whole files, and the lines it leaves out decide which of
# several shortest diffs the search finds.
pair line_counts_a 'a\nc\na\nc\nc\na\n' 'a\nc\nc\n'
pair line_counts_b 'j\n\n\ng\nj\n' '\nj\n'
pair line_counts_c 'a\nc\n' 'a\na\nc\nc\ne\n'
pair line_counts_d '\n\n    b;\n' '\na;\n    b;\n    b;\n    c;\n'
pair line_counts_e '}\n\n' '}\n}\n\n\n}\n'

# With `--minimal`, Git leaves no frequent line out. The only empty line of the old file occurs
# often enough in the new file to be left out otherwise, and then no line matches.
pair minimal_frequent_line '      a;\n      a;\n    }\nb;\n  c;\n\n  d;\n  e;\n' '\n\n\n\n'

# After a hunk of a histogram diff moved or absorbed another one, Git diffs it again with Myers'
# algorithm against the hunk it faces in the other file, which turns the lines they share into
# unchanged lines.
pair rediff_a 'a\n\na\na\n' 'a\na\na\n'
pair rediff_b 'a\n\t}\na\n\t}\n' 'a\na\n\t}\n'
pair rediff_c 'b\nc\n}\n' '}\nb\n}\n'

# Git's histogram diff extends matches only around lines that occur at most 64 times in the old
# file, and falls back to Myers' algorithm when there are none. With 64 `b` lines it matches the run
# of `b`, with 65 Myers' algorithm matches the longer run of `a`.
{ lines 65 a; lines 64 b; } >assets/fallback_64_before.blob
{ lines 64 b; lines 65 a; } >assets/fallback_64_after.blob
{ lines 66 a; lines 65 b; } >assets/fallback_65_before.blob
{ lines 65 b; lines 66 a; } >assets/fallback_65_after.blob

# The shortest diff of these files of 2000 lines changes 1236 lines. Without `--minimal`, Git's
# `xdl_split()` stops searching for the middle of the shortest diff after 256 steps and takes the
# path that reached furthest instead.
pseudo_random_pair many_changes 1 2000 97 25 90

# Git's indent heuristic places a hunk that can slide by the indentation around its possible
# positions. Only spaces and tabs indent a line: a form feed ends the indentation like any other
# character, and a line with one isn't blank.
pair form_feed_a '\f        goto out;\n' '\f        goto out;\n        a;\n\f        goto out;\n'
{ lines 2 ''; printf '\f\n'; lines 18 ''; } >assets/form_feed_b_before.blob
{ lines 2 ''; printf '\f\n'; lines 8 ''; } >assets/form_feed_b_after.blob

# After 20 blank lines, the indent heuristic stops looking for a line with indentation and continues
# as if a line without indentation followed, also when the blank lines reach the start or the end
# of the file.
{ lines 10 ''; printf '    a;\n'; } >assets/blank_run_start_before.blob
{ lines 23 ''; printf '    a;\n'; } >assets/blank_run_start_after.blob
lines 29 '' >assets/blank_run_end_before.blob
lines 17 '' >assets/blank_run_end_after.blob

# A line indented less than the one above it starts a block only if a line indented more follows it,
# and otherwise ends one. At the end of the file, no line follows.
pair dedent '\t\tb;\n\n\ta;\n  e;\n' '\t\tb;\n\n\ta;\n  e;\n\ta;\n\ta;\n  e;\n'

for before in assets/*_before.blob; do
  name="${before#assets/}"
  name="${name%_before.blob}"
  after="assets/${name}_after.blob"
  for algorithm in myers minimal histogram; do
    git -c diff.algorithm="$algorithm" diff --no-index --no-ext-diff --no-color --indent-heuristic \
      "$before" "$after" >"${name}_before-${name}_after.$algorithm.baseline" || test $? -eq 1
    git -c diff.algorithm="$algorithm" diff --no-index --no-ext-diff --no-color --no-indent-heuristic \
      "$before" "$after" >"${name}_before-${name}_after.$algorithm.no-indent.baseline" || test $? -eq 1
  done
done
