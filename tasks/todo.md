# Tasks

## Phase 0 — Scaffold

- [x] Task 1: Cargo skeleton
  - Acceptance: `cargo build`; `tele --help` lists groups; `cargo test` passes; clippy configured
  - Verify: `cargo run -- --help` && `cargo test` && `cargo clippy --all-targets -- -D warnings`

- [x] Task 2: Capability contract
  - Acceptance: test fails if a `want` row has no CLI command / raw registry entry; `--help` lists declared groups
  - Verify: `cargo test contract`
  - Files: `tests/contract.rs` (14 tests), `docs/capabilities.md` (read)

## Phase 1 — Kernel

- [x] Task 3: Config + env
  - Acceptance: loads API id/hash from `.env`; TOML accounts/tags/global+per-account proxy; missing secrets error clearly
  - Note: proxy config parsed (`src/config.rs`) but not wired to SenderPool yet → matrix `kernel.proxy` stays `want`

- [x] Task 4: Sessions + selection
  - Acceptance: `{app_dir}/sessions/{name}.session`; resolve `--account` / `--tag` / `all`; reject unknown names
  - Files: `src/session.rs`, `src/executor.rs` (select_accounts)

- [x] Task 5: Client + executor
  - Acceptance: builds client with flood 60 + AutoSleep retry; sequential default; `--parallel` clamped 1–3; never two clients on one session
  - Files: `src/client.rs`, `src/executor.rs`

- [x] Task 6: Output / global flags
  - Acceptance: `--json`, `--dry-run`, `-q`/`-v` on root; dry-run does not send
  - Files: `src/output.rs`, `src/serialize.rs`

## Phase 2 — Auth

- [x] Task 7: `tele account add|list|status`
- [x] Task 8: `tele account login` (code, 2FA, QR)
  - Matrix `auth.code` `auth.2fa` `auth.qr` → done. Verify live: code, 2FA, `--method qr`
- [x] Task 9: Logout vs local delete (`logout` sign_out + session delete; `remove` local-only)

## Phase 3 — Daily operators

- [x] Task 10: `tele msg send` + schedule + file (dry-run no send)
- [x] Task 11: `tele msg edit|delete|forward|pin|get|read|react|search`
- [x] Task 12: `tele chat join|leave|invite|participants|kick|admin|adminlog|stats|create`
- [x] Task 13: `tele dialog list` + `topic`

## Phase 4 — Listen + raw

- [x] Task 14: `tele listen`
  - `--events` allowlist (NewMessage default; MessageEdited, MessageDeleted, Raw) validated before connect (exit 1); `--chat` filter
  - Note: `--from`/`--pattern` filters and auto_reconnect deferred; matrix `listen.action|user|album` → later (no friendly UserStatus variant in grammers 0.10)
- [x] Task 15: `tele raw`
  - Positional registry name + `--args` JSON; unregistered name exits 1 before connect with `raw method not in registry; add an arm in src/commands/raw.rs`

## Phase 5 — Remaining want rows

- [x] contact: list/add/block/unblock
- [x] profile: get/set (name, bio, photo)
- [x] privacy: get/set rules
- [x] takeout: start/export/finish
- [ ] msg.poll — deferred to `later`: grammers 0.10 has no friendly poll (InputMessage has no `poll`); raw arm deferred
- [x] kernel.proxy — wired: `config::proxy_url_for` (per-account overrides global, socks5-only) → `SenderPool::with_configuration` with `ConnectionParams { proxy_url }` in `ClientGuard::connect` (41 call sites: hoisted `config_path` + inner closure clone); 5 unit tests
  - Note: proxy connection itself needs live verification through an actual socks5 proxy (tor 9050) — see checklist below

## Phase 6 — Last (do not start, ask first)

- [ ] MCP `tele mcp serve`
- [ ] Agent skill

## Manual live verification checklist (real sessions in %APPDATA%\telecli)

Verified 2026-08-13 by agent against real sessions 1 and 2 (personal +98 numbers):

- [x] `tele account status` / `list` against existing sessions — status `{"authorized":true}` exit 0
- [x] `tele msg send --chat me --text ...` then `get`/`edit`/`delete` — full roundtrip (msg 928) exit 0
- [x] `tele listen --events NewMessage` — full cross-account proof: `listen` on account 1 + `msg send --chat <account1-phone>` from account 2 → JSONL `{"event":"NewMessage","account":"1","chat_id":...,"id":...,"out":false,"text":"LISTEN-TEST ...","date":"..."}` received; self-sends don't emit updates; `--events Raw` rows carry base64 `raw` + `state` (`date`/`seq` plus `pts`/`qts`/`channel_id` per message-box variant)
- [ ] `tele chat participants` on a group; `adminlog` on a channel you admin — no group/channel on test accounts
- [x] `tele profile get --chat me` — real profile (name/bio/phone/username) exit 0
- [x] `tele takeout export --message-limit 3` — 3 contacts, 16 dialogs → `%APPDATA%\telecli\export\1` exit 0
- [x] `tele raw messages.GetAllDrafts` exit 0; `tele raw messages.Nope` exit 1
- [x] `--dry-run` variants of send/delete/join (no network) — exit 0, `"dry_run":true`
- [x] proxy: `type = "http"` errors clearly (`proxy type http unsupported (grammers supports socks5 only)`, exit 3); positive socks5 path skipped by user decision (would need tor 9050)

Bug found + fixed during live verification: `client.resolve_peer(InputPeerSelf)` in grammers 0.10 fails with a misleading `InvocationError::Dropped` ("request error: dropped (cancelled)") because the response's user id never matches the sentinel id. Fix in `src/entities.rs`: `--chat me` now uses `client.get_me()` instead. Numeric ids only resolve when the session already has the peer's access_hash (self works).

New capability (user-approved): `kernel.peers` — `--chat +phone` targets via raw `contacts.ImportContacts` (no friendly path in grammers 0.10). Caveats learned live: a leading `+` makes `parse::<i64>()` succeed, so the phone branch must come BEFORE the numeric-id branch (it does); importContacts only returns the user if the target's phone-number privacy allows adding (2→1 worked first try, 1→2 only after the contact became mutual); side effect: importContacts adds the number to the account's contact list.

## Standing DoD (every task)

- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test` (no network) — 14 contract tests
- [x] `docs/capabilities.md` updated if a capability shipped

## Deep-review follow-up (2026-08-15, docs/review-2026-08.md)

Shipped (all clippy-clean, 353 tests green, one logical commit each):

- [x] M1–M4 listen: real reconnect (bounded 5 attempts, RAII drop), `--timeout-secs` honored (`min(3600, remaining)`), `--raw` emits base64 payload + state through the `--events` allowlist, per-account connections clamped to `effective_parallel` (1–3)
- [x] M5 `chat admin-log` human action column renders `kind: detail`, char-safe truncation
- [x] M6 `-100…` bot-API channel ids resolve when cached (`PeerId::from_bot_api_dialog_id` first, bare probes fallback)
- [x] M8 QR login imports the migrate-to token via `invoke_in_dc(dc_id, …)`
- [x] M9 `msg forward --silent` extracts ids from `updatesCombined`; errors instead of empty success
- [x] M10 shared `TEST_ENV_LOCK` across msg/takeout unit suites; REL-12 unique contract temp tags
- [x] REL-05 `classify_target` extraction + exhaustive tests (phone/me/link/numeric/username/invalid)
- [x] REL-07 machine-API JSON shape locked by tests; `media_name` panic-proof on empty documents
- [x] REL-08 `run_fanout` panic containment + ordering tests via `run_one`/`collect_one` seam
- [x] ARCH-07 per-account `UsageError` now exits 1 (cli-contract.md:28), keyed on JSON `type` so `ConfigError` keeps exit 3

Still open (not in scope): M7 `topic create --emoji` (wrong ID type — needs `messages.searchCustomEmoji` or re-document), listen auto_reconnect config flag, SEC-01/02/03/05/06/09, OBS-01/02 docs drift, INT-07/08/09/10/12, REL-06/13, README/RAW-01 doc examples, MCP (Phase 6, ask first).

## Bug-hunt 2026-08 triage (2026-08-16, docs/bug-hunt-2026-08.md)

Report written against pre-merge code; HEAD `a702832` already merged 21 fix plans. Every CRITICAL/HIGH + dedup-theme finding re-verified at HEAD by independent read-only agents (verdicts below; "stale" = fixed by a702832 or earlier).

### Verified FIXED at HEAD (no action; report lines stale)
- 10.1 CRITICAL chat join (fixed `b7fe038`; residual bare-`t.me/+hash` forms = ticket T12 below)
- 4.F1/8.H3/M3 numeric peer-kind + `-100…` range gap (fixed `0baad73`)
- 2.1 Windows trailing-dot/space upload bypass (fixed `5cde619` via `validate_filename`)
- 1.7/5.7/6.F6 `print_json` EPIPE panic (fixed; locked by `print_json_to_closed_pipe_returns_err`)
- 3.1 empty env var → CWD (fixed `44c5902`); 3.2 `.env` BOM (fixed `d225a9b`); 3.3/7.10 non-atomic `write_config` (fixed `0afb190` tmp+rename)
- 1.3/3.4/5.4/6.F2 config-failure exit-code taxonomy (fixed `8e151c1`: `ConfigError` → exit 1, contract test flipped) — residual: `listen` still collapses config failures to 3 (T13)
- 1.4/3.6/7.1 `account add` name validation + `all` reserved (fixed `2a61e5b`); 12.11 `..` export_dir escape (fixed)
- 2.3/7.4 session-file exclusive lock (fixed `8cfb717` try_lock + lockfile)
- 1.5/6.F3 `-q`/`-v` vs `TELE_LOG` (fixed `dbe35ef`)
- 5.1/9.1 download overwrite/corruption (fixed `e8596ce` temp+rename + `refuse_existing_download_target`; cross-account collision now fails loudly instead of corrupting)
- 13.1 raw human-mode silence (fixed); 13.2 raw mutating account gate (fixed)
- 15.H1 privacy base-rule destruction — claim WRONG at HEAD (`merge_privacy_rules` fetches+merges base rules, tested)
- 15.H2 privacy human table (fixed); 14.1 `--folder 0` / 14.2 dialogFolder rows (fixed `unwrap_or(0)` + skip)
- 2.2 listen forever-stall — mostly fixed (`catch_up: true`, timeout, backoff); narrow edge (GetState fail + no saved state) still possible, T14

### LIVE backlog (verified at HEAD) — priority order
- **T1 HIGH** 8.H1: `msg delete` reports `{"deleted":0}` exit 0 on no-op/partial deletion; grammers `revoke:true` hardcoded, no self-only option. msg.rs:475-478; grammers messages.rs:884-890
- **T2 HIGH** 8.H2: `msg forward --silent` errors "forward succeeded but no new message ids" after a successful RPC → retry duplicates; non-Updates variants dropped; adjacent `chunk[i]` index panic at msg.rs:622. msg.rs:691-705, 741
- **T3 HIGH** 12.1: takeout `GetContacts` not wrapped in `InvokeWithTakeout` (only GetDialogs/GetHistory are; takeout_id IS persisted). takeout.rs:222-226
- **T4 HIGH** 10.2: `chat create --kind group` prints positive chat_id that `--chat <id>` cannot resolve (cached_ref probes user+channel only, never `PeerId::chat`). entities.rs:147-151, chat.rs:781-785
- **T5 MED** 4.F3/8.H4/15.M2: `+phone` targets permanently import number into contacts as side effect of any command. entities.rs:21-33
- **T6 MED** 10.4: `chat join` discards returned peer; access_hash never cached → follow-up id commands fail. chat.rs:168-182
- **T7 MED** 3.5: empty `TELE_API_HASH=` accepted (only presence checked). config.rs:199-201
- **T8 MED** 3.7: `write_config` re-serializes → comments/unknown keys destroyed on every account add/remove. config.rs:288
- **T9 MED** 9.2: Windows path guards case-sensitive + raw-path fallback on canonicalize failure (upload + download). msg.rs:296/314/323, config.rs:5-7
- **T10 MED** 12.3: `MessageEmpty` in EditChannelMessage panics via `serialize.rs:30` → grammers message.rs:247 `expect` → kills account stream. listen.rs:206/223
- **T11 MED** 8.M1: empty/whitespace `--text` passes validation → server MESSAGE_EMPTY exit 3 instead of Usage exit 1. msg.rs:180-192
- **T12 MED** 10.3: bare `t.me/+hash` invite forms rejected (no https:// normalization); only full URLs + bare `+hash` work. chat.rs:167-172
- **T13 MED** listen residual: config failure exits 3 via `aggregate_exit` instead of 1. listen.rs:289-298
- **T14 LOW** 2.2 edge: GetState fail + pristine message box → silent stall; + 10.9 grammers `take_user().unwrap()` panic on basic-group participants (participant.rs:217/224/231)
- **T15 LOW** 13.4: `raw --args '{"chat":""}'` → fabricated INVALID_PEER_ID exit 3 instead of Usage exit 1. raw.rs:146-153, entities.rs:70
- **T16 LOW** 8.L1: `validate_markdown` rejects any text containing `tg://user?id=` substring even inside a normal URL. msg.rs:215
- **T17 LOW** 8.L4: `--file <nonexistent>` → exit 3, guard bypassed for non-existent files. msg.rs:323
- **T18 LOW** 12.7/12.8: listen `failures` never reset on successful reconnect; connect errors outside retry loop. listen.rs:140-171

### Unverified remainder (medium/low, not re-checked at HEAD; verify via RED test when scheduled)
Slices 4-15 mediums/lows not listed above, incl. 3.9/3.10/3.12/3.13, 4.F5/F7/F8, 5.5/5.6, 6.F4/F5/F7/F8, 7.5-7.9/7.11, 8.L2/L3/L5/L6/L7, 9.3-9.8, 10.5-10.12, 11.F1-F8, 12.4-12.6/12.9/12.10/12.12/12.13, 13.3/13.5/13.6/13.7, 14.3-14.9, 15.M1/M3, 15.L1-L6 — each gets a RED test before code in its ticket.
