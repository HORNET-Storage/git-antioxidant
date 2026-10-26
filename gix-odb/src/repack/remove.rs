use std::{
    io,
    path::{Path, PathBuf},
};

use gix_error::{Result, ResultExt, message};

use crate::repack::{Pack, Written};

/// The extensions of the files of a pack, its index first, in the order git's `unlink_pack_path()` removes them.
const PACK_FILES: [&str; 7] = ["idx", "pack", "rev", "keep", "bitmap", "promisor", "mtimes"];

/// Remove the `combined` packs from `pack_dir`, except one with the same name as the `written` pack, and return the
/// paths of the indices of the removed packs.
///
/// First the multi-pack-indices that refer to a removed pack are removed, as they would break readers that use them.
/// Then each pack is removed with its index first, after which readers no longer find it. A pack whose index can't be
/// removed keeps its data, so that it remains usable, and isn't reported.
pub(crate) fn remove_packs(pack_dir: &Path, combined: &[&Pack], written: Option<&Written>) -> Result<Vec<PathBuf>> {
    let packs: Vec<&Pack> = combined
        .iter()
        .copied()
        .filter(|pack| written.is_none_or(|written| written.index_path.file_name() != pack.index_path.file_name()))
        .collect();
    if packs.is_empty() {
        return Ok(Vec::new());
    }
    clear_multi_pack_indices(pack_dir, &packs)?;

    let mut removed = Vec::with_capacity(packs.len());
    for pack in packs {
        if let Err(_err) = remove_if_present(&pack.index_path) {
            gix_trace::warn!(
                "Could not remove the pack index {}: {}",
                pack.index_path.display(),
                _err
            );
            continue;
        }
        for extension in &PACK_FILES[1..] {
            let path = pack.index_path.with_extension(extension);
            if let Err(_err) = remove_if_present(&path) {
                gix_trace::warn!("Could not remove {}: {}", path.display(), _err);
            }
        }
        removed.push(pack.index_path.clone());
    }
    Ok(removed)
}

/// Remove the multi-pack-indices in `pack_dir` that refer to one of the `packs`, along with their bitmaps and reverse
/// indices, as git's `clear_midx_file()` and `clear_incremental_midx_files()` do.
///
/// That's the multi-pack-index if it lists one of the `packs` or can't be read, and the incremental multi-pack-index,
/// which isn't read, in any case.
fn clear_multi_pack_indices(pack_dir: &Path, packs: &[&Pack]) -> Result {
    let path = pack_dir.join("multi-pack-index");
    if path.is_file() {
        let refers_to_packs = match gix_pack::multi_index::File::at(&path, None) {
            Ok(index) => index
                .index_names()
                .iter()
                .any(|name| packs.iter().any(|pack| pack.index_path.file_stem() == name.file_stem())),
            Err(_) => true,
        };
        if refers_to_packs {
            remove_if_present(&path)
                .or_raise(|| message!("Could not remove the multi-pack-index {}", path.display()))?;
            remove_files(pack_dir, &["bitmap", "rev"]);
        }
    }

    let incremental = pack_dir.join("multi-pack-index.d");
    if incremental.is_dir() {
        let chain = incremental.join("multi-pack-index-chain");
        remove_if_present(&chain)
            .or_raise(|| message!("Could not remove the multi-pack-index chain {}", chain.display()))?;
        remove_files(&incremental, &["midx", "bitmap", "rev"]);
    }
    Ok(())
}

/// Remove the files in `dir` whose names start with `multi-pack-index-` and end in one of the `extensions`.
fn remove_files(dir: &Path, extensions: &[&str]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(|entry| entry.ok().map(|entry| entry.path())) {
        let matches = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("multi-pack-index-"))
            && path
                .extension()
                .is_some_and(|extension| extensions.iter().any(|wanted| extension == *wanted));
        if matches && let Err(_err) = remove_if_present(&path) {
            gix_trace::warn!("Could not remove {}: {}", path.display(), _err);
        }
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
