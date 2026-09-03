# Ticket: Slice 2 — Correctness bugs

Type: `wayfinder:task` (AFK execution)
Branch: `slice/2-correctness`
Blocks: Slice 4
Question: Fix the correctness/concurrency bugs from the perf + interface review agents.

Scope (ranked):
1. Rate limiter resets to full bucket on reconnect (client.rs:51, serve.rs:1304, listen.rs:632).
2. Serve dispatcher blocks on full mutate lane (serve.rs:916-927).
3. EOF drain aborts workers before draining responses (serve.rs:937-957).
4. Fan-out has no timeout; one hung account hangs envelope (executor.rs:117).
5. `stream.resync` doesn't enable catch-up (serve.rs:1392).
6. Album buffer single-slot flushes partial albums (listen.rs:426-470).
7. Blocking sync I/O on async runtime (session.rs:302, serve.rs:584).
8. Doc drift: MCP timeouts exist but doc says none; `may_have_executed`/`StreamDown`/`Interrupted` undocumented (cli-contract.md:711, 422).

Done = clippy + `cargo test` green; cli-contract.md updated for the doc-drift items.
