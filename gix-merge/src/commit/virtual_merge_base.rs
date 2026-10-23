/// The outcome produced by [`commit::merge_base()`](crate::commit::virtual_merge_base()).
pub struct Outcome {
    /// The commit ids of all the virtual merge bases we have produced in the process of recursively merging the merge-bases.
    /// As they have been written to the object database, they are still available until they are garbage collected.
    /// The last one is the most recently produced and the one returned as `commit_id`.
    /// This is never empty.
    pub virtual_merge_bases: nonempty::NonEmpty<gix_hash::ObjectId>,
    /// The id of the commit that was created to hold the merged tree.
    pub commit_id: gix_hash::ObjectId,
    /// The hash of the merged tree.
    pub tree_id: gix_hash::ObjectId,
}

pub(super) mod function {
    use gix_error::Result;
    use gix_error::{ResultExt, message};
    use gix_object::FindExt;

    use crate::blob::builtin_driver;

    /// Create a single virtual merge-base by merging `first_commit`, `second_commit` and `others` into one.
    /// Note that `first_commit` and `second_commit` are expected to have been popped off `others`, so `first_commit`
    /// was the last provided merge-base of function that provides multiple merge-bases for a pair of commits.
    ///
    /// The parameters `graph`, `diff_resource_cache`, `blob_merge`, `objects`, `abbreviate_hash` and `options` are passed
    /// directly to [`tree()`](crate::tree()) for merging the trees of two merge-bases at a time.
    /// Note that most of `options` are overwritten to match the requirements of a merge-base merge.
    ///
    /// As in Git, the merged merge-bases keep their conflicts. Conflicting text keeps conflict markers, two
    /// characters longer for each level of recursion, in the conflict style of `options` if it keeps conflicts
    /// and in the default style otherwise. Conflicting binary files and symbolic links keep the version of their
    /// merge-base: a binary file that both sides added becomes empty, and a symbolic link that both sides added is
    /// absent. Tree conflicts are resolved as [`ResolveWith::VirtualMergeBase`](crate::tree::ResolveWith::VirtualMergeBase)
    /// describes, and merge-bases without a common ancestor are merged with the empty tree.
    #[expect(clippy::too_many_arguments)]
    pub fn virtual_merge_base<'objects>(
        first_commit: gix_hash::ObjectId,
        second_commit: gix_hash::ObjectId,
        mut others: Vec<gix_hash::ObjectId>,
        graph: &mut gix_revwalk::Graph<'_, '_, gix_revwalk::graph::Commit<gix_revision::merge_base::Flags>>,
        diff_resource_cache: &mut gix_diff::blob::Platform,
        blob_merge: &mut crate::blob::Platform,
        objects: &'objects (impl gix_object::FindObjectOrHeader + gix_object::Write),
        abbreviate_hash: &mut dyn FnMut(&gix_hash::oid) -> String,
        mut options: crate::tree::Options,
    ) -> Result<super::Outcome> {
        let mut merged_commit_id = first_commit;
        others.push(second_commit);

        // Each merge of two merge-bases is one level of recursion deeper than the merge that needs them.
        options.marker_size_multiplier = options.marker_size_multiplier.saturating_add(1);
        // The merged merge-base must be complete, whatever the caller considers unresolved.
        options.fail_on_conflict = None;
        options.tree_conflicts = Some(crate::tree::ResolveWith::VirtualMergeBase);
        // Directory renames are only detected in the outermost merge, as merge-ort does.
        options.directory_renames = crate::tree::DirectoryRenames::Ignore;
        options.blob_merge.is_virtual_ancestor = true;
        let conflict = &mut options.blob_merge.text.conflict;
        if !matches!(conflict, builtin_driver::text::Conflict::Keep { .. }) {
            *conflict = builtin_driver::text::Conflict::default();
        }
        let favor_ancestor = Some(builtin_driver::binary::ResolveWith::Ancestor);
        options.blob_merge.resolve_binary_with = favor_ancestor;
        options.symlink_conflicts = favor_ancestor;
        let labels = builtin_driver::text::Labels {
            current: Some("Temporary merge branch 1".into()),
            other: Some("Temporary merge branch 2".into()),
            ancestor: None,
        };
        let mut virtual_merge_bases = Vec::new();
        let mut tree_id = None;
        while let Some(next_commit_id) = others.pop() {
            // Conflicts remain part of the merged tree, as Git ignores whether merges of merge-bases are clean.
            let mut out = crate::commit(
                merged_commit_id,
                next_commit_id,
                labels,
                graph,
                diff_resource_cache,
                blob_merge,
                objects,
                abbreviate_hash,
                crate::commit::Options {
                    allow_missing_merge_base: true,
                    tree_merge: options.clone(),
                    use_first_merge_base: false,
                },
            )?;

            let merged_tree_id = out.tree_merge.tree.write(|tree| objects.write(tree))?;

            tree_id = Some(merged_tree_id);
            merged_commit_id = create_virtual_commit(objects, merged_commit_id, next_commit_id, merged_tree_id)?;

            virtual_merge_bases.extend(out.virtual_merge_bases);
            virtual_merge_bases.push(merged_commit_id);
        }

        Ok(super::Outcome {
            virtual_merge_bases: nonempty::NonEmpty::from_vec(virtual_merge_bases)
                .expect("the virtual merge-base process always creates at least one commit"),
            commit_id: merged_commit_id,
            tree_id: tree_id.map_or_else(
                || {
                    let mut buf = Vec::new();
                    objects.find_commit(&merged_commit_id, &mut buf).map(|c| c.tree())
                },
                Ok,
            )?,
        })
    }

    fn create_virtual_commit(
        objects: &(impl gix_object::Find + gix_object::Write),
        parent_a: gix_hash::ObjectId,
        parent_b: gix_hash::ObjectId,
        tree_id: gix_hash::ObjectId,
    ) -> Result<gix_hash::ObjectId> {
        let mut buf = Vec::new();
        let commit_ref = objects
            .find_commit(&parent_a, &mut buf)
            .or_raise(|| message("Could not find commit to use as basis for a virtual commit"))?;
        let mut commit = commit_ref
            .to_owned()
            .or_raise(|| message("Failed to decode a commit needed to build a virtual merge-base"))?;
        commit.parents = vec![parent_a, parent_b].into();
        commit.tree = tree_id;
        objects
            .write(&commit)
            .or_raise(|| message("Failed to write tree for merged merge-base or virtual commit"))
    }
}
