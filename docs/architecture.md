# Architecture

Tele is a Rust CLI that drives real Telegram user accounts through the MTProto protocol. This document explains how the codebase is structured and how data flows through the system.

## High-level architecture

```
┌─────────────────────────────────────────────────────────┐
│                      main.rs                            │
│                   clap derive                           │
│                global flags, dispatch                   │
└──────────────────────┬──────────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────────┐
│                   commands/                             │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐      │
│  │ msg.rs  │ │ chat.rs │ │  ...    │ │ mcp.rs  │      │
│  └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘      │
│       │           │           │           │             │
│  ┌────▼───────────▼───────────▼───────────▼────┐       │
│  │            executor.rs                       │       │
│  │    run_fanout(), run_one()                   │       │
│  │    account resolution, parallel              │       │
│  └──────────────────┬──────────────────────────┘       │
└─────────────────────┼───────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────┐
│                    kernel/                               │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐      │
│  │client.rs│ │session.rs│ │config.rs│ │error.rs │      │
│  └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘      │
│       │           │           │           │             │
│  ┌────▼───────────▼───────────▼───────────▼────┐       │
│  │              grammers-client                 │       │
│  │           (MTProto protocol)                 │       │
│  └─────────────────────────────────────────────┘       │
└─────────────────────────────────────────────────────────┘
```

## Code structure

```
src/
├── main.rs              Entry point, clap derive, --verbose/--quiet
├── client.rs            grammers ClientGuard, SenderPool, connect()
├── config.rs            App data dir, config.toml, .env, proxy
├── entities.rs          Peer cache, access_hash, chat/user/channel refs
├── error.rs             TeleError enum, exit codes (0,1-4,130)
├── executor.rs          run_fanout(), select_accounts(), parallel semaphore
├── output.rs            Envelope, machine_mode(), log_line()
├── serialize.rs         message_to_json(), peer_key(), media_name()
├── session.rs           FileSession (SQLite) per named account
├── logging.rs           stderr-only structured logging
├── fs_util.rs           Permission helpers, sensitive-file detection
├── rate_limiter.rs      Per-account token-bucket RPC rate limiter
└── commands/
    ├── mod.rs           Subcommand enum dispatch
    ├── account.rs       add, login, logout, remove, status, list, sessions, password, export-session, import-session, ttl, delete, phone
    ├── msg.rs           send, get, edit, delete, forward, search, react, download, read, pin, vote, typing, click
    ├── chat.rs          join, create, leave, participants, kick, admin, admin-log, stats, invite, requests, settings, edit, link
    ├── dialog.rs        list, drafts, draft, archive/unarchive, delete, pin
    ├── topic.rs         list, create, close, reopen, edit, delete, pin
    ├── contact.rs       list, add, remove, block, unblock
    ├── profile.rs       get, set, photo, emoji-status
    ├── privacy.rs       get, set (14 keys)
    ├── takeout.rs       start, export, finish
    ├── listen.rs        JSONL streaming
    ├── raw.rs           Typed TL registry (25 methods)
    ├── completions.rs   bash, zsh, fish, powershell
    ├── stickers.rs      list, search, show, install, remove
    ├── stories.rs       send, list, read, delete, pin, unpin
    ├── serve.rs         JSONL server over stdin/stdout
    ├── mcp.rs           MCP stdio server (tools/call)
    ├── credentials.rs   creds(), creds_api_id()
    └── helpers.rs       peer_id(), stats_*()

tests/
├── contract.rs          Integration tests (binary subprocess)
└── selection.rs         Account selection unit tests
```

## Data flow

### Command execution

1. `main.rs` parses CLI args via clap derive
2. The subcommand enum dispatches to `commands/<group>.rs::run()`
3. `run()` validates args offline (before any network call)
4. Validation returns `TeleError::Usage` on bad input
5. For dry-run, the command builds a `would` payload and returns
6. For real runs, `run_fanout()` resolves accounts and runs closures
7. The closure connects via `client.rs` and makes RPC calls
8. `output.rs` formats the result as a table or JSON envelope

### Account resolution

```
--account work  →  config.toml [accounts.work]  →  sessions/work.session
--tag bulk      →  config.toml [accounts.*.tags] containing "bulk"  →  all matching sessions
(none)          →  all configured accounts
```

### Session locking

Each command takes an exclusive OS lock on `{name}.session.lock` before opening the session. A second process receives `session {name} is in use by another process` and stops.

```
tele msg send --account work ...  →  locks work.session.lock  →  opens work.session  →  executes  →  unlocks
```

### Rate limiting

Each account has a token-bucket rate limiter configured by `rpc_per_minute` in config.toml. The limiter refills at the configured rate and blocks when empty. FloodWait and SlowModeWait errors trigger automatic sleep.

```
rpc_per_minute = 20.0  →  20 tokens/minute  →  ~3.3 requests/second
```

## Key abstractions

### ClientGuard (client.rs)

Owns the grammers `Client` and session lock. Disconnects on drop (RAII).

```rust
let (_guard, client, _sender) = connect(&name, &config_path).await?;
// _guard drops here, releasing the session lock
```

### GlobalFlags (executor.rs)

Parsed global args (`--json`, `--dry-run`, `--account`, etc.) threaded through every command.

```rust
pub struct GlobalFlags {
    pub json: bool,
    pub jsonl: bool,
    pub dry_run: bool,
    pub verbose: u8,
    pub quiet: bool,
    pub accounts: Vec<String>,
    pub tags: Vec<String>,
    pub parallel: u32,
    pub config_path: PathBuf,
}
```

### TeleError (error.rs)

Typed error enum. Maps to exit codes.

```rust
pub enum TeleError {
    Usage(String),      // exit 1
    Telegram(String),   // exit 3
    Auth(String),       // exit 4
    Io(std::io::Error), // exit 3
    // ...
}
```

### Envelope (output.rs)

Wraps per-account results into the JSON envelope on stdout.

```json
{
  "ok": true,
  "command": "msg send",
  "dry_run": false,
  "results": [
    { "account": "work", "ok": true, "data": {}, "error": null }
  ]
}
```

## MCP server (mcp.rs)

The MCP server exposes 67 tools through the Model Context Protocol. Each tool maps to a CLI command:

```
tele mcp --account work
  ↓
tools/list → [{ name: "msg_send", ... }, { name: "dialog_list", ... }, ...]
  ↓
tools/call → msg_send({ chat: "@team", text: "hello" })
  ↓
Same core as `tele msg send --chat @team --text "hello"`
```

## Duplex server (serve.rs)

The serve server is a duplex control plane over stdin/stdout:

```
Driver (your script)          Server (tele serve)
       │                              │
       │──── hello ──────────────────▶│
       │◀─── hello ──────────────────│
       │                              │
       │──── { id:1, op:"msg send" } ─▶│
       │◀─── { type:"response", id:1 }│
       │                              │
       │◀─── { event:"NewMessage" } ──│
       │                              │
       │──── EOF ────────────────────▶│
       │                    (clean exit)
```

## Testing

- **Unit tests**: inline `#[cfg(test)]` in source files. Test validation, dry-run shapes, serialization. No network.
- **Contract tests**: `tests/contract.rs` spawns the binary as a subprocess. Tests exit codes, JSON shapes, error messages.
- **Selection tests**: `tests/selection.rs` tests account selection and tag matching.

All tests run offline. No Telegram connection is made.

## Platform considerations

### Windows

- Session files use Win32 DACL for permission restriction
- Console mode set for password input (echo disabled)
- Binary: `tele.exe`

### Unix

- Session files chmod 0600
- App directory chmod 0700
- Binary: `tele`

### Cross-compilation

The release workflow uses `cross` for Linux targets (C toolchain for libsql's SQLite). The `linux-arm64-musl` build is fully static and runs in Termux/Android.
