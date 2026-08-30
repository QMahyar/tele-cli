use std::str::FromStr;

use crate::error::{TeleError, TeleResult};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChatTarget(String);

impl ChatTarget {
    pub fn parse(s: &str) -> TeleResult<Self> {
        Self::parse_flag(s, "chat")
    }

    pub fn parse_flag(s: &str, flag: &str) -> TeleResult<Self> {
        if s.trim().is_empty() {
            return Err(TeleError::Usage(format!("--{flag} must not be empty")));
        }
        match crate::entities::parse_target(s) {
            Ok(rt) if rt.msg_id.is_some() => {
                return Err(TeleError::Usage(format!(
                    "--{flag} \"{s}\" carries a deep-link message id; deep-link message ids are only accepted by: tele msg get"
                )));
            }
            _ => {}
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    pub(crate) fn new_unchecked(s: String) -> Self {
        Self(s)
    }
}

impl FromStr for ChatTarget {
    type Err = TeleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl std::fmt::Display for ChatTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ChatTarget {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for ChatTarget {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<ChatTarget> for String {
    fn from(c: ChatTarget) -> Self {
        c.0
    }
}

impl<'de> serde::Deserialize<'de> for ChatTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl serde::Serialize for ChatTarget {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl rmcp::schemars::JsonSchema for ChatTarget {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ChatTarget")
    }

    fn json_schema(
        generator: &mut rmcp::schemars::generate::SchemaGenerator,
    ) -> rmcp::schemars::Schema {
        String::json_schema(generator)
    }
}
