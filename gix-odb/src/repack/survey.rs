use std::{
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use gix_error::{Result, ResultExt, message};

/// A pack in the pack directory of an object database, as [`Survey::at()`] found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    /// The path of its index, which ends in `.idx`.
    pub index_path: PathBuf,
    /// The amount of objects it holds.
    pub num_objects: u32,
    /// When its data was last modified. Of the combined packs that hold an object, the object is copied from the one
    /// modified last, as git does.
    pub modified: SystemTime,
    /// Whether a `.keep` file protects it from being combined and removed.
    pub keep: bool,
    /// Whether a `.promisor` file marks it as received from a promisor remote.
    pub promisor: bool,
    /// Whether a `.mtimes` file makes it a cruft pack.
    pub cruft: bool,
}

impl Pack {
    /// The path of its data, which ends in `.pack`.
    pub fn data_path(&self) -> PathBuf {
        self.index_path.with_extension("pack")
    }

    /// Whether it takes part in the geometric progression, which kept, promisor and cruft packs don't.
    pub fn is_geometric(&self) -> bool {
        !(self.keep || self.promisor || self.cruft)
    }
}

/// The packs of an object database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Survey {
    /// The packs, ordered by their amount of objects and then by their path.
    pub packs: Vec<Pack>,
}

impl Survey {
    /// Find the packs of the object database in `objects_dir`, whose object ids are `object_hash` hashes, as git does: each
    /// file in its pack directory whose name ends in `.idx` is the index of a pack if the data of the pack exists beside
    /// it. An object database without a pack directory has no packs.
    pub fn at(objects_dir: &Path, object_hash: gix_hash::Kind) -> Result<Self> {
        let pack_dir = objects_dir.join("pack");
        let entries = match std::fs::read_dir(&pack_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Survey::default()),
            Err(err) => return Err(err).or_raise(|| message!("Could not list the packs in {}", pack_dir.display())),
        };
        let mut packs = Vec::new();
        for entry in entries {
            let index_path = entry
                .or_raise(|| message!("Could not list the packs in {}", pack_dir.display()))?
                .path();
            if index_path.extension().is_none_or(|extension| extension != "idx") {
                continue;
            }
            let data_path = index_path.with_extension("pack");
            let modified = match std::fs::metadata(&data_path) {
                Ok(metadata) if metadata.is_file() => metadata
                    .modified()
                    .or_raise(|| message!("Could not read when {} was modified", data_path.display()))?,
                Ok(_) => continue,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err).or_raise(|| message!("Could not access {}", data_path.display())),
            };
            let num_objects = match gix_pack::index::File::at(&index_path, object_hash) {
                Ok(index) => index.num_objects(),
                // The pack was removed since its directory was listed.
                Err(_) if !index_path.is_file() => continue,
                Err(err) => {
                    return Err(err).or_raise(|| message!("Could not read the pack index {}", index_path.display()));
                }
            };
            let has = |extension: &str| index_path.with_extension(extension).exists();
            let (keep, promisor, cruft) = (has("keep"), has("promisor"), has("mtimes"));
            packs.push(Pack {
                index_path,
                num_objects,
                modified,
                keep,
                promisor,
                cruft,
            });
        }
        packs.sort_by(|a, b| {
            a.num_objects
                .cmp(&b.num_objects)
                .then_with(|| a.index_path.cmp(&b.index_path))
        });
        Ok(Survey { packs })
    }

    /// The packs that take part in the geometric progression, from the one with the fewest objects.
    pub fn geometric(&self) -> impl Iterator<Item = &Pack> {
        self.packs.iter().filter(|pack| pack.is_geometric())
    }

    /// The amount of the [geometric](Self::geometric()) packs with the fewest objects that [`repack()`](super::repack())
    /// combines with `split_factor`, as `git repack --geometric=<split_factor>` determines it.
    ///
    /// The packs up to the last one that holds fewer than `split_factor` times the objects of the pack before it are
    /// combined, and then each following pack while it holds fewer than `split_factor` times the objects of all packs
    /// combined before it. This is 0 if the packs form a geometric progression already, and never 1.
    pub fn split(&self, split_factor: u32) -> usize {
        let weights: Vec<u32> = self.geometric().map(|pack| pack.num_objects).collect();
        split(&weights, split_factor)
    }
}

/// The amount of the packs with the ascending `weights` that are combined so that the remaining ones form a geometric
/// progression with `factor`, as git's `compute_pack_geometry_split()` determines it.
fn split(weights: &[u32], factor: u32) -> usize {
    // The products can exceed 64 bits, where git refuses to go on.
    let times_factor = |weight: u64| u128::from(factor) * u128::from(weight);
    let mut split = (1..weights.len())
        .rev()
        .find(|&index| u128::from(weights[index]) < times_factor(u64::from(weights[index - 1])))
        .map_or(0, |index| index + 1);
    let mut total: u64 = weights[..split].iter().map(|&weight| u64::from(weight)).sum();
    for &weight in &weights[split..] {
        if u128::from(weight) >= times_factor(total) {
            break;
        }
        split += 1;
        total += u64::from(weight);
    }
    split
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn packs_combine_until_the_rest_form_a_geometric_progression() {
        assert_eq!(split(&[], 2), 0, "there is nothing to combine");
        assert_eq!(split(&[5], 2), 0, "a single pack is never combined");
        assert_eq!(split(&[1, 1], 2), 2, "two equal packs are combined");
        assert_eq!(split(&[1, 2], 2), 0, "the larger pack holds twice the objects already");
        assert_eq!(
            split(&[1, 3, 4, 100], 2),
            3,
            "4 < 2 * 3, so the packs up to it are combined"
        );
        assert_eq!(split(&[1, 1, 5, 7], 2), 4, "7 < 2 * 5, so all packs are combined");
        assert_eq!(
            split(&[2, 2, 5, 20], 2),
            3,
            "the first two packs are combined as 2 < 2 * 2, then 5 < 2 * 4 is combined with them, while 20 >= 2 * 9 is not"
        );
        assert_eq!(split(&[1, 3, 9], 3), 0, "each pack holds 3 times the one before it");
        assert_eq!(split(&[1, 2, 9], 3), 2, "2 < 3 * 1, and 9 >= 3 * 3");
    }

    #[test]
    fn small_factors_and_empty_packs_combine_nothing() {
        assert_eq!(
            split(&[3, 3, 3], 1),
            0,
            "no pack holds fewer objects than the one before it"
        );
        assert_eq!(split(&[3, 3, 3], 0), 0);
        assert_eq!(
            split(&[0, 0, 0], 2),
            0,
            "as in git, packs without objects aren't combined"
        );
    }

    #[test]
    fn large_counts_are_compared_without_overflow() {
        assert_eq!(split(&[u32::MAX, u32::MAX], 2), 2, "u32::MAX < 2 * u32::MAX");
        assert_eq!(split(&[u32::MAX; 3], u32::MAX), 3, "u32::MAX < u32::MAX * u32::MAX");
        assert_eq!(
            split(&[1, 1, u32::MAX], u32::MAX),
            3,
            "1 < u32::MAX * 1 combines the first two packs, and u32::MAX < u32::MAX * 2 is combined with them"
        );
    }
}
