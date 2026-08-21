use std::path::PathBuf;

use grammers_client::session::storages::SqliteSession;

pub fn session_dir() -> PathBuf {
    crate::config::app_data_dir().join("sessions")
}

pub fn validate_name(name: &str) -> Result<(), String> {
    if matches!(name, "all" | "." | "..") {
        return Err(format!("invalid account name {name:?}: reserved"));
    }
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "invalid account name {name:?}: use [A-Za-z0-9._-] only"
        ))
    }
}

pub fn session_path(name: &str) -> PathBuf {
    session_dir().join(format!("{name}.session"))
}

pub fn lock_path(name: &str) -> PathBuf {
    session_dir().join(format!("{name}.session.lock"))
}

pub fn list_session_names() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(session_dir()) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(base) = name.strip_suffix(".session") {
            names.push(base.to_string());
        }
    }
    names.sort();
    names
}

const SESSION_SIDECAR_SUFFIXES: [&str; 3] = ["-journal", "-wal", "-shm"];

fn sidecar_path(name: &str, suffix: &str) -> PathBuf {
    session_dir().join(format!("{name}.session{suffix}"))
}

fn pre_restrict_sidecars(name: &str) -> anyhow::Result<()> {
    for suffix in SESSION_SIDECAR_SUFFIXES {
        let path = sidecar_path(name, suffix);
        if !path.try_exists()? {
            std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)?;
        }
        crate::fs_util::restrict_file_private(&path)?;
    }
    Ok(())
}

fn restrict_session_files(name: &str) -> anyhow::Result<()> {
    let prefix = format!("{name}.session");
    for entry in std::fs::read_dir(session_dir())? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            crate::fs_util::restrict_file_private(&entry.path())?;
        }
    }
    Ok(())
}

pub fn remove_session(name: &str) -> anyhow::Result<()> {
    validate_name(name).map_err(anyhow::Error::msg)?;
    if !session_path(name).try_exists()? {
        return Ok(());
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path(name))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(anyhow::anyhow!(
                "session {name} is in use by another process"
            ));
        }
        Err(e) => return Err(e.into()),
    }
    let mut targets = vec![session_path(name), lock_path(name)];
    for suffix in SESSION_SIDECAR_SUFFIXES {
        targets.push(sidecar_path(name, suffix));
    }
    let result = targets
        .iter()
        .try_for_each(|path| match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        });
    drop(lock);
    result?;
    Ok(())
}

pub struct SessionLock {
    file: Option<std::fs::File>,
}

impl SessionLock {
    fn new(file: std::fs::File) -> Self {
        Self { file: Some(file) }
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        drop(self.file.take());
    }
}

pub struct LockedSession {
    pub session: SqliteSession,
    pub(crate) lock: SessionLock,
}

pub async fn open_session(name: &str) -> anyhow::Result<LockedSession> {
    validate_name(name).map_err(anyhow::Error::msg)?;
    let path = session_path(name);
    let dir = session_dir();
    crate::config::ensure_app_data_dir()?;
    crate::fs_util::create_dir_private(&dir)?;
    let lock_path = lock_path(name);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(anyhow::anyhow!(
                "session {name} is in use by another process"
            ));
        }
        Err(e) => return Err(e.into()),
    }
    pre_restrict_sidecars(name)?;
    let session = SqliteSession::open(&path).await?;
    restrict_session_files(name)?;
    Ok(LockedSession {
        session,
        lock: SessionLock::new(lock),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_accepts_plain_names() {
        for name in ["work", "home2", "a.b-c_d", "all-1", "All"] {
            assert!(validate_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn validate_name_rejects_reserved_names() {
        for name in ["all", ".", ".."] {
            assert!(validate_name(name).is_err(), "{name}");
        }
    }

    #[test]
    fn validate_name_rejects_empty_and_separators() {
        for name in ["", " ", "a/b", "a\\b", "../x"] {
            assert!(validate_name(name).is_err(), "{name}");
        }
    }

    fn test_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("telecli-{tag}-{}", std::process::id()))
    }

    async fn lock_env() -> tokio::sync::MutexGuard<'static, ()> {
        crate::config::TEST_ENV_LOCK.lock().await
    }

    #[tokio::test]
    async fn open_session_lock_is_released_on_drop() {
        let _guard = lock_env().await;
        let dir = test_dir("session-lock-drop");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let first = open_session("work").await.unwrap();
        drop(first);
        let second = open_session("work").await.unwrap();
        drop(second);
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn open_session_keeps_stale_lock_file_on_drop() {
        let _guard = lock_env().await;
        let dir = test_dir("session-lock-file-stale");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let held = open_session("work").await.unwrap();
        assert!(lock_path("work").exists());
        drop(held);
        assert!(
            lock_path("work").exists(),
            "stale lock file stays behind; the OS lock is what guards exclusivity"
        );
        let second = open_session("work").await.unwrap();
        drop(second);
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_session_restricts_sqlite_sidecars() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = lock_env().await;
        let dir = test_dir("session-sidecars");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        {
            let held = open_session("work").await.unwrap();
            drop(held);
            for suffix in ["-journal", "-wal", "-shm"] {
                let path = sidecar_path("work", suffix);
                if !path.exists() {
                    std::fs::write(&path, b"x").unwrap();
                }
                let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "sidecar {suffix}");
            }
        }
        remove_session("work").unwrap();
        assert!(!sidecar_path("work", "-journal").exists());
        assert!(!sidecar_path("work", "-wal").exists());
        assert!(!sidecar_path("work", "-shm").exists());
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn open_session_rejects_concurrent_use() {
        let _guard = lock_env().await;
        let dir = test_dir("session-lock-held");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let held = open_session("work").await.unwrap();
        let err = match open_session("work").await {
            Err(e) => e,
            Ok(_) => panic!("second open should fail while lock is held"),
        };
        assert!(err.to_string().contains("is in use by another process"));
        drop(held);
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_session_cleans_session_and_lock_files() {
        let _guard = lock_env().await;
        let dir = test_dir("session-lock-remove");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let held = open_session("work").await.unwrap();
        drop(held);
        remove_session("work").unwrap();
        assert!(!session_path("work").exists());
        assert!(!lock_path("work").exists());
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_session_rejects_in_use_session() {
        let _guard = lock_env().await;
        let dir = test_dir("session-lock-remove-held");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let held = open_session("work").await.unwrap();
        let err = remove_session("work").unwrap_err();
        assert!(err.to_string().contains("is in use by another process"));
        drop(held);
        remove_session("work").unwrap();
        assert!(!session_path("work").exists());
        assert!(!lock_path("work").exists());
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_session_tolerates_never_created_session_file() {
        let _guard = lock_env().await;
        let dir = test_dir("session-lock-remove-never-created");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        remove_session("work").unwrap();
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
