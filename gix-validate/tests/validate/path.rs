#[test]
fn component_is_windows_device() {
    for device in ["con", "CONIN$", "lpt1.txt", "AUX", "Prn", "NUL", "COM9", "nul.a.b "] {
        assert!(gix_validate::path::component_is_windows_device(device.into()));
    }
    for not_device in ["coni", "CONIN", "lpt", "AUXi", "aPrn", "NULl", "COM", "a.nul.b "] {
        assert!(!gix_validate::path::component_is_windows_device(not_device.into()));
    }
}

mod component {
    use gix_validate::path::component;

    const NO_OPTS: component::Options = component::Options {
        protect_windows: false,
        protect_hfs: false,
        protect_ntfs: false,
    };
    const ALL_OPTS: component::Options = component::Options {
        protect_windows: true,
        protect_hfs: true,
        protect_ntfs: true,
    };
    const UNIX_OPTS: component::Options = component::Options {
        protect_windows: false,
        protect_hfs: true,
        protect_ntfs: true,
    };

    mod valid {
        use bstr::ByteSlice;
        use gix_validate::path::component::Mode::Symlink;

        use crate::path::component::{ALL_OPTS, NO_OPTS, UNIX_OPTS};
        macro_rules! mktest {
            ($name:ident, $input:expr) => {
                mktest!($name, $input, ALL_OPTS);
            };
            ($name:ident, $input:expr, $opts:expr) => {
                #[test]
                fn $name() {
                    assert!(gix_validate::path::component($input.as_bstr(), None, $opts).is_ok())
                }
            };
            ($name:ident, $input:expr, $mode:expr, $opts:expr) => {
                #[test]
                fn $name() {
                    assert!(gix_validate::path::component($input.as_bstr(), Some($mode), $opts).is_ok())
                }
            };
        }

        mktest!(ascii, b"ascii-only_and-that");
        mktest!(unicode, "😁👍👌".as_bytes());
        mktest!(backslashes_on_unix, br"\", UNIX_OPTS);
        mktest!(drive_letters_on_unix, b"c:", UNIX_OPTS);
        mktest!(virtual_drive_letters_on_unix, "֍:".as_bytes(), UNIX_OPTS);
        mktest!(unc_path_on_unix, br"\\?\pictures", UNIX_OPTS);
        mktest!(not_dot_git_longer, b".gitu", NO_OPTS);
        mktest!(not_dot_git_longer_all, b".gitu");
        mktest!(not_dot_gitmodules_shorter, b".gitmodule", Symlink, NO_OPTS);
        mktest!(not_dot_gitmodules_shorter_all, b".gitmodule", Symlink, ALL_OPTS);
        mktest!(not_dot_gitmodules_longer, b".gitmodulesa", Symlink, NO_OPTS);
        mktest!(not_dot_gitmodules_longer_all, b".gitmodulesa", Symlink, ALL_OPTS);
        mktest!(dot_gitmodules_as_file, b".gitmodules", UNIX_OPTS);
        mktest!(backslash_in_a_name_on_unix, br"a\b", UNIX_OPTS);
        mktest!(not_dot_git_after_a_backslash_on_unix, br"a\.gitu", UNIX_OPTS);
        mktest!(dot_gitmodules_as_file_after_a_backslash, br"a\.gitmodules", UNIX_OPTS);
        mktest!(not_dot_git_shorter, b".gi", NO_OPTS);
        mktest!(not_dot_git_shorter_ntfs_8_3, b"gi~1");
        mktest!(not_dot_git_longer_ntfs_8_3, b"gitu~1");
        mktest!(not_dot_git_shorter_ntfs_8_3_disabled, b"git~1", NO_OPTS);
        mktest!(not_dot_git_longer_hfs, ".g\u{200c}itu".as_bytes());
        mktest!(not_dot_git_shorter_hfs, ".g\u{200c}i".as_bytes());
        mktest!(com_0_lower, b"com0");
        mktest!(com_without_number_0_lower, b"comm");
        mktest!(conout_without_dollar_with_extension, b"conout.c");
        mktest!(conin_without_dollar_with_extension, b"conin.c");
        mktest!(conin_without_dollar, b"conin");
        mktest!(not_con, b"com");
        mktest!(also_not_con, b"co");
        mktest!(con_as_middle, b"x.CON.zip");
        mktest!(con_after_space, b" CON");
        mktest!(con_after_space_mixed, b" coN.tar.xz");
        mktest!(not_nul, b"null");
        mktest!(
            not_dot_gitmodules_shorter_hfs,
            ".gitm\u{200c}odule".as_bytes(),
            Symlink,
            UNIX_OPTS
        );
        mktest!(dot_gitmodules_as_file_hfs, ".g\u{200c}itmodules".as_bytes(), UNIX_OPTS);
        mktest!(dot_gitmodules_ntfs_8_3_disabled, b"gItMOD~1", Symlink, NO_OPTS);
        mktest!(
            not_dot_gitmodules_longer_hfs,
            "\u{200c}.gitmodulesa".as_bytes(),
            Symlink,
            UNIX_OPTS
        );
    }

    mod invalid {
        use bstr::ByteSlice;
        use gix_validate::path::component::{Error, Mode::Symlink};

        use crate::path::component::{ALL_OPTS, NO_OPTS, UNIX_OPTS};

        macro_rules! mktest {
            ($name:ident, $input:expr, $expected:pat, @$snapshot:literal) => {
                mktest!($name, $input, $expected, ALL_OPTS, @$snapshot);
            };
            ($name:ident, $input:expr, $expected:pat, $opts:expr, @$snapshot:literal) => {
                #[test]
                fn $name() {
                    let err = gix_validate::path::component($input.as_bstr(), None, $opts).expect_err("the input is invalid");
                    insta::assert_debug_snapshot!(err, "invalid path components retain their specific failure", @$snapshot);
                    assert!(matches!(err, $expected), "the failure retains its error variant");
                }
            };
            ($name:ident, $input:expr, $expected:pat, $mode:expr, $opts:expr, @$snapshot:literal) => {
                #[test]
                fn $name() {
                    let err = gix_validate::path::component($input.as_bstr(), Some($mode), $opts).expect_err("the input is invalid");
                    insta::assert_debug_snapshot!(err, "invalid path components retain their specific failure", @$snapshot);
                    assert!(matches!(err, $expected), "the failure retains its error variant");
                }
            };
        }

        mktest!(empty, b"", Error::Empty, @"Empty");
        mktest!(dot_git_lower, b".git", Error::DotGitDir, NO_OPTS, @"DotGitDir");
        mktest!(dot_git_lower_hfs, ".g\u{200c}it".as_bytes(), Error::DotGitDir, @"DotGitDir");
        mktest!(dot_git_mixed_hfs_simple, b".Git", Error::DotGitDir, @"DotGitDir");
        mktest!(dot_git_upper, b".GIT", Error::DotGitDir, NO_OPTS, @"DotGitDir");
        mktest!(
            starts_with_dot_git_with_backslashes_on_windows,
            br".git\hooks\pre-commit",
            Error::PathSeparator, @"PathSeparator");
        mktest!(
            starts_with_dot_git_with_backslashes_on_unix,
            br".git\hooks\pre-commit",
            Error::DotGitDir,
            UNIX_OPTS, @"DotGitDir");
        mktest!(dot_git_after_a_backslash_on_unix, br"a\.git", Error::DotGitDir, UNIX_OPTS, @"DotGitDir");
        mktest!(
            dot_gitmodules_symlink_after_a_backslash_on_unix,
            br"a\gitmod~1",
            Error::SymlinkedGitModules,
            Symlink,
            UNIX_OPTS, @"SymlinkedGitModules");
        mktest!(dot_git_upper_hfs, ".GIT\u{200e}".as_bytes(), Error::DotGitDir, @"DotGitDir");
        mktest!(dot_git_upper_ntfs_8_3, b"GIT~1", Error::DotGitDir, @"DotGitDir");
        mktest!(dot_git_mixed, b".gIt", Error::DotGitDir, NO_OPTS, @"DotGitDir");
        mktest!(dot_git_mixed_ntfs_8_3, b"gIt~1", Error::DotGitDir, @"DotGitDir");
        mktest!(
            dot_gitmodules_mixed,
            b".gItmodules",
            Error::SymlinkedGitModules,
            Symlink,
            NO_OPTS, @"SymlinkedGitModules");
        mktest!(dot_git_mixed_hfs, "\u{206e}.gIt".as_bytes(), Error::DotGitDir, @"DotGitDir");
        mktest!(
            dot_git_ntfs_8_3_numbers_only,
            b"~1000000",
            Error::SymlinkedGitModules,
            Symlink,
            ALL_OPTS, @"SymlinkedGitModules");
        mktest!(
            dot_git_ntfs_8_3_numbers_only_too,
            b"~9999999",
            Error::SymlinkedGitModules,
            Symlink,
            ALL_OPTS, @"SymlinkedGitModules");
        mktest!(
            dot_gitmodules_mixed_hfs,
            "\u{206e}.gItmodules".as_bytes(),
            Error::SymlinkedGitModules,
            Symlink,
            ALL_OPTS, @"SymlinkedGitModules");
        mktest!(
            dot_gitmodules_mixed_ntfs_8_3,
            b"gItMOD~1",
            Error::SymlinkedGitModules,
            Symlink,
            ALL_OPTS, @"SymlinkedGitModules");
        mktest!(
            dot_gitmodules_mixed_ntfs_stream,
            b".giTmodUles:$DATA",
            Error::SymlinkedGitModules,
            Symlink,
            ALL_OPTS, @"SymlinkedGitModules");
        mktest!(
            dot_gitmodules_lower_ntfs_stream_default_implicit,
            b".gitmodules::$DATA",
            Error::SymlinkedGitModules,
            Symlink,
            ALL_OPTS, @"SymlinkedGitModules");
        mktest!(
            ntfs_stream_default_implicit,
            b"file::$DATA",
            Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(
            ntfs_stream_explicit,
            b"file:ANYTHING_REALLY:$DATA",
            Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(
            dot_gitmodules_lower_ntfs_stream,
            b".gitmodules:$DATA:$DATA",
            Error::SymlinkedGitModules,
            Symlink,
            ALL_OPTS, @"SymlinkedGitModules");
        mktest!(
            not_gitmodules_trailing_space,
            b".gitmodules x ",
            Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(
            not_gitmodules_trailing_stream,
            b".gitmodules,:$DATA",
            Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(path_separator_slash_between, b"a/b", Error::PathSeparator, @"PathSeparator");
        mktest!(path_separator_slash_leading, b"/a", Error::PathSeparator, @"PathSeparator");
        mktest!(path_separator_slash_trailing, b"a/", Error::PathSeparator, @"PathSeparator");
        mktest!(path_separator_slash_only, b"/", Error::PathSeparator, @"PathSeparator");
        mktest!(slashes_on_windows, b"/", Error::PathSeparator, ALL_OPTS, @"PathSeparator");
        mktest!(backslashes_on_windows, br"\", Error::PathSeparator, ALL_OPTS, @"PathSeparator");
        mktest!(path_separator_backslash_between, br"a\b", Error::PathSeparator, @"PathSeparator");
        mktest!(path_separator_backslash_leading, br"\a", Error::PathSeparator, @"PathSeparator");
        mktest!(path_separator_backslash_trailing, br"a\", Error::PathSeparator, @"PathSeparator");
        mktest!(aux_mixed, b"Aux", Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(aux_with_extension, b"aux.c", Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(com_lower, b"com1", Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(com_upper_with_extension, b"COM9.c", Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(trailing_space, b"win32 ", Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(trailing_dot, b"win32.", Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(trailing_dot_dot, b"win32 . .", Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(colon_inbetween, b"colon:separates", Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(left_arrow, b"arrow<left", Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(right_arrow, b"arrow>right", Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(apostrophe, b"a\"b", Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(pipe, b"a|b", Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(questionmark, b"a?b", Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(asterisk, b"a*b", Error::WindowsIllegalCharacter, @"WindowsIllegalCharacter");
        mktest!(lpt_mixed_with_number, b"LPt8", Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(nul_mixed, b"NuL", Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(prn_mixed_with_extension, b"PrN.abc", Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(con, b"CON", Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(con_with_extension, b"CON.abc", Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(con_with_middle, b"CON.tar.xz", Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(con_mixed_with_middle, b"coN.tar.xz ", Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(dot_dot, b"..", Error::Relative, @"Relative");
        mktest!(dot_dot_no_opts, b"..", Error::Relative, NO_OPTS, @"Relative");
        mktest!(single_dot, b".", Error::Relative, @"Relative");
        mktest!(single_dot_no_opts, b".", Error::Relative, NO_OPTS, @"Relative");
        mktest!(
            conout_mixed_with_extension,
            b"ConOut$  .xyz",
            Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(conin_mixed, b"conIn$  ", Error::WindowsReservedName, @"WindowsReservedName");
        mktest!(drive_letters, b"c:", Error::WindowsPathPrefix, ALL_OPTS, @"WindowsPathPrefix");
        mktest!(
            virtual_drive_letters,
            "֍:".as_bytes(),
            Error::WindowsPathPrefix,
            ALL_OPTS, @"WindowsPathPrefix");
        mktest!(unc_net_path, br"\\host", Error::PathSeparator, ALL_OPTS, @"PathSeparator");
        mktest!(unc_path, br"\\?\pictures", Error::PathSeparator, ALL_OPTS, @"PathSeparator");
        mktest!(unc_device_path, br"\\.\pictures", Error::PathSeparator, ALL_OPTS, @"PathSeparator");
        mktest!(unc_nt_obj_path, br"\??\pictures", Error::PathSeparator, ALL_OPTS, @"PathSeparator");

        #[test]
        fn ntfs_gitmodules() {
            for invalid in [
                ".gitmodules",
                ".Gitmodules",
                ".gitmoduleS",
                ".gitmodules ",
                ".gitmodules.",
                ".gitmodules  ",
                ".gitmodules. ",
                ".gitmodules .",
                ".gitmodules..",
                ".gitmodules   ",
                ".gitmodules.  ",
                ".gitmodules . ",
                ".gitmodules  .",
                ".Gitmodules ",
                ".Gitmodules.",
                ".Gitmodules  ",
                ".Gitmodules. ",
                ".Gitmodules .",
                ".Gitmodules..",
                ".Gitmodules   ",
                ".Gitmodules.  ",
                ".Gitmodules . ",
                ".Gitmodules  .",
                "GITMOD~1",
                "gitmod~1",
                "GITMOD~2",
                "giTmod~3",
                "GITMOD~4",
                "GITMOD~1 ",
                "gitMod~2.",
                "GITMOD~3  ",
                "gitmod~4. ",
                "GITMoD~1 .",
                "gitmod~2   ",
                "GITMOD~3.  ",
                "gitmoD~4 . ",
                "GI7EBA~1",
                "gi7eba~9",
                "GI7EB~10",
                "GI7EB~11",
                "GI7EB~99",
                "GI7EB~10",
                "GI7E~100",
                "GI7E~101",
                "GI7E~999",
                ".gitmodules:$DATA",
                "gitmod~4 . :$DATA",
            ] {
                match gix_validate::path::component(invalid.into(), Some(Symlink), ALL_OPTS) {
                    Ok(_) => {
                        unreachable!("{invalid:?} should not validate successfully")
                    }
                    Err(err) => {
                        assert!(matches!(err, Error::SymlinkedGitModules));
                    }
                }
            }

            for valid in [
                ".gitmodules x",
                ".gitmodules .x",
                " .gitmodules",
                "..gitmodules",
                "gitmodules",
                ".gitmodule",
                ".gitmodules .x",
                "GI7EBA~",
                "GI7EBA~0",
                "GI7EBA~~1",
                "GI7EBA~X",
                "Gx7EBA~1",
                "GI7EBX~1",
                "GI7EB~1",
                "GI7EB~01",
                "GI7EB~1X",
            ] {
                gix_validate::path::component(valid.into(), Some(Symlink), ALL_OPTS)
                    .unwrap_or_else(|_| panic!("{valid:?} should have been valid"));
            }
        }
    }

    /// `git update-index` passes the names that it adds through `verify_path()`.
    #[test]
    fn names_with_backslashes_are_refused_as_git_refuses_them() -> gix_testtools::TestResult {
        const FILE: &str = "100644";
        const SYMLINK: &str = "120000";
        let cases = [
            (r".git\config", FILE),
            (r".GIT\config", FILE),
            (r".git \config", FILE),
            (r".git.\config", FILE),
            (r".git. .\config", FILE),
            (r"git~1\config", FILE),
            (r"GIT~1\config", FILE),
            (r"a\.git", FILE),
            (r"a\.git ", FILE),
            (r"a\.git.", FILE),
            (r"a\.git:stream", FILE),
            (r"a\git~1", FILE),
            (r"a\.git\config", FILE),
            (r"a\b\.git\config", FILE),
            (r"a\b", FILE),
            (r"a\.gitu", FILE),
            (r"a\.git u", FILE),
            (r".gitu\config", FILE),
            (r"git~2\config", FILE),
            (r"a\.gitmodules", FILE),
            (r"a\.gitmodules", SYMLINK),
            (r"a\.gitmodules ", SYMLINK),
            (r"a\.gitmodules:stream", SYMLINK),
            (r"a\gitmod~1", SYMLINK),
            (r"a\gi7eba~1", SYMLINK),
            (r".gitmodules\b", SYMLINK),
            (r"a\.gitmodules\b", SYMLINK),
        ];

        let dir = gix_testtools::tempfile::tempdir()?;
        gix_testtools::git(dir.path(), "init -q --object-format=sha1")?;
        for (name, mode) in cases {
            let git_accepts = gix_testtools::git_command(dir.path())
                .args(["-c", "core.protectNTFS=true", "-c", "core.protectHFS=false"])
                .args(["update-index", "--add", "--cacheinfo", mode])
                .args(["e69de29bb2d1d6434b8b29ae775ad8c2e48c5391", name])
                .output()?
                .status
                .success();
            let options = component::Options {
                protect_windows: cfg!(windows),
                protect_hfs: false,
                protect_ntfs: true,
            };
            let symlink = (mode == SYMLINK).then_some(component::Mode::Symlink);
            assert_eq!(
                component(name.into(), symlink, options).is_ok(),
                git_accepts,
                "{name:?} as {mode}"
            );
        }
        Ok(())
    }

    /// Under `core.protectHFS`, Git's `verify_path()` reads each component of the names that `git update-index` adds
    /// as `next_hfs_char()` does, where invalid UTF-8, U+FFFE and U+FFFF end a name. Some names aren't valid UTF-8, so
    /// they reach Git on its standard input, where it skips the names it refuses, and each goes to an index of its own
    /// to see whether Git added it.
    #[test]
    fn names_that_hfs_takes_for_dot_git_are_refused_as_git_refuses_them() -> gix_testtools::TestResult {
        use std::io::Write;

        use bstr::ByteSlice;

        const FILE: &str = "100644";
        const SYMLINK: &str = "120000";
        let cases: &[(&[u8], &str)] = &[
            (b".git\x80/config", FILE),
            (".git\u{fffe}/config".as_bytes(), FILE),
            (".git\u{ffff}/config".as_bytes(), FILE),
            (b".git\xc0\xae/config", FILE),
            (b".git\xed\xa0\x80/config", FILE),
            (b".git\xf4\x90\x80\x80/config", FILE),
            (b".git\xe2\x80/config", FILE),
            (b".GIT\x80/config", FILE),
            (".g\u{200c}it/config".as_bytes(), FILE),
            ("\u{feff}.git\u{200d}/config".as_bytes(), FILE),
            (".git\u{fffd}/config".as_bytes(), FILE),
            (b".gitx/config", FILE),
            (b".g\x80it/config", FILE),
            (b".gi\x80t/config", FILE),
            (b"\x80.git/config", FILE),
            (b".gitmodules\x80", FILE),
            (b".gitmodules\x80", SYMLINK),
            (".gitmodules\u{ffff}".as_bytes(), SYMLINK),
            (b".gitmodules\xc0\x80", SYMLINK),
            ("a/.gitmodules\u{fffe}".as_bytes(), SYMLINK),
            (".gitmodules\u{fffd}".as_bytes(), SYMLINK),
            (b".gitmodulesx", SYMLINK),
        ];

        let dir = gix_testtools::tempfile::tempdir()?;
        gix_testtools::git(dir.path(), "init -q --object-format=sha1")?;
        let options = component::Options {
            protect_windows: cfg!(windows),
            protect_hfs: true,
            protect_ntfs: false,
        };
        for (number, &(name, mode)) in cases.iter().enumerate() {
            let index = dir.path().join(format!("index-{number}"));
            let mut git = gix_testtools::git_command(dir.path())
                .env("GIT_INDEX_FILE", &index)
                .args(["-c", "core.protectHFS=true", "-c", "core.protectNTFS=false"])
                .args(["update-index", "--index-info"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            let mut line = format!("{mode} e69de29bb2d1d6434b8b29ae775ad8c2e48c5391\t").into_bytes();
            line.extend_from_slice(name);
            line.push(b'\n');
            git.stdin.take().expect("piped").write_all(&line)?;
            assert!(git.wait()?.success(), "Git skips the names it refuses");
            let listed = gix_testtools::git_command(dir.path())
                .env("GIT_INDEX_FILE", &index)
                .args(["ls-files", "-z"])
                .output()?;
            assert!(listed.status.success(), "the index can be read");
            let git_accepts = !listed.stdout.is_empty();
            if git_accepts {
                assert_eq!(
                    listed.stdout.as_bstr(),
                    [name, b"\0"].concat().as_bstr(),
                    "Git added the name as it is"
                );
            }

            let symlink = (mode == SYMLINK).then_some(component::Mode::Symlink);
            let gix_accepts = gix_validate::path::entry(name.as_bstr(), symlink, options).is_ok();
            assert_eq!(gix_accepts, git_accepts, "{:?} as {mode}", name.as_bstr());
        }
        Ok(())
    }
}

mod entry {
    use gix_validate::path::{component, entry};

    /// Git's `verify_path()` refuses a symbolic link whose path leads through a directory that it takes for `.gitmodules`:
    /// named so in any case, or so as HFS+ reads names, or so as NTFS reads them if a `:` ends the name, as Git's
    /// `is_ntfs_dotgitmodules()` ends a name only at the end of the path or at a `:`. A file there is fine. Each path goes
    /// to an index of its own, and Windows refuses some of them for their `:` or `\` alone.
    #[test]
    fn links_in_directories_named_like_gitmodules_are_refused_as_git_refuses_them() -> gix_testtools::TestResult {
        const FILE: &str = "100644";
        const SYMLINK: &str = "120000";
        let cases = [
            (".gitmodules/link", SYMLINK),
            (".gitmodules/file", FILE),
            ("a/.gitmodules/link", SYMLINK),
            (".GitModules/link", SYMLINK),
            ("gitmod~1/link", SYMLINK),
            ("gitmod~1/file", FILE),
            ("gitmod~1:x/link", SYMLINK),
            ("gitmod~1:x/file", FILE),
            ("gi7eba~9 . :x/link", SYMLINK),
            ("a\\gitmod~1:x/link", SYMLINK),
            (".gitmodules./link", SYMLINK),
            (".gi\u{200c}tmodules/link", SYMLINK),
            (".gitmodulesx/link", SYMLINK),
            ("a/.gitmodules", SYMLINK),
        ];

        let dir = gix_testtools::tempfile::tempdir()?;
        gix_testtools::git(dir.path(), "init -q --object-format=sha1")?;
        let options = component::Options {
            protect_windows: cfg!(windows),
            protect_hfs: true,
            protect_ntfs: true,
        };
        for (number, (path, mode)) in cases.into_iter().enumerate() {
            let git_accepts = gix_testtools::git_command(dir.path())
                .env("GIT_INDEX_FILE", dir.path().join(format!("index-{number}")))
                .args(["-c", "core.protectHFS=true", "-c", "core.protectNTFS=true"])
                .args(["update-index", "--add", "--cacheinfo", mode])
                .args(["e69de29bb2d1d6434b8b29ae775ad8c2e48c5391", path])
                .output()?
                .status
                .success();
            let symlink = (mode == SYMLINK).then_some(component::Mode::Symlink);
            assert_eq!(
                entry(path.into(), symlink, options).is_ok(),
                git_accepts,
                "{path:?} as {mode}"
            );
        }
        Ok(())
    }
}
