use std::path::PathBuf;

pub const APP_DIR_NAME: &str = "telecli";

pub fn app_data_dir() -> PathBuf {
    app_data_dir_from_env(|k| std::env::var(k))
}

pub fn ensure_app_data_dir() -> std::io::Result<()> {
    crate::fs_util::create_dir_private(&app_data_dir())
}

#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn app_data_dir_from_env(
    mut get: impl FnMut(&str) -> Result<String, std::env::VarError>,
) -> PathBuf {
    if let Ok(dir) = get("TELE_APP_DIR") {
        return PathBuf::from(dir);
    }
    if cfg!(windows) {
        if let Ok(appdata) = get("APPDATA") {
            return PathBuf::from(appdata).join(APP_DIR_NAME);
        }
        if let Ok(local) = get("LOCALAPPDATA") {
            return PathBuf::from(local).join(APP_DIR_NAME);
        }
        if let Ok(profile) = get("USERPROFILE") {
            return PathBuf::from(profile).join(".config").join(APP_DIR_NAME);
        }
    } else if let Ok(xdg) = get("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join(APP_DIR_NAME);
    }
    match get("HOME") {
        Ok(home) => PathBuf::from(home).join(".config").join(APP_DIR_NAME),
        Err(_) => std::env::temp_dir().join(APP_DIR_NAME),
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
    let _ = crate::fs_util::restrict_file_private(&path);
    let stamp = FileStamp::of(&path);
    if let Some(creds) = CREDS_CACHE.lock().unwrap().get(&(path.clone(), stamp)) {
        return Ok(creds.clone());
    }
    let mut env = load_env(&path);
    for (k, v) in std::env::vars() {
        env.insert(k, v);
    }
    let api_id = parse_api_id(&env)?;
    let api_hash = env
        .get("TELE_API_HASH")
        .ok_or_else(|| anyhow::anyhow!("TELE_API_HASH must be set (see .env.example)"))?;
    let creds = Credentials {
        api_id,
        api_hash: api_hash.clone(),
    };
    CREDS_CACHE
        .lock()
        .unwrap()
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

pub fn load_config(path: Option<&std::path::Path>) -> anyhow::Result<AppConfig> {
    let cfg_path = match path {
        Some(p) => p.to_path_buf(),
        None => app_data_dir().join("config.toml"),
    };
    let stamp = FileStamp::of(&cfg_path);
    if let Some(cfg) = CONFIG_CACHE.lock().unwrap().get(&(cfg_path.clone(), stamp)) {
        return Ok(cfg.clone());
    }
    let cfg = read_config(&cfg_path)?;
    CONFIG_CACHE
        .lock()
        .unwrap()
        .insert((cfg_path, stamp), cfg.clone());
    Ok(cfg)
}

fn read_config(cfg_path: &std::path::Path) -> anyhow::Result<AppConfig> {
    if !cfg_path.exists() {
        return Ok(AppConfig::default());
    }
    let text = std::fs::read_to_string(cfg_path)?;
    let mut cfg: AppConfig = toml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", cfg_path.display()))?;
    cfg.parallel_max = cfg.parallel_max.clamp(1, 3);
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
    let text = toml::to_string_pretty(cfg)?;
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp_path = std::path::PathBuf::from(tmp_name);
    let result = std::fs::write(&tmp_path, text).and_then(|()| {
        crate::fs_util::restrict_file_private(&tmp_path)?;
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
    fn parallel_max_clamped_to_one_to_three_on_load() {
        let dir = std::env::temp_dir().join(format!("telecli-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "parallel_max = 9\n").unwrap();
        assert_eq!(load_config(Some(&path)).unwrap().parallel_max, 3);
        std::fs::write(&path, "parallel_max = 0\n").unwrap();
        assert_eq!(load_config(Some(&path)).unwrap().parallel_max, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unset_knobs_use_documented_defaults() {
        let cfg: AppConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.flood_sleep_threshold, 60);
        assert_eq!(cfg.parallel_max, 1);
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
}
