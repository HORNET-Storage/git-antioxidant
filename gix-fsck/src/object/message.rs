use std::fmt;

use bstr::BStr;

use super::Severity;

macro_rules! message_ids {
    ($($(#[doc = $doc:literal])* $variant:ident = $name:literal, $severity:ident;)*) => {
        /// A check of Git's `fsck`, named as in the `fsck.<id>` configuration and in reports.
        ///
        /// The list is Git's, in Git's order, with each check's default severity. Object checks report all
        /// of them except those about references, which concern reference checks, [`UnknownType`](Self::UnknownType),
        /// as every [`Kind`](gix_object::Kind) is known, and [`EmptyName`](Self::EmptyName), as a tree entry
        /// without a name makes the tree [unparseable](Self::BadTree), as in Git.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[non_exhaustive]
        pub enum MessageId {
            $($(#[doc = $doc])* $variant,)*
        }

        impl MessageId {
            /// All checks, in Git's order.
            pub const ALL: &'static [MessageId] = &[$(MessageId::$variant,)*];

            /// The name of the check, as Git prints it and as `fsck.<id>` configures it.
            pub fn name(self) -> &'static str {
                match self {
                    $(MessageId::$variant => $name,)*
                }
            }

            /// The severity of the check unless it's configured otherwise.
            pub fn default_severity(self) -> Severity {
                match self {
                    $(MessageId::$variant => Severity::$severity,)*
                }
            }
        }
    };
}

message_ids! {
    /// A NUL byte in the headers of a commit or tag.
    NulInHeader = "nulInHeader", Fatal;
    /// Headers of a commit or tag that don't end with a newline.
    UnterminatedHeader = "unterminatedHeader", Fatal;
    /// A `gpgsig` continuation line of a tag that is cut short.
    BadHeaderContinuation = "badHeaderContinuation", Error;
    /// A date in an author, committer or tagger line that isn't a number.
    BadDate = "badDate", Error;
    /// A date in an author, committer or tagger line that is too large.
    BadDateOverflow = "badDateOverflow", Error;
    /// An email in an author, committer or tagger line that isn't closed by `>`.
    BadEmail = "badEmail", Error;
    /// A `gpgsig` header of a tag that is cut short.
    BadGpgsig = "badGpgsig", Error;
    /// A reference check: `HEAD` is a symbolic reference that doesn't refer to a branch.
    BadHeadTarget = "badHeadTarget", Error;
    /// A name in an author, committer or tagger line that contains `>`.
    BadName = "badName", Error;
    /// The `object` line of a tag doesn't hold an object ID.
    BadObjectSha1 = "badObjectSha1", Error;
    /// A reference check: an invalid entry in `packed-refs`.
    BadPackedRefEntry = "badPackedRefEntry", Error;
    /// A reference check: an invalid header in `packed-refs`.
    BadPackedRefHeader = "badPackedRefHeader", Error;
    /// A `parent` line of a commit doesn't hold an object ID.
    BadParentSha1 = "badParentSha1", Error;
    /// A reference check: an invalid target name of a symbolic reference.
    BadReferentName = "badReferentName", Error;
    /// A reference check: a reference with invalid content.
    BadRefContent = "badRefContent", Error;
    /// A reference check: a reference of an invalid file type.
    BadRefFiletype = "badRefFiletype", Error;
    /// A reference check: an invalid reference name.
    BadRefName = "badRefName", Error;
    /// A reference check: a reference that points to an invalid object ID.
    BadRefOid = "badRefOid", Error;
    /// A time zone in an author, committer or tagger line that isn't `+hhmm` or `-hhmm`.
    BadTimezone = "badTimezone", Error;
    /// A tree that can't be parsed.
    BadTree = "badTree", Error;
    /// The `tree` line of a commit doesn't hold an object ID.
    BadTreeSha1 = "badTreeSha1", Error;
    /// The `type` line of a tag names no object type.
    BadType = "badType", Error;
    /// A tree with two entries of the same name.
    DuplicateEntries = "duplicateEntries", Error;
    /// An object named `.gitattributes` in a tree that isn't a blob.
    GitattributesBlob = "gitattributesBlob", Error;
    /// A `.gitattributes` blob too large to parse.
    GitattributesLarge = "gitattributesLarge", Error;
    /// A `.gitattributes` blob with lines too long to parse.
    GitattributesLineLength = "gitattributesLineLength", Error;
    /// A `.gitattributes` blob that a tree names but that can't be read.
    GitattributesMissing = "gitattributesMissing", Error;
    /// An object named `.gitmodules` in a tree that isn't a blob.
    GitmodulesBlob = "gitmodulesBlob", Error;
    /// A `.gitmodules` blob too large to parse.
    GitmodulesLarge = "gitmodulesLarge", Error;
    /// A `.gitmodules` blob that a tree names but that can't be read.
    GitmodulesMissing = "gitmodulesMissing", Error;
    /// A submodule name in `.gitmodules` that is empty or has a `..` component.
    GitmodulesName = "gitmodulesName", Error;
    /// A submodule path in `.gitmodules` that starts with `-`.
    GitmodulesPath = "gitmodulesPath", Error;
    /// A `.gitmodules` entry in a tree that is a symbolic link.
    GitmodulesSymlink = "gitmodulesSymlink", Error;
    /// A submodule update setting in `.gitmodules` that runs a command.
    GitmodulesUpdate = "gitmodulesUpdate", Error;
    /// A submodule URL in `.gitmodules` that could be taken for an option or could reach beyond its host.
    GitmodulesUrl = "gitmodulesUrl", Error;
    /// A commit without an `author` line.
    MissingAuthor = "missingAuthor", Error;
    /// A commit without a `committer` line.
    MissingCommitter = "missingCommitter", Error;
    /// An author, committer or tagger line without an email.
    MissingEmail = "missingEmail", Error;
    /// An author, committer or tagger line whose email isn't preceded by a name.
    MissingNameBeforeEmail = "missingNameBeforeEmail", Error;
    /// A tag without an `object` line.
    MissingObject = "missingObject", Error;
    /// An author, committer or tagger line without a space before its date.
    MissingSpaceBeforeDate = "missingSpaceBeforeDate", Error;
    /// An author, committer or tagger line without a space before its email.
    MissingSpaceBeforeEmail = "missingSpaceBeforeEmail", Error;
    /// A tag that ends in its `tag` line.
    MissingTag = "missingTag", Error;
    /// A tag without a `tag` line.
    MissingTagEntry = "missingTagEntry", Error;
    /// A commit without a `tree` line.
    MissingTree = "missingTree", Error;
    /// A tag that ends in its `type` line.
    MissingType = "missingType", Error;
    /// A tag without a `type` line.
    MissingTypeEntry = "missingTypeEntry", Error;
    /// A commit with more than one `author` line.
    MultipleAuthors = "multipleAuthors", Error;
    /// A reference check: an entry of `packed-refs` without a newline.
    PackedRefEntryNotTerminated = "packedRefEntryNotTerminated", Error;
    /// A reference check: `packed-refs` isn't sorted.
    PackedRefUnsorted = "packedRefUnsorted", Error;
    /// A tree whose entries aren't in Git's order.
    TreeNotSorted = "treeNotSorted", Error;
    /// An object of an unknown type.
    UnknownType = "unknownType", Error;
    /// A date in an author, committer or tagger line with leading zeros.
    ZeroPaddedDate = "zeroPaddedDate", Error;
    /// A reference check: a reftable table with an invalid name.
    BadReftableTableName = "badReftableTableName", Warn;
    /// A tree entry without a name.
    EmptyName = "emptyName", Warn;
    /// A tree entry whose name contains `/`.
    FullPathname = "fullPathname", Warn;
    /// A tree entry named `.`.
    HasDot = "hasDot", Warn;
    /// A tree entry named `..`.
    HasDotdot = "hasDotdot", Warn;
    /// A tree entry whose name a file system may take for `.git`.
    HasDotgit = "hasDotgit", Warn;
    /// A tree entry with a name longer than the [limit](super::Options::max_path_component_len()).
    LargePathname = "largePathname", Warn;
    /// A tree entry that points to the null object ID.
    NullSha1 = "nullSha1", Warn;
    /// A commit that contains a NUL byte.
    NulInCommit = "nulInCommit", Warn;
    /// A tree entry whose mode has a leading zero.
    ZeroPaddedFilemode = "zeroPaddedFilemode", Warn;
    /// A tree entry with a mode Git doesn't write.
    BadFilemode = "badFilemode", Info;
    /// The `tag` line of a tag names an invalid reference.
    BadTagName = "badTagName", Info;
    /// A reference check: an empty `packed-refs` file.
    EmptyPackedRefsFile = "emptyPackedRefsFile", Info;
    /// A `.gitattributes` entry in a tree that is a symbolic link.
    GitattributesSymlink = "gitattributesSymlink", Info;
    /// A `.gitignore` entry in a tree that is a symbolic link.
    GitignoreSymlink = "gitignoreSymlink", Info;
    /// A `.gitmodules` blob that can't be parsed as configuration.
    GitmodulesParse = "gitmodulesParse", Info;
    /// A `.mailmap` entry in a tree that is a symbolic link.
    MailmapSymlink = "mailmapSymlink", Info;
    /// A tag without a `tagger` line.
    MissingTaggerEntry = "missingTaggerEntry", Info;
    /// A reference check: a loose reference that doesn't end with a newline.
    RefMissingNewline = "refMissingNewline", Info;
    /// A reference check: a symbolic link used as a symbolic reference.
    SymlinkRef = "symlinkRef", Info;
    /// A reference check: a symbolic reference that points neither to a root reference nor below `refs/`.
    SymrefTargetIsNotARef = "symrefTargetIsNotARef", Info;
    /// A reference check: a loose reference with trailing content.
    TrailingRefContent = "trailingRefContent", Info;
    /// Headers of a tag after its `tagger` line.
    ExtraHeaderEntry = "extraHeaderEntry", Ignore;
}

/// The number of checks.
pub(super) const COUNT: usize = MessageId::ALL.len();

impl MessageId {
    /// The check whose name is `name`, compared without regard to ASCII case as Git compares it.
    pub fn from_name(name: &BStr) -> Option<MessageId> {
        MessageId::ALL
            .iter()
            .copied()
            .find(|id| id.name().as_bytes().eq_ignore_ascii_case(name))
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
