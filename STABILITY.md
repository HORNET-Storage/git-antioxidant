# Stability

git-antioxidant is pre-release software. Its interfaces can change in any commit when a better design or compatibility
with Git calls for it.

## Versions

The crates aren't published, and the repository has no releases. The version numbers in the manifests are those the
crates had in gitoxide when the projects split; they don't identify a state of the code, the commit does. Applications
depend on an exact commit of `main`, through Git or by path, and update by moving to a newer commit.

## Breaking changes

A change that requires dependent code to change is a breaking change. Its commit title marks it with `!` after the
scope, as in `change(gix)!: …`, and its message says what callers have to change. The commit history records the
changes since the split; the `CHANGELOG.md` files of the crates record gitoxide's releases up to it.

What Git defines, such as the formats on disk, the protocols and the meaning of configuration, only changes to follow
Git.

## The minimum supported Rust version (MSRV)

`gix` and all crates it depends on build with the `rust-version` of [`gix/Cargo.toml`](gix/Cargo.toml), which the
`msrv` job of the `ci.yml` workflow checks. All other crates may need the latest stable Rust. Raising the MSRV isn't a
breaking change.
