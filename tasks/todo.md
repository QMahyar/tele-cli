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
