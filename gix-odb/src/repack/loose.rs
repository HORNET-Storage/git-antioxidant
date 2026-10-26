use std::{
    io,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use gix_error::{Result, ResultExt, bail, cancelled, message};
use gix_hash::ObjectId;

use crate::repack::Survey;

/// How many loose objects are looked at between two checks of the interrupt flag.
const OBJECTS_PER_INTERRUPT_CHECK: usize = 4096;

/// Whether the object database in `objects_dir`, whose object ids are `object_hash` hashes, has more than `limit` loose
/// objects by the estimate of git's `too_many_loose_objects()`.
///
/// As object ids are distributed evenly, git counts only the loose objects in the fanout directory `17` and expects the
/// other 255 to hold as many: there are too many if `17` holds more than `limit` divided by 256 and rounded up. If the
/// directory can't be read, there are none.
pub fn too_many_loose_objects(objects_dir: &Path, object_hash: gix_hash::Kind, limit: u64) -> bool {
    let Ok(entries) = std::fs::read_dir(objects_dir.join("17")) else {
        return false;
    };
    let name_len = object_hash.len_in_hex() - 2;
    let threshold = limit.div_ceil(256);
    let mut count = 0;
    for entry in entries {
        // Like git, stop counting where the directory can't be read further.
        let Ok(entry) = entry else {
            break;
        };
        let name = entry.file_name();
        let name = name.as_encoded_bytes();
        if name.len() == name_len && name.iter().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
            count += 1;
            if count > threshold {
                return true;
            }
        }
    }
    false
}

/// The loose objects of the object database in `objects_dir`, whose object ids are `object_hash` hashes, in the order
/// in which git lists them: the fanout directories from `00` to `ff`, and the objects in each in the order in which
/// the file system lists them. Fanout directories that don't exist hold no objects, and files whose names aren't object
/// ids are left out.
pub(crate) fn list(objects_dir: &Path, object_hash: gix_hash::Kind) -> Result<Vec<ObjectId>> {
    let name_len = object_hash.len_in_hex() - 2;
    let mut ids = Vec::new();
    let mut hex = Vec::with_capacity(object_hash.len_in_hex());
    for fanout in 0..=u8::MAX {
        let fanout = format!("{fanout:02x}");
        let dir = objects_dir.join(&fanout);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err).or_raise(|| message!("Could not list the loose objects in {}", dir.display())),
        };
        for entry in entries {
            let name = entry
                .or_raise(|| message!("Could not list the loose objects in {}", dir.display()))?
                .file_name();
            let name = name.as_encoded_bytes();
            if name.len() != name_len {
                continue;
            }
            hex.clear();
            hex.extend_from_slice(fanout.as_bytes());
            hex.extend_from_slice(name);
            if let Ok(id) = ObjectId::from_hex(&hex) {
                ids.push(id);
            }
        }
    }
    Ok(ids)
}

/// Remove the loose objects of the object database in `objects_dir` that a pack holds, as `git prune-packed` does, and
/// return how many were removed.
///
/// The packs and loose objects are listed anew, so that only objects that a pack holds now are removed. Loose objects
/// that can't be listed or removed remain, and so do the fanout directories.
pub(crate) fn prune(objects_dir: &Path, object_hash: gix_hash::Kind, should_interrupt: &AtomicBool) -> Result<usize> {
    let mut indices = Vec::new();
    for pack in Survey::at(objects_dir, object_hash)?.packs {
        match gix_pack::index::File::at(&pack.index_path, object_hash) {
            Ok(index) => indices.push(index),
            // The pack was removed since it was found.
            Err(_) if !pack.index_path.is_file() => {}
            Err(err) => {
                return Err(err).or_raise(|| message!("Could not read the pack index {}", pack.index_path.display()));
            }
        }
    }

    let loose = crate::loose::Store::at(objects_dir, object_hash);
    let mut removed = 0;
    for (count, id) in loose.iter().enumerate() {
        if count.is_multiple_of(OBJECTS_PER_INTERRUPT_CHECK) && should_interrupt.load(Ordering::Relaxed) {
            bail!(cancelled("Interrupted while removing loose objects that a pack holds"));
        }
        // The packs were combined already, so an object that can't be listed is left for the next repack.
        let id = match id {
            Ok(id) => id,
            Err(_err) => {
                gix_trace::warn!("Could not list loose objects in {}: {}", objects_dir.display(), _err);
                continue;
            }
        };
        if !indices.iter().any(|index| index.lookup(id).is_some()) {
            continue;
        }
        let path = loose.object_path(&id);
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(_err) => gix_trace::warn!("Could not remove the loose object {}: {}", path.display(), _err),
        }
    }
    Ok(removed)
}
