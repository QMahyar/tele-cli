use std::collections::BTreeSet;
use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::error::{TeleError, TeleResult};
use crate::output::{log_line, AccountOutcome};

pub struct GlobalFlags {
    pub account: Vec<String>,
    pub tag: Vec<String>,
    pub parallel: Option<u32>,
    pub json: bool,
    pub jsonl: bool,
    pub dry_run: bool,
    pub quiet: bool,
    pub config_path: Option<std::path::PathBuf>,
    pub command: String,
}

pub async fn run_fanout(
    flags: &GlobalFlags,
    handler: impl Fn(
            String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = TeleResult<serde_json::Value>> + Send>,
        > + Send
        + 'static,
) -> TeleResult<crate::output::Envelope> {
    let names = select_accounts(flags)?;
    if names.is_empty() {
        return Err(TeleError::Usage(
            "no accounts selected: use --account <name> or --tag <tag>".to_string(),
        ));
    }
    let cfg = crate::config::load_config(flags.config_path.as_deref())?;
    let parallel = effective_parallel(flags.parallel, cfg.parallel_max) as usize;
    let semaphore = Arc::new(Semaphore::new(parallel));
    let mut handles: Vec<(String, tokio::task::JoinHandle<TeleResult<AccountOutcome>>)> =
        Vec::new();
    for name in names {
        let semaphore = Arc::clone(&semaphore);
        let future = handler(name.clone());
        handles.push((
            name.clone(),
            tokio::task::spawn(async move {
                let _permit = semaphore
                    .acquire_owned()
                    .await
                    .map_err(|e| TeleError::Other(e.to_string()))?;
                match future.await {
                    Ok(data) => Ok(AccountOutcome {
                        account: name,
                        ok: true,
                        error: None,
                        data: Some(data),
                        exit_code: None,
                    }),
                    Err(e) => Err(e),
                }
            }),
        ));
    }
    let mut outcomes = Vec::new();
    for (name, handle) in handles {
        match handle.await {
            Ok(Ok(outcome)) => outcomes.push(outcome),
            Ok(Err(e)) => outcomes.push(failed_outcome(name, e)),
            Err(_) => outcomes.push(failed_outcome(
                name,
                TeleError::Other("account task panicked".to_string()),
            )),
        }
    }
    outcomes.sort_by(|a, b| a.account.cmp(&b.account));
    if !flags.quiet {
        for o in &outcomes {
            if let Some(err) = &o.error {
                log_line(
                    "error",
                    &format!("{}: {}", o.account, err["message"].as_str().unwrap_or("")),
                );
            }
        }
    }
    Ok(crate::output::Envelope::new(
        outcomes,
        flags.dry_run,
        &flags.command,
    ))
}

fn failed_outcome(account: String, e: TeleError) -> AccountOutcome {
    AccountOutcome {
        account,
        ok: false,
        error: Some(e.as_json()),
        data: None,
        exit_code: Some(e.exit_code()),
    }
}

fn effective_parallel(flag: Option<u32>, cfg_max: u32) -> u32 {
    flag.unwrap_or(cfg_max).clamp(1, 3)
}

pub fn select_accounts(flags: &GlobalFlags) -> TeleResult<Vec<String>> {
    let cfg = crate::config::load_config(flags.config_path.as_deref())?;
    let sessions: BTreeSet<String> = crate::session::list_session_names().into_iter().collect();
    let configured: BTreeSet<String> = cfg.accounts.keys().cloned().collect();
    let mut selected: BTreeSet<String> = BTreeSet::new();
    for name in &flags.account {
        if name == "all" {
            selected.extend(sessions.iter().cloned());
            continue;
        }
        if !configured.contains(name) && !sessions.contains(name) {
            return Err(TeleError::Usage(format!("unknown account {name}")));
        }
        selected.insert(name.clone());
    }
    for tag in &flags.tag {
        let tagged: BTreeSet<String> = cfg
            .accounts
            .iter()
            .filter(|(_, a)| a.tags.iter().any(|t| t == tag))
            .map(|(n, _)| n.clone())
            .collect();
        if tagged.is_empty() {
            return Err(TeleError::Usage(format!("no accounts with tag {tag}")));
        }
        selected.extend(tagged.intersection(&sessions).cloned());
    }
    if flags.account.is_empty() && flags.tag.is_empty() {
        selected = sessions;
    }
    Ok(selected.into_iter().collect())
}

pub fn print_envelope(flags: &GlobalFlags, envelope: &crate::output::Envelope) -> TeleResult<()> {
    if flags.json || flags.jsonl {
        let value = serde_json::to_value(envelope)?;
        crate::output::print_json(&value);
    }
    Ok(())
}

pub fn finish(flags: &GlobalFlags, envelope: &crate::output::Envelope) -> TeleResult<i32> {
    print_envelope(flags, envelope)?;
    Ok(envelope_exit_code(envelope))
}

pub fn envelope_exit_code(envelope: &crate::output::Envelope) -> i32 {
    use crate::error::*;
    if envelope.ok {
        return EXIT_OK;
    }
    let ok_count = envelope.accounts.iter().filter(|a| a.ok).count();
    if ok_count > 0 {
        return EXIT_PARTIAL;
    }
    let auth_count = envelope
        .accounts
        .iter()
        .filter(|a| a.exit_code == Some(EXIT_AUTH))
        .count();
    if auth_count == envelope.accounts.len() {
        return EXIT_AUTH;
    }
    EXIT_ALL_FAILED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_overrides_config() {
        assert_eq!(effective_parallel(Some(1), 3), 1);
        assert_eq!(effective_parallel(Some(2), 1), 2);
        assert_eq!(effective_parallel(Some(3), 1), 3);
    }

    #[test]
    fn config_is_fallback_default() {
        assert_eq!(effective_parallel(None, 1), 1);
        assert_eq!(effective_parallel(None, 2), 2);
        assert_eq!(effective_parallel(None, 3), 3);
    }

    #[test]
    fn both_sides_clamped_to_one_to_three() {
        assert_eq!(effective_parallel(Some(0), 3), 1);
        assert_eq!(effective_parallel(Some(9), 1), 3);
        assert_eq!(effective_parallel(None, 0), 1);
        assert_eq!(effective_parallel(None, 99), 3);
    }
}
