//! Every object remains available while packs are combined and removed, also to readers that opened the object database
//! before, an interrupted repack leaves the object database as it was, and what a repack that ended early left behind is
//! removed by the next one.

use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gix_hash::ObjectId;
use gix_object::Find;
use gix_odb::repack::Options;
use gix_testtools::TestResult;
use gix_utils::progress::{self, Count, Id, MessageLevel, NestedProgress, Progress, Step, StepShared, UNKNOWN, Unit};

use super::{Case, file_name};

#[test]
fn a_reader_that_opened_the_object_database_before_finds_every_object() -> TestResult {
    let case = Case::writable("geometric")?;
    let object_hash = gix_testtools::object_hash();
    let ids = case.objects()?;
    let objects = gix_odb::at(case.objects_dir(), object_hash)?;
    let mut buf = Vec::new();
    for id in &ids {
        assert!(
            objects.try_find(id, &mut buf)?.is_some(),
            "{id} is found before the repack"
        );
    }

    let outcome = case.repack(Options::default())?;
    assert!(
        !outcome.removed_packs.is_empty() && outcome.removed_loose_objects > 0,
        "packs and loose objects that the reader found objects in were removed"
    );

    for id in &ids {
        let object = objects
            .try_find(id, &mut buf)?
            .ok_or_else(|| format!("{id} is found after the repack"))?;
        assert_eq!(
            gix_object::compute_hash(object_hash, object.kind, object.data)?,
            *id,
            "{id} has its data"
        );
    }
    Ok(())
}

#[test]
fn an_interrupt_before_the_repack_leaves_the_object_database_as_it_was() -> TestResult {
    let case = Case::writable("geometric")?;
    let before = case.entries()?;
    let err = gix_odb::repack::repack(
        &case.objects_dir(),
        gix_testtools::object_hash(),
        &mut progress::Discard,
        &AtomicBool::new(true),
        Options::default(),
    )
    .expect_err("the repack is interrupted");
    assert!(err.is_cancelled(), "the interrupt is reported as cancellation: {err:?}");
    assert_eq!(case.entries()?, before, "the object database is as it was");
    Ok(())
}

#[test]
fn an_interrupt_while_the_pack_is_written_leaves_the_object_database_as_it_was() -> TestResult {
    let case = Case::writable("chain")?;
    let before = case.entries()?;
    let should_interrupt = Arc::new(AtomicBool::new(false));
    let mut progress = InterruptWhenCounting::new("write pack", Arc::clone(&should_interrupt));
    let err = gix_odb::repack::repack(
        &case.objects_dir(),
        gix_testtools::object_hash(),
        &mut progress,
        &should_interrupt,
        Options::default(),
    )
    .expect_err("the repack is interrupted");
    assert!(
        should_interrupt.load(Ordering::Relaxed),
        "the repack was interrupted once part of the pack was written"
    );
    assert!(err.is_cancelled(), "the interrupt is reported as cancellation: {err:?}");
    assert_eq!(
        case.entries()?,
        before,
        "the object database is as it was, without the directory of the new pack"
    );
    Ok(())
}

#[test]
fn the_directory_of_a_new_pack_that_an_ended_repack_left_is_removed() -> TestResult {
    let case = Case::writable("nothing")?;
    let pack_dir = case.pack_dir();
    let left = pack_dir.join("tmp_repack_4294967295_0");
    std::fs::create_dir(&left)?;
    std::fs::write(left.join("pack-incomplete.pack"), b"the start of a pack")?;
    // `git index-pack` writes its temporary pack as a file of this name.
    let index_pack = pack_dir.join("tmp_pack_Xy3abc");
    std::fs::write(&index_pack, b"the start of another pack")?;

    let outcome = case.repack(Options::default())?;

    assert_eq!(outcome.pack, None, "the case needs no new pack");
    assert!(
        !left.exists(),
        "the directory is removed with its files, also by a repack that writes nothing"
    );
    assert!(index_pack.is_file(), "the temporary file of git index-pack remains");
    Ok(())
}

#[test]
fn loose_objects_that_cant_be_read_are_left_and_reported() -> TestResult {
    let case = Case::writable("loose-only")?;
    let object_hash = gix_testtools::object_hash();
    let loose = case.loose()?;
    let readable = ObjectId::from_hex(loose.first().ok_or("the case has loose objects")?.as_bytes())?;
    let misnamed = gix_object::compute_hash(object_hash, gix_object::Kind::Blob, b"data that isn't stored")?;
    let garbage = gix_object::compute_hash(object_hash, gix_object::Kind::Blob, b"other data that isn't stored")?;
    let unreadable = [
        // The data of another object.
        (misnamed, std::fs::read(case.loose_path(&readable))?),
        (garbage, b"not a compressed object".to_vec()),
    ];
    for (id, data) in &unreadable {
        let path = case.loose_path(id);
        std::fs::create_dir_all(path.parent().ok_or("loose objects are in fanout directories")?)?;
        std::fs::write(path, data)?;
    }

    let outcome = case.repack(Options::default())?;

    let mut skipped = outcome.skipped_loose_objects.clone();
    skipped.sort();
    let mut expected = vec![misnamed, garbage];
    expected.sort();
    assert_eq!(skipped, expected, "both objects are reported");
    for (id, data) in &unreadable {
        assert_eq!(&std::fs::read(case.loose_path(id))?, data, "{id} is left as it was");
    }
    let written = outcome.pack.ok_or("the loose objects that can be read were packed")?;
    assert_eq!(
        case.objects_of_packs(&BTreeSet::from([file_name(&written.index_path)?]))?,
        case.record("new-objects")?,
        "the other loose objects were packed as git packed them"
    );
    let mut remaining: Vec<String> = expected.iter().map(ToString::to_string).collect();
    remaining.sort();
    assert_eq!(
        case.loose()?,
        remaining,
        "only the objects that can't be read remain loose"
    );
    Ok(())
}

/// A progress that sets `should_interrupt` once the progress named `trigger` counts anything, as if the repack was
/// interrupted at that point.
struct InterruptWhenCounting {
    trigger: &'static str,
    name: Option<String>,
    should_interrupt: Arc<AtomicBool>,
    step: StepShared,
}

impl InterruptWhenCounting {
    fn new(trigger: &'static str, should_interrupt: Arc<AtomicBool>) -> Self {
        InterruptWhenCounting {
            trigger,
            name: None,
            should_interrupt,
            step: StepShared::default(),
        }
    }

    fn counted(&self, step: Step) {
        if step > 0 && self.name.as_deref() == Some(self.trigger) {
            self.should_interrupt.store(true, Ordering::Relaxed);
        }
    }
}

impl Count for InterruptWhenCounting {
    fn set(&self, step: Step) {
        self.step.store(step, Ordering::Relaxed);
        self.counted(step);
    }

    fn step(&self) -> Step {
        self.step.load(Ordering::Relaxed)
    }

    fn inc_by(&self, step: Step) {
        self.step.fetch_add(step, Ordering::Relaxed);
        self.counted(step);
    }

    fn counter(&self) -> StepShared {
        Arc::clone(&self.step)
    }
}

impl Progress for InterruptWhenCounting {
    fn init(&mut self, _max: Option<Step>, _unit: Option<Unit>) {}

    fn set_name(&mut self, name: String) {
        self.name = Some(name);
    }

    fn name(&self) -> Option<String> {
        self.name.clone()
    }

    fn id(&self) -> Id {
        UNKNOWN
    }

    fn message(&self, _level: MessageLevel, _message: String) {}
}

impl NestedProgress for InterruptWhenCounting {
    type SubProgress = Self;

    fn add_child(&mut self, name: impl Into<String>) -> Self {
        InterruptWhenCounting {
            trigger: self.trigger,
            name: Some(name.into()),
            should_interrupt: Arc::clone(&self.should_interrupt),
            step: StepShared::default(),
        }
    }

    fn add_child_with_id(&mut self, name: impl Into<String>, _id: Id) -> Self {
        NestedProgress::add_child(self, name)
    }
}
