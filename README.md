<p align="center">
  <img src="docs/demo/banner.svg" alt="Tele-Cli" width="600">
</p>

<h3 align="center">Telegram automation from your terminal</h3>

<p align="center">
  Multi-account. Parallel. Scriptable. No bot tokens.
</p>

<p align="center">
  <a href="https://github.com/QMahyar/tele-cli/releases"><img src="https://img.shields.io/github/v/release/QMahyar/tele-cli" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-green.svg" alt="License"></a>
  <a href="https://github.com/QMahyar/tele-cli/actions"><img src="https://img.shields.io/github/actions/workflow/status/QMahyar/tele-cli/ci.yml?branch=main" alt="CI"></a>
  <img src="https://img.shields.io/badge/rust-1.89+-orange" alt="Rust">
</p>

---

**Tele** drives real Telegram user accounts from the command line. Built on [grammers](https://docs.rs/grammers-client) (MTProto). No bot tokens.

```bash
npm install -g @qmahyar/telecli
tele msg send --chat me --text "hello from tele"
```

## What you get

- **16 command groups**: accounts, messages, chats, dialogs, forum topics, contacts, profile, privacy, stories, stickers, takeout, listen, serve, mcp, raw, completions. Run `tele --help` for every command and flag.
- **Multi-account**: named sessions, tags, parallel fan-out (`--parallel 1-32`) with per-account rate limiting and FloodWait handling.
- **Machine output**: every command supports `--json` and `--jsonl` with a stable envelope. `tele listen` streams events as JSONL.
- **MCP server**: `tele mcp` exposes 67 tools for Claude, Cursor, and any MCP client.
- **Duplex server**: `tele serve` runs a JSONL request/response protocol over stdin/stdout for embedding.
- **Dry-run everywhere**: `--dry-run` validates and prints the exact intended action without any network call.
- **Raw TL access**: `tele raw` invokes 25 typed Telegram API methods through an allowlist.
- **Cross-platform**: Windows, macOS, and Linux binaries for 13 targets, including a static `linux-arm64-musl` build for Termux/Android.

## Install

### npm (recommended)

```bash
npm install -g @qmahyar/telecli
tele --version
```

The npm package bundles all platform binaries and installs the one that matches your system.

### Binary download

Download a binary from [Releases](https://github.com/QMahyar/tele-cli/releases).

### Build from source

Requires [Rust 1.89+](https://www.rust-lang.org/tools/install).

```bash
git clone https://github.com/QMahyar/tele-cli.git
cd tele-cli
cargo build --release
```

### Shell completions

```bash
tele completions bash >> ~/.bashrc
tele completions zsh  >> ~/.zshrc
tele completions fish > ~/.config/fish/completions/tele.fish
# PowerShell: tele completions powershell | Out-String | Invoke-Expression
```

## Quick start

**1. Get API credentials.** Create an app at [my.telegram.org](https://my.telegram.org) and save your `api_id` and `api_hash` to the config directory:

```bash
# Linux/macOS (~%APPDATA%\telecli on Windows)
mkdir -p ~/.config/telecli
echo 'TELE_API_ID=1234567' > ~/.config/telecli/.env
echo 'TELE_API_HASH=0123456789abcdef0123456789abcdef' >> ~/.config/telecli/.env
```

**2. Add and log in an account.** `account login` prompts for the login code, or use `--method qr`:

```bash
tele account add --name work
tele account login --name work --method code --phone +1XXXXXXXXXX
```

**3. Use it.**

```bash
tele msg send --chat me --text "hello from tele"   # send yourself a message
tele dialog list                                    # recent chats
tele msg send --chat me --text "test" --dry-run     # preview, no network
```

See [docs/getting-started.md](docs/getting-started.md) for the full walkthrough.

## Multi-account

Accounts live in `config.toml` with tags. Target one account, a tag group, or all of them, and fan out in parallel:

```bash
tele msg send --account work --chat me --text "from work"
tele msg send --tag bulk --chat @channel --text "broadcast" --parallel 8
```

```toml
# ~/.config/telecli/config.toml
[accounts.work]
tags = ["bulk"]

[accounts.work.proxy]
type = "socks5"
host = "127.0.0.1"
port = 1080
```

## Machine output

Every command emits one JSON envelope with per-account results:

```json
{
  "ok": true,
  "command": "msg send",
  "results": [
    { "account": "work", "ok": true, "data": { "id": 42 }, "error": null }
  ]
}
```

Exit codes: `0` success, `1` usage error, `2` partial failure, `3` all failed, `4` auth required, `130` interrupted.

Most commands accept `--chat` or `--target` as `@username`, a `t.me/` link, a numeric ID, `me`, or `+phone`.

The full JSON shapes, MCP tool table, and protocol rules are the public contract in [docs/cli-contract.md](docs/cli-contract.md). Contract changes are additive; anything else is a bug.

## MCP for agents

```json
{
  "mcpServers": {
    "tele": {
      "command": "tele",
      "args": ["mcp", "--account", "work"]
    }
  }
}
```

Add `--read-only` to hide destructive tools, or `--groups msg,dialog` to filter tool groups.

## Security

- Sessions live under the OS app data directory, are permission-restricted, and hold an OS-level exclusive lock per account.
- API keys, phone numbers, and 2FA passwords are scrubbed from logs and JSON output.
- `tele raw` has full account power. Treat it like your Telegram client.

See [docs/security.md](docs/security.md) for the threat model.

## Documentation

| Document | Purpose |
|---|---|
| [Getting started](docs/getting-started.md) | Setup walkthrough and troubleshooting |
| [Examples](docs/examples.md) | Recipes for common tasks |
| [CLI contract](docs/cli-contract.md) | Machine API reference: JSON shapes, exit codes, MCP tools |
| [Architecture](docs/architecture.md) | Codebase structure and data flow |
| [Security](docs/security.md) | Threat model and trust boundaries |
| [Capabilities](docs/capabilities.md) | Feature matrix with Telegram API mapping |
| [Contributing](docs/CONTRIBUTING.md) | Adding commands, conventions, testing |
| [Observability](docs/observability.md) | Logging and output streams |
| [Release](docs/release.md) | Build targets and publishing |
| [Decisions](docs/decisions/) | Architecture decision records |

For coding agents: [AGENTS.md](AGENTS.md) carries the project conventions. Load it before touching the code.

## License

[MIT](LICENSE). Built by [QMahyar](https://github.com/QMahyar). Powered by [grammers](https://github.com/LonamiWebs/grammers).
