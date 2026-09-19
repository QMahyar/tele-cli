# Performance baselines and budgets-as-protocol

No numeric SLOs are set here. Per the budgets-as-protocol decision, every
optimisation ships before/after numbers on the named curves below with a
no-regression gate against the recorded baseline; numeric SLOs get proposed
only after the first measured baselines exist.

## Measured offline floor

Measured 2026-09-19 on the maintainer Windows box, debug profile, deps-cached:

- `tele --help`: ~72 ms (process start + clap parse, no config, no network).
- `tele doctor` on an empty isolated `TELE_APP_DIR`: ~85 ms (start + app-dir
  resolve + local checks, no network).
- Together these bound the fixed per-invocation floor: short-command wall
  time above ~0.1 s is handshake + RPC, not startup.
- Debug binary `target/debug/tele.exe`: 44,486,144 bytes (~42.4 MiB).
- Single-crate incremental rebuild (`cargo build`, deps cached): ~45 s.

## Curve protocols + no-regression gates

1. Pagination time-vs-limit (100/1k/10k dialogs, deep history), limiter
   disabled vs enabled. Live spot-check. Gate: no curve point regresses vs
   the recorded baseline at the same limit.
2. Fan-out `--parallel` 1/4/16/32 on dry-run workloads. Offline-capable
   (dry-run returns before connect). Gate: semaphore/limiter overhead within
   noise vs baseline sweep.
3. Cache bulk-insert across batch sizes; FTS5 query latency at 10k/100k rows.
   Design fact: sync writes are already a single transaction per batch
   (`BEGIN IMMEDIATE`/`COMMIT` in `cache_db.rs`), page size 100. Gate: batch
   trial records time-vs-size before any batch-size change lands.
4. 10k-row table vs JSON render with `--fields` cost broken out. Gate:
   render time recorded before any output-path change lands.
5. Per-target binary sizes across the 13-target matrix + single npm bundle.
   Local debug size above is the developer proxy; release sizes are
   CI-measured per `docs/release.md`. Gate: size table before/after any
   feature-trim or codegen proposal.
6. `cargo build --timings` for `codegen-units`/feature proposals. Local
   single-crate rebuild (~45 s, deps cached) is the developer proxy;
   full-fresh timing is CI-measured. Gate: timings before/after.
7. Connect/handshake share of short commands: floor above vs live wall time
   (manual spot check). Gate: any session-reuse proposal argues against this
   floor with measured handshake numbers.
8. Stream backpressure past the 1000-update queue (`update_queue_limit`
   + `catch_up` in `client.rs`): live-flood spot check. Gate: behaviour
   documented before any queue/lane change lands.

## First-cut scope notes

- FTS5: the `messages_au` update trigger closed the open "no update trigger"
  question from the baseline audit. Before: a raw UPDATE bypassed the FTS
  index and left stale docs; after: the trigger deletes + reinserts the
  index row (proven by `update_keeps_fts_index_in_sync`). The hot path is
  untouched by construction — sync writes go through `INSERT OR REPLACE`
  inside the existing single transaction, which never fires the UPDATE
  trigger — so no batch-size regression risk.
- Output streaming vs buffering: buffered envelopes stay. Fan-out must join
  all accounts before printing one envelope (contract), so streaming would
  break the machine shape; accepted as designed pending curve-4 numbers.
- Feature-trim/build: no trim shipped. The `Cargo.toml` features-audit
  comment records that further trimming risks build breaks for unmeasured
  gain; `cargo deny check` is green. Trimming stays out until curve 5/6
  numbers justify it.
