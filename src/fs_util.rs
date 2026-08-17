use std::path::Path;

pub fn create_dir_private(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    restrict(path, 0o700)?;
    Ok(())
}

#[cfg(unix)]
pub fn restrict_file_private(path: &Path) -> std::io::Result<()> {
    restrict(path, 0o600)
}

#[cfg(not(unix))]
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
    match path.to_string_lossy().strip_prefix(r"\\?\") {
        Some(rest) => std::path::PathBuf::from(rest),
        None => path,
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
