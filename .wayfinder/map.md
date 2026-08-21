# Wayfinder Map — review-findings

Label: wayfinder:map
Tracker: local markdown (`.wayfinder/tickets/`)
Status: COMPLETE — all tickets closed; integration branch `fix/review-findings` ready for review/merge to main

## Destination

All confirmed findings from the 5-agent codebase review fixed on integration branch `fix/review-findings`: fmt/clippy/tests green, machine contract evolved additively only, docs synced. `main` untouched; user reviews and merges.

## Notes

- Rust CLI, grammers 0.10. Read AGENTS.md before any ticket.
- Conventions: no comments in code; clippy `-D warnings` + fmt + full test suite green per ticket; commit prefixes `fix|refactor|test|docs|chore:`; never push; never touch `main`.
- Branch flow: each ticket branch cut from current head of `fix/review-findings`, merged back after gate. Sequential (single checkout, no worktrees).
- proptest approved as dev-dependency. CI edits approved. New runtime deps NOT approved.
- Subagent execution was down mid-effort; tickets were implemented directly by the orchestrating session instead of per-ticket agents. Branch-per-ticket isolation was preserved.

## Decisions so far

- [T1 output pipe safety](tickets/T1-output-pipe-safety.md) — all stdout/stderr writes fallible; BrokenPipe → silent exit 0 at run_command boundary; comfy-table Dynamic arrangement.
- [T2 auth and error surfaces](tickets/T2-auth-error-surfaces.md) — Windows no-echo 2FA password (warning fallback elsewhere); RPC `code`/`name` additive JSON keys; Dropped translated once to peer-cache hint; `media_kind`/`media_label` additive; `--show-token` gates raw QR URI; phone warning always fires.
- [T3 session security hardening](tickets/T3-session-security.md) — lock files persist as stale markers (no unlink race); SQLite sidecars pre-created restricted + swept post-open; PROTECTED_DACL flag; directory ACLs inheritable; private-key upload blocklist; config tmp-file ordering; .env restrict failure warns; config errors no longer leak full path.
- [T4 fanout cancellation](tickets/T4-fanout-cancellation.md) — AbortOnDrop guard structurally cancels pending account tasks on Ctrl+C; ClientGuard owns runner task (`close()` + abort-on-drop); semaphore AcquireError honestly mapped; dead flood cooldown removed (ADR-008 amended); exit aggregation unified on executor rule (listen mixed usage+other now exits 1 like fanout).
- [T5 CLI contract consistency](tickets/T5-cli-contract.md) — clap `conflicts_with` for json/jsonl and promote/demote, preset/rights (manual guards kept as defense-in-depth); dialog archive/delete validate `--chat`; `--raw` help says "also emit"; raw.rs helpers deduped into helpers.rs; i64→i32 casts → try_from Usage errors; argv hint stops at unknown flags; contract doc gains drafts negated-id convention + listen Ctrl+C/backoff/catch_up semantics.
- [T6 test hardening gaps](tickets/T6-test-gaps.md) — proptest property tests (message_to_json never panics/serializes, .env round-trip + garbage tolerance, classify_target invariants); structural-abort regression test; oversized-input contract tests capped at 2K chars (Windows CreateProcess rejects longer argv — multi-MB coverage needs a stdin/config path the CLI doesn't have).
- [T7 CI and docs sync](tickets/T7-ci-docs.md) — cargo-audit job, MSRV check job pinned 1.89 (floor set by `File::try_lock`), concurrency group; rust-version pinned; AGENTS.md test count corrected; capabilities matrix + ADR-008 amendment reflect AutoSleep-only flood handling.

## Not yet specified

- macOS CI job: release.yml ships mac binaries but ci.yml never tests the target — needs runner/budget decision beyond this effort's approved CI scope.

## Out of scope

- raw.rs registry developer-facing error message ("add an arm in src/commands/raw.rs") — text is mandated verbatim by docs/cli-contract.md; changing it is a contract-major concern, not a fix.
