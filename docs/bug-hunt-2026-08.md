# Tele-Cli Bug Hunt Report — 2026-08-15

**15 parallel deep-dive bug-hunt agents (all using the rust-engineering skill) audited 15 disjoint slices of the codebase. Strictly read-only; every finding was verified by tracing the actual code path, including the vendored grammers-client 0.10.0 / grammers-session 0.10.0 / grammers-tl-types 0.10.0 / libsql sources and the generated TL code under `target/debug/build/`. No builds or tests were run; nothing was modified.**

Slices: 1) entry/errors/dispatch · 2) client/session/fs · 3) config/credentials · 4) entities/peer cache · 5) executor/fanout · 6) output/json/logging/contract · 7) account lifecycle · 8) msg core · 9) msg advanced · 10) chat core · 11) chat admin · 12) listen+takeout · 13) raw+completions · 14) dialog/topic/helpers · 15) contact/profile/privacy.

---

## Summary

| Slice | Findings | Critical | High | Medium | Low |
|-------|----------|----------|------|--------|-----|
| 1. Entry / errors / dispatch | 12 | 0 | 2 | 6 | 4 |
| 2. Client / session / fs | 8 | 0 | 1 | 3 | 4 |
| 3. Config / credentials | 13 | 0 | 3 | 4 | 6 |
| 4. Entities / peer cache | 8 | 0 | 1 | 3 | 4 |
| 5. Executor / fanout | 7 | 0 | 2 | 3 | 2 |
| 6. Output / JSON / logging | 8 | 0 | 1 | 3 | 4 |
| 7. Account lifecycle | 11 | 0 | 0 | 5 | 6 |
| 8. Msg core | 14 | 0 | 4 | 3 | 7 |
| 9. Msg advanced | 8 | 0 | 0 | 2 | 6 |
| 10. Chat core | 12 | 1 | 1 | 3 | 7 |
| 11. Chat admin | 8 | 0 | 0 | 4 | 4 |
| 12. Listen + takeout | 13 | 0 | 1 | 5 | 7 |
| 13. Raw + completions | 7 | 0 | 1 | 2 | 4 |
| 14. Dialog / topic / helpers | 9 | 0 | 2 | 3 | 4 |
| 15. Contact / profile / privacy | 11 | 0 | 2 | 3 | 6 |
| **TOTAL (raw)** | **149** | **1** | **20** | **52** | **75** |

**Top risk clusters** (each reported independently by ≥2 slices, listed once here, details per-slice below):

1. **Numeric peer resolution** (slices 4, 8, 10, 14): wrong peer-kind probes for negative/positive bare ids (up to sending to the wrong chat), uncached `-100…` channel ids rejected with `INVALID_PEER_ID` (range gap in grammers-session `PeerId`), uncached basic groups resolved as channels → `CHANNEL_INVALID`.
2. **`+phone` targets permanently import the number into the contact list** as a side effect of any command, including read-only ones (slices 4, 8, 15).
3. **Config/credential failures exit 3 instead of 1** — the `TeleError::Config → EXIT_USAGE` arm is dead code; broken `config.toml` converts via `From<anyhow>` to `Other`; contract tests lock the wrong shape (slices 1, 3, 5, 6).
4. **Human-mode fanout output** is printed inside per-account parallel closures: unlabeled, nondeterministic, line-interleaved (slices 1, 5, 10).
5. **`print_json` panics on broken pipe** (`EPIPE`, exit 101) — `print_json_result` already does it right (slices 1, 5, 6).
6. **`account add` accepts invalid names** (`""`, `"a b"`, `".."`, `"all"`) that `remove` can then never delete; `all` poisons the reserved fanout keyword (slices 1, 3, 5, 7).
7. **Non-atomic `write_config`** — crash mid-write silently degrades config to defaults (slices 3, 7).
8. **"Exclusive lock; refuse start if locked" documented but not implemented** — two processes can open one session file (slices 2, 7).
9. **`--dry-run` JSON missing `data.would`** promised by `docs/cli-contract.md:75`; contract test asserts the wrong shape (slices 6, 8).
10. **Windows-only guard bypasses**: trailing-dot/space basenames defeat the sensitive-file upload guard; case-sensitive prefix compare + non-canonical fallback defeat the download-dir guard (slices 2, 9).

---

## Slice 1 — CLI entry, errors, dispatch (`src/main.rs`, `src/error.rs`, `src/commands/mod.rs`)

### 1.1 HIGH — Runtime/Telegram-side conditions misclassified as `Usage` → wrong exit code
- **Locations:** `src/commands/msg.rs:922` (`message {id} not found`), `src/commands/msg.rs:933` (`message has no media`), `src/commands/chat.rs:237-239` (`cannot leave a private chat`), `src/commands/chat.rs:311` (`cannot invite…`)
- **What:** These errors are raised *inside* the per-account fanout closures, so they become `failed_outcome` with `error.type == "UsageError"` and `exit_code = 1`. `envelope_exit_code` (executor.rs:196-203) then maps them to `EXIT_USAGE`.
- **Why wrong:** cli-contract.md:28 defines exit 1 as "Usage / validation"; cli-contract.md:34 explicitly says "Do not overload 1 for Telegram errors". A missing message or a private-chat target is a runtime condition → must be exit 3. Single-account `tele msg download --chat x --id 9999` exits 1, poisoning machine-consumer logic that treats 1 as "fix your invocation".
- **Fix:** return `TeleError::Invocation`/`Other` for these (exit 3); keep `Usage` only for arg validation done before the closure.

### 1.2 HIGH — Same error yields envelope vs. empty stdout in `--json` depending on command, plus error-type drift
- **Locations:** `src/commands/account.rs:93` (status), `:199` (login), `:308` (logout) vs `src/commands/credentials.rs:3-5`; `src/main.rs:183-189`
- **What:** Missing `TELE_API_ID/HASH` is handled two ways: `account status/login/logout` call `config::credentials()?` directly (anyhow → `TeleError::Other`), failing *before* any envelope exists. `msg send` calls `creds_api_id()` → `TeleError::Config`, failing per-account *inside* the envelope.
- **Impact:** `tele account login --json` with no creds → exit 3, empty stdout; `tele msg send --json` with no creds → exit 3, stdout **has** an envelope with `error.type == "ConfigError"`. Same condition, two machine-API shapes and two error types.
- **Fix:** route account status/login/logout through `credentials::creds()` or produce a failed envelope on the direct path.

### 1.3 MEDIUM — `ConfigError` exit-code taxonomy is internally contradictory
- **Locations:** `src/error.rs:20-21` (`Config → EXIT_USAGE`) vs `src/executor.rs:179-205` and `tests/contract.rs:373-405`
- **What:** `TeleError::Config.exit_code()` returns 1, but the envelope path ignores per-account exit codes except auth/usage and checks `error.type == "UsageError"`, so a `ConfigError` outcome exits **3** (locked in by `error_envelope_shape` at contract.rs:390). Meanwhile `load_config` (config.rs:218) returns `anyhow::Result`, so a broken `config.toml` becomes `TeleError::Other` → exit 3 even on the direct path (`executor.rs:30,125`, `account.rs:55,146,337`). `TeleError::Config` is nearly dead code, and direct vs envelope paths disagree (1 vs 3).
- **Fix:** make `load_config` return `TeleResult<AppConfig>` producing `TeleError::Config`; give `envelope_exit_code` a Config→1 arm; update the contract test.

### 1.4 MEDIUM — `account add` stores unvalidated names that `account remove` can then never delete
- **Locations:** `src/commands/account.rs:121-151` (add, no `validate_name`) vs `src/commands/account.rs:336` + `src/session.rs:42-50` (remove validates)
- **What:** `tele account add --name "a b"` writes the invalid key to `config.toml` without validation. `login --name "a b"` later fails with a confusing exit-3 `invalid account name "a b"` (session.rs:17-19). `remove --name "a b"` also fails validation *before* touching the config entry — the corrupt entry is permanently stuck (manual config edit required).
- **Fix:** validate the name in `add` (reject before write) and/or have `remove` tolerate invalid names by removing the config entry unconditionally.

### 1.5 MEDIUM — `--quiet`/`--verbose` silently disabled when `TELE_LOG` is set
- **Location:** `src/logging.rs:41-44`
- **What:** `set_flags` early-returns whenever `TELE_LOG` exists, so with `TELE_LOG=debug` set, `-q` no longer suppresses `log_line` output and `-v` does nothing. Help text (main.rs:53-62) promises suppression/verbosity; the env var silently wins. Scripts exporting `TELE_LOG` and relying on `-q` get unexpected stderr noise.
- **Fix:** combine rather than early-return — apply flags as a floor/ceiling on top of the env-derived level.

### 1.6 MEDIUM — Human-mode fan-out output: unlabeled tables, nondeterministic order, interleaving
- **Locations:** `src/commands/msg.rs:705-717`, `:872-885`, `src/commands/chat.rs:369,560`, `src/commands/dialog.rs:125,189`, `src/commands/topic.rs:145`, `src/commands/contact.rs:104`, `src/commands/profile.rs:116-118`
- **What:** every fanout command prints its human table *inside* the per-account closure. `tele msg get --account all` (parallel) prints N unlabeled tables in task-completion order — nondeterministic and different from the account-sorted order of the `--json` envelope (executor.rs:116). `profile get` (profile.rs:117) prints multi-line `k: v` blocks that interleave line-by-line between accounts. `account status` is the only command that collects-then-prints with an account column (account.rs:113-118).
- **Fix:** collect rows per account, print after `collect_outcomes`, prefixing with the account name (or adding an account column).

### 1.7 MEDIUM — `print_json` panics on broken pipe
- **Location:** `src/output.rs:48-50`
- **What:** `println!` panics when stdout is a closed pipe: `tele account list --json | head -1` → `panicked at 'failed printing to stdout: Broken pipe'` + exit 101. `print_json_result` (output.rs:52-60) already does it correctly (used by listen). Known as REL-09 in review-2026-08.md; still present.
- **Fix:** route `print_json` through the `print_json_result` writer pattern (ignore `EPIPE` or exit cleanly).

### 1.8 MEDIUM — Login code errors: wrong message and wrong exit-code family
- **Locations:** `src/commands/account.rs:270-275`
- **What:** `SignInError::InvalidCode` → `Usage("invalid code")` (exit 1) — a server-side auth condition exiting as CLI usage. Worse, the `SignInError::InvalidPassword` arm (account.rs:273-275) reports `"invalid code"` — if ever reached (e.g. `PASSWORD_HASH_INVALID` during `sign_in`), the user is told their *code* was wrong when their 2FA *password* was. (The `check_password` path at account.rs:259-260 correctly reports "invalid 2FA password" as Auth/exit 4.)
- **Fix:** classify as `TeleError::Auth` or `Invocation` with accurate messages; fix the `InvalidPassword` arm's message.

### 1.9 LOW — Help-text and spec drift
- **Locations:** `src/main.rs:77` vs `src/commands/chat.rs:21`; `docs/spec.md:71`
- `tele chat --help` advertises `adminlog`; the actual subcommand is `admin-log` (clap kebab-cases `AdminLog`). `docs/spec.md:71` documents `msg ... pin|unpin` as separate subcommands; only `tele msg pin --unpin` exists.

### 1.10 LOW — `completions` ignores `--json`/`--jsonl`
- **Location:** `src/commands/completions.rs:18-54`
- With `--json`, the shell script goes to stdout and no envelope is emitted — the only command besides `listen` violating "Stdout is one JSON object"; `listen` has a contract carve-out, `completions` does not.

### 1.11 LOW — Listen reconnect opens the session file while the old guard still holds it
- **Location:** `src/commands/listen.rs:91-92`
- On reconnect, `ClientGuard::connect` opens a second `SqliteSession` on the same `.session` file while the previous guard is still alive until the binding is replaced — a brief violation of the AGENTS.md "Never two clients on one file" invariant. Low practical risk.

### 1.12 LOW — `--limit 0` passes validation
- **Location:** `src/commands/mod.rs:18-26` (`validate_limit` is max-only)
- `tele msg get --limit 0` (and `search`, `dialog list`, `topic list`) yields a silent empty result instead of a usage error, unlike `takeout --message-limit 0` which is rejected (takeout.rs:93-95).

### Verified clean (slice 1)
Dispatch completeness (all 12 root variants, every subcommand enum covered); clap error handling (parse → exit 1 via `use_stderr()`, help/version → 0); SIGINT → 130 via `tokio::select!`; `--json`+`--jsonl` mutual exclusion; exit taxonomy 0/1/2/3/4/130 consistent; 401 → Auth/exit 4; flood → `error.seconds`; AutoSleep threshold honored; `--parallel` clamped 1–3; phone-before-numeric in `classify_target`; `me` → `get_me()`; no secrets in any error path.

---

## Slice 2 — Client connection, session storage, filesystem permissions (`src/client.rs`, `src/session.rs`, `src/fs_util.rs`)

### 2.1 HIGH — Windows trailing-dot/space bypass defeats the upload credential guard
- **Location:** `src/commands/msg.rs:258-274`
- **What:** the basename check runs on the **raw user string**, not the OS-resolved name (`msg.rs:266`, `lower.ends_with(".session")`). `--file "C:\backup\1.session."` → basename `"1.session."` → `ends_with(".session")` is **false**. On Windows, Win32 path normalization strips trailing dots/spaces, so `upload_file` (msg.rs:349) opens the real `1.session` (auth-key DB). Same for `"1.session "`, `".env."`, `"1.session-journal."`, mixed case. The app-dir canonical check (msg.rs:261) still catches files *inside* app_dir, so the bypass matters for session copies outside it (backup/restore) — exactly the case security.md:44 says the basename check protects. Tests (msg.rs:1003-1017, 1089-1103, 1409-1429) don't cover trailing-dot/space variants. Unix unaffected (trailing dots are literal filename chars).
- **Impact:** a copied session uploaded to Telegram = full account takeover.
- **Fix:** normalize the raw basename (`trim_end_matches(['.', ' '])`) before the sensitive check, and/or check the canonicalized basename.

### 2.2 MEDIUM — `tele listen` can silently stall forever (no error, no reconnect)
- **Locations:** `src/commands/listen.rs:119-169` × grammers `updates.rs:108-134, 221-244`
- **What:** if the initial `GetState` fails, grammers swallows it (`Err(_err) => {}`, updates.rs:129-133) and never retries. With `catch_up=false` the message box is pristine (no Common entry). A later connection drop → runner sends `ConnectionClosed` → `try_begin_get_diff(Common)` fails (no entry, mod.rs:413) → `Err(Gap)` swallowed (updates.rs:244) → loop blocks in `recv()` forever; `get_difference()` is `None`; the stream **never returns Err**. The reconnect branch (listen.rs:132-166) is unreachable in this scenario; the 3600s poll timeout (listen.rs:168) re-polls and stays blocked. With default `--timeout 0` the task emits nothing forever.
- **Trigger:** session revoked while idle, or a network failure on the very first GetState.
- **Fix:** set `catch_up` so a Common entry exists (gap recovery then works), surface the swallowed GetState error, or add a progress health-check.

### 2.3 MEDIUM — Documented "exclusive lock; refuse start if locked" is not implemented
- **Locations:** `docs/security.md:19` vs `src/session.rs:52-61`
- **What:** `open_session` uses `SqliteSession::open` with libsql default `OpenFlags = READ_WRITE|CREATE` (no `EXCLUSIVE`) and **no busy_timeout** (verified in libsql-0.9.30 and libsqlite3-sys sqlite3.c:38525/39079). Two `tele` processes on one account (e.g. `listen` + `send`, or two `listen`) both open the same SQLite file → concurrent writes fail with immediate `SQLITE_BUSY`, and both MTProto connections share one auth key (the `AUTH_KEY_DUPLICATED` risk the doc claims is mitigated). AGENTS.md:64 "Never two clients on one file" is convention only, not enforced.
- **Fix:** implement the lock (flock/lockfile in `open_session`) or set a busy_timeout and re-document.

### 2.4 MEDIUM — `tele listen` drops rows under backpressure (JSONL contract)
- **Locations:** `src/commands/listen.rs:113-118` × grammers `updates.rs:260-279`
- **What:** `UpdatesConfiguration::default()` → `update_queue_limit: Some(100)` (client.rs:97-104). The listen consumer writes each row with a blocking stdout lock+flush (output.rs:52-60). If stdout is a slow pipe/file, the worker blocks, the internal buffer exceeds 100, and grammers **silently truncates** excess updates (only a stderr warn). Missing rows in the machine-facing JSONL stream.
- **Fix:** raise/unset `update_queue_limit` for listen and/or write rows via an async/spawn_blocking writer.

### 2.5 LOW — Permission hardening is a no-op on Windows
- **Location:** `src/fs_util.rs:15-18`
- `restrict_file_private` = `Ok(())` on non-Unix. The `0o600`/`0o700` guarantees (security.md:18, session.rs:59) only hold on Unix; on Windows protection relies entirely on the parent-dir ACL (fine under `%APPDATA%`, not if `TELE_APP_DIR` points elsewhere).

### 2.6 LOW — Fail-open `.env` chmod
- **Location:** `src/config.rs:177-179`
- `let _ = crate::fs_util::restrict_file_private(&path);` swallows the chmod error silently; a `0644` `.env` stays `0644` with no warning (the tightening test only covers success).

### 2.7 LOW — `account remove` deletes the session before the config write; no Windows retry
- **Locations:** `src/commands/account.rs:336-343`, `src/session.rs:42-50`
- If `write_config` fails after `remove_session`, the account lingers in config without a session (recovers as a confusing "not logged in" on next connect). `remove_session` has no retry (unlike `logout`'s `remove_session_file_retry`), so a concurrently-held file on Windows fails immediately.

### 2.8 LOW — `takeout export` "resume" overpromise
- **Location:** `src/commands/takeout.rs:124-130`
- The error message implies resumability, but `run_export` re-creates/truncates all three files and re-downloads from scratch; only the server-side takeout session is kept alive.

### Verified clean (slice 2)
`ClientGuard::drop` → `client.disconnect()` is non-blocking (`Request::Quit` via mpsc) — no hang, correct RAII (client.rs:56-60). `takeout.rs:79` irrefutable let is safe (`account.takeout#4dba4501` single constructor). `SenderPool::new(session, 12345)` at serialize.rs:102/chat.rs:940 is test-only (MemorySession, offline). Phone-before-numeric parse order correct. SQLite journal files copy the DB mode — no journal exposure once the DB is 0600. Listen reconnect does fire on in-flight invoke failures; the stall (2.2) is limited to the GetState-failure edge case. No secrets logged.

---

## Slice 3 — Configuration, env, proxy, credentials (`src/config.rs`, `src/commands/credentials.rs`)

### 3.1 HIGH — Empty-string env vars silently redirect the app dir into the CWD
- **Location:** `src/config.rs:19-38` (`app_data_dir_from_env`); downstream `config.rs:178` (`.env`), `config.rs:221` (`config.toml`), `src/session.rs:5-6` (`sessions/`)
- **What:** `std::env::var` returns `Ok("")` for set-but-empty vars. `TELE_APP_DIR=""` → `PathBuf::from("")` → `app_data_dir().join(".env")` = the **relative** path `.env`; sessions land in `./sessions/`. Same for `APPDATA=""`/`LOCALAPPDATA=""`/`USERPROFILE=""` on Windows, `XDG_CONFIG_HOME=""`/`HOME=""` on Unix (→ `./telecli`, `./.config/telecli`). Empty `HOME`/`TELE_APP_DIR` is common in CI/service-unit env files. Worse: `credentials()` chmods a `.env` **in the CWD** (config.rs:179), and in `write_config` `path.starts_with(&app_dir)` is true for an empty base (config.rs:272) → `create_dir_private("")` → confusing `NotFound`/`InvalidInput` on `account add`. Tests cover only `VarError::NotPresent` (config.rs:467-472), never set-but-empty.
- **Impact:** session/config/.env files written to and read from the CWD — security-boundary violation, data in the wrong place, baffling failures.
- **Fix:** treat empty/whitespace-only env values as unset in `app_data_dir_from_env`; add a test for `Ok("")`.

### 3.2 HIGH — `.env` with a UTF-8 BOM (U+FEFF) breaks credential loading silently
- **Location:** `src/config.rs:90-110` (`load_env`)
- **What:** `read_to_string` keeps a leading `\u{feff}`; `line.trim()` does **not** remove it (U+FEFF is not Unicode White_Space), so the first key becomes `"\u{feff}TELE_API_ID"` → `parse_api_id` (config.rs:206-208) reports `TELE_API_ID must be set (see .env.example)` — even though the variable is plainly visible. Windows Notepad writes UTF-8-with-BOM by default, and `.env` is the one file the docs tell Windows users to create by hand. The config.toml path is safe (`toml_edit` strips BOM, verified toml_edit-0.22.27 parser), but the hand-rolled `.env` parser has no counterpart.
- **Fix:** strip a leading `\u{feff}` from the first line before parsing.

### 3.3 HIGH — `write_config` is non-atomic; a truncated config silently degrades to defaults
- **Location:** `src/config.rs:278-281` (`std::fs::write` truncate-then-write), `src/config.rs:235-244` (`read_config`)
- **What:** a crash/power loss mid-write leaves an empty/partial `config.toml`. `toml::from_str("")` succeeds and yields `AppConfig::default()` (proven by the existing test at config.rs:433-437) — the file is **not** rejected as malformed. Every configured account silently vanishes (`account list` shows none) with no error at all. Partial garbage (e.g. half a `[accounts]` header) does error, but the empty case is silent.
- **Fix:** write to a temp file + atomic rename; optionally treat empty-but-existing config as an error.

### 3.4 MEDIUM — Missing credentials exit 1 on some commands, 3 on others (same failure)
- **Locations:** fan-out path `src/executor.rs:70-78` + `failed_outcome` + `envelope_exit_code` (`executor.rs:179-205`); direct path `src/commands/account.rs:93,199,308`
- **What:** `msg/chat/dialog/contact/profile/privacy/topic/raw/takeout` call `creds_api_id()?` **inside** the per-account closure → `TeleError::Config` becomes a per-account outcome → `envelope_exit_code` only recognizes `type == "UsageError"` → **exit 3**. `account status/login/logout` resolve creds **before** fan-out → exit **1**. The inconsistency is locked in by test `config_failure_exits_all_failed_not_usage` (executor.rs:465-474). Also `read_config` parse failures convert via `From<anyhow::Error>` to `TeleError::Other` → exit 3, while `creds()` maps the same class to `TeleError::Config` → exit 1.
- **Fix:** classify `TeleError::Config` (and config parse errors) as usage (exit 1) consistently, including in `envelope_exit_code`.

### 3.5 MEDIUM — `TELE_API_HASH` is never validated for emptiness
- **Location:** `src/config.rs:189-191`
- **What:** `TELE_API_HASH=` (or an env var set to empty) passes `.ok_or_else(...)` and yields an empty hash. `api_id` gets a strict check (`parse_api_id`, config.rs:205-216), the hash gets none. Empty/malformed hashes flow into `request_login_code`/`ExportLoginToken` (account.rs:232, client.rs:90) and fail later with cryptic grammers/RPC errors. Env values also aren't trimmed (a trailing space survives).
- **Fix:** reject empty/whitespace-only hash after trimming.

### 3.6 MEDIUM — `account add` accepts invalid account names
- **Location:** `src/commands/account.rs:146-151` (add); compare `session.rs:42-43` (remove validates)
- **What:** `add` never calls `session::validate_name`. `--name ""`, `--name "../x"`, and `--name all` are all written to config. `--name all` poisons the special `all` keyword in `select_accounts` (executor.rs:139-142): the account can never be selected by name and `--account all` expands to all sessions instead. Invalid names only fail later at connect time (with exit 3, compounding 3.4).
- **Fix:** validate in `add` (and `login`/`logout`) up front with a Usage error.

### 3.7 MEDIUM — `write_config` destroys comments, formatting, and unknown keys
- **Location:** `src/config.rs:278-279` (`toml::to_string_pretty` full rewrite); callers `account.rs:151,343`
- **What:** every `account add`/`remove` re-serializes the whole struct: user comments vanish, `BTreeMap` reorders accounts alphabetically, and unknown tables (e.g. the superseded spec's `[app]` section) are silently dropped. Known-field round-trips are safe (verified `toml_write` quotes dotted keys).
- **Fix:** mutate via `toml_edit` (document-preserving) instead of full re-serialization.

### 3.8 LOW — Proxy host is concatenated into a URL without validation
- **Location:** `src/config.rs:267`
- **What:** `format!("socks5://{}:{}", p.host, p.port)` with no host-shape check. Verified against vendored `url-2.5.8`: `host = "::1"` (IPv6 literal) → `socks5://::1:9050` → `ParseError::EmptyHost`; `host = "127.0.0.1:9050"` (user error) → invalid port. Both surface as cryptic connect-time errors from `grammers-mtsender-0.10.0/src/net/tcp.rs:55-65`. Also no username/password fields even though grammers supports them (tcp.rs:93-100).
- **Fix:** validate/normalize host (reject `:`/`@`/`/`, accept `[::1]`), or parse-and-reformat the URL; document bracket syntax.

### 3.9 LOW — FileStamp cache can go stale on coarse-mtime filesystems
- **Location:** `src/config.rs:142-164, 177-232`
- **What:** cache keys are `(path, exists, len, modified)`. On FAT/exFAT (2s mtime granularity), editing `.env`/`config.toml` within the same second with the same byte length yields a stale stamp — long-lived processes (`tele listen`) keep old creds/config. Short-lived CLI invocations unaffected.
- **Fix:** include a content hash (or short read) in the stamp.

### 3.10 LOW — `account remove` deletes the session before the config write
- **Location:** `src/commands/account.rs:336-343`
- **What:** `session::remove_session` runs first; if `write_config` then fails (read-only FS), the session file is gone while config still lists the account — `account list`/`status` report an account with no session, and tag-based selection silently drops it.
- **Fix:** write config first (or roll back the session deletion on failure).

### 3.11 LOW — `TELE_PHONE` is referenced but never implemented
- **Locations:** `src/commands/account.rs:445`; `docs/security.md:37`
- The non-TTY warning tells users to "prefer TELE_PHONE env or a stdin prompt", and security.md documents `TELE_PHONE` as the automation path — grep shows no code ever reads it. The only phone source is `--phone`/stdin.
- **Fix:** implement `TELE_PHONE` in the code-login path or reword both spots.

### 3.12 LOW — Escaped quotes corrupt `.env` values
- **Location:** `src/config.rs:113-140` (`strip_env_value`)
- **What:** no backslash-escape handling: `TELE_API_HASH="a\"b"` closes the quote at the escaped `"` and returns `a\"b"` (stray quote + backslash). Values containing `\"` are mis-parsed. (Plain quotes, inline comments, CRLF handled correctly — tests at config.rs:544-568.)
- **Fix:** handle `\` escapes or reject them.

### 3.13 LOW — Windows: no file-permission tightening
- **Location:** `src/fs_util.rs:15-18`
- security.md:42 promises `0o600` for session files; on Windows, `.env`/`config.toml`/sessions get default user-profile ACLs.
- **Fix:** document, or apply `icacls`-style restriction.

### Verified clean (slice 3)
Proxy consistency (every command builds clients exclusively via `ClientGuard::connect` → `proxy_url_for` honored everywhere; per-account proxy overrides global, tested); round-trip of known config fields safe; config.toml BOM handled by toml_edit (unlike `.env`); env precedence (real env overrides `.env`); CRLF/inline comments/quotes/`export` prefix handled; api_id validation (positive-integer, overflow/float/empty); no panics on any config parse/IO path (`CREDS_CACHE.lock().unwrap()` is poison-safe); no api_id/hash/phone in error strings/logs/JSON; `.env` chmod-before-read on Unix; cache stamps not invalidated by chmod (ctime ≠ mtime); `flood_sleep_threshold = 0` is not a busy-loop (grammers retry_policy.rs:62-82); creds() never prompts, no double-prompt risk.

---

## Slice 4 — Peer cache, access hashes, entity resolution (`src/entities.rs`)

Verified environment facts (grammers 0.10.0): `Client::resolve_peer` always fires an RPC (`users.getUsers`/`messages.getChats`/`channels.getChannels`) using the ref's cached hash; result `peers.take(peer.id).ok_or(InvocationError::Dropped)` — the source of the AGENTS.md "dropped (cancelled)" gotcha. `build_peer_map` auto-caches only peers **with** an auth hash. `PeerId::from_bot_api_dialog_id` encodes bot-api convention: positive=user, `-999999999999..=-1`=chat, `-1997852516352..=-1000000000001`=channel. Basic groups (`InputPeerChat`) need no access hash.

### F1 HIGH — Negative/positive numeric ids resolve to the **wrong peer kind**, up to sending to the wrong chat
- **Locations:** `src/entities.rs:44-54` (`resolve_peer` Numeric), `114-117` (`cached_dialog_ref`), `119-137` (`cached_ref`), `139-158` (`checked_fallback_ref`)
- **What:** the three probes interpret the *same token* with **different conventions** (token `-123`: step 1 bot-api → chat; step 2 probes channel first then chat; step 3 channel only):
  - **(a) Uncached basic group by bot-api id:** `--chat -123`, group 123 not cached → step 3 builds `InputPeerChannel{channel_id:123, access_hash:0}` → `channels.getChannels` on a chat → `CHANNEL_INVALID`, even though `messages.getChats` (hash-free) would have resolved it. 100% reachable for any uncached group id. (Previously flagged INT-08 — re-confirmed.)
  - **(b) Wrong peer — send to the wrong chat:** group 123 uncached but *channel with bare id 123* cached → step 2 returns the channel ref → resolves channel `-100123` → `tele msg send --chat -123` sends to the channel. Contradicted by the code's own step-1 convention. Enshrined by contradictory tests `cached_dialog_ref_treats_negative_bare_id_as_chat_not_channel` (entities.rs:571-588) vs `cached_ref_prefers_channel_over_chat_for_negative_id` (entities.rs:441-476).
  - **(c) Same collision for positive ids:** `--chat 123`, user 123 uncached but channel bare 123 cached → step 2 probes `[user, channel]` → resolves the **channel**.
- **Fix:** pick one convention per token (bot-api for all negatives: small negative → `InputPeerChat`, `-100…` → channel) and apply it identically in all three probes; build `InputPeerChat` in the fallback.

### F2 MEDIUM — Stale cached user access_hash → silent empty-user stub (no refresh path)
- **Location:** `src/entities.rs:44-53` + grammers `chats.rs:621-633, 914-922`
- **What:** if the session's cached hash for a user is stale, `users.getUsers` returns `userEmpty`; grammers' `peers.take(peer.id)` **succeeds** (stub carries same id), so `resolve_peer` returns `Ok(Peer::User(empty))` — no error. The stub has `auth() = None`, `to_ref` falls back to the session's stale hash; `userEmpty` is never re-cached. Result: `profile get` prints a nameless user and exits 0; `msg send` fails later with `USER_ID_INVALID`/`PEER_NOT_CACHED` with no hint. Channels in the same state fail hard with `CHANNEL_INVALID`.
- **Fix:** after `resolve_peer`, detect `Peer::User` whose `raw` is `User::Empty` → emit "user deleted or hash stale; run `tele dialog list` or use @username"; consider dropping the stale hash and retrying once.

### F3 MEDIUM — `+phone` resolution permanently adds the number to the account's contact list
- **Location:** `src/entities.rs:17-29` (the `Target::Phone` branch)
- **What:** every command that accepts `+<phone>` (`msg send`, `contact block/unblock`, `privacy allow/deny`, `chat invite/kick` users, `listen --chat`) fires `contacts.importContacts`, which **persists the number as a contact** — even when the subsequent command fails (`USER_NOT_FOUND`; privacy-gated `ImportContacts` returns no user but still imports the number; documented in AGENTS.md gotcha). Unwanted side effect on an account the CLI owns, invisible to the user. (Previously flagged INT-07 — re-confirmed.)
- **Fix:** after extracting the user, immediately `contacts.deleteContacts` the id, or keep `ImportContacts` only in `contact add` and resolve phones another way.

### F4 MEDIUM — `dialog drafts` emits chat/channel ids in a third, incompatible convention
- **Location:** `src/commands/dialog.rs:159-163`
- **What:** `Peer::Chat(p) => -p.chat_id`, `Peer::Channel(p) => -p.channel_id` — a channel draft id comes out as `-123456`, which is **neither** the bot-api id (`-100123456`) **nor** the bare id (`123456`) emitted by `dialog list`/`peer_key` (`serialize.rs:49-55` → `PeerId::bare_id`). Feeding a drafts id back into `--chat` routes into F1's ambiguity or `INVALID_PEER_ID`. `Peer::User` ids are consistent; only group/channel are mangled.
- **Fix:** encode like the rest of the codebase — channel → `-(1000000000000 + id)`, chat → `-id`, or bare positive ids.

### F5 LOW/MEDIUM — t.me link edge forms are mis-parsed into usernames
- **Locations:** `src/entities.rs:91-101` (`classify_target`), `160-178` (`parse_username`)
- `t.me/+<phone>` (phone deep link) → parsed as username → `USERNAME_NOT_FOUND` (the phone branch only fires on bare `+phone` targets, not link-embedded ones). `t.me/c/<id>` (private-channel link) → `USERNAME_NOT_FOUND`. `t.me/s/<user>/<post>` (story links) → `"s"`. `t.me/joinchat/<hash>` outside `chat join` → `"joinchat"` (`chat join` only works because chat.rs:163 intercepts via `parse_invite_link` first). `t.me/` (empty) → `resolve_username("")` → `USERNAME_INVALID`.
- **Fix:** classify the link's extracted segment (strip `+`-phones → phone branch; detect `c/`, `s/`, `joinchat`, `+` invite hashes and error with a targeted message or route to the invite path).

### F6 LOW — Uncached numeric ids can never actually resolve: the "fallback" is functionally dead
- **Locations:** `src/entities.rs:51-54, 139-158`
- Positive uncached id: `checked_fallback_ref` builds `InputPeerUser{hash:0}` → `getUsers` returns `userEmpty` stub → downstream `peer_ref` → `PEER_NOT_CACHED`, or "dropped (cancelled)". Uncached `-1001234567890`: `raw = unsigned_abs()` falls outside `PeerId::channel`'s `MONOFORUM_ID_RANGE` start (peer.rs:157) → `INVALID_PEER_ID` — misleading (the id is valid; the hash is unknown). **Correction to review-2026-08.md M6:** *cached* `-100…` ids DO resolve today via `cached_dialog_ref` (`from_bot_api_dialog_id`, proven by test entities.rs:326-345); M6's "even when cached" claim is stale for current code.
- **Fix:** error-message hint ("id not in cache: use @username or run `tele dialog list`"); try `PeerId::from_bot_api_dialog_id` before the bare probes.

### F7 LOW — Every numeric resolution costs a network RPC even when fully cached
- **Locations:** `src/entities.rs:45, 49, 52` → grammers `chats.rs:621-667`
- `resolve_peer` never short-circuits on cached `PeerInfo`; each `--chat <id>` issues `getUsers`/`getChats`/`getChannels`. With `--parallel 3` fanout each account pays 1-2 RPCs per command; `getUsers`-style calls count toward flood limits. Performance/flood-avoidance nit, not a correctness bug.
- **Fix:** skip the RPC when the session has a full `PeerInfo` for the id.

### F8 LOW — Security nit: failed resolution echoes a `+phone` target to stderr
- **Location:** `src/commands/listen.rs:99-101`
- `cannot resolve chat {target:?}: …` prints the user-supplied `--chat` value — a `+<phone>` number if one was passed — to stderr, which per AGENTS.md is where logs live. Other commands echo targets only into `--json` output (user's own input; acceptable).
- **Fix:** redact `+`-prefixed targets in error strings, or document.

### Verified clean (slice 4)
Phone-before-numeric precedence consistent at every one of the 81 call sites (all funnel through `resolve_peer`; no caller re-implements parsing). "me" handling (`me`, `@me`, `t.me/me`, `https://t.me/me` → `get_me()`); no `resolve_peer(InputPeerSelf)` anywhere. Parse edge cases (trim, i64 extremes, `0`, empty, `+`-alone, `@`-stripping, `?`/`#`/path suffixes) covered by tests; overflow impossible. Cache write correctness (`cache_chat` → `PeerInfo::from` maps kind+hash correctly; created chats cached; no cross-keying). Concurrency: no cache shared across fanout tasks, no lock held across `.await`. Access hashes never reach logs or JSON. Error variants correct (`USERNAME_NOT_FOUND`, `INVALID_PHONE`, `INVALID_PEER_ID` at the right sites).

---

## Slice 5 — Execution engine, account selection, parallelism (`src/executor.rs`)

### 5.1 HIGH — `msg download` — cross-account file collision
- **Locations:** `src/commands/msg.rs:893-946` (handler at 898-943), invoked via `run_fanout` (executor.rs:21)
- **What:** `--dir` is shared by all accounts. Each per-account task computes `path = out_dir.join(download_name(&msg))` (msg.rs:930); the filename depends only on the message's media (`photo.jpg` for any photo, msg.rs:952; document name otherwise). With `--account a --account b --parallel 2 --chat X --id 5`, both tasks call `download_media(&path)` on the *same* file concurrently → interleaved partial writes corrupt the file; the `bytes` value (msg.rs:935-940) races. Even sequentially, account B silently overwrites account A's download. `--json` reports `path`+`bytes` per account which look correct while the on-disk content is B's or corrupted.
- **Fix:** per-account subdir (`out_dir/<account>/`) or account-prefixed filename.

### 5.2 HIGH — Human-mode output printed from inside parallel handlers
- **Locations:** `profile.rs:115-119` (worst: per-line `println!`), `chat.rs:360-370`, `chat.rs:549-561`, `dialog.rs:109-126`, `dialog.rs:174-190`, `contact.rs:93-105`, `topic.rs:134-146`, `msg.rs:705-718`, `msg.rs:872-885` — all inside the per-account closure bodies.
- **What:** with `--parallel 2-3` and 2+ accounts, `profile get` prints `k: v` lines from different accounts interleaved arbitrarily (no account label); table-printing commands emit whole tables atomically but in nondeterministic order with identical headers — the reader cannot tell which table belongs to which account. The executor collects results in sorted account order (executor.rs:116) and JSON is deterministic, but human stdout is emitted by racing tasks before `collect_outcomes` — the ordering contract is broken exactly where the account label matters most.
- **Fix:** buffer rows per account in `data`, print tables once after `run_fanout` returns (pattern already used by `account status`, account.rs:113-118), with account headers.

### 5.3 MEDIUM — Failed connect leaves a zombie session file that pollutes all future `--account all` runs
- **Locations:** `executor.rs:143-146` (selection accepts configured-but-sessionless names) + `session.rs:58` (`SqliteSession::open` creates the file) + `client.rs:25` + `client.rs:62-70` (authorize fails)
- **What:** `--account pending` where `pending` is registered in `config.toml` but never logged in: selection passes, `open_session` creates `sessions/pending.session` (empty, no auth key), `authorize` fails with `AuthError`. The file persists. The *next* `--account all` run now includes `pending` (executor.rs:140 extends from `sessions`), so every future default/`all` fanout attempts a connection and fails for that account → permanent `EXIT_PARTIAL` (2) even though the user never logged it in.
- **Fix:** don't create the session file in `open_session` when it doesn't exist (or clean the empty file when authorize fails).

### 5.4 MEDIUM — Config/credentials failures in the fanout path exit 3, not 1 — `exit_code` field is dead data
- **Locations:** `executor.rs:179-205` (`envelope_exit_code`), `credentials.rs:4`, `error.rs:21`, `executor.rs:30` + `executor.rs:125`
- **What:** `envelope_exit_code` only inspects `a.exit_code` for the `EXIT_AUTH` count (executor.rs:188-195); the per-account `exit_code` field is otherwise ignored, and the usage branch keys on the JSON string `"UsageError"` only. Consequences: (a) missing/broken `.env` in a handler → `ConfigError` → exit 3, contradicting `error.rs:21`; (b) malformed `config.toml` at executor.rs:30 → `anyhow` → `Other` → 3. Test `config_failure_exits_all_failed_not_usage` (executor.rs:465-474) enshrines the mismatch. A brand-new user with no `.env` gets exit 3 on every command.
- **Fix:** map `load_config`/`credentials` failures to `TeleError::Config` at the `run_fanout` boundary (before spawn); have `envelope_exit_code` honor per-account `exit_code` or check `"ConfigError"` type.

### 5.5 MEDIUM — `--tag` silently narrows the fanout when tagged accounts lack sessions
- **Location:** `executor.rs:148-159`
- **What:** `--tag iran` on a config with 3 tagged accounts where only 2 have session files: `tagged` (from config) ∩ `sessions` silently yields 2 — no error, no warning about the dropped account. Same for `--account all` + `--tag` intersection (executor.rs:158). By contrast `--account pending` (sessionless) *is* selected and fails loudly — inconsistent semantics between the two selectors.
- **Fix:** warn per skipped account (config-tagged but no session) on stderr, or error like the unknown-tag branch.

### 5.6 LOW — Account named `all` is unselectable
- **Locations:** `executor.rs:139` + `session.rs:9-21` (`validate_name` accepts `"all"`)
- `tele account add --name all` succeeds; `--account all` then expands to every session and the literal account `all` can never be addressed.
- **Fix:** reject `all` in `validate_name` (reserved word), or document the ambiguity.

### 5.7 LOW — `print_json` panics on broken pipe
- **Locations:** `output.rs:48-50`, called from `executor.rs:166-172` (`print_envelope`)
- `tele msg get --json | head -1` → `println!` panics on `EPIPE` → exit 101 with a panic trace on stderr. `print_json_result` (output.rs:52-60) already does it correctly with `?`.
- **Fix:** use `print_json_result`-style `writeln!` + `?` in `print_json`.

### Verified clean (slice 5)
Semaphore & clamp (`effective_parallel` clamps flag and config to 1-3; `--parallel 0`/huge both clamp; `u32` rejects negatives at clap parse; permits acquired before the handler, dropped on panic — no deadlock). Panic containment & task joins (every `JoinHandle` awaited; a panicking task becomes a failed outcome without aborting others). Deterministic ordering (outcomes sorted by account name). FloodWait/SlowModeWait via `AutoSleep` with `flood_sleep_threshold` from config; above-threshold waits surface with `seconds`; no retry storms. Cancellation/RAII (Ctrl+C → 130; `ClientGuard::drop → disconnect()` on every path). No cross-account shared mutable state. Selection core: exact-match only, empty selection → `EXIT_USAGE`, `all`/default → sessions only, unknown account/tag → exit 1; 22 unit tests.

---

## Slice 6 — Output envelopes, JSON serialization, logging, contract tests (`src/output.rs`, `src/serialize.rs`, `src/logging.rs`, `tests/contract.rs`)

### F1 HIGH — `--dry-run` violates the documented `data.would` contract; the contract test locks the wrong shape
- **Locations:** `docs/cli-contract.md:75` (rule) vs `src/commands/msg.rs:339, 398, 439, 508, 641, 677, 745, 805, 849, 904` (violations); `tests/contract.rs:344-370` (`dry_run_envelope_shape`)
- **What:** the contract mandates `--dry-run` yields `ok=true`, `dry_run=true`, and `data.would` describing the action. Only `account add/login/logout/remove` emit `would` (`src/commands/account.rs:400`). Every `msg` subcommand returns `{"dry_run":true,"chat":...}` (or `id`/`ids`/`limit`/`query`/`last`/`unread` — never `would`). The contract test asserts `data["dry_run"]` and `data["chat"]` — it verifies the implementation's shape, not the documented contract, so the deviation ships green. Same pattern in chat/dialog/topic/profile paths.
- **Fix:** emit `would` on all dry-run handlers; add a contract test asserting `data.would` is a non-empty string, keeping the existing assertions.

### F2 MEDIUM — Config/credential failures exit 3 ("Telegram/IO"); the `Config => EXIT_USAGE` arm is dead
- **Locations:** `src/error.rs:21`, `src/error.rs:61-65` (`From<anyhow> → Other`), `src/commands/credentials.rs:4`, `src/executor.rs:179-205`, `tests/contract.rs:372-405`
- **What:** `TeleError::Config::exit_code()` returns 1, but no production path reaches it (details in 3.4 / 5.4). `listen` creds failure (listen.rs:76) → `aggregate_exit` → 3. Net: config failures are consistently 3, which the contract defines as "All selected accounts failed (Telegram / IO)" — a broken config is neither.
- **Fix:** route config/credential failures to `TeleError::Usage` (exit 1) consistently; update `error_envelope_shape`.

### F3 MEDIUM — `logging.rs` unit tests fail deterministically whenever `TELE_LOG` is set
- **Locations:** `src/logging.rs:41-44` (early return), `:74-87`, `:129-141`
- **What:** `set_flags` returns immediately when `TELE_LOG` is in the environment; the tests then call `set_flags(0, true)` and assert `min_line_level() == LEVEL_ERROR`; since the call was a no-op, the assertion fails. AGENTS.md itself instructs `$env:TELE_LOG="debug"`, so the documented dev shell breaks `cargo test`. CI (no `TELE_LOG`) stays green → test blindness in exactly the environment the feature is meant to be exercised in.
- **Fix:** `std::env::remove_var("TELE_LOG")` before `set_flags` in the tests (restore after), or make `set_flags` take the env lookup injectably.

### F4 MEDIUM — `peer_key.id` (bare) vs `peer_name` fallback (signed) disagree for uncached peers; empty names emitted as `""`
- **Locations:** `src/serialize.rs:10-11` vs `:49-55`
- **What:** `peer_name` falls back to `peer.id().to_string()` — for a channel, the signed PeerId (`-1001234567890`) — while `peer_key` emits `"id": peer.id().bare_id()` (`1234567890`). One object can contain `{"id":1234567890,"name":"-1001234567890"}`. Triggers when `Peer::name()` returns `None`: inaccessible/deleted group or user, or the self-user sentinel. Separately, a user with an empty first name yields `"name":""` instead of the id fallback.
- **Fix:** fall back to `peer.id().bare_id()` (or consistently the signed id) in `peer_name`; treat empty names as `None`.

### F5 LOW — `peer_key` silently emits `id: 0` via `bare_id().unwrap_or_default()`
- **Location:** `src/serialize.rs:51`
- **What:** `bare_id()` is `None` only for `PeerId::self_user()` (grammers-session peer.rs:259-262); `unwrap_or_default()` manufactures `0`. Reachability is narrow (grammers synthesizes the sentinel in `Message::sender_id()` for outgoing private messages without `from_id`), but when it surfaces the row contains `"id":0` instead of an omitted key, and `name` falls back to the sentinel number `1099511627776` (compounding F4).
- **Fix:** skip `id` when `bare_id()` is `None`, the way `listen.rs` already omits `chat_id`.

### F6 LOW — `print_json` panics on broken pipe and uses `expect`; `print_json_result` already does it right
- **Locations:** `src/output.rs:48-50` vs `:52-60`
- `--json | head -1` → panic "failed printing to stdout: Broken pipe", exit 101; the `expect` would likewise panic if a `Value` ever failed to serialize. This is REL-09 from review-2026-08.md, still open.
- **Fix:** route through `print_json_result`.

### F7 LOW — Dual log paths: `log_line("debug")` can never print, even under `-vv` or `TELE_LOG=debug`
- **Locations:** `src/output.rs:34-46`, `src/logging.rs:41-55`
- `-v`/`-vv` raise only the `log` crate level; `MIN_LINE` (the `log_line` floor) is touched only by `--quiet`. With `TELE_LOG=debug`, `set_flags` early-returns, so `MIN_LINE` stays 1 and `log_line("debug")` (mapped to 0) is always filtered. Latent today (no production caller passes `"debug"`); a trap for future contributors.
- **Fix:** derive `MIN_LINE` from the same effective level used by `set_max_level`, or drop the second system.

### F8 LOW — Machine-API contract-test coverage gaps in `tests/contract.rs`
- **Locations:** `tests/contract.rs:298-405`; unit side `src/serialize.rs:373-443`
- (a) `message_to_json`'s populated `peer`/`sender` path is never exercised — every unit test builds messages via `from_raw_short_updates` with an empty peer map, so `peer`/`sender` are always `null` and the F4/F5 id/name logic is untested; (b) no unicode / large-id / control-char-in-text case; (c) no `data.would` assertion (F1); (d) `completions` writes a shell script to stdout even with `--json`, and no contract test pins clean-JSON stdout for every `--json` invocation.
- **Fix:** add a unit test with a populated `PeerMap` asserting `peer`/`sender` objects; add `would`/unicode/i64-edge assertions.

### Verified clean (slice 6)
JSON escaping entirely via `serde_json::json!` — no manual escaping, unicode/control chars handled by serde; `media_name` has no panic path (single-variant enum, exhaustive match). Key ordering deterministic (`serde_json::Map` = BTreeMap; `Envelope` = declaration order). Timestamps consistent (message `date` RFC3339 UTC whole seconds; `Raw` `state.date`/`seq` unix ints; both documented). null/omitted/empty-string internally consistent. Large ids exact i64 JSON integers (>2^53 JS-precision caveat is external). No api_hash/phone/session-string in any log line or JSON allowlist; proxy URL carries no userinfo. Every human table in machine mode is gated by `machine_mode()`; logs go to stderr only; `print_json_result` locks+flushes per line — no JSONL corruption. Contract tests are real, not vacuous (they spawn the actual binary). No panic in `serialize.rs`; the only serialization `expect` is output.rs:49 (F6). listen `Raw` rows: the M3 "raw payload dropped" finding is fixed at HEAD.

---

## Slice 7 — Account lifecycle (`src/commands/account.rs`)

### 7.1 MEDIUM — `add` never validates the account name, so it can register entries that are unusable and un-removable
- **Locations:** `src/commands/account.rs:146-151` (insert); `session::validate_name` at `src/session.rs:9-21` enforced only by `open_session` (session.rs:53) and `remove_session` (session.rs:43)
- **What:** `tele account add --name "foo bar"` (or `""`, `"../x"`) writes the raw string as a TOML key and reports `registered: true` (exit 0). Every later operation fails: `login` → `validate_name` error; `remove` → `validate_name` error first (account.rs:336), so the poisoned entry can never be cleaned up through the CLI. `list`/`status` never show it (session-file driven). `select_from` (executor.rs:143) still accepts the bad name → per-account failures in fanout.
- **Fix:** call `session::validate_name` in `add` before writing.

### 7.2 MEDIUM — Ctrl-C cannot interrupt the blocking code/2FA prompts (login appears to hang)
- **Locations:** `src/commands/account.rs:235-238`, `250-251` call `prompt_line` (account.rs:452-464) which does a **synchronous** `read_line` on the runtime worker thread; top-level select is `src/main.rs:146-149`
- **What:** `tokio::select!` runs both branches on one task. While `prompt_line` blocks in `read_line`, the `ctrl_c()` branch can never be polled. Pressing Ctrl-C while waiting for an SMS code does nothing visible; the user must type + Enter (which submits a garbage code and exits with "invalid code"). The SIGINT → exit-130 handling is silently bypassed on this path.
- **Fix:** read stdin off-thread (`tokio::task::spawn_blocking` for `read_line`) or select with `ctrl_c`.

### 7.3 MEDIUM — `TELE_PHONE` env is advertised but never implemented
- **Locations:** warning text `src/commands/account.rs:442-450`; `docs/security.md:37`; `validate_login` at `account.rs:165-182` hard-requires `--phone`
- **What:** grep shows `TELE_PHONE` is referenced in only those two text spots and **never read**. A user following the documented advice sets `TELE_PHONE`, runs `tele account login --name work --method code`, and gets `--phone required for code login` (exit 1). The argv warning's suggested alternative doesn't exist.
- **Fix:** read `TELE_PHONE` in `validate_login`/`login` as fallback when `--phone` is absent, or delete the claim from the warning + security.md.

### 7.4 MEDIUM — "exclusive lock; refuse start if locked" (security.md:19) is not implemented — two clients on one session file is possible
- **Locations:** `src/session.rs:52-60`, `src/client.rs:18-53` — no lock/flock/lockfile anywhere (grep confirms)
- **What:** any two concurrent `tele` invocations for the same account (e.g. `listen` + `status`, or two `login` runs) both open the same SQLite file via libsql and share one auth key. The second `login` can overwrite the session's auth key under the first's feet; server-side this can surface as `AUTH_KEY_DUPLICATED` or SQLite "database is locked" mid-run.
- **Fix:** OS advisory lock (or `{name}.lock` acquired with try-lock) in `open_session`, refusing a second client — matching security.md's stated mitigation.

### 7.5 MEDIUM-LOW — `remove`/`logout` delete only the main `.session`; journal/WAL/shm siblings are never cleaned
- **Locations:** `src/session.rs:42-50`, `src/commands/account.rs:357-376` delete only `{name}.session`; the project acknowledges the sibling in `.gitignore` (`*.session-journal`) and `docs/security.md:41`
- **What:** if libsql left a rollback `-journal` or `-wal`/`-shm`, `remove`/`logout` leave it behind. A later `login` at the same path opens a fresh DB while a stale hot journal may exist → SQLite hot-journal rollback ("file is not a database"), and the leftover can carry session-key material on disk past the point the user believes the session is gone.
- **Fix:** delete `{name}.session`, `{name}.session-journal`, `{name}.session-wal`, `{name}.session-shm` together. *(Caveat: libsql's exact journal lifecycle is not verifiable offline.)*

### 7.6 LOW-MEDIUM — `status`/`login` create a session file for accounts never logged in, so `list` reports "session: present" falsely
- **Locations:** `ClientGuard::connect` → `src/session.rs:52-60`; reached from `status` (account.rs:98) and `login` (account.rs:200-201); `list` hardcodes `"session": "present"` at account.rs:67
- **What:** `tele account status --account pending` (configured, never logged in) creates `sessions/pending.session` containing a fresh, never-used auth key. `tele account list` then shows `pending` with `session: present` even though no login ever completed.
- **Fix:** in `status`, check file existence before `open_session`; have `list` verify `is_authorized` or track a login-completed marker instead of hardcoding `present`.

### 7.7 LOW — QR login never persists the self-user peer or update state (`complete_login` is not invoked)
- **Location:** `src/client.rs:72-131` (`qr_login`) — the `LoginToken::Success(_)` arm (client.rs:128) discards the returned `auth::Authorization`; `complete_login` (grammers auth.rs:111-145) is only called from `sign_in`/`check_password`/`bot_sign_in`
- **What:** after QR login the session has a registered auth key but an empty `peer_info` (no `is_self`) and no `update_state`; after code login both would exist. Practically: `get_me()` still works, but `resolve_peer(InputPeerSelf)` fails and `listen` starts from a blank update state instead of a proper baseline.
- **Fix:** after `qr_login` succeeds, replicate `complete_login`'s side effects (get_me, cache `is_self`, set update state).

### 7.8 LOW — dead `SignInError::InvalidPassword` arm in the `sign_in` match, mapped to a misleading message
- **Location:** `src/commands/account.rs:273-275`
- **What:** verified in vendored grammers `sign_in` (auth.rs:339-365) that `InvalidPassword` is returned **only** by `check_password`, never by `sign_in`. The arm is dead code; if a future grammers bump ever surfaces it here, the user would be told "invalid code" when they entered a wrong 2FA password.
- **Fix:** remove the arm or map it to `Auth("invalid password")`.

### 7.9 LOW — failed login leaves a partial session file and no config entry, with no wrong-code retry loop
- **Location:** `src/commands/account.rs:244-282`
- **What:** `InvalidCode`/2FA-failure/`SignUpRequired` return `Err` without touching the session file created by `connect` (client.rs:25). `list` is session-driven, so a failed login still shows the name as `session: present`. No retry loop exists — the next attempt re-runs `request_login_code` (a new SMS each time) rather than re-prompting for the code. Codes are single-shot (`PHONE_CODE_*` → `InvalidCode`, verified grammers auth.rs:362).
- **Fix:** on failed `sign_in`, remove the freshly-created empty session or keep an explicit "not logged in" marker; consider bounded re-prompting.

### 7.10 LOW — non-atomic `write_config` (crash mid-write → corrupt `config.toml` breaks every later command)
- **Location:** `src/config.rs:270-282` (plain `std::fs::write`), called from `add` (account.rs:151) and `remove` (account.rs:343). Already flagged as L-S2 in docs/audit-2026-08.md; still present.
- **Fix:** write to `config.toml.tmp` then atomic rename.

### 7.11 LOW/UX — `account` subcommands use `--name` while the global selection flag is `--account`
- **Locations:** `LoginArgs`/`LogoutArgs`/`RemoveArgs`/`AddArgs` (account.rs:18-43) define `--name`; the global `--account` (main.rs:26-33) is honored only by `status` (via `run_fanout`)
- **What:** `tele account login --account work` fails with clap's "required arguments not provided: --name", while `tele account status --account work` works. Inconsistent selection model within one command group.
- **Fix:** accept the global `--account` as a fallback for `--name`, or drop `--name` and document that account ops use the global selector.

### Verified clean (slice 7)
Secret hygiene: no phone/api_hash/2FA/code ever reaches logs or the JSON envelope; phone appears only in the interactive stderr prompt when stderr is a TTY; 2FA is stdin-only; `invocation_message` (error.rs:85-90) exposes only RPC name/code. File permissions applied (`open_session`, `write_config`, `.env`). `logout` error handling: non-401 `sign_out` failure returns an error and **keeps** the local session (no false "logged out"); 401 path deletes the file; `remove_session_file_retry` (20×5ms) handles the Windows open-handle race (tested). `add` rollback: single `write_config`; failure leaves no half-written entry; `CONFIG_CACHE` is stamp-keyed so post-write re-reads are not stale. `is_authorized` is a network `GetState`, so revoked sessions are correctly reported and re-login works. Name validation on session paths rejects bad names before touching the filesystem; no path traversal reachable. Exit codes consistent with cli-contract. (Note: AGENTS.md's "spawn with piped stdin, detect prompt, poll code file" describes the *caller's* integration — the binary itself has no code-file polling.)

---

## Slice 8 — `msg` core (send / get / edit / delete / forward) (`src/commands/msg.rs`)

### H1 HIGH — `msg delete` reports a no-op / partial deletion as exit-0 success; "both-sides" deletion is hardcoded with no opt-out
- **Locations:** `src/commands/msg.rs:475-483` (batch delete), `src/commands/msg.rs:64-76` (DeleteArgs — no `--revoke`/self-only flag), `grammers-client-0.10.0/src/client/messages.rs:883-886` (`messages.deleteMessages { revoke: true }`)
- **What:** `Client::delete_messages` returns `affected.pts_count` — the count actually deleted, *not* the number of requested ids. For messages you cannot delete (others' messages in private chats, already-deleted ids, no permission), Telegram silently skips them: the CLI returns `{"deleted": 0}` (or a smaller count) with **exit 0**. `revoke: true` is hardcoded in grammers with no way to delete only for yourself.
- **Impact:** wrong output + wrong exit code on a destructive command; irreversible both-sides deletion of private-chat messages with no self-only option or warning.
- **Fix:** surface per-id outcomes or at least `requested` vs `deleted` counts; add `--revoke`/`--self-only` plumbing (raw `messages.deleteMessages` with `revoke: false`); consider exit 2 (partial) when `deleted < ids.len()`.

### H2 HIGH — `msg forward --silent` reports failure for a forward that already succeeded → retry duplicates the messages
- **Locations:** `src/commands/msg.rs:584-589` (error branch in `forwarded_ids`), invoked at `src/commands/msg.rs:623-624`
- **What:** `forward_silent` fires `messages.forwardMessages` (RPC succeeds — messages ARE forwarded), then `forwarded_ids` errors with `"forward succeeded but no new message ids were reported"` when the response `updates`/`updatesCombined` contains no `UpdateMessageId` matching the generated `random_id`s, or the response is any other `Updates` variant (`_ => &[]`, msg.rs:574). The account then fails with exit 3 even though the forward happened. Retrying duplicates messages.
- **Fix:** after a successful `invoke`, treat "no ids extracted" as a warning with `{"forwarded": []}` plus exit 2, or re-verify by fetching the destination's latest messages — never fail the account when the RPC succeeded.

### H3 HIGH — numeric `-100…` channel targets fail with `INVALID_PEER_ID` for any channel not already in the session cache
- **Locations:** `src/entities.rs:47-54` (Numeric branch), `114-117` (`cached_dialog_ref`), `139-158` (`checked_fallback_ref`)
- **What:** `raw = id.unsigned_abs()` for `-1001234567890` is `1001234567890`, which lands in the gap between grammers-session's `SUPERGROUP_AND_CHANNEL_ID_RANGE` and `MONOFORUM_ID_RANGE` (verified `grammers-session-0.10.0/src/peer.rs:151-157`). Both `cached_ref` and `checked_fallback_ref` probes return `None` → `INVALID_PEER_ID`. `cached_dialog_ref` (via `PeerId::from_bot_api_dialog_id`) rescues only the *already-cached* case. (Partially contradicts review M6, which claimed "rejected even when cached" — the cached path works today; the *uncached* path is the live bug.)
- **Fix:** when `id < -1000000000000`, first try `PeerId::channel(id + 1000000000000)` (bare id) as a live `channels.getChannels` probe before the range-gapped fallback.

### H4 MEDIUM — phone `--chat` targets permanently add the number to contacts as a side effect of any command
- **Location:** `src/entities.rs:17-29` (`contacts.ImportContacts` in the phone branch)
- **What:** `msg send --chat +<phone>` (or get/edit/delete/forward) calls `contacts.importContacts`, which **adds the number to the account's contact list permanently**, even if the eventual resolution fails or the command is read-only (`msg get`). Documented as INT-07; still present.
- **Fix:** use `contacts.resolvePhone` (or a dedicated lookup) in the resolve path; keep `ImportContacts` only for `contact add`.

### M1 MEDIUM — `send`/`edit` accept empty or whitespace-only `--text` → server `MESSAGE_EMPTY`, exit 3 instead of exit 1
- **Locations:** `src/commands/msg.rs:180-192` (`validate_send` only rejects `None`), `:361-365` (builds `InputMessage::new().text("")`), `:388-417` (`edit` has no validation at all)
- **What:** `--text ""` or `--text "   "` passes validation; the send connects and the server rejects with `rpc error 400: MESSAGE_EMPTY` → `TeleError::Invocation` → **exit 3** instead of a pre-flight usage error **exit 1**. Classic trigger: empty shell-var expansion.
- **Fix:** reject empty/whitespace-only `--text` in `validate_send`; add equivalent validation for `edit` (allow empty only if the intent is caption-clear, then require media context or document it).

### M2 MEDIUM — `msg forward` silently drops forwards that failed; no per-id failure info
- **Location:** `src/commands/msg.rs:535-540` (`filter_map` over `Vec<Option<Message>>`)
- **What:** grammers `forward_messages` returns `None` for any id it couldn't map/forward. tele-cli drops the `None`s and returns only the successes with exit 0 — a 5-of-100 failure is indistinguishable from full success. (The `--silent` path instead hard-errors — see H2.)
- **Fix:** report `requested` vs `forwarded` counts (or per-id map); consider exit 2 (partial).

### M3 MEDIUM — `--chat <small negative id>` (uncached basic group) sends the wrong peer type → `CHANNEL_INVALID`
- **Location:** `src/entities.rs:149-157` (`checked_fallback_ref` always builds `InputPeerChannel` for negative ids)
- **What:** for an *uncached* basic group `--chat -123`, the fallback produces `InputPeerChannel { channel_id: 123, access_hash: 0 }` → `channels.getChannels` → `CHANNEL_INVALID`. The group should be probed with `messages.getChats`/`InputPeerChat`. (Cached groups are rescued earlier.)
- **Fix:** for negative ids that parse as small chat-range ids, build `InputPeerChat` and probe via `messages.getChats` before the channel probe.

### L1 LOW — `validate_markdown` over-rejects valid text (false-positive Usage error)
- **Location:** `src/commands/msg.rs:215-226` (scans raw text for any `tg://user?id=` substring, regardless of context)
- **What:** `[x](https://example.com/tg://user?id=abc)` — a normal URL link whose dest merely *contains* the prefix — is rejected with `"invalid tg://user?id= mention"`, even though grammers' markdown parser treats it as `TextUrl` (no panic, no mention).
- **Fix:** only validate mentions grammers would classify as mentions — run detection on link destinations (or reuse the grammers parse and check for `MentionName` entities) rather than substring scanning the raw text.

### L2 LOW — `--format markdown` is silently ignored for file sends
- **Locations:** `src/commands/msg.rs:196-203` (only `--no-preview`+`--file` rejected), `:348-359` (file branch never consults `format`)
- `msg send --file x.pdf --format markdown` passes validation and the flag does nothing; a markdown caption is sent as literal text. (L-B5 in audit-2026-08 still open.)
- **Fix:** reject `--format` with `--file`, or parse the caption with the chosen format.

### L3 LOW — msg dry-run JSON omits `data.would` promised by the contract
- **Locations:** `src/commands/msg.rs:338-340` (send), `:398` (edit), `:439` (delete), `:509` (forward), `:677-683` (get) — vs `docs/cli-contract.md:75`
- **Fix:** include `"would": "send … to <chat>"` etc. in each dry-run payload.

### L4 LOW — `msg send --file <nonexistent>` → exit 3 instead of exit 1; upload-path guard bypassed for non-existent files
- **Locations:** `src/commands/msg.rs:349-353` (`upload_file` error → `TeleError::Other` → exit 3); `src/commands/msg.rs:288-293` (`canonicalize().unwrap_or_else(|_| path.into())` falls back to the raw relative path for non-existent files, bypassing the app-data-dir guard)
- **Fix:** check `Path::exists()` (or `try_exists`) in `validate_upload_path` and reject non-existent/sensitive-relative paths as Usage before connecting.

### L5 LOW — `msg delete --all` + `--ids` together: `--all` silently wins
- **Locations:** `src/commands/msg.rs:419-426` (`validate_delete` allows both), `:448-470` (`if all` branch returns before `:472-483`)
- **Fix:** reject the combination in `validate_delete`.

### L6 LOW — `send` may report a successful `id: 0` message (grammers fallback) instead of an error
- **Location:** `src/commands/msg.rs:376-381`; `grammers-client-0.10.0/src/client/messages.rs:673-696` (`map_random_ids_to_messages` → `MessageEmpty { id: 0 }` fallback when the sent message can't be located in the response updates)
- **Fix:** detect `msg.id() == 0` (or `Message::Empty`) after `send_message` and emit an explicit warning/partial result instead of a success payload.

### L7 LOW — `spec.md` documents `--to`; the CLI and README use `--chat`
- **Locations:** `docs/spec.md:175` (`--to me`) and `:135` vs `src/commands/msg.rs:31` and `README.md:53-54`
- **Fix:** update spec examples to `--chat`.

### Verified clean (slice 8)
Peer-resolution order (phone-before-numeric; `me`/`@me`/`t.me/me` via `get_me()`); FloodWait/SlowModeWait honoring; ClientGuard RAII; no unwrap/expect/`let _ =` error-swallowing in production msg/entities/executor paths; batching (delete/forward chunk ≤100; `delete --all` streams fetch-100/delete-100; `random_ids` monotonic & unique across concurrent accounts); `get` semantics (`--limit` capped 10000; `offset_id` exclusive-before verified in grammers iter_buffer; `--last` = limit 1; JSON shape `{"messages":[…]}`); security guards (sensitive-file upload guard, download-dir guard, `download_name` traversal/illegal-char sanitization, tests present); dry-run discipline (validate + short-circuit before connect); JSON envelope shape and exit-code taxonomy match cli-contract, locked by tests/contract.rs.

### Non-findings (checked, not reported as bugs)
- The 4096-char `--text` length limit is not validated client-side; offline sources can't confirm whether the server errors or silently truncates.
- `edit --text ""` clearing a media caption is legitimate; M1's fix for `edit` must be context-aware.
- `resolve_peer`'s user branch calls `users.getUsers` with `access_hash: 0` for uncached users — the server resolves by id, so uncached lookup is fine; no bug.

---

## Slice 9 — `msg` advanced (search / react / download / read / pin) (`src/commands/msg.rs`, `src/fs_util.rs`)

### 9.1 MEDIUM — `msg download` silently overwrites existing files; multi-account runs corrupt the shared file
- **Locations:** `src/commands/msg.rs:930-931` (`path = out_dir.join(name)` + `msg.download_media(&path)`); name selection `msg.rs:948-958`; grammers `client/files.rs:198,235`
- **What:** `download_name` maps every photo to `photo.jpg`, every sticker to `sticker.webp`, everything else to the document's raw filename or `media.bin`. grammers' `download_media` is documented *"If the file already exists, it will be overwritten"* and opens with `fs::File::create` (truncate). No unique-suffix, overwrite prompt, or existence check anywhere. Two different messages with the same media name into the same `--dir` silently destroy the first file. With `--account all` every account writes the same path — sequentially last-writer-wins; with `--parallel 2..3` two concurrent truncate+write streams interleave into one corrupt file. Result JSON reports one identical `path`/`bytes` for all accounts.
- **Impact:** silent data loss / corrupted downloads; no indication anything went wrong. (Same root as 5.1.)
- **Fix:** unique-suffix or `exists` check (skip/rename/ask); namespace by account (`out_dir/<account>/...`) when fanning out.

### 9.2 MEDIUM — App-data / sessions guard is bypassable on Windows (case-sensitive prefix compare + non-existent-tail fallback)
- **Locations:** `src/commands/msg.rs:276-293` (`validate_download_dir`, `canonical_guard_path`); app dir built non-canonicalized in `src/config.rs:5-39`
- **What:** `canonical_guard_path` falls back to the *raw, un-canonicalized* path whenever `fs::canonicalize` fails (any non-existent tail component). The result is compared with `Path::starts_with(&app_dir)`, which on Windows is **case-sensitive** (`OsStr` equality), while `app_dir`/`sessions_dir` are built straight from env vars and are themselves never canonicalized.
- **When:** `--dir C:\Users\...\AppData\Roaming\telecli\dl` (hand-typed casing differs from `%APPDATA%`) with a not-yet-existing `dl` → `canonicalize` fails → raw path compared case-sensitively → guard passes → `create_dir_all`+download land inside the real app-data dir. Same for a parent component that is a symlink/junction into app data, or any relative path (relative never starts-with an absolute prefix).
- **Impact:** a downloaded file (named e.g. `config.toml` or `.env` from a hostile/odd message) can be written over the CLI's own config/credentials; with a `sessions\...` tail, a media file named `me.session` could collide with a live session file. Reopens the previously-fixed SEC-09 (review-2026-08.md:97) in a subtler form. Same pattern in `validate_upload_path` (msg.rs:258-274).
- **Fix:** canonicalize `app_dir`/`sessions_dir` once, resolve the full `dir` path (create-then-verify or `create_dir_all` before the check), and on Windows compare case-insensitively (lowercase both sides, or `dunce::canonicalize`).

### 9.3 LOW — Failed download leaves a partial/corrupt file at the final path
- **Location:** `src/commands/msg.rs:931`; grammers `client/files.rs:234-241` (`load`: `fs::File::create` then chunked writes, `Err` returned without removing the file) and `files.rs:253-256` (concurrent path pre-allocates `set_len(size)`)
- **What:** if the transfer fails mid-stream, grammers returns an error but the partial file stays at the final destination. Large files (>10 MB) are additionally pre-allocated to full size, so the leftover can look complete while being mostly zeros.
- **Fix:** download to a temp name in `dir`, rename on success, delete on error.

### 9.4 LOW — `msg pin --silent` is dead; pins are always silent
- **Locations:** `src/commands/msg.rs:103` (field parsed) vs handler `msg.rs:631-661` (never reads `silent`); grammers `client/messages.rs:1233` (`update_pinned` hardcodes `silent: true`)
- **What:** `--silent` is parsed but never read; grammers unconditionally sends `silent:true`. Default `msg pin` (which per help implies notification) is actually always silent; `pm_oneside` likewise hardcoded `false`.
- **Fix:** drop the flag or route through raw `tl::functions::messages::UpdatePinnedMessage`.

### 9.5 LOW — `msg react`: no client-side emoji validation; `--remove` silently wins when both flags given
- **Locations:** `src/commands/msg.rs:784-834`, decision at `msg.rs:815-823`; grammers `message/reactions.rs:35-42`
- **What:** `InputReactions::emoticon` does zero validation — the string is passed straight through. An empty string or non-reaction text is only rejected by the server after connect (RPC 400, e.g. `REACTION_INVALID`) as exit 3 instead of a Usage error exit 1 (other commands validate pre-connect). Also `--remove` + `--reaction` together: remove wins, reaction silently ignored.
- **Fix:** validate a single grapheme/emoji before connect (like `topic create --emoji`); reject `--remove --reaction` together.

### 9.6 LOW — `sanitize_download_name` doesn't handle Windows reserved device names
- **Location:** `src/commands/msg.rs:960-975`
- **What:** a document literally named `NUL`, `CON`, `PRN`, `AUX`, `COM1..9`, `LPT1..9` (or `NUL.txt` — the Win32 parser treats the token before the first dot as a device) becomes `out_dir\NUL`, which opens the NUL device: `File::create` succeeds, all bytes are discarded, `metadata().len()` reports `0` while the command returns success.
- **Fix:** map reserved device basenames to `document.bin` in `sanitize_download_name`.

### 9.7 LOW — `msg download` maps runtime failures to `Usage` (exit 1)
- **Locations:** `src/commands/msg.rs:922` (`message {id} not found`), `:933` (`message has no media`)
- Per cli-contract.md:34 ("Do not overload 1 for Telegram errors"), a missing message id or missing media is a runtime failure (exit 3), not usage.
- **Fix:** use `TeleError::Other` for not-found/no-media cases.

### 9.8 LOW — `msg search` feature gap vs the capability claim
- **Locations:** `src/commands/msg.rs:836-891`; `docs/capabilities.md:40`
- The matrix row `msg.search` claims "Search / filters" / `search_all_messages`, but the CLI exposes only in-chat text search with no date filters, no media filter, no `offset_id`/paging, no global search. Empty query is valid and returns every message in the chat.
- **Fix:** add `--min-date/--max-date/--offset-id` (and a global `--chat all`) or trim the matrix/help text.

### Verified clean (slice 9)
search/get paging & termination: `SearchIter`/`MessageIter` honor `.limit(n)` as a hard cap via `IterBuffer::limit_reached`/`determine_limit` — loops cannot run past `--limit` or spin; `--limit 0` yields 0; `--last` → `.limit(1)`. Media-filename path traversal: `sanitize_download_name` strips all `/`/`\`; `..`/`.`/trailing-dot cases covered by tests. read semantics: `mark_as_read` sends `max_id: 0` → whole conversation marked read, matching the contract. react/pin/read/download dry-run & exit flow short-circuit before connect; no unwrap/expect on RPC; all RPC errors through `tele_invocation` (401→Auth, 420→`seconds`, AutoSleep ≤60s). Message-to-chat ownership: `get_messages_by_id` filters by `peer_id` — no cross-chat leak. JSON shapes additive; human tables char-safe (`truncate_text`).

---

## Slice 10 — `chat` core (join / create / leave / invite / participants) (`src/commands/chat.rs`)

### 10.1 CRITICAL — `chat join` with **any** invite link always fails with `INVITE_HASH_INVALID`
- **Locations:** `src/commands/chat.rs:163-168` (call site), grammers `client/chats.rs:742-787` + `chats.rs:793-815`
- **What:** `Client::parse_invite_link(&target)` returns the **bare hash** (`https://t.me/+hash` → `Some("hash")`, `https://t.me/joinchat/ABC` → `Some("ABC")` — grammers chats.rs:770-772/777-779). chat.rs:166 then passes that **bare hash** into `accept_invite_link(&link)`. But `accept_invite_link` *re-parses its own argument* via `Self::parse_invite_link(invite_link)` (chats.rs:797), which requires a full URL with a valid host (chats.rs:743-766). `url::Url::parse("hash")` fails → `None` → `accept_invite_link` returns `Err(400 INVITE_HASH_INVALID)` (chats.rs:808-813).
- **Trigger:** `tele chat join --chat https://t.me/+<hash>` or `https://t.me/joinchat/<hash>` — i.e. *every* private-chat join, the primary purpose of the command.
- **Impact:** private-chat joins are 100% broken; the username/public path (chat.rs:170-178) is the only working path.
- **Fix:** pass the full URL: `client.accept_invite_link(&target).await`, or invoke `tl::functions::messages::ImportChatInvite { hash: link }` directly with the extracted hash.

### 10.2 HIGH — `chat create --kind group` prints a `chat_id` that `--chat <id>` cannot resolve
- **Locations:** `src/commands/chat.rs:675` + `src/entities.rs:120-130` (`cached_ref`) and `139-158` (`checked_fallback_ref`)
- **What:** a basic group's `Chat` id is positive (e.g. `123`); `create` prints `{"chat_id":123}` and caches it as `PeerKind::Chat`. But `resolve_peer(123)` probes only `PeerId::user(123)` and `PeerId::channel(123)` — it never probes `chat(123)`. Fallback builds `InputPeerUser{access_hash:0}` → `users.getUsers` returns nothing → `InvocationError::Dropped` → misleading "request error: dropped (cancelled)".
- **Trigger:** immediately after `tele chat create --kind group`, run `tele chat participants --chat 123` (the printed id). Only the negated form `-123` works. Contradicts the shipped claim in `docs/capabilities.md:118` ("chat create caches the created chat's access_hash … so `--chat <id>` works immediately after") — works for supergroups/channels, broken specifically for basic groups.
- **Fix:** in `cached_ref`'s positive branch, also probe `PeerId::chat(raw)`, or have `create` emit the negative dialog id for basic groups.

### 10.3 MEDIUM — invite-link forms only parse as full `https://` URLs; all bare forms misclassify into misleading errors
- **Locations:** `src/commands/chat.rs:163` (only entry point) + `src/entities.rs:80-102` (`classify_target`), `104-112` (`is_link`), `160-178` (`parse_username`); grammers `chats.rs:741-766`
- **What:** `parse_invite_link` requires `url::Url::parse` to succeed with a t.me host. Bare `t.me/+hash` / `t.me/joinchat/hash` (extremely common paste) have no scheme/host → `None`. Fallthrough: `t.me/+hash` → `Target::Link("+hash")` → `USERNAME_NOT_FOUND`; bare `+hash` → `Target::Phone("")` → `INVALID_PHONE`; bare numeric hash → bogus peer-id resolution.
- **Fix:** normalize bare `t.me/...` to `https://t.me/...` before `parse_invite_link`; give invite-link-shaped inputs a clear "invite link must be a full URL" message.

### 10.4 MEDIUM — `join` discards the returned peer; joined chat's access_hash is never cached
- **Locations:** `src/commands/chat.rs:164-178` (both branches `.map_err(...)?;` drop the `Option<Peer>` result); contrast `create` which calls `cache_created_chat` (chat.rs:676, 697, 718, 741-750)
- **What:** `accept_invite_link`/`join_chat` return the joined chat (grammers `updates_to_chat`, chats.rs:343-360), but the code ignores it. The peer is not written to the session, and RPC-response updates are not auto-dispatched to the update cache. Follow-up id-based commands (`participants --chat <id>`, `leave --chat <id>`) fail with `PEER_NOT_CACHED`/`Dropped` until the session otherwise learns the peer.
- **Fix:** capture the returned peer and `entities::cache_chat(guard.session, ...)` like `create` does.

### 10.5 MEDIUM-LOW — `join` has no `ensure_chat_peer`; joining a user/bot peer surfaces a raw server error
- **Locations:** `src/commands/chat.rs:170-178` (no `ensure_chat_peer`; `kick`/`participants` have it at chat.rs:342, 395)
- `join --chat @some_person` resolves to `Peer::User`, then `join_chat` → `channels.joinChannel` with an `InputPeerUser` → raw `CHANNEL_INVALID` (400). `leave`/`invite` handle this case with a clean `Usage` error (chat.rs:236-240, 310-314); `join` doesn't.
- **Fix:** reject `Peer::User` before `join_chat` with a Usage message.

### 10.6 LOW — re-joining an already-joined chat errors instead of being idempotent
- **Location:** `src/commands/chat.rs:176` (`join_chat`); `channels.joinChannel` returns `CHANNEL_ALREADY_JOINED`/`USER_ALREADY_PARTICIPANT`
- With the fan-out design (re-running joins across many accounts) this produces spurious per-account failures.
- **Fix:** treat already-joined as an idempotent success.

### 10.7 LOW — `create --kind group` silently drops `--description` and `--forum`
- **Locations:** `src/commands/chat.rs:663-678` (group arm never reads `description`/`forum`); `--forum` also hard-ignored for `channel` (chat.rs:707). Pre-documented as INT-10 for `--forum`.
- **Fix:** reject in `validate_create` (chat.rs:636-643) or apply `description` via `messages.editChatAbout`.

### 10.8 LOW — help text advertises "invite link" as a valid `--chat` form for leave/invite/participants/kick, but none can resolve one
- **Locations:** `src/commands/chat.rs:27-33, 39-46, 51-55` (help strings) vs `entities.rs:80-178` (no invite-link handling outside `join`)
- `chat participants --chat https://t.me/+hash` → `USERNAME_NOT_FOUND`.
- **Fix:** narrow help text to `join`, or route invite links through `messages.checkChatInvite`+peer extraction.

### 10.9 LOW — basic-group `participants` can panic via grammers `take_user().unwrap()`
- **Locations:** `src/commands/chat.rs:344-358` (iterates `iter_participants` on `PeerKind::Chat`) → grammers `peer/participant.rs:216-219` (`peers.take_user(p.user_id).unwrap()`)
- **What:** `messages.GetFullChat` returns `full.users`; if a participant's `User` is missing from that vector (Telegram does omit members in edge cases — see the same crate's own note at chats.rs:158-163), `unwrap()` panics. The executor converts it to `"account task panicked"` (executor.rs:105).
- **Fix:** pre-check membership of each participant's user in the returned `users` before relying on grammers' iterator (or iterate `full.users` directly for basic groups).

### 10.10 LOW — `create` may emit `chat_id: 0`
- **Locations:** `src/commands/chat.rs:675` + `created_chat` at `734-739`
- `created_chat` returns `None` unless the response is `Updates::Updates`/`Combined` with a `chats` vector. If the server answers `updateShort` (rare but legal for `messages.createChat`/`channels.createChannel`), `chat_id` prints `0`.
- **Fix:** fall back to a second fetch (`messages.getChats`/`channels.getChannels` by id) when `chats` is empty.

### 10.11 LOW — human-mode `participants` table printed inside the per-account parallel handler
- **Location:** `src/commands/chat.rs:360-370`
- Tables interleave on stdout unlabeled by account with `--parallel 2-3`/multiple accounts. Pre-documented as ARCH-14.
- **Fix:** collect rows and print once outside the loop, or tag by account.

### 10.12 LOW — `chat.invite` implements "add user to chat", not "export/edit invites" as the spec & matrix claim
- **Locations:** `src/commands/chat.rs:249-321` vs `docs/capabilities.md:54` ("Export / edit invites") and `docs/ideas/tele-cli.md:64`
- No command path exports an invite link; only `tele raw messages.ExportChatInvite` covers the matrix intent.
- **Fix:** add an `invite-link` subcommand or a `--link` flag to `chat invite`.

### Verified clean (slice 10)
FloodWait/SlowModeWait routing through `tele_invocation`; AutoSleep ≤ `flood_sleep_threshold`; above-threshold `FLOOD_WAIT` carries `seconds`. Client disconnect on error via `ClientGuard::drop` on every path. Phone-before-numeric in both `--chat` and `--user` resolution. `leave` peer dispatch: channel/supergroup → `channels.leaveChannel`; basic group → `messages.deleteChatUser` self with `revoke_history:false`; user peer → clean Usage error. `invite` dispatch: supergroup/channel → `channels.inviteToChannel`; basic group → `messages.addChatUser` with `fwd_limit:0`; user peer → clean Usage error. `participants` termination: loop bounded by `count < limit` and `None` break; no infinite-pagination or cursor bug; `limit=0` returns `[]`; `limit>10000` rejected as Usage before connect. Participant role mapping matches the `Role` enum exactly. No unwrap/expect on RPC in the slice handlers (the only reachable panic is grammers-internal, finding 10.9).

---

## Slice 11 — `chat` admin (kick / admin / admin-log / stats) (`src/commands/chat.rs`)

### F1 MEDIUM — `admin-log` silently truncates — single non-paginated `channels.getAdminLog` call can never honor `--limit`
- **Location:** `src/commands/chat.rs:523-535`
- **What:** `validate_limit` accepts up to 10,000, and the handler makes exactly **one** `channels.getAdminLog` request passing `limit: limit as i32` with `max_id: 0, min_id: 0`. The server's effective per-call page size is ~100 events (pyrogram pages `GetAdminLog` in chunks of `min(100, total)`; the bot-API equivalent hard-caps at 100). No reference implementation passes a single large limit.
- **Impact:** `tele chat admin-log --limit 500` returns at most ~100 events, exits 0, prints `"events": [...]` with no warning — silent data loss in the machine API. Default 20 is unaffected.
- **Fix:** loop: request min(100, remaining), set `max_id = last_event.id` per page, stop at `limit` or empty page.

### F2 MEDIUM — `kick` on an already-banned member silently **unbans** them
- **Location:** `src/commands/chat.rs:403-407` → `kick_participant` in `grammers-client-0.10.0/src/client/chats.rs:473-498`
- **What:** for channels, grammers' kick = step 1 `EditBanned(view_messages taken away, until_date = now+60s)` (KICK_BAN_DURATION=60) — this **replaces** the target's banned rights wholesale — then step 2 `EditBanned(all-false rights, until_date 0)` = unban. For a member with an existing permanent ban, step 1 downgrades the ban to 60s and step 2 removes it entirely. The CLI then reports `"kicked": true`, exit 0. (Same flow on a non-member/left user is a silent no-op reported as success.)
- **Fix:** before kicking, `channels.GetParticipant`; if `Banned`/`Left`, error out (or explicitly re-apply a permanent ban instead of unbanning).

### F3 MEDIUM — `admin --promote` grants an incomplete admin rights set
- **Location:** `src/commands/chat.rs:464-475`
- **What:** promote sets 9 flags: change_info, post_messages, edit_messages, delete_messages, ban_users, invite_users, pin_messages, add_admins, manage_call. It never sets `manage_topics`, `post_stories`, `edit_stories`, `delete_stories`, `manage_direct_messages`, `manage_ranks`. In a forum supergroup the promoted admin **cannot create/close/pin/reorder topics**; in channels with stories they cannot manage stories. Help text says "grant admin rights".
- **Fix:** set every right flag to `true` on promote (optionally gate `anonymous`/`manage_call` behind flags).

### F4 MEDIUM-LOW — `admin-log` drops the acting admin's `user_id` and the `users`/`chats` vectors
- **Location:** `src/commands/chat.rs:538-548`
- **What:** the TL event is `channelAdminLogEvent id:long date:int user_id:long action:...` (grammers-tl-types api.tl:1118) and the response carries `chats`/`users`. The handler emits only `{id, date, action}` and discards `user_id` plus both resolution vectors. Machine-API consumers cannot attribute any action to an admin.
- **Fix:** add `"user_id": event.user_id`; optionally emit actor name via the response's `users`.

### F5 LOW — `admin_action_summary` collapses ~35 of ~52 action variants to `"other"` — including forum & rank events this CLI itself produces
- **Location:** `src/commands/chat.rs:783-853` (catch-all at :851)
- **What:** unhandled variants include `ToggleForum`, `CreateTopic`, `EditTopic`, `DeleteTopic`, `PinTopic`, `ParticipantEditRank`, `ChangeHistoryTTL`, `StopPoll`, `ToggleSlowMode`, `DefaultBannedRights`, `ChangeAvailableReactions`, `ToggleNoForwards`, `ExportedInviteDelete/Revoke/Edit`, `ParticipantVolume`, etc. (api.tl:1079-1116). `tele topic create` and `tele chat admin --title` produce exactly these event classes.
- **Fix:** add summary arms for the forum/rank/settings families.

### F6 LOW — `stats` JSON: epoch-int period dates + undocumented keys + count-only top lists
- **Locations:** `src/commands/helpers.rs:11-17` (`stats_period` → raw `min_date`/`max_date` epochs), `src/commands/chat.rs:599-627`
- `admin-log` emits RFC3339 while `stats` emits raw unix ints for the same period concept; `stats` key shapes (`period`, `followers`, `views_per_post`, …) are documented nowhere despite cli-contract.md:70 requiring new keys to be documented; and `top_posters.len()`, `top_admins.len()`, `top_inviters.len()`, `recent_posts_interactions.len()` (chat.rs:606,624-626) drop every actual value — consumers get counts only.
- **Fix:** convert period dates to RFC3339, document keys, emit the full aggregator objects (additively).

### F7 LOW — `ensure_chat_peer` inconsistency + usage-class errors exiting 3
- **Locations:** `src/commands/chat.rs:395` (kick checks), `:453-462` admin, `:520-522` admin-log, `:586-588` stats (no checks); `ensure_chat_peer` itself at `:774-781`
- `kick --chat me` → clean "kick requires a chat, got a user"; `admin --chat me` → `PEER_ID_INVALID` RPC; `admin-log`/`stats --chat <basic group>` → `CHAT_NOT_CHANNEL`. The clean path raises `TeleError::Other` (chat.rs:776) → exit 3, and the unclean paths surface cryptic `rpc error 400:` messages.
- **Fix:** add the same `ensure_chat_peer` gate to admin/admin-log/stats; raise `TeleError::Usage` for non-chat targets.

### F8 LOW — `kick`/`admin` feature gaps vs. capabilities matrix
- **Locations:** `docs/capabilities.md:56-57` vs `src/commands/chat.rs:378-413`
- Only `kick_participant` is exposed: basic-group kicks use `DeleteChatUser` with hardcoded `revoke_history: false` — no `--delete-history`, no ban/unban command, so the matrix's `set_banned_rights` path is unreachable. `admin --title` on a basic group is silently ignored (`EditChatAdmin` has no rank field).
- **Fix:** add `--revoke-history` or trim the matrix rows; warn on `--title` for basic groups.

### Verified clean (slice 11)
Kick ≠ permanent ban for normal members: grammers' `kick_participant` is ban(60s)+unban — correct kick semantics for members in good standing. Demote: builder defaults all-false → correct; rank cleared on demote. admin-log date handling (`from_timestamp` i32→RFC3339 UTC, no TZ bug); empty log → empty rows + exit 0; single-variant `ChannelAdminLogEvent::Event` destructure is exhaustive-safe; no pagination race (only one call). stats math: no local arithmetic — values are server-precomputed with correct field mapping; no division-by-zero; `--broadcast` correctly selects `GetBroadcastStats` vs `GetMegagroupStats`. Dry-run short-circuits before connect in all four handlers; `--promote`/`--demote` mutual exclusion enforced; FloodWait honored; no unwrap/expect on RPC results.

---

## Slice 12 — Streaming listener + takeout export (`src/commands/listen.rs`, `src/commands/takeout.rs`)

### 12.1 HIGH — Takeout export never wraps a single request in `invokeWithTakeout` — the takeout session is decorative
- **Location:** `src/commands/takeout.rs:169-236` (GetContacts at 171, `iter_dialogs`→`messages.getDialogs` at 203-204, `iter_messages`→`messages.getHistory` at 217-220); session id discarded at `takeout.rs:79-85`
- **What:** `start` calls `account.initTakeoutSession` and prints `takeout_id`, but `export` never uses it. `run_export` invokes all requests plain (grammers friendly iterators call `client.invoke` with bare `GetDialogs`/`GetHistory`). Per core.telegram.org/api/takeout: *"each query must be wrapped using invokeWithTakeout, with the id returned by account.initTakeoutSession"* — `tl::functions::InvokeWithTakeout { takeout_id, query }` exists in grammers-tl-types 0.10 (verified in generated_functions.rs). The `takeout_id` is also never persisted, so `export` couldn't wrap even if it wanted to.
- **Impact:** the entire purpose of a takeout session (relaxed rate limits, split-range pagination) is silently unused; on a large account the plain export hits `FLOOD_WAIT` and fails outright — precisely the failure takeout exists to avoid.
- **Fix:** persist `takeout_id` (e.g. `export/{name}/takeout.json`) in `start`; have `export` wrap `GetContacts`/`GetDialogs`/`GetHistory` in `InvokeWithTakeout` and follow the split-range procedure — or drop the takeout command group until it actually uses the session.

### 12.2 MEDIUM — `--chat` filter is bypassed for `Raw` events
- **Location:** `src/commands/listen.rs:226-235` (the `_ =>` arm), contrast 177-181/195-199/213-217
- **What:** the parsed branches check `resolved`, the Raw branch never does. `tele listen --chat @foo --events Raw` emits base64 updates from **every** chat on every selected account.
- **Fix:** gate the `_` arm on `events` (already done) and, when `resolved` is set, drop Raw rows whose raw update carries a peer that doesn't match (or document that Raw ignores `--chat`).

### 12.3 MEDIUM — Panic on `EditChannelMessage` carrying `MessageEmpty` kills the account stream
- **Locations:** `src/commands/listen.rs:196, 204` (`m.peer_id()` in both filter and row), via grammers `client/update/update.rs:107-111` → `message/message.rs:243-248` (`expect("empty messages from updates should contain peer_id")`)
- **What:** grammers maps `updateEditChannelMessage` to `Update::MessageEdited` **without** the empty-message guard it applies to the other three variants. If the raw message is `messages.messageEmpty` with `peer_id: None`, `Message::peer_id()` panics (updates pass `fetched_in: None`, update.rs:96-98). A panic aborts the whole per-account task — it is not an `Err`, so there is no per-event recovery and no reconnect; the account exits 3 and the remaining stream is gone. (NewMessage is safe — grammers routes peer-less messages to `Update::Raw`.)
- **Fix:** use the non-panicking accessors (`m.peer()` / `m.sender_id()`) and skip the row when absent.

### 12.4 MEDIUM — Burst event loss and unbounded buffering (no backpressure)
- **Locations:** `src/commands/listen.rs:115` (`UpdatesConfiguration::default()`), `src/client.rs:9-14` (unbounded channel), grammers `client/updates.rs:260-283` (`update_queue_limit` drop path)
- **What:** the default `update_queue_limit = Some(100)` truncates any batch that would overflow the stream's internal buffer — excess updates are **dropped**, with only a `TELE_LOG`-gated stderr warn. A burst (e.g. a busy channel diff arriving as one `UpdatesLike` item) silently loses events beyond 100. Meanwhile the runner→stream channel is `mpsc::unbounded` — a slow stdout consumer (piped to a slow agent) accumulates memory with no bound.
- **Fix:** raise `update_queue_limit` (e.g. `Some(4096)`) and log drops loudly on stderr via `log_line`; document the bound.

### 12.5 MEDIUM — Export failures flattened: wrong exit codes, misleading "kept alive" message
- **Locations:** `src/commands/takeout.rs:152-154` (`map_err(|e| TeleError::Other(export_error_message(...)))`), message at 124-130
- **What:** every failure — 401 auth, `TAKEOUT_REQUIRED` (no session at all), `FLOOD_WAIT` seconds — is collapsed into `TeleError::Other` → exit 3 (auth should be exit 4), the flood `seconds` field is dropped from JSON, and the message claims "server-side takeout session kept alive for resume" even when no session exists or auth failed.
- **Fix:** map `tele_invocation(e)` first; only add the "kept alive" suffix when the failure is export-side.

### 12.6 MEDIUM — `--chat` + `MessageDeleted` silently emits nothing for basic groups / private chats
- **Locations:** `src/commands/listen.rs:213-217, 279-287` (`deleted_matches` returns false on `None`)
- **What:** `MessageDeletion::channel_id()` is `None` for `updateDeleteMessages` (non-channel deletes); `deleted_matches(None, target)` → false. With `--chat <group|user> --events MessageDeleted` the filter matches nothing, silently.
- **Fix:** protocol-limited (no peer in `DeleteMessages`) — warn once at startup ("delete events for non-channel chats can't be chat-filtered") or document in cli-contract.

### 12.7 LOW — `failures`/`backoff` never reset on a *successful* reconnect
- **Location:** `src/commands/listen.rs:140-171` (reset only at 170-171 inside `Ok(Ok(u))`)
- After a stream error → sleep → reconnect succeeds, `failures` is not reset. On a quiet account with a flapping connection, five error→reconnect cycles with no update delivered in between → permanent give-up ("5 consecutive times") and exit 3, even though every reconnect succeeded.
- **Fix:** reset `failures`/`backoff` after a successful `stream_updates()` (on reconnect, not only on received update).

### 12.8 LOW — Connect/authorize errors are outside the reconnect loop
- **Locations:** `src/commands/listen.rs:92-93, 111-118`
- Only `stream.next()` errors get the 5-attempt backoff. A single transient `ClientGuard::connect` / `authorize` / `stream_updates` failure at reconnect time propagates via `?` and kills the account permanently.
- **Fix:** wrap the whole connect+stream cycle in the retry loop.

### 12.9 LOW — `takeout finish` exits 0 when there is no active session
- **Location:** `src/commands/takeout.rs:268-275`
- `TAKEOUT_REQUIRED` maps to `finished:false` inside an *ok* outcome → exit 0. A script can't distinguish "takeout finished" from "nothing to finish".
- **Fix:** emit a non-ok outcome with exit 3 (or a documented distinct code).

### 12.10 LOW — "Resume" truncates prior partial export before re-exporting
- **Location:** `src/commands/takeout.rs:198-199` (`File::create` truncates `messages.jsonl`)
- Re-running `export` after a failure truncates the partial `messages.jsonl` and re-downloads from scratch; if the retry then fails early, the prior partial data is gone. No dedup/resume state.
- **Fix:** write to a temp file and rename on completion, or append with dedup by message id per dialog.

### 12.11 LOW — `export_dir` path escape via account name `..`
- **Locations:** `src/commands/takeout.rs:99-101` + `src/session.rs:9-21` (`validate_name` allows `"."`/`".."`)
- An account named `..` makes `export_dir("..")` = `<app_data>/export/..` = the app-data root; `contacts.json`/`messages.jsonl`/`dialogs.json` get written over the root directory. (Matches known REL-14.)
- **Fix:** reject `.`/`..` in `validate_name` (or canonicalize and verify the export dir is inside `app_data/export`).

### 12.12 LOW — `--photos`/`files` granted but export downloads no media; no progress output
- **Locations:** `src/commands/takeout.rs:74-76` (session flags), `run_export` (no downloads), no progress reporting anywhere (known M-U5)
- `files: photos, file_max_size: 5 GiB` promises file export; `run_export` writes metadata only. Long exports show zero progress; `--json` consumers get only the final envelope.
- **Fix:** download media under takeout or rename the flag; emit per-dialog progress rows.

### 12.13 LOW — `--raw` help text contradicts additive behavior
- **Location:** `src/commands/listen.rs:24` ("output raw TL updates **instead of** parsed events") vs behavior at 42-44 (additive push of "Raw"). Doc drift only (INT-05 from the prior review, still open).

### Verified clean (slice 12)
JSONL framing: one JSON object per line — `serde_json::to_string` escapes `\n`; `print_json_result` writes under a stdout lock and flushes per line; no interleaved/corrupted lines; no partial line on Ctrl+C. Ctrl+C/termination: main.rs select drops the command future → `JoinSet` abort → `ClientGuard::drop` → disconnect; exit 130; reconnect sleep capped at the remaining deadline. `MessageDeleted` ids: `d.messages()` raw `Vec<i32>`; `channel_id` matches channel-bare-ids correctly (kind-aware compare). Raw safety: unknown updates map to `Update::Raw`, emitted base64 — no unwrap on unknown types. Event-row contract matches cli-contract.md:79-97 (additive-only). Stderr discipline: all diagnostics via `log_line` to stderr; stdout carries only JSONL rows. Takeout message panic safety: `iter_messages` messages carry `fetched_in`, so `message_to_json`'s `peer_id()` cannot panic for history-fetched messages. Export dir permissions: `create_dir_private` → 0700 on Unix; no `media_name` used in any path. No mutex held across await in either module.

### Not verified (would need live testing)
Actual frequency of `MessageEmpty` in `updateEditChannelMessage`; server-side takeout-session expiry duration (at minimum a leaked session blocks a fresh `takeout start` and keeps the account in takeout mode until server expiry).

---

## Slice 13 — Raw TL registry + shell completions (`src/commands/raw.rs`, `src/commands/completions.rs`)

### 13.1 HIGH — `tele raw` prints nothing to stdout in human mode; results silently vanish
- **Locations:** `src/commands/raw.rs:27-60` (no human-mode output anywhere in `run`); gate at `src/executor.rs:166-172` (`print_envelope` only emits when `flags.json || flags.jsonl`)
- **What:** `raw::run` builds the envelope via `run_fanout` and calls `executor::finish` — the only stdout path. In human mode `finish`/`print_envelope` prints nothing. So `tele raw messages.ExportChatInvite --args '{"chat":"@x"}'` creates a real invite and exits 0 with **zero stdout**. Same for `contacts.Search`, `stats.GetBroadcastStats`, `messages.GetAllDrafts`. Every sibling read command prints a human table, and `docs/cli-contract.md:77` promises "Human mode (no `--json`): Rich tables on stdout." The offline contract test only exercises the `--json` + `--dry-run` path (tests/contract.rs:750-792), so the defect is untested and shipping.
- **Fix:** print a table (invite link / found peers / stats rows / update summaries) in the closure when `!machine_mode(json, jsonl)`; add an offline contract test asserting human-mode stdout is non-empty.

### 13.2 MEDIUM — "Destructive raw calls still require `--account`" is not enforced (contract vs code)
- **Locations:** `docs/cli-contract.md:113` vs `src/executor.rs:160-162` + `src/commands/raw.rs:39`
- **What:** `raw::run` calls `run_fanout` with no account gate; when `--account`/`--tag` are absent, `select_from` defaults to **all sessions** (executor.rs:160-162). The registry's state-changing entries — `account.UpdateProfile` (raw.rs:301) and `messages.ExportChatInvite` (raw.rs:176) — therefore execute on every logged-in account.
- **Fix:** enforce `--account`/`--tag` for mutating registry entries (add a `mutating: bool` flag), or amend the contract to document the all-sessions default.

### 13.3 MEDIUM — Unreachable dispatch fallback fabricates a fake Telegram RPC error
- **Location:** `src/commands/raw.rs:320-327`
- **What:** the `_ =>` arm returns `InvocationError::Rpc(RpcError { code: 400, name: "RAW_NOT_REGISTERED", .. })`, which `tele_invocation` turns into `"rpc error 400: RAW_NOT_REGISTERED"` (exit 3). Today unreachable (all 6 `REGISTERED` names have arms, enforced by tests/contract.rs:754-759), so the failure mode is *future drift*: any edit that adds a `REGISTERED` entry without an arm surfaces a message that looks like a Telegram server error.
- **Fix:** replace the arm with a `TeleError::Usage("raw method not in registry …")` (matching the pre-fanout error at raw.rs:33-35).

### 13.4 LOW — Empty-string required fields pass validation → wrong error class (exit 3, not 1)
- **Locations:** `src/commands/raw.rs:63-70` (`req_str`) → `src/entities.rs:98-99` (`Target::Invalid`) → `src/entities.rs:66` (`INVALID_PEER_ID`)
- **What:** `req_str` accepts `""` (it is a string). `tele raw messages.ExportChatInvite --args '{"chat":""}'` (or `stats.Get*` with `"channel":""`) passes `validate_params`, then `resolve_peer("")` returns `Target::Invalid` → fabricated `RpcError(400, "INVALID_PEER_ID")` → exit 3 with `InvocationError`, where the sibling commands treat this as a Usage error (exit 1).
- **Fix:** treat empty strings as missing in `req_str`, or classify `Target::Invalid` before dispatch as `TeleError::Usage("invalid --args chat/channel")`.

### 13.5 LOW — `account.UpdateProfile --args '{}'` is a silent no-op success
- **Locations:** `src/commands/raw.rs:136-142` (no required key for `account.UpdateProfile`) + `src/commands/raw.rs:301-308`
- **What:** all three fields are optional; `account.updateProfile` with all flags unset returns the current user unchanged. `tele raw account.UpdateProfile --args '{}'` exits 0, doing nothing, indistinguishable from a successful update; user typos (wrong key names) that reduce to no-op are never surfaced.
- **Fix:** require at least one of `first_name`/`last_name`/`about` in `validate_params` for this name (Usage error otherwise).

### 13.6 LOW — `tele raw` registry not discoverable: not in `--help`, not completable, path leak in error
- **Locations:** `src/commands/raw.rs:12-13` (RawArgs help shows 2 examples), `src/commands/raw.rs:34` (error leaks `src/commands/raw.rs`), `docs/cli-contract.md:105-108` (shows only 3 of 6 names)
- `tele raw --help` lists neither the 6 registered names nor the `--args` key shapes; `RawArgs.name` has no `value_parser`/possible-values so TAB completion offers nothing; the unknown-method error message embeds the internal repo path. (Both M-U6 and M-U9 from docs/audit-2026-08.md remain open.)
- **Fix:** enumerate `REGISTERED` in the `--help` text (or a `value_parser`), and reword the error without the path.

### 13.7 LOW — `tele completions` exists but is undocumented in the CLI contract and spec
- **Locations:** `src/main.rs:102-104` (wired), `src/commands/completions.rs:6-16`; absent from `docs/spec.md:70-82`, `docs/cli-contract.md`, `docs/capabilities.md`, and `tests/contract.rs` (no test asserts `tele completions *` exits 0 or emits a script)
- The command works, but no doc or offline test pins its existence; the only reference is the audit's past "H11" suggestion, whose proposed shape was `tele completions --shell bash|zsh|fish` — the shipped shape is a positional subcommand, so the audit trail doesn't match the implementation either.
- **Fix:** add a `tele completions` row to the contract docs and an offline test (e.g. `completions bash` output contains `_tele`/`tele` and subcommand names).

### Verified clean (slice 13)
Registry signatures: all 6 entries construct their `tl::functions::*` structs with exactly the generated fields/types — verified against `generated_functions.rs` (`ExportChatInvite` incl. `subscription_pricing`, `Search`, `GetBroadcastStats`, `GetMegagroupStats`, `UpdateProfile`, `GetAllDrafts`); no wrong-arg-count/type mismatches. Result deserialization: every enum variant/field used in output matches generated code; registry names are the TL camelCase names, no case mismatch. i32 truncation (audit M-B3) fully mitigated — every `as i32` cast in `int_field`/`opt_int_field` (raw.rs:409-421) is pre-validated by `i32::try_from` in `opt_i32` (raw.rs:81-90); no truncation path remains. Arg parsing: non-object `--args`, unknown keys, wrong JSON types, bad JSON syntax, bool-as-string, missing required keys all fail cleanly with exit 1 and specific messages (unit tests raw.rs:432-544 + contract tests). Security: registry contains no destructive function (`account.deleteAccount`, `messages.deleteMessages`, reset/logout all absent); no TL-field injection possible (typed structs + key allowlist); `--dry-run` short-circuits before connect and still runs validation; peer resolution honors phone-before-numeric. Completions: `clap_complete::generate` used with the correct 4.6.9 signature; all 4 contract shells wired to the right `Shell` variants; output to stdout; tree generated from `Cli::command()` (main.rs:154-156) so completions are in sync with the derive tree **by construction** — no hardcoded subcommand lists to drift. (Note: dry-run envelope shape `{"dry_run":true,"method":...}` is what the contract tests lock in; it diverges from cli-contract.md:75's generic `data.would` wording — see slice 6 F1.)

---

## Slice 14 — Dialogs, topics, and shared helpers (`src/commands/dialog.rs`, `src/commands/topic.rs`, `src/commands/helpers.rs`)

### 14.1 HIGH — `dialog list --folder 0` returns an empty list (main-folder listing is broken)
- **Locations:** `src/commands/dialog.rs:88-92` (folder filter), compared against `d.folder_id` read at dialog.rs:84
- **What:** the filter is `if dialog_folder != Some(f) { continue; }`. In TL, `dialog.folder_id` is `flags.4?int` (verified grammers-tl-types-0.10.0/tl/api.tl:214): the flag is **absent (`None`) for every dialog in the main folder** and only set (to `1`, archive) for archived dialogs. So with `--folder 0` (documented as "0=main" at dialog.rs:24), every dialog is `None != Some(0)` → all skipped. The command connects, iterates the entire dialog list, and emits `{"dialogs": []}` with **exit code 0** — silent wrong output, no error.
- **Fix:** compare with the default: `if dialog_folder.unwrap_or(0) != f { continue; }`.

### 14.2 HIGH — `dialog list` emits phantom/duplicate rows for `dialogFolder` entries
- **Locations:** `src/commands/dialog.rs:75-92` + grammers `src/peer/dialog.rs:41-49`
- **What:** on any account with the folders (archive) feature enabled, `messages.getDialogs` responses include a `dialogFolder` entry (api.tl:215). `Dialog::Folder(_)` is mapped to `(0, "", None)` and **pushed as a normal row** — `peer_key(&dialog.peer)` is the *last real peer of the folder* (the TL constructor carries a real `peer`), so the row is a bogus duplicate of a real chat's key with `unread: 0`, `draft: ""`, `last_message: ""`. Only the `--folder` filter accidentally hides it (14.1's `None != Some(f)` path). Secondary hazard: grammers `Dialog::new` does `peers.get(peer_id).expect("dialogs use an unknown peer")` (peer/dialog.rs:47-48) — if the server ever omits that peer from `users/chats`, the command panics and surfaces as `account task panicked` (exit 3) via `collect_one` (executor.rs:105).
- **Fix:** skip `tl::enums::Dialog::Folder(_)` rows entirely in the list loop — or render them as an explicit folder marker, never as a chat row.

### 14.3 MEDIUM — `dialog drafts` emits channel ids in a different convention than every other command (doesn't round-trip)
- **Location:** `src/commands/dialog.rs:159-163`
- **What:** drafts emits `Peer::Channel(p) => -p.channel_id` (plain negative, e.g. `-1234567890`). The rest of the machine API uses `PeerId::bare_id()` — `serialize.rs:51` (`peer_key`), `listen.rs:186,204` — which for channels is `-id - 1000000000000` (e.g. `-1001234567890`, verified grammers-session peer.rs:266, 276-278). `PeerId::kind()` classifies a plain negative id like `-1234567890` as **Chat** (peer.rs:227-228), so feeding a drafts `id` back into `--chat` probes the Chat kind first and only resolves the channel correctly if it happens to be in the session cache. Uncached → wrong-kind fallback → wrong peer / RPC failure. The two commands in the same group disagree about the identity of the same channel.
- **Fix:** emit the id via the same `bare_id()`-based `peer_key` used by `dialog list` (ideally the full `{"id","kind","name"}` shape, which also closes audit M-U13 — drafts currently carries no peer kind).

### 14.4 MEDIUM — `topic list` fetches a single page and passes `--limit` (>100) raw to the API
- **Location:** `src/commands/topic.rs:110-121`
- **What:** one `messages.GetForumTopics` call with the user's `limit` (`limit as i32`, up to 10,000 via `validate_limit`). `getForumTopics` caps at 100 topics per request; there is no pagination loop (the 0.10 TL `messages.forumTopics` has no `next_offset_*` fields — api.tl:1649 — pagination must re-call with the last topic's `date`/`id`). With default `--limit 20` and a forum of 50 topics, 30 topics are silently dropped; with `--limit 1000` the request likely errors or is capped server-side.
- **Fix:** clamp the request limit to 100 and loop with `offset_date`/`offset_id`/`offset_topic` set from the last received topic until `limit` rows are collected or the page shrinks.

### 14.5 MEDIUM — `topic create --emoji` sends a packed codepoint where the server requires a custom-emoji document id
- **Locations:** `src/commands/topic.rs:178-180` (packing), documented as broken in `docs/capabilities.md:64` and audit M7 (`docs/review-2026-08.md:61-62`)
- **What:** `icon_emoji_id` = the 4 UTF-8 bytes of the emoji as an i64. Telegram expects a custom-emoji **document id** (~1e18); the server rejects/ignores the value — the shipped `done` capability cannot work when `--emoji` is used (worst case the whole `CreateForumTopic` request fails, not just the icon).
- **Fix:** implement `messages.searchCustomEmoji` document-id lookup, or reject `--emoji` with a clear "not supported" Usage error until then.

### 14.6 LOW — `validate_emoji` rejects valid 3-byte emoji with a false error message
- **Locations:** `src/commands/topic.rs:165-171`
- **What:** `is_emoji_codepoint` accepts `0x2300..=0x23FF`, `0x2600..=0x27BF`, `0x2B00..=0x2BFF` (topic.rs:183-188) — but every codepoint in those ranges is 2-3 UTF-8 bytes, so the `bytes.len() != 4` gate at topic.rs:166 rejects them all. E.g. `--emoji "☀"` (U+2600, single codepoint, in range) errors with "must be a single-codepoint emoji (4 UTF-8 bytes)" — a false claim about the input. The accepted ranges are dead code except for producing misleading errors.
- **Fix:** drop the byte-length check (validate codepoint count + range only), or narrow the ranges to 4-byte emoji and reword the message.

### 14.7 LOW — `topic create` discards the created topic id from the Updates response
- **Location:** `src/commands/topic.rs:65-77` (`let _: tl::enums::Updates = ...`), acknowledged as INT-12 in `docs/review-2026-08.md:110`
- The response contains the new `ForumTopic` (id, date); it is thrown away. Consumers must re-list topics to learn the id.
- **Fix:** extract the `updateNewForumTopic`/`ForumTopic` from the Updates and add `"topic_id"` to the JSON.

### 14.8 LOW — `dialog drafts --folder N` is accepted and silently ignored
- **Location:** `src/commands/dialog.rs:134-140` (`Drafts(ListArgs)` reuses `ListArgs`, `folder` is never read)
- **Fix:** use a dedicated `DraftsArgs` without `folder`, or error on `--folder` with `dialog drafts`.

### 14.9 LOW — `raw.rs` duplicates the four helpers verbatim (audit M-P1)
- **Locations:** `src/commands/raw.rs:338-344, 378-400` vs `src/commands/helpers.rs:3-33`
- Identical logic, no behavioral divergence today — but a drift hazard: a future fix to one copy (e.g. finding 14.3's convention) would silently miss the other.
- **Fix:** import from `helpers.rs` in raw.rs.

### helpers.rs contract verification (per the slice brief)
- **Precedence chain:** the phone-before-numeric rule lives in `entities::classify_target` (`src/entities.rs:80-102`), not `helpers::peer_id`. Verified: `+`-branch runs before `parse::<i64>()`; covered by tests `classify_phone_precedes_numeric_parse` (entities.rs:590-596) and friends. **Every** user-input `--chat` caller funnels through `entities::resolve_peer` (30+ call sites grep-confirmed) — no command parses peers itself. `helpers::peer_id` is a pure `tl::enums::Peer → i64` converter, correctly used by its only two callers (chat.rs:758-759, banned/left participants), consistent with the other `participant_user_id` arms (chat.rs:752-761). No precedence bug exists.
- **stats_\*** are passthrough serializers of server fields — no date math, no timezone/DST/month-boundary logic, no division (percent is `part`/`total` integers, not computed). Verified clean in chat.rs:600-627 and raw.rs copies.

### Verified clean (slice 14)
dialog list pagination: grammers `DialogIter` terminates via `last_chunk` when a chunk < 100 arrives; `next_raw` stops on empty buffer + last_chunk; the `while count < limit` loop can't loop infinitely; no off-by-one; client-side folder filtering doesn't disturb server offsets. Ordering: pinned-first on the first chunk, then date-desc — matches Telegram's default. Draft semantics: empty `DraftMessageEmpty` → empty string matches "empty draft = deleted draft"; GetAllDrafts only returns non-empty drafts; no save/clear claim made. archive/unarchive: `folders.EditPeerFolders` with folder_id 1/0 is the correct API, idempotent, `"archive": !unarchive` echoes correctly; unarchive (folder_id=0) correctly clears the folder. delete: grammers `delete_dialog` = `channels.LeaveChannel` for channels (does **not** delete the channel), `DeleteChatUser` for groups, `DeleteHistory{revoke:false}` for users — safe semantics; `--dry-run` honored; errors mapped through `tele_invocation`. No `unwrap`/`expect` in the slice; exit codes and envelope flow consistent. Confidence caveat: findings 14.1-14.4 and 14.6-14.9 fully traced through vendored source; 14.2's panic branch depends on server response contents not observable offline.

---

## Slice 15 — Contacts, profile, privacy (`src/commands/contact.rs`, `src/commands/profile.rs`, `src/commands/privacy.rs`)

### H1 HIGH — `privacy set` never sends a base rule: silently destroys the previous rule set and cannot express "nobody"/"contacts only"
- **Locations:** `src/commands/privacy.rs:192-223` (rules construction), `:219-223` (SetPrivacy)
- **What:** `--allow @alice` produces `rules = [InputPrivacyValueAllowUsers([alice])]`; `--deny @bob` produces `[InputPrivacyValueDisallowUsers([bob])]`. `account.SetPrivacy` **replaces** the whole rule list for the key (core.telegram.org: rules vector is the new full set; Telethon's canonical pattern for "nobody except X" is `[DisallowAll, AllowUsers([X])]`). There is no base-rule flag at all.
- **When:** any `tele privacy set` on a key that already had a base rule (e.g. last-seen "my contacts" = `AllowContacts`), or `--allow` used with the natural intent "only these users".
- **Why wrong:** after `--deny @bob` on a "contacts only" key, the base rule is gone and the effective setting is "everyone except bob" (no matching rule → allowed) — a **silent privacy widening**. `--allow @alice` alone does not restrict anyone else; the command reports success `{"key","allow","deny"}` either way. "Nobody" / "my contacts" / "everyone" are unreachable. Also no handling of a user present in both `--allow` and `--deny` (first-match silently wins).
- **Fix:** read current rules first and re-apply the existing base rule (or add `--base none|contacts|everyone`, base rule first in the list); warn when the base rule would change.

### H2 HIGH — `privacy get` (and `set`) print nothing on stdout in human mode
- **Locations:** `src/commands/privacy.rs:102-147` (get), `:175-229` (set); contrast `contact.rs:93-104`, `profile.rs:115-118`
- **What:** the handler only returns the envelope; `executor::finish` → `print_envelope` (executor.rs:166-172) prints only under `--json/--jsonl`. Every other command group has a human table. `tele privacy get` exits 0 with zero output.
- **Why wrong:** a read command that silently outputs nothing; violates cli-contract.md:77 ("Human mode… Rich tables on stdout").
- **Fix:** print a `key | rules` table per account inside the closure, like contact.rs:93-104.

### M1 MEDIUM — `contact add` with only one of `--first`/`--last` silently discards it
- **Location:** `src/commands/contact.rs:134-143`
- **What:** `(Some(f), Some(l))` is the only arm that honors user input; any single-flag combo falls into `_` and recomputes both names from the peer's *current* name. `contact add --user @alice --first "Alice"` applies the peer's old names instead. If the peer has no name, both become `""` and AddContact may fail server-side.
- **Fix:** handle `(Some(f), None)` → `(f, "")` and `(None, Some(l))` → `("", l)`.

### M2 MEDIUM — phone-target flows silently mutate the contact list (ImportContacts side effect); failed `contact add --user +phone` can leave a nameless contact while reporting failure
- **Locations:** shared `src/entities.rs:18-38` (ImportContacts in resolve_peer), triggered by `contact.rs:129`, `profile.rs:62-64`, `privacy.rs:196-199, 208-212`
- **What:** per the AGENTS.md gotcha, `contacts.ImportContacts` adds the number to MY contact list; when the target's phone privacy blocks it, `users` is empty → `USER_NOT_FOUND` (handled gracefully, no panic — verified entities.rs:33-38). But for `contact add --user +phone` the import side effect precedes AddContact, so a reported failure may still have created a nameless contact entry; and `privacy set --deny +phone` / `profile get --chat +phone` / `contact block --user +phone` all mutate the contact list as a side effect of an unrelated operation.
- **Fix:** `contacts.ResolvePhone` for resolution (prior INT-07), keep ImportContacts only inside `contact add` (pass the `InputPhoneContact` directly).

### M3 MEDIUM — `profile set` multi-field update is non-atomic: name/bio committed before photo upload
- **Location:** `src/commands/profile.rs:170-228` (UpdateProfile at :181-189, photo steps at :191-227)
- **What:** `--name/--bio` are applied first; an invalid/non-image/oversized photo then fails, leaving the name/bio changed but the command exiting 3 with only a photo error and no rollback.
- **Impact:** partial application reported as failure (user can't tell the profile changed).
- **Fix:** validate the image (extension/magic/size) before UpdateProfile, or commit the photo first, or apply name/bio last.

### L1 LOW — `profile set --photo ""` isn't a usage error; photo deletion unsupported
- **Locations:** `profile.rs:133-135, 191-196`; `validate_upload_path("")` passes (empty basename not caught, msg.rs:258-274) → raw IO error (`TeleError::Other`, exit 3). No path calls `photos.DeletePhotos`.
- **Fix:** reject empty path; add `--delete-photo`.

### L2 LOW — `profile set` name/bio validation absent
- **Location:** `profile.rs:127-137, 171-189`
- `--name ""` / whitespace-only / >64-char names and >70-char bios go to the server as RPC errors (FIRSTNAME_INVALID etc.) instead of Usage errors.
- **Fix:** client-side non-empty + length caps.

### L3 LOW — privacy keys incomplete vs Telegram
- **Location:** `privacy.rs:51-79`
- `InputPrivacyKey` has 14 variants (verified generated_enums.rs:8475-8490); the CLI exposes 9 — notably missing `phone_p2p` (P2P calls, a real privacy key users expect), plus `birthday`, `saved_music`, `star_gifts_auto_save`, `no_paid_messages`.
- **Fix:** add `phone_p2p` (+ others) and docs.

### L4 LOW — `contact list` silently truncates at `--limit` and drops `User::Empty` entries without notice; no stable sort
- **Location:** `contact.rs:75-91` (`take(limit)`, `if let User::User`)
- **Fix:** report total vs shown; sort by id/name.

### L5 LOW — `profile get` completeness/consistency
- **Locations:** `profile.rs:23-24, 139-145`
- No `photo`/`status` despite `UserFull.profile_photo` being available; `--show-phone` also reveals *other* users' phones while its help says "the account phone number"; user-target JSON (`phone/bio/bot`) differs in shape from chat-target JSON (`kind` only).
- **Fix:** document/adjust; add fields additively.

### L6 LOW — mutating commands echo raw `--user`/`--allow` strings to stdout JSON
- **Locations:** `contact.rs:156,188,220`; `privacy.rs:224`
- `+phone` targets are echoed and no resolved user id is returned, so consumers can't verify which user was affected.
- **Fix:** include resolved id.

### Verified clean (slice 15)
No panic paths: every `unwrap`/irrefutable-let traced safe (contact.rs:139 `unwrap_or`; profile.rs:116 `as_object()` on a constructed object; profile.rs:209 single-variant `photos::Photo`; privacy.rs:131 single-variant `account::PrivacyRules` — both verified against generated_enums.rs). Block/unblock: `contacts.Block`/`contacts.Unblock` are the correct APIs for users (not `account.setPrivacy`), idempotent, non-user targets error cleanly; `my_stories_from: false` fine. ImportContacts empty response handled gracefully (USER_NOT_FOUND), no crash — only the side effect (M2) remains. Privacy rule summary: exhaustive match over all 12 `PrivacyRule` variants; rule `ids` are plain `Vec<i64>` — no access-hash leak. Profile partial updates preserve unspecified fields (`Option<String>`; `None` leaves name/bio untouched). Already-fixed from the 2026-08 review: `profile get` phone redaction + `--show-phone` (SEC-03), `profile set --photo` sensitive-file guard (ARCH-09). Exit codes / FloodWait: all three files route errors through `tele_invocation` (401→Auth, 420→`seconds`); contact-list phone display is documented-intended behavior.

---

## Cross-cutting dedup index

Findings reported independently by multiple slices (fix once, resolves all):

| Theme | Slices |
|-------|--------|
| Config/credential failures exit 3 instead of 1 (dead `Config→1` arm) | 1.2/1.3, 3.4, 5.4, 6.F2 |
| `account add` name validation; `all` reserved-word poisoning | 1.4, 3.6, 5.6, 7.1 |
| Human-mode output inside parallel closures (unlabeled/interleaved) | 1.6, 5.2, 10.11 |
| `print_json` EPIPE panic | 1.7, 5.7, 6.F6 |
| Non-atomic `write_config` | 3.3, 7.10 |
| Session-file lock not implemented | 2.3, 7.4 |
| `--dry-run` missing `data.would` | 6.F1, 8.L3, 13 (noted) |
| Numeric id resolution wrong-kind / `-100…` gap / basic-group `CHANNEL_INVALID` | 4.F1/F6, 8.H3/M3, 10.2, 14.3 |
| `+phone` ImportContacts side effect | 4.F3, 8.H4, 15.M2 |
| Zombie/partial session files on failed paths | 5.3, 7.6, 7.9 |
| Empty env var → CWD redirect (related: empty basename validation) | 3.1, 7.11-related |
| Windows permission hardening no-op | 2.5, 3.13 |
| Windows path-guard bypasses (trailing dot/space; case-sensitive compare) | 2.1, 9.2 |
| `TELE_PHONE` advertised but unimplemented | 3.11, 7.3 |
| `msg download` silent overwrite / cross-account collision / partial file | 5.1, 9.1, 9.3 |
| admin-log pagination / topic-list pagination (single-page truncation) | 11.F1, 14.4 |
| Help/docs drift (`adminlog`, `--to`, completions undocumented) | 1.9, 1.10, 8.L7, 13.7 |

---

## Method notes

- All 15 agents loaded the `rust-engineering` skill and followed its guidance; all were instructed to be strictly read-only (no builds, no tests, no edits).
- Findings were verified by tracing call paths into actual source, including vendored crate sources: `grammers-client-0.10.0`, `grammers-session-0.10.0`, `grammers-tl-types-0.10.0` (generated TL code under `target/debug/build/grammers-tl-types-*/out/generated_{functions,enums,types}.rs`), `grammers-mtsender-0.10.0`, `libsql-0.9.30`, `libsqlite3-sys`, `url-2.5.8`, `toml_edit-0.22.27`, `toml_write-0.1.2`, `clap_complete-4.6.9`.
- Prior review docs (`docs/review-2026-08.md`, `docs/audit-2026-08.md`) were cross-checked; only findings that still hold at HEAD are reported, and stale prior claims are corrected inline where found (e.g. M6 "rejected even when cached" — the cached `-100…` path works today).
- Items requiring live-network verification are explicitly marked; nothing was reported from guessing.
- Real phone numbers and hashes were scrubbed from all examples.