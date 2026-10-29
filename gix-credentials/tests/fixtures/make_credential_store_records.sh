#!/usr/bin/env bash
set -eu -o pipefail

# Git runs the helper `store`, its builtin `credential-store`, for `git credential fill`, `approve` and `reject`, and the
# tests run the same steps through gix-credentials, which runs `store` in this process, to compare what `fill` printed
# and the credentials files with Git's.
#
# Each scenario is a repository in which Git ran the steps with `HOME` set to its `home`, and `XDG_CONFIG_HOME` to its
# `config` unless `xdg-config-home` says `unset`. `helper` holds the helper, and `initial` the credentials files the
# scenario starts with. `steps/<n>` holds the operation of `git credential` in the n-th step in `op`, its input in
# `input`, what it printed in `output`, and the credentials files after it in `files`. The credentials files that Git
# left are in the scenario directory. All credentials are fictitious, and the hosts are example domains.

root=$PWD
# The credentials files of the scenarios, relative to their directory.
files=(creds creds.lock home/creds home/.git-credentials config/git/credentials home/.config/git/credentials)

# Print the directory $1 as Git sees it, which on Windows is a path with a drive letter.
native_dir() {
  (cd "$1" && { pwd -W 2>/dev/null || pwd -P; })
}

# Start the scenario $1 with the helper $2, and with `XDG_CONFIG_HOME` unset if $3 is `unset`.
scenario() {
  dir=$root/$1
  helper=$2
  xdg_config_home=${3:-config}
  n=0
  git init -q "$dir"
  mkdir -p "$dir/home" "$dir/config" "$dir/initial"
  printf '%s' "$helper" >"$dir/helper"
  printf '%s' "$xdg_config_home" >"$dir/xdg-config-home"
}

# Start the current scenario with the credentials file $1, with the content $2 and its escapes of `printf '%b'`.
initial() {
  mkdir -p "$(dirname "$dir/initial/$1")" "$(dirname "$dir/$1")"
  printf '%b' "$2" >"$dir/initial/$1"
  cp "$dir/initial/$1" "$dir/$1"
}

# Run `git credential $1` in the current scenario with the input $2 and its escapes of `printf '%b'`, and record the
# step.
step() {
  local op=$1 record file
  n=$((n + 1))
  record=$dir/steps/$n
  mkdir -p "$record/files"
  printf '%s' "$op" >"$record/op"
  printf '%b' "$2" >"$record/input"
  (
    cd "$dir"
    HOME=$(native_dir "$dir/home")
    export HOME
    if test "$xdg_config_home" = unset; then
      unset XDG_CONFIG_HOME
    else
      XDG_CONFIG_HOME=$(native_dir "$dir/config")
      export XDG_CONFIG_HOME
    fi
    git -c "credential.helper=$helper" -c credential.useHttpPath=true credential "$op" <"$record/input" >"$record/output"
  )
  for file in "${files[@]}"; do
    if test -e "$dir/$file"; then
      mkdir -p "$(dirname "$record/files/$file")"
      cp "$dir/$file" "$record/files/$file"
    fi
  done
}

# A file named with `--file`, relative to the repository: lines that aren't credentials, a credential without password,
# a line with a carriage return and a last line without line feed are kept as they are, the line of a new credential
# comes first and replaces those of the same user, and erasing matches the password only if it's given.
scenario file 'store --file=creds'
initial creds 'https://alice:old-password@example.com\nnot a credential\n\nhttps://no-password@example.com\nhttps://example.com\nhttps://bob:pass%20word@example.org/path\r\nhttps://carol:third@example.com/org/repo.git'
step approve 'protocol=https\nhost=example.com\nusername=alice\npassword=fictitious password/with:reserved@chars\n'
step fill 'protocol=https\nhost=example.com\n'
step fill 'protocol=https\nhost=example.com\npath=org/repo.git\nusername=carol\n'
step approve "protocol=https\nhost=example.com\npath=org/other.git\nusername=dave\npassword=dave's password\n"
step reject 'protocol=https\nhost=example.com\nusername=alice\npassword=wrong\n'
step reject 'protocol=https\nhost=example.com\nusername=alice\n'
step reject 'protocol=https\nhost=example.com\n'

# Without files, a credential is stored in `~/.git-credentials`.
scenario defaults store
step approve 'protocol=https\nhost=example.com\nusername=bob\npassword=fictitious-password\n'
step fill 'protocol=https\nhost=example.com\n'

# If only `$XDG_CONFIG_HOME/git/credentials` exists, it's used.
scenario xdg store
initial config/git/credentials 'https://erin:xdg-password@example.net\n'
step fill 'protocol=https\nhost=example.net\n'
step approve 'protocol=https\nhost=example.net\nusername=frank\npassword=fictitious-password\n'
step reject 'protocol=https\nhost=example.net\nusername=erin\n'

# With both files, `~/.git-credentials` is read first and stored to, and erasing applies to both.
scenario both store
initial home/.git-credentials 'https://gina:home-password@example.com\n'
initial config/git/credentials 'https://gina:xdg-password@example.com\nhttps://hank:xdg-password@example.com\n'
step fill 'protocol=https\nhost=example.com\n'
step fill 'protocol=https\nhost=example.com\nusername=hank\n'
step approve 'protocol=https\nhost=example.com\nusername=ivan\npassword=fictitious-password\n'
step reject 'protocol=https\nhost=example.com\nusername=gina\n'

# Without `XDG_CONFIG_HOME`, its file is `~/.config/git/credentials`.
scenario xdg-unset store unset
initial home/.config/git/credentials 'https://judy:config-password@example.com\n'
step fill 'protocol=https\nhost=example.com\n'
step approve 'protocol=https\nhost=example.com\nusername=karl\npassword=fictitious-password\n'

# The shell expands a `~` that starts the word of the file.
scenario tilde 'store --file ~/creds'
step approve 'protocol=https\nhost=example.com\nusername=lena\npassword=fictitious-password\n'
step fill 'protocol=https\nhost=example.com\n'

# While another process holds the lock of the file, nothing is stored.
scenario locked 'store --file=creds'
initial creds 'https://mike:old-password@example.com\n'
initial creds.lock ''
step approve 'protocol=https\nhost=example.com\nusername=mike\npassword=new-password\n'
