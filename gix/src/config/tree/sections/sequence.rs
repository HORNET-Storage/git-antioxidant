use crate::{
    config,
    config::tree::{Key, Section, Sequence, keys},
};

impl Sequence {
    /// The `sequence.editor` key, the program that edits the todo list of an interactive rebase, which
    /// `GIT_SEQUENCE_EDITOR` overrides. Without either, the [editor](crate::Repository::editor()) edits it.
    pub const EDITOR: keys::Program =
        keys::Program::new_program("editor", &config::Tree::SEQUENCE).with_environment_override("GIT_SEQUENCE_EDITOR");
}

impl Section for Sequence {
    fn name(&self) -> &str {
        "sequence"
    }

    fn keys(&self) -> &[&dyn Key] {
        &[&Self::EDITOR]
    }
}
