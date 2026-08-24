# ACCT-1: password email confirm + recovery + reset wait (fixes EMAIL_UNCONFIRMED dead end)

**Branch:** `feat/acct1-password-email` · **Files:** `src/commands/account.rs`, `docs/capabilities.md`, `CHANGELOG.md` · **Deps:** none (solo — no other agent touches account.rs concurrently)

## Goal

Make password flows self-service when a recovery email exists: confirm the EMAIL_UNCONFIRMED code, full forgot-password recovery, and the 7-day reset escape hatch.

## Acceptance

- [ ] `tele account password --confirm-email CODE` → raw `account.confirmPasswordEmail#8fdf1920`; `--resend-email` → `account.resendPasswordEmail#7a7f2a15`; `--cancel-email` → `account.cancelPasswordEmail#c1cbd5b6`. All present in tl/api.tl at 227 — verify line numbers before coding
- [ ] Auto-prompt loop: when `--set/--change` with `--recovery-email` hits `EMAIL_UNCONFIRMED`, prompt `Enter email code:` (max 3 retries matching MAX_CODE_ATTEMPTS pattern) and call confirmPasswordEmail inline; surface `email_unconfirmed_pattern` from GetPassword so users know which inbox
- [ ] `tele account password status`: has_password / has_recovery / hint / email_unconfirmed_pattern / pending_reset_date via account.getPassword
- [ ] Recovery flow: `tele account password --recover` — unauthenticated chain: auth.requestPasswordRecovery#d897bc66 → masked email_pattern shown → user enters code → auth.checkRecoveryPassword#d36bf79 optional pre-check → auth.recoverPassword#37096c70 with optional PasswordInputSettings (reuse PH2 from set path; empty = just remove). Works WITHOUT knowing old password. NOTE: this is an UNAUTHENTICATED flow — it runs BEFORE login, so wire into the pre-auth section of account.rs (like login), not the authenticated password_flow
- [ ] Reset wait: `tele account password --reset-start` → account.resetPassword#9308ce1b, print ResetPasswordResult variant (Ok / RequestedWait until_date / FailedWait retry_date); `tele account password --decline-reset` → account.declinePasswordReset#4c9409f6 with current-password SRP proof
- [ ] Mode-flag matrix extended: --recover/--reset-start/--decline-reset mutually exclusive with --set/--change/--remove/--confirm-email etc.; dry-run never prompts and prints honest would-rows; secrets never logged/in output
- [ ] Docs: capabilities.md auth.password-manage row updated (confirm-email/recover/reset shipped); CHANGELOG entry
- [ ] Offline tests: mode matrix, code-retry loop with mockable input seam, recovery chain arg validation, reset result shaping fixtures, secret-absence assertions. TDD first. Gates green.

## Boundaries

Only src/commands/account.rs + the two docs files. Live verification stays manager checklist.
