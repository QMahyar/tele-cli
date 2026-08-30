use std::path::Path;

pub fn create_dir_private(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        #[cfg(unix)]
        restrict(path, 0o700)?;
        #[cfg(windows)]
        set_user_only_dacl(path, true)?;
        return Ok(());
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_name().ok_or_else(|| {
        std::io::Error::other(format!(
            "cannot create private dir: no file name in {}",
            path.display()
        ))
    })?;
    let tmp = parent.join(format!(".{}_tmp", stem.to_string_lossy()));
    std::fs::create_dir_all(&tmp)?;
    #[cfg(unix)]
    restrict(&tmp, 0o700)?;
    #[cfg(windows)]
    set_user_only_dacl(&tmp, true)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                #[cfg(unix)]
                restrict(path, 0o700)?;
                #[cfg(windows)]
                set_user_only_dacl(path, true)?;
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

#[cfg(unix)]
pub fn write_file_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true).mode(0o600);
    let mut file = opts.open(path)?;
    use std::io::Write as _;
    file.write_all(bytes)?;
    Ok(())
}

#[cfg(windows)]
pub fn write_file_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut file = match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
    {
        Ok(f) => {
            set_user_only_dacl(path, false)?;
            f
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(path)?;
            set_user_only_dacl(path, false)?;
            f
        }
        Err(e) => return Err(e),
    };
    file.write_all(bytes)?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub fn write_file_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(unix)]
pub fn restrict_file_private(path: &Path) -> std::io::Result<()> {
    restrict(path, 0o600)
}

#[cfg(windows)]
pub fn restrict_file_private(path: &Path) -> std::io::Result<()> {
    set_user_only_dacl(path, false)
}

#[cfg(windows)]
fn set_user_only_dacl(path: &Path, inheritable: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE,
        SET_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TokenUser, ACE_FLAGS, ACL, CONTAINER_INHERIT_ACE,
        DACL_SECURITY_INFORMATION, OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, PSID,
        TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        if let Err(e) = OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) {
            return Err(std::io::Error::other(e.to_string()));
        }

        let mut size = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        let mut buffer = vec![0u8; size as usize];
        if let Err(e) = GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr() as *mut _),
            size,
            &mut size,
        ) {
            let _ = CloseHandle(token);
            return Err(std::io::Error::other(e.to_string()));
        }

        let token_user = std::ptr::read_unaligned(buffer.as_ptr() as *const TOKEN_USER);
        let user_sid = PSID(token_user.User.Sid.0);

        let trustee = TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: PWSTR(user_sid.0 as *mut u16),
        };

        let ea = EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS.0,
            grfAccessMode: SET_ACCESS,
            grfInheritance: if inheritable {
                CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE
            } else {
                ACE_FLAGS(0)
            },
            Trustee: trustee,
        };

        let mut new_acl: *mut ACL = std::ptr::null_mut();
        let result = SetEntriesInAclW(Some(&[ea]), None, &mut new_acl);
        if result.0 != 0 {
            let _ = CloseHandle(token);
            return Err(std::io::Error::other(format!(
                "SetEntriesInAclW failed: {}",
                result.0
            )));
        }

        let path_wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let result = SetNamedSecurityInfoW(
            PCWSTR(path_wide.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(new_acl),
            None,
        );

        if !new_acl.is_null() {
            let _ = LocalFree(HLOCAL(new_acl as *mut _));
        }
        let _ = CloseHandle(token);

        if result.0 != 0 {
            return Err(std::io::Error::other(format!(
                "SetNamedSecurityInfoW failed: {}",
                result.0
            )));
        }

        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
pub fn restrict_file_private(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

pub fn path_under_guard(candidate: &Path, guard: &Path) -> bool {
    #[cfg(windows)]
    {
        let candidate: Vec<_> = candidate.components().collect();
        let guard: Vec<_> = guard.components().collect();
        candidate.len() >= guard.len()
            && candidate.iter().zip(&guard).all(|(c, g)| {
                c.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&g.as_os_str().to_string_lossy())
            })
    }
    #[cfg(not(windows))]
    {
        candidate.starts_with(guard)
    }
}

pub fn resolve_for_guard(path: &Path) -> std::path::PathBuf {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path;
    let resolved = loop {
        if let Ok(canonical) = std::fs::canonicalize(cursor) {
            let mut rebuilt = canonical;
            for part in tail.iter().rev() {
                rebuilt.push(part);
            }
            break rebuilt;
        }
        match cursor.file_name() {
            None => break path.to_path_buf(),
            Some(name) => tail.push(name.to_os_string()),
        }
        match cursor.parent() {
            None => break path.to_path_buf(),
            Some(parent) => cursor = parent,
        }
    };
    #[cfg(windows)]
    let resolved = strip_verbatim_prefix(resolved);
    resolved
}

#[cfg(windows)]
fn strip_verbatim_prefix(path: std::path::PathBuf) -> std::path::PathBuf {
    let os = path.into_os_string();
    let bytes = os.as_encoded_bytes();
    let prefix = br"\\?\";
    if bytes.starts_with(prefix) {
        let stripped = &bytes[prefix.len()..];
        std::path::PathBuf::from(unsafe {
            std::ffi::OsString::from_encoded_bytes_unchecked(stripped.to_vec())
        })
    } else {
        std::path::PathBuf::from(os)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("telecli-fs-{tag}-{}", std::process::id()))
    }

    #[cfg(unix)]
    #[test]
    fn create_dir_private_sets_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_path("dir");
        let _ = std::fs::remove_dir_all(&dir);
        create_dir_private(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn restrict_file_private_sets_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_path("file");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("s.session");
        std::fs::write(&file, b"x").unwrap();
        restrict_file_private(&file).unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn create_dir_private_tightens_existing_dir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_path("tighten");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_dir_private(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn path_under_guard_requires_component_prefix() {
        let guard = std::path::Path::new("guard").join("sub");
        let under = guard.join("file.txt");
        assert!(path_under_guard(&under, &guard));
        assert!(!path_under_guard(&guard, &under));
        let sibling = std::path::Path::new("guard").join("sub2").join("file.txt");
        assert!(!path_under_guard(&sibling, &guard));
        let prefix_like = std::path::Path::new("guard").join("submarine").join("x");
        assert!(!path_under_guard(&prefix_like, &guard));
        assert!(!path_under_guard(std::path::Path::new("guard"), &guard));
    }

    #[cfg(windows)]
    #[test]
    fn path_under_guard_ignores_ascii_case_on_windows() {
        let guard = std::path::Path::new(r"C:\Users\Alice\AppData\Roaming\TeleCli");
        let under =
            std::path::Path::new(r"c:\users\alice\appdata\roaming\telecli\sessions\me.session");
        assert!(path_under_guard(under, guard));
        let lookalike = std::path::Path::new(r"C:\USERS\ALICE\APPDATA\ROAMING\TELECLI-BAK\x");
        assert!(!path_under_guard(lookalike, guard));
    }

    #[test]
    fn resolve_for_guard_rebuilds_nonexisting_tail() {
        let base = temp_path("resolve");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let deep = base.join("a").join("b").join("c.txt");
        let resolved = resolve_for_guard(&deep);
        let resolved_lower = resolved.to_string_lossy().to_lowercase();
        // The existing prefix is canonicalized (may use 8.3 short names on
        // Windows), so the expectation is the canonical base, not the raw one.
        let canon_base = std::fs::canonicalize(&base).unwrap();
        let canon_base = canon_base.to_string_lossy().to_string();
        #[cfg(windows)]
        let canon_base = canon_base.trim_start_matches(r"\\?\");
        let base_lower = canon_base.to_lowercase();
        assert!(resolved_lower.starts_with(&base_lower), "{resolved:?}");
        assert!(
            resolved_lower.ends_with(r"a\b\c.txt") || resolved_lower.ends_with("a/b/c.txt"),
            "{resolved:?}"
        );
        let canon = std::fs::canonicalize(&base).unwrap();
        let canon = canon.to_string_lossy();
        #[cfg(windows)]
        let canon = canon.trim_start_matches(r"\\?\");
        assert_eq!(resolve_for_guard(&base).to_string_lossy(), canon);
        let _ = std::fs::remove_dir_all(&base);
    }
}
