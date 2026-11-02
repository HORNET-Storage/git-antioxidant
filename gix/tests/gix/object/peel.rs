//! Objects that only a corrupt repository has, which Git refuses to peel: a tag that leads back to itself, which
//! `make_self_referencing_tag.sh` makes, and at which all that follows tags stops with an error, as it would follow the
//! tag forever otherwise, and commits whose tree is another kind of object, which
//! `make_commits_with_trees_of_other_kinds.sh` makes.

use gix::ObjectId;

fn repo_with_tag() -> crate::Result<(gix::Repository, ObjectId)> {
    let repo = crate::named_repo("make_self_referencing_tag.sh")?;
    let tag = repo.find_reference("refs/tags/loop")?.id().detach();
    Ok((repo, tag))
}

#[track_caller]
fn assert_stops_at_the_tag<T>(result: gix::Result<T>) {
    let Err(err) = result else {
        panic!("a tag that leads back to itself has no end");
    };
    assert!(format!("{err:?}").contains("leads back to itself"), "{err:?}");
}

#[test]
fn peeling_objects() -> crate::Result {
    let (repo, tag) = repo_with_tag()?;
    assert_stops_at_the_tag(repo.find_object(tag)?.peel_tags_to_end());
    assert_stops_at_the_tag(repo.find_object(tag)?.peel_to_kind(gix::object::Kind::Commit));
    Ok(())
}

#[test]
fn commits_whose_tree_is_another_kind_of_object_are_corrupt() -> crate::Result {
    let repo = crate::named_repo("make_commits_with_trees_of_other_kinds.sh")?;
    for kind in ["commit", "tag", "blob"] {
        let hex = std::fs::read_to_string(repo.workdir().expect("a worktree").join(kind))?;
        let commit = ObjectId::from_hex(hex.trim().as_bytes())?;
        let err = repo
            .find_object(commit)?
            .peel_to_tree()
            .expect_err("Git refuses to peel it as well");
        assert!(err.is_corrupted(), "the tree of the commit is a {kind}: {err:?}");
    }
    Ok(())
}

#[cfg(feature = "revision")]
#[test]
fn peeling_revisions() -> crate::Result {
    let (repo, _) = repo_with_tag()?;
    assert_stops_at_the_tag(repo.rev_parse_single("refs/tags/loop^{commit}"));
    Ok(())
}

#[test]
fn peeling_references() -> crate::Result {
    let (repo, _) = repo_with_tag()?;
    assert_stops_at_the_tag(repo.find_reference("refs/tags/loop")?.peel_to_id());
    Ok(())
}

#[test]
fn writing_packed_references() -> crate::Result {
    use gix::refs::{
        Target,
        file::transaction::PackedRefs,
        transaction::{Change, LogChange, PreviousValue, RefEdit},
    };

    let (repo, _tmp) = crate::repo_rw("make_self_referencing_tag.sh")?;
    let tag = repo.find_reference("refs/tags/loop")?.id().detach();
    let edit = RefEdit {
        change: Change::Update {
            log: LogChange::default(),
            expected: PreviousValue::Any,
            new: Target::Object(tag),
        },
        name: "refs/tags/packed".try_into()?,
        deref: false,
    };
    let fail = gix::lock::acquire::Fail::Immediately;
    assert_stops_at_the_tag(
        repo.refs
            .transaction()
            .packed_refs(PackedRefs::DeletionsAndNonSymbolicUpdates(Box::new(
                repo.objects.clone(),
            )))
            .prepare([edit], fail, fail),
    );
    Ok(())
}

#[cfg(feature = "rebase")]
#[test]
fn reading_todo_lists() -> crate::Result {
    use gix::{
        bstr::ByteSlice,
        sequencer::todo::{ById, Resolve},
    };

    let (repo, tag) = repo_with_tag()?;
    let mut resolve = ById::new(&repo.objects, repo.object_hash());
    assert_stops_at_the_tag(resolve.resolve(tag.to_string().as_bytes().as_bstr()));
    Ok(())
}

#[cfg(any(feature = "async-network-client", feature = "blocking-network-client"))]
#[test]
fn counting_the_objects_of_packs() -> crate::Result {
    use std::sync::atomic::AtomicBool;

    use gix::{
        odb::pack::data::output::count::{self, objects::ObjectExpansion},
        utils::progress::Discard,
    };

    let (repo, tag) = repo_with_tag()?;
    let odb = gix::odb::at(repo.git_dir().join("objects"), repo.object_hash())?;
    let interrupt = AtomicBool::new(false);
    for expansion in [
        ObjectExpansion::TreeContents,
        ObjectExpansion::TreeAdditionsComparedToAncestor,
    ] {
        assert_stops_at_the_tag(count::objects_unthreaded(
            &odb,
            &mut std::iter::once(Ok(tag)),
            &Discard,
            &interrupt,
            expansion,
            usize::MAX,
        ));
    }
    assert_stops_at_the_tag(count::revisions::objects(
        &repo.objects,
        [tag],
        [],
        &Discard,
        &interrupt,
        count::revisions::Options::default(),
    ));
    Ok(())
}
