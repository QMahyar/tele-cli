# @qmahyar/telecli

Telegram user-account CLI — messages, chats, groups, contacts, privacy, live streaming, and an MCP server for LLM agents. No bot tokens.

## Install

```sh
npm install -g @qmahyar/telecli
telecli --version
```

The right native binary for your platform installs automatically via `optionalDependencies`:

| Package | Platform |
|---|---|
| `@qmahyar/telecli-win32-x64` | Windows, Intel/AMD 64-bit |
| `@qmahyar/telecli-linux-x64-gnu` | Linux x86_64 (glibc) |
| `@qmahyar/telecli-linux-x64-musl` | Linux x86_64 (static musl) |
| `@qmahyar/telecli-linux-arm64-gnu` | Linux ARM64 (glibc) |
| `@qmahyar/telecli-linux-arm64-musl` | Linux ARM64 (static musl — also runs in **Termux/Android**) |
| `@qmahyar/telecli-darwin-arm64` | macOS Apple Silicon |
| `@qmahyar/telecli-darwin-x64` | macOS Intel |

## Termux / Android (no npm needed)

Download the static binary from [GitHub Releases](https://github.com/QMahyar/tele-cli/releases):

```sh
curl -fLO https://github.com/QMahyar/tele-cli/releases/latest/download/telecli-<version>-linux-arm64-musl
chmod +x telecli-<version>-linux-arm64-musl
mv telecli-<version>-linux-arm64-musl $PREFIX/bin/telecli
tele --version
```

## First run

```sh
tele account login --name main --phone +15551234567
tele msg send --chat me --text "hello from telecli"
tele mcp --account main   # MCP stdio server for Claude/Cursor/agents
```

Docs: [cli-contract](https://github.com/QMahyar/tele-cli/blob/main/docs/cli-contract.md) · [capabilities](https://github.com/QMahyar/tele-cli/blob/main/docs/capabilities.md) · [security](https://github.com/QMahyar/tele-cli/blob/main/docs/security.md)
