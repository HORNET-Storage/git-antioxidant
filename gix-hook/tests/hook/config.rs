use bstr::ByteSlice;
use gix_hook::config::{Configured, Entry};
use gix_testtools::Result;

use crate::{config_values, fixture, read_text, scenario};

#[test]
fn entries_from_keys() {
    let value = Some(b"pre-commit".as_bstr());
    assert_eq!(
        Entry::from_key("hook.linter.event".into(), value),
        Some(Entry {
            name: Some("linter".into()),
            key: "event".into(),
            value
        }),
        "the name is between the section and the key"
    );
    assert_eq!(
        Entry::from_key("Hook.Name.With.Dots.command".into(), value),
        Some(Entry {
            name: Some("Name.With.Dots".into()),
            key: "command".into(),
            value
        }),
        "the section is matched without regard to case, and the name extends to the last dot"
    );
    assert_eq!(
        Entry::from_key("hook.jobs".into(), value),
        Some(Entry {
            name: None,
            key: "jobs".into(),
            value
        }),
        "keys of the section itself have no name"
    );
    assert_eq!(
        Entry::from_key("core.hooksPath".into(), value),
        None,
        "keys of other sections aren't entries"
    );
}

#[test]
fn configurations_git_refuses_are_refused_with_its_message() -> Result {
    let Some(root) = fixture()? else {
        return Ok(());
    };
    let dir = scenario(&root, "config-errors");
    for case in ["missing-command", "known-name", "missing-value"] {
        let values = config_values(&dir.join(format!("{case}-config")))?;
        let err =
            Configured::from_entries(values.iter().filter_map(|(key, value)| {
                Entry::from_key(key.as_bstr(), value.as_ref().map(|value| value.as_bstr()))
            }))
            .expect_err("Git refuses this configuration");
        let message = err.error().to_string();
        let git_message = read_text(&dir.join(case))?;
        assert!(
            git_message.contains(&message),
            "{case}: the configuration is refused as Git refuses it, with {git_message:?}, not {message:?}"
        );
    }
    Ok(())
}
