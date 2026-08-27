# tele

A Rust CLI for driving real Telegram user accounts. Messages, chats, groups, contacts, privacy, live streaming, and more. No bot tokens.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)

---

## What it does

- **User accounts, not bots.** The full MTProto client surface: scheduled messages, forum topics, admin log, takeout export, raw TL calls. Anything your Telegram account can do, `tele` can do.
- **Multi-account.** Name accounts, tag them, fan out a command across all of them. Sequential by default, `--parallel 1–32` when you need speed.
- **Human and machine.** Tables on your terminal for people. One JSON envelope (or JSONL from `listen`) on stdout for scripts and AI agents. Logs go to stderr only.
- **Pure Rust client.** Built on [grammers](https://docs.rs/grammers-client) 0.10 (MTProto). The only C is the SQLite engine bundled for Telethon session import.
- **MCP server built in.** `tele mcp` exposes the whole CLI as tools for Claude, Cursor, or any MCP client.

---

## Install

npm (Windows, Linux x64/arm64, macOS). The matching native binary installs automatically:

```bash
npm install -g @qmahyar/telecli
tele --version
```

Or grab a binary from [Releases](https://github.com/QMahyar/tele-cli/releases). Builds cover 7 targets, including a **static linux-arm64-musl build that runs in Termux/Android** and a static linux-x64-musl for any distro.

Build from source (requires [Rust stable](https://www.rust-lang.org/)):

```bash
git clone https://github.com/QMahyar/tele-cli.git
cd tele-cli
cargo build --release
cargo run -- --help          # smoke-test
```

The binary is `target/release/tele` (`.exe` on Windows). For development builds use
`cargo build` and `cargo run -- ...` instead.

**Shell completions** (after install):

```bash
tele completions bash >> ~/.bashrc
tele completions zsh  >> ~/.zshrc
tele completions fish > ~/.config/fish/completions/tele.fish
```

---

## Quick start

```bash
# 1. Add an account (prompts for phone + code)
tele account add --name work
tele account login --name work --method code --phone +1XXXXXXXXXX

# 2. Send a message (dry-run first — touches nothing)
tele msg send --chat me --text "hello from tele" --dry-run
tele msg send --chat me --text "hello from tele"

# 3. Stream updates as JSONL
tele listen --events NewMessage --chat me --timeout-secs 30
```

Every command supports `--dry-run`, `--json`, and `--account` / `--tag` for account selection.

---

## Commands

| Group | Commands |
|---|---|
| **Accounts** | `account list`, `account add`, `account login`, `account logout`, `account remove`, `account status` |
| **Messages** | `msg send` (albums, topics, TTL, URL upload, copy-media), `msg get`, `msg edit`, `msg delete`, `msg forward`, `msg search --global`, `msg react`, `msg download --chunk-size-kb`, `msg read --mentions`, `msg pin --show/--all/--notify` |
| **Chats** | `chat join`, `chat create`, `chat leave`, `chat participants --role`, `chat kick --ban`, `chat admin`, `chat admin-log`, `chat stats`, `chat invite` (full link suite), `chat settings`, `chat edit`, `chat link` |
| **Dialogs** | `dialog list`, `dialog drafts`, `dialog draft`, `dialog archive`, `dialog delete --revoke`, `dialog pin` |
| **Topics** | `topic list`, `topic create`, `topic close`, `topic reopen`, `topic edit`, `topic delete`, `topic pin` |
| **Contacts** | `contact list`, `contact add`, `contact remove`, `contact block`, `contact unblock` |
| **Profile** | `profile get`, `profile set` (name/bio/username), `profile photo --remove`, `profile emoji-status` |
| **Privacy** | `privacy get` (14 keys), `privacy set` (incl. chat-participant rules) |
| **Takeout** | `takeout start`, `takeout export` (progress + resume), `takeout finish --abandon` |
| **Listen** | `listen` - stream JSONL updates (`NewMessage`, `MessageEdited`, `MessageDeleted`, `Raw`, `Album`, `Gap`, `Service`, `ChatAction`, `UserUpdate`) |
| **Raw** | `raw` - typed registry for 25 supported TL methods |
| **Stickers** | `stickers list`, `stickers sets`, `stickers add`, `stickers delete` |
| **Stories** | `stories list`, `stories send`, `stories view`, `stories delete`, `stories archive` |
| **Serve** | `serve` — JSONL server over stdin/stdout |
| **MCP** | `mcp` — MCP stdio server |
| **Completions** | `completions bash`, `completions zsh`, `completions fish`, `completions powershell` |

Run `tele --help` for the full reference, or `tele <command> --help` for a specific command.

---

## Global flags

| Flag | Description |
|---|---|
| `--account <name>` | Target a specific account (default: all sessions) |
| `--tag <tag>` | Target accounts with this tag |
| `--json` | Machine-readable JSON output |
| `--jsonl` | JSON Lines output (one object per line) |
| `--dry-run` | Validate and print what would happen — no network calls |
| `--parallel <1-32>` | Fan out across accounts in parallel |
| `--config <path>` | Override config file location |
| `-v` / `-vv` | Verbose logging (info / debug) on stderr |
| `-q` | Quiet — errors only |
| `--verbose` | Shorthand for `-v` |

---

## Configuration

Config lives at `%APPDATA%\telecli` (Windows) or `~/.config/telecli` (macOS/Linux). Override with `--config` or `TELE_APP_DIR`.

**`.env`** — API credentials (from [my.telegram.org](https://my.telegram.org)):

```
TELE_API_ID=1234567
TELE_API_HASH=0123456789abcdef0123456789abcdef
```

**`config.toml`** — accounts, proxy, tuning:

```toml
flood_sleep_threshold = 60
parallel_max = 3

[proxy]
type = "socks5"
host = "127.0.0.1"
port = 9050

[accounts.work]
tags = ["iran", "work"]

[accounts.work.proxy]
type = "socks5"
host = "127.0.0.1"
port = 1080

[accounts.spam]
tags = ["bulk"]
rpc_per_minute = 20.0
flood_sleep_threshold = 30
```

Per-account keys (optional, override globals):

| Key | Type | Default | Description |
|---|---|---|---|
| `flood_sleep_threshold` | `u64` | global (60) | AutoSleep threshold in seconds |
| `rpc_per_minute` | `f64` | unlimited | Token-bucket RPC rate budget (tokens refill per minute) |

---

## Machine output

Every one-shot command prints a single JSON object on stdout with `--json`:

```json
{
  "ok": true,
  "command": "msg send",
  "dry_run": false,
  "results": [
    {
      "account": "work",
      "ok": true,
      "data": { "id": 42, "date": "2026-08-13T12:00:00+00:00", "text": "hello" },
      "error": null
    }
  ]
}
```

`listen` streams one JSONL object per update on stdout. Events: `NewMessage`, `MessageEdited`, `MessageDeleted`, `Raw` (selected via the `--events` allowlist; `Raw` rows carry a base64 `raw` payload plus a `state` object).

**Exit codes:** `0` all succeeded · `1` usage error · `2` partial failure · `3` all failed · `4` auth required · `130` interrupted (SIGINT).

See [docs/cli-contract.md](docs/cli-contract.md) for the full machine API reference.

---

## Security

- Secrets (API keys, sessions, phone numbers) live **outside the repo** under the app data dir and are never logged.
- One session file per account. Never share a session across processes.
- `--dry-run` is honored everywhere. No network calls, no file writes.
- `tele raw` is full account power. Treat it with the same care as your Telegram client.

See [docs/security.md](docs/security.md) for the full threat model.

---

## Development

```bash
git clone https://github.com/QMahyar/tele-cli.git
cd tele-cli
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Hundreds of tests across unit, contract, and selection suites. All run offline by default.

---

## Contributing

Contributions welcome. Open an issue first for anything non-trivial. Run the full test suite before submitting:

```bash
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check
```

---

## License

[MIT](LICENSE)
