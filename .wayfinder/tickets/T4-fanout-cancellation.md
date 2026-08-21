# T4 — fix/fanout-cancellation

Status: closed
Labels: wayfinder:task
Branch: fix/t4-fanout-cancellation
Blocked by: T3

## Question

Task lifetimes at the edges of the fanout are unowned and cancellation is incidental — what closes them structurally without changing sequential-default behavior?

## Scope

1. executor.rs:39-51 — replace Vec<JoinHandle> collection with `tokio::task::JoinSet` so Ctrl+C (main.rs select! dropping run_command) aborts children structurally; outcomes still collected via join_next. Keep deterministic output ordering (sorted by account, tested executor.rs:119).
2. client.rs:48 — SenderPool runner `tokio::spawn` handle is discarded; store it in ClientGuard, add explicit async close() that disconnects then awaits runner with timeout; Drop stays best-effort fallback.
3. executor.rs:85 — AcquireError mapped to TaskPanic mislabels closed-semaphore as panic; give it an honest variant/mapping (check error.rs taxonomy + exit-code matrix before choosing; keep exit codes stable).
4. rate_limiter.rs record_flood is dead in production (AutoSleep retry policy client.rs:50-53 handles FloodWait). DECISION: remove cooldown machinery (record_flood + cooldown fields/tests) rather than rewiring RPC paths; document AutoSleep as sole FloodWait/SlowModeWait mechanism in code-adjacent docs if a natural home exists. Token-bucket rate gating itself STAYS.
5. Exit-aggregator consolidation: listen.rs:317 aggregate_exit ("all failures usage → 1") vs executor.rs:212 envelope_exit_code ("any usage row → 1"). Consolidate on ONE function (shared home: commands/helpers.rs or error.rs); preserve each caller's documented exit semantics only if they are intentionally different — otherwise unify on executor's rule and update contract doc note via T5.

## Done when

Ctrl+C aborts fanout tasks promptly (test with JoinSet abort semantics where testable offline); clippy/fmt/tests green.

## Notes

Do not touch listen.rs streaming loop beyond aggregate_exit consolidation (T6 owns shutdown test).
