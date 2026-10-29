//! The todo list that a rebase starts with, as `sequencer_make_script()` and `make_script_with_merges()` make it from
//! the commits of a [walk](crate::walk).

use std::collections::{HashMap, HashSet};

use bstr::{BStr, BString, ByteSlice};
use gix_error::Result;
use gix_hash::{ObjectId, oid};
use gix_ref::FullName;
use gix_sequencer::todo::{Command, Instruction, Item, List, ShortenId};

use crate::{commits::Commits, walk::Walk};

/// The longest label that `--rebase-merges` makes from a message, as `GIT_MAX_LABEL_LENGTH` is with a `NAME_MAX` of 255,
/// which leaves room for `.lock` and for a number that makes the label unique.
pub const MAX_LABEL_LENGTH: usize = 234;

/// How `--rebase-merges` treats commits that don't descend from `upstream`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum RebaseMerges {
    /// Keep them where they are, resetting to them by their abbreviated id, as `--rebase-merges=no-rebase-cousins` does.
    #[default]
    NoRebaseCousins,
    /// Replay them onto `onto` too, as `--rebase-merges=rebase-cousins` does.
    RebaseCousins,
}

/// How [`make()`] makes the todo list, as the flags of `sequencer_make_script()` say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options<'a> {
    /// List empty commits as `pick` with ` # empty` after their subject, as `--keep-empty` does, which is the default.
    /// Otherwise they are left out.
    pub keep_empty: bool,
    /// Keep the merges and the branches they merge, as `--rebase-merges` does.
    pub rebase_merges: Option<RebaseMerges>,
    /// `--root` with `--onto`, after which branches without a parent in the list reset to `onto` instead of
    /// `[new root]`.
    pub root_with_onto: bool,
    /// The string that starts comments, the value of `core.commentString` or `core.commentChar`.
    pub comment: &'a BStr,
    /// The longest label that a message makes, the value of `rebase.maxLabelLength`.
    pub max_label_length: usize,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options {
            keep_empty: true,
            rebase_merges: None,
            root_with_onto: false,
            comment: gix_sequencer::todo::default_comment(),
            max_label_length: MAX_LABEL_LENGTH,
        }
    }
}

/// A function that returns the text of a commit after its id in a todo list, `# <subject>` for the default
/// `rebase.instructionFormat` of `# %s`.
pub type Oneline<'a> = dyn FnMut(&oid, &crate::commits::Commit) -> Result<BString> + 'a;

/// Return the text of `commit` after its id for the default `rebase.instructionFormat` of `# %s`, as
/// `pretty_print_commit()` formats it.
pub fn default_oneline(_id: &oid, commit: &crate::commits::Commit) -> Result<BString> {
    let mut out = BString::from("# ");
    out.extend_from_slice(&commit.subject());
    Ok(out)
}

/// The branches that point at commits, in the order in which `load_branch_decorations()` decorates each commit with
/// them, which puts the last of them by name first.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Decorations {
    by_commit: HashMap<ObjectId, Vec<FullName>>,
}

impl Decorations {
    /// Collect the branches of `refs`, each a full reference name with the commit it points at. Only the references in
    /// `refs/heads/` decorate commits, as `load_branch_decorations()` loads only these.
    pub fn new(refs: impl IntoIterator<Item = (FullName, ObjectId)>) -> Self {
        let mut refs: Vec<_> = refs
            .into_iter()
            .filter(|(name, _)| name.as_bstr().starts_with(b"refs/heads/"))
            .collect();
        refs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut by_commit: HashMap<ObjectId, Vec<FullName>> = HashMap::new();
        for (name, id) in refs {
            by_commit.entry(id).or_default().insert(0, name);
        }
        Decorations { by_commit }
    }

    /// Return the branches that point at `commit`, the last of them by name first.
    pub fn of(&self, commit: &oid) -> &[FullName] {
        self.by_commit.get(commit).map_or(&[], Vec::as_slice)
    }
}

/// What [`make()`] needs from the repository besides the commits.
pub struct Context<'a, 'b> {
    /// The commits of the repository.
    pub commits: &'a mut Commits<'b>,
    /// The function that formats the text of a commit after its id, like [`default_oneline()`].
    pub oneline: &'a mut Oneline<'b>,
    /// The function that returns the shortest unique abbreviation of an id, which names commits that `--rebase-merges`
    /// resets to without replaying them, as `repo_find_unique_abbrev()` does.
    pub abbreviate: &'a mut ShortenId<'b>,
    /// The branches of the repository, which name the branches that `--rebase-merges` merges.
    pub decorations: &'a Decorations,
}

/// Make the todo list of a rebase that replays the commits of `walk`, as `sequencer_make_script()` makes it with
/// `options`, or as `make_script_with_merges()` does for `--rebase-merges`.
///
/// Each commit becomes a `pick`, except that commits whose patch is upstream are left out, and empty commits too unless
/// they are kept. An empty list gets a `noop`, as `complete_action()` adds it.
pub fn make(context: &mut Context<'_, '_>, walk: &Walk, options: &Options<'_>) -> Result<List> {
    let mut items = match options.rebase_merges {
        None => {
            let mut items = Vec::new();
            for commit in &walk.commits {
                if is_left_out(commit, options) {
                    continue;
                }
                let arg = pick_arg(oneline(context, &commit.id)?, commit, options);
                items.push(Item::Command(Instruction::new(Command::Pick, Some(commit.id), arg)));
            }
            items
        }
        Some(merges) => with_merges(context, walk, options, merges)?,
    };
    if items.is_empty() {
        items.push(Item::Command(Instruction::new(Command::Noop, None, "")));
    }
    Ok(List::from_items(items))
}

/// Return `true` if `commit` isn't replayed: its patch is upstream, or it is empty and empty commits aren't kept.
fn is_left_out(commit: &crate::walk::Commit, options: &Options<'_>) -> bool {
    (!commit.empty && commit.patch_same) || (commit.empty && !options.keep_empty)
}

/// Return the argument of the `pick` of `commit` with the text `line`, which gets ` # empty` after it for an empty
/// commit.
fn pick_arg(mut line: BString, commit: &crate::walk::Commit, options: &Options<'_>) -> BString {
    if commit.empty {
        line.push(b' ');
        line.extend_from_slice(options.comment);
        line.extend_from_slice(b" empty");
    }
    line
}

/// Return the text of the commit `id` after its id.
fn oneline(context: &mut Context<'_, '_>, id: &oid) -> Result<BString> {
    let commit = context.commits.get(id)?.clone();
    (context.oneline)(id, &commit)
}

/// Make the todo list of `--rebase-merges`, as `make_script_with_merges()` does in three phases: the first labels the
/// branches that merges merge, the second labels the commits from which branches fork, and the third lists the branches
/// one after the other, each with a reset to where it starts.
fn with_merges(
    context: &mut Context<'_, '_>,
    walk: &Walk,
    options: &Options<'_>,
    merges: RebaseMerges,
) -> Result<Vec<Item>> {
    let rebase_cousins = merges == RebaseMerges::RebaseCousins;
    let mut labels = Labels::new(options.max_label_length, context.commits.object_hash());
    if let Some(base) = walk.merge_base {
        labels.commit2label.insert(base, "onto".into());
        labels.labels.insert("onto".into());
    }

    let mut interesting = HashSet::new();
    let mut commit2todo: HashMap<ObjectId, Item> = HashMap::new();
    let mut tips = Vec::new();
    for commit in &walk.commits {
        interesting.insert(commit.id);
        if is_left_out(commit, options) {
            continue;
        }
        let line = oneline(context, &commit.id)?;
        let parents = context.commits.parents(&commit.id)?;
        if parents.len() < 2 {
            let arg = pick_arg(line, commit, options);
            commit2todo.insert(
                commit.id,
                Item::Command(Instruction::new(Command::Pick, Some(commit.id), arg)),
            );
            continue;
        }
        let label_from_message = label_from_message(&line);
        let mut arg = BString::default();
        for parent in &parents[1..] {
            let label = context
                .decorations
                .of(parent)
                .first()
                .and_then(|name| name.as_bstr().strip_prefix(b"refs/heads/").map(BString::from))
                .unwrap_or_else(|| label_from_message.clone());
            if !arg.is_empty() {
                arg.push(b' ');
            }
            if !interesting.contains(parent) {
                arg.extend_from_slice(&labels.label(parent, None, context.abbreviate)?);
                continue;
            }
            tips.push(*parent);
            arg.extend_from_slice(&labels.label(parent, Some(label.as_bstr()), context.abbreviate)?);
        }
        arg.push(b' ');
        arg.extend_from_slice(&line);
        commit2todo.insert(
            commit.id,
            Item::Command(Instruction::new(Command::Merge, Some(commit.id), arg)),
        );
    }

    let mut child_seen = HashSet::new();
    for (index, commit) in walk.commits.iter().enumerate() {
        for parent in context.commits.parents(&commit.id)? {
            if interesting.contains(&parent) && !child_seen.insert(parent) {
                labels.label(&parent, Some(b"branch-point".as_bstr()), context.abbreviate)?;
            }
        }
        if index + 1 == walk.commits.len() {
            tips.push(commit.id);
        }
    }

    let mut items = vec![Item::Command(Instruction::new(Command::Label, None, "onto"))];
    let mut shown = HashSet::new();
    for tip in tips {
        if shown.contains(&tip) {
            continue;
        }
        items.push(Item::Comment(BString::default()));
        if let Some(label) = labels.commit2label.get(&tip) {
            let mut comment = BString::from(options.comment.as_bytes());
            comment.extend_from_slice(b" Branch ");
            comment.extend_from_slice(label);
            items.push(Item::Comment(comment));
        }
        let mut branch = Vec::new();
        let mut commit = Some(tip);
        while let Some(id) = commit
            && interesting.contains(&id)
            && !shown.contains(&id)
        {
            branch.push(id);
            commit = context.commits.parents(&id)?.first().copied();
        }
        branch.reverse();
        let reset = match commit {
            None if rebase_cousins || options.root_with_onto => BString::from("onto"),
            None => BString::from("[new root]"),
            Some(id) => {
                let to = match labels.commit2label.get(&id) {
                    Some(label) => Some(label.clone()),
                    None if !rebase_cousins => Some(labels.label(&id, None, context.abbreviate)?),
                    None => None,
                };
                match to {
                    Some(to) if to != "onto" => {
                        let mut arg = to;
                        arg.push(b' ');
                        arg.extend_from_slice(&oneline(context, &id)?);
                        arg
                    }
                    _ => BString::from("onto"),
                }
            }
        };
        items.push(Item::Command(Instruction::new(Command::Reset, None, reset)));
        for id in branch {
            if let Some(item) = commit2todo.get(&id) {
                items.push(item.clone());
            }
            if let Some(label) = labels.commit2label.get(&id) {
                items.push(Item::Command(Instruction::new(Command::Label, None, label.clone())));
            }
            shown.insert(id);
        }
    }
    Ok(items)
}

/// Return the label that the merge with the text `line` gives the branches it merges, as `make_script_with_merges()`
/// takes it from `# Merge branch '<label>'`, from `# Merge pull request #<n> from <label>`, or the whole text otherwise.
fn label_from_message(line: &[u8]) -> BString {
    if let Some(rest) = line.strip_prefix(b"# Merge ")
        && let Some(start) = rest.find_byte(b'\'')
        && let Some(len) = rest[start + 1..].find_byte(b'\'')
    {
        return rest[start + 1..start + 1 + len].into();
    }
    if let Some(rest) = line.strip_prefix(b"# Merge pull request ")
        && let Some(start) = rest.find(b" from ")
    {
        return rest[start + b" from ".len()..].into();
    }
    line.into()
}

/// The labels of `--rebase-merges`, as `struct label_state` keeps them.
struct Labels {
    commit2label: HashMap<ObjectId, BString>,
    labels: HashSet<BString>,
    max_label_length: usize,
    object_hash: gix_hash::Kind,
}

impl Labels {
    fn new(max_label_length: usize, object_hash: gix_hash::Kind) -> Self {
        Labels {
            commit2label: HashMap::new(),
            labels: HashSet::new(),
            max_label_length,
            object_hash,
        }
    }

    /// Return the label of `id`, giving it one if it has none, as `label_oid()` does: with `text`, the text with each run
    /// of characters that aren't alphanumeric replaced by a dash and made unique with a number, or otherwise the
    /// shortest abbreviation of `id` that `abbreviate` returns, longer if it is taken.
    fn label(&mut self, id: &oid, text: Option<&BStr>, abbreviate: &mut ShortenId<'_>) -> Result<BString> {
        if let Some(label) = self.commit2label.get(id) {
            return Ok(label.clone());
        }
        let label = match text {
            None => {
                let abbreviation = abbreviate(id)?;
                if self.labels.contains(&abbreviation) {
                    let hex = BString::from(id.to_hex().to_string());
                    let free = (abbreviation.len() + 1..hex.len())
                        .map(|len| BString::from(&hex[..len]))
                        .find(|prefix| !self.labels.contains(prefix));
                    free.unwrap_or(hex)
                } else {
                    abbreviation
                }
            }
            Some(text) => {
                let mut label = sanitize(text, self.max_label_length);
                if label.is_empty() {
                    label.extend_from_slice(b"rev-");
                    label.extend_from_slice(&abbreviate(id)?);
                }
                let is_full_id =
                    label.len() == self.object_hash.len_in_hex() && label.iter().all(u8::is_ascii_hexdigit);
                if is_full_id || label == "#" || self.labels.contains(&label) {
                    let len = label.len();
                    for number in 2.. {
                        label.truncate(len);
                        label.extend_from_slice(format!("-{number}").as_bytes());
                        if !self.labels.contains(&label) {
                            break;
                        }
                    }
                }
                label
            }
        };
        self.labels.insert(label.clone());
        self.commit2label.insert(id.to_owned(), label.clone());
        Ok(label)
    }
}

/// Return `text` with each run of characters that aren't alphanumeric replaced by a dash, without a leading dash and at
/// most `max_len - 1` bytes long, as `label_oid()` sanitizes labels. Non-ASCII characters are kept whole if `text` is
/// UTF-8 so far, and byte by byte otherwise.
fn sanitize(text: &[u8], max_len: usize) -> BString {
    let mut out = BString::default();
    let mut is_utf8 = true;
    let mut index = 0;
    while index < text.len() && out.len() + 1 < max_len {
        let byte = text[index];
        if byte.is_ascii_alphanumeric() || (!is_utf8 && byte & 0x80 != 0) {
            out.push(byte);
        } else if byte & 0x80 != 0 {
            match utf8_char_len(&text[index..]) {
                Some(len) => {
                    if out.len() + len > max_len {
                        break;
                    }
                    out.extend_from_slice(&text[index..index + len]);
                    index += len - 1;
                }
                None => {
                    is_utf8 = false;
                    out.push(byte);
                }
            }
        } else if out.last().is_some_and(|last| *last != b'-') {
            out.push(b'-');
        }
        index += 1;
    }
    out
}

/// Return the length of the UTF-8 character at the start of `bytes`, or `None` if it isn't one, as
/// `pick_one_utf8_char()` decodes it, which rejects overlong forms, surrogates, U+FFFE and U+FFFF, and characters
/// beyond U+10FFFF.
fn utf8_char_len(bytes: &[u8]) -> Option<usize> {
    let at = |index: usize| bytes.get(index).copied();
    let continuation = |index: usize| at(index).is_some_and(|byte| byte & 0xc0 == 0x80);
    let first = at(0)?;
    if first < 0x80 {
        Some(1)
    } else if first & 0xe0 == 0xc0 {
        (continuation(1) && first & 0xfe != 0xc0).then_some(2)
    } else if first & 0xf0 == 0xe0 {
        let second = at(1)?;
        let invalid = (first == 0xe0 && second & 0xe0 == 0x80)
            || (first == 0xed && second & 0xe0 == 0xa0)
            || (first == 0xef && second == 0xbf && at(2).is_some_and(|third| third & 0xfe == 0xbe));
        (continuation(1) && continuation(2) && !invalid).then_some(3)
    } else if first & 0xf8 == 0xf0 {
        let second = at(1)?;
        let invalid = (first == 0xf0 && second & 0xf0 == 0x80) || (first == 0xf4 && second > 0x8f) || first > 0xf4;
        (continuation(1) && continuation(2) && continuation(3) && !invalid).then_some(4)
    } else {
        None
    }
}
