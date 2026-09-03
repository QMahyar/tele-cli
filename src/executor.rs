use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::error::{TeleError, TeleResult};
use crate::output::{log_line, AccountOutcome};

const ACCOUNT_TIMEOUT_SECS: u64 = 300;

#[derive(Clone)]
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
        + Sync
        + 'static,
) -> TeleResult<crate::output::Envelope> {
    let cfg = crate::config::load_config(flags.config_path.as_deref())?;
    let names = select_accounts_from_cfg(&cfg, flags)?;
    if names.is_empty() {
        return Err(TeleError::Usage(
            "no accounts selected: use --account <name> or --tag <tag>".to_string(),
        ));
    }
    let parallel = effective_parallel(flags.parallel, cfg.parallel_max)? as usize;
    let semaphore = Arc::new(Semaphore::new(parallel));
    let handler = Arc::new(handler);
    let mut handles: Vec<(String, tokio::task::JoinHandle<TeleResult<AccountOutcome>>)> =
        Vec::new();
    for name in names {
        let semaphore = Arc::clone(&semaphore);
        let handler = Arc::clone(&handler);
        handles.push((
            name.clone(),
            tokio::task::spawn(async move {
                let permit = semaphore.acquire_owned().await;
                let future = handler(name.clone());
                run_one(name, permit, future).await
            }),
        ));
    }
    let outcomes = collect_outcomes(&mut handles).await;
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

struct AbortOnDrop<'a>(&'a mut Vec<(String, tokio::task::JoinHandle<TeleResult<AccountOutcome>>)>);

impl Drop for AbortOnDrop<'_> {
    fn drop(&mut self) {
        for (_, handle) in self.0.iter() {
            handle.abort();
        }
    }
}

fn outcome_error_line(o: &AccountOutcome) -> Option<String> {
    o.error.as_ref().map(|err| {
        format!(
            "{}: {}",
            o.account,
            err["message"].as_str().unwrap_or("<unprintable error>")
        )
    })
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
    let _permit = permit.map_err(|e| TeleError::Other(format!("internal semaphore error: {e}")))?;
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

async fn collect_outcomes(
    handles: &mut Vec<(String, tokio::task::JoinHandle<TeleResult<AccountOutcome>>)>,
) -> Vec<AccountOutcome> {
    collect_outcomes_with_budget(handles, Duration::from_secs(ACCOUNT_TIMEOUT_SECS)).await
}

async fn collect_outcomes_with_budget(
    handles: &mut Vec<(String, tokio::task::JoinHandle<TeleResult<AccountOutcome>>)>,
    budget: Duration,
) -> Vec<AccountOutcome> {
    let mut outcomes: Vec<AccountOutcome> = Vec::new();
    {
        let _abort_guard = AbortOnDrop(handles);
        for (name, handle) in _abort_guard.0.iter_mut() {
            let outcome = match tokio::time::timeout(budget, handle).await {
                Ok(joined) => match joined {
                    Ok(Ok(outcome)) => outcome,
                    Ok(Err(e)) => failed_outcome(name.clone(), e),
                    Err(e) if e.is_cancelled() => failed_outcome(
                        name.clone(),
                        TeleError::Other("account task cancelled".to_string()),
                    ),
                    Err(e) => {
                        let msg = match e.try_into_panic() {
                            Ok(payload) => {
                                if let Some(s) = payload.downcast_ref::<&str>() {
                                    (*s).to_string()
                                } else if let Some(s) = payload.downcast_ref::<String>() {
                                    s.clone()
                                } else {
                                    "account task panicked".to_string()
                                }
                            }
                            Err(_) => "account task panicked".to_string(),
                        };
                        failed_outcome(name.clone(), TeleError::TaskPanic(msg))
                    }
                },
                Err(_) => failed_outcome(
                    name.clone(),
                    TeleError::Timeout(format!(
                        "account task exceeded {}s deadline",
                        budget.as_secs()
                    )),
                ),
            };
            outcomes.push(outcome);
        }
    }
    outcomes.sort_by(|a, b| a.account.cmp(&b.account));
    outcomes
}

pub fn effective_parallel(flag: Option<u32>, cfg_max: u32) -> TeleResult<u32> {
    let p = flag.unwrap_or(cfg_max);
    if !(1..=32).contains(&p) {
        return Err(TeleError::Usage(format!(
            "--parallel {p} must be between 1 and 32"
        )));
    }
    Ok(p)
}

pub fn select_accounts(flags: &GlobalFlags) -> TeleResult<Vec<String>> {
    let cfg = crate::config::load_config(flags.config_path.as_deref())?;
    select_accounts_from_cfg(&cfg, flags)
}

pub fn require_explicit_selection(command: &str, flags: &GlobalFlags) -> TeleResult<()> {
    if flags.account.is_empty() && flags.tag.is_empty() {
        return Err(TeleError::Usage(format!(
            "{command} requires --account <name> or --tag <tag>"
        )));
    }
    Ok(())
}

fn select_accounts_from_cfg(
    cfg: &crate::config::AppConfig,
    flags: &GlobalFlags,
) -> TeleResult<Vec<String>> {
    let sessions: BTreeSet<String> = crate::session::list_session_names().into_iter().collect();
    select_from(cfg, &sessions, &flags.account, &flags.tag)
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

pub fn select_sessions(
    cfg: &crate::config::AppConfig,
    sessions: &[String],
    accounts: &[String],
    tags: &[String],
) -> TeleResult<Vec<String>> {
    let session_set: BTreeSet<String> = sessions.iter().cloned().collect();
    let selected = select_from(cfg, &session_set, accounts, tags)?;
    Ok(selected
        .into_iter()
        .filter(|n| session_set.contains(n))
        .collect())
}

pub fn finish(flags: &GlobalFlags, envelope: &crate::output::Envelope) -> TeleResult<i32> {
    if flags.json || flags.jsonl {
        let value = serde_json::to_value(envelope)?;
        crate::output::print_json(&value)?;
    }
    Ok(envelope_exit_code(envelope))
}

pub fn envelope_exit_code(envelope: &crate::output::Envelope) -> i32 {
    use crate::error::*;
    if envelope.ok {
        let partial = envelope.accounts.iter().any(|a| {
            a.data
                .as_ref()
                .is_some_and(|d| d.get("partial").and_then(|p| p.as_bool()) == Some(true))
        });
        return if partial { EXIT_PARTIAL } else { EXIT_OK };
    }
    let ok_count = envelope.accounts.iter().filter(|a| a.ok).count();
    let failed: Vec<i32> = envelope
        .accounts
        .iter()
        .filter(|a| !a.ok)
        .map(|a| a.exit_code.unwrap_or(EXIT_ALL_FAILED))
        .collect();
    aggregate_exit_code(ok_count, &failed)
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
                        ..Default::default()
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

    fn pick_list(
        cfg: &crate::config::AppConfig,
        s: &[&str],
        accounts: &[&str],
        tags: &[&str],
    ) -> TeleResult<Vec<String>> {
        select_sessions(cfg, &names(s), &names(accounts), &names(tags))
    }

    #[test]
    fn list_explicit_account_filters_to_that_session() {
        let cfg = cfg_with(&[("home", vec!["iran"]), ("work", vec!["iran"])]);
        let got = pick_list(&cfg, &["home", "work"], &["home"], &[]).unwrap();
        assert_eq!(got, vec!["home"]);
    }

    #[test]
    fn list_configured_account_without_session_selects_nothing() {
        let cfg = cfg_with(&[("home", vec![]), ("pending", vec![])]);
        let got = pick_list(&cfg, &["home"], &["pending"], &[]).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn list_unknown_account_is_usage_error() {
        let cfg = cfg_with(&[("home", vec![])]);
        let err = pick_list(&cfg, &["home"], &["bogus"], &[]).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.message().contains("unknown account bogus"));
    }

    #[test]
    fn list_unknown_tag_is_usage_error() {
        let cfg = cfg_with(&[("home", vec!["iran"])]);
        let err = pick_list(&cfg, &["home"], &[], &["nosuch"]).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(err.message().contains("no accounts with tag nosuch"));
    }

    #[test]
    fn list_no_selection_keeps_all_sessions() {
        let cfg = cfg_with(&[("home", vec![]), ("work", vec![])]);
        let got = pick_list(&cfg, &["home", "work", "orphan"], &[], &[]).unwrap();
        assert_eq!(got, vec!["home", "orphan", "work"]);
    }

    #[test]
    fn list_account_all_expands_to_all_sessions() {
        let cfg = cfg_with(&[]);
        let got = pick_list(&cfg, &["home", "work"], &["all"], &[]).unwrap();
        assert_eq!(got, vec!["home", "work"]);
    }

    #[test]
    fn list_tag_filters_to_tagged_sessions() {
        let cfg = cfg_with(&[("home", vec!["iran"]), ("work", vec!["us"])]);
        let got = pick_list(&cfg, &["home", "work"], &[], &["iran"]).unwrap();
        assert_eq!(got, vec!["home"]);
    }

    fn global_flags(accounts: &[&str], tags: &[&str]) -> GlobalFlags {
        GlobalFlags {
            account: names(accounts),
            tag: names(tags),
            parallel: None,
            json: false,
            jsonl: false,
            dry_run: false,
            quiet: false,
            config_path: None,
            command: "listen".to_string(),
        }
    }

    #[test]
    fn explicit_selection_guard_rejects_empty_flags() {
        let err = require_explicit_selection("listen", &global_flags(&[], &[])).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE);
        assert!(
            err.message().contains("listen requires --account"),
            "err: {err}"
        );
    }

    #[test]
    fn explicit_selection_guard_error_names_command() {
        let err =
            require_explicit_selection("takeout finish", &global_flags(&[], &[])).unwrap_err();
        assert!(err.message().contains("takeout finish"), "err: {err}");
    }

    #[test]
    fn explicit_selection_guard_accepts_account_name() {
        let flags = global_flags(&["work"], &[]);
        assert!(require_explicit_selection("listen", &flags).is_ok());
    }

    #[test]
    fn explicit_selection_guard_accepts_account_all() {
        let flags = global_flags(&["all"], &[]);
        assert!(require_explicit_selection("listen", &flags).is_ok());
    }

    #[test]
    fn explicit_selection_guard_accepts_tag() {
        let flags = global_flags(&[], &["iran"]);
        assert!(require_explicit_selection("takeout start", &flags).is_ok());
    }

    #[test]
    fn explicit_selection_guard_accepts_account_and_tag_together() {
        let flags = global_flags(&["home"], &["iran"]);
        assert!(require_explicit_selection("takeout export", &flags).is_ok());
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
        assert_eq!(effective_parallel(Some(1), 3).unwrap(), 1);
        assert_eq!(effective_parallel(Some(2), 1).unwrap(), 2);
        assert_eq!(effective_parallel(Some(32), 1).unwrap(), 32);
    }

    #[test]
    fn config_is_fallback_default() {
        assert_eq!(effective_parallel(None, 1).unwrap(), 1);
        assert_eq!(effective_parallel(None, 2).unwrap(), 2);
        assert_eq!(effective_parallel(None, 32).unwrap(), 32);
    }

    #[test]
    fn parallel_flag_below_one_errors() {
        assert!(matches!(
            effective_parallel(Some(0), 3),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn parallel_flag_above_thirty_two_errors() {
        assert!(matches!(
            effective_parallel(Some(33), 1),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            effective_parallel(Some(99), 3),
            Err(TeleError::Usage(_))
        ));
    }

    #[test]
    fn parallel_config_out_of_range_errors() {
        assert!(matches!(
            effective_parallel(None, 0),
            Err(TeleError::Usage(_))
        ));
        assert!(matches!(
            effective_parallel(None, 999),
            Err(TeleError::Usage(_))
        ));
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
    fn auth_failure_outranks_telegram_failure_in_all_failed_mix() {
        let env = envelope(vec![
            outcome("a", false, Some(EXIT_AUTH)),
            outcome("b", false, Some(EXIT_ALL_FAILED)),
        ]);
        assert_eq!(envelope_exit_code(&env), EXIT_AUTH);
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
    fn all_ok_with_partial_data_exits_partial() {
        let partial = AccountOutcome {
            account: "a".to_string(),
            ok: true,
            error: None,
            data: Some(serde_json::json!({"deleted": 1, "partial": true})),
            exit_code: None,
        };
        let env = envelope(vec![partial, outcome("b", true, None)]);
        assert_eq!(envelope_exit_code(&env), EXIT_PARTIAL);
    }

    #[test]
    fn all_ok_without_partial_data_exits_ok() {
        let env = envelope(vec![outcome("a", true, None)]);
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
    fn telegram_failure_outranks_usage_among_failures() {
        let env = envelope(vec![
            usage_outcome("a"),
            outcome("b", false, Some(EXIT_ALL_FAILED)),
        ]);
        assert_eq!(envelope_exit_code(&env), EXIT_ALL_FAILED);
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
    fn auth_failure_outranks_usage_among_failures() {
        let env = envelope(vec![
            outcome("a", false, Some(EXIT_AUTH)),
            usage_outcome("b"),
        ]);
        assert_eq!(envelope_exit_code(&env), EXIT_AUTH);
    }

    #[test]
    fn config_failure_exits_usage() {
        let env = envelope(vec![AccountOutcome {
            account: "a".to_string(),
            ok: false,
            error: Some(serde_json::json!({"type": "ConfigError"})),
            data: None,
            exit_code: Some(EXIT_USAGE),
        }]);
        assert_eq!(envelope_exit_code(&env), EXIT_USAGE);
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
    fn failed_outcome_missing_exit_code_defaults_to_all_failed() {
        let env = envelope(vec![AccountOutcome {
            account: "a".to_string(),
            ok: false,
            error: Some(serde_json::json!({"type": "Error"})),
            data: None,
            exit_code: None,
        }]);
        assert_eq!(envelope_exit_code(&env), EXIT_ALL_FAILED);
    }

    #[test]
    fn broken_pipe_exit_ok_filtered_from_failures() {
        let env = envelope(vec![
            outcome("a", false, Some(EXIT_OK)),
            outcome("b", false, Some(EXIT_ALL_FAILED)),
        ]);
        assert_eq!(envelope_exit_code(&env), EXIT_ALL_FAILED);
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
        assert_eq!(
            outcome_error_line(&o),
            Some("a: <unprintable error>".to_string())
        );
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
        let outcome = handle.await.unwrap().unwrap_err();
        let outcome = failed_outcome("a".to_string(), outcome);
        assert!(!outcome.ok);
        assert_eq!(outcome.exit_code, Some(EXIT_AUTH));
        assert_eq!(outcome.error.unwrap()["type"], "AuthError");
    }

    #[tokio::test]
    async fn panicking_task_yields_failed_outcome_not_abort() {
        let handle = tokio::task::spawn(async {
            run_one("a".to_string(), permit().await, async { panic!("boom") }).await
        });
        let joined = handle.await;
        assert!(joined.is_err(), "panicked task joins with JoinError");
        let outcome = failed_outcome(
            "a".to_string(),
            TeleError::TaskPanic("account task panicked".to_string()),
        );
        assert!(!outcome.ok);
        assert_eq!(outcome.exit_code, Some(EXIT_ALL_FAILED));
        let error = outcome.error.unwrap();
        assert_eq!(error["type"], "TaskPanicError");
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
        assert!(matches!(outcome, TeleError::Other(m) if m.contains("semaphore")));
    }

    #[tokio::test]
    async fn dropping_collection_aborts_pending_account_tasks() {
        let mut handles: Vec<(String, tokio::task::JoinHandle<TeleResult<AccountOutcome>>)> =
            vec![(
                "hang".to_string(),
                tokio::task::spawn(async {
                    let _ = std::future::pending::<()>().await;
                    unreachable!("aborted task must never complete")
                }),
            )];
        let collected = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            collect_outcomes(&mut handles),
        )
        .await;
        assert!(collected.is_err(), "hanging task must time out the collect");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !handles[0].1.is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "dropping the collection must abort pending account tasks"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
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
            collect_outcomes(&mut vec![("b".to_string(), slow), ("a".to_string(), fast)]).await;
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
        let outcomes = collect_outcomes(&mut vec![
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

    #[tokio::test(start_paused = true)]
    async fn hung_account_task_times_out_into_failed_outcome() {
        let hang = tokio::task::spawn(async {
            run_one("a".to_string(), permit().await, async {
                let _ = std::future::pending::<()>().await;
                unreachable!()
            })
            .await
        });
        let healthy = tokio::task::spawn(async {
            run_one("b".to_string(), permit().await, async { ok_data() }).await
        });
        let outcomes = collect_outcomes_with_budget(
            &mut vec![("a".to_string(), hang), ("b".to_string(), healthy)],
            Duration::from_millis(50),
        )
        .await;
        assert_eq!(outcomes.len(), 2, "healthy sibling still collected");
        let hung = &outcomes[0];
        assert_eq!(hung.account, "a");
        assert!(!hung.ok);
        let error = hung.error.as_ref().unwrap();
        assert_eq!(error["type"], "Timeout");
        assert!(
            error["message"].as_str().unwrap().contains("deadline"),
            "err: {error}"
        );
        assert_eq!(hung.exit_code, Some(EXIT_ALL_FAILED));
        assert!(outcomes[1].ok);
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_budget_greater_than_task_runs_to_completion() {
        let handle = tokio::task::spawn(async {
            run_one("a".to_string(), permit().await, async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                ok_data()
            })
            .await
        });
        let outcomes = collect_outcomes_with_budget(
            &mut vec![("a".to_string(), handle)],
            Duration::from_secs(300),
        )
        .await;
        assert!(outcomes[0].ok, "task finishing inside budget is ok");
    }

    #[tokio::test]
    async fn default_parallel_1_runs_tasks_sequentially() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;
        let max_concurrent = Arc::new(AtomicUsize::new(0));
        let running = Arc::new(AtomicUsize::new(0));
        let cfg = crate::config::AppConfig {
            parallel_max: 1,
            ..Default::default()
        };
        let parallel = effective_parallel(None, cfg.parallel_max).unwrap() as usize;
        assert_eq!(parallel, 1);
        let semaphore = Arc::new(Semaphore::new(parallel));
        let mut handles = Vec::new();
        for i in 0..3 {
            let sem = Arc::clone(&semaphore);
            let running = Arc::clone(&running);
            let max_concurrent = Arc::clone(&max_concurrent);
            handles.push(tokio::task::spawn(async move {
                let _permit = sem.acquire_owned().await.unwrap();
                let prev = running.fetch_add(1, Ordering::SeqCst);
                let cur = prev + 1;
                max_concurrent.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                running.fetch_sub(1, Ordering::SeqCst);
                Ok::<_, TeleError>(serde_json::json!({"i": i}))
            }));
        }
        for h in handles {
            h.await.unwrap().unwrap();
        }
        assert_eq!(max_concurrent.load(Ordering::SeqCst), 1);
    }
}
