# ACCT-2: account lifecycle quick wins — TTL, delete guard, web sessions, phone change, login resend/cancel

**Branch:** `feat/acct2-lifecycle` · **Files:** `src/commands/account.rs`, `src/commands/raw.rs` (registry additions only if needed), `docs/capabilities.md`, `CHANGELOG.md` · **Deps:** waits for ACCT-1 merge (same file)

## Goal

Five small account-lifecycle wins in one slice.

## Acceptance

- [ ] TTL: `tele account ttl get` (account.getAccountTTL#8fc711d) / `tele account ttl set --days 180` (account.setAccountTTL#2442485e)
- [ ] Delete: `tele account delete --reason "..." --yes` → account.deleteAccount#a2c0cf74; requires --yes AND explicit --account; prompts current password SRP if getPassword.has_password (reuse input_check_password_srp); without --yes prints what would happen and exits 1
- [ ] Web sessions: `tele account sessions --web` lists getWebAuthorizations#182e6d6f rows; `--terminate-web HASH` → resetWebAuthorization#2d01b9ef; `--terminate-all-web` → resetWebAuthorizations#682d2594; per-device flag toggle via changeAuthorizationSettings#2338 (--hash H --disable-encrypted true / --disable-call-requests true)
- [ ] Phone change: `tele account phone change --phone +XXX [--allow-flashcall]` sends sendChangePhoneCode#82574ae5 returning phone_code_hash; `tele account phone confirm --code NNN --phone-hash H` → changePhone#70c32edb; staged two-step like login
- [ ] Login codes: staged login gains `--stage resend` (auth.resendCode#cae47523 using pending phone_code_hash) and server-side cancel (`auth.cancelCode#1f040578`) distinct from existing local --stage cancel; document difference in help
- [ ] All mutators: explicit --account + --dry-run gates; delete/terminate need extra confirmation flags per above; secrets/phone numbers never logged raw
- [ ] Docs: capabilities.md rows for ttl/delete/web-sessions/phone-change/login-resend added or extended under Auth/Account; CHANGELOG entry
- [ ] Offline tests: validation matrices per command, destructive-guard assertions (--yes required, dry-run no-op), row shaping fixtures, staged resend/cancel state handling. TDD first. Gates green.

## Boundaries

Only src/commands/account.rs (+ raw.rs registry lines if an arm helps), docs. No other command files.
