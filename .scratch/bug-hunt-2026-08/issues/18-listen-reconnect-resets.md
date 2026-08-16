# 18 — listen reconnect resets failure counters; connect inside retry loop

**What to build:** `tele listen` must not give up after 5 reconnect cycles on a flapping connection when every reconnect succeeded — the failure/backoff counters must reset after a successful reconnect. And a transient failure at connect/authorize time (not just mid-stream) must go through the same bounded backoff instead of killing the account immediately.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: successful reconnect resets failure count/backoff
- [ ] Offline test: connect-phase failure enters the retry loop (bounded attempts, then give-up)
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md findings 12.7 (counters reset only on received update) and 12.8 (connect errors outside loop)