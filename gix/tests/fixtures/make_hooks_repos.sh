#!/usr/bin/env bash
set -eu -o pipefail

git init -q default

git init -q --bare bare.git

git init -q hooks-path
(cd hooks-path
  mkdir custom-hooks
  git config core.hooksPath custom-hooks
)

git init -q configured
(cd configured
  git config hook.linter.event pre-commit
  git config hook.linter.command 'echo lint'
  git config hook.off.event pre-commit
  git config hook.off.command 'echo off'
  git config hook.off.enabled false
)

git init -q invalid
(cd invalid
  git config hook.pre-commit.event pre-commit
  git config hook.pre-commit.command 'echo collides'
)
