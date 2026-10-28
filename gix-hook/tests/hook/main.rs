//! Compare how `gix-hook` finds and starts hooks with what Git did in the scenarios of `make_hook_records.sh`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use bstr::{BString, ByteSlice};
use gix_hook::{
    Config, Context, Found, Hook, Hooks, Skipped,
    config::{Configured, Entry},
};
use gix_testtools::Result;

mod config;
mod find;
mod run;

/// Return the root of the fixture, where Git ran hooks that recorded how it started them, or `None` with Git before
/// 2.55, which `gix-hook` follows: 2.54 added `git hook list` and hooks in the configuration, and 2.55 refuses hooks
/// named like an event. The fixture has no archive, as its records depend on the Git of the test host.
fn fixture() -> Result<Option<PathBuf>> {
    let root = gix_testtools::scripted_fixture_read_only_with_git_version("make_hook_records.sh", |version| {
        version >= (2, 55, 0)
    })?;
    Ok(root.map(std::path::absolute).transpose()?)
}

/// Return the directory with what Git recorded in `scenario`.
fn scenario(root: &Path, scenario: &str) -> PathBuf {
    root.join("records").join(scenario)
}

/// Return the records of hooks in `dir` by file name, leaving out what the commands of a scenario printed, their exit
/// status, and what Git listed about it.
fn records(dir: &Path) -> Result<BTreeMap<String, BString>> {
    let mut records = BTreeMap::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "the names of records are UTF-8")?;
        if matches!(name.as_str(), "out" | "err" | "status" | "config") || name.starts_with("list-") {
            continue;
        }
        records.insert(name, std::fs::read(entry.path())?.into());
    }
    Ok(records)
}

/// Return the text in `path` without the newline that ends it.
fn read_text(path: &Path) -> Result<String> {
    Ok(std::fs::read_to_string(path)?.trim_end().to_owned())
}

/// Return the object the fixture wrote to `oids/<name>`.
fn oid(root: &Path, name: &str) -> Result<gix_hash::ObjectId> {
    Ok(gix_hash::ObjectId::from_hex(
        read_text(&root.join("oids").join(name))?.as_bytes(),
    )?)
}

/// Return `path` as hooks receive it in a variable of their environment, with slashes on Windows, as Git writes paths.
fn env_path(path: &Path) -> Result<String> {
    let path = path.to_str().ok_or("the paths of the fixture are UTF-8")?;
    Ok(if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    })
}

/// Return the hooks of the main worktree at `worktree`, with `config`.
fn hooks_with(worktree: &Path, config: Config<'_>) -> Result<Hooks> {
    let git_dir = worktree.join(".git");
    Ok(Hooks::new(
        Context {
            git_dir: &git_dir,
            common_dir: &git_dir,
            worktree: Some(worktree),
        },
        config,
    )?)
}

/// Return the hooks of the main worktree at `worktree`, without configuration.
fn hooks_of(worktree: &Path) -> Result<Hooks> {
    hooks_with(worktree, Config::default())
}

/// Return the `hook.*` values that `git config --get-regexp` wrote to `path` in Git's order, as `<key> <value>`, or
/// `<key>` for a key without value.
fn config_values(path: &Path) -> Result<Vec<(BString, Option<BString>)>> {
    Ok(std::fs::read(path)?
        .lines()
        .map(|line| match line.split_once_str(" ") {
            Some((key, value)) => (key.into(), Some(value.into())),
            None => (line.into(), None),
        })
        .collect())
}

/// Collect the configured hooks of the values that `git config --get-regexp` wrote to `path`.
fn configured(path: &Path) -> Result<Configured> {
    let values = config_values(path)?;
    Ok(Configured::from_entries(values.iter().filter_map(|(key, value)| {
        Entry::from_key(key.as_bstr(), value.as_ref().map(|value| value.as_bstr()))
    }))?)
}

/// Return the name of `hook` as Git's records and listings name it.
fn name_of(hook: &Hook) -> String {
    match hook {
        Hook::Program { path } => path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        Hook::Command { name, .. } => name.to_string(),
    }
}

/// Assert that `found` lists the hooks of `event` as `git hook list` did in `listing`: the configured hooks that run in
/// their order, followed by the hook of the hooks directory, and the configured hooks that don't run.
fn assert_listed_as_git(found: &Found, listing: &str, event: &str) {
    let mut running = Vec::new();
    let mut skipped = Vec::new();
    let mut from_hooks_dir = false;
    for line in listing.lines() {
        match line.split_once('\t') {
            Some(("disabled", name)) => skipped.push(Skipped::Disabled { name: name.into() }),
            Some(("event-disabled", name)) => skipped.push(Skipped::EventDisabled { name: name.into() }),
            _ if line == "hook from hookdir" => from_hooks_dir = true,
            _ => running.push(line.to_owned()),
        }
    }
    let configured: Vec<_> = found
        .hooks
        .iter()
        .filter(|hook| matches!(hook, Hook::Command { .. }))
        .map(name_of)
        .collect();
    assert_eq!(
        configured, running,
        "the configured hooks of {event} run in Git's order"
    );
    assert_eq!(
        matches!(found.hooks.last(), Some(Hook::Program { .. })),
        from_hooks_dir,
        "the program in the hooks directory runs last for {event}, if Git found it"
    );
    let not_running: Vec<_> = found
        .skipped
        .iter()
        .filter(|skipped| !matches!(skipped, Skipped::NotExecutable { .. }))
        .cloned()
        .collect();
    assert_eq!(
        not_running, skipped,
        "the configured hooks of {event} that don't run are skipped for the same reason as in Git"
    );
}
