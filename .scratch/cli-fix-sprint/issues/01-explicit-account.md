# 01 — Sensitive commands require explicit account selection

**What to build:** `tele listen` and `tele takeout start|export|finish` must refuse to run unless the user explicitly named accounts via `--account` or `--tag`. Today a bare `tele listen` silently selects every configured account and streams live message content to stdout forever; a bare `tele takeout start` mutates server-side takeout state on all accounts. Both are safety hazards: the user must state their intent.

- `tele listen` with no `--account`/`--tag` → exit 1, Usage error: `listen requires --account <name> or --tag <tag>` (or similar), BEFORE any connection. `tele listen --account 1` still works. `tele listen --account all` still works (explicit `all` is intent).
- Same rule for `takeout start`, `takeout export`, `takeout finish`: no explicit selection → exit 1, Usage error, before connect. `--account all` remains valid.
- `--dry-run` does NOT exempt the requirement: dry-run also needs an explicit selection.
- Only these commands change; `account list`, `account status`, and all other commands keep today's default-to-all behavior.
- Error shape: `TeleError::Usage`, so `--json` mode emits the standard UsageError envelope (exit 1).

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

**Verified context (commit a647834):** `src/commands/listen.rs:47-52` calls `select_accounts(flags)` and only errors when the result is empty; `executor::select_from` (`src/executor.rs:170-172`) defaults to all sessions when no flags are given. `src/commands/takeout.rs` `start`/`export`/`finish` (lines 67, 188, 546) all go through `run_fanout` with the same default-to-all behavior.

**Acceptance criteria:**
- [ ] RED test first: unit tests proving a guard rejects empty account+tag selection (exit code 1, Usage error) while accepting explicit names and `all`
- [ ] `tele listen` (no flags) errors with exit 1 and never connects
- [ ] `tele takeout start|export|finish` (no flags) error with exit 1 and never connect
- [ ] `--account 1` and `--account all` still accepted for both commands
- [ ] Guard shared (one helper, e.g. in `src/executor.rs`) used by listen + all three takeout subcommands
- [ ] `docs/capabilities.md` updated if the `listen.*`/`takeout` rows describe selection behavior
- [ ] `cargo test` (all pass), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check`
- [ ] Commit as `fix:` prefix, one logical change, on branch `fix/01-explicit-account`