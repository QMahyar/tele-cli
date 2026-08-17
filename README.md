# tele

> A Rust CLI for driving real Telegram user accounts — messages, chats, groups, contacts, privacy, live streaming, and more. No bot tokens.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)

---

## Why?

- **User accounts, not bots.** Full MTProto client surface — scheduled messages, forum topics, admin log, takeout export, raw TL calls. Anything your Telegram account can do, `tele` can do.
- **Multi-account.** Name them, tag them, fan out a command across all of them. Sequential by default, `--parallel 1–3` when you need speed.
- **Human + machine.** Comfy tables on your terminal; a single JSON envelope (or JSONL from `listen`) on stdout for scripts and AI agents. Logs on stderr only.
- **Pure Rust.** Built on [grammers](https://docs.rs/grammers-client) 0.10 (MTProto). Zero C/C++ dependencies. `cargo build` and done.

---

## Install

Install: `cargo build --release` (binary `target/release/telecli.exe`); an npm wrapper `@qmahyar/telecli` (win32-x64) is published per `docs/release.md` when you say so. Release gate (ADR-005) is met.
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

That's it. Every command supports `--dry-run`, `--json`, and `--account` / `--tag` for account selection.

---

## Commands

| Group | Commands |
|---|---|
| **Accounts** | `account list`, `account add`, `account login`, `account logout`, `account remove`, `account status` |
| **Messages** | `msg send`, `msg get`, `msg edit`, `msg delete`, `msg forward`, `msg search`, `msg react`, `msg download`, `msg read`, `msg pin` |
| **Chats** | `chat join`, `chat create`, `chat leave`, `chat participants`, `chat kick`, `chat admin`, `chat admin-log`, `chat stats`, `chat invite` |
| **Dialogs** | `dialog list`, `dialog drafts`, `dialog archive`, `dialog delete` |
| **Topics** | `topic list`, `topic create` |
| **Contacts** | `contact list`, `contact add`, `contact block` |
| **Profile** | `profile get`, `profile set` |
| **Privacy** | `privacy get`, `privacy set` |
| **Takeout** | `takeout start`, `takeout export`, `takeout finish` |
| **Listen** | `listen` — stream JSONL updates in real time |
| **Raw** | `raw` — typed registry for any supported TL method |
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

**Exit codes:** `0` all succeeded · `1` usage error · `2` partial failure · `3` all failed · `4` auth required.

See [docs/cli-contract.md](docs/cli-contract.md) for the full machine API reference.

---

## Security

- Secrets (API keys, sessions, phone numbers) live **outside the repo** under the app data dir and are never logged.
- One session file per account. Never share a session across processes.
- `--dry-run` is honored everywhere — no network calls, no file writes.
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

578 tests across unit, contract, and selection suites. All run offline by default.

---

## Contributing

Contributions welcome. Open an issue first for anything non-trivial. Run the full test suite before submitting:

```bash
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check
```

---

## License

[MIT](LICENSE)
