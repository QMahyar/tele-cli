# LIVE-1: serve restart replays entire catch-up history (HIGH)

**Branch:** `feat/live1-serve-state` · **Files:** `src/commands/serve.rs`, `src/commands/listen.rs`, `src/session.rs`, `src/client.rs` · **Deps:** none (solo wave — no other agent touches these files concurrently)

## Goal

Serve must start from NOW and persist update state across restarts; no full history replay on every start, no duplicate deliveries on flaky-proxy reconnects.

## Acceptance

- [ ] Root cause investigated: how grammers 0.10 persists `updates.State` (pts/qts/date/seq) via `SqliteSession` / `Session` trait; where `set_update_state` / `get_update_state` is called by `Client` / `UpdateStream`; why current serve leaves state stale on disk (compare listen.rs handling, GapTracker, `catch_up:true` semantics, proxy-induced reconnects)
- [ ] Fix: state persisted after each processed update (or batched safely) so `catch_up:true` on next start resumes from last persisted point, not from ancient state; reconnects within one run do not re-deliver overlapping windows (dedupe by pts/seq if grammers already does not)
- [ ] New behavior: `tele serve` starts from current state by default (no replay of week-old history); document whether `--catch-up` flag is needed or `catch_up:true` should become `catch_up:false` by default for serve (game scripts want live-only); choose one and make it explicit in --help
- [ ] Flaky-proxy reconnects: bounded retries do not re-emit already-delivered rows; if grammers already dedupes, add an explicit in-process dedupe set keyed by (chat_id, msg_id, pts) as safety net
- [ ] Offline tests: state persistence unit tests with fixture SqliteSession files (no network); catch-up vs live-only mode help text; gates green (fmt, clippy -D warnings, cargo test all suites)

## Boundaries

No other command files; no docs/ edits (manager flips matrix at merge). Touching `Cargo.toml` only if a new dep is truly required (ask first — none expected).
