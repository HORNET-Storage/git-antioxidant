use crate::config::tree::{Key, Rebase, Section};

impl Section for Rebase {
    fn name(&self) -> &str {
        "rebase"
    }

    fn keys(&self) -> &[&dyn Key] {
        &[]
    }
}
