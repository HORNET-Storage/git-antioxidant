# Security policy

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's
[private vulnerability reporting](https://github.com/HORNET-Storage/git-antioxidant/security/advisories/new), never in
a public issue, pull request or discussion. A report helps most with the affected crates and functions, the platform,
the steps or the input that reproduce it, and what Git does in the same situation.

The maintainers confirm that they received the report, keep the reporter informed while they work on a fix, and credit
the reporter if an advisory is published, unless the reporter prefers otherwise.
[`etc/security/irp.md`](etc/security/irp.md) describes how a report is handled.

## Supported versions

Fixes go to `main` only. git-antioxidant has no releases: applications use it at an exact commit and move to the commit
that fixes a vulnerability.

## Scope

git-antioxidant treats repositories, remotes and the files of other owners as untrusted, as Git does; the
[threat model](etc/security/threat-model.md) describes what it defends against. Vulnerabilities of Git itself belong on
[Git's security list](https://git-scm.com/community#git-security).
