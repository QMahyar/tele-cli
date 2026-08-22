# W1-4: auth.password-manage + auth.sessions-manage

**Branch:** `feat/w1-4-auth-sessions` · **Files:** `src/commands/account.rs` only · **Deps:** none

## Goal

Cloud password lifecycle and remote-session control under `tele account`.

## Acceptance

- [ ] `tele account sessions [--terminate HASH]`: list authorizations via raw `account.GetAuthorizations` (rows: hash, device, app, ip, country, date_current marker); `--terminate` wraps `account.ResetAuthorization {hash}` — refuses to terminate the current session's own hash (detect via `current:true` field) with honest error; mutating gate: explicit `--account` + `--dry-run` honored
- [ ] `tele account password [--set|--change|--remove]` (+ `--hint H`, `--recovery-email E`): FLAT subcommand with mode flags (house style: profile photo --remove / dialog draft --clear); secrets ALWAYS via no-echo prompt reusing the login-flow Windows pattern — never argv/env/flags; raw `account.GetPassword` → build `InputCheckPasswordSRP`. HONESTY CLAUSE: constructing SRP for UpdatePasswordSettings requires SRP primitives grammers implements internally for check_password; inspect `grammers_client::client` / `grammers_mtproto::srp` for reusable public pieces BEFORE building anything. If no public path exists, implement as far as possible and STOP with a precise report of the blocked piece (do not hand-roll crypto, do not fake success)
- [ ] Password prompts: no-echo input reusing the existing Windows pattern in login flow; never log secrets
- [ ] Offline tests: sessions row shaping fixture, terminate-current refusal, arg validation; gates green

## Boundaries

Only `src/commands/account.rs`.
