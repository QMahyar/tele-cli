use std::path::PathBuf;

pub const APP_DIR_NAME: &str = "telecli";

pub fn app_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TELE_APP_DIR") {
        return PathBuf::from(dir);
    }
    if cfg!(windows) {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join(APP_DIR_NAME);
        }
    } else if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join(APP_DIR_NAME);
    }
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(".config").join(APP_DIR_NAME),
        Err(_) => PathBuf::from(".").join(APP_DIR_NAME),
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
    3
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
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

pub fn credentials() -> anyhow::Result<Credentials> {
    let mut env = load_env(&app_data_dir().join(".env"));
    for (k, v) in std::env::vars() {
        env.insert(k, v);
    }
    let api_id = env
        .get("TELE_API_ID")
        .ok_or_else(|| anyhow::anyhow!("TELE_API_ID must be set (see .env.example)"))?;
    let api_hash = env
        .get("TELE_API_HASH")
        .ok_or_else(|| anyhow::anyhow!("TELE_API_HASH must be set (see .env.example)"))?;
    let api_id = api_id
        .parse::<i32>()
        .map_err(|_| anyhow::anyhow!("TELE_API_ID must be a number"))?;
    Ok(Credentials {
        api_id,
        api_hash: api_hash.clone(),
    })
}

pub fn load_config(path: Option<&std::path::Path>) -> anyhow::Result<AppConfig> {
    let cfg_path = match path {
        Some(p) => p.to_path_buf(),
        None => app_data_dir().join("config.toml"),
    };
    if !cfg_path.exists() {
        return Ok(AppConfig::default());
    }
    let text = std::fs::read_to_string(&cfg_path)?;
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
    if p.host.is_empty() || p.port == 0 {
        return Ok(None);
    }
    Ok(Some(format!("socks5://{}:{}", p.host, p.port)))
}

pub fn write_config(path: &std::path::Path, cfg: &AppConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(cfg)?;
    std::fs::write(path, text)?;
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
    fn empty_host_means_no_proxy() {
        let cfg = AppConfig {
            proxy: Some(ProxyConfig {
                r#type: "socks5".to_string(),
                host: String::new(),
                port: 9050,
            }),
            ..Default::default()
        };
        assert_eq!(proxy_url_for(&cfg, "work").unwrap(), None);
    }
}
