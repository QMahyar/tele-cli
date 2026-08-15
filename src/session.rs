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
    let path = session_path(name);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub async fn open_session(name: &str) -> anyhow::Result<SqliteSession> {
    validate_name(name).map_err(anyhow::Error::msg)?;
    let path = session_path(name);
    let dir = session_dir();
    crate::config::ensure_app_data_dir()?;
    crate::fs_util::create_dir_private(&dir)?;
    let session = SqliteSession::open(&path).await?;
    crate::fs_util::restrict_file_private(&path)?;
    Ok(session)
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
}
