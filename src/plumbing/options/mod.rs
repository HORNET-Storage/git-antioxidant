use std::path::PathBuf;

use clap_complete::Shell;
use gitoxide_core as core;
use gix::bstr::BString;

use crate::shared::{AsRange, AsTime};

#[derive(Debug, clap::Parser)]
#[clap(name = "gix", about = "The git underworld", version = option_env!("GIX_VERSION"))]
#[clap(subcommand_required = true)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// The repository to access.
    #[clap(short = 'r', long, default_value = ".")]
    pub repository: PathBuf,

    /// Add these values to the configuration in the form of `key=value` or `key`.
    ///
    /// For example, if `key` is `core.abbrev`, set configuration like `[core] abbrev = key`,
    /// or `remote.origin.url = foo` to set `[remote "origin"] url = foo`.
    #[clap(long, short = 'c', value_parser = crate::shared::AsBString)]
    pub config: Vec<BString>,

    /// The amount of threads to use for some operations.
    ///
    /// If unset, or the value is 0, there is no limit and all logical cores can be used.
    #[clap(long)]
    pub threads: Option<usize>,

    /// Display verbose messages and progress information
    #[clap(long, short = 'v')]
    pub verbose: bool,

    /// Display tracing output; repeat for more detail and a flat format.
    ///
    /// Once is forest-formatted at `info`, twice is forest-formatted at `debug`,
    /// three times is flat at `debug`, and four times is flat at `trace`.
    /// Output is buffered independently and written to stderr when the command finishes.
    /// Colors follow stderr's terminal capabilities and environment configuration.
    #[clap(
        long,
        short = 't',
        action = clap::ArgAction::Count,
        value_parser = clap::value_parser!(u8).range(0..=4)
    )]
    #[cfg(feature = "tracing")]
    pub trace: u8,

    /// Turn off verbose message display for commands where these are shown by default.
    #[clap(long, conflicts_with("verbose"))]
    pub no_verbose: bool,

    /// Don't default malformed configuration flags, but show an error instead. Ignore IO errors as well.
    ///
    /// Note that some subcommands use strict mode by default.
    // TODO: needs a 'lenient' mutually exclusive counterpart. Opens the gate to auto-verbose some commands, and add --no-verbose
    //       for these.
    #[clap(long, short = 's')]
    pub strict: bool,

    /// Determine the format to use when outputting statistics.
    #[clap(
        long,
        short = 'f',
        default_value = "human",
        value_parser = crate::shared::AsOutputFormat
    )]
    pub format: core::OutputFormat,

    /// The object format to assume when reading files that don't inherently know about it, or when writing files.
    #[clap(long, default_value_t = gix::hash::Kind::default(), value_parser = crate::shared::AsHashKind)]
    pub object_hash: gix::hash::Kind,

    #[clap(subcommand)]
    pub cmd: Subcommands,
}

#[derive(Debug, clap::Subcommand)]
pub enum Subcommands {
    /// Subcommands for creating worktree archives.
    #[cfg(feature = "gitoxide-core-tools-archive")]
    Archive(archive::Platform),
    /// Interact with branches.
    #[clap(visible_alias = "branches")]
    Branch(branch::Platform),
    /// Remove untracked files from the working tree.
    #[cfg(feature = "gitoxide-core-tools-clean")]
    Clean(clean::Command),
    /// Subcommands for interacting with commit-graph files.
    #[clap(subcommand)]
    CommitGraph(commitgraph::Subcommands),
    /// Interact with the object database.
    #[clap(subcommand)]
    Odb(odb::Subcommands),
    /// Check for missing objects.
    Fsck(fsck::Platform),
    /// Interact with tree objects.
    #[clap(subcommand)]
    Tree(tree::Subcommands),
    /// Commit the index like `git commit` does without an editor, or interact with commit objects.
    Commit(commit::Platform),
    /// Interact with tag objects.
    #[clap(visible_alias = "tags")]
    Tag(tag::Platform),
    /// Reset `HEAD`, the index and the worktree to a commit, or index entries to a tree, like `git reset`.
    Reset(reset::Platform),
    /// Rebase a branch onto another commit like `git rebase` does with its merge backend.
    Rebase(rebase::Platform),
    /// Verify the integrity of the entire repository
    Verify {
        #[clap(flatten)]
        args: free::pack::VerifyOptions,
    },
    /// Query and obtain information about revisions.
    #[clap(subcommand)]
    Revision(revision::Subcommands),
    /// A program just like `git credential`.
    #[clap(subcommand)]
    Credential(credential::Subcommands),
    /// Fetch data from remotes and store it in the repository.
    #[cfg(feature = "gitoxide-core-blocking-client")]
    Fetch(fetch::Platform),
    /// Clone a repository into a new directory.
    #[cfg(feature = "gitoxide-core-blocking-client")]
    Clone(clone::Platform),
    /// Update references of a remote along with the objects they need, and report like `git push --porcelain`.
    #[cfg(feature = "gitoxide-core-blocking-client")]
    Push(push::Platform),
    /// Interact with the mailmap.
    #[clap(subcommand)]
    Mailmap(mailmap::Subcommands),
    /// Interact with the remote hosts.
    #[cfg(any(feature = "gitoxide-core-async-client", feature = "gitoxide-core-blocking-client"))]
    Remote(remote::Platform),
    /// Interact with the attribute files like .gitattributes.
    #[clap(subcommand, visible_alias = "attrs")]
    Attributes(attributes::Subcommands),
    /// Interact with the exclude files like .gitignore.
    #[clap(subcommand)]
    Exclude(exclude::Subcommands),
    /// Interact with a worktree index like .git/index.
    #[clap(subcommand)]
    Index(index::Subcommands),
    /// Interact with submodules.
    #[clap(alias = "submodules")]
    Submodule(submodule::Platform),
    /// Show whatever object is at the given spec.
    Cat {
        /// The object to print to stdout.
        revspec: String,
    },
    /// Check for changes in the repository, treating this as an error.
    IsClean,
    /// Check for changes in the repository, treating their absence as an error.
    IsChanged,
    /// Show which git configuration values are used or planned.
    ConfigTree,
    Status(status::Platform),
    Dirwalk(dirwalk::Platform),
    Config(config::Platform),
    #[cfg(feature = "gitoxide-core-tools-corpus")]
    Corpus(corpus::Platform),
    MergeBase(merge_base::Command),
    Merge(merge::Platform),
    /// Print paths relevant to the Git installation.
    Env,
    /// Open paths in the editor selected by Git's precedence rules.
    Editor {
        /// Paths to open, or none to start the editor without a file.
        paths: Vec<PathBuf>,
    },
    Diff(diff::Platform),
    Log(log::Platform),
    /// Interactively browse commits and their graph.
    #[cfg(feature = "tix")]
    #[clap(visible_alias = "tui", visible_alias = "interactive", visible_alias = "i")]
    Tix(gix_tix::command::Platform),
    Worktree(worktree::Platform),
    /// Subcommands that need no Git repository to run.
    #[clap(subcommand)]
    Free(free::Subcommands),
    /// Blame lines in a file.
    Blame {
        /// Print additional statistics to help understanding performance.
        #[clap(long, short = 's')]
        statistics: bool,
        /// The file to create the blame information for.
        file: std::ffi::OsString,
        /// Only blame lines in the given 1-based inclusive range '<start>,<end>', e.g. '20,40'.
        #[clap(short='L', value_parser=AsRange, action=clap::ArgAction::Append)]
        ranges: Vec<std::ops::RangeInclusive<u32>>,
        /// Don't consider commits before the given date.
        #[clap(long,  value_parser=AsTime, value_name = "DATE")]
        since: Option<gix::date::Time>,
    },
    /// Generate shell completions to stdout or a directory.
    #[clap(visible_alias = "generate-completions", visible_alias = "shell-completions")]
    Completions {
        /// The shell to generate completions for. Otherwise it's derived from the environment.
        #[clap(long, short)]
        shell: Option<Shell>,
        /// The output directory in case multiple files are generated. If not provided, will write to stdout.
        out_dir: Option<String>,
    },
}

#[cfg(feature = "gitoxide-core-tools-archive")]
pub mod archive {
    use std::path::PathBuf;

    #[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
    pub enum Format {
        /// An internal format that is for debugging, it should not be persisted and cannot be read back.
        ///
        /// However, it represents that bare data stream without with minimal overhead, and is a good
        /// metric for throughput.
        Internal,
        /// Use the `.tar` file format, uncompressed.
        Tar,
        /// Use the `.tar.gz` file format, compressed with `gzip`.
        TarGz,
        /// Use the `.zip` container format.
        Zip,
    }

    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        /// Explicitly set the format. Otherwise derived from the suffix of the output file.
        #[clap(long, short = 'f', value_enum)]
        pub format: Option<Format>,
        /// Apply the prefix verbatim to any path we add to the archive. Use a trailing `/` if prefix is a directory.
        #[clap(long)]
        pub prefix: Option<String>,
        /// The compression strength to use for `.zip` and `.tar.gz` archives, valid from 0-9.
        #[clap(long, short = 'l', requires = "format")]
        pub compression_level: Option<u8>,
        /// Add the given path to the archive. Directories will always be empty.
        #[clap(long, short = 'p')]
        pub add_path: Vec<PathBuf>,
        /// Add the new file from a slash-separated path, which must happen in pairs of two, first the path, then the content.
        #[clap(long, short = 'v')]
        pub add_virtual_file: Vec<String>,
        /// The file to write the archive to.
        ///
        /// It's extension determines the archive format, unless `--format` is set.
        pub output_file: PathBuf,

        /// The revspec of the commit or tree to traverse, or the tree at `HEAD` if unspecified.
        ///
        /// If commit, the commit timestamp will be used as timestamp for each file in the archive.
        pub treeish: Option<String>,
    }
}

pub mod branch {
    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        #[clap(subcommand)]
        pub cmd: Subcommands,
    }

    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// List branches.
        List {
            /// List remote-tracking as well as local branches.
            #[clap(long, short = 'a')]
            all: bool,
        },
    }
}

pub mod status {
    use gix::bstr::BString;

    use crate::shared::{CheckPathSpec, ParseRenameFraction};

    #[derive(Default, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
    pub enum Submodules {
        /// display all information about submodules, including ref changes, modifications and untracked files.
        #[default]
        All,
        /// Compare only the configuration of the superprojects commit with the actually checked out `HEAD` commit.
        RefChange,
        /// See if there are worktree modifications compared to the index, but do not check for untracked files.
        Modifications,
        /// Ignore all submodule changes.
        None,
    }

    #[derive(Default, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
    pub enum Ignored {
        /// display all ignored files and directories, but collapse them if possible to simplify.
        #[default]
        Collapsed,
        /// Show exact matches. Note that this may show directories if these are a match as well.
        ///
        /// Simplification will not happen in this mode.
        Matching,
        // TODO: figure out how to implement traditional, which right now can't be done as it requires ignored folders
        //       to be fully expanded. This should probably be implemented in `gix_dir` which then simply works by not
        //       allowing to ignore directories, naturally traversing the entire content.
    }

    #[derive(Default, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
    pub enum Untracked {
        /// Do not show untracked files.
        No,
        /// Collapse untracked directories when possible.
        Normal,
        /// Show individual untracked files.
        #[default]
        All,
    }

    #[derive(Default, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
    pub enum Format {
        /// A basic format that is easy to read, and useful for a first glimpse as flat list.
        #[default]
        Simplified,
        /// Output very similar to `git status --porcelain=2`.
        PorcelainV2,
    }

    #[derive(Debug, clap::Parser)]
    #[command(about = "Compute repository status similar to `git status`")]
    pub struct Platform {
        /// The way status data is displayed.
        #[clap(long, short = 'f')]
        pub format: Option<Format>,
        /// If enabled, show ignored files and directories.
        #[clap(long)]
        pub ignored: Option<Option<Ignored>>,
        /// Define how to show untracked files. If set without a value, show all individual untracked files.
        #[clap(long, short = 'u', require_equals = true)]
        pub untracked: Option<Option<Untracked>>,
        /// Define how to display the submodule status. Defaults to git configuration if unset.
        #[clap(long)]
        pub submodules: Option<Submodules>,
        /// Print additional statistics to help understanding performance.
        #[clap(long, short = 's')]
        pub statistics: bool,
        /// Don't write back a changed index, which forces this operation to always be idempotent.
        #[clap(long)]
        pub no_write: bool,
        /// Enable rename tracking between the index and the working tree, preventing the collapse of folders as well.
        #[clap(long, value_parser = ParseRenameFraction)]
        pub index_worktree_renames: Option<Option<f32>>,
        /// The git path specifications to list attributes for, or unset to read from stdin one per line.
        #[clap(value_parser = CheckPathSpec)]
        pub pathspec: Vec<BString>,
    }
}

pub mod dirwalk {
    use gix::bstr::BString;

    use crate::shared::CheckPathSpec;

    #[derive(Default, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
    pub enum Untracked {
        /// Collapse untracked directories when possible.
        #[default]
        Collapsed,
        /// Emit matching untracked files and directories.
        Matching,
    }

    #[derive(Debug, clap::Parser)]
    #[command(about = "Run only the repository directory walk")]
    pub struct Platform {
        /// Print additional statistics to help understanding performance.
        #[clap(long, short = 's')]
        pub statistics: bool,
        /// How untracked files should be emitted.
        #[clap(long, default_value = "collapsed")]
        pub untracked: Untracked,
        /// The git path specifications to walk.
        #[clap(value_parser = CheckPathSpec)]
        pub pathspec: Vec<BString>,
    }
}

pub mod merge_base {
    #[derive(Debug, clap::Parser)]
    #[command(about = "A command for calculating all merge-bases")]
    pub struct Command {
        /// A revspec for the first commit.
        pub first: String,
        /// Revspecs for the other commits to compute the merge-base with.
        pub others: Vec<String>,
    }
}

pub mod worktree {
    use std::path::PathBuf;

    use gix::bstr::BString;

    #[derive(Debug, clap::Parser)]
    #[command(about = "Commands for handling worktrees")]
    pub struct Platform {
        #[clap(subcommand)]
        pub cmd: SubCommands,
    }

    #[derive(Debug, clap::Subcommand)]
    pub enum SubCommands {
        /// List all worktrees, along with some accompanying information.
        List,
        /// Add and check out a linked worktree.
        ///
        /// Without COMMIT-ISH, reuse the local branch named after PATH or create it from HEAD.
        /// An explicit local branch is attached; other revisions produce a detached HEAD.
        /// New branches are retained if subsequent worktree setup fails, like Git.
        #[clap(visible_alias = "create")]
        Add {
            /// Create a new branch at COMMIT-ISH, or HEAD if it is omitted.
            #[clap(short = 'b', long, value_name = "NEW-BRANCH", value_parser = crate::shared::AsBString, conflicts_with = "detach")]
            branch: Option<BString>,
            /// Detach HEAD instead of attaching to or creating a branch.
            #[clap(short = 'd', long)]
            detach: bool,
            /// Where to create the checkout, relative to the current directory.
            path: PathBuf,
            /// An existing local branch or a revision that resolves to a commit.
            #[clap(value_name = "COMMIT-ISH", value_parser = crate::shared::AsBString)]
            commit_ish: Option<BString>,
        },
        /// Remove a linked worktree and its private Git directory, keeping its branch.
        Remove {
            /// Discard changes and initialized submodules; repeat to override a worktree lock.
            #[clap(short, long, action = clap::ArgAction::Count)]
            force: u8,
            /// The worktree path or a unique path suffix.
            worktree: PathBuf,
        },
    }
}

#[cfg(feature = "gitoxide-core-tools-corpus")]
pub mod corpus {
    use std::path::PathBuf;

    #[derive(Debug, clap::Parser)]
    #[command(about = "Run algorithms on a corpus of git repositories and store their results for later analysis")]
    pub struct Platform {
        /// The path to the database to read and write depending on the sub-command.
        #[arg(long, default_value = "corpus.db")]
        pub db: PathBuf,
        /// The path to the root of the corpus to search repositories in.
        #[arg(long, short = 'p', default_value = ".")]
        pub path: PathBuf,
        #[clap(subcommand)]
        pub cmd: SubCommands,
    }

    #[derive(Debug, clap::Subcommand)]
    pub enum SubCommands {
        /// Perform a corpus run on all registered repositories.
        Run {
            /// Don't run any task, but print all repos that would be traversed once.
            ///
            /// Note that this will refresh repositories if necessary and store them in the database, it just won't run tasks.
            #[clap(long, short = 'n')]
            dry_run: bool,

            /// The SQL that will be appended to the actual select statement for repositories to apply additional filtering, like `LIMIT 10`.
            ///
            /// The string must be trusted even though the engine will only execute a single statement.
            #[clap(long, short = 'r')]
            repo_sql_suffix: Option<String>,

            /// The short_names of the tasks to include when running.
            #[clap(long, short = 't')]
            include_task: Vec<String>,
        },
        /// Re-read all repositories under the corpus directory, and add or update them.
        Refresh,
    }
}

pub mod merge {
    use std::path::PathBuf;

    use gitoxide_core::repository::merge::into_head::MessagePart;
    use gix::bstr::BString;

    #[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
    pub enum ResolveWith {
        /// Use ours then theirs in case of conflict.
        Union,
        /// Use only ours in case of conflict.
        Ours,
        /// Use only theirs in case of conflict.
        Theirs,
    }

    impl From<ResolveWith> for gix::merge::blob::builtin_driver::text::Conflict {
        fn from(value: ResolveWith) -> Self {
            match value {
                ResolveWith::Union => gix::merge::blob::builtin_driver::text::Conflict::ResolveWithUnion,
                ResolveWith::Ours => gix::merge::blob::builtin_driver::text::Conflict::ResolveWithOurs,
                ResolveWith::Theirs => gix::merge::blob::builtin_driver::text::Conflict::ResolveWithTheirs,
            }
        }
    }

    #[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
    pub enum FileFavor {
        /// Use only ours in case of conflict.
        Ours,
        /// Use only theirs in case of conflict.
        Theirs,
    }

    #[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
    pub enum TreeFavor {
        /// Use only the previous tree entry in case of conflict.
        Ancestor,
        /// Use only ours tree entry in case of conflict.
        Ours,
    }

    impl From<FileFavor> for gix::merge::tree::FileFavor {
        fn from(value: FileFavor) -> Self {
            match value {
                FileFavor::Ours => gix::merge::tree::FileFavor::Ours,
                FileFavor::Theirs => gix::merge::tree::FileFavor::Theirs,
            }
        }
    }

    impl From<TreeFavor> for gix::merge::tree::TreeFavor {
        fn from(value: TreeFavor) -> Self {
            match value {
                TreeFavor::Ancestor => gix::merge::tree::TreeFavor::Ancestor,
                TreeFavor::Ours => gix::merge::tree::TreeFavor::Ours,
            }
        }
    }

    #[derive(Debug, clap::Parser)]
    pub struct SharedOptions {
        /// Keep all objects to be written in memory to avoid any disk IO.
        ///
        /// Note that the resulting tree won't be available or inspectable.
        #[clap(long, short = 'm')]
        pub in_memory: bool,
        /// Decide how to resolve content conflicts in files. If unset, write conflict markers and fail.
        #[clap(long, short = 'f')]
        pub file_favor: Option<FileFavor>,
        /// Decide how to resolve conflicts in trees, i.e. modification/deletion. If unset, try to preserve both states and fail.
        #[clap(long, short = 't')]
        pub tree_favor: Option<TreeFavor>,
        /// Print additional information about conflicts for debugging.
        #[clap(long, short = 'd')]
        pub debug: bool,
    }

    #[derive(Debug, clap::Parser)]
    #[command(about = "Merge a commit into `HEAD` like `git merge` does, or perform merges of various kinds")]
    #[clap(args_conflicts_with_subcommands = true)]
    pub struct Platform {
        /// Fast-forward if `HEAD` is an ancestor of the commit, and make a merge commit otherwise.
        ///
        /// This is the default unless `merge.ff` says otherwise.
        #[clap(long, overrides_with_all = ["no_ff", "ff_only"])]
        pub ff: bool,

        /// Make a merge commit even if `HEAD` could be fast-forwarded.
        #[clap(long, overrides_with_all = ["ff", "ff_only"])]
        pub no_ff: bool,

        /// Fast-forward if `HEAD` is an ancestor of the commit, and refuse to merge otherwise.
        #[clap(long, overrides_with_all = ["ff", "no_ff"])]
        pub ff_only: bool,

        /// Change the index and the worktree as the merge does, but neither make a commit nor move `HEAD`.
        #[clap(long)]
        pub squash: bool,

        /// Stop before the merge commit, which `--continue` makes then.
        #[clap(long)]
        pub no_commit: bool,

        #[clap(flatten)]
        pub message: Message,

        /// How to clean up the message: `strip`, `whitespace`, `verbatim`, `scissors` or `default`.
        ///
        /// If unset, `commit.cleanup` decides.
        #[clap(long, value_name = "MODE")]
        pub cleanup: Option<String>,

        /// Don't run the `pre-merge-commit` and `commit-msg` hooks.
        #[clap(long)]
        pub no_verify: bool,

        /// Merge a commit that shares no history with `HEAD`.
        #[clap(long)]
        pub allow_unrelated_histories: bool,

        /// Conclude the merge in progress with its commit once its conflicts are resolved.
        #[clap(long = "continue")]
        pub continue_merge: bool,

        /// Abort the merge in progress and go back to the state before it.
        #[clap(long)]
        pub abort: bool,

        /// Forget the merge in progress, and keep the index and the worktree as they are.
        #[clap(long)]
        pub quit: bool,

        /// The commit to merge into `HEAD`, or the upstream of the current branch if unset. A commit whose name is that of
        /// a subcommand is merged if `--` comes before it, as in `gix merge -- tree`.
        #[clap(value_name = "COMMIT", value_parser = crate::shared::AsBString)]
        pub commit: Option<BString>,

        #[clap(subcommand)]
        pub cmd: Option<SubCommands>,
    }

    /// The parts of the message of a merge, as `-m` and `-F` give them, in the order of the command line.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct Message(pub Vec<MessagePart>);

    impl clap::FromArgMatches for Message {
        fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
            let mut parts = Vec::new();
            if let (Some(indices), Some(paragraphs)) =
                (matches.indices_of("message"), matches.get_many::<BString>("message"))
            {
                parts.extend(indices.zip(paragraphs.cloned().map(MessagePart::Paragraph)));
            }
            if let (Some(indices), Some(files)) = (matches.indices_of("file"), matches.get_many::<PathBuf>("file")) {
                parts.extend(indices.zip(files.cloned().map(MessagePart::File)));
            }
            parts.sort_by_key(|(index, _)| *index);
            Ok(Message(parts.into_iter().map(|(_, part)| part).collect()))
        }

        fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
            *self = Self::from_arg_matches(matches)?;
            Ok(())
        }
    }

    impl clap::Args for Message {
        fn augment_args(command: clap::Command) -> clap::Command {
            command
                .arg(
                    clap::Arg::new("message")
                        .long("message")
                        .short('m')
                        .value_name("MESSAGE")
                        .value_parser(crate::shared::AsBString)
                        .action(clap::ArgAction::Append)
                        .help("Use the given message as the message of the merge commit")
                        .long_help(
                            "Use the given message as the message of the merge commit.\n\nIf given more than once, the messages are concatenated as separate paragraphs, and with `--file`, in the order of the command line.",
                        ),
                )
                .arg(
                    clap::Arg::new("file")
                        .long("file")
                        .short('F')
                        .value_name("FILE")
                        .value_parser(clap::value_parser!(PathBuf))
                        .action(clap::ArgAction::Append)
                        .help("Read the message of the merge commit from the given file")
                        .long_help(
                            "Read the message of the merge commit from the given file.\n\nWith `--message` or more than once, the contents follow what comes before them on the command line on a line of their own.",
                        ),
                )
        }

        fn augment_args_for_update(command: clap::Command) -> clap::Command {
            Self::augment_args(command)
        }
    }

    #[derive(Debug, clap::Subcommand)]
    pub enum SubCommands {
        /// Merge a file by specifying ours, base and theirs.
        File {
            /// Decide how to resolve conflicts. If unset, write conflict markers and fail.
            #[clap(long, short = 'c')]
            resolve_with: Option<ResolveWith>,

            /// A path or revspec to our file.
            #[clap(value_name = "OURS", value_parser = crate::shared::AsBString)]
            ours: BString,
            /// A path or revspec to the base for both ours and theirs.
            #[clap(value_name = "BASE", value_parser = crate::shared::AsBString)]
            base: BString,
            /// A path or revspec to their file.
            #[clap(value_name = "THEIRS", value_parser = crate::shared::AsBString)]
            theirs: BString,
        },

        /// Merge a tree by specifying ours, base and theirs, writing it to the object database.
        Tree {
            #[clap(flatten)]
            opts: SharedOptions,

            /// Create a commit on top of HEAD with this message and the merged tree.
            #[clap(long)]
            message: Option<String>,
            /// Update HEAD to the created commit and set the index to its tree.
            #[clap(long, requires = "message", conflicts_with = "in_memory")]
            update_head: bool,

            /// A revspec to our treeish.
            #[clap(value_name = "OURS", value_parser = crate::shared::AsBString)]
            ours: BString,
            /// A revspec to the base as treeish for both ours and theirs.
            #[clap(value_name = "BASE", value_parser = crate::shared::AsBString)]
            base: BString,
            /// A revspec to their treeish.
            #[clap(value_name = "THEIRS", value_parser = crate::shared::AsBString)]
            theirs: BString,
        },
        /// Merge a commits by specifying ours, and theirs, writing the tree to the object database.
        Commit {
            #[clap(flatten)]
            opts: SharedOptions,

            /// A revspec to our committish.
            #[clap(value_name = "OURS", value_parser = crate::shared::AsBString)]
            ours: BString,
            /// A revspec to their committish.
            #[clap(value_name = "THEIRS", value_parser = crate::shared::AsBString)]
            theirs: BString,
        },
    }
}

pub mod diff {
    use gix::bstr::BString;

    /// Print all changes between two objects.
    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        #[clap(subcommand)]
        pub cmd: SubCommands,
    }

    #[derive(Debug, clap::Subcommand)]
    pub enum SubCommands {
        /// Diff two trees.
        Tree {
            /// A rev-spec representing the 'before' or old tree.
            #[clap(value_parser = crate::shared::AsBString)]
            old_treeish: BString,
            /// A rev-spec representing the 'after' or new tree.
            #[clap(value_parser = crate::shared::AsBString)]
            new_treeish: BString,
        },
        /// Diff two versions of a file.
        File {
            /// A rev-spec representing the 'before' or old state of the file, like '@~100:file'
            #[clap(value_parser = crate::shared::AsBString)]
            old_revspec: BString,
            /// A rev-spec representing the 'after' or new state of the file, like ':file'
            #[clap(value_parser = crate::shared::AsBString)]
            new_revspec: BString,
        },
    }
}

pub mod log {
    use gix::bstr::BString;

    #[derive(Debug, Copy, Clone, clap::ValueEnum)]
    pub enum Sorting {
        /// Visit commits breadth-first, without guaranteeing children precede parents.
        BreadthFirst,
        /// Prioritize the newest queued commits, without guaranteeing children precede parents.
        NewestFirst,
        /// Prioritize the oldest queued commits, starting with the tips rather than the oldest ancestors.
        OldestFirst,
        /// Show children before parents without intermixing branches.
        TopoOrder,
        /// Show children before parents, otherwise prioritizing newer commit dates.
        DateOrder,
    }

    impl From<Sorting> for gix::revision::walk::Sorting {
        fn from(value: Sorting) -> Self {
            use gix::revision::walk::CommitTimeOrder;

            match value {
                Sorting::BreadthFirst => Self::BreadthFirst,
                Sorting::NewestFirst => Self::ByCommitTime(CommitTimeOrder::NewestFirst),
                Sorting::OldestFirst => Self::ByCommitTime(CommitTimeOrder::OldestFirst),
                Sorting::TopoOrder => Self::TopoOrder,
                Sorting::DateOrder => Self::DateOrder,
            }
        }
    }

    /// List reachable commits in the chosen order, optionally excluding hidden ancestry.
    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        /// The order in which to return commits.
        #[clap(long, value_enum, default_value = "date-order")]
        pub sort: Sorting,
        /// Follow only the first parent of visible commits; hidden ancestry follows all parents.
        #[clap(long)]
        pub first_parent: bool,
        /// Revision expressions such as HEAD, main..topic, main...topic, or ^main. Defaults to HEAD if omitted.
        #[clap(value_name = "REVISION", value_parser = crate::shared::AsBString)]
        pub revisions: Vec<BString>,
        /// Git pathspecs limiting the history, separated from revisions by -- (currently unsupported).
        #[clap(last = true, value_name = "PATHSPEC", value_parser = crate::shared::AsBString)]
        pub pathspecs: Vec<BString>,
    }
}

pub mod config {
    use std::path::PathBuf;

    use gix::bstr::BString;

    /// Print all entries in a configuration file or access other sub-commands.
    #[derive(Debug, clap::Parser)]
    #[clap(subcommand_required(false))]
    pub struct Platform {
        /// The filter terms to limit the output to matching sections and subsections only.
        ///
        /// Typical filters are `branch` or `remote.origin` or `remote.or*` - git-style globs are supported
        /// and comparisons are case-insensitive.
        #[clap(value_parser = crate::shared::AsBString)]
        pub filter: Vec<BString>,

        /// Subcommands for working with configuration files.
        #[clap(subcommand)]
        pub cmd: Option<Subcommands>,
    }

    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// Show all resolved configuration entries, optionally filtered by section or subsection.
        Show,
        /// List all configuration files contributing to the resolved configuration.
        ///
        /// Included files are shown as well, along with the Source they inherit and their inclusion level.
        List,
        /// Format a git configuration file, normalizing insignificant whitespace.
        ///
        /// Includes are never resolved; only whitespace, newlines and the `=` separator are rewritten.
        Fmt {
            /// Write the formatted result back to the input file instead of to standard output.
            #[clap(long)]
            in_place: bool,
            /// The configuration file to format. If unset, the repository-local configuration is used.
            in_file: Option<PathBuf>,
            /// Where to write the formatted result. If unset, it is written to standard output.
            out_file: Option<PathBuf>,
        },
    }
}

#[cfg(feature = "gitoxide-core-blocking-client")]
pub mod fetch {
    use std::num::NonZeroU32;

    use gix::remote::fetch::Shallow;

    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        /// Don't change the local repository, but otherwise try to be as accurate as possible.
        #[clap(long, short = 'n')]
        pub dry_run: bool,

        /// Output additional typically information provided by the server as part of the connection handshake.
        #[clap(long, short = 'H')]
        pub handshake_info: bool,

        /// Print statistics about negotiation phase.
        #[clap(long, short = 's')]
        pub negotiation_info: bool,

        /// Open the commit graph used for negotiation and write an SVG file to PATH.
        #[clap(long, value_name = "PATH", short = 'g')]
        pub open_negotiation_graph: Option<std::path::PathBuf>,

        #[clap(flatten)]
        pub shallow: ShallowOptions,

        /// The name of the remote to connect to, or the url of the remote to connect to directly.
        ///
        /// If unset, the current branch will determine the remote.
        #[clap(long, short = 'r')]
        pub remote: Option<String>,

        /// Override the built-in and configured ref-specs with one or more of the given ones.
        #[clap(value_parser = crate::shared::AsBString)]
        pub ref_spec: Vec<gix::bstr::BString>,
    }

    #[derive(Debug, clap::Parser)]
    pub struct ShallowOptions {
        /// Fetch with the history truncated to the given number of commits as seen from the remote.
        #[clap(long, help_heading = Some("SHALLOW"), conflicts_with_all = ["shallow_since", "shallow_exclude", "deepen", "unshallow"])]
        pub depth: Option<NonZeroU32>,

        /// Extend the current shallow boundary by the given number of commits, with 0 meaning no change.
        #[clap(long, help_heading = Some("SHALLOW"), value_name = "DEPTH", conflicts_with_all = ["depth", "shallow_since", "shallow_exclude", "unshallow"])]
        pub deepen: Option<u32>,

        /// Cutoff all history past the given date. Can be combined with shallow-exclude.
        #[clap(long, help_heading = Some("SHALLOW"), value_parser = crate::shared::AsTime, value_name = "DATE", conflicts_with_all = ["depth", "deepen", "unshallow"])]
        pub shallow_since: Option<gix::date::Time>,

        /// Cutoff all history past the tag-name or ref-name. Can be combined with shallow-since.
        #[clap(long, help_heading = Some("SHALLOW"), value_parser = crate::shared::AsPartialRefName, value_name = "REF_NAME", conflicts_with_all = ["depth", "deepen", "unshallow"])]
        pub shallow_exclude: Vec<gix::refs::PartialName>,

        /// Remove the shallow boundary and fetch the entire history available on the remote.
        #[clap(long, help_heading = Some("SHALLOW"), conflicts_with_all = ["shallow_since", "shallow_exclude", "depth", "deepen"])]
        pub unshallow: bool,
    }

    impl From<ShallowOptions> for Shallow {
        fn from(opts: ShallowOptions) -> Self {
            if let Some(depth) = opts.depth {
                Shallow::DepthAtRemote(depth)
            } else if !opts.shallow_exclude.is_empty() {
                Shallow::Exclude {
                    remote_refs: opts.shallow_exclude,
                    since_cutoff: opts.shallow_since,
                }
            } else if let Some(cutoff) = opts.shallow_since {
                Shallow::Since { cutoff }
            } else if let Some(depth) = opts.deepen {
                Shallow::Deepen(depth)
            } else if opts.unshallow {
                Shallow::undo()
            } else {
                Shallow::default()
            }
        }
    }
}

#[cfg(feature = "gitoxide-core-blocking-client")]
pub mod push {
    use gix::bstr::BString;

    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        /// Update remote references even if the updates aren't fast-forwards or a lease doesn't hold.
        #[clap(long, short = 'f')]
        pub force: bool,

        /// Update remote references only if they point to what is expected: the object that `<expect>` names, no object
        /// if `<expect>` is empty, or what the remote-tracking reference of `<refname>` points to if it's left out.
        ///
        /// Without value, all remote references are expected to point to what their remote-tracking references point to.
        /// Can be given more than once.
        #[clap(
            long,
            value_name = "REFNAME[:EXPECT]",
            num_args = 0..=1,
            require_equals = true,
            default_missing_value = "",
            value_parser = crate::shared::AsBString
        )]
        pub force_with_lease: Vec<BString>,

        /// Update all remote references or none of them.
        #[clap(long)]
        pub atomic: bool,

        /// Push all tags along with what the refspecs push.
        #[clap(long)]
        pub tags: bool,

        /// Also push the annotated tags that point into what the remote has after the push.
        #[clap(long)]
        pub follow_tags: bool,

        /// Delete the remote references that the refspecs name.
        #[clap(long, short = 'd')]
        pub delete: bool,

        /// Do everything but update the remote.
        #[clap(long, short = 'n')]
        pub dry_run: bool,

        /// Don't run the `pre-push` hook.
        #[clap(long)]
        pub no_verify: bool,

        /// Send this string to the hooks of the remote. Can be given more than once.
        #[clap(long = "push-option", short = 'o', value_name = "OPTION", value_parser = crate::shared::AsBString)]
        pub push_options: Vec<BString>,

        /// Don't send deltas against objects that only the remote has.
        #[clap(long)]
        pub no_thin: bool,

        /// The name of the remote to push to, or its URL.
        ///
        /// If unset, the configuration of the current branch determines the remote.
        #[clap(value_parser = crate::shared::AsBString)]
        pub repository: Option<BString>,

        /// The refspecs to push, like `main`, `HEAD:refs/heads/topic` or `:refs/heads/old`.
        ///
        /// If unset, the push refspecs of the remote are used, or the one that `push.default` implies.
        #[clap(value_parser = crate::shared::AsBString)]
        pub refspecs: Vec<BString>,
    }
}

#[cfg(feature = "gitoxide-core-blocking-client")]
pub mod clone {
    use std::{ffi::OsString, num::NonZeroU32, path::PathBuf};

    use gix::remote::fetch::Shallow;

    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        /// Output additional typically information provided by the server as part of the connection handshake.
        #[clap(long, short = 'H')]
        pub handshake_info: bool,

        /// Clone branches into refs/heads without remote-tracking refs, upstream configuration, or a working tree.
        #[clap(long)]
        pub bare: bool,

        /// Do not clone any tags. Useful to reduce the size of the clone if only branches are needed.
        #[clap(long)]
        pub no_tags: bool,

        #[clap(flatten)]
        pub shallow: ShallowOptions,

        /// The url of the remote to connect to, like `https://github.com/byron/gitoxide`.
        pub remote: OsString,

        /// The name of the reference to check out.
        #[clap(
            long = "ref",
            value_parser = crate::shared::AsPartialRefName,
            value_name = "REF_NAME",
            conflicts_with = "revision"
        )]
        pub ref_name: Option<gix::refs::PartialName>,

        /// Fetch only this full reference or object ID and check it out with a detached HEAD.
        #[clap(long, value_parser = crate::shared::AsBString, value_name = "REVISION")]
        pub revision: Option<gix::bstr::BString>,

        /// The directory to initialize with the new repository and to which all data should be written.
        pub directory: Option<PathBuf>,
    }

    #[derive(Debug, clap::Parser)]
    pub struct ShallowOptions {
        /// Create a shallow clone with the history truncated to the given number of commits.
        #[clap(long, help_heading = Some("SHALLOW"), conflicts_with_all = ["shallow_since", "shallow_exclude"])]
        pub depth: Option<NonZeroU32>,

        /// Cutoff all history past the given date. Can be combined with shallow-exclude.
        #[clap(long, help_heading = Some("SHALLOW"), value_parser = crate::shared::AsTime, value_name = "DATE")]
        pub shallow_since: Option<gix::date::Time>,

        /// Cutoff all history past the tag-name or ref-name. Can be combined with shallow-since.
        #[clap(long, help_heading = Some("SHALLOW"), value_parser = crate::shared::AsPartialRefName, value_name = "REF_NAME")]
        pub shallow_exclude: Vec<gix::refs::PartialName>,
    }

    impl From<ShallowOptions> for Shallow {
        fn from(opts: ShallowOptions) -> Self {
            if let Some(depth) = opts.depth {
                Shallow::DepthAtRemote(depth)
            } else if !opts.shallow_exclude.is_empty() {
                Shallow::Exclude {
                    remote_refs: opts.shallow_exclude,
                    since_cutoff: opts.shallow_since,
                }
            } else if let Some(cutoff) = opts.shallow_since {
                Shallow::Since { cutoff }
            } else {
                Shallow::default()
            }
        }
    }
}

#[cfg(any(feature = "gitoxide-core-async-client", feature = "gitoxide-core-blocking-client"))]
pub mod remote {
    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        /// The name of the remote to connect to, or the URL of the remote to connect to directly.
        ///
        /// If unset, the current branch will determine the remote.
        #[clap(long, short = 'n')]
        pub name: Option<String>,

        /// Output additional typically information provided by the server as part of the connection handshake.
        #[clap(long, short = 'H')]
        pub handshake_info: bool,

        /// Subcommands
        #[clap(subcommand)]
        pub cmd: Subcommands,
    }

    #[derive(Debug, clap::Subcommand)]
    #[clap(visible_alias = "remotes")]
    pub enum Subcommands {
        /// Print the effective URL of the remote.
        Url {
            /// Print all effective URLs instead of only the first one.
            #[clap(long)]
            all: bool,
            /// Print push URLs instead of fetch URLs.
            #[clap(long)]
            push: bool,
        },
        /// Print all references available on the remote.
        Refs,
        /// Print all references available on the remote as filtered through ref-specs.
        RefMap {
            /// Also display remote references that were sent by the server, but filtered by the refspec locally.
            #[clap(long, short = 'u')]
            show_unmapped_remote_refs: bool,
            /// Override the built-in and configured ref-specs with one or more of the given ones.
            #[clap(value_parser = crate::shared::AsBString)]
            ref_spec: Vec<gix::bstr::BString>,
        },
    }
}

pub mod mailmap {
    use gix::bstr::BString;

    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// Print all entries in configured mailmaps, inform about errors as well.
        Entries,
        /// Print the canonical form of contacts according to the configured mailmaps.
        Check {
            /// One or more `Name <email>` or `<email>` to pass through the mailmap.
            contacts: Vec<BString>,
        },
    }
}

#[cfg(feature = "gitoxide-core-tools-clean")]
pub mod clean {
    use gix::bstr::BString;

    use crate::shared::CheckPathSpec;

    #[derive(Default, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
    pub enum FindRepository {
        All,
        #[default]
        NonBare,
    }

    impl From<FindRepository> for gitoxide_core::repository::clean::FindRepository {
        fn from(value: FindRepository) -> Self {
            match value {
                FindRepository::All => gitoxide_core::repository::clean::FindRepository::All,
                FindRepository::NonBare => gitoxide_core::repository::clean::FindRepository::NonBare,
            }
        }
    }

    #[derive(Debug, clap::Parser)]
    pub struct Command {
        /// Print additional debug information to help understand decisions it made.
        #[arg(long)]
        pub debug: bool,
        /// A dummy to easy with muscle-memory. This flag is assumed if provided or not, and has no effect.
        #[arg(short = 'n', long)]
        pub dry_run: bool,
        /// Actually perform the operation, which deletes files on disk without chance of recovery.
        #[arg(long, short = 'e')]
        pub execute: bool,
        /// Remove ignored (and expendable) files.
        #[arg(long, short = 'x')]
        pub ignored: bool,
        /// Remove precious files.
        #[arg(long, short = 'p')]
        pub precious: bool,
        /// Remove whole directories.
        #[arg(long, short = 'd')]
        pub directories: bool,
        /// Remove nested repositories, even outside ignored directories.
        #[arg(long, short = 'r')]
        pub repositories: bool,
        /// Pathspec patterns are used to match the result of the dirwalk, not the dirwalk itself.
        ///
        /// Use this if there is trouble using wildcard pathspecs, which affect the directory walk
        /// in reasonable, but often unexpected ways.
        #[arg(long, short = 'm')]
        pub pathspec_matches_result: bool,
        /// Enter ignored directories to skip repositories contained within.
        ///
        /// This identifies and avoids deleting separate repositories that are nested inside
        /// ignored directories eligible for removal.
        #[arg(long)]
        pub skip_hidden_repositories: Option<FindRepository>,
        /// What kind of repositories to find inside of untracked directories.
        #[arg(long, default_value = "non-bare")]
        pub find_untracked_repositories: FindRepository,
        /// The git path specifications to list attributes for, or unset to read from stdin one per line.
        #[clap(value_parser = CheckPathSpec)]
        pub pathspec: Vec<BString>,
    }
}

pub mod odb {
    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// Print all object names.
        Entries,
        /// Provide general information about the object database.
        Info,
        /// Count and obtain information on all, possibly duplicate, objects in the database.
        #[clap(visible_alias = "statistics")]
        Stats {
            /// Lookup headers again, but without preloading indices.
            #[clap(long)]
            extra_header_lookup: bool,
        },
    }
}

pub mod fsck {
    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        /// A revspec to start the connectivity check from.
        pub spec: Option<String>,
    }
}

pub mod tree {
    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// Print entries in a given tree
        Entries {
            /// Traverse the entire tree and its subtrees respectively, not only this tree.
            #[clap(long, short = 'r')]
            recursive: bool,

            /// Provide files size as well. This is expensive as the object is decoded entirely.
            #[clap(long, short = 'e')]
            extended: bool,

            /// The revspec of the tree to traverse, or the tree at `HEAD` if unspecified.
            treeish: Option<String>,
        },
        /// Provide information about a tree.
        Info {
            /// Provide files size as well. This is expensive as the object is decoded entirely.
            #[clap(long, short = 'e')]
            extended: bool,
            /// The revspec of the tree to traverse, or the tree at `HEAD` if unspecified.
            treeish: Option<String>,
        },
    }
}

pub mod commit {
    use gix::bstr::BString;

    #[derive(Debug, clap::Parser)]
    #[clap(args_conflicts_with_subcommands = true)]
    pub struct Platform {
        /// Use the given message as the commit message.
        ///
        /// If given more than once, the messages are concatenated as separate paragraphs.
        #[clap(long, short = 'm', value_name = "MESSAGE", value_parser = crate::shared::AsBString)]
        pub message: Vec<BString>,

        /// Take the commit message from the given file, or from standard input if it is `-`.
        #[clap(long, short = 'F', value_name = "FILE")]
        pub file: Option<std::path::PathBuf>,

        /// Replace the tip of the current branch with a new commit, which keeps its message unless one is given.
        #[clap(long)]
        pub amend: bool,

        /// Allow a commit with the same tree as its parent.
        #[clap(long)]
        pub allow_empty: bool,

        /// Allow a commit with an empty message.
        #[clap(long)]
        pub allow_empty_message: bool,

        /// Don't run the `pre-commit` and `commit-msg` hooks.
        #[clap(long, short = 'n')]
        pub no_verify: bool,

        /// How to clean up the commit message: `strip`, `whitespace`, `verbatim`, `scissors` or `default`.
        ///
        /// If unset, `commit.cleanup` decides.
        #[clap(long, value_name = "MODE")]
        pub cleanup: Option<String>,

        #[clap(subcommand)]
        pub cmds: Option<Subcommands>,
    }

    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// Verify the signature of a commit.
        Verify {
            /// A specification of the revision to verify, or the current `HEAD` if unset.
            rev_spec: Option<String>,
        },
        /// Sign a commit and print the signed commit's id to stdout.
        ///
        /// This command does not change symbolic refs.
        Sign {
            /// A specification of the revision to sign, or the current `HEAD` if unset.
            rev_spec: Option<String>,
        },
        /// Describe the current commit or the given one using the name of the closest annotated tag in its ancestry.
        Describe {
            /// Use annotated tag references only, not all tags.
            #[clap(long, short = 't', conflicts_with("all_refs"))]
            annotated_tags: bool,

            /// Use all references under the `ref/` namespaces, which includes tag references, local and remote branches.
            #[clap(long, short = 'a', conflicts_with("annotated_tags"))]
            all_refs: bool,

            /// Only follow the first parent when traversing the commit graph.
            #[clap(long, short = 'f')]
            first_parent: bool,

            /// Always display the long format, even if that would not be necessary as the id is located directly on a reference.
            #[clap(long, short = 'l')]
            long: bool,

            /// Consider only the given `n` candidates. This can take longer, but potentially produces more accurate results.
            #[clap(long, short = 'c', default_value = "10")]
            max_candidates: usize,

            /// Print information on stderr to inform about performance statistics
            #[clap(long, short = 's')]
            statistics: bool,

            #[clap(long)]
            /// If there was no way to describe the commit, fallback to using the abbreviated input revision.
            always: bool,

            /// Set the suffix to append if the repository is dirty (not counting untracked files).
            #[clap(short = 'd', long)]
            dirty_suffix: Option<Option<String>>,

            /// A specification of the revision to use, or the current `HEAD` if unset.
            rev_spec: Option<String>,
        },
    }
}

pub mod reset {
    use gix::bstr::BString;

    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        /// Only move `HEAD`, and keep the index and the worktree.
        #[clap(long, overrides_with_all = ["mixed", "hard", "merge", "keep"])]
        pub soft: bool,

        /// Move `HEAD` and reset the index, but keep the worktree. This is the default.
        #[clap(long, overrides_with_all = ["soft", "hard", "merge", "keep"])]
        pub mixed: bool,

        /// Move `HEAD`, and reset the index and the worktree, discarding the changes of tracked files.
        #[clap(long, overrides_with_all = ["soft", "mixed", "merge", "keep"])]
        pub hard: bool,

        /// Move `HEAD`, reset the index and the files that differ between `HEAD` and the commit, and keep the unstaged
        /// changes of other files, but refuse if a file to reset has unstaged changes.
        #[clap(long, overrides_with_all = ["soft", "mixed", "hard", "keep"])]
        pub merge: bool,

        /// Move `HEAD`, reset the index and the files that differ between `HEAD` and the commit, and keep the local
        /// changes of other files, but refuse if a file to reset has local changes.
        #[clap(long, overrides_with_all = ["soft", "mixed", "hard", "merge"])]
        pub keep: bool,

        /// The commit to reset to, or with paths the tree-ish to take their index entries from, which defaults to `HEAD`,
        /// followed by the paths whose index entries to reset if they can't be mistaken for a revision.
        #[clap(value_parser = crate::shared::AsBString)]
        pub args: Vec<BString>,

        /// The paths whose index entries to reset, after `--`.
        #[clap(last = true, value_parser = crate::shared::AsBString)]
        pub paths: Vec<BString>,
    }
}

pub mod rebase {
    use gix::bstr::BString;

    /// Which commits `--rebase-merges` rebases besides the descendants of the upstream.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
    pub enum RebaseMerges {
        /// Keep the commits that don't descend from the upstream on their original base.
        NoRebaseCousins,
        /// Rebase the commits that don't descend from the upstream onto it as well.
        RebaseCousins,
    }

    /// What a rebase does with a commit that becomes empty.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
    pub enum Empty {
        /// Leave it out.
        Drop,
        /// Make an empty commit of it.
        Keep,
        /// Stop for the user to decide.
        Stop,
    }

    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        /// Replay the commits onto this revision instead of the upstream.
        #[clap(long, value_name = "NEWBASE", value_parser = crate::shared::AsBString)]
        pub onto: Option<BString>,

        /// Replay the commits onto the merge base of the upstream and the branch.
        #[clap(long)]
        pub keep_base: bool,

        /// Replay all commits of the branch, onto `--onto` or onto a new root commit.
        ///
        /// The first argument is the branch to rebase then.
        #[clap(long)]
        pub root: bool,

        /// Let the sequence editor edit the list of commands to perform.
        #[clap(long, short = 'i')]
        pub interactive: bool,

        /// Run this command with the shell after each commit, and stop if it fails. Can be given more than once.
        #[clap(long, short = 'x', value_name = "CMD", value_parser = crate::shared::AsBString)]
        pub exec: Vec<BString>,

        /// Move the commits whose subjects start with `fixup!`, `squash!` or `amend!` after the commits they name.
        #[clap(long, overrides_with = "no_autosquash")]
        pub autosquash: bool,

        /// Don't move the commits whose subjects start with `fixup!`, `squash!` or `amend!`.
        #[clap(long, overrides_with = "autosquash")]
        pub no_autosquash: bool,

        /// Update the branches that point to the commits that are rebased.
        #[clap(long, overrides_with = "no_update_refs")]
        pub update_refs: bool,

        /// Don't update the branches that point to the commits that are rebased.
        #[clap(long, overrides_with = "update_refs")]
        pub no_update_refs: bool,

        /// Keep the merges and the shape of the history, instead of replaying it as one line.
        #[clap(
            long,
            short = 'r',
            value_name = "MODE",
            num_args = 0..=1,
            require_equals = true,
            default_missing_value = "no-rebase-cousins",
            overrides_with = "no_rebase_merges"
        )]
        pub rebase_merges: Option<RebaseMerges>,

        /// Replay the commits of merged branches as one line of history, even if `rebase.rebaseMerges` says otherwise.
        #[clap(long, overrides_with = "rebase_merges")]
        pub no_rebase_merges: bool,

        /// Leave out the commits that the reference log of the upstream had.
        ///
        /// This is the default without upstream unless `rebase.forkPoint` is `false`.
        #[clap(long, overrides_with = "no_fork_point")]
        pub fork_point: bool,

        /// Replay all commits that the upstream doesn't have.
        #[clap(long, overrides_with = "fork_point")]
        pub no_fork_point: bool,

        /// Replay all commits, even those whose parents wouldn't change.
        #[clap(long, short = 'f')]
        pub force_rebase: bool,

        /// What to do with commits that become empty: `drop`, `keep` or `stop`.
        ///
        /// If unset, they stop an interactive rebase, are kept with `--exec`, and dropped otherwise.
        #[clap(long, value_name = "MODE", ignore_case = true)]
        pub empty: Option<Empty>,

        /// Keep the commits that start empty, which is the default.
        #[clap(long, short = 'k')]
        pub keep_empty: bool,

        /// Don't run the `pre-rebase` hook.
        #[clap(long)]
        pub no_verify: bool,

        /// Put a failed `exec` command back into the todo list, to run again once the rebase continues.
        #[clap(long)]
        pub reschedule_failed_exec: bool,

        /// Commit the staged changes, and continue the rebase in progress.
        #[clap(long = "continue", exclusive = true)]
        pub continue_rebase: bool,

        /// Leave out the commit at which the rebase in progress stopped, and continue.
        #[clap(long, exclusive = true)]
        pub skip: bool,

        /// Abort the rebase in progress, and go back to the branch as it was before it.
        #[clap(long, exclusive = true)]
        pub abort: bool,

        /// Forget the rebase in progress, and keep `HEAD`, the index and the worktree as they are.
        #[clap(long, exclusive = true)]
        pub quit: bool,

        /// The revision whose history isn't replayed, or the upstream of the current branch if unset.
        ///
        /// With `--root`, the branch to rebase.
        #[clap(value_name = "UPSTREAM", value_parser = crate::shared::AsBString)]
        pub upstream: Option<BString>,

        /// The branch or commit to check out and rebase, or `HEAD` if unset.
        #[clap(value_name = "BRANCH", value_parser = crate::shared::AsBString)]
        pub branch: Option<BString>,
    }
}

pub mod tag {
    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        #[clap(subcommand)]
        pub cmds: Option<Subcommands>,
    }

    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// List all tags.
        List,
    }
}

pub mod credential {
    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// Get the credentials fed for `url=<url>` via STDIN.
        #[clap(visible_alias = "get")]
        Fill,
        /// Approve the information piped via STDIN as obtained with last call to `fill`
        #[clap(visible_alias = "store")]
        Approve,
        /// Try to resolve the given revspec and print the object names.
        #[clap(visible_alias = "erase")]
        Reject,
    }
}

///
pub mod commitgraph {
    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// Verify the integrity of a commit graph file.
        Verify {
            /// output statistical information about the graph.
            #[clap(long, short = 's')]
            statistics: bool,
        },
        /// List all entries in the commit-graph file as reachable by starting from `HEAD`.
        List {
            /// Display long hashes, instead of expensively shortened versions for best performance.
            #[clap(long, short = 'l')]
            long_hashes: bool,
            /// The rev-spec to list reachable commits from.
            #[clap(default_value = "@")]
            spec: std::ffi::OsString,
        },
    }
}

pub mod revision {
    pub mod resolve {
        #[derive(Default, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
        pub enum TreeMode {
            /// Show the raw bytes - only useful for piping into files for use with tooling.
            Raw,
            /// Display a tree in human-readable form.
            #[default]
            Pretty,
        }

        #[derive(Default, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
        pub enum BlobFormat {
            /// The version stored in the Git Object Database.
            #[default]
            Git,
            /// The version that would be checked out into the worktree, including filters.
            Worktree,
            /// The version that would be diffed (Worktree + Text-Conversion).
            Diff,
            /// The version that would be diffed if there is a text-conversion, or the one stored in Git otherwise.
            DiffOrGit,
        }
    }

    #[derive(Debug, clap::Subcommand)]
    #[clap(visible_alias = "rev", visible_alias = "r")]
    pub enum Subcommands {
        /// List all commits reachable from the given rev-spec.
        #[clap(visible_alias = "l")]
        List {
            /// Display long hashes, instead of expensively shortened versions for best performance.
            #[clap(long, short = 'l')]
            long_hashes: bool,
            /// How many commits to list at most.
            #[clap(long)]
            limit: Option<usize>,
            /// Write the graph as SVG file to the given path.
            #[clap(long, short = 's')]
            svg: Option<std::path::PathBuf>,
            /// The rev-spec to list reachable commits from.
            #[clap(default_value = "@")]
            spec: std::ffi::OsString,
        },
        /// Provide the revision specification like `@~1` to explain.
        #[clap(visible_alias = "e")]
        Explain { spec: std::ffi::OsString },
        /// Try to resolve the given revspec and print the object names.
        #[clap(visible_alias = "query", visible_alias = "parse", visible_alias = "p")]
        Resolve {
            /// Instead of resolving a rev-spec, explain what would be done for the first spec.
            ///
            /// Equivalent to the `explain` subcommand.
            #[clap(short = 'e', long)]
            explain: bool,
            /// Also show the name of the reference which led to the object.
            #[clap(short = 'r', long, conflicts_with = "explain")]
            reference: bool,
            /// Show the first resulting object similar to how `git cat-file` would, but don't show the resolved spec.
            #[clap(short = 'c', long, conflicts_with = "explain")]
            cat_file: bool,
            /// How to display blobs.
            #[clap(short = 'b', long, default_value = "git")]
            blob_format: resolve::BlobFormat,
            /// How to display trees as obtained with `@:dirname` or `@^{tree}`.
            #[clap(short = 't', long, default_value = "pretty")]
            tree_mode: resolve::TreeMode,
            /// rev-specs like `@`, `@~1` or `HEAD^2`.
            #[clap(required = true)]
            specs: Vec<std::ffi::OsString>,
        },
        /// Return the names and hashes of all previously checked-out branches.
        #[clap(visible_alias = "prev")]
        PreviousBranches,
    }
}

pub mod attributes {
    use gix::bstr::BString;

    use crate::shared::CheckPathSpec;

    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// Run `git check-attr` and `git check-ignore` on all files of the index or all files
        /// passed via stdin and validate that we get the same outcome when computing attributes.
        ValidateBaseline {
            /// Print various statistics to stderr.
            #[clap(long, short = 's')]
            statistics: bool,
            /// Don't validated excludes as obtaining them with `check-ignore` can be very slow.
            #[clap(long)]
            no_ignore: bool,
        },
        /// List all attributes of the given path-specs and display the result similar to `git check-attr`.
        Query {
            /// Print various statistics to stderr.
            #[clap(long, short = 's')]
            statistics: bool,
            /// The Git path specifications to list attributes for, or unset to read from stdin one per line.
            #[clap(value_parser = CheckPathSpec)]
            pathspec: Vec<BString>,
        },
    }
}

pub mod exclude {
    use std::ffi::OsString;

    use gix::bstr::BString;

    use crate::shared::AsBString;

    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// Check if paths are excluded and print the result similar to `git check-ignore`.
        Query {
            /// Print various statistics to stderr.
            #[clap(long, short = 's')]
            statistics: bool,
            /// Show actual ignore patterns instead of un-excluding an entry.
            ///
            /// That way one can understand why an entry might not be excluded.
            #[clap(long, short = 'i')]
            show_ignore_patterns: bool,
            /// Additional patterns to use for exclusions. They have the highest priority.
            ///
            /// Useful for undoing previous patterns using the '!' prefix.
            #[clap(long, short = 'p')]
            patterns: Vec<OsString>,
            /// The paths to check for exclusion, or unset to read from stdin one per line.
            #[clap(value_parser = AsBString)]
            paths: Vec<BString>,
        },
    }
}

pub mod index {
    use std::path::PathBuf;

    use gix::bstr::BString;

    use crate::shared::CheckPathSpec;

    pub mod entries {
        #[derive(Default, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
        pub enum Format {
            /// Show only minimal information, useful for first glances.
            #[default]
            Simple,
            /// Show much more information that is still human-readable.
            Rich,
        }
    }

    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// Print all entries to standard output.
        Entries {
            /// How to output index entries.
            #[clap(long, short = 'f', default_value = "simple", value_enum)]
            format: entries::Format,
            /// Do not visualize excluded entries or attributes per path.
            #[clap(long)]
            no_attributes: bool,
            /// Load attribute and ignore files from the index, don't look at the worktree.
            ///
            /// This is to see what IO for probing attribute/ignore files does to performance.
            #[clap(long, short = 'i', conflicts_with = "no_attributes")]
            attributes_from_index: bool,
            /// Display submodule entries as well if their repository exists.
            #[clap(long, short = 'r')]
            recurse_submodules: bool,
            /// Print various statistics to stderr.
            #[clap(long, short = 's')]
            statistics: bool,
            /// The git path specifications to match entries to print.
            #[clap(value_parser = CheckPathSpec)]
            pathspec: Vec<BString>,
        },
        /// Create an index from a tree-ish.
        #[clap(visible_alias = "read-tree")]
        FromTree {
            /// Overwrite the specified index file if it already exists.
            #[clap(long, short = 'f')]
            force: bool,
            /// Path to the index file to be written.
            ///
            /// If none is given it will be kept in memory only as a way to measure performance.
            /// One day we will probably write the index back by default, but that requires us to
            /// write more of the index to work.
            #[clap(long, short = 'i')]
            index_output_path: Option<PathBuf>,
            /// Don't write the trailing hash for a performance gain.
            #[clap(long, short = 's')]
            skip_hash: bool,
            /// A revspec that points to the to generate the index from.
            spec: std::ffi::OsString,
        },
    }
}

pub mod submodule {
    #[derive(Debug, clap::Parser)]
    pub struct Platform {
        #[clap(subcommand)]
        pub cmds: Option<Subcommands>,
    }

    #[derive(Debug, clap::Subcommand)]
    pub enum Subcommands {
        /// Print all direct submodules to standard output.
        List {
            /// Set the suffix to append if the repository is dirty (not counting untracked files).
            #[clap(short = 'd', long)]
            dirty_suffix: Option<Option<String>>,
        },
    }
}

///
pub mod free;
