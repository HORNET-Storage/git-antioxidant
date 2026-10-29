# Incident Response Plan (IRP) for Vulnerabilities

This plan describes how git-antioxidant handles a vulnerability, from its report to its fix. It is a living document that changes with what each incident teaches.

## Phase One: Triage

A report usually arrives through GitHub's private vulnerability reporting, as [`SECURITY.md`](../../SECURITY.md) describes, which opens a draft advisory. The plan applies as well when a vulnerability becomes known in another way, including a public disclosure.

1. **Acknowledge the report** to the reporter.
2. **Understand the report**, and ask the reporter where it is unclear.
3. **Validate the vulnerability**: it must be plausibly exploitable, harm confidentiality, integrity or availability, and lie in git-antioxidant or in the way it uses other software. Reproduce it with the reporter's proof of concept, or write one.
4. **Assess the severity** with a [CVSS score](https://www.first.org/cvss/).

## Phase Two: Investigation

1. **Scope the impact**: the affected crates, functions, platforms and configurations, and the commit that introduced the flaw.
2. **Look for relatives**: the same flaw elsewhere in git-antioxidant, an incomplete earlier fix, and similar issues in [Git](https://git-scm.com/) and [Git for Windows](https://gitforwindows.org/), whose behavior is the reference. Vulnerabilities of Git itself go to [Git's security list](https://git-scm.com/community#git-security).
3. **Record the findings** in the draft advisory.

## Phase Three: Fix

1. **Fix the cause**, with a regression test that fails without the fix, and adapt every caller in the repository.
2. **Run every check of CI** before the fix lands on `main`.
3. **Tell the reporter and the applications that use git-antioxidant** which commit fixes the vulnerability, so that they can move to it.
4. **Decide whether to publish the advisory**, with its CVE identifier if one is requested, and credit the reporter in it unless they prefer otherwise.

## Phase Four: Review

1. **Find the root cause**: how the flaw was introduced, and whether a review, a test or a fuzz target would have caught it.
2. **Look for the same assumption elsewhere**, such as the same misunderstanding of a platform, a format or Git's behavior, and correct it.
3. **Update this plan** with what the incident taught.
