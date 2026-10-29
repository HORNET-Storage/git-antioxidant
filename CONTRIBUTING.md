# Contributing

git-antioxidant is developed by HORNET Storage in this repository. It doesn't follow gitoxide and doesn't send changes
to it, so all work on git-antioxidant happens here.

## Changes

- Changes come as pull requests to this repository. Describe what motivated a change, not only what it does.
- Each change implements Git's behavior, with tests that compare against Git where Git can run them, follows the rules in
  [`AGENTS.md`](AGENTS.md) and keeps every check of CI passing. `just test` runs them locally, and
  [`DEVELOPMENT.md`](DEVELOPMENT.md) describes the tools.
- Commit messages follow the conventions of [`AGENTS.md`](AGENTS.md): conventional commits scoped to a crate, with `!`
  for breaking changes.

## Vulnerabilities

Report vulnerabilities privately, as [`SECURITY.md`](SECURITY.md) describes, never in public.
