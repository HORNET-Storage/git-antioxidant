use crate::{Repository, Result, error::bail};

impl Repository {
    /// Return the hooks of this repository, to find and run them as Git does.
    ///
    /// They are the programs of the directory that `core.hooksPath` names, or else of the `hooks` directory of the
    /// [common Git directory](Self::common_dir()), and the commands that the `hook.*` values of the configuration
    /// name, in the order Git reads them. Hooks run in the root of the worktree, or in the Git directory of a bare
    /// repository.
    ///
    /// ### Security
    ///
    /// Hooks start programs that the repository names, so this fails unless the
    /// [Git directory is fully trusted](Self::git_dir_trust()), as Git refuses to work in a repository of another owner
    /// unless `safe.directory` allows it.
    pub fn hooks(&self) -> Result<gix_hook::Hooks> {
        if self.git_dir_trust() != gix_sec::Trust::Full {
            bail!("Hooks only run in repositories whose Git directory is fully trusted".permission_denied());
        }
        let current_dir = self.current_dir();
        let git_dir = current_dir.join(self.git_dir());
        let common_dir = current_dir.join(self.common_dir());
        let worktree = self.workdir().map(|worktree| current_dir.join(worktree));
        self.config.hooks(gix_hook::Context {
            git_dir: &git_dir,
            common_dir: &common_dir,
            worktree: worktree.as_deref(),
        })
    }
}
