# ADR-008: Per-account flood weights

## Status
Accepted (supersedes ADR-004)

## Date
2026-08-17

## Context
ADR-004 enforced a single global semaphore for parallel accounts and deferred a token-bucket until measured. With multi-account fan-out now production traffic, the global semaphore over-throttles safe accounts when one account hits a FLOOD_WAIT, and under-throttles accounts with tight rate limits.

## Decision
Layer a per-account rate limiter (token bucket + flood cooldown) on top of the existing global concurrency semaphore. The semaphore (size = `effective_parallel`, default 1, clamp 1..=32) caps how many accounts run concurrently. The per-account `RateLimiter` inside `ClientGuard` throttles inter-RPC frequency for each account independently.

New config keys (additive, backward-compatible):
- `[accounts.<name>] rpc_per_minute: Option<f64>` — token bucket budget. `None` = unlimited.
- `[accounts.<name>] flood_sleep_threshold: Option<u64>` — per-account AutoSleep threshold. `None` = global `flood_sleep_threshold`.

Token bucket semantics: `capacity = ceil(budget)`, refill rate = `budget / 60` tokens per second. Fractional credit accumulates across poll cycles (last_refill only advances when >= 1 token is granted). Parked waiters are woken via `Notify::notify_waiters()` when tokens are refilled, with a 100ms timed poll fallback to prevent lost notifications.

Acquire granularity: **handler-rate**, not RPC-rate. Each command handler calls `guard.rate_limiter.acquire().await` once before its RPC batch. A handler that issues N RPCs (batch delete, forward chunks, iterated search) consumes 1 token for the entire batch. This keeps per-callsite churn to one line and matches the natural unit of work (one CLI invocation per account).

The global semaphore remains as the outer concurrency cap — `--parallel` controls how many accounts run concurrently (default 1 = sequential). The per-account rate limiter handles finer-grained throttling within each account's task.

Login/logout flows and `listen` are exempt (auth probes and long-lived streams).

## Alternatives considered

### Wrap every Client method
Rejected: grammers Client has dozens of methods; wrapping each couples tele to the grammers API surface.

### Global semaphore + per-account sleep
Rejected: still blocks unrelated accounts during a flood cooldown.

### Per-RPC acquire (inside batch loops)
Rejected: multiplies callsite churn for every batched handler; the CLI invocation is the natural rate-limit unit, not individual RPCs.

### Inline semaphore per task
Rejected: duplicates tokio Semaphore semantics already available in rate_limiter.

## Consequences
- Per-account isolation: one flooded account does not block siblings.
- Default sequential safety preserved: `--parallel` semaphore defaults to 1, clamped 1..=32.
- Callers add one line per handler (`guard.rate_limiter.acquire().await`) — minimal callsite churn.
- Flood cooldown is advisory: `record_flood(seconds)` is available but not auto-called on FLOOD_WAIT; the existing JSON `error.seconds` field surfaces the wait.
- ADR-004 is superseded; its history is preserved.

## Amendment (2026-08)
The advisory flood-cooldown half (`record_flood` / cooldown window) was removed:
it was never wired into any RPC path and FloodWait/SlowModeWait is handled
solely by the grammers `AutoSleep` retry policy configured in `src/client.rs`.
The token-bucket rate limiter itself remains unchanged.
