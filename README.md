[![CI](https://github.com/HORNET-Storage/git-antioxidant/actions/workflows/ci.yml/badge.svg)](https://github.com/HORNET-Storage/git-antioxidant/actions/workflows/ci.yml)
<img src="etc/msrv-badge.svg">

# git-antioxidant

git-antioxidant is an implementation of Git in Rust by [HORNET Storage](https://github.com/HORNET-Storage). It began as a
fork of [gitoxide](https://github.com/GitoxideLabs/gitoxide) and is now a project of its own: it no longer takes changes
from gitoxide, and its own changes stay here.

It aims for the highest quality in speed and security:

- **Speed**: Git's algorithms, using every core where the work allows it, measured against Git on representative
  repositories.
- **Security**: whatever repositories, remotes and configuration files contain is untrusted until it has been checked.
  The paths of trees and of the index are validated before files are written or removed, with Git's protections for
  NTFS and HFS. Hooks and the `exec` commands of rebases only run in repositories whose Git directory is fully trusted,
  as Git refuses to work in repositories of other owners unless `safe.directory` allows it, and editors only come from
  trusted configuration and the environment.
- **Compatibility**: repositories, configuration, hooks, credential helpers and remotes work as they do with Git. Git is
  the reference that the tests compare against, and every remaining difference is documented.

## Names

The library keeps the name `gix` and its plumbing crates the names `gix-*`; the command-line programs `gix` and `ein`,
the packages that build them and the configuration keys of the `gitoxide` section, which gix adds to Git's, keep theirs
too. Code, scripts and configuration written for them keep working.

## What it adds

git-antioxidant implements Git's workflows on top of gitoxide's plumbing:

- **Commits**: `Repository::commit_index()` commits the index as `git commit` does, with its hooks, the cleanup of the
  message, `--amend`, merge commits and signatures.
- **Merges**: `Repository::merge_into_head()` merges as `git merge` does with its default strategy: fast-forwards,
  `--no-ff`, `--ff-only`, `--squash` and `--no-commit`, conflicts kept in `MERGE_HEAD`, `MERGE_MSG` and `MERGE_MODE`, and
  `--continue`, `--abort` and `--quit`.
- **Rebases**: `Repository::rebase()` rebases as `git rebase` does with its merge backend: the commands of the todo list,
  stops at conflicts, `edit`, `break` and failed `exec` lines, the sequence editor, the hooks, and `--continue`, `--skip`,
  `--abort` and `--quit`. `gix-rebase` plans the todo list, and `gix-sequencer` reads and writes it with the state of the
  rebase.
- **Pushes**: refspecs, `push.default`, `--delete`, `--tags`, `--follow-tags`, `--force`, `--force-with-lease`,
  `--atomic`, `--dry-run`, push options, the `pre-push` hook and the update of remote-tracking references, with thin
  packs whose deltas are searched as `git pack-objects` searches them.
- **Reset, switch and add**: `Repository::reset()` with `--soft`, `--mixed`, `--hard`, `--merge` and `--keep`,
  and the reset of paths; `gix_worktree_state::switch()` with Git's two-way merge rules; `gix::add::plumbing::add_all()`
  as `git add --all` does.
- **Hooks**: `gix-hook` finds and runs hooks as Git does, from `core.hooksPath` and the `hook.*` configuration, with Git's
  working directory, environment, arguments and input.
- **Packs**: delta compression in `gix-pack`, repacking with deltas in `gix-odb` as `git repack` does, and pack imports
  that stream blobs above `core.bigFileThreshold` as `git index-pack` does.
- **Diffs**: rename and copy detection that chooses its sources as Git's `diffcore-rename.c` does.
- **Integrity**: `gix-fsck` checks commits, tags and trees, and the `.gitmodules` and `.gitattributes` files that trees
  name, with Git's messages, severities and `fsck.<id>` configuration.
- **Trust**: whether a repository of another owner is fully trusted is decided as Git decides it, by `safe.directory`.
- **Credentials**: Git's `store` helper runs in process, and other helpers are found in Git's directories and on `PATH`.
- **Bundles**: `gix-bundle` reads and writes the headers of bundles, verifies them and writes their packs. Creating
  bundles and fetching from them are in progress.

[`crate-status.md`](crate-status.md) lists the state of every crate. [`SHORTCOMINGS.md`](SHORTCOMINGS.md) and the
documentation of each function list where they still differ from Git.

## Using the library

The crates aren't published on crates.io, where the crates of the same names belong to gitoxide. Use them from this
repository at an exact commit:

```toml
[dependencies]
gix = { git = "https://github.com/HORNET-Storage/git-antioxidant", rev = "<commit>" }
```

or by path from a checkout. `cargo doc -p gix --open` builds the documentation. The `Cargo.toml` of each crate describes
its features, which `cargo +nightly doc -p gix --features document-features --open` also lists in the documentation.
The minimum supported Rust version is the `rust-version` of [`gix/Cargo.toml`](gix/Cargo.toml).

## The programs

`gix` offers low-level commands for development and for trying functionality in real repositories, `ein` higher-level
ones. Neither is meant for use in scripts. To install both from this repository:

```sh
# Pure Rust, needing only a Rust toolchain and a C compiler.
cargo install --git https://github.com/HORNET-Storage/git-antioxidant --locked --no-default-features --features max-pure gitoxide

# The default build, `max`, which also needs `cmake`.
cargo install --git https://github.com/HORNET-Storage/git-antioxidant --locked gitoxide
```

## Development

[`DEVELOPMENT.md`](DEVELOPMENT.md) describes the tools and practices, [`AGENTS.md`](AGENTS.md) the rules that every
change follows and [`STABILITY.md`](STABILITY.md) how the interfaces change. `just test` runs the lints, the builds of
the feature sets, the tests and the documentation builds.

## Security

Report vulnerabilities privately, as [`SECURITY.md`](SECURITY.md) describes. The
[threat model](etc/security/threat-model.md) describes what git-antioxidant defends against.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## History and credits

git-antioxidant is built on [gitoxide](https://github.com/GitoxideLabs/gitoxide) by Sebastian Thiel and its
contributors, whose work makes up most of this repository. The history before the split is gitoxide's, including the
changelogs of the crates and the monthly reports in [`etc/reports`](etc/reports). Git itself is the reference for every
behavior implemented here.

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option, as gitoxide is.
