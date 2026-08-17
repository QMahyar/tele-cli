# ADR-007: Per-account flood weights

## Status
Accepted (supersedes ADR-004)

## Date
2026-08-14

## Context
ADR-004 enforced a single global semaphore for parallel accounts and deferred a token-bucket until measured. With multi-account fan-out now production traffic, the global semaphore over-throttles safe accounts when one account hits a FLOOD_WAIT, and under-throttles accounts with tight rate limits.

## Decision
Replace the global semaphore with a per-account rate limiter (token bucket + flood cooldown) inside `ClientGuard`. Each account gets its own `Arc<RateLimiter>` created during `connect`, resolved from `AccountConfig { rpc_per_minute, flood_sleep_threshold }` with global fallbacks.

New config keys (additive, backward-compatible):
- `[accounts.<name>] rpc_per_minute: Option<f64>` — token bucket budget. `None` = unlimited.
- `[accounts.<name>] flood_sleep_threshold: Option<u64>` — per-account AutoSleep threshold. `None` = global `flood_sleep_threshold`.

Global `parallel_max` clamp raised to 1..=32 (was 1..=3). `--parallel` now selects how many accounts to run concurrently; per-account rate limiters handle inter-RPC throttling.

Command handlers call `guard.rate_limiter.acquire().await` before each RPC batch. Login/logout flows and `listen` are exempt (auth probes and long-lived streams).

## Alternatives considered

### Wrap every Client method
Rejected: grammers Client has dozens of methods; wrapping each couples tele to the grammers API surface.

### Global semaphore + per-account sleep
Rejected: still blocks unrelated accounts during a flood cooldown.

### Inline semaphore per task
Rejected: duplicates tokio Semaphore semantics already available in rate_limiter.

## Consequences
- Per-account isolation: one flooded account does not block siblings.
- Callers must explicitly call `acquire()` before RPC batches — a one-line addition per handler.
- Flood cooldown is advisory: `record_flood(seconds)` is available but not auto-called on FLOOD_WAIT; the existing JSON `error.seconds` field surfaces the wait.
- ADR-004 is superseded; its history is preserved.
