use std::path::PathBuf;

use crate::error::{TeleError, TeleResult};

pub const APP_DIR_NAME: &str = "telecli";

pub fn app_data_dir() -> PathBuf {
    app_data_dir_from_env(|k| std::env::var(k))
}

pub fn ensure_app_data_dir() -> std::io::Result<()> {
    crate::fs_util::create_dir_private(&app_data_dir())
}

#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn env_nonempty(
    get: &mut impl FnMut(&str) -> Result<String, std::env::VarError>,
    key: &str,
) -> Option<String> {
    get(key).ok().filter(|v| !v.trim().is_empty())
}

fn app_data_dir_from_env(
    mut get: impl FnMut(&str) -> Result<String, std::env::VarError>,
) -> PathBuf {
    if let Some(dir) = env_nonempty(&mut get, "TELE_APP_DIR") {
        return PathBuf::from(dir);
    }
    if cfg!(windows) {
        if let Some(appdata) = env_nonempty(&mut get, "APPDATA") {
            return PathBuf::from(appdata).join(APP_DIR_NAME);
        }
        if let Some(local) = env_nonempty(&mut get, "LOCALAPPDATA") {
            return PathBuf::from(local).join(APP_DIR_NAME);
        }
        if let Some(profile) = env_nonempty(&mut get, "USERPROFILE") {
            return PathBuf::from(profile).join(".config").join(APP_DIR_NAME);
        }
    } else if let Some(xdg) = env_nonempty(&mut get, "XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join(APP_DIR_NAME);
    }
    match env_nonempty(&mut get, "HOME") {
        Some(home) => PathBuf::from(home).join(".config").join(APP_DIR_NAME),
        None => std::env::temp_dir().join(APP_DIR_NAME),
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AccountConfig {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
    #[serde(default)]
    pub flood_sleep_threshold: Option<u64>,
    #[serde(default)]
    pub rpc_per_minute: Option<f64>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_flood")]
    pub flood_sleep_threshold: u64,
    #[serde(default = "default_parallel_max")]
    pub parallel_max: u32,
    #[serde(default)]
    pub accounts: std::collections::BTreeMap<String, AccountConfig>,
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
}

fn default_flood() -> u64 {
    60
}

fn default_parallel_max() -> u32 {
    1
}

#[derive(Clone)]
pub struct Credentials {
    pub api_id: i32,
    pub api_hash: String,
}

pub fn load_env(path: &std::path::Path) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    for line in text.lines() {
        let mut line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line
            .strip_prefix("export")
            .filter(|rest| rest.starts_with(char::is_whitespace))
        {
            line = rest.trim_start();
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if key.is_empty() {
            continue;
        }
        out.insert(key.to_string(), strip_env_value(v.trim()));
    }
    out
}

fn strip_env_value(v: &str) -> String {
    let mut in_quote: Option<char> = None;
    let mut end = v.len();
    for (i, ch) in v.char_indices() {
        match in_quote {
            Some(q) => {
                if ch == q {
                    in_quote = None;
                }
            }
            None => match ch {
                '"' | '\'' => in_quote = Some(ch),
                '#' if i == 0 || v[..i].ends_with(char::is_whitespace) => {
                    end = i;
                    break;
                }
                _ => {}
            },
        }
    }
    let value = v[..end].trim();
    if let Some(quote) = value.chars().next().filter(|c| *c == '"' || *c == '\'') {
        if value.len() >= 2 && value.ends_with(quote) {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FileStamp {
    exists: bool,
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl FileStamp {
    fn of(path: &std::path::Path) -> Self {
        match std::fs::metadata(path) {
            Ok(meta) => Self {
                exists: true,
                len: meta.len(),
                modified: meta.modified().ok(),
            },
            Err(_) => Self {
                exists: false,
                len: 0,
                modified: None,
            },
        }
    }
}

static CONFIG_CACHE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<(std::path::PathBuf, FileStamp), AppConfig>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

static CREDS_CACHE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<(std::path::PathBuf, FileStamp), Credentials>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(test)]
static ENV_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn credentials() -> anyhow::Result<Credentials> {
    let path = app_data_dir().join(".env");
    if path.exists() {
        if let Err(e) = crate::fs_util::restrict_file_private(&path) {
            crate::output::log_line(
                "warn",
                &format!("failed to tighten permissions on the credentials file: {e}"),
            );
        }
    } else {
        let _ = crate::fs_util::restrict_file_private(&path);
    }
    let stamp = FileStamp::of(&path);
    if let Some(creds) = CREDS_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(path.clone(), stamp))
    {
        return Ok(creds.clone());
    }
    let mut env = load_env(&path);
    for (k, v) in std::env::vars() {
        if !v.trim().is_empty() {
            env.insert(k, v);
        }
    }
    let api_id = parse_api_id(&env)?;
    let api_hash = env
        .get("TELE_API_HASH")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("TELE_API_HASH must be set (see .env.example)"))?;
    let creds = Credentials {
        api_id,
        api_hash: api_hash.to_string(),
    };
    CREDS_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((path, stamp), creds.clone());
    #[cfg(test)]
    ENV_READS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Ok(creds)
}

fn parse_api_id(env: &std::collections::HashMap<String, String>) -> anyhow::Result<i32> {
    let api_id = env
        .get("TELE_API_ID")
        .ok_or_else(|| anyhow::anyhow!("TELE_API_ID must be set (see .env.example)"))?;
    let api_id = api_id
        .parse::<i32>()
        .map_err(|_| anyhow::anyhow!("TELE_API_ID must be a positive integer"))?;
    if api_id <= 0 {
        return Err(anyhow::anyhow!("TELE_API_ID must be a positive integer"));
    }
    Ok(api_id)
}

pub fn load_config(path: Option<&std::path::Path>) -> TeleResult<AppConfig> {
    let cfg_path = match path {
        Some(p) => p.to_path_buf(),
        None => app_data_dir().join("config.toml"),
    };
    let stamp = FileStamp::of(&cfg_path);
    if let Some(cfg) = CONFIG_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(cfg_path.clone(), stamp))
    {
        return Ok(cfg.clone());
    }
    let cfg = read_config(&cfg_path).map_err(|e| TeleError::Config(format!("{e:#}")))?;
    CONFIG_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((cfg_path, stamp), cfg.clone());
    Ok(cfg)
}

fn config_display_name(cfg_path: &std::path::Path) -> String {
    cfg_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_string())
}

fn read_config(cfg_path: &std::path::Path) -> anyhow::Result<AppConfig> {
    if !cfg_path.exists() {
        return Ok(AppConfig::default());
    }
    if cfg_path.is_dir() {
        return Err(anyhow::anyhow!(
            "failed to read config: {} is a directory",
            config_display_name(cfg_path)
        ));
    }
    let text = std::fs::read_to_string(cfg_path)?;
    let mut cfg: AppConfig = toml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", config_display_name(cfg_path)))?;
    cfg.parallel_max = cfg.parallel_max.clamp(1, 32);
    Ok(cfg)
}

pub fn proxy_url_for(cfg: &AppConfig, name: &str) -> anyhow::Result<Option<String>> {
    let p = cfg
        .accounts
        .get(name)
        .and_then(|a| a.proxy.as_ref())
        .or(cfg.proxy.as_ref());
    let Some(p) = p else {
        return Ok(None);
    };
    if !p.r#type.is_empty() && p.r#type != "socks5" {
        return Err(anyhow::anyhow!(
            "proxy type {} unsupported (grammers supports socks5 only)",
            p.r#type
        ));
    }
    if p.host.is_empty() {
        return Err(anyhow::anyhow!("proxy for {name}: host must not be empty"));
    }
    if p.port == 0 {
        return Err(anyhow::anyhow!("proxy for {name}: port must be non-zero"));
    }
    Ok(Some(format!("socks5://{}:{}", p.host, p.port)))
}

pub fn write_config(path: &std::path::Path, cfg: &AppConfig) -> anyhow::Result<()> {
    let app_dir = app_data_dir();
    if path.starts_with(&app_dir) {
        crate::fs_util::create_dir_private(&app_dir)?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = if path.exists() {
        let existing = std::fs::read_to_string(path)?;
        let mut doc: toml_edit::DocumentMut = existing
            .parse()
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;
        let mut fresh: toml_edit::DocumentMut = toml_edit::ser::to_string_pretty(cfg)?
            .parse()
            .map_err(|e| anyhow::anyhow!("failed to serialize config: {e}"))?;
        match fresh.as_table_mut().remove("accounts") {
            Some(accounts) => {
                doc["accounts"] = accounts;
            }
            None => {
                doc.as_table_mut().remove("accounts");
            }
        }
        doc.to_string()
    } else {
        toml_edit::ser::to_string_pretty(cfg)?
    };
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp_path = std::path::PathBuf::from(tmp_name);
    let result = std::fs::write(&tmp_path, "")
        .and_then(|()| crate::fs_util::restrict_file_private(&tmp_path))
        .and_then(|()| std::fs::write(&tmp_path, text))
        .and_then(|()| {
            #[cfg(windows)]
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            std::fs::rename(&tmp_path, path)
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socks5(host: &str, port: u16) -> ProxyConfig {
        ProxyConfig {
            r#type: "socks5".to_string(),
            host: host.to_string(),
            port,
        }
    }

    fn write_env(tag: &str, content: &str) -> std::collections::HashMap<String, String> {
        let dir = std::env::temp_dir().join(format!("telecli-env-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        std::fs::write(&path, content).unwrap();
        let map = load_env(&path);
        let _ = std::fs::remove_dir_all(&dir);
        map
    }

    fn env_get<'a>(
        map: &'a std::collections::HashMap<String, String>,
        key: &str,
    ) -> Option<&'a str> {
        map.get(key).map(String::as_str)
    }

    #[test]
    fn no_proxy_when_nothing_configured() {
        let cfg = AppConfig::default();
        assert_eq!(proxy_url_for(&cfg, "work").unwrap(), None);
    }

    #[test]
    fn global_socks5_becomes_url() {
        let cfg = AppConfig {
            proxy: Some(socks5("127.0.0.1", 9050)),
            ..Default::default()
        };
        assert_eq!(
            proxy_url_for(&cfg, "work").unwrap(),
            Some("socks5://127.0.0.1:9050".to_string())
        );
    }

    #[test]
    fn per_account_overrides_global() {
        let cfg = AppConfig {
            proxy: Some(socks5("global", 9050)),
            accounts: [(
                "work".to_string(),
                AccountConfig {
                    tags: vec![],
                    proxy: Some(socks5("local", 1080)),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        assert_eq!(
            proxy_url_for(&cfg, "work").unwrap(),
            Some("socks5://local:1080".to_string())
        );
        assert_eq!(
            proxy_url_for(&cfg, "other").unwrap(),
            Some("socks5://global:9050".to_string())
        );
    }

    #[test]
    fn unsupported_proxy_type_errors() {
        let cfg = AppConfig {
            proxy: Some(ProxyConfig {
                r#type: "http".to_string(),
                host: "127.0.0.1".to_string(),
                port: 8080,
            }),
            ..Default::default()
        };
        let err = proxy_url_for(&cfg, "work").unwrap_err().to_string();
        assert!(err.contains("socks5"), "err: {err}");
    }

    #[test]
    fn empty_host_is_a_config_error() {
        let cfg = AppConfig {
            proxy: Some(ProxyConfig {
                r#type: "socks5".to_string(),
                host: String::new(),
                port: 9050,
            }),
            ..Default::default()
        };
        let err = proxy_url_for(&cfg, "work").unwrap_err().to_string();
        assert!(err.contains("host"), "err: {err}");
    }

    #[test]
    fn zero_port_is_a_config_error() {
        let cfg = AppConfig {
            proxy: Some(socks5("127.0.0.1", 0)),
            ..Default::default()
        };
        let err = proxy_url_for(&cfg, "work").unwrap_err().to_string();
        assert!(err.contains("port"), "err: {err}");
    }

    #[test]
    fn missing_proxy_port_errors_after_load() {
        let dir = std::env::temp_dir().join(format!("telecli-config-port-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[proxy]\ntype = \"socks5\"\nhost = \"127.0.0.1\"\n").unwrap();
        let cfg = load_config(Some(&path)).unwrap();
        let err = proxy_url_for(&cfg, "work").unwrap_err().to_string();
        assert!(err.contains("port"), "err: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_proxy_port_fails_config_parse() {
        let dir =
            std::env::temp_dir().join(format!("telecli-config-garbagport-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[proxy]\nhost = \"127.0.0.1\"\nport = \"abc\"\n").unwrap();
        assert!(load_config(Some(&path)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_config_is_config_kind_exiting_usage() {
        let dir =
            std::env::temp_dir().join(format!("telecli-config-badkind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        for bad in ["not [valid toml", "parallel_max = \"abc\"\n"] {
            std::fs::write(&path, bad).unwrap();
            let err = load_config(Some(&path)).unwrap_err();
            assert!(matches!(err, TeleError::Config(_)), "{bad:?}: {err}");
            assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
            assert_eq!(err.as_json()["type"], "ConfigError");
        }
        std::fs::write(&path, b"\x00\x01\x02garbage\xff\xfe").unwrap();
        let err = load_config(Some(&path)).unwrap_err();
        assert!(matches!(err, TeleError::Config(_)), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_as_config_path_is_config_kind() {
        let dir = std::env::temp_dir().join(format!("telecli-config-isdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let err = load_config(Some(&dir)).unwrap_err();
        assert!(matches!(err, TeleError::Config(_)), "{err}");
        assert_eq!(err.exit_code(), crate::error::EXIT_USAGE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_config_rejects_directory_with_clear_message() {
        let _guard = TEST_ENV_LOCK.blocking_lock();
        let dir =
            std::env::temp_dir().join(format!("telecli-config-rd-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let err = read_config(&dir).unwrap_err().to_string();
        assert!(err.contains("directory"), "err: {err}");
        let leaf = dir.file_name().unwrap().to_string_lossy().to_string();
        assert!(err.contains(&leaf), "err: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_config_missing_path_is_defaults() {
        let _guard = TEST_ENV_LOCK.blocking_lock();
        let dir =
            std::env::temp_dir().join(format!("telecli-config-rd-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_config(&dir.join("config.toml")).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_config_malformed_toml_is_parse_error() {
        let _guard = TEST_ENV_LOCK.blocking_lock();
        let dir =
            std::env::temp_dir().join(format!("telecli-config-rd-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "not [valid toml").unwrap();
        let err = read_config(&path).unwrap_err().to_string();
        assert!(err.contains("failed to parse"), "err: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_config_file_is_defaults_not_error() {
        let dir =
            std::env::temp_dir().join(format!("telecli-config-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load_config(Some(&dir.join("config.toml"))).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parallel_max_clamped_to_one_to_thirty_two_on_load() {
        let dir = std::env::temp_dir().join(format!("telecli-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "parallel_max = 99\n").unwrap();
        assert_eq!(load_config(Some(&path)).unwrap().parallel_max, 32);
        std::fs::write(&path, "parallel_max = 0 # clamp to minimum\n").unwrap();
        assert_eq!(load_config(Some(&path)).unwrap().parallel_max, 1);
        std::fs::write(&path, "parallel_max = 32\n").unwrap();
        assert_eq!(load_config(Some(&path)).unwrap().parallel_max, 32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unset_knobs_use_documented_defaults() {
        let cfg: AppConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.flood_sleep_threshold, 60);
        assert_eq!(cfg.parallel_max, 1);
        assert!(cfg.accounts.is_empty());
    }

    #[test]
    fn per_account_flood_sleep_threshold_none_when_absent() {
        let cfg: AppConfig = toml::from_str("").unwrap();
        assert!(cfg.accounts.is_empty());
    }

    #[test]
    fn per_account_fields_round_trip() {
        let toml_str = "parallel_max = 2\n\n[accounts.work]\ntags = [\"team\"]\nflood_sleep_threshold = 30\nrpc_per_minute = 120.0\n";
        let cfg: AppConfig = toml::from_str(toml_str).unwrap();
        let acct = &cfg.accounts["work"];
        assert_eq!(acct.flood_sleep_threshold, Some(30));
        assert_eq!(acct.rpc_per_minute, Some(120.0));
        assert_eq!(cfg.parallel_max, 2);
    }

    #[test]
    fn per_account_optional_fields_default_to_none() {
        let toml_str = "[accounts.work]\ntags = [\"a\"]\n";
        let cfg: AppConfig = toml::from_str(toml_str).unwrap();
        let acct = &cfg.accounts["work"];
        assert_eq!(acct.flood_sleep_threshold, None);
        assert_eq!(acct.rpc_per_minute, None);
    }

    #[test]
    fn per_account_rpc_per_minute_round_trip_serialization() {
        let cfg = AppConfig {
            accounts: [(
                "work".to_string(),
                AccountConfig {
                    tags: vec![],
                    proxy: None,
                    flood_sleep_threshold: Some(30),
                    rpc_per_minute: Some(120.0),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let dir = atomic_dir("per-acct-roundtrip");
        let path = dir.join("config.toml");
        write_config(&path, &cfg).unwrap();
        let back = read_config(&path).unwrap();
        let acct = &back.accounts["work"];
        assert_eq!(acct.flood_sleep_threshold, Some(30));
        assert_eq!(acct.rpc_per_minute, Some(120.0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn per_account_flood_threshold_none_uses_global() {
        let cfg = AppConfig {
            flood_sleep_threshold: 120,
            accounts: [(
                "work".to_string(),
                AccountConfig {
                    tags: vec![],
                    proxy: None,
                    flood_sleep_threshold: None,
                    rpc_per_minute: None,
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let acct = &cfg.accounts["work"];
        assert_eq!(acct.flood_sleep_threshold, None);
        assert_eq!(cfg.flood_sleep_threshold, 120);
    }

    #[test]
    fn per_account_rpc_per_minute_none_means_unlimited() {
        let cfg = AppConfig {
            accounts: [(
                "work".to_string(),
                AccountConfig {
                    tags: vec![],
                    proxy: None,
                    flood_sleep_threshold: None,
                    rpc_per_minute: None,
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        assert_eq!(cfg.accounts["work"].rpc_per_minute, None);
    }

    #[test]
    fn app_data_dir_prefers_teleg_app_dir_override() {
        let get = |k: &str| match k {
            "TELE_APP_DIR" => Ok("/tmp/custom-appdir".to_string()),
            "APPDATA" => Ok("C:\\Users\\t\\AppData\\Roaming".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        assert_eq!(
            app_data_dir_from_env(get),
            PathBuf::from("/tmp/custom-appdir")
        );
    }

    #[test]
    fn app_data_dir_uses_home_config_dir() {
        let get = |k: &str| match k {
            "HOME" => Ok("/home/tester".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        assert_eq!(
            app_data_dir_from_env(get),
            PathBuf::from("/home/tester")
                .join(".config")
                .join(APP_DIR_NAME)
        );
    }

    #[test]
    fn app_data_dir_never_uses_cwd() {
        let dir = app_data_dir_from_env(|_k| Err(std::env::VarError::NotPresent));
        assert_eq!(dir, std::env::temp_dir().join(APP_DIR_NAME));
        assert!(dir.is_absolute());
        assert_ne!(dir, std::env::current_dir().unwrap().join(APP_DIR_NAME));
    }

    #[test]
    fn app_data_dir_ignores_empty_override() {
        let get = |k: &str| match k {
            "TELE_APP_DIR" => Ok(String::new()),
            "HOME" => Ok("/home/tester".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        assert_eq!(
            app_data_dir_from_env(get),
            PathBuf::from("/home/tester")
                .join(".config")
                .join(APP_DIR_NAME)
        );
    }

    #[test]
    fn app_data_dir_ignores_whitespace_override() {
        let get = |k: &str| match k {
            "TELE_APP_DIR" => Ok("   ".to_string()),
            "HOME" => Ok("/home/tester".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        assert_eq!(
            app_data_dir_from_env(get),
            PathBuf::from("/home/tester")
                .join(".config")
                .join(APP_DIR_NAME)
        );
    }

    #[test]
    fn app_data_dir_ignores_empty_home() {
        let get = |k: &str| match k {
            "HOME" => Ok(String::new()),
            _ => Err(std::env::VarError::NotPresent),
        };
        assert_eq!(
            app_data_dir_from_env(get),
            std::env::temp_dir().join(APP_DIR_NAME)
        );
    }

    #[test]
    fn env_nonempty_falls_through_for_empty_and_whitespace() {
        let _guard = TEST_ENV_LOCK.blocking_lock();
        std::env::set_var("TELE_TEST_ENV_NONEMPTY", "");
        let mut get = |k: &str| std::env::var(k);
        assert!(env_nonempty(&mut get, "TELE_TEST_ENV_NONEMPTY").is_none());
        std::env::set_var("TELE_TEST_ENV_NONEMPTY", " \t ");
        assert!(env_nonempty(&mut get, "TELE_TEST_ENV_NONEMPTY").is_none());
        std::env::set_var("TELE_TEST_ENV_NONEMPTY", "/tmp/x");
        assert_eq!(
            env_nonempty(&mut get, "TELE_TEST_ENV_NONEMPTY").as_deref(),
            Some("/tmp/x")
        );
        std::env::remove_var("TELE_TEST_ENV_NONEMPTY");
        assert!(env_nonempty(&mut get, "TELE_TEST_ENV_NONEMPTY").is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn app_data_dir_prefers_xdg_config_home() {
        let get = |k: &str| match k {
            "XDG_CONFIG_HOME" => Ok("/xdg/config".to_string()),
            "HOME" => Ok("/home/tester".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        assert_eq!(
            app_data_dir_from_env(get),
            PathBuf::from("/xdg/config").join(APP_DIR_NAME)
        );
    }

    #[cfg(windows)]
    #[test]
    fn app_data_dir_prefers_appdata() {
        let get = |k: &str| match k {
            "APPDATA" => Ok("C:\\Users\\t\\AppData\\Roaming".to_string()),
            "HOME" => Ok("C:\\Users\\t".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        assert_eq!(
            app_data_dir_from_env(get),
            PathBuf::from("C:\\Users\\t\\AppData\\Roaming").join(APP_DIR_NAME)
        );
    }

    #[cfg(windows)]
    #[test]
    fn app_data_dir_falls_back_to_localappdata_then_userprofile() {
        let get = |k: &str| match k {
            "LOCALAPPDATA" => Ok("C:\\Users\\t\\AppData\\Local".to_string()),
            "USERPROFILE" => Ok("C:\\Users\\t".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        assert_eq!(
            app_data_dir_from_env(get),
            PathBuf::from("C:\\Users\\t\\AppData\\Local").join(APP_DIR_NAME)
        );
        let get = |k: &str| match k {
            "USERPROFILE" => Ok("C:\\Users\\t".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        assert_eq!(
            app_data_dir_from_env(get),
            PathBuf::from("C:\\Users\\t")
                .join(".config")
                .join(APP_DIR_NAME)
        );
    }

    #[test]
    fn env_parser_skips_comments_and_blank_lines() {
        let env = write_env("comments", "# full-line comment\n\n   \nTELE_API_ID=123\n");
        assert_eq!(env_get(&env, "TELE_API_ID"), Some("123"));
        assert_eq!(env.len(), 1);
    }

    #[test]
    fn env_parser_strips_export_prefix() {
        let env = write_env(
            "export",
            "export TELE_API_ID=123\n   export\tTELE_API_HASH=abc\n",
        );
        assert_eq!(env_get(&env, "TELE_API_ID"), Some("123"));
        assert_eq!(env_get(&env, "TELE_API_HASH"), Some("abc"));
    }

    #[test]
    fn env_parser_strips_quotes() {
        let env = write_env(
            "quotes",
            "TELE_API_ID=\"123\"\nTELE_API_HASH='deadbeef'\nQUOTED_HASH=\"a'b\"\n",
        );
        assert_eq!(env_get(&env, "TELE_API_ID"), Some("123"));
        assert_eq!(env_get(&env, "TELE_API_HASH"), Some("deadbeef"));
        assert_eq!(env_get(&env, "QUOTED_HASH"), Some("a'b"));
    }

    #[test]
    fn env_parser_strips_inline_comments() {
        let env = write_env(
            "inline-comments",
            "TELE_API_ID=123 # trailing comment\nTELE_API_HASH=abc#not-a-comment\n",
        );
        assert_eq!(env_get(&env, "TELE_API_ID"), Some("123"));
        assert_eq!(env_get(&env, "TELE_API_HASH"), Some("abc#not-a-comment"));
    }

    #[test]
    fn env_parser_keeps_hash_inside_quotes() {
        let env = write_env("quoted-hash", "TELE_API_HASH=\"abc # keep me\"\n");
        assert_eq!(env_get(&env, "TELE_API_HASH"), Some("abc # keep me"));
    }

    #[test]
    fn env_parser_skips_lines_without_equals() {
        let env = write_env("no-equals", "TELE_API_ID\nnoise\nTELE_API_HASH=xyz\n");
        assert_eq!(env.len(), 1);
        assert_eq!(env_get(&env, "TELE_API_HASH"), Some("xyz"));
    }

    #[test]
    fn env_parser_empty_value_parses_to_empty_string() {
        let env = write_env("empty-value", "TELE_API_HASH=\nTELE_API_ID=123\n");
        assert_eq!(env_get(&env, "TELE_API_HASH"), Some(""));
        assert_eq!(env_get(&env, "TELE_API_ID"), Some("123"));
    }

    #[test]
    fn env_parser_strips_utf8_bom() {
        let dir = std::env::temp_dir().join(format!("telecli-env-bom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        std::fs::write(&path, b"\xef\xbb\xbfTELE_API_ID=123\nTELE_API_HASH=abc\n").unwrap();
        let env = load_env(&path);
        assert_eq!(env_get(&env, "TELE_API_ID"), Some("123"));
        assert_eq!(env_get(&env, "TELE_API_HASH"), Some("abc"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn api_id_must_be_a_positive_integer() {
        let missing = parse_api_id(&std::collections::HashMap::new());
        assert!(missing.unwrap_err().to_string().contains("TELE_API_ID"));
        for bad in ["0", "-5", "abc", "1.5", "99999999999", ""] {
            let env = [("TELE_API_ID", bad)]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let err = parse_api_id(&env).unwrap_err().to_string();
            assert!(err.contains("positive integer"), "{bad:?}: {err}");
        }
        let ok: std::collections::HashMap<String, String> = [("TELE_API_ID", "1234567")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(parse_api_id(&ok).unwrap(), 1234567);
    }

    fn creds_env_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("telecli-config-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("TELE_APP_DIR", &dir);
        dir
    }

    #[test]
    fn credentials_same_path_is_read_once() {
        let _guard = TEST_ENV_LOCK.blocking_lock();
        let dir = creds_env_dir("creds-once");
        std::fs::write(dir.join(".env"), "TELE_API_ID=111\nTELE_API_HASH=dddd\n").unwrap();
        let before = ENV_READS.load(std::sync::atomic::Ordering::SeqCst);
        let c1 = credentials().unwrap();
        let c2 = credentials().unwrap();
        let after = ENV_READS.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(c1.api_id, 111);
        assert_eq!(c2.api_id, 111);
        assert_eq!(after - before, 1, "second call must hit the cache");
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn credentials_refresh_when_env_file_changes() {
        let _guard = TEST_ENV_LOCK.blocking_lock();
        let dir = creds_env_dir("creds-refresh");
        std::fs::write(
            dir.join(".env"),
            "TELE_API_ID=1234567\nTELE_API_HASH=aaaa\n",
        )
        .unwrap();
        assert_eq!(credentials().unwrap().api_id, 1234567);
        std::fs::write(dir.join(".env"), "TELE_API_ID=9\nTELE_API_HASH=bbbb\n").unwrap();
        assert_eq!(credentials().unwrap().api_id, 9);
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn credentials_tighten_exposed_env_file() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = TEST_ENV_LOCK.blocking_lock();
        let dir = creds_env_dir("creds-tighten");
        let path = dir.join(".env");
        std::fs::write(&path, "TELE_API_ID=5\nTELE_API_HASH=cccc\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        credentials().unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn credentials_rejects_empty_api_hash() {
        let _guard = TEST_ENV_LOCK.blocking_lock();
        let dir = creds_env_dir("creds-empty-hash");
        std::fs::write(dir.join(".env"), "TELE_API_ID=1234567\nTELE_API_HASH=\n").unwrap();
        let err = credentials()
            .err()
            .expect("empty TELE_API_HASH must be rejected")
            .to_string();
        assert!(err.contains("TELE_API_HASH must be set"), "err: {err}");
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn credentials_rejects_whitespace_only_api_hash() {
        let _guard = TEST_ENV_LOCK.blocking_lock();
        let dir = creds_env_dir("creds-ws-hash");
        std::fs::write(dir.join(".env"), "TELE_API_ID=1234567\nTELE_API_HASH=   \n").unwrap();
        let err = credentials()
            .err()
            .expect("whitespace-only TELE_API_HASH must be rejected")
            .to_string();
        assert!(err.contains("TELE_API_HASH must be set"), "err: {err}");
        std::env::remove_var("TELE_APP_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn atomic_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("telecli-config-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tmp_leftovers(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp-"))
            .collect()
    }

    #[test]
    fn write_config_round_trips_and_leaves_no_temp() {
        let dir = atomic_dir("atomic");
        let path = dir.join("config.toml");
        let cfg = AppConfig {
            flood_sleep_threshold: 42,
            parallel_max: 2,
            accounts: [(
                "work".to_string(),
                AccountConfig {
                    tags: vec!["team".to_string()],
                    proxy: None,
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            proxy: Some(socks5("127.0.0.1", 9050)),
        };
        write_config(&path, &cfg).unwrap();
        let back = read_config(&path).unwrap();
        assert_eq!(back.flood_sleep_threshold, 42);
        assert_eq!(back.parallel_max, 2);
        assert_eq!(back.accounts["work"].tags, vec!["team".to_string()]);
        assert_eq!(back.proxy.unwrap().host, "127.0.0.1");
        assert!(tmp_leftovers(&dir).is_empty(), "temp files left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_config_failure_cleans_up_temp_file() {
        let dir = atomic_dir("atomic-fail");
        let path = dir.join("config.toml");
        std::fs::create_dir(&path).unwrap();
        assert!(write_config(&path, &AppConfig::default()).is_err());
        assert!(
            tmp_leftovers(&dir).is_empty(),
            "temp file not cleaned up on failure"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_config_preserves_comments_and_unknown_keys_roundtrip() {
        let dir = atomic_dir("preserve");
        let path = dir.join("config.toml");
        let original = "# my comment\nflood_sleep_threshold = 60\nparallel_max = 2\n\n[custom]\nanswer = 42\n\n[accounts.foo]\ntags = []\n";
        std::fs::write(&path, original).unwrap();
        let cfg = AppConfig {
            flood_sleep_threshold: 60,
            parallel_max: 2,
            accounts: [
                (
                    "foo".to_string(),
                    AccountConfig {
                        tags: vec![],
                        proxy: None,
                        ..Default::default()
                    },
                ),
                (
                    "bar".to_string(),
                    AccountConfig {
                        tags: vec!["new".to_string()],
                        proxy: None,
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
            proxy: None,
        };
        write_config(&path, &cfg).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my comment"), "comment lost:\n{text}");
        assert!(text.contains("[custom]"), "unknown table lost:\n{text}");
        assert!(text.contains("answer = 42"), "unknown key lost:\n{text}");
        assert!(
            text.contains("flood_sleep_threshold = 60"),
            "existing key lost:\n{text}"
        );
        assert!(
            text.contains("[accounts.bar]"),
            "new account missing:\n{text}"
        );
        let back = read_config(&path).unwrap();
        assert_eq!(back.accounts.len(), 2);
        assert_eq!(back.accounts["bar"].tags, vec!["new".to_string()]);
        assert_eq!(back.parallel_max, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
