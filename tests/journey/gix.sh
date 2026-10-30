# Must be sourced into the main journey test

title plumbing "${kind}"
snapshot="$snapshot/plumbing"

if test "$kind" != "small"; then
title "gix tracing"
(with "buffered traces on stderr"
  sandbox
  unset CLICOLOR CLICOLOR_FORCE FORCE_COLOR NO_COLOR

  # Pipe stderr to exercise terminal detection, keeping command output separate.
  # pipefail preserves the command's failure status through the capture pipeline.
  function capture-trace() (
    set -o pipefail
    "$exe_plumbing" "$@" 2>&1 >trace-out | cat >trace-err
  )

  expect_run $SUCCESSFULLY capture-trace env
  cp trace-out expected-out
  for trace in -t -tt -ttt -tttt; do
    case "$trace" in
      -t|-tt) span_marker='run [' ;;
      *) span_marker='run:' ;;
    esac
    it "$trace preserves stdout and emits plain spans without progress" && {
      expect_run $SUCCESSFULLY capture-trace "$trace" --no-verbose env
      expect_run $SUCCESSFULLY cmp expected-out trace-out
      expect_run $SUCCESSFULLY grep -Fq "$span_marker" trace-err
      expect_run $WITH_FAILURE grep -Fq $'\033' trace-err
    }
  done

  it "traces independently of verbose progress" && {
    expect_run $SUCCESSFULLY capture-trace -t --verbose env
    expect_run $SUCCESSFULLY cmp expected-out trace-out
    expect_run $SUCCESSFULLY grep -Fq 'run [' trace-err
  }

  it "flushes completed spans when a command fails" && {
    expect_run $WITH_FAILURE capture-trace -t --format json env
    expect_run $SUCCESSFULLY test ! -s trace-out
    expect_run $SUCCESSFULLY grep -Fq 'run [' trace-err
    expect_run $SUCCESSFULLY grep -Fq "JSON output isn't supported" trace-err
    expect_run $WITH_FAILURE grep -Fq $'\033' trace-err
  }

  it "traces completion generation without changing the generated script" && {
    expect_run $SUCCESSFULLY capture-trace completions --shell bash
    cp trace-out expected-completions
    expect_run $SUCCESSFULLY capture-trace -t completions --shell bash
    expect_run $SUCCESSFULLY cmp expected-completions trace-out
    expect_run $SUCCESSFULLY grep -Fq 'run [' trace-err
  }

  if test "$kind" = "max" || test "$kind" = "async"; then
    # Empty bare repos keep OPNR cheap; fresh databases give each case run IDs 1 and 2.
    mkdir corpus
    git init --bare -q corpus/one.git
    git init --bare -q corpus/two.git

    for threads in 1 2; do
      it "keeps corpus runs silent without tracing ($threads threads)" && {
        expect_run $SUCCESSFULLY capture-trace --no-verbose --threads "$threads" \
          corpus --db "corpus-$threads-off.db" --path corpus run --include-task OPNR
        expect_run $SUCCESSFULLY test ! -s trace-out
        expect_run $SUCCESSFULLY test ! -s trace-err
      }

      for trace in -t -tt -ttt -tttt; do
        it "$trace flushes both corpus run traces ($threads threads)" && {
          expect_run $SUCCESSFULLY capture-trace "$trace" --no-verbose --threads "$threads" \
            corpus --db "corpus-$threads$trace.db" --path corpus run --include-task OPNR
          expect_run $SUCCESSFULLY test ! -s trace-out
          for run_id in 1 2; do
            case "$trace" in
              -t|-tt) span_pattern="run \[.*run_id: $run_id$" ;;
              *) span_pattern="run\{run_id=$run_id\}:.*close" ;;
            esac
            expect_run $SUCCESSFULLY grep -Eq "$span_pattern" trace-err
          done
          expect_run $WITH_FAILURE grep -Fq $'\033' trace-err
        }
      done
    done
  fi
  
  it "keeps status worker spans under the command's root span" && {
    git init -q status-repo
    printf 'tracked\n' >status-repo/tracked
    git -C status-repo add tracked
    git -C status-repo commit -qm initial
    expect_run $SUCCESSFULLY capture-trace -tt --no-verbose -r status-repo status
    for span in gix::tree_index_status gix::index_worktree_status gix_status::index_as_worktree walk; do
      expect_run $SUCCESSFULLY grep -Fq "$span [" trace-err
    done
    expect_run $SUCCESSFULLY test "$(grep -Ec '^(INFO|DEBUG) +[[:alnum:]_].* \[' trace-err)" = 1
  }
)
fi

title "gix-tempfile crate"
(when "testing 'gix-tempfile'"
  snapshot="$snapshot/gix-tempfile"
  sandbox
  manifest="$root/../gix-tempfile/Cargo.toml"
  ABORTED=143

  (when "running the example program to raise a signal with a tempfile present"
    it "fails as the process aborts" && {
      expect_run $ABORTED cargo run --manifest-path "$manifest" --features signals --example delete-tempfiles-on-sigterm
    }
    TEMPFILE="$(cargo run --manifest-path "$manifest" --features signals --example delete-tempfiles-on-sigterm 2>/dev/null || true)"
    it "outputs a tempfile with an expected name" && {
      expect_run $SUCCESSFULLY test "$TEMPFILE" = "tempfile.ext"
    }
    it "cleans up the tempfile '$TEMPFILE' it created" && {
      expect_run $WITH_FAILURE test -e "$TEMPFILE"
    }
  )

  (when "running the example program to help assure there cannot be deadlocks"
    ABORTED=134
    it "succeeds as it won't deadlock" && {
      expect_run $ABORTED cargo run --manifest-path "$manifest" --release --features signals --example try-deadlock-on-cleanup -- 1
    }
  )
)

title '`gix` crate'
(when "testing 'gix'"
  snapshot="$snapshot/gix"
  sandbox
  manifest="$root/../gix/Cargo.toml"
  ABORTED=143

  (when "running the example program to check order of signal handlers"
    it "fails as the process aborts" && {
      expect_run $ABORTED cargo run --manifest-path "$manifest" --no-default-features --features interrupt --example interrupt-handler-allows-graceful-shutdown
    }
    it "cleans up the tempfile it created" && {
      expect_run $WITH_FAILURE test -e "example-file.tmp"
    }
  )
  (when "running the example program to check reversibility of signal handlers"
    it "fails as the process aborts" && {
      expect_run $ABORTED cargo run --manifest-path "$manifest" --no-default-features --features interrupt --example reversible-interrupt-handlers
    }
  )
)

title "gix (with repository)"
(with "a git repository"
  snapshot="$snapshot/repository"

  title "gix editor"
  (sandbox
    git init -q
    touch first second
    printf '%s\n' '#!/bin/sh' \
      'test "$#" = 2 && test "$1" = first && test "$2" = second' >check-editor
    cp check-editor "check editor"
    chmod +x "check editor"

    it "launches the configured editor with all paths" && {
      expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose -c "core.editor=sh check-editor" editor first second
    }
    it "preserves shell execution for editor command strings" && {
      expect_run $SUCCESSFULLY env GIT_EDITOR='exec true' "$exe_plumbing" --no-verbose editor first second
    }
    it "launches an existing editor path with spaces directly" && {
      expect_run $SUCCESSFULLY env GIT_EDITOR="$PWD/check editor" "$exe_plumbing" --no-verbose editor first second
    }
    it "gives a relative editor path with spaces to the shell as a command line, which doesn't find it, as Git does" && {
      expect_run $WITH_FAILURE env GIT_EDITOR='./check editor' "$exe_plumbing" --no-verbose editor first second
    }
    it "rejects an explicitly empty editor" && {
      expect_run $WITH_FAILURE env GIT_EDITOR= "$exe_plumbing" --no-verbose editor
    }
    it "honors the whitespace-padded no-op editor" && {
      expect_run $SUCCESSFULLY env 'GIT_EDITOR=: ' "$exe_plumbing" --no-verbose editor first second
    }
  )

  title "gix index"
  (with "the 'entries' sub-command"
    snapshot="$snapshot/index/entries"
    (sandbox
      git init -q

      # Keep the hostile names in the index only so this regression also works on
      # filesystems that reject terminal control characters in paths.
      empty_blob="$(git hash-object -w --stdin </dev/null)"
      ansi_path="$(printf 'ansi-\033[31mred\033[0m')"
      newline_path="$(printf 'line\nbreak')"
      git update-index --add --cacheinfo "100644,$empty_blob,$ansi_path"
      git update-index --add --cacheinfo "100644,$empty_blob,$newline_path"

      it "escapes repository-controlled paths in simple output" && {
        WITH_SNAPSHOT="$snapshot/control-characters-simple" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose index entries --no-attributes
      }
      it "escapes repository-controlled paths in rich output" && {
        WITH_SNAPSHOT="$snapshot/control-characters-rich" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose index entries --no-attributes --format rich
      }
    )
  )

  title "gix exclude"
  (with "the 'query' sub-command"
    snapshot="$snapshot/exclude/query"
    (sandbox
      git init -q

      # An ignored directory with an indexed file must still expose untracked ignored children.
      mkdir -p src/bin other/bin
      printf 'bin/\nbuild/\n' >.gitignore
      printf 'fn main() {}\n' >src/bin/stub_gen.rs
      git add -f .gitignore src/bin/stub_gen.rs
      printf 'extra\n' >src/bin/extra.txt
      printf 'other\n' >other/bin/file.txt
      printf '%s\n' \
        src/bin \
        src/bin/ \
        src/bin/stub_gen.rs \
        src/bin/extra.txt \
        other/bin \
        other/bin/file.txt >paths

      it "handles tracked paths below ignored directories" && {
        WITH_SNAPSHOT="$snapshot/tracked-paths-below-ignored-directories" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose exclude query \
          src/bin src/bin/ src/bin/stub_gen.rs src/bin/extra.txt other/bin other/bin/file.txt
      }
      it "produces the same output for paths read from stdin" && {
        WITH_SNAPSHOT="$snapshot/same-paths-from-stdin" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose exclude query <paths
      }
      it "resolves stdin paths relative to the current directory" && {
        (
          cd src &&
          WITH_SNAPSHOT="$snapshot/stdin-paths-relative-to-cwd" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose exclude query \
            < <(printf 'bin/stub_gen.rs\nbin/extra.txt\n')
        )
      }
      it "doesn't show ignore patterns for tracked paths" && {
        WITH_SNAPSHOT="$snapshot/tracked-path-with-ignore-patterns" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose exclude query --show-ignore-patterns src/bin/stub_gen.rs
      }
      it "keeps positional output in input order" && {
        WITH_SNAPSHOT="$snapshot/positional-output-order" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose exclude query src/bin/stub_gen.rs other/bin/file.txt
      }
      it "preserves directory intent for missing paths" && {
        WITH_SNAPSHOT="$snapshot/missing-directory" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose exclude query build/
      }
    )
  )

  (small-repo-in-sandbox
    (with "the 'verify' sub-command"
      snapshot="$snapshot/verify"
      (with 'human output format'
        it "generates correct output" && {
          WITH_SNAPSHOT="$snapshot/success-format-human" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose --format human verify -s
        }
      )
      if test "$kind" = "max" || test "$kind" = "max-pure"; then
      (with "--format json"
        it "generates the correct output in JSON format" && {
          WITH_SNAPSHOT="$snapshot/success-format-json" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose --format json verify --statistics
        }
      )
      fi
    )
  )

  title "gix diff"
  (with "the 'diff' sub-command"
    snapshot="$snapshot/diff"
    (small-repo-in-sandbox
      (with "'file', an index revspec, and an on-disk path"
        it "diffs the blob in the index against the file on disk" && {
          echo "an addition" >> b &&
          WITH_SNAPSHOT="$snapshot/file-index-vs-worktree" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose diff file :b b
        }
      )
    )
  )

  title "gix remote"
  (when "running 'remote'"
    snapshot="$snapshot/remote"
    title "gix remote refs"
    (with "the 'refs' subcommand"
      snapshot="$snapshot/refs"
      (small-repo-in-sandbox
        if [[ "$kind" != "small" ]]; then

        if [[ "$kind" != "async" ]]; then
        (with "file:// protocol"
          (with "version 1"
            it "generates the correct output" && {
              WITH_SNAPSHOT="$snapshot/file-v-any" \
              expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose -c protocol.version=1 remote -n .git refs
            }
          )
          (with "version 2"
            it "generates the correct output" && {
              WITH_SNAPSHOT="$snapshot/file-v-any" \
              expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose -c protocol.version=2  remote -n "$PWD" refs
            }
          )
          if test "$kind" = "max" || test "$kind" = "max-pure"; then
          (with "--format json"
            it "generates the correct output in JSON format" && {
              WITH_SNAPSHOT="$snapshot/file-v-any-json" \
              expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose --format json remote -n . refs
            }
          )
          fi
        )
        fi

        # Use a wrapper to bind to an ephemeral port and only continue once the
        # daemon can serve a real Git request.
        (with "git:// protocol"
          launch-git-daemon
          (with "version 1"
            it "generates the correct output" && {
              WITH_SNAPSHOT="$snapshot/file-v-any" \
              expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose --config protocol.version=1 remote --name "$git_daemon_url" refs
            }
          )
          (with "version 2"
            it "generates the correct output" && {
              WITH_SNAPSHOT="$snapshot/file-v-any" \
              expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose -c protocol.version=2 remote -n "$git_daemon_url" refs
            }
            if [[ "$kind" == "async" ]]; then
            it "keeps async progress alive while tracing" && {
              expect_run $SUCCESSFULLY bash -c \
                '"$1" -t -c protocol.version=2 remote -n "$2" refs >remote-out 2>remote-err' \
                -- "$exe_plumbing" "$git_daemon_url"
              expect_run $SUCCESSFULLY test -s remote-out
              expect_run $SUCCESSFULLY grep -Fq 'Connecting to' remote-err
              expect_run $SUCCESSFULLY grep -Fq 'run [' remote-err
            }
            fi
          )
        )
        if [[ "$kind" == "small" ]]; then
        (with "https:// protocol (in small builds)"
          it "fails as http is not compiled in" && {
            WITH_SNAPSHOT="$snapshot/fail-http-in-small" \
            expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose -c protocol.version=1 remote -n https://github.com/byron/gitoxide refs
          }
        )
        fi
        (on_ci
          if test "$kind" = "max" || test "$kind" = "max-pure"; then
          (with "https:// protocol"
            (with "version 1"
              it "generates the correct output" && {
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose -c protocol.version=1 remote -n https://github.com/byron/gitoxide refs
              }
            )
            (with "version 2"
              it "generates the correct output" && {
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose -c protocol.version=2 remote -n https://github.com/byron/gitoxide refs
              }
            )
          )
          fi
        )
        else
          it "fails as the CLI doesn't include networking in 'small' mode" && {
            WITH_SNAPSHOT="$snapshot/remote ref-list-no-networking-in-small-failure" \
            expect_run 2 "$exe_plumbing" --no-verbose -c protocol.version=1 remote -n .git refs
          }
        fi
      )
    )
  )
  if test "$kind" = "max" || test "$kind" = "max-pure"; then
  title "gix fetch"
  (when "running 'fetch'"
    snapshot="$snapshot/fetch"
    (with "a SHA-256 repository"
      (sandbox
        git init --object-format=sha256 -q remote
        (
          cd remote
          git checkout -q -b main
          echo first >file
          git add file
          git commit -q -m "first"
        )
        git clone -q "$PWD/remote" clone
        (
          cd remote
          echo second >file
          git commit -q -am "second"
        )

        it "fetches from origin" && {
          expect_run_sh $SUCCESSFULLY "cd clone && \"$exe_plumbing\" --no-verbose fetch"
        }
        it "updates the remote-tracking branch" && {
          expect_run_sh $SUCCESSFULLY 'test "$(git -C clone rev-parse refs/remotes/origin/main)" = "$(git -C remote rev-parse refs/heads/main)"'
        }
      )
    )
  )
  fi

  title "gix commit"
  (when "running 'commit'"
    snapshot="$snapshot/commit"
    (sandbox
      git init -q
      echo content >file
      git add file

      it "refuses to amend without a commit" && {
        WITH_SNAPSHOT="$snapshot/amend-without-commit-failure" \
        expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose commit --amend -m "amended"
      }
      it "refuses an empty message" && {
        WITH_SNAPSHOT="$snapshot/empty-message-failure" \
        expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose commit -m ""
      }
      it "refuses a message from both -m and -F" && {
        WITH_SNAPSHOT="$snapshot/message-and-file-failure" \
        expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose commit -m "subject" -F file
      }
      it "refuses an unknown cleanup mode" && {
        WITH_SNAPSHOT="$snapshot/unknown-cleanup-mode-failure" \
        expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose commit --cleanup=unknown -m "subject"
      }
      it "makes the root commit with a paragraph for each -m" && {
        WITH_SNAPSHOT="$snapshot/root-commit" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit -m "first" --message "the body"
      }
      it "joins the paragraphs as Git joins them" && {
        WITH_SNAPSHOT="$snapshot/root-commit-message" \
        expect_run $SUCCESSFULLY git log -1 --format=%B
      }
      it "refuses to commit the tree of the parent" && {
        WITH_SNAPSHOT="$snapshot/nothing-to-commit-failure" \
        expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose commit -m "nothing"
      }
      it "commits the tree of the parent with --allow-empty" && {
        WITH_SNAPSHOT="$snapshot/allow-empty" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit --allow-empty -m "empty"
      }
      it "commits an empty message with --allow-empty-message" && {
        WITH_SNAPSHOT="$snapshot/allow-empty-message" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit --allow-empty --allow-empty-message -m ""
      }
      it "takes the message from a file with -F" && {
        echo second >>file
        git add file
        printf 'from a file\n\nwith a body\n' >.git/message-file
        WITH_SNAPSHOT="$snapshot/message-from-file" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit -F .git/message-file
      }
      it "takes the message from standard input with --file -" && {
        echo third >>file
        git add file
        WITH_SNAPSHOT="$snapshot/message-from-stdin" \
        expect_run_sh $SUCCESSFULLY "echo 'from standard input' | \"$exe_plumbing\" --no-verbose commit --file -"
      }
      it "amends the commit of HEAD and keeps its message" && {
        echo amended >>file
        git add file
        WITH_SNAPSHOT="$snapshot/amend" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit --amend
      }
      it "amends the commit of HEAD with a new message" && {
        WITH_SNAPSHOT="$snapshot/amend-with-message" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit --amend -m "amended"
      }
      it "shows the author if it isn't the committer" && {
        echo other >>file
        git add file
        WITH_SNAPSHOT="$snapshot/other-author" \
        expect_run $SUCCESSFULLY env GIT_AUTHOR_NAME="Other Author" GIT_AUTHOR_EMAIL="other@example.com" \
          "$exe_plumbing" --no-verbose commit -m "by another author"
      }
      it "summarizes additions, renames, mode changes and deletions" && {
        mkdir dir
        echo one >dir/old
        echo two >removed
        git add dir removed
        git commit -q -m "more files"
        git mv dir/old dir/new
        git rm -q removed
        echo three >added
        chmod +x file
        git add added file
        WITH_SNAPSHOT="$snapshot/summary" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit -m "changes"
      }
      it "strips comments with --cleanup=strip" && {
        WITH_SNAPSHOT="$snapshot/cleanup-strip" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit --allow-empty --cleanup=strip -m "kept" -m "# dropped"
      }
      it "commits the stripped message" && {
        WITH_SNAPSHOT="$snapshot/cleanup-strip-message" \
        expect_run $SUCCESSFULLY git log -1 --format=%B
      }
      it "keeps the message as it is with --cleanup=verbatim" && {
        WITH_SNAPSHOT="$snapshot/cleanup-verbatim" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit --allow-empty --cleanup=verbatim -m "  kept  " -m "# kept"
      }
      it "commits the message as it is" && {
        WITH_SNAPSHOT="$snapshot/cleanup-verbatim-commit" \
        expect_run $SUCCESSFULLY git cat-file commit HEAD
      }
      it "commits on a detached HEAD" && {
        git checkout -q --detach
        WITH_SNAPSHOT="$snapshot/detached-head" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit --allow-empty -m "detached"
        git checkout -q main
      }
      (with "a failing pre-commit hook"
        mkdir -p .git/hooks
        printf '#!/bin/sh\necho "pre-commit refuses" >&2\nexit 1\n' >.git/hooks/pre-commit
        chmod +x .git/hooks/pre-commit
        echo hooked >>file
        git add file
        it "doesn't commit" && {
          WITH_SNAPSHOT="$snapshot/pre-commit-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose commit -m "hooked"
        }
        it "commits without running it with -n" && {
          WITH_SNAPSHOT="$snapshot/no-verify" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit -n -m "hooked"
        }
        rm .git/hooks/pre-commit
      )
      (with "a merge with a conflict"
        git checkout -q -b side
        echo side >conflict
        git add conflict
        git commit -q -m "side"
        git checkout -q main
        echo main >conflict
        git add conflict
        git commit -q -m "main"
        git merge -q side &>/dev/null || true
        it "refuses to commit unmerged entries" && {
          WITH_SNAPSHOT="$snapshot/unmerged-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose commit -m "merged"
        }
        it "concludes the merge with its message once the conflict is resolved" && {
          echo merged >conflict
          git add conflict
          WITH_SNAPSHOT="$snapshot/merge" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit
        }
      )
    )
  )

  title "gix reset"
  (when "running 'reset'"
    snapshot="$snapshot/reset"
    (small-repo-in-sandbox
      git tag third

      it "moves only HEAD with --soft" && {
        WITH_SNAPSHOT="$snapshot/soft" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose reset --soft HEAD~1
      }
      it "keeps the index and the worktree with --soft" && {
        WITH_SNAPSHOT="$snapshot/soft-status" \
        expect_run $SUCCESSFULLY git status --porcelain
      }
      it "resets the index to HEAD by default and lists the files that differ from it" && {
        WITH_SNAPSHOT="$snapshot/mixed" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose reset
      }
      it "resets the index and the worktree with --hard" && {
        WITH_SNAPSHOT="$snapshot/hard" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose reset --hard third
      }
      it "leaves no changes after --hard" && {
        WITH_SNAPSHOT="$snapshot/hard-status" \
        expect_run $SUCCESSFULLY git status --porcelain
      }
      (with "local changes"
        echo local >>a
        echo local >>b
        it "refuses to reset a changed file with --keep" && {
          WITH_SNAPSHOT="$snapshot/keep-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose reset --keep HEAD~1
        }
        it "refuses to reset a file with unstaged changes with --merge" && {
          WITH_SNAPSHOT="$snapshot/merge-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose reset --merge HEAD~1
        }
        it "keeps the unstaged changes of other files with --merge" && {
          git checkout -- b
          WITH_SNAPSHOT="$snapshot/merge" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose reset --merge HEAD~1
        }
        it "keeps the local changes of other files with --keep" && {
          WITH_SNAPSHOT="$snapshot/keep" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose reset --keep third
        }
        it "leaves the local changes" && {
          WITH_SNAPSHOT="$snapshot/keep-status" \
          expect_run $SUCCESSFULLY git status --porcelain
        }
      )
      (with "paths"
        git reset -q --hard third
        echo change >>b
        git add b
        it "resets the index entries of the paths after --" && {
          WITH_SNAPSHOT="$snapshot/paths" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose reset -- b
        }
        it "resets the index entries of the paths to those of a tree-ish" && {
          WITH_SNAPSHOT="$snapshot/paths-from-tree-ish" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose reset annotated b
        }
        it "warns about --mixed with paths" && {
          WITH_SNAPSHOT="$snapshot/mixed-with-paths" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose reset --mixed -- b
        }
        it "refuses --hard with paths" && {
          WITH_SNAPSHOT="$snapshot/hard-with-paths-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose reset --hard -- b
        }
        it "refuses --soft with paths" && {
          WITH_SNAPSHOT="$snapshot/soft-with-paths-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose reset --soft HEAD -- b
        }
        it "refuses an argument that names a revision and a file" && {
          touch dev
          WITH_SNAPSHOT="$snapshot/ambiguous-argument-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose reset dev
          rm dev
        }
        it "refuses an argument that names neither a revision nor a file" && {
          WITH_SNAPSHOT="$snapshot/unknown-argument-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose reset unknown
        }
        it "refuses a tree-ish that doesn't resolve" && {
          WITH_SNAPSHOT="$snapshot/unknown-tree-ish-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose reset unknown -- b
        }
      )
    )
    (sandbox
      git init -q
      touch file
      git add file
      it "removes all entries from the index on an unborn branch" && {
        WITH_SNAPSHOT="$snapshot/unborn" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose reset
      }
      it "leaves their files untracked" && {
        WITH_SNAPSHOT="$snapshot/unborn-status" \
        expect_run $SUCCESSFULLY git status --porcelain
      }
    )
  )

  title "gix merge"
  (when "running 'merge'"
    snapshot="$snapshot/merge"
    (sandbox
      git init -q -b main
      printf 'one\ntwo\nthree\n' >file
      git add file
      git commit -q -m "base"
      git checkout -q -b topic
      echo four >>file
      echo new >added
      git add added file
      git commit -q -m "topic"
      git checkout -q main

      it "fast-forwards and shows the statistics of the changes" && {
        WITH_SNAPSHOT="$snapshot/fast-forward" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge topic
      }
      it "says that HEAD contains the commit already" && {
        WITH_SNAPSHOT="$snapshot/up-to-date" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge topic
      }
      it "makes a merge commit with --no-ff" && {
        git checkout -q -b no-ff
        echo five >>file
        git commit -q -am "no-ff"
        git checkout -q main
        WITH_SNAPSHOT="$snapshot/no-ff" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge --no-ff no-ff
      }
      it "gives the merge commit the default message" && {
        WITH_SNAPSHOT="$snapshot/no-ff-message" \
        expect_run $SUCCESSFULLY git log -1 --format=%B
      }
      (with "a branch that diverged"
        git checkout -q -b side main~1
        echo side >side
        git add side
        git commit -q -m "side"
        git checkout -q main
        it "refuses to merge it with --ff-only" && {
          WITH_SNAPSHOT="$snapshot/ff-only-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose merge --ff-only side
        }
        it "merges it with a merge commit" && {
          WITH_SNAPSHOT="$snapshot/merge-commit" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge side
        }
        it "names the branch in the default message" && {
          WITH_SNAPSHOT="$snapshot/merge-commit-message" \
          expect_run $SUCCESSFULLY git log -1 --format=%B
        }
      )
      (with "the messages of -m and -F"
        git checkout -q -b with-message main~1
        echo with-message >with-message
        git add with-message
        git commit -q -m "with message"
        git checkout -q main
        printf 'from a file\n' >.git/message-file
        it "merges with the message" && {
          WITH_SNAPSHOT="$snapshot/message" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge -m "first" -F .git/message-file -m "third" with-message
        }
        it "joins the messages as Git joins them" && {
          WITH_SNAPSHOT="$snapshot/message-commit" \
          expect_run $SUCCESSFULLY git log -1 --format=%B
        }
      )
      (with "--squash"
        git checkout -q -b squashed main~1
        echo squashed >squashed
        git add squashed
        git commit -q -m "squashed"
        git checkout -q main
        it "applies the changes without commit" && {
          WITH_SNAPSHOT="$snapshot/squash" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge --squash squashed
        }
        it "leaves the changes staged" && {
          WITH_SNAPSHOT="$snapshot/squash-status" \
          expect_run $SUCCESSFULLY git status --porcelain
        }
        git reset -q --hard
      )
      (with "--no-commit"
        git checkout -q -b uncommitted main~1
        echo uncommitted >uncommitted
        git add uncommitted
        git commit -q -m "uncommitted"
        git checkout -q main
        it "stops before the merge commit" && {
          WITH_SNAPSHOT="$snapshot/no-commit" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge --no-commit uncommitted
        }
        it "refuses --continue with other arguments" && {
          WITH_SNAPSHOT="$snapshot/continue-with-argument-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose merge --continue --no-verify
        }
        it "makes the merge commit with --continue" && {
          WITH_SNAPSHOT="$snapshot/no-commit-continue" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge --continue
        }
      )
      (with "a conflict"
        git checkout -q -b conflicting
        printf 'one\nconflicting\nthree\nfour\nfive\n' >file
        git commit -q -am "conflicting"
        git checkout -q main
        printf 'one\nmain\nthree\nfour\nfive\n' >file
        git commit -q -am "main"
        it "fails and leaves the conflict to be resolved" && {
          WITH_SNAPSHOT="$snapshot/conflict-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose merge conflicting
        }
        it "leaves the conflict in the index" && {
          WITH_SNAPSHOT="$snapshot/conflict-status" \
          expect_run $SUCCESSFULLY git status --porcelain
        }
        it "refuses to continue while the conflict isn't resolved" && {
          WITH_SNAPSHOT="$snapshot/continue-unresolved-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose merge --continue
        }
        it "concludes the merge with --continue once the conflict is resolved" && {
          echo resolved >file
          git add file
          WITH_SNAPSHOT="$snapshot/conflict-continue" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge --continue
        }
      )
      (with "a conflict to abort"
        git checkout -q -b aborted
        echo aborted >file
        git commit -q -am "aborted"
        git checkout -q main
        echo main >file
        git commit -q -am "main again"
        it "fails at the conflict" && {
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose merge aborted
        }
        it "goes back to the state before the merge with --abort" && {
          WITH_SNAPSHOT="$snapshot/abort" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge --abort
        }
        it "leaves no changes" && {
          WITH_SNAPSHOT="$snapshot/abort-status" \
          expect_run $SUCCESSFULLY git status --porcelain
        }
        it "refuses --abort without merge" && {
          WITH_SNAPSHOT="$snapshot/abort-without-merge-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose merge --abort
        }
      )
      (with "a branch named like a subcommand"
        git checkout -q -b tree
        echo tree >tree-file
        git add tree-file
        git commit -q -m "tree"
        git checkout -q main
        it "merges it after --" && {
          WITH_SNAPSHOT="$snapshot/double-dash" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge -- tree
        }
      )
      (with "an upstream"
        git checkout -q -b upstream
        echo upstream >upstream
        git add upstream
        git commit -q -m "upstream"
        git checkout -q main
        git config branch.main.remote .
        git config branch.main.merge refs/heads/upstream
        it "merges it without commit" && {
          WITH_SNAPSHOT="$snapshot/upstream" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge
        }
        git config --unset branch.main.merge
        it "refuses to merge without commit and upstream" && {
          WITH_SNAPSHOT="$snapshot/no-upstream-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose merge
        }
      )
    )
    (small-repo-in-sandbox
      it "still merges files with 'file'" && {
        printf 'one\ntwo\nthree\n' >base.txt
        printf 'ONE\ntwo\nthree\n' >ours.txt
        printf 'one\ntwo\nTHREE\n' >theirs.txt
        WITH_SNAPSHOT="$snapshot/file" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge file ours.txt base.txt theirs.txt
      }
      it "still merges trees with 'tree'" && {
        WITH_SNAPSHOT="$snapshot/tree" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge tree main unannotated dev
      }
      it "still merges commits with 'commit'" && {
        WITH_SNAPSHOT="$snapshot/commit" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose merge commit main dev
      }
    )
  )

  title "gix rebase"
  (when "running 'rebase'"
    snapshot="$snapshot/rebase"
    (sandbox
      git init -q -b main
      printf 'one\ntwo\nthree\n' >file
      git add file
      git commit -q -m "base"
      git checkout -q -b topic
      echo t1 >t1
      git add t1
      git commit -q -m "t1"
      echo t2 >t2
      git add t2
      git commit -q -m "t2"
      git checkout -q main
      echo m1 >m1
      git add m1
      git commit -q -m "m1"
      git checkout -q topic

      it "replays the commits of the branch onto the upstream" && {
        WITH_SNAPSHOT="$snapshot/upstream" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose rebase main
      }
      it "puts them on top of the upstream" && {
        WITH_SNAPSHOT="$snapshot/upstream-log" \
        expect_run $SUCCESSFULLY git log --format=%s
      }
      it "says that the branch is up to date" && {
        WITH_SNAPSHOT="$snapshot/up-to-date" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose rebase main
      }
      it "replays the commits after a revision onto another with --onto" && {
        git branch old-main main
        git checkout -q main
        echo m2 >m2
        git add m2
        git commit -q -m "m2"
        git checkout -q topic
        WITH_SNAPSHOT="$snapshot/onto" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose rebase --onto main old-main topic
      }
      it "puts them on top of the new base" && {
        WITH_SNAPSHOT="$snapshot/onto-log" \
        expect_run $SUCCESSFULLY git log --format=%s
      }
      it "replays all commits of the branch with --root" && {
        WITH_SNAPSHOT="$snapshot/root" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose rebase --root
      }
      it "keeps their history" && {
        WITH_SNAPSHOT="$snapshot/root-log" \
        expect_run $SUCCESSFULLY git log --format=%s
      }
      (with "a conflict"
        git checkout -q -b conflicting main
        printf 'one\nconflicting\nthree\n' >file
        git commit -q -am "conflicting"
        echo after >after
        git add after
        git commit -q -m "after"
        git checkout -q main
        printf 'one\nmain\nthree\n' >file
        git commit -q -am "main"
        git checkout -q conflicting
        it "stops at the commit that doesn't apply" && {
          WITH_SNAPSHOT="$snapshot/conflict-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose rebase main
        }
        it "refuses to continue while the conflict isn't resolved" && {
          WITH_SNAPSHOT="$snapshot/continue-unresolved-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose rebase --continue
        }
        it "continues once the conflict is resolved" && {
          echo resolved >file
          git add file
          WITH_SNAPSHOT="$snapshot/conflict-continue" \
          expect_run $SUCCESSFULLY env GIT_EDITOR=: "$exe_plumbing" --no-verbose rebase --continue
        }
        it "keeps the resolution and the commits after it" && {
          WITH_SNAPSHOT="$snapshot/conflict-continue-log" \
          expect_run $SUCCESSFULLY git log --format=%s
        }
      )
      (with "a conflict to skip"
        git checkout -q -b skipped main~1
        printf 'one\nskipped\nthree\n' >file
        git commit -q -am "skipped"
        echo kept >kept
        git add kept
        git commit -q -m "kept"
        it "stops at the commit that doesn't apply" && {
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose rebase main
        }
        it "leaves the commit out with --skip" && {
          WITH_SNAPSHOT="$snapshot/skip" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose rebase --skip
        }
        it "replays the commits after it" && {
          WITH_SNAPSHOT="$snapshot/skip-log" \
          expect_run $SUCCESSFULLY git log --format=%s
        }
      )
      (with "a conflict to abort"
        git checkout -q -b aborted main~1
        printf 'one\naborted\nthree\n' >file
        git commit -q -am "aborted"
        it "stops at the commit that doesn't apply" && {
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose rebase main
        }
        it "goes back to the branch as it was with --abort" && {
          WITH_SNAPSHOT="$snapshot/abort" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose rebase --abort
        }
        it "restores the branch, its commits and the worktree" && {
          WITH_SNAPSHOT="$snapshot/abort-status" \
          expect_run_sh $SUCCESSFULLY 'git branch --show-current && git log --format=%s && git status --porcelain'
        }
        it "refuses --abort without rebase" && {
          WITH_SNAPSHOT="$snapshot/abort-without-rebase-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose rebase --abort
        }
      )
      (with "an interactive rebase"
        git checkout -q -b interactive main~1
        echo i1 >i1
        git add i1
        git commit -q -m "i1"
        echo i2 >i2
        git add i2
        git commit -q -m "i2"
        it "performs the todo list as the sequence editor leaves it" && {
          WITH_SNAPSHOT="$snapshot/interactive" \
          expect_run $SUCCESSFULLY env GIT_SEQUENCE_EDITOR="sed -i.bak -e 1d" "$exe_plumbing" --no-verbose rebase -i main
        }
        it "leaves out the commit whose line the editor removed" && {
          WITH_SNAPSHOT="$snapshot/interactive-log" \
          expect_run $SUCCESSFULLY git log --format=%s
        }
      )
      (with "--exec"
        it "runs the command after each commit" && {
          WITH_SNAPSHOT="$snapshot/exec" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose rebase --force-rebase --exec "echo executed" main
        }
        it "stops where the command fails" && {
          WITH_SNAPSHOT="$snapshot/exec-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose rebase --force-rebase --exec false main
        }
      )
    )
  )

  if test "$kind" = "max" || test "$kind" = "max-pure"; then
  title "gix push"
  (when "running 'push'"
    snapshot="$snapshot/push"
    (sandbox
      git init -q --bare remote.git
      git init -q local
      cd local
      git commit -q --allow-empty -m "first"
      git remote add origin ../remote.git

      it "creates a branch" && {
        WITH_SNAPSHOT="$snapshot/new-branch" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose push origin main
      }
      it "updates a branch" && {
        git commit -q --allow-empty -m "second"
        WITH_SNAPSHOT="$snapshot/updated-branch" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose push origin main
      }
      it "reports a branch that is up to date" && {
        WITH_SNAPSHOT="$snapshot/up-to-date" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose push origin main
      }
      it "rejects an update that isn't a fast-forward" && {
        git reset -q --hard HEAD~1
        git commit -q --allow-empty -m "diverged"
        WITH_SNAPSHOT="$snapshot/rejected" \
        expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose push origin main
      }
      it "rejects an update if the remote reference isn't where the lease expects it" && {
        WITH_SNAPSHOT="$snapshot/stale-lease" \
        expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose push --force-with-lease=main:HEAD origin main
      }
      it "forces an update if the remote reference is where its remote-tracking reference is" && {
        WITH_SNAPSHOT="$snapshot/forced-with-lease" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose push --force-with-lease origin main
      }
      it "forces an update with --force" && {
        git reset -q --hard HEAD~1
        git commit -q --allow-empty -m "forced"
        WITH_SNAPSHOT="$snapshot/forced" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose push --force origin main
      }
      it "deletes a branch" && {
        git push -q origin main:topic
        WITH_SNAPSHOT="$snapshot/deleted" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose push --delete origin topic
      }
      it "reports what it would push in a dry run" && {
        WITH_SNAPSHOT="$snapshot/dry-run" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose push --dry-run origin main:dry-run
      }
      it "doesn't change the remote in a dry run" && {
        expect_run $WITH_FAILURE git -C ../remote.git rev-parse --verify -q refs/heads/dry-run
      }
      it "pushes all tags with --tags" && {
        git tag v1
        WITH_SNAPSHOT="$snapshot/tags" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose push --tags origin
      }
      it "pushes the annotated tags of what it pushes with --follow-tags" && {
        git commit -q --allow-empty -m "tagged"
        git tag -a -m "annotated" v2
        WITH_SNAPSHOT="$snapshot/follow-tags" \
        expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose push --follow-tags origin main
      }
      it "updates no reference with --atomic if one is rejected" && {
        git branch other
        WITH_SNAPSHOT="$snapshot/atomic-failure" \
        expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose push --atomic origin other HEAD~1:main
      }
      (with "a failing pre-push hook"
        mkdir -p .git/hooks
        printf '#!/bin/sh\necho "pre-push refuses" >&2\nexit 1\n' >.git/hooks/pre-push
        chmod +x .git/hooks/pre-push
        it "doesn't push" && {
          WITH_SNAPSHOT="$snapshot/pre-push-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose push origin other
        }
        it "pushes without running it with --no-verify" && {
          WITH_SNAPSHOT="$snapshot/no-verify" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose push --no-verify origin other
        }
        rm .git/hooks/pre-push
      )
      (with "push options"
        it "refuses them if the remote doesn't support them" && {
          WITH_SNAPSHOT="$snapshot/push-option-unsupported-failure" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose push -o ci.skip origin main:with-option
        }
        git -C ../remote.git config receive.advertisePushOptions true
        mkdir -p ../remote.git/hooks
        printf '#!/bin/sh\nprintf "%%s\\n" "$GIT_PUSH_OPTION_COUNT" "$GIT_PUSH_OPTION_0" >push-options\n' >../remote.git/hooks/post-receive
        chmod +x ../remote.git/hooks/post-receive
        it "sends them to the hooks of the remote" && {
          WITH_SNAPSHOT="$snapshot/push-option" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose push --push-option ci.skip --no-thin origin main:with-option
        }
        it "passes them to the hooks" && {
          WITH_SNAPSHOT="$snapshot/push-option-received" \
          expect_run $SUCCESSFULLY cat ../remote.git/push-options
        }
      )
      it "refuses a repository that names a branch of a remote" && {
        WITH_SNAPSHOT="$snapshot/ref-like-repository-failure" \
        expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose push origin/main
      }
    )
  )
  fi
)

title "gix attributes"
(with "gix attributes"
  (with "the 'validate-baseline' sub-command"
    (sandbox
      git init -q
      # Cover attribute inheritance, overrides, binary files, and ignore rules in a disposable repository.
      printf '%s\n' '*.txt text' '*.bin binary' 'nested/** custom=root' >.gitattributes
      printf '%s\n' '*.ignored' >.gitignore
      mkdir nested
      printf '%s\n' '*.txt -text custom=nested' >nested/.gitattributes
      printf 'text\n' >file.txt
      printf '\0binary\n' >file.bin
      printf 'nested\n' >nested/file.txt
      touch file.ignored
      git add .gitattributes .gitignore file.txt file.bin nested

      it "agrees with Git for fixture attributes and ignore rules" && {
        expect_run_sh_no_pipefail $SUCCESSFULLY "find . -path ./.git -prune -o -type f -print | sed 's|^./||' | $exe_plumbing --no-verbose attributes validate-baseline"
      }
    )
  )
)

title "gix commit-graph"
(when "running 'commit-graph'"
  snapshot="$snapshot/commit-graph"
  title "gix commit-graph verify"
  (with "the 'verify' sub-command"
    snapshot="$snapshot/verify"

    (small-repo-in-sandbox
      (with "a valid and complete commit-graph file"
        git commit-graph write --reachable
        (with "statistics"
          it "generates the correct output" && {
            WITH_SNAPSHOT="$snapshot/statistics-success" \
            expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose commit-graph verify -s
          }
        )
        if test "$kind" = "max" || test "$kind" = "max-pure"; then
        (with "statistics --format json"
          it "generates the correct output" && {
            WITH_SNAPSHOT="$snapshot/statistics-json-success" \
            expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose --format json commit-graph verify -s
          }
        )
        fi
      )
    )
  )
)

(with "gix free"
  snapshot="$snapshot/no-repo"
  title "gix free commit-graph verify"
  (when "verifying standalone commit-graphs"
    for object_format in sha1 sha256; do
      (sandbox
        # Copy both graph layouts out of the repository so verification cannot infer its object format from it.
        git init --object-format="$object_format" -q repo
        (
          cd repo
          git -c commit.gpgsign=false commit --allow-empty -q -m "first"
          git commit-graph write --reachable
          cp -R .git/objects/info ../graph-info
          rm .git/objects/info/commit-graph
          git commit-graph write --reachable --split
          git -c commit.gpgsign=false commit --allow-empty -q -m "second"
          git commit-graph write --reachable --split=no-merge
          cp -R .git/objects/info ../split-graph-info
        )

        if test "$object_format" = sha1; then
          mismatched_format=sha256
        else
          mismatched_format=sha1
        fi
        for graph_path in graph-info graph-info/commit-graph split-graph-info split-graph-info/commit-graphs; do
          it "verifies $object_format at $graph_path with an explicit object format" && {
            expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose --object-hash "$object_format" free commit-graph verify "$graph_path"
          }
          it "rejects $object_format at $graph_path when $mismatched_format is requested" && {
            expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose --object-hash "$mismatched_format" free commit-graph verify "$graph_path"
          }
        done

        it "does not hide a corrupt $object_format monolithic graph behind a valid split graph" && {
          printf 'invalid graph\n' >split-graph-info/commit-graph
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose --object-hash "$object_format" free commit-graph verify split-graph-info
        }
      )
    done
  )

  title "gix free pack"
  (when "running 'pack'"
    snapshot="$snapshot/pack"

    title "gix free pack receive"
    (with "the 'receive' sub-command"
      snapshot="$snapshot/receive"
      (small-repo-in-sandbox
        if [[ "$kind" != 'small' ]]; then

        if [[ "$kind" != 'async' ]]; then
        (with "file:// protocol"
          (with "version 1"
            (with "NO output directory"
              it "generates the correct output" && {
                WITH_SNAPSHOT="$snapshot/file-v-any-no-output" \
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack receive -p 1 .git
              }
            )
            (with "output directory"
              mkdir out
              it "generates the correct output" && {
                WITH_SNAPSHOT="$snapshot/file-v-any-with-output" \
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack receive -p 1 .git out/
              }
              it "creates an index and a pack in the output directory" && {
                WITH_SNAPSHOT="$snapshot/ls-in-output-dir" \
                expect_run $SUCCESSFULLY ls out/
              }
              (with "--write-refs set"
                it "generates the correct output" && {
                  WITH_SNAPSHOT="$snapshot/file-v-any-with-output" \
                  expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack receive -p 1 --refs-directory out/all-refs .git out/
                }
                it "writes references into the refs folder of the output directory" && {
                  expect_snapshot "$snapshot/repo-refs" out/all-refs
                }
              )
              rm -Rf out
            )
            if test "$kind" = "max" || test "$kind" = "max-pure"; then
            (with "--format json"
              it "generates the correct output in JSON format" && {
                WITH_SNAPSHOT="$snapshot/file-v-any-no-output-json" \
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose --format json free pack receive --protocol 1 .git
              }
            )
            fi
          )
          (with "version 2"
            (with "NO output directory"
              it "generates the correct output" && {
                WITH_SNAPSHOT="$snapshot/file-v-any-no-output-p2" \
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack receive -p 2 .git
              }
            )
            (with "output directory"
              mkdir out/
              it "generates the correct output" && {
                WITH_SNAPSHOT="$snapshot/file-v-any-with-output-p2" \
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack receive .git out/
              }
              it "creates an index and a pack in the output directory" && {
                WITH_SNAPSHOT="$snapshot/ls-in-output-dir" \
                expect_run $SUCCESSFULLY ls out/
              }
              rm -Rf out
            )
            if test "$kind" = "max" || test "$kind" = "max-pure"; then
            (with "--format json"
              it "generates the correct output in JSON format" && {
                WITH_SNAPSHOT="$snapshot/file-v-any-no-output-json-p2" \
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose --format json free pack receive --protocol 2 .git
              }
            )
            fi
          )
        )
        fi
        (with "git:// protocol"
          launch-git-daemon
          (with "version 1"
            (with "NO output directory"
              (with "no wanted refs"
                it "generates the correct output" && {
                  WITH_SNAPSHOT="$snapshot/file-v-any-no-output" \
                  expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack receive -p 1 "$git_daemon_url"
                }
              )
              (with "wanted refs"
                it "generates the correct output" && {
                  WITH_SNAPSHOT="$snapshot/file-v-any-no-output-wanted-ref-p1" \
                  expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose free pack receive -p 1 "$git_daemon_url" -r =refs/heads/main
                }
              )
            )
            (with "output directory"
              mkdir out
              it "generates the correct output" && {
                WITH_SNAPSHOT="$snapshot/file-v-any-with-output" \
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack receive -p 1 "$git_daemon_url" out/
              }
            )
          )
          (with "version 2"
            (with "NO output directory"
              (with "NO wanted refs"
                it "generates the correct output" && {
                  WITH_SNAPSHOT="$snapshot/file-v-any-no-output-p2" \
                  expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack receive -p 2 "$git_daemon_url"
                }
              )
              (with "wanted refs"
                it "generates the correct output" && {
                  WITH_SNAPSHOT="$snapshot/file-v-any-no-output-single-ref" \
                  expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack receive -p 2 "$git_daemon_url" -r refs/heads/main
                }
                (when "ref does not exist"
                  it "fails with a detailed error message including what the server said" && {
                    WITH_SNAPSHOT="$snapshot/file-v-any-no-output-non-existing-single-ref" \
                    expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose free pack receive -p 2 "$git_daemon_url" -r refs/heads/does-not-exist
                  }
                )
              )
            )
            (with "output directory"
              it "generates the correct output" && {
                WITH_SNAPSHOT="$snapshot/file-v-any-with-output-p2" \
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack receive "$git_daemon_url" out/
              }
            )
          )
        )
        (on_ci
          if test "$kind" = "max" || test "$kind" = "max-pure"; then
          (with "https:// protocol"
            (with "version 1"
              it "works" && {
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack receive -p 1 https://github.com/byron/gitoxide
              }
            )
            (with "version 2"
              it "works" && {
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack receive -p 2 https://github.com/byron/gitoxide
              }
            )
          )
          (with "an ambiguous ssh username which could be mistaken for an argument"
            snapshot="$snapshot/fail-ambiguous-username"
            (with "explicit ssh (true url with scheme)"
              it "fails without trying to pass it to command-line programs" && {
                WITH_SNAPSHOT="$snapshot/explicit-ssh" \
                expect_run $WITH_FAILURE "$exe_plumbing" free pack receive 'ssh://-Fconfigfile@foo/bar'
              }
            )
            (with "implicit ssh (special syntax with no scheme)"
              it "fails without trying to pass it to command-line programs" && {
                WITH_SNAPSHOT="$snapshot/implicit-ssh" \
                expect_run $WITH_FAILURE "$exe_plumbing" free pack receive -- '-Fconfigfile@foo:bar/baz'
              }
            )
          )
          (with "an ambiguous ssh host which could be mistaken for an argument"
              it "fails without trying to pass it to command-line programs" && {
                WITH_SNAPSHOT="$snapshot/fail-ambiguous-host" \
                expect_run $WITH_FAILURE "$exe_plumbing" free pack receive 'ssh://-oProxyCommand=open$IFS-aCalculator/foo'
              }
          )
          (with "an ambiguous ssh path which could be mistaken for an argument"
              it "fails without trying to pass it to command-line programs" && {
                WITH_SNAPSHOT="$snapshot/fail-ambiguous-path" \
                expect_run $WITH_FAILURE "$exe_plumbing" free pack receive 'git@foo:-oProxyCommand=open$IFS-aCalculator/bar'
              }
          )
          fi
        )
        elif [[ "$kind" = "small" ]]; then
          it "fails as the CLI doesn't have networking in 'small' mode" && {
            WITH_SNAPSHOT="$snapshot/pack receive-no-networking-in-small-failure" \
            expect_run 2 "$exe_plumbing" --no-verbose free pack receive -p 1 .git
          }
        fi
      )
    )
    if test "$kind" = "max" || test "$kind" = "max-pure"; then
    (with "the 'clone' sub-command"
        snapshot="$snapshot/clone"
        (with "a SHA-256 remote"
          (sandbox
            git init --object-format=sha256 -q remote
            (
              cd remote
              git checkout -q -b main
              echo first >file
              git add file
              git commit -q -m "first"
            )

            it "adopts the remote object format" && {
              expect_run $SUCCESSFULLY "$exe_plumbing" clone "$PWD/remote" clone
            }
            it "persists the adopted object format" && {
              expect_run_sh $SUCCESSFULLY 'test "$(git -C clone config --get extensions.objectformat)" = sha256'
            }
            it "persists the origin remote" && {
              expect_run_sh $SUCCESSFULLY 'test -n "$(git -C clone config --get remote.origin.url)"'
            }
            it "records the remote default as a symbolic tracking reference" && {
              expect_run_sh $SUCCESSFULLY 'test "$(git -C clone symbolic-ref refs/remotes/origin/HEAD)" = refs/remotes/origin/main'
              expect_run_sh $SUCCESSFULLY 'test "$(git -C clone rev-parse refs/remotes/origin/HEAD)" = "$(git -C remote rev-parse HEAD)"'
            }
            it "prints the initial bare mapping without persisting it" && {
              function clone-bare() {
                "$exe_plumbing" --no-verbose clone --bare "$PWD/remote" bare >bare-output
              }
              expect_run $SUCCESSFULLY clone-bare
              expect_run $SUCCESSFULLY grep -Fq '+refs/heads/*:refs/heads/*' bare-output
              expect_run_sh $SUCCESSFULLY 'test "$(git -C bare rev-parse refs/heads/main)" = "$(git -C remote rev-parse HEAD)"'
              expect_run_sh $SUCCESSFULLY 'test "$(git -C bare for-each-ref --format="%(refname)" refs/remotes)" = ""'
              expect_run_sh $SUCCESSFULLY 'test "$(git -C bare config --get-all remote.origin.fetch)" = ""'
              expect_run_sh $SUCCESSFULLY 'test "$(git -C bare config --get branch.main.remote)" = ""'
            }
          )
        )
        (with "an ambiguous ssh username which could be mistaken for an argument"
          snapshot="$snapshot/fail-ambiguous-username"
          (with "explicit ssh (true url with scheme)"
            it "fails without trying to pass it to command-line programs" && {
              WITH_SNAPSHOT="$snapshot/explicit-ssh" \
              expect_run $WITH_FAILURE "$exe_plumbing" clone 'ssh://-Fconfigfile@foo/bar'
            }
          )
          (with "implicit ssh (special syntax with no scheme)"
            it "fails without trying to pass it to command-line programs" && {
              WITH_SNAPSHOT="$snapshot/implicit-ssh" \
              expect_run $WITH_FAILURE "$exe_plumbing" clone -- '-Fconfigfile@foo:bar/baz'
            }
          )
        )
        (with "an ambiguous ssh host which could be mistaken for an argument"
            it "fails without trying to pass it to command-line programs" && {
              WITH_SNAPSHOT="$snapshot/fail-ambiguous-host" \
              expect_run $WITH_FAILURE "$exe_plumbing" clone 'ssh://-oProxyCommand=open$IFS-aCalculator/foo'
            }
        )
        (with "an ambiguous ssh path which could be mistaken for an argument"
            it "fails without trying to pass it to command-line programs" && {
              WITH_SNAPSHOT="$snapshot/fail-ambiguous-path" \
              expect_run $WITH_FAILURE "$exe_plumbing" clone 'git@foo:-oProxyCommand=open$IFS-aCalculator/bar'
            }
        )
    )
    fi
    (with "the 'index' sub-command"
      snapshot="$snapshot/index"
      title "gix free pack index create"
      (with "the 'create' sub-command"
        snapshot="$snapshot/create"
        PACK_FILE="$fixtures/packs/pack-11fdfa9e156ab73caae3b6da867192221f2089c2.pack"
        (with "a valid and complete pack file"
          (with "NO output directory specified"
            (with "pack file passed as file"
              it "generates an index into a sink and outputs pack and index information" && {
                WITH_SNAPSHOT="$snapshot/no-output-dir-success" \
                expect_run $SUCCESSFULLY "$exe_plumbing" free pack index create -p "$PACK_FILE"
              }
            )
            (with "pack file passed from stdin"
              it "generates an index into a sink and outputs pack and index information" && {
                WITH_SNAPSHOT="$snapshot/no-output-dir-success" \
                expect_run $SUCCESSFULLY "$exe_plumbing" free pack index create < "$PACK_FILE"
              }
              if test "$kind" = "max" || test "$kind" = "max-pure"; then
              (with "--format json"
                it "generates the index into a sink and outputs information as JSON" && {
                  WITH_SNAPSHOT="$snapshot/no-output-dir-as-json-success" \
                  expect_run $SUCCESSFULLY "$exe_plumbing" --format json free pack index create < "$PACK_FILE"
                }
              )
              fi
            )
          )
          (sandbox
            (with "with an output directory specified"
              it "generates an index and outputs information" && {
                WITH_SNAPSHOT="$snapshot/output-dir-success" \
                expect_run $SUCCESSFULLY "$exe_plumbing" free pack index create -p "$PACK_FILE" "$PWD"
              }
              it "writes the index and pack into the directory (they have the same names, different suffixes)" && {
                WITH_SNAPSHOT="$snapshot/output-dir-content" \
                expect_run $SUCCESSFULLY ls
              }
            )
          )
        )
        (with "'restore' iteration mode"
          (sandbox
            cp "${PACK_FILE}" .
            PACK_FILE="${PACK_FILE##*/}"
            "$jtt" mess-in-the-middle "${PACK_FILE}"

            it "generates an index and outputs information (instead of failing)" && {
              WITH_SNAPSHOT="$snapshot/output-dir-restore-success" \
              expect_run $SUCCESSFULLY "$exe_plumbing" free pack index create -i restore -p "$PACK_FILE" "$PWD"
            }

            if test "$kind" = "max" || test "$kind" = "max-pure"; then
            (with "--format json and the very same output directory"
              it "generates the index, overwriting existing files, and outputs information as JSON" && {
                WITH_SNAPSHOT="$snapshot/output-dir-restore-as-json-success" \
                SNAPSHOT_FILTER=remove-paths \
                expect_run $SUCCESSFULLY "$exe_plumbing" --format json free pack index create -i restore $PWD < "$PACK_FILE"
              }
            )
            fi
          )
        )
      )
    )

    title "gix free pack multi-index"
    (with "the 'multi-index' sub-command"
        snapshot="$snapshot/multi-index"
        title "gix free pack multi-index create"
        (with "the 'create' sub-command"
            snapshot="$snapshot/create"
            (with 'multiple pack indices'
              (sandbox
                it "creates a multi-index successfully" && {
                  expect_run $SUCCESSFULLY "$exe_plumbing" free pack multi-index -i multi-pack-index create $fixtures/packs/pack-*.idx
                }
              )
            )
        )
    )

    title "gix free pack explode"
    (with "the 'explode' sub-command"
      snapshot="$snapshot/explode"
      PACK_FILE="$fixtures/packs/pack-11fdfa9e156ab73caae3b6da867192221f2089c2"
      (with "no objects directory specified"
        it "explodes the pack successfully and with desired output" && {
          WITH_SNAPSHOT="$snapshot/to-sink-success" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose no-repo pack explode "${PACK_FILE}.idx"
        }

        (when "using the --delete-pack flag"
          (sandbox
            (with "a valid pack"
              cp "${PACK_FILE}".idx "${PACK_FILE}".pack .
              PACK_FILE="${PACK_FILE##*/}"
              it "explodes the pack successfully and deletes the original pack and index" && {
                WITH_SNAPSHOT="$snapshot/to-sink-delete-pack-success" \
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack explode --check skip-file-checksum --delete-pack "${PACK_FILE}.pack"
              }
              it "removes the original files" && {
                expect_run $WITH_FAILURE test -e "${PACK_FILE}".pack
                expect_run $WITH_FAILURE test -e "${PACK_FILE}".idx
              }
            )
            (with "a pack file that is invalid somewhere"
              cp ${PACK_FILE}.idx ${PACK_FILE}.pack .
              PACK_FILE="${PACK_FILE##*/}"
              "$jtt" mess-in-the-middle "${PACK_FILE}".pack

              (with "and all safety checks"
                it "does not explode the file at all" && {
                  WITH_SNAPSHOT="$snapshot/broken-delete-pack-to-sink-failure" \
                  expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose free pack explode --sink-compress --check all --delete-pack "${PACK_FILE}.pack"
                }

                it "did not touch index or pack file" && {
                  expect_exists "${PACK_FILE}".pack
                  expect_exists "${PACK_FILE}".idx
                }
              )

              (with "and no safety checks at all (and an output directory)"
                it "does explode the file" && {
                  WITH_SNAPSHOT="$snapshot/broken-delete-pack-to-sink-skip-checks-success" \
                  expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack explode --verify --check skip-file-and-object-checksum-and-no-abort-on-decode \
                                            --delete-pack "${PACK_FILE}.pack" .
                }

                it "removes the original files" && {
                  expect_run $WITH_FAILURE test -e "${PACK_FILE}".pack
                  expect_run $WITH_FAILURE test -e "${PACK_FILE}".idx
                }

                it "creates all pack objects, but the broken ones" && {
                  WITH_SNAPSHOT="$snapshot/broken-with-objects-dir-skip-checks-success-tree" \
                  expect_run_sh $SUCCESSFULLY 'find . -type f | sort'
                }
              )
            )
          )
        )
      )
      (with "a non-existing directory specified"
        it "fails with a helpful error message" && {
          WITH_SNAPSHOT="$snapshot/missing-objects-dir-fail" \
          expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose free pack explode -c skip-file-and-object-checksum "${PACK_FILE}.idx" does-not-exist
        }
      )
      (with "an existing directory specified"
        (sandbox
          it "succeeds" && {
            WITH_SNAPSHOT="$snapshot/with-objects-dir-success" \
            expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack explode -c skip-file-and-object-checksum-and-no-abort-on-decode \
                                                     "${PACK_FILE}.pack" .
          }

          it "creates all pack objects" && {
            WITH_SNAPSHOT="$snapshot/with-objects-dir-success-tree" \
            expect_run_sh $SUCCESSFULLY 'find . -type f | sort'
          }
        )
      )
    )

    title "gix free pack verify"
    (with "the 'verify' sub-command"
      snapshot="$snapshot/verify"
      (with "a valid pack file"
        PACK_FILE="$fixtures/packs/pack-11fdfa9e156ab73caae3b6da867192221f2089c2.pack"
        it "verifies the pack successfully and with desired output" && {
          WITH_SNAPSHOT="$snapshot/success" \
          expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack verify "$PACK_FILE"
        }
      )
      (with "a valid pack INDEX file"
        MULTI_PACK_INDEX="$fixtures/packs/pack-11fdfa9e156ab73caae3b6da867192221f2089c2.idx"
        (with "no statistics"
          it "verifies the pack index successfully and with desired output" && {
            WITH_SNAPSHOT="$snapshot/index-success" \
            expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack verify "$MULTI_PACK_INDEX"
          }
        )
        (with "statistics"
          it "verifies the pack index successfully and with desired output" && {
            WITH_SNAPSHOT="$snapshot/index-with-statistics-success" \
            expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack verify --statistics "$MULTI_PACK_INDEX"
          }

          (with "and the less-memory algorithm"
            it "verifies the pack index successfully and with desired output" && {
              WITH_SNAPSHOT="$snapshot/index-with-statistics-success" \
              expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack verify --algorithm less-memory --statistics "$MULTI_PACK_INDEX"
            }
          )
        )
        (with "decode"
          it "verifies the pack index successfully and with desired output, and decodes all objects" && {
            WITH_SNAPSHOT="$snapshot/index-success" \
            expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose  free pack verify --algorithm less-memory --decode "$MULTI_PACK_INDEX"
          }
        )
        (with "re-encode"
          it "verifies the pack index successfully and with desired output, and re-encodes all objects" && {
            WITH_SNAPSHOT="$snapshot/index-success" \
            expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack verify --algorithm less-time --re-encode "$MULTI_PACK_INDEX"
          }
        )
        if test "$kind" = "max" || test "$kind" = "max-pure"; then
        (with "statistics (JSON)"
          it "verifies the pack index successfully and with desired output" && {
            WITH_SNAPSHOT="$snapshot/index-with-statistics-json-success" \
            expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose --format json --threads 1 free pack verify --statistics "$MULTI_PACK_INDEX"
          }
        )
        fi
      )
      (with "a valid multi-pack index"
        snapshot="$snapshot/multi-index"
        (sandbox
          MULTI_PACK_INDEX=multi-pack-index
          cp $fixtures/packs/pack-* .
          $exe_plumbing free pack multi-index -i $MULTI_PACK_INDEX create *.idx

          (when "using fast validation via 'pack multi-index verify'"
            it "verifies the pack index successfully and with desired output" && {
              WITH_SNAPSHOT="$snapshot/fast-index-success" \
              expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack multi-index -i "$MULTI_PACK_INDEX" verify
            }
          )

          (with "no statistics"
            it "verifies the pack index successfully and with desired output" && {
              WITH_SNAPSHOT="$snapshot/index-success" \
              expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack verify "$MULTI_PACK_INDEX"
            }
          )
          (with "statistics"
            it "verifies the pack index successfully and with desired output" && {
              WITH_SNAPSHOT="$snapshot/index-with-statistics-success" \
              expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack verify --statistics "$MULTI_PACK_INDEX"
            }

            (with "and the less-memory algorithm"
              it "verifies the pack index successfully and with desired output" && {
                WITH_SNAPSHOT="$snapshot/index-with-statistics-success" \
                expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack verify --algorithm less-memory --statistics "$MULTI_PACK_INDEX"
              }
            )
          )
          (with "decode"
            it "verifies the pack index successfully and with desired output, and decodes all objects" && {
              WITH_SNAPSHOT="$snapshot/index-success" \
              expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack verify --algorithm less-memory --decode "$MULTI_PACK_INDEX"
            }
          )
          (with "re-encode"
            it "verifies the pack index successfully and with desired output, and re-encodes all objects" && {
              WITH_SNAPSHOT="$snapshot/index-success" \
              expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose free pack verify --algorithm less-time --re-encode "$MULTI_PACK_INDEX"
            }
          )
          if test "$kind" = "max" || test "$kind" = "max-pure"; then
          (with "statistics (JSON)"
            it "verifies the pack index successfully and with desired output" && {
              WITH_SNAPSHOT="$snapshot/index-with-statistics-json-success" \
              expect_run $SUCCESSFULLY "$exe_plumbing" --no-verbose --format json --threads 1 free pack verify --statistics "$MULTI_PACK_INDEX"
            }
          )
          fi
        )
      )
      (sandbox
        (with "an INvalid pack INDEX file"
          MULTI_PACK_INDEX="$fixtures/packs/pack-11fdfa9e156ab73caae3b6da867192221f2089c2.idx"
          cp $MULTI_PACK_INDEX index.idx
          echo $'\0' >> index.idx
          it "fails to verify the pack index and with desired output" && {
            WITH_SNAPSHOT="$snapshot/index-failure" \
            expect_run $WITH_FAILURE "$exe_plumbing" --no-verbose free pack verify index.idx
          }
        )
      )
    )
  )
)
