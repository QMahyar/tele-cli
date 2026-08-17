# Tele-Cli — Agent Context

> Load this file every session. For a slice, also load only the matching `docs/capabilities.md` rows plus `docs/ideas/tele-cli.md` sections you will touch.

## Tech Stack

- **Language:** Rust stable (edition 2021)
- **Telegram:** grammers-client 0.10 (MTProto, Lonami) — no TDLib, no C/C++
- **CLI:** clap 4 (derive), clap_complete 4
- **Async:** tokio (rt-multi-thread, macros, sync, time, io-std)
- **Serialization:** serde + serde_json, toml 0.8
- **Output:** comfy-table 7 (human), JSON/JSONL (machine)
- **Other:** chrono, base64, qrcode, log (env-gated)

## Commands

```bash
cargo build                              # debug build
cargo build --release                    # optimized build
cargo test                               # 578 tests, no network
cargo clippy --all-targets -- -D warnings # lint
cargo fmt --all -- --check               # format check (cargo fmt --all to fix)
cargo run -- --help                      # CLI help
$env:TELE_LOG="debug"; cargo run ...     # stderr logs (trace = grammers internals)
```

## Project Map

```
src/
├── main.rs              Entry point, clap derive, --verbose/--quiet
├── client.rs            grammers ClientGuard, SenderPool, connect()
├── config.rs            App data dir, config.toml, .env, proxy
├── entities.rs          Peer cache, access_hash, chat/user/channel refs
├── error.rs             TeleError enum, exit codes (1-4)
├── executor.rs          run_fanout(), select_accounts(), parallel semaphore
├── output.rs            Envelope, machine_mode(), log_line()
├── serialize.rs         message_to_json(), peer_key(), media_name()
├── session.rs           FileSession (SQLite) per named account
├── logging.rs           stderr-only structured logging
├── fs_util.rs           Permission helpers (create_dir_private, restrict_file_private); sensitive-file detection lives in msg.rs validate_upload_path()
└── commands/
    ├── mod.rs           Subcommand enum dispatch
    ├── account.rs       add, login (code/QR), logout, remove, status, list
    ├── msg.rs           send, get, edit, delete, forward, search, react, download, read, pin
    ├── chat.rs          join, create, leave, participants, kick, admin, admin-log, stats, invite
    ├── dialog.rs        list, drafts, archive/unarchive, delete
    ├── topic.rs         list, create
    ├── contact.rs       list, add, block/unblock
    ├── profile.rs       get, set (name, bio, photo)
    ├── privacy.rs       get, set (9 keys)
    ├── takeout.rs       start, export, finish
    ├── listen.rs        JSONL streaming (NewMessage, MessageEdited, MessageDeleted, Raw)
    ├── raw.rs           Typed TL registry
    ├── completions.rs   bash, zsh, fish, powershell
    ├── credentials.rs   creds(), creds_api_id() shared across commands
    └── helpers.rs       peer_id(), stats_*() shared utilities
```

## Code Conventions

- **No comments in code** unless the user explicitly asks for them.
- `src/client.rs` owns grammers. Commands resolve accounts and print; they never build a `Client` themselves.
- One `FileSession` per named account under the app data dir. Never CWD. Never two clients on one file.
- Friendly grammers methods first. Raw `tl::functions.*` only via `tele raw` or when no friendly path exists.
- Sequential default. `--parallel N` clamped to 1–3. Honor FloodWait / SlowModeWait.
- `--json` / JSONL is the public machine API. Change it only additively. See `docs/cli-contract.md`.
- Structured logs on **stderr only**. Secrets, session strings, phone numbers, 2FA, api_hash never logged.
- `--chat` targets: numeric id, `@username`, `t.me/...` link, `me`, `+phone`. Phone branch BEFORE numeric parse.
- Import style: `use crate::commands::credentials::{creds, creds_api_id};` for shared functions.

## Gotchas (grammers 0.10)

- `client.resolve_peer(InputPeerSelf)` fails with misleading `InvocationError::Dropped` — use `client.get_me()` for "me".
- "request error: dropped (cancelled)" during peer resolution = RPC response lacked the requested peer, not network failure.
- `contacts.ImportContacts` adds the number to contact list and returns user only when phone-number privacy permits.
- `"+98...".parse::<i64>()` succeeds — phone branch must be BEFORE numeric id parse in peer resolution.

## Boundaries

**Always:**
- Update `docs/capabilities.md` in the same change that ships a capability
- `cargo clippy` + `cargo test` pass before any slice is done
- Disconnect clients in `finally` / RAII guard
- Scrub real phone numbers and hashes from docs before committing

**Ask first:**
- Flip a matrix row to `never`
- Add a runtime dependency
- Introduce a daemon
- Change session backend
- Implement MCP/skill (Phase 6)
- Publish/tag/CI setup
- Push (unless asked)

**Never:**
- Commit `.env`, sessions, phones, api_hash
- Share a session file
- Use bot-token as the product path
- Voice/video, secret chats, Passport in v1
- Telegram from unit tests
- Log secrets

## Live Environment

- Credentials live OUTSIDE the repo: `%APPDATA%\telecli\.env` (`TELE_API_ID`/`TELE_API_HASH`)
- Config: `config.toml` (accounts 1, 2), Sessions: `sessions/{name}.session`
- `tele account login` prompts interactively. For non-TTY: spawn with piped stdin, detect prompt, poll code file.

## Slice Workflow

1. Pick one `tasks/todo.md` item and matching capability ids.
2. RED: failing test in `tests/` (offline). Live behavior verified manually.
3. GREEN: minimal kernel + command.
4. Mark matrix rows `done` if shipped.
5. Do not start next slice until clippy + `cargo test` pass.
6. Commit: `feat|fix|refactor|test|docs|chore:` prefix, one logical change.

## Pointers

- Intent: `docs/ideas/tele-cli.md`
- Spec: `docs/ideas/tele-cli.md`
- Matrix: `docs/capabilities.md`
- CLI contract: `docs/cli-contract.md`
- Security: `docs/security.md`
- Observability: `docs/observability.md`
- Release: `docs/release.md`
- ADRs: `docs/decisions/` (006 = Rust/grammers pivot; 005 = release gate)
- Tasks: `tasks/todo.md` (live tracker)
