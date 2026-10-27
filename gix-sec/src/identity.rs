use std::path::Path;

#[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
/// An account based identity
pub struct Account {
    /// The user's name
    pub username: String,
    /// The user's password
    pub password: String,
    /// An OAuth refresh token that may accompany the password. It is to be treated confidentially, just like the password.
    pub oauth_refresh_token: Option<String>,
}

/// Who owns a path, as determined by [`path_ownership()`] with the rules Git uses to decide whether a repository is
/// owned by the current user.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Ownership {
    /// The path is owned by the current user, or by an owner that Git treats like the current user.
    ///
    /// On Unix, this includes paths owned by `root` if the current user is `root`, and paths owned by the user in
    /// `SUDO_UID` if the current user is `root`. On Windows, this includes paths owned by the Administrators group if
    /// the current user is a member, even if only through the elevated token linked to a limited token, as well as
    /// the directory Git for Windows uses as home directory.
    CurrentUser,
    /// The path is owned by another user or group.
    OtherUser {
        /// A description of the owner, like `uid 1001` on Unix or `DOMAIN/name (S-1-5-21-…)` on Windows.
        owner: String,
        /// A description of the current user in the same format as `owner`.
        current_user: String,
    },
    /// The path is on a file system that doesn't record ownership, like FAT32 on Windows.
    NotRecorded,
    /// The owner of the path couldn't be determined, for instance because the path is on a network share that doesn't
    /// provide it, or because its metadata can't be read.
    Unknown {
        /// Why the owner couldn't be determined.
        reason: String,
    },
}

impl Ownership {
    /// Return `true` if the path is owned by the current user.
    pub fn is_current_user(&self) -> bool {
        matches!(self, Ownership::CurrentUser)
    }
}

/// Complete a sentence that starts with the path, like `'/a/b' ` followed by this display.
impl std::fmt::Display for Ownership {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ownership::CurrentUser => f.write_str("is owned by the current user"),
            Ownership::OtherUser { owner, current_user } => {
                write!(f, "is owned by {owner}, but the current user is {current_user}")
            }
            Ownership::NotRecorded => f.write_str("is on a file system that does not record ownership"),
            Ownership::Unknown { reason } => write!(f, "has an owner that could not be determined: {reason}"),
        }
    }
}

/// Returns true if the given `path` is owned by the user who is executing the current process.
///
/// This is [`path_ownership()`] reduced to [`Ownership::is_current_user()`], see there for details.
pub fn is_path_owned_by_current_user(path: &Path) -> std::io::Result<bool> {
    path_ownership(path).map(|ownership| ownership.is_current_user())
}

/// Determine who owns `path` as seen by the current process, with the rules Git uses to decide whether a repository
/// is owned by the current user.
///
/// * On Unix, `path` itself is inspected without following a symbolic link. A process running as `root` owns paths
///   owned by `root`, and otherwise acts as the user whose id is in `SUDO_UID`, which is ignored for all other users.
/// * On Windows, the owner of `path` is compared with the user of the current thread or process. Paths owned by the
///   Administrators group are owned if the current user is a member of it, if necessary through the elevated token
///   linked to a limited token. The directory Git for Windows uses as home directory is owned as well: `HOME`, or if
///   that isn't set, `HOMEDRIVE` followed by `HOMEPATH` if that is a directory, or `USERPROFILE` otherwise. Its value
///   must match the resolved `path` with backslashes as separators, ignoring only the case of ASCII letters.
/// * On WASI, which has no notion of users, all paths are owned.
///
/// Fail with [`std::io::ErrorKind::NotFound`] if `path` doesn't exist. Any other failure to obtain the owner is
/// reported as [`Ownership::Unknown`], which doesn't count as owned, as in Git.
pub fn path_ownership(path: &Path) -> std::io::Result<Ownership> {
    impl_::path_ownership(path)
}

/// Git's rules for ownership on Unix, separated from system calls so they can be tested on every platform.
#[cfg_attr(any(windows, target_os = "wasi"), allow(dead_code))]
mod unix {
    use super::Ownership;

    /// The user id of `root`.
    const ROOT_UID: u32 = 0;

    /// Return the ownership of a path owned by user id `owner` for a process running as effective user id `euid`, with
    /// `sudo_uid` being the value of the `SUDO_UID` environment variable if it is set.
    ///
    /// `root` owns the paths owned by `root`, and otherwise acts as the user in `SUDO_UID`, which is ignored for all
    /// other users. `unsigned_long_max` is the largest value of C's `unsigned long`, which bounds `SUDO_UID`.
    pub(super) fn ownership(owner: u32, euid: u32, sudo_uid: Option<&[u8]>, unsigned_long_max: u64) -> Ownership {
        let mut current_user = euid;
        if euid == ROOT_UID {
            if owner == ROOT_UID {
                return Ownership::CurrentUser;
            }
            if let Some(uid) = sudo_uid.and_then(|value| parse_uid(value, unsigned_long_max)) {
                current_user = uid;
            }
        }
        if owner == current_user {
            Ownership::CurrentUser
        } else {
            Ownership::OtherUser {
                owner: format!("uid {owner}"),
                current_user: format!("uid {current_user}"),
            }
        }
    }

    /// Parse `value` like Git parses `SUDO_UID` with `strtoul()`, or return `None` if Git would ignore it.
    ///
    /// After optional C whitespace and an optional sign, `value` must consist of decimal digits whose number fits into
    /// `unsigned_long_max`. A minus sign negates the number modulo `unsigned_long_max + 1`, and the result is truncated
    /// to the 32 bits of a user id.
    pub(super) fn parse_uid(value: &[u8], unsigned_long_max: u64) -> Option<u32> {
        let is_c_space = |byte: &u8| matches!(byte, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r');
        let value = &value[value.iter().position(|byte| !is_c_space(byte))?..];
        let (is_negative, digits) = match value.split_first() {
            Some((b'-', digits)) => (true, digits),
            Some((b'+', digits)) => (false, digits),
            _ => (false, value),
        };
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let number = digits.iter().try_fold(0_u64, |number, digit| {
            number.checked_mul(10)?.checked_add(u64::from(digit - b'0'))
        })?;
        if number > unsigned_long_max {
            return None;
        }
        let number = if is_negative {
            number.wrapping_neg() & unsigned_long_max
        } else {
            number
        };
        Some(number as u32)
    }

    /// Return the ownership of a path whose metadata can't be read due to `err`. Like Git, consider such paths not owned,
    /// but report a missing path as error, which callers distinguish.
    pub(super) fn metadata_failure(err: std::io::Error) -> std::io::Result<Ownership> {
        if err.kind() == std::io::ErrorKind::NotFound {
            Err(err)
        } else {
            Ok(Ownership::Unknown {
                reason: err.to_string(),
            })
        }
    }
}

/// Git for Windows' rules for ownership, separated from system calls so they can be tested on every platform.
#[cfg_attr(not(windows), allow(dead_code))]
mod windows {
    use std::{borrow::Cow, io, path::Path};

    use super::Ownership;

    /// The number of UTF-16 code units Git for Windows provides to read `HOME`, including the terminating null.
    const MAX_PATH: usize = 260;

    /// Return the home directory as Git for Windows sets `HOME` when it starts: `home` if set, otherwise `home_drive`
    /// followed by `home_path` if `is_dir()` confirms that they form a directory, or `user_profile` otherwise.
    ///
    /// Return `None` if there is no home directory, or if it's empty or too long for Git for Windows to read it.
    pub(super) fn home_directory(
        home: Option<String>,
        home_drive: Option<String>,
        home_path: Option<String>,
        user_profile: Option<String>,
        is_dir: impl FnOnce(&str) -> bool,
    ) -> Option<String> {
        let home = match home {
            Some(home) => home,
            None => match home_drive.zip(home_path).map(|(drive, path)| drive + &path) {
                Some(home) if is_dir(&home) => home,
                _ => user_profile?,
            },
        };
        let len = home.encode_utf16().count();
        (len != 0 && len < MAX_PATH).then_some(home)
    }

    /// Spell the resolved `path` like Git for Windows when it compares it with the home directory, which is without the
    /// `\\?\` prefix of verbatim paths and with `\\server\share` instead of `\\?\UNC\server\share`.
    pub(super) fn git_spelling(path: &str) -> Cow<'_, str> {
        if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
            Cow::Owned(format!(r"\\{unc}"))
        } else {
            Cow::Borrowed(path.strip_prefix(r"\\?\").unwrap_or(path))
        }
    }

    /// Return `true` if `path`, spelled as by [`git_spelling()`], is the `home` directory, ignoring only the case of ASCII
    /// letters like `wcsicmp()` does in the C locale.
    pub(super) fn is_home(path: &str, home: &str) -> bool {
        path.eq_ignore_ascii_case(home)
    }

    /// The owner of a path, as far as Git for Windows tells owners apart.
    pub(super) enum Owner {
        /// The current user.
        CurrentUser,
        /// The Administrators group.
        Administrators,
        /// The World group, which file systems without access control lists like FAT report for every path.
        World,
        /// Any other user or group.
        Other,
    }

    /// Return the ownership of a path owned by `owner`, like Git for Windows' `is_path_owned_by_current_sid()`.
    ///
    /// Paths of the Administrators group are owned if `is_administrator()` confirms that the current user is a member,
    /// and paths of the World group are on a file system that doesn't record ownership unless `acls_supported()` says
    /// otherwise. `describe()` provides the owner and the current user of paths that aren't owned. Each is only called if
    /// the decision depends on it.
    pub(super) fn ownership(
        owner: Owner,
        is_administrator: impl FnOnce() -> bool,
        acls_supported: impl FnOnce() -> bool,
        describe: impl FnOnce() -> (String, String),
    ) -> Ownership {
        match owner {
            Owner::CurrentUser => Ownership::CurrentUser,
            Owner::Administrators if is_administrator() => Ownership::CurrentUser,
            Owner::World if !acls_supported() => Ownership::NotRecorded,
            Owner::Administrators | Owner::World | Owner::Other => {
                let (owner, current_user) = describe();
                Ownership::OtherUser { owner, current_user }
            }
        }
    }

    /// Return the ownership of `path` if `GetNamedSecurityInfoW()` failed with error `code`. Like Git for Windows,
    /// consider such paths not owned, which happens on network shares, but report a missing path as error, which callers
    /// distinguish.
    pub(super) fn query_failure(code: u32, path: &Path) -> io::Result<Ownership> {
        /// `ERROR_FILE_NOT_FOUND` and `ERROR_PATH_NOT_FOUND`.
        const MISSING: [u32; 2] = [2, 3];
        if MISSING.contains(&code) {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("\"{}\" does not exist.", path.display()),
            ))
        } else {
            let err = io::Error::from_raw_os_error(code as i32);
            Ok(Ownership::Unknown {
                reason: format!("failed to get the owner: {err}"),
            })
        }
    }
}

// Wasi doesn't have a concept of a user, so this is implicitly true.
#[cfg(target_os = "wasi")]
mod impl_ {
    pub fn path_ownership(_path: &std::path::Path) -> std::io::Result<super::Ownership> {
        Ok(super::Ownership::CurrentUser)
    }
}

#[cfg(all(not(windows), not(target_os = "wasi")))]
mod impl_ {
    use std::{os::unix::ffi::OsStrExt, path::Path};

    use super::Ownership;

    pub fn path_ownership(path: &Path) -> std::io::Result<Ownership> {
        use std::os::unix::fs::MetadataExt;

        let owner = match std::fs::symlink_metadata(path) {
            Ok(meta) => meta.uid(),
            Err(err) => return super::unix::metadata_failure(err),
        };
        // SAFETY: there is no documented possibility for failure
        #[expect(unsafe_code)]
        let euid = unsafe { libc::geteuid() };
        let sudo_uid = std::env::var_os("SUDO_UID");
        #[allow(
            clippy::useless_conversion,
            reason = "C's `unsigned long` is narrower than `u64` on some platforms"
        )]
        let unsigned_long_max = libc::c_ulong::MAX.into();
        Ok(super::unix::ownership(
            owner,
            euid,
            sudo_uid.as_deref().map(OsStrExt::as_bytes),
            unsigned_long_max,
        ))
    }
}

#[cfg(windows)]
#[expect(unsafe_code, reason = "Windows ownership requires Win32 security APIs")]
mod impl_ {
    use std::{
        io,
        mem::{self, MaybeUninit},
        os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle},
        path::{Component, Path, PathBuf},
        ptr,
    };

    use windows_sys::Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, GetLastError, HANDLE, LocalFree},
        Security::{
            Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT},
            CheckTokenMembership, DACL_SECURITY_INFORMATION, EqualSid, GetTokenInformation, IsValidSid, IsWellKnownSid,
            LookupAccountSidW, OWNER_SECURITY_INFORMATION, PSID, TOKEN_ELEVATION_TYPE, TOKEN_INFORMATION_CLASS,
            TOKEN_LINKED_TOKEN, TOKEN_QUERY, TOKEN_USER, TokenElevationType, TokenElevationTypeLimited,
            TokenLinkedToken, TokenUser, WinBuiltinAdministratorsSid, WinWorldSid,
        },
        Storage::FileSystem::GetVolumeInformationW,
        System::Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
    };

    use super::Ownership;

    /// The file system flag indicating that ownership is recorded, defined here to avoid the large
    /// `Win32_System_SystemServices` feature of `windows-sys`.
    const FILE_PERSISTENT_ACLS: u32 = 0x0000_0008;

    pub fn path_ownership(path: &Path) -> io::Result<Ownership> {
        // Git for Windows considers its home directory owned by the current user, even though it's typically owned
        // by the Administrators group.
        if is_home_directory(path) {
            return Ok(Ownership::CurrentUser);
        }

        let wide_path = to_wide_path(path);
        let mut owner: PSID = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        // SAFETY: `wide_path` is null-terminated, and all other pointers are null or valid for writes.
        let result = unsafe {
            GetNamedSecurityInfoW(
                wide_path.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        // `owner` points into the descriptor, which must outlive it.
        let _descriptor = LocalMemory(descriptor);
        if result != ERROR_SUCCESS {
            return super::windows::query_failure(result, path);
        }
        // SAFETY: `owner` is null or points to a security identifier.
        if owner.is_null() || unsafe { IsValidSid(owner) } == 0 {
            return Ok(Ownership::Unknown {
                reason: "the owner is not a valid security identifier".into(),
            });
        }

        let token = current_token();
        // The security identifier of the current user points into this buffer.
        let token_user = token.as_ref().and_then(|token| token_information(token, TokenUser));
        let current_user = token_user.as_ref().map(|buf| {
            // SAFETY: the buffer holds a `TOKEN_USER`, which may be unaligned in it.
            unsafe { ptr::read_unaligned(buf.as_ptr().cast::<TOKEN_USER>()) }
                .User
                .Sid
        });
        // SAFETY: `current_user` is only compared once it's known to be a valid security identifier, like `owner`.
        let is_current_user = current_user
            .is_some_and(|current_user| unsafe { IsValidSid(current_user) != 0 && EqualSid(owner, current_user) != 0 });
        // SAFETY: `owner` is a valid security identifier.
        let owner_kind = if is_current_user {
            super::windows::Owner::CurrentUser
        } else if unsafe { IsWellKnownSid(owner, WinBuiltinAdministratorsSid) } != 0 {
            super::windows::Owner::Administrators
        } else if unsafe { IsWellKnownSid(owner, WinWorldSid) } != 0 {
            super::windows::Owner::World
        } else {
            super::windows::Owner::Other
        };
        Ok(super::windows::ownership(
            owner_kind,
            || is_administrator(token.as_ref(), owner),
            || acls_supported(path),
            || {
                let current_user = match current_user {
                    None => "(none)".into(),
                    // SAFETY: `current_user` points to a security identifier.
                    Some(current_user) if unsafe { IsValidSid(current_user) } == 0 => "(invalid)".into(),
                    Some(current_user) => describe(current_user),
                };
                (describe(owner), current_user)
            },
        ))
    }

    /// Return `true` if `path` is the directory that Git for Windows uses as home directory.
    fn is_home_directory(path: &Path) -> bool {
        // A value that isn't valid Unicode can't match a path we can compare, so it counts as set, but empty.
        let var = |name: &str| std::env::var_os(name).map(|value| value.into_string().unwrap_or_default());
        let Some(home) = super::windows::home_directory(
            var("HOME"),
            var("HOMEDRIVE"),
            var("HOMEPATH"),
            var("USERPROFILE"),
            |dir| Path::new(dir).is_dir(),
        ) else {
            return false;
        };
        std::fs::canonicalize(path)
            .ok()
            .and_then(|path| path.into_os_string().into_string().ok())
            .is_some_and(|path| super::windows::is_home(&super::windows::git_spelling(&path), &home))
    }

    /// Open the token of the current thread if it impersonates a user, or the token of the current process otherwise.
    fn current_token() -> Option<OwnedHandle> {
        let mut token = ptr::null_mut();
        // SAFETY: `token` is valid for writes, and we own the handle if one of the calls succeeds.
        unsafe {
            if OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) == 0
                && OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0
            {
                return None;
            }
            Some(OwnedHandle::from_raw_handle(token))
        }
    }

    /// Read token information of variable size from `token`, or return `None` if that fails.
    fn token_information(token: &OwnedHandle, class: TOKEN_INFORMATION_CLASS) -> Option<Vec<u8>> {
        let mut buf = vec![0; 64];
        loop {
            let mut required_size = 0;
            // SAFETY: `buf` is valid for writes of its length.
            let success = unsafe {
                GetTokenInformation(
                    token.as_raw_handle(),
                    class,
                    buf.as_mut_ptr().cast(),
                    buf.len() as u32,
                    &mut required_size,
                )
            } != 0;
            if success {
                return Some(buf);
            }
            // SAFETY: there is no documented possibility for failure
            if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || required_size as usize <= buf.len() {
                return None;
            }
            buf.resize(required_size as usize, 0);
        }
    }

    /// Read a fixed-size token information record of type `T` from `token`, or return `None` if that fails.
    fn fixed_size_token_information<T: Copy>(token: &OwnedHandle, class: TOKEN_INFORMATION_CLASS) -> Option<T> {
        let mut info = MaybeUninit::<T>::uninit();
        let mut returned_size = 0;
        // SAFETY: `info` is valid for writes of the size of `T`, and initialized if the call succeeds.
        unsafe {
            (GetTokenInformation(
                token.as_raw_handle(),
                class,
                info.as_mut_ptr().cast(),
                mem::size_of::<T>() as u32,
                &mut returned_size,
            ) != 0)
                .then(|| info.assume_init())
        }
    }

    /// Return the elevated token linked to `token` if it's the limited token of an administrator.
    fn linked_token(token: &OwnedHandle) -> Option<OwnedHandle> {
        let elevation_type = fixed_size_token_information::<TOKEN_ELEVATION_TYPE>(token, TokenElevationType)?;
        if elevation_type != TokenElevationTypeLimited {
            return None;
        }
        let linked = fixed_size_token_information::<TOKEN_LINKED_TOKEN>(token, TokenLinkedToken)?;
        // SAFETY: the linked token is a new handle that we own.
        Some(unsafe { OwnedHandle::from_raw_handle(linked.LinkedToken) })
    }

    /// Return `true` if `token`, or the token of the current thread if it's null, has `sid` as enabled group.
    fn is_member(token: HANDLE, sid: PSID) -> bool {
        let mut is_member = 0;
        // SAFETY: `sid` is a valid security identifier, and `is_member` is valid for writes.
        unsafe { CheckTokenMembership(token, sid, &mut is_member) != 0 && is_member != 0 }
    }

    /// Return `true` if the current user is a member of the `administrators` group, directly or through the elevated
    /// token linked to the limited `token` it runs with.
    fn is_administrator(token: Option<&OwnedHandle>, administrators: PSID) -> bool {
        is_member(ptr::null_mut(), administrators)
            || token
                .and_then(linked_token)
                .is_some_and(|linked| is_member(linked.as_raw_handle(), administrators))
    }

    /// Return `true` if the volume containing `path` records access control lists, and with them ownership.
    fn acls_supported(path: &Path) -> bool {
        let Ok(path) = std::path::absolute(path) else {
            return false;
        };
        let mut components = path.components();
        let (Some(Component::Prefix(prefix)), Some(Component::RootDir)) = (components.next(), components.next()) else {
            return false;
        };
        let mut root = PathBuf::from(prefix.as_os_str());
        root.push(Component::RootDir);
        let mut flags = 0;
        // SAFETY: `root` is null-terminated, and all other pointers are null or valid for writes.
        let success = unsafe {
            GetVolumeInformationW(
                to_wide_path(&root).as_ptr(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut flags,
                ptr::null_mut(),
                0,
            )
        } != 0;
        success && flags & FILE_PERSISTENT_ACLS != 0
    }

    /// Describe `sid` like Git for Windows does, as `DOMAIN/name (S-1-…)`.
    fn describe(sid: PSID) -> String {
        format!(
            "{} ({})",
            account_name(sid).unwrap_or_else(|| "(inconvertible)".into()),
            sid_string(sid).unwrap_or_else(|| "(inconvertible)".into())
        )
    }

    fn account_name(sid: PSID) -> Option<String> {
        let (mut name_len, mut domain_len, mut sid_use) = (0, 0, 0);
        // SAFETY: without buffers, this only obtains their required sizes.
        unsafe {
            LookupAccountSidW(
                ptr::null(),
                sid,
                ptr::null_mut(),
                &mut name_len,
                ptr::null_mut(),
                &mut domain_len,
                &mut sid_use,
            )
        };
        let mut name = vec![0_u16; name_len as usize];
        let mut domain = vec![0_u16; domain_len as usize];
        // SAFETY: the buffers are valid for writes of the sizes passed along with them.
        let success = unsafe {
            LookupAccountSidW(
                ptr::null(),
                sid,
                name.as_mut_ptr(),
                &mut name_len,
                domain.as_mut_ptr(),
                &mut domain_len,
                &mut sid_use,
            )
        } != 0;
        success.then(|| {
            format!(
                "{}/{}",
                String::from_utf16_lossy(&domain[..(domain_len as usize).min(domain.len())]),
                String::from_utf16_lossy(&name[..(name_len as usize).min(name.len())])
            )
        })
    }

    fn sid_string(sid: PSID) -> Option<String> {
        let mut string = ptr::null_mut();
        // SAFETY: on success, `string` is a null-terminated string that we must free.
        unsafe {
            if ConvertSidToStringSidW(sid, &mut string) == 0 {
                return None;
            }
            let _string = LocalMemory(string.cast());
            let mut len = 0;
            while *string.add(len) != 0 {
                len += 1;
            }
            Some(String::from_utf16_lossy(std::slice::from_raw_parts(string, len)))
        }
    }

    /// Memory that the system allocated for us, which is freed with `LocalFree()` when dropped.
    struct LocalMemory(*mut core::ffi::c_void);

    impl Drop for LocalMemory {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the memory was allocated by the system to be freed by us.
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }

    fn to_wide_path(path: impl AsRef<Path>) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        let mut wide_path: Vec<_> = path.as_ref().as_os_str().encode_wide().collect();
        wide_path.push(0);
        wide_path
    }
}

#[cfg(test)]
mod tests {
    mod unix {
        use crate::identity::{Ownership, unix::*};

        const ULONG_MAX: u64 = u64::MAX;

        fn other(owner: u32, current_user: u32) -> Ownership {
            Ownership::OtherUser {
                owner: format!("uid {owner}"),
                current_user: format!("uid {current_user}"),
            }
        }

        #[test]
        fn regular_users_own_their_paths_and_ignore_sudo_uid() {
            assert_eq!(ownership(1000, 1000, None, ULONG_MAX), Ownership::CurrentUser);
            assert_eq!(ownership(1001, 1000, None, ULONG_MAX), other(1001, 1000));
            assert_eq!(
                ownership(1001, 1000, Some(b"1001"), ULONG_MAX),
                other(1001, 1000),
                "only root acts as the user in SUDO_UID"
            );
            assert_eq!(ownership(0, 1000, None, ULONG_MAX), other(0, 1000));
        }

        #[test]
        fn root_owns_root_paths_and_otherwise_acts_as_the_sudo_user() {
            assert_eq!(ownership(0, 0, None, ULONG_MAX), Ownership::CurrentUser);
            assert_eq!(
                ownership(0, 0, Some(b"1000"), ULONG_MAX),
                Ownership::CurrentUser,
                "root keeps owning root-owned paths under sudo"
            );
            assert_eq!(ownership(1000, 0, Some(b"1000"), ULONG_MAX), Ownership::CurrentUser);
            assert_eq!(ownership(1001, 0, Some(b"1000"), ULONG_MAX), other(1001, 1000));
            assert_eq!(ownership(1000, 0, None, ULONG_MAX), other(1000, 0));
            assert_eq!(
                ownership(1000, 0, Some(b"x"), ULONG_MAX),
                other(1000, 0),
                "an invalid SUDO_UID is ignored"
            );
        }

        #[test]
        fn sudo_uid_is_parsed_like_strtoul() {
            for (value, expected) in [
                (&b"1000"[..], Some(1000)),
                (b"0", Some(0)),
                (b"+1000", Some(1000)),
                (b" \t\n\x0b\x0c\r1000", Some(1000)),
                (b"-1", Some(u32::MAX)),
                (b"-0", Some(0)),
                (b"4294967296", Some(0)),
                (b"4294968296", Some(1000)),
                (b"18446744073709551615", Some(u32::MAX)),
                (b"", None),
                (b" ", None),
                (b"+", None),
                (b"-", None),
                (b"1000 ", None),
                (b"10x", None),
                (b"0x10", None),
                (b"+-1", None),
                (b"18446744073709551616", None),
                (b"99999999999999999999999", None),
            ] {
                assert_eq!(
                    parse_uid(value, ULONG_MAX),
                    expected,
                    "{:?}",
                    value.escape_ascii().to_string()
                );
            }
        }

        #[test]
        fn sudo_uid_is_bounded_by_the_width_of_unsigned_long() {
            let ulong_max = u64::from(u32::MAX);
            assert_eq!(parse_uid(b"4294967295", ulong_max), Some(u32::MAX));
            assert_eq!(parse_uid(b"4294967296", ulong_max), None, "out of range");
            assert_eq!(parse_uid(b"-1", ulong_max), Some(u32::MAX));
            assert_eq!(parse_uid(b"-4294967295", ulong_max), Some(1));
        }

        #[test]
        fn unreadable_metadata_means_the_owner_is_unknown() -> std::io::Result<()> {
            let denied = metadata_failure(std::io::ErrorKind::PermissionDenied.into())?;
            assert!(matches!(denied, Ownership::Unknown { .. }), "{denied:?}");
            assert!(!denied.is_current_user(), "like Git, such paths aren't owned");
            let missing = metadata_failure(std::io::ErrorKind::NotFound.into());
            assert!(
                missing.is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound),
                "only missing paths are errors"
            );
            Ok(())
        }
    }

    mod windows {
        use crate::identity::{Ownership, windows::*};

        fn some(value: &str) -> Option<String> {
            Some(value.into())
        }

        #[test]
        fn home_directory_prefers_home_then_home_drive_and_path_then_user_profile() {
            let unreachable = |_: &str| -> bool { unreachable!("HOME is used as is") };
            assert_eq!(
                home_directory(
                    some(r"D:\home"),
                    some("C:"),
                    some(r"\Users\a"),
                    some(r"C:\Users\b"),
                    unreachable
                ),
                some(r"D:\home")
            );
            assert_eq!(
                home_directory(None, some("C:"), some(r"\Users\a"), some(r"C:\Users\b"), |dir| {
                    assert_eq!(dir, r"C:\Users\a");
                    true
                }),
                some(r"C:\Users\a")
            );
            assert_eq!(
                home_directory(None, some("C:"), some(r"\Users\a"), some(r"C:\Users\b"), |_| false),
                some(r"C:\Users\b"),
                "a home share that isn't a directory falls back to the user profile"
            );
            assert_eq!(
                home_directory(None, some("C:"), None, some(r"C:\Users\b"), |_| true),
                some(r"C:\Users\b")
            );
            assert_eq!(
                home_directory(None, None, some(r"\Users\a"), some(r"C:\Users\b"), |_| true),
                some(r"C:\Users\b")
            );
            assert_eq!(home_directory(None, None, None, None, |_| true), None);
        }

        #[test]
        fn home_directory_must_be_readable_by_git() {
            assert_eq!(
                home_directory(some(""), None, None, some(r"C:\Users\b"), |_| true),
                None,
                "an empty HOME is set, but never matches"
            );
            let longest = format!(r"C:\{}", "a".repeat(256));
            assert_eq!(
                home_directory(Some(longest.clone()), None, None, None, |_| true),
                Some(longest)
            );
            let too_long = format!(r"C:\{}", "a".repeat(257));
            assert_eq!(home_directory(Some(too_long), None, None, None, |_| true), None);
        }

        #[test]
        fn resolved_paths_are_spelled_without_verbatim_prefix() {
            assert_eq!(git_spelling(r"\\?\C:\Users\a"), r"C:\Users\a");
            assert_eq!(
                git_spelling(r"\\?\UNC\wsl.localhost\Ubuntu\tmp"),
                r"\\wsl.localhost\Ubuntu\tmp"
            );
            assert_eq!(git_spelling(r"C:\Users\a"), r"C:\Users\a");
            assert_eq!(git_spelling(r"\\server\share\a"), r"\\server\share\a");
        }

        #[test]
        fn home_matches_ignoring_only_the_case_of_ascii_letters() {
            assert!(is_home(r"C:\Users\a", r"c:\USERS\A"));
            assert!(
                !is_home(r"C:\Users\a", "C:/Users/a"),
                "separators are compared literally"
            );
            assert!(!is_home(r"C:\Users\a", r"C:\Users\a\"), "trailing separators matter");
            assert!(
                !is_home(r"C:\Users\ä", r"C:\Users\Ä"),
                "non-ASCII letters keep their case"
            );
        }

        #[test]
        fn owners_are_told_apart_like_git_for_windows() {
            let unused = || -> bool { unreachable!("the decision doesn't depend on it") };
            let describe = || ("owner".to_owned(), "user".to_owned());
            let other = Ownership::OtherUser {
                owner: "owner".into(),
                current_user: "user".into(),
            };

            let user = ownership(Owner::CurrentUser, unused, unused, describe);
            assert_eq!(user, Ownership::CurrentUser, "users own their paths");
            let administrators = ownership(Owner::Administrators, || true, unused, describe);
            assert_eq!(administrators, Ownership::CurrentUser, "as members of the group");
            let administrators = ownership(Owner::Administrators, || false, unused, describe);
            assert_eq!(administrators, other, "others don't own the paths of administrators");
            let world = ownership(Owner::World, unused, || false, describe);
            assert_eq!(world, Ownership::NotRecorded, "like FAT and exFAT");
            let world = ownership(Owner::World, unused, || true, describe);
            assert_eq!(world, other, "with recorded ownership, World is an owner");
            let others = ownership(Owner::Other, unused, unused, describe);
            assert_eq!(others, other, "any other owner");
        }

        #[test]
        fn failing_owner_queries_mean_the_owner_is_unknown() -> std::io::Result<()> {
            let path = std::path::Path::new("path");
            // Network shares like `\\wsl.localhost` fail with `ERROR_INVALID_FUNCTION`, and `ERROR_ACCESS_DENIED` and
            // `ERROR_NOT_SUPPORTED` stand for all other failures.
            for code in [1, 5, 50] {
                let ownership = query_failure(code, path)?;
                assert!(matches!(ownership, Ownership::Unknown { .. }), "{ownership:?}");
                assert!(!ownership.is_current_user(), "like Git, such paths aren't owned");
            }
            // `ERROR_FILE_NOT_FOUND` and `ERROR_PATH_NOT_FOUND`.
            for code in [2, 3] {
                let missing = query_failure(code, path);
                assert!(
                    missing.is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound),
                    "{code}: only missing paths are errors"
                );
            }
            Ok(())
        }
    }
}
