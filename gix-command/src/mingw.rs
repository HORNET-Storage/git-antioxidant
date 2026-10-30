//! How Git for Windows finds and starts programs, as `mingw_spawnvpe()` and its helpers in Git's `compat/mingw.c` do.
//!
//! Windows can't start scripts, so Git for Windows reads the `#!` line of a program and starts the interpreter that it
//! names, which it looks up in `PATH`, with the script as argument. Nothing here depends on the platform.

use std::{
    ffi::{OsStr, OsString},
    io::Read,
    path::{Path, PathBuf},
};

use bstr::ByteSlice;

/// Return `true` if `name` ends in `.exe`, in any case.
pub fn is_exe(name: &OsStr) -> bool {
    let name = name.as_encoded_bytes();
    name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(b".exe")
}

/// Return the name of the interpreter of the script at `program` as `parse_interpreter()` does: the last component of
/// the path on its `#!` line, without arguments, or `None` if `program` ends in `.exe` or doesn't have such a line,
/// ended within its first 99 bytes.
pub fn interpreter(program: &Path) -> Option<OsString> {
    if is_exe(program.as_os_str()) {
        return None;
    }
    let mut buf = [0; 99];
    let len = std::fs::File::open(program).ok()?.read(&mut buf).ok()?;
    let buf = &buf[..len];
    if len < 4 || !buf.starts_with(b"#!") {
        return None;
    }
    let end = buf.iter().position(|byte| matches!(byte, b'\r' | b'\n' | 0))?;
    if buf[end] == 0 {
        return None;
    }
    let line = &buf[2..end];
    let start = line.rfind_byte(b'/').or_else(|| line.rfind_byte(b'\\'))? + 1;
    let name = &line[start..];
    let name = name.find_byte(b' ').map_or(name, |space| &name[..space]);
    gix_path::from_byte_slice(name)
        .ok()
        .map(|name| name.as_os_str().to_owned())
}

/// Find the program `name` as `path_lookup()` does: `name` is the path if it has a slash or backslash, and otherwise it
/// is looked up in the non-empty entries of `dirs`, in each first with `.exe` appended unless it ends in `.exe`, and
/// then as it is if that's not a directory and `exe_only` is `false` or it ends in `.exe`.
pub fn path_lookup(name: &OsStr, dirs: impl IntoIterator<Item = PathBuf>, exe_only: bool) -> Option<PathBuf> {
    if name.as_encoded_bytes().iter().any(|byte| matches!(byte, b'/' | b'\\')) {
        return Some(name.into());
    }
    let is_exe = is_exe(name);
    dirs.into_iter()
        .filter(|dir| !dir.as_os_str().is_empty())
        .find_map(|dir| {
            let path = dir.join(name);
            if !is_exe {
                let mut with_exe = path.clone().into_os_string();
                with_exe.push(".exe");
                let with_exe = PathBuf::from(with_exe);
                if with_exe.exists() {
                    return Some(with_exe);
                }
            }
            ((!exe_only || is_exe) && path.exists() && !path.is_dir()).then_some(path)
        })
}

/// Return `path` with slashes instead of backslashes, as Git for Windows names a script to its interpreter, so that the
/// script finds the files next to it through the path it receives as `$0`.
pub fn script_argument(path: &Path) -> OsString {
    match path.to_str() {
        Some(path) => path.replace('\\', "/").into(),
        None => path.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    /// Write `content` to a file in a new directory and return its interpreter.
    fn interpreter_of(name: &str, content: &[u8]) -> gix_testtools::Result<Option<OsString>> {
        let dir = gix_testtools::tempfile::tempdir()?;
        let path = dir.path().join(name);
        std::fs::write(&path, content)?;
        Ok(interpreter(&path))
    }

    #[test]
    fn interpreters_are_found_as_git_for_windows_finds_them() -> gix_testtools::Result {
        for (content, expected) in [
            (&b"#!/bin/sh\n"[..], Some("sh")),
            (b"#!/bin/sh\r\nmore", Some("sh")),
            (b"#! /usr/bin/env python\n", Some("env")),
            (b"#!/bin/sh -x -y\n", Some("sh")),
            (b"#!C:\\tools\\shell.exe\n", Some("shell.exe")),
            (b"#!/a\\b\n", Some("a\\b")),
            (b"#!sh\n", None),
            (b"#!/\n", Some("")),
            (b"#!/x", None),
            (b"#!/bin/sh", None),
            (b"#!/bin\0/sh\n", None),
            (b"# /bin/sh\n", None),
            (b"", None),
        ] {
            assert_eq!(
                interpreter_of("script", content)?.as_deref(),
                expected.map(OsStr::new),
                "{:?}",
                content.as_bstr()
            );
        }
        Ok(())
    }

    #[test]
    fn only_the_first_99_bytes_are_read_and_the_line_has_to_end_there() -> gix_testtools::Result {
        let line = |len: usize| {
            let mut line = b"#!/".to_vec();
            line.resize(len, b'x');
            line.push(b'\n');
            line
        };
        assert_eq!(
            interpreter_of("script", &line(98))?.as_deref(),
            Some(OsStr::new(&"x".repeat(95))),
            "the line ends in the last byte that's read"
        );
        assert_eq!(
            interpreter_of("script", &line(99))?,
            None,
            "the end of the line is one byte too far"
        );
        Ok(())
    }

    #[test]
    fn programs_ending_in_exe_are_never_scripts() -> gix_testtools::Result {
        for name in ["program.exe", "PROGRAM.EXE", "program.Exe", ".exe"] {
            assert_eq!(interpreter_of(name, b"#!/bin/sh\n")?, None, "{name}");
        }
        assert_eq!(
            interpreter_of("program.exe.sh", b"#!/bin/sh\n")?.as_deref(),
            Some(OsStr::new("sh"))
        );
        Ok(())
    }

    #[test]
    fn lookup_prefers_exe_files_of_earlier_directories_and_skips_what_it_may_not_use() -> gix_testtools::Result {
        let root = gix_testtools::tempfile::tempdir()?;
        for dir in ["a", "b", "c"] {
            std::fs::create_dir(root.path().join(dir))?;
        }
        std::fs::write(root.path().join("a").join("tool"), "")?;
        std::fs::create_dir(root.path().join("a").join("subdir"))?;
        std::fs::write(root.path().join("b").join("tool.exe"), "")?;
        std::fs::write(root.path().join("c").join("subdir"), "")?;
        std::fs::write(root.path().join("c").join("tool.sh.exe"), "")?;
        std::fs::write(root.path().join("a").join("tool.sh"), "")?;
        let dirs = ["a", "b", "c"].map(|dir| root.path().join(dir));
        let lookup = |name: &str, exe_only| path_lookup(OsStr::new(name), dirs.clone(), exe_only);

        assert_eq!(
            lookup("tool", false),
            Some(root.path().join("a").join("tool")),
            "each directory is searched in full before the next one"
        );
        assert_eq!(
            lookup("tool", true),
            Some(root.path().join("b").join("tool.exe")),
            "interpreters have to be programs"
        );
        assert_eq!(
            lookup("tool.exe", true),
            Some(root.path().join("b").join("tool.exe")),
            "nothing is appended to a name that ends in `.exe`"
        );
        assert_eq!(
            lookup("tool.sh", false),
            Some(root.path().join("a").join("tool.sh")),
            "the first directory with the file wins, with or without `.exe`"
        );
        assert_eq!(
            lookup("subdir", false),
            Some(root.path().join("c").join("subdir")),
            "directories don't count"
        );
        assert_eq!(lookup("missing", false), None);
        assert_eq!(
            lookup("a/tool", true),
            Some("a/tool".into()),
            "with a slash, the name is the path"
        );
        assert_eq!(
            lookup(r"a\tool", true),
            Some(r"a\tool".into()),
            "with a backslash, the name is the path"
        );
        assert_eq!(
            path_lookup(OsStr::new("Cargo.toml"), [PathBuf::new()], false),
            None,
            "empty entries don't stand for the current directory"
        );
        Ok(())
    }

    #[test]
    fn scripts_are_named_with_slashes() {
        assert_eq!(
            script_argument(Path::new(r"C:\dir\script")),
            OsStr::new("C:/dir/script")
        );
        assert_eq!(script_argument(Path::new("dir/script")), OsStr::new("dir/script"));
    }
}
