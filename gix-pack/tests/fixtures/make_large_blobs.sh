#!/usr/bin/env bash
set -eu -o pipefail

# Packs with blobs above the `core.bigFileThreshold` of 4 KiB that the tests set, so that `git index-pack` streams these
# blobs while it indexes the packs, as it streams blobs above 512 MiB by default. The packs are written with Git's
# default threshold, so that Git stores the large blobs as deltas of each other where they resemble each other, as it
# stores small files.
#
# The history of `main` has three commits, and an annotated tag `v1` points to the last one:
# 1. `whole.bin` (96 KiB), `README` and `src/numbers`. No other object resembles `whole.bin`, so every pack stores it
#    whole and no delta needs it as its base.
# 2. `versioned.bin` (64 KiB) and a changed `README`.
# 3. `versioned.bin` with 2 KiB inserted in its middle, `extra.bin` (48 KiB) and a changed `src/numbers`. The packs
#    of the whole history store the new version of `versioned.bin` whole, as the base of the old one, which they
#    store as a delta.
#
# The large files consist of printable characters from generators that differ by file, so that zlib compresses them
# to about four fifths of their size and no two of them share runs of characters. All other objects are smaller than
# 4 KiB.
#
# `git repack -a -d` puts the objects of the repository into one pack, the object database that completes the thin pack.
# `packs/` holds packs that `git pack-objects` writes, each beside the index that
# `git -c core.bigFileThreshold=4k index-pack` writes for it:
# * `whole.pack`: the objects of the first commit, so that its only large blob is stored whole and no delta needs it.
# * `ofs.pack`: all objects, whose deltas refer to their bases by offset (`--delta-base-offset`).
# * `ref.pack`: all objects, whose deltas refer to their bases by id.
# `thin.bundle` is the bundle that `git bundle create` writes for `main~1..main` and `v1`. Its pack is thin: it stores
# the new version of `versioned.bin` as a delta of the old one, which only the second commit has, and refers to that
# base by id. The large files of the work tree are removed at the end, as the tests only read objects.

# printable <seed> <count> writes <count> printable characters and no newline. They come from the sum of three
# generators of Wichmann and Hill, which <seed> starts at other points; their products stay below 2^23, so that every
# awk computes them exactly.
printable() {
  awk -v seed="$1" -v count="$2" 'BEGIN {
    x = seed % 30268 + 1; y = seed * 7 % 30306 + 1; z = seed * 13 % 30322 + 1
    for (i = 0; i < count; i++) {
      x = x * 171 % 30269; y = y * 172 % 30307; z = z * 170 % 30323
      printf "%c", 33 + (x + y + z) % 94
    }
  }'
}

git init -q
printable 1 98304 > whole.bin
echo 'packs with large blobs' > README
mkdir src
seq 1 500 > src/numbers
git add -A
git commit -q -m first

printable 2 65536 > versioned.bin
echo 'the second commit adds versioned.bin' >> README
git add -A
git commit -q -m second

{
  head -c 32768 versioned.bin
  printable 3 2048
  tail -c +32769 versioned.bin
} > versioned.bin.new
mv versioned.bin.new versioned.bin
printable 4 49152 > extra.bin
seq 1 2 1000 > src/numbers
git add -A
git commit -q -m third
git tag -a -m 'version 1' v1

mkdir packs
echo main~2 | git -c pack.threads=1 pack-objects -q --revs --delta-base-offset --stdout > packs/whole.pack
git -c pack.threads=1 pack-objects -q --revs --all --delta-base-offset --stdout < /dev/null > packs/ofs.pack
git -c pack.threads=1 pack-objects -q --revs --all --stdout < /dev/null > packs/ref.pack
for pack in packs/*.pack; do
  git -c core.bigFileThreshold=4k index-pack --no-rev-index "$pack" > /dev/null
done
git -c pack.threads=1 bundle create -q thin.bundle main~1..main v1
git -c pack.threads=1 repack -q -a -d
rm whole.bin versioned.bin extra.bin
