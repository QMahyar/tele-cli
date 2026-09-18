use clap::Args;

use crate::error::{TeleError, TeleResult};
use crate::executor::GlobalFlags;

#[derive(Args)]
pub struct WizardArgs {
    #[arg(long, help = "account name to register (default: personal)")]
    pub name: Option<String>,
}

const MAX_ATTEMPTS: usize = 3;

pub(crate) fn parse_api_id_input(value: &str) -> Result<i32, String> {
    let trimmed = value.trim();
    trimmed
        .parse::<i32>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| {
            format!(
                "invalid api_id {trimmed:?}: use the positive integer from https://my.telegram.org"
            )
        })
}

pub(crate) fn validate_api_hash_input(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("api_hash must not be empty".to_string());
    }
    if trimmed.contains('"') || trimmed.contains(['\n', '\r']) {
        return Err("api_hash must not contain quotes or line breaks".to_string());
    }
    Ok(trimmed.to_string())
}

pub(crate) fn resolve_account_name(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    let name = if trimmed.is_empty() {
        "personal"
    } else {
        trimmed
    };
    crate::session::validate_name(name).map(|()| name.to_string())
}

fn prompt_line(prompt: &str) -> TeleResult<Option<String>> {
    use std::io::{BufRead as _, Write as _};
    let mut stderr = std::io::stderr();
    stderr.write_all(prompt.as_bytes())?;
    stderr.flush()?;
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line))
}

fn ask_new_value(
    prompt: &str,
    parse: impl Fn(&str) -> Result<String, String>,
) -> TeleResult<String> {
    for _ in 1..=MAX_ATTEMPTS {
        match prompt_line(prompt)? {
            Some(line) if line.trim().is_empty() => {
                crate::output::log_line("warn", "a value is required");
            }
            Some(line) => match parse(&line) {
                Ok(value) => return Ok(value),
                Err(e) => crate::output::log_line("warn", &e),
            },
            None => {
                return Err(TeleError::Usage("input required; stdin closed".to_string()));
            }
        }
    }
    Err(TeleError::Usage(
        "too many invalid inputs; re-run tele wizard to try again".to_string(),
    ))
}

fn ask_keep_or_new(
    prompt: &str,
    parse: impl Fn(&str) -> Result<String, String>,
) -> TeleResult<Option<String>> {
    for _ in 1..=MAX_ATTEMPTS {
        match prompt_line(prompt)? {
            Some(line) if line.trim().is_empty() => return Ok(None),
            Some(line) => match parse(&line) {
                Ok(value) => return Ok(Some(value)),
                Err(e) => crate::output::log_line("warn", &e),
            },
            None => {
                return Err(TeleError::Usage("input required; stdin closed".to_string()));
            }
        }
    }
    Err(TeleError::Usage(
        "too many invalid inputs; re-run tele wizard to try again".to_string(),
    ))
}

fn env_file_value(map: &std::collections::HashMap<String, String>, key: &str) -> bool {
    map.get(key).is_some_and(|v| !v.trim().is_empty())
}

fn render_env_value(value: &str) -> String {
    if value.contains(' ') || value.contains('#') {
        format!("\"{value}\"")
    } else {
        value.to_string()
    }
}

fn write_env_values(
    dir: &std::path::Path,
    id: Option<&str>,
    hash: Option<&str>,
) -> TeleResult<bool> {
    if id.is_none() && hash.is_none() {
        return Ok(false);
    }
    crate::fs_util::create_dir_private(dir)
        .map_err(|e| TeleError::Config(format!("failed to prepare {}: {e}", dir.display())))?;
    let path = dir.join(".env");
    let mut map = crate::config::load_env(&path);
    if let Some(v) = id {
        map.insert("TELE_API_ID".to_string(), v.to_string());
    }
    if let Some(v) = hash {
        map.insert("TELE_API_HASH".to_string(), v.to_string());
    }
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut text = String::new();
    for key in keys {
        text.push_str(&format!("{key}={}\n", render_env_value(&map[key])));
    }
    crate::fs_util::write_file_private(&path, text.as_bytes())
        .map_err(|e| TeleError::Config(format!("failed to write {}: {e}", path.display())))?;
    Ok(true)
}

fn next_steps(name: &str) -> Vec<String> {
    vec![
        format!("tele account login --name {name} --method qr"),
        format!("tele account login --name {name} --method code --phone +15550001111"),
        "tele doctor".to_string(),
    ]
}

pub async fn run(args: &WizardArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    let dir = crate::config::app_data_dir_checked()?;
    let requested = args.name.as_deref().unwrap_or("personal");
    let name = resolve_account_name(requested).map_err(TeleError::Usage)?;
    if flags.dry_run {
        let would = format!(
            "guide credential setup for account {name} (api_id/api_hash prompts, .env + config writes)"
        );
        let envelope = crate::output::Envelope::new(
            vec![crate::output::AccountOutcome {
                account: name.clone(),
                ok: true,
                error: None,
                data: Some(serde_json::json!({"dry_run": true, "would": would})),
                exit_code: None,
            }],
            true,
            &flags.command,
        );
        return crate::executor::finish(flags, &envelope);
    }
    let machine = crate::output::machine_mode(flags.json, flags.jsonl);
    if !machine {
        crate::output::print_line("tele wizard: guided credential setup (no network)")?;
        crate::output::print_line("1. Get api_id/api_hash from https://my.telegram.org (Apps).")?;
        crate::output::print_line(
            format!(
                "2. Values are stored in {} (private file, never in the repo).",
                dir.join(".env").display()
            )
            .as_str(),
        )?;
    }
    let existing = crate::config::load_env(&dir.join(".env"));
    let id_prompt = "api_id (Enter to keep the configured value): ";
    let hash_prompt = "api_hash (Enter to keep the configured value): ";
    let api_id = if env_file_value(&existing, "TELE_API_ID")
        || std::env::var("TELE_API_ID")
            .ok()
            .is_some_and(|v| !v.trim().is_empty())
    {
        ask_keep_or_new(id_prompt, |v| parse_api_id_input(v).map(|n| n.to_string()))?
    } else {
        Some(ask_new_value(
            "api_id from https://my.telegram.org: ",
            |v| parse_api_id_input(v).map(|n| n.to_string()),
        )?)
    };
    let api_hash = if env_file_value(&existing, "TELE_API_HASH")
        || std::env::var("TELE_API_HASH")
            .ok()
            .is_some_and(|v| !v.trim().is_empty())
    {
        ask_keep_or_new(hash_prompt, validate_api_hash_input)?
    } else {
        Some(ask_new_value(
            "api_hash from https://my.telegram.org: ",
            validate_api_hash_input,
        )?)
    };
    let env_written = write_env_values(&dir, api_id.as_deref(), api_hash.as_deref())?;
    if env_written
        && ["TELE_API_ID", "TELE_API_HASH"]
            .iter()
            .any(|k| std::env::var(k).ok().is_some_and(|v| !v.trim().is_empty()))
    {
        crate::output::log_line(
            "warn",
            "a credential is set in the process environment and shadows the .env file",
        );
    }
    let mut cfg = crate::config::load_config(flags.config_path.as_deref())?;
    let is_new = !cfg.accounts.contains_key(&name);
    if is_new {
        cfg.accounts
            .insert(name.clone(), crate::config::AccountConfig::default());
        let path = match flags.config_path.clone() {
            Some(p) => p,
            None => dir.join("config.toml"),
        };
        crate::config::write_config(&path, &cfg)
            .map_err(|e| TeleError::Config(format!("failed to write config: {e:#}")))?;
    }
    let steps = next_steps(&name);
    let data = serde_json::json!({
        "account": name,
        "env_written": env_written,
        "config_updated": is_new,
        "next": steps,
    });
    if !machine {
        crate::output::print_line(&format!("account {name} is ready for login:"))?;
        for step in &steps {
            crate::output::print_line(format!("  {step}").as_str())?;
        }
    }
    let envelope = crate::output::Envelope::new(
        vec![crate::output::AccountOutcome {
            account: name,
            ok: true,
            error: None,
            data: Some(data),
            exit_code: None,
        }],
        false,
        &flags.command,
    );
    crate::executor::finish(flags, &envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_api_id_accepts_positive_integers() {
        assert_eq!(parse_api_id_input("42").unwrap(), 42);
        assert_eq!(parse_api_id_input("  1234567  ").unwrap(), 1234567);
    }

    #[test]
    fn parse_api_id_rejects_zero_negative_and_garbage() {
        for bad in ["0", "-5", "abc", "1.5", "99999999999", ""] {
            assert!(parse_api_id_input(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn validate_api_hash_rejects_empty_and_line_breaks() {
        assert_eq!(validate_api_hash_input("  deadbeef  ").unwrap(), "deadbeef");
        for bad in ["", "   ", "a\nb", "a\rb", "a\"b"] {
            assert!(validate_api_hash_input(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn resolve_account_name_defaults_to_personal() {
        assert_eq!(resolve_account_name("").unwrap(), "personal");
        assert_eq!(resolve_account_name("  ").unwrap(), "personal");
        assert_eq!(resolve_account_name(" work ").unwrap(), "work");
    }

    #[test]
    fn resolve_account_name_rejects_reserved_names() {
        for bad in ["all", ".", "..", "has space", "con"] {
            assert!(resolve_account_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn next_steps_name_the_account() {
        let steps = next_steps("work");
        assert_eq!(steps.len(), 3);
        assert!(steps[0].contains("work"), "{steps:?}");
        assert!(steps[2].contains("doctor"), "{steps:?}");
    }

    #[test]
    fn render_env_value_quotes_spaced_values() {
        assert_eq!(render_env_value("abc"), "abc");
        assert_eq!(render_env_value("a b"), "\"a b\"");
        assert_eq!(render_env_value("a#b"), "\"a#b\"");
    }
}
