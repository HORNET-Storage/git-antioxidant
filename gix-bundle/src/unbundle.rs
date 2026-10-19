use std::{
    cell::RefCell,
    io::{self, BufRead},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use gix_error::{Result, ResultExt, bail, message};
use gix_hash::{ObjectId, oid};
use gix_object::{Exists, Find};
use gix_pack::bundle::write::{Options, Outcome};
use gix_utils::progress::DynNestedProgress;

use crate::{Header, connectivity::missing_objects};

impl Header {
    /// Read the pack that follows this header from `pack`, write it with an index into the pack `directory` of a
    /// repository, and check that everything each reference needs is present, as fetching from a bundle file with
    /// `git fetch` does.
    ///
    /// * `pack` is the rest of the bundle, as [`Self::from_read()`] leaves it. Like `git index-pack` reading a bundle
    ///   file, more input after the pack is refused.
    /// * `directory` is the pack directory whose packs `objects` reads, usually `objects/pack`.
    /// * `objects` provides the prerequisites and the bases of a thin pack, which are added to the written pack so that
    ///   it doesn't depend on other packs. It must use `object_hash`, which the bundle must use as well.
    /// * `complete_tips` are commits whose history is known to be complete in `objects`, as for
    ///   [`Self::verify_prerequisites()`].
    /// * `options` configure how the pack and its index are written.
    ///
    /// First the prerequisites are verified. Then the pack is written into a new directory inside `directory`, where
    /// readers of the repository don't look for packs, and its objects are checked. Only then is it moved into
    /// `directory`, so that a refused bundle leaves no objects behind. A bundle with a filter is refused, as its pack
    /// lacks objects on purpose.
    ///
    /// The outcome describes the pack in `directory`. If its `keep_path` is set, a `.keep` file protects the new pack
    /// from removal until references point to its objects, after which the caller should delete it. If the same pack
    /// was present already, `keep_path` is `None`, and if the bundle has no objects to add, no paths are set.
    #[expect(clippy::too_many_arguments)]
    pub fn unbundle(
        &self,
        pack: &mut dyn BufRead,
        directory: &Path,
        objects: impl Find + Exists,
        object_hash: gix_hash::Kind,
        complete_tips: &[ObjectId],
        progress: &mut dyn DynNestedProgress,
        should_interrupt: &AtomicBool,
        options: Options,
    ) -> Result<Outcome> {
        if self.object_hash != object_hash {
            bail!(
                "The bundle uses {} object ids while the repository uses {object_hash}".unsupported(),
                self.object_hash
            );
        }
        if let Some(filter) = &self.filter {
            bail!(
                "The bundle was created with the filter {filter:?}, and filtered bundles aren't supported"
                    .unsupported()
            );
        }
        self.verify_prerequisites(&objects, complete_tips, should_interrupt)?;

        let staging = Staging::new(directory)?;
        let staged = gix_pack::Bundle::write_to_directory(
            pack,
            Some(staging.path.as_path()),
            progress,
            should_interrupt,
            Some(&objects),
            object_hash,
            options,
        )
        .or_raise(|| message("Could not write the pack of the bundle"))?;
        let rest = pack
            .fill_buf()
            .or_raise(|| message("Could not read past the pack of the bundle"))?;
        if !rest.is_empty() {
            bail!(message("The bundle continues after its pack").corrupted());
        }
        self.verify_references(&staged, &objects, complete_tips, should_interrupt)?;
        move_into(directory, staged)
    }

    /// Check that everything reachable from each reference is present, looking up objects in the pack that `staged`
    /// describes before `objects`.
    fn verify_references(
        &self,
        staged: &Outcome,
        objects: &(impl Find + Exists),
        complete_tips: &[ObjectId],
        should_interrupt: &AtomicBool,
    ) -> Result {
        let new_pack = staged
            .to_bundle()
            .transpose()
            .or_raise(|| message("Could not open the pack written from the bundle"))?;
        let objects = WithNewPack {
            pack: new_pack.as_ref(),
            inflate: RefCell::default(),
            objects,
        };
        let complete = complete_tips
            .iter()
            .copied()
            .chain(self.prerequisites.iter().map(|prerequisite| prerequisite.id));
        let missing = missing_objects(
            &objects,
            self.references.iter().map(|reference| reference.id),
            complete,
            should_interrupt,
        )?;
        if !missing.is_empty() {
            bail!(
                "The bundle lacks {} object(s) that its references need: {}".corrupted(),
                missing.count(),
                missing.describe()
            );
        }
        Ok(())
    }
}

/// The objects of a pack written from a bundle, looked up before the objects of the repository it was written for.
struct WithNewPack<'a, T> {
    pack: Option<&'a gix_pack::Bundle>,
    inflate: RefCell<gix_zlib::Inflate>,
    objects: T,
}

impl<T: Find> Find for WithNewPack<'_, T> {
    fn try_find<'b>(&self, id: &oid, buffer: &'b mut Vec<u8>) -> Result<Option<gix_object::Data<'b>>> {
        if let Some(pack) = self.pack
            && pack.index.lookup(id).is_some()
        {
            return Ok(pack
                .find(id, buffer, &mut self.inflate.borrow_mut(), &mut gix_pack::cache::Never)?
                .map(|(object, _location)| object));
        }
        self.objects.try_find(id, buffer)
    }
}

impl<T: Exists> Exists for WithNewPack<'_, T> {
    fn exists(&self, id: &oid) -> bool {
        self.pack.is_some_and(|pack| pack.index.lookup(id).is_some()) || self.objects.exists(id)
    }
}

/// A directory inside a pack directory that holds a pack until its objects were checked.
///
/// Readers of a repository look for packs only in the pack directory itself. The directory and whatever remains in it
/// are removed when it is dropped. Like the temporary files of `git index-pack`, its name starts with `tmp_`, so that
/// `git prune` removes it if a crash left it behind.
struct Staging {
    path: PathBuf,
}

impl Staging {
    fn new(directory: &Path) -> Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        loop {
            let path = directory.join(format!(
                "tmp_bundle_{}_{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Staging { path }),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => {
                    return Err(err).or_raise(|| {
                        message!(
                            "Could not create a directory for the pack of the bundle in {}",
                            directory.display()
                        )
                    });
                }
            }
        }
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        // Remaining files were written from the bundle and aren't visible to readers of the repository, even if they
        // can't be removed.
        std::fs::remove_dir_all(&self.path).ok();
    }
}

/// Move the pack that `staged` describes into `directory`, as if it had been written there, unless the same pack is
/// present already.
fn move_into(directory: &Path, staged: Outcome) -> Result<Outcome> {
    let (Some(staged_data), Some(staged_index)) = (&staged.data_path, &staged.index_path) else {
        return Ok(staged);
    };
    let data_path = directory.join(staged_data.file_name().expect("a written pack has a file name"));
    let index_path = data_path.with_extension("idx");
    // Like `gix_pack::Bundle::write_to_directory()`, create the keep file first and move the index last, so that
    // readers see the pack only once it is complete and it is protected from removal before that.
    let keep_path = if data_path.is_file() {
        None
    } else {
        let keep_path = data_path.with_extension("keep");
        std::fs::write(&keep_path, b"").or_raise(|| message("Could not create the keep file of the pack"))?;
        std::fs::rename(staged_data, &data_path)
            .or_raise(|| message("Could not move the pack into the pack directory"))?;
        Some(keep_path)
    };
    if !index_path.is_file() {
        std::fs::rename(staged_index, &index_path)
            .or_raise(|| message("Could not move the pack index into the pack directory"))?;
    }
    Ok(Outcome {
        index_path: Some(index_path),
        data_path: Some(data_path),
        keep_path,
        ..staged
    })
}
