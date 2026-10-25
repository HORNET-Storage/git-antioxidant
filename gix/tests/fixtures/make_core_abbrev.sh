#!/usr/bin/env bash
set -eu -o pipefail

# `abbrev.baseline` has a line for each value of `core.abbrev` below: the value in single quotes, a tab, and the id of
# HEAD that `git rev-parse --short` prints with that value, or `error` if Git refuses the value. `git -c` passes each
# value as it is written here. With two objects in the repository, every abbreviation of HEAD has the requested length.
#
# Values with leading whitespace are left out, as Git's `strtoimax()` skips it and gix-config's integer parser doesn't,
# and so are `0b` prefixes, which only some C libraries read.
#
# Before version 2.46, Git refuses lengths above the hash length instead of printing the full id.

git init -q
git commit -q --allow-empty -m empty

values=(
  auto AUTO Auto ' auto' 'auto ' autox
  '' false FALSE no NO Off 'false '
  true yes on
  4 7 12 +12 40 50 64 65 1000 0x10 0X10 010 1k 4K 1g 2147483647 2097151k 0x7fffffff
  3 1 0 -0 0k -1 -0x5 2147483648 2097152k 2g 0x80000000 9223372036854775807k
  7x 08 0x 4kb '12 ' '   ' foo
)
for value in "${values[@]}"; do
  if short=$(git -c "core.abbrev=$value" rev-parse --short HEAD 2>/dev/null); then
    printf "'%s'\t%s\n" "$value" "$short"
  else
    printf "'%s'\terror\n" "$value"
  fi
done >abbrev.baseline
