use std::sync::atomic::AtomicBool;

use gix_error::{Result, bail};
use gix_hash::ObjectId;
use gix_object::{Exists, Find};

use crate::{
    Header,
    connectivity::{Missing, missing_objects},
};

impl Header {
    /// Check that `objects` contains every prerequisite of the bundle along with all objects reachable from it,
    /// as `git bundle verify` does.
    ///
    /// `complete_tips` are commits whose history is known to be complete in `objects`, usually the commits that the
    /// references of the repository point to, with annotated tags peeled. Objects reachable from them aren't checked.
    /// `objects` must use [`Self::object_hash`], as the ids are looked up as the header stores them.
    ///
    /// Like `git bundle verify`, this doesn't read the pack, which [`Self::unbundle()`] checks.
    pub fn verify_prerequisites(
        &self,
        objects: impl Find + Exists,
        complete_tips: &[ObjectId],
        should_interrupt: &AtomicBool,
    ) -> Result {
        let mut absent = Missing::default();
        for prerequisite in &self.prerequisites {
            if !objects.exists(&prerequisite.id) {
                absent.add(&prerequisite.id);
            }
        }
        if !absent.is_empty() {
            bail!(
                "The repository lacks {} prerequisite(s) of the bundle: {}".not_found(),
                absent.count(),
                absent.describe()
            );
        }

        let missing = missing_objects(
            &objects,
            self.prerequisites.iter().map(|prerequisite| prerequisite.id),
            complete_tips.iter().copied(),
            should_interrupt,
        )?;
        if !missing.is_empty() {
            bail!(
                "The prerequisites of the bundle are present, but they aren't connected to the history of the repository, which lacks {} object(s) reachable from them: {}"
                    .not_found(),
                missing.count(),
                missing.describe()
            );
        }
        Ok(())
    }
}
