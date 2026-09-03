# Ticket: Slice 4 — Architecture cleanup

Type: `wayfinder:task` (AFK execution)
Branch: `slice/4-architecture`
Blocks: Slice 3
Question: Reduce architectural debt so Slice 3 lands on a cleaner base.

Scope (ranked):
1. Extract inline `mod tests` from god files (chat/mod.rs 6.5k, msg/mod.rs 5.2k, listen.rs 4.2k, account/mod.rs 5.2k).
2. Dedupe `join`/`leave` vs their `_core` functions (chat/mod.rs:424 vs :1810).
3. `anyhow::Result` downgrades exit codes in config/session/client (config.rs:307 missing TELE_API_HASH exits 3 not 1).
4. One shared `CappedMap` instead of 3 copies (serve.rs:100, listen.rs:317, listen.rs:515).
5. Move pagination out of rate_limiter.rs (rate_limiter.rs:7-10).
6. Extract `account/login.rs` (account/mod.rs:500-948) — three concerns in one file.
7. Add `run_fanout` integration test + client.rs auth-flow tests (testing gap).

Done = clippy + `cargo test` green. Pure refactor: no behavior change.
