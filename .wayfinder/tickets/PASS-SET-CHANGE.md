# PASS-SET-CHANGE: account password --set / --change via local PH2

**Branch:** `feat/pass-set-change` · **Files:** `src/commands/account.rs`, `Cargo.toml`, `docs/capabilities.md`, `docs/security.md` (if password section exists), `CHANGELOG.md` · **Deps:** approved — add `pbkdf2`, `hmac`, `sha2`, `rand` matching versions already in Cargo.lock where possible

## Goal

Unlock `tele account password --set` / `--change` which are blocked upstream (grammers-crypto 0.10 keeps `new_password_hash` private).

## Acceptance

- [ ] Inspect `grammers-crypto 0.10` source (`%USERPROFILE%\.cargo\registry\src\*\grammers-crypto-0.10*\src\two_factor_auth.rs`) for `new_password_hash` / PH2 steps: salt generation (32B secure_random), pbkdf2-hmac-sha512 ×100k, `PasswordKdfAlgo` TL construction, hash wrapping. Replicate VERBATIM — do not invent; cite source lines in report
- [ ] Add deps pinned to versions already in lockfile where possible (check `Cargo.lock` for `pbkdf2`/`hmac`/`sha2`/`rand` before adding; prefer `default-features = false` where applicable)
- [ ] `tele account password --set [--hint H] [--recovery-email E]`: no-echo prompts for new password + confirm; when account already has password → honest error suggesting --change
- [ ] `tele account password --change`: prompts current password (no-echo) + new password/confirm; builds BOTH current SRP proof (via existing `calculate_2fa`) and new PH2 hash; calls `account.UpdatePasswordSettings` with `current` + `new_settings` (new_password_hash + hint/email where given)
- [ ] `hint`/`recovery-email` ride the same UpdatePasswordSettings call (no extra RPC) — when given with --set/--change, include them; with --remove keep current validation (requires --remove alone)
- [ ] Security: passwords never logged, never appear in --json output, never in process title; dry-run still prompts? NO — dry-run must NOT prompt (honest row with would+hint/email, no secret collection)
- [ ] Docs: `docs/capabilities.md` auth.password-manage row → `done` with note "set/change via local PH2 (pbkdf2-hmac-sha512 ×100k, replicated from grammers-crypto 0.10; remove via SRP)"; `docs/security.md` password section if present; `CHANGELOG.md` entry
- [ ] Offline tests: deterministic PH2 fixture (fixed salt/password → known hash vector from grammers-crypto source or self-generated via same crates, byte-exact), set/change validation matrices, dry-run no-prompt, secret-never-in-output assertions; gates green

## Boundaries

No other command files. Live test remains manager/user checklist (throwaway password set then immediate remove).
