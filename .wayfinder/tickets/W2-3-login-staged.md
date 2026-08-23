# W2-3: kernel.login-staged — non-TTY staged login

**Branch:** `feat/w2-3-login-staged` · **Files:** `src/commands/account.rs` · **Deps:** W1-4 + W2-2 merged (both done)

## Goal

Scripts and agents can complete login without a TTY, resumable across process invocations.

## Acceptance

- [ ] `tele account login --stage begin --name A [--phone P]`: sends the code (honors TELE_PHONE / --phone), persists pending-auth state under the app dir (`{app}/pending/{name}.login.json`: phone, api_id hash NOT secrets — phone_hash token from SendCode, code_hash), prints JSON row; refuses if account already authorized
- [ ] `tele account login --stage code --name A --code 12345 [--password]`: resumes from state file, signs in; on SESSION_PASSWORD_NEEDED prompts no-echo for password (interactive) or errors honestly in non-TTY with exit 4; deletes state file on success
- [ ] `tele account login --stage status --name A` / `--stage cancel --name A`: inspect / discard pending state
- [ ] State file: restricted perms (fs_util patterns); never stores code or password; stale-state detection (expired code_hash → honest error suggesting cancel+begin)
- [ ] Existing interactive `tele account login` behavior unchanged (flag-gated)
- [ ] Offline tests first (TDD): stage matrix (begin twice, code without begin, cancel semantics, state-file shape minus secrets, perms assertion, authorized-refusal); gates green (fmt/clippy -D warnings/cargo test)

## Boundaries

Only `src/commands/account.rs`. Live two-account verification stays a manager/user checklist item.
