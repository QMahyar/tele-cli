# 06 — account list honors --account/--tag

**What to build:** `tele account list` currently ignores `--account` and `--tag` entirely — `--account 1 account list` lists every session. Every other command honors the selection. Make `account list` respect it:

- With explicit `--account`/`--tag` (or `all`): list only the selected accounts' sessions; unknown account name → Usage error exit 1 (same as other commands); unknown tag → Usage error.
- With no selection: keep today's behavior (list all sessions — `account list` is a documented exception to the empty-selection error).
- Human table and `--json`/`--jsonl` envelope both reflect the filtered set. The `accounts` top-level array in the list envelope must match the filtered set too.
- `account add` unchanged (also a documented exception).

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

**Verified context (commit a647834):** `src/commands/account.rs` `list` (71-107) reads `session::list_session_names()` directly and never consults selection; `executor::select_accounts_from_cfg`/`select_from` (`src/executor.rs:132-174`) implements selection + unknown-account/tag errors. Existing executor tests cover `select_from` semantics.

**Acceptance criteria:**
- [ ] RED test first: a pure selection test for the new behavior (e.g. list-filter function or selection-into-list) fails before implementation
- [ ] `--account 1 account list --json` lists only account 1 (results + accounts array)
- [ ] `--account bogus account list` exits 1 UsageError; `--tag nosuch account list` exits 1
- [ ] `account list` (no flags) still lists all sessions
- [ ] No comments added; AGENTS.md conventions followed
- [ ] `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check` pass
- [ ] Commit as `fix:` prefix, one logical change, on branch `fix/06-account-list-filter`