//! The hooks that the configuration names: `hook.<name>.command`, run for the events that `hook.<name>.event` lists.

use std::collections::{BTreeMap, BTreeSet};

use bstr::{BStr, BString, ByteSlice};
use gix_error::{Result, bail};

use crate::{Found, Hook, Skipped, is_known_event};

/// One value of a `hook.*` key of the configuration.
///
/// [`Configured::from_entries()`] takes them in the order Git reads its configuration: the system, global, local and
/// worktree files, then the values on its command line, each from top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The `<name>` of `hook.<name>.<key>`, whose case matters, or `None` for a key like `hook.jobs`.
    pub name: Option<&'a BStr>,
    /// The `<key>`, like `command` or `event`, whose case doesn't matter.
    pub key: &'a BStr,
    /// The value, or `None` if the key has no `=`.
    pub value: Option<&'a BStr>,
}

impl<'a> Entry<'a> {
    /// Split the full `key` of `value`, like `hook.linter.command`, into an entry, or return `None` if the key isn't in
    /// the `hook` section.
    pub fn from_key(key: &'a BStr, value: Option<&'a BStr>) -> Option<Self> {
        let (section, rest) = key.split_once_str(".")?;
        if !section.eq_ignore_ascii_case(b"hook") {
            return None;
        }
        Some(match rest.rsplit_once_str(".") {
            Some((name, key)) => Entry {
                name: Some(name.as_bstr()),
                key: key.as_bstr(),
                value,
            },
            None => Entry {
                name: None,
                key: rest.as_bstr(),
                value,
            },
        })
    }
}

/// A hook that the configuration names for an event.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Named {
    name: BString,
    /// The command, which only a disabled hook may lack, as Git keeps disabled hooks without one.
    command: Option<BString>,
    disabled: bool,
}

/// The hooks that the configuration names, by event, collected as Git 2.55 does in `hook_config_lookup_all()` and
/// `build_hook_config_map()`.
///
/// - `hook.<name>.event` adds the hook `<name>` to the hooks of the event its value names. If the hook was added to
///   that event before, Git first removes it, putting the last hook of the event in its place, and then appends it.
///   An empty value removes the hook the same way from the events it was added to before.
/// - `hook.<name>.command` is the command of the hook, and the last value counts.
/// - `hook.<name>.enabled` set to false disables the hook, and set to true enables it again. Values that aren't
///   booleans are ignored.
/// - `hook.<event>.enabled` set to false disables all configured hooks of the event, unless `<event>` is the name of
///   a hook, which it is if it has a command, an event or `parallel` set to true.
/// - `hook.<name>.parallel`, `hook.<event>.jobs` and `hook.jobs` let Git run hooks at the same time, but hooks run
///   one after another here, so `parallel` only tells hooks from events.
///
/// Like Git, [`Configured::from_entries()`] fails for a hook with events but no command unless it's disabled, for an
/// event added to a hook named like one of the [known events](crate::KNOWN_EVENTS), and for a key without value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Configured {
    by_event: BTreeMap<BString, Vec<Named>>,
    disabled_events: BTreeSet<BString>,
}

impl Configured {
    /// Collect the hooks from `entries`, the `hook.*` values in the order Git reads them.
    pub fn from_entries<'a>(entries: impl IntoIterator<Item = Entry<'a>>) -> Result<Self> {
        let mut commands = BTreeMap::<&BStr, &BStr>::new();
        let mut events = BTreeMap::<&BStr, Vec<&BStr>>::new();
        let mut disabled = Vec::<&BStr>::new();
        let mut parallel = BTreeMap::<&BStr, bool>::new();
        for Entry { name, key, value } in entries {
            let Some(name) = name else {
                continue;
            };
            let Some(value) = value else {
                bail!("missing value for 'hook.{name}.{key}'".validation());
            };
            if key.eq_ignore_ascii_case(b"event") {
                if value.is_empty() {
                    for names in events.values_mut() {
                        remove(names, name);
                    }
                } else {
                    if is_known_event(name) {
                        bail!(
                            "hook friendly-name '{name}' collides with a known event name; please choose a different friendly-name"
                                .validation()
                        );
                    }
                    let names = events.entry(value).or_default();
                    remove(names, name);
                    names.push(name);
                }
            } else if key.eq_ignore_ascii_case(b"command") {
                commands.insert(name, value);
            } else if key.eq_ignore_ascii_case(b"enabled") {
                match maybe_bool(value) {
                    Some(false) if !disabled.contains(&name) => disabled.push(name),
                    Some(true) => remove(&mut disabled, name),
                    _ => {}
                }
            } else if key.eq_ignore_ascii_case(b"parallel")
                && let Some(value) = maybe_bool(value)
            {
                parallel.insert(name, value);
            }
        }

        let is_hook = |name: &BStr| {
            commands.contains_key(name)
                || parallel.get(name) == Some(&true)
                || events.values().any(|names| names.contains(&name))
        };
        let disabled_events = disabled
            .iter()
            .filter(|name| !is_hook(name))
            .map(|name| (*name).to_owned())
            .collect();
        let mut by_event = BTreeMap::new();
        for (event, names) in &events {
            let mut hooks = Vec::with_capacity(names.len());
            for &name in names {
                let is_disabled = disabled.contains(&name);
                let command = commands.get(name).map(|command| (*command).to_owned());
                if command.is_none() && !is_disabled {
                    bail!(
                        "'hook.{name}.command' must be configured or 'hook.{name}.event' must be removed; aborting."
                            .validation()
                    );
                }
                hooks.push(Named {
                    name: name.to_owned(),
                    command,
                    disabled: is_disabled,
                });
            }
            by_event.insert((*event).to_owned(), hooks);
        }
        Ok(Configured {
            by_event,
            disabled_events,
        })
    }

    /// Return `true` if the configuration names no hook for any event.
    pub fn is_empty(&self) -> bool {
        self.by_event.values().all(Vec::is_empty)
    }

    /// Add the hooks configured for `event` to `found`: those that run to its hooks, the others to the skipped ones.
    ///
    /// A hook that is disabled while its event is disabled as well is skipped for its event, which `git hook list`
    /// reports first.
    pub(crate) fn find(&self, event: &str, found: &mut Found) {
        let event = event.as_bytes().as_bstr();
        let Some(hooks) = self.by_event.get(event) else {
            return;
        };
        let event_disabled = self.disabled_events.contains(event);
        for hook in hooks {
            let name = hook.name.clone();
            if event_disabled {
                found.skipped.push(Skipped::EventDisabled { name });
            } else if hook.disabled {
                found.skipped.push(Skipped::Disabled { name });
            } else if let Some(command) = &hook.command {
                found.hooks.push(Hook::Command {
                    name,
                    command: command.clone(),
                });
            }
        }
    }
}

/// Remove the first `name` from `names` as Git's `unsorted_string_list_remove()` does, which puts the last name in its
/// place.
fn remove(names: &mut Vec<&BStr>, name: &BStr) {
    if let Some(pos) = names.iter().position(|candidate| *candidate == name) {
        names.swap_remove(pos);
    }
}

/// Parse `value` as Git's `git_parse_maybe_bool()` does, or return `None` if it isn't a boolean.
fn maybe_bool(value: &BStr) -> Option<bool> {
    gix_config_value::Boolean::try_from(value).ok().map(bool::from)
}
