# BUG-1: Rate-limiter zero-budget silent hang

**Source:** deep-dive kernel review, HIGH finding.

## Question/Problem

`RateLimiter::new` maps `rpc_per_minute <= 0` to `capacity = 0`; `acquire()` then spins in its 100ms-timeout loop forever. Any command for an account with `rpc_per_minute = "0"` (or negative) in config freezes silently — no error, no log.

## Acceptance criteria

- [ ] `budget <= 0` never produces a hang: treat as unlimited (documented choice) OR reject at config read (`read_config`) with a clear Usage error — pick one, test it.
- [ ] RED test first: zero/negative budget case fails before fix, passes after.
- [ ] Existing rate-limiter tests still pass; no behavior change for positive budgets.

## Verification

- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo test`

## Files

- `src/rate_limiter.rs`
- possibly `src/config.rs` (only if validation-at-read chosen)

## Constraints

Branch `fix/bug-1-rate-limiter`. Commit prefix `fix:`. No comments in code.
