# Wayfinder map: tele-cli optimization pass (10-agent review)

Label: `wayfinder:map` (local-markdown tracker; no remote tracker configured)

## Destination

Execute the four optimization slices from the 10-agent review, in order:
1 → 2 → 4 → 3, each on its own git branch off `main`, merged back after
clippy + `cargo test` pass. "Do the work" mode — this effort carries
execution in its Notes (wayfinder override), so tickets are task tickets,
not decision tickets.

## Notes

- Repo: E:\Code\Tele-Cli. Rules: AGENTS.md (no comments in code, clippy -D warnings,
  cargo test ~1475 offline, commit prefix `feat|fix|refactor|test|docs|chore:`).
- Branch naming: `slice/1-security`, `slice/2-correctness`, `slice/4-architecture`,
  `slice/3-features`. No worktrees — one checkout, sequential slices.
- Delegate to subagents in parallel where file sets are disjoint; subagents edit
  but do NOT commit; the driver verifies + commits.
- Every slice must leave `cargo test` + `cargo clippy --all-targets -- -D warnings` green.

## Decisions so far

- [Slice 1 — Security fixes](tickets/slice-1-security.md): merged to main as `merge: slice 1 — security fixes` (raw confirm gate, session/file perms, download sweep, sensitive suffixes).
- [Slice 2 — Correctness bugs](tickets/slice-2-correctness.md): merged to main as `merge: slice 2 — correctness fixes` (limiter survives reconnects, lane backpressure, EOF drain, resync catch-up, album multi-buffer, account timeout, JsonlWriter) + cli-contract doc drift fixed.
- [Slice 4 — Architecture cleanup](tickets/slice-4-architecture.md): merged to main as `merge: slice 4 — architecture cleanup` (god-file test extraction, CappedMap, pagination.rs, exit-code bridge, account/login.rs, join/leave dedupe, run_fanout + client tests).
- Slice 3 pause point (2026-09-03): branch `slice/3-features` holds 5 features, all green (1546 tests: 1431+92+23). Features 1–4 verified by agents; feature 5 (search filters) landed pre-cancellation but was never agent-verified — needs a verification pass on resume.

## Not yet specified

- Slice 3 completion: verify search filters end-to-end (live dry-run + serve/MCP params), update `docs/capabilities.md` + `docs/cli-contract.md` for all 5 features (agents were told to skip docs), then merge `slice/3-features` to main.

## Out of scope

- Anything from the review outside the 4 chosen slices (interface doc drift is
  folded into slice 2 verification; ecosystem/crates.io work is not in this pass).
