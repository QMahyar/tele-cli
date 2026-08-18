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
    drop(lock);
    for path in [session_path(name), lock_path(name)] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

pub struct SessionLock {
    file: Option<std::fs::File>,
    path: PathBuf,
}

impl SessionLock {
    fn new(file: std::fs::File, path: PathBuf) -> Self {
        Self {
            file: Some(file),
            path,
        }
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
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
    let session = SqliteSession::open(&path).await?;
    crate::fs_util::restrict_file_private(&path)?;
    Ok(LockedSession {
        session,
        lock: SessionLock::new(lock, lock_path),
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
    async fn open_session_removes_lock_file_on_drop() {
        let _guard = lock_env().await;
        let dir = test_dir("session-lock-file-gone");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        let held = open_session("work").await.unwrap();
        assert!(lock_path("work").exists());
        drop(held);
        assert!(!lock_path("work").exists());
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
