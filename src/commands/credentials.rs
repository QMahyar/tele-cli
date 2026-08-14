use crate::error::{TeleError, TeleResult};

pub fn creds() -> TeleResult<crate::config::Credentials> {
    crate::config::credentials().map_err(|e| TeleError::Config(e.to_string()))
}

pub fn creds_api_id() -> TeleResult<i32> {
    Ok(creds()?.api_id)
}
