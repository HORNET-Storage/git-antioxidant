use crate::config::tree::{Key, Section, Sequence};

impl Section for Sequence {
    fn name(&self) -> &str {
        "sequence"
    }

    fn keys(&self) -> &[&dyn Key] {
        &[]
    }
}
