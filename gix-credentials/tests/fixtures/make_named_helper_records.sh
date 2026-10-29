#!/usr/bin/env bash
set -eu -o pipefail

# Git runs `recorder`, a helper named in the configuration, as the program `git-credential-recorder` in `bin`, which is
# in `PATH`, and the tests run the same helper through gix-credentials to compare its records with those Git made.
#
# `git-credential-recorder` appends its arguments, its input and `---` to `$CREDENTIAL_RECORDS`, and answers `get` with
# a fictitious credential. Each scenario is a repository in which Git ran `git credential fill` for a URL, and then
# `approve` and `reject` with what `fill` printed, which is in `fill`. `helper` holds the helper of the scenario,
# `use-http-path` the value of `credential.useHttpPath`, and `records` what the helper recorded.

mkdir bin
cat >bin/git-credential-recorder <<'EOF'
#!/bin/sh
for action; do :; done
{
  printf 'arguments:'
  printf ' [%s]' "$@"
  printf '\n'
  cat
  printf '%s\n' ---
} >>"$CREDENTIAL_RECORDS"
if test "$action" = get; then
  printf 'username=alice\npassword=fictitious-password\n'
fi
EOF
chmod +x bin/git-credential-recorder
export PATH="$PWD/bin:$PATH" RECORDER_MODE=from-environment

# Record in the repository $1 how Git runs the helper $2 for `fill`, `approve` and `reject`, with
# `credential.useHttpPath` set to $3.
scenario() {
  local dir=$1 helper=$2 use_http_path=$3
  git init -q "$dir"
  (
    cd "$dir"
    printf '%s' "$helper" >helper
    printf '%s' "$use_http_path" >use-http-path
    export CREDENTIAL_RECORDS="$PWD/records"
    local git=(git -c "credential.helper=$helper" -c "credential.useHttpPath=$use_http_path")
    printf 'url=https://example.com/org/repo.git\n' | "${git[@]}" credential fill >fill
    "${git[@]}" credential approve <fill
    "${git[@]}" credential reject <fill
  )
}

scenario plain recorder false
scenario quoted-arguments 'recorder --mode="two words"' false
scenario shell-arguments 'recorder --mode=$RECORDER_MODE' false
scenario http-path recorder true
