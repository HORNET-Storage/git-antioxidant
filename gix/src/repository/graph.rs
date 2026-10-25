use crate::Result;
impl crate::Repository {
    /// Create a graph data-structure capable of accelerating graph traversals and storing state of type `T` with each commit
    /// it encountered.
    ///
    /// Note that the `cache` will be used if present, no object replacements are active and the repository isn't shallow,
    /// and it's best obtained with [`commit_graph_if_enabled()`](crate::Repository::commit_graph_if_enabled()).
    ///
    /// Note that a commitgraph is only allowed to be used if `core.commitGraph` is true (the default), and that configuration errors are
    /// ignored as well.
    ///
    /// ### Shallow repositories
    ///
    /// The commits that the `shallow` file of a shallow repository lists have no parents in the graph, as Git sees them,
    /// and the `cache` isn't used there, as Git reads no commit-graph in a shallow repository.
    /// An error is returned if the `shallow` file can't be read.
    ///
    /// ### Performance
    ///
    /// Note that the [Graph][gix_revwalk::Graph] can be sensitive to various object database settings that may affect the performance
    /// of the commit walk.
    pub fn revision_graph<'cache, T>(
        &self,
        cache: Option<&'cache gix_commitgraph::Graph>,
    ) -> Result<gix_revwalk::Graph<'_, 'cache, T>> {
        let shallow_commits = self.shallow_commits()?;
        let cache = cache.filter(|_| !self.has_object_replacements() && shallow_commits.is_none());
        let graph = gix_revwalk::Graph::new(&self.objects, cache);
        Ok(match shallow_commits {
            Some(commits) => graph.with_shallow_commits(commits.iter().copied()),
            None => graph,
        })
    }

    /// Return a cache for commits and their graph structure, as managed by `git commit-graph`, for accelerating commit walks on
    /// a low level.
    ///
    /// Note that [`revision_graph()`][crate::Repository::revision_graph()] should be preferred for general purpose walks that don't
    /// rely on the actual commit cache to be present, while leveraging the commit-graph if possible.
    /// Parent-buffer allocations respect `gitoxide.objects.allocLimit`.
    pub fn commit_graph(&self) -> Result<gix_commitgraph::Graph> {
        gix_commitgraph::Graph::at(
            &self.objects.store_ref().path().join("info"),
            self.object_hash(),
            self.config.alloc_limit_bytes,
        )
    }

    /// Return a newly opened commit-graph if it is available, enabled in Git configuration, no object replacements are active
    /// and the repository isn't shallow, as Git reads no commit-graph in a shallow repository.
    pub fn commit_graph_if_enabled(&self) -> Result<Option<gix_commitgraph::Graph>> {
        (!self.has_object_replacements() && !self.is_shallow()? && self.config.may_use_commit_graph()?)
            .then(|| self.commit_graph())
            .transpose()
            .or_else(|err| if err.is_not_found() { Ok(None) } else { Err(err) })
    }

    pub(crate) fn has_object_replacements(&self) -> bool {
        !self.objects.ignore_replacements && self.objects.store_ref().replacements().next().is_some()
    }
}
