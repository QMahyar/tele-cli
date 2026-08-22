# BUG-4: Login UX cluster

**Source:** deep-dive auth/session review A1-A4 + C3.

## Question/Problem

Five defects in `tele account login`/`add`:
1. Warning at `account.rs:498-502` recommends `TELE_PHONE` env that nothing reads.
2. Invalid code → immediate exit; wrong 2FA password → full restart incl. new SMS request (FloodWait risk); no retry loops reusing the active token.
3. Aborted login leaves phantom `{name}.session` (+lockfile) created pre-auth at `client.rs:34`; `account list` then reports it present and fanout selects it.
4. QR poll loop (`client.rs:115-159`) has no overall deadline and aborts fatally on any single transient stream error; MigrateTo fallback discards migrated state.
5. `account add` re-run without `--tags` wipes existing tags (`account.rs:179-182`).

## Acceptance criteria

- [ ] Implement `TELE_PHONE` env read as phone default for code login (clap manual read or arg default); warning text now truthful.
- [ ] Invalid-code retry: up to 3 prompts reusing the same login token while valid; clear error when a new request is required.
- [ ] 2FA password retry: up to 3 attempts reusing `pw_token` where grammers permits; no new SMS/code request.
- [ ] On failed/aborted login where account was unauthorized at entry: delete session + lockfile + sidecars created during the attempt (no phantom entries in `account list`).
- [ ] QR login: overall deadline (default 300s, flag-overridable) → clean Usage error; tolerate bounded transient stream errors (retry with backoff, e.g. 3 attempts) before failing.
- [ ] `account add` only overwrites tags when `--tags` was explicitly passed.
- [ ] Unit tests per behavior (offline seams); no network in tests.

## Verification

- [ ] clippy -D warnings / fmt check / full `cargo test`

## Files

- `src/commands/account.rs`, `src/client.rs`, possibly `src/session.rs`, `docs/cli-contract.md` (TELE_PHONE doc), tests

## Constraints

Branch `fix/bug-4-login-ux`. Do not change session backend. No comments. Update `docs/cli-contract.md` additively if env var becomes part of contract.
