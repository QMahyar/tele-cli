# 05 — Human-mode per-account table headers in fanout commands

**What to build:** When a command fans out over multiple accounts (e.g. `--account all`), human-mode output currently prints one table per account with no indication of which account each table belongs to — N successful accounts produce N indistinguishable tables. Fix: when more than one account is selected, prefix each account's table with a header line naming the account (e.g. `== NAME ==`) printed to stdout before that account's table. When exactly one account is selected (the common case), output is byte-for-byte unchanged — no header.

Affected sites (tables printed inside the per-account closure): `dialog list`, `dialog drafts`, `contact list`, `topic list`, `chat participants`, `chat admin-log`, `msg get`, `msg search`. (`account status` already aggregates into one table via the envelope — leave it. `account list`/`raw`/`completions` are not fanout — leave them.)

Implementation shape: one helper in `src/output.rs` (e.g. `print_account_table(account: &str, multi: bool, headers, rows)`) that prints the `== NAME ==` line only when `multi` is true, then the table. Each affected command computes `multi` once before `run_fanout` (selected-names count > 1 — can reuse the selection logic; `select_accounts(flags)` result length) and captures it into the closure. Do not print headers inside table rows or change `--json`/`--jsonl` output in any way.

**Blocked by:** None — can start immediately. (Merges after 02 if both land in a sprint — same files msg.rs/chat.rs.)

**Status:** ready-for-agent

**Verified context (commit a647834):** `src/output.rs:79` `print_table`; per-account print sites: `src/commands/dialog.rs:127,197`, `src/commands/contact.rs:105`, `src/commands/topic.rs:160`, `src/commands/chat.rs:476,837`, `src/commands/msg.rs:875,1057`. `executor::select_accounts` (`src/executor.rs:127-138`) gives the selected names.

**Acceptance criteria:**
- [ ] RED test first: helper prints header iff multi; single-account path unchanged (pure function tests — no network)
- [ ] Two-account dry-run/human run of `dialog list` shows `== 1 ==` / `== 2 ==` headers; single account shows none
- [ ] `--json`/`--jsonl` output untouched by this change
- [ ] All affected command files use the shared helper (no new copy-paste)
- [ ] No comments added; AGENTS.md conventions followed
- [ ] `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check` pass
- [ ] Commit as `fix:` prefix, one logical change, on branch `fix/05-table-headers`