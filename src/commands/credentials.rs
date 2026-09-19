use crate::error::{TeleError, TeleResult};

pub fn creds() -> TeleResult<crate::config::Credentials> {
    crate::config::credentials().map_err(|e| TeleError::Config(e.to_string()))
}

pub fn creds_api_id() -> TeleResult<i32> {
    Ok(creds()?.api_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_failures_surface_as_config_errors_exiting_usage() {
        let _guard = crate::config::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = std::env::temp_dir().join(format!("telecli-creds-kind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        std::env::remove_var("TELE_API_ID");
        std::env::remove_var("TELE_API_HASH");
        std::fs::write(dir.join(".env"), "TELE_API_ID=nope\n").unwrap();
        let err = creds().expect_err("malformed credentials must fail");
        assert!(matches!(err, TeleError::Config(_)), "{err}");
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
        assert_eq!(err.as_json()["type"], "ConfigError");
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
