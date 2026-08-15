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
    let cfg = crate::config::load_config(flags.config_path.as_deref())?;
    let names = select_accounts(flags)?;
    if names.is_empty() {
        return Err(TeleError::Usage(
            "no accounts selected: use --account <name> or --tag <tag>".to_string(),
        ));
    }
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
                run_one(name, semaphore.acquire_owned().await, future).await
            }),
        ));
    }
    let outcomes = collect_outcomes(handles).await;
    for o in &outcomes {
        if let Some(line) = outcome_error_line(o) {
            log_line("error", &line);
        }
    }
    Ok(crate::output::Envelope::new(
        outcomes,
        flags.dry_run,
        &flags.command,
    ))
}

fn outcome_error_line(o: &AccountOutcome) -> Option<String> {
    o.error
        .as_ref()
        .map(|err| format!("{}: {}", o.account, err["message"].as_str().unwrap_or("")))
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

async fn run_one(
    name: String,
    permit: Result<tokio::sync::OwnedSemaphorePermit, tokio::sync::AcquireError>,
    future: impl std::future::Future<Output = TeleResult<serde_json::Value>>,
) -> TeleResult<AccountOutcome> {
    let _permit = permit.map_err(|e| TeleError::Other(e.to_string()))?;
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
}

async fn collect_one(
    name: String,
    handle: tokio::task::JoinHandle<TeleResult<AccountOutcome>>,
) -> AccountOutcome {
    match handle.await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(e)) => failed_outcome(name, e),
        Err(_) => failed_outcome(name, TeleError::Other("account task panicked".to_string())),
    }
}

async fn collect_outcomes(
    handles: Vec<(String, tokio::task::JoinHandle<TeleResult<AccountOutcome>>)>,
) -> Vec<AccountOutcome> {
    let mut outcomes = Vec::new();
    for (name, handle) in handles {
        outcomes.push(collect_one(name, handle).await);
    }
    outcomes.sort_by(|a, b| a.account.cmp(&b.account));
    outcomes
}

fn effective_parallel(flag: Option<u32>, cfg_max: u32) -> u32 {
    flag.unwrap_or(cfg_max).clamp(1, 3)
}

pub fn select_accounts(flags: &GlobalFlags) -> TeleResult<Vec<String>> {
    let cfg = crate::config::load_config(flags.config_path.as_deref())?;
    let sessions: BTreeSet<String> = crate::session::list_session_names().into_iter().collect();
    select_from(&cfg, &sessions, &flags.account, &flags.tag)
}

fn select_from(
    cfg: &crate::config::AppConfig,
    sessions: &BTreeSet<String>,
    accounts: &[String],
    tags: &[String],
) -> TeleResult<Vec<String>> {
    let configured: BTreeSet<String> = cfg.accounts.keys().cloned().collect();
    let mut selected: BTreeSet<String> = BTreeSet::new();
    for name in accounts {
        if name == "all" {
            selected.extend(sessions.iter().cloned());
            continue;
        }
        if !configured.contains(name) && !sessions.contains(name) {
            return Err(TeleError::Usage(format!("unknown account {name}")));
        }
        selected.insert(name.clone());
    }
    for tag in tags {
        let tagged: BTreeSet<String> = cfg
            .accounts
            .iter()
            .filter(|(_, a)| a.tags.iter().any(|t| t == tag))
            .map(|(n, _)| n.clone())
            .collect();
        if tagged.is_empty() {
            return Err(TeleError::Usage(format!("no accounts with tag {tag}")));
        }
        selected.extend(tagged.intersection(sessions).cloned());
    }
    if accounts.is_empty() && tags.is_empty() {
        selected = sessions.clone();
    }
    Ok(selected.into_iter().collect())
}

pub fn print_envelope(flags: &GlobalFlags, envelope: &crate::output::Envelope) -> TeleResult<()> {
    if flags.json || flags.jsonl {
        let value = serde_json::to_value(envelope)?;
        crate::output::print_json(&value)?;
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
    let usage = envelope.accounts.iter().any(|a| {
        a.error
            .as_ref()
            .is_some_and(|e| e["type"].as_str() == Some("UsageError"))
    });
    if usage {
        return EXIT_USAGE;
    }
    EXIT_ALL_FAILED
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::*;
    use crate::output::{AccountOutcome, Envelope};

    fn outcome(account: &str, ok: bool, code: Option<i32>) -> AccountOutcome {
        AccountOutcome {
            account: account.to_string(),
            ok,
            error: None,
            data: None,
            exit_code: code,
        }
    }

    fn usage_outcome(account: &str) -> AccountOutcome {
        AccountOutcome {
            account: account.to_string(),
            ok: false,
            error: Some(serde_json::json!({"type": "UsageError"})),
            data: None,
            exit_code: Some(EXIT_USAGE),
        }
    }

    fn envelope(accounts: Vec<AccountOutcome>) -> Envelope {
        Envelope::new(accounts, false, "msg send")
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn sessions(v: &[&str]) -> BTreeSet<String> {
        names(v).into_iter().collect()
    }

    fn cfg_with(accounts: &[(&str, Vec<&str>)]) -> crate::config::AppConfig {
        let accounts = accounts
            .iter()
            .map(|(name, tags)| {
                (
                    name.to_string(),
                    crate::config::AccountConfig {
                        tags: tags.iter().map(|t| t.to_string()).collect(),
                        proxy: None,
                    },
                )
            })
            .collect();
        crate::config::AppConfig {
            accounts,
            ..Default::default()
        }
    }

    fn pick(
        cfg: &crate::config::AppConfig,
        s: &[&str],
        accounts: &[&str],
        tags: &[&str],
    ) -> TeleResult<Vec<String>> {
        select_from(cfg, &sessions(s), &names(accounts), &names(tags))
    }

    #[test]
    fn no_flags_defaults_to_all_sessions() {
        let cfg = cfg_with(&[("home", vec!["iran"]), ("work", vec!["iran"])]);
        let got = pick(&cfg, &["home", "work"], &[], &[]).unwrap();
        assert_eq!(got, vec!["home", "work"]);
    }

    #[test]
    fn account_all_expands_to_all_sessions() {
        let cfg = cfg_with(&[]);
        let got = pick(&cfg, &["home", "work"], &["all"], &[]).unwrap();
        assert_eq!(got, vec!["home", "work"]);
    }

    #[test]
    fn account_all_with_no_sessions_selects_nothing() {
        let cfg = cfg_with(&[]);
        assert!(pick(&cfg, &[], &["all"], &[]).unwrap().is_empty());
    }

    #[test]
    fn repeated_accounts_union_sorted_deduped() {
        let cfg = cfg_with(&[("home", vec![]), ("work", vec![])]);
        let got = pick(&cfg, &["home", "work"], &["work", "home", "work"], &[]).unwrap();
        assert_eq!(got, vec!["home", "work"]);
    }

    #[test]
    fn configured_only_account_is_accepted() {
        let cfg = cfg_with(&[("pending", vec![])]);
        let got = pick(&cfg, &["work"], &["work", "pending"], &[]).unwrap();
        assert_eq!(got, vec!["pending", "work"]);
    }

    #[test]
    fn unknown_account_is_usage_error() {
        let cfg = cfg_with(&[("work", vec![])]);
        let err = pick(&cfg, &["work"], &["bogus"], &[]).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.message().contains("unknown account bogus"));
    }

    #[test]
    fn tag_unions_with_account_flags() {
        let cfg = cfg_with(&[("home", vec!["iran"]), ("work", vec!["iran"])]);
        let got = pick(&cfg, &["home", "work"], &["work"], &["iran"]).unwrap();
        assert_eq!(got, vec!["home", "work"]);
    }

    #[test]
    fn tag_selects_only_tagged_accounts_with_sessions() {
        let cfg = cfg_with(&[("home", vec!["iran"]), ("pending", vec!["iran"])]);
        let got = pick(&cfg, &["home"], &[], &["iran"]).unwrap();
        assert_eq!(got, vec!["home"]);
    }

    #[test]
    fn unknown_tag_is_usage_error() {
        let cfg = cfg_with(&[("home", vec!["iran"])]);
        let err = pick(&cfg, &["home"], &[], &["nosuch"]).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.message().contains("no accounts with tag nosuch"));
    }

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

    #[test]
    fn flag_above_three_clamps_to_three() {
        assert_eq!(effective_parallel(Some(4), 1), 3);
        assert_eq!(effective_parallel(Some(99), 3), 3);
    }

    #[test]
    fn all_auth_failures_exit_auth() {
        let env = envelope(vec![
            outcome("a", false, Some(EXIT_AUTH)),
            outcome("b", false, Some(EXIT_AUTH)),
        ]);
        assert_eq!(envelope_exit_code(&env), EXIT_AUTH);
    }

    #[test]
    fn all_failed_exits_all_failed_even_with_auth_mix() {
        let env = envelope(vec![
            outcome("a", false, Some(EXIT_AUTH)),
            outcome("b", false, Some(EXIT_ALL_FAILED)),
        ]);
        assert_eq!(envelope_exit_code(&env), EXIT_ALL_FAILED);
    }

    #[test]
    fn mixed_success_and_auth_failure_is_partial() {
        let env = envelope(vec![
            outcome("a", true, None),
            outcome("b", false, Some(EXIT_AUTH)),
        ]);
        assert_eq!(envelope_exit_code(&env), EXIT_PARTIAL);
    }

    #[test]
    fn all_success_exits_ok() {
        let env = envelope(vec![outcome("a", true, None), outcome("b", true, None)]);
        assert_eq!(envelope_exit_code(&env), EXIT_OK);
    }

    #[test]
    fn single_success_exits_ok() {
        let env = envelope(vec![outcome("a", true, None)]);
        assert_eq!(envelope_exit_code(&env), EXIT_OK);
    }

    #[test]
    fn single_failure_exits_all_failed() {
        let env = envelope(vec![outcome("a", false, Some(EXIT_ALL_FAILED))]);
        assert_eq!(envelope_exit_code(&env), EXIT_ALL_FAILED);
    }

    #[test]
    fn single_auth_failure_exits_auth() {
        let env = envelope(vec![outcome("a", false, Some(EXIT_AUTH))]);
        assert_eq!(envelope_exit_code(&env), EXIT_AUTH);
    }

    #[test]
    fn some_ok_some_failed_is_partial() {
        let env = envelope(vec![
            outcome("a", true, None),
            outcome("b", false, Some(EXIT_ALL_FAILED)),
        ]);
        assert_eq!(envelope_exit_code(&env), EXIT_PARTIAL);
    }

    #[test]
    fn usage_among_failures_exits_usage() {
        let env = envelope(vec![
            usage_outcome("a"),
            outcome("b", false, Some(EXIT_ALL_FAILED)),
        ]);
        assert_eq!(envelope_exit_code(&env), EXIT_USAGE);
    }

    #[test]
    fn telegram_and_other_failures_exit_all_failed() {
        let env = envelope(vec![
            outcome("a", false, Some(EXIT_ALL_FAILED)),
            outcome("b", false, Some(EXIT_ALL_FAILED)),
        ]);
        assert_eq!(envelope_exit_code(&env), EXIT_ALL_FAILED);
    }

    #[test]
    fn single_usage_failure_exits_usage() {
        let env = envelope(vec![usage_outcome("a")]);
        assert_eq!(envelope_exit_code(&env), EXIT_USAGE);
    }

    #[test]
    fn all_usage_failures_exit_usage() {
        let env = envelope(vec![usage_outcome("a"), usage_outcome("b")]);
        assert_eq!(envelope_exit_code(&env), EXIT_USAGE);
    }

    #[test]
    fn auth_mixed_with_usage_exits_usage() {
        let env = envelope(vec![
            outcome("a", false, Some(EXIT_AUTH)),
            usage_outcome("b"),
        ]);
        assert_eq!(envelope_exit_code(&env), EXIT_USAGE);
    }

    #[test]
    fn config_failure_exits_all_failed_not_usage() {
        let env = envelope(vec![AccountOutcome {
            account: "a".to_string(),
            ok: false,
            error: Some(serde_json::json!({"type": "ConfigError"})),
            data: None,
            exit_code: Some(EXIT_USAGE),
        }]);
        assert_eq!(envelope_exit_code(&env), EXIT_ALL_FAILED);
    }

    #[test]
    fn usage_mixed_with_success_is_partial() {
        let env = envelope(vec![outcome("a", true, None), usage_outcome("b")]);
        assert_eq!(envelope_exit_code(&env), EXIT_PARTIAL);
    }

    #[test]
    fn empty_envelope_is_vacuous_ok() {
        let env = envelope(vec![]);
        assert_eq!(envelope_exit_code(&env), EXIT_OK);
    }

    #[test]
    fn outcome_error_line_formats_account_and_message() {
        let o = AccountOutcome {
            account: "a".to_string(),
            ok: false,
            error: Some(serde_json::json!({
                "type": "AuthError",
                "message": "session invalid"
            })),
            data: None,
            exit_code: Some(EXIT_AUTH),
        };
        assert_eq!(
            outcome_error_line(&o),
            Some("a: session invalid".to_string())
        );
    }

    #[test]
    fn outcome_error_line_none_for_success_outcome() {
        assert_eq!(outcome_error_line(&outcome("b", true, None)), None);
    }

    #[test]
    fn outcome_error_line_handles_missing_message_key() {
        let o = AccountOutcome {
            account: "a".to_string(),
            ok: false,
            error: Some(serde_json::json!({"type": "Error"})),
            data: None,
            exit_code: Some(EXIT_ALL_FAILED),
        };
        assert_eq!(outcome_error_line(&o), Some("a: ".to_string()));
    }

    async fn permit() -> Result<tokio::sync::OwnedSemaphorePermit, tokio::sync::AcquireError> {
        Ok(Arc::new(tokio::sync::Semaphore::new(1))
            .acquire_owned()
            .await
            .unwrap())
    }

    fn ok_data() -> TeleResult<serde_json::Value> {
        Ok(serde_json::json!({"sent": true}))
    }

    #[tokio::test]
    async fn successful_task_yields_ok_outcome() {
        let outcome = run_one("a".to_string(), permit().await, async { ok_data() })
            .await
            .unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.account, "a");
        assert_eq!(outcome.data, Some(serde_json::json!({"sent": true})));
        assert_eq!(outcome.exit_code, None);
        assert!(outcome.error.is_none());
    }

    #[tokio::test]
    async fn erroring_task_yields_failed_outcome() {
        let handle = tokio::task::spawn(async {
            run_one("a".to_string(), permit().await, async {
                Err(TeleError::Auth("session invalid".to_string()))
            })
            .await
        });
        let outcome = collect_one("a".to_string(), handle).await;
        assert!(!outcome.ok);
        assert_eq!(outcome.exit_code, Some(EXIT_AUTH));
        assert_eq!(outcome.error.unwrap()["type"], "AuthError");
    }

    #[tokio::test]
    async fn panicking_task_yields_failed_outcome_not_abort() {
        let handle = tokio::task::spawn(async {
            run_one("a".to_string(), permit().await, async { panic!("boom") }).await
        });
        let outcome = collect_one("a".to_string(), handle).await;
        assert!(!outcome.ok);
        assert_eq!(outcome.exit_code, Some(EXIT_ALL_FAILED));
        let error = outcome.error.unwrap();
        assert_eq!(error["type"], "Error");
        assert_eq!(error["message"], "account task panicked");
    }

    #[tokio::test]
    async fn closed_semaphore_yields_error_outcome() {
        let sem = Arc::new(tokio::sync::Semaphore::new(1));
        sem.close();
        let outcome = run_one("a".to_string(), sem.acquire_owned().await, async {
            ok_data()
        })
        .await
        .unwrap_err();
        assert!(matches!(outcome, TeleError::Other(_)));
    }

    #[tokio::test]
    async fn outcomes_collected_in_account_order_regardless_of_completion() {
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let slow = tokio::task::spawn(async move {
            let _ = wait.await;
            run_one("b".to_string(), permit().await, async { ok_data() }).await
        });
        let fast = tokio::task::spawn(async {
            run_one("a".to_string(), permit().await, async { ok_data() }).await
        });
        let _ = release.send(());
        let outcomes =
            collect_outcomes(vec![("b".to_string(), slow), ("a".to_string(), fast)]).await;
        let names: Vec<&str> = outcomes.iter().map(|o| o.account.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
        assert!(outcomes.iter().all(|o| o.ok));
    }

    #[tokio::test]
    async fn panicking_task_does_not_abort_other_accounts() {
        let panicking = tokio::task::spawn(async {
            run_one("a".to_string(), permit().await, async { panic!("boom") }).await
        });
        let healthy = tokio::task::spawn(async {
            run_one("b".to_string(), permit().await, async { ok_data() }).await
        });
        let outcomes = collect_outcomes(vec![
            ("a".to_string(), panicking),
            ("b".to_string(), healthy),
        ])
        .await;
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].account, "a");
        assert!(!outcomes[0].ok);
        assert_eq!(outcomes[1].account, "b");
        assert!(outcomes[1].ok);
    }
}
