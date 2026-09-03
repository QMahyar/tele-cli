# @qmahyar/telecli

Telegram user-account CLI — messages, chats, groups, contacts, privacy, live streaming, and an MCP server for LLM agents. No bot tokens.

## Install

```sh
npm install -g @qmahyar/telecli
telecli --version
```

The single bundled package contains all 13 platform binaries; `bin/tele.js` picks the correct one for your OS/arch automatically — no separate `optionalDependencies` platform packages. The `linux-arm64-musl` binary is static and runs in **Termux/Android**.

## Termux / Android (no npm needed)

Download the static binary from [GitHub Releases](https://github.com/QMahyar/tele-cli/releases):

```sh
curl -fLO "https://github.com/QMahyar/tele-cli/releases/latest/download/tele-<version>-aarch64-unknown-linux-musl.tar.gz"
tar -xzf tele-<version>-aarch64-unknown-linux-musl.tar.gz
cp tele-<version>-aarch64-unknown-linux-musl/tele $PREFIX/bin/tele
chmod +x $PREFIX/bin/tele
tele --version
```

## First run

```sh
tele account login --name main --phone +15551234567
tele msg send --chat me --text "hello from telecli"
tele mcp --account main   # MCP stdio server for Claude/Cursor/agents
```

Docs: [cli-contract](https://github.com/QMahyar/tele-cli/blob/main/docs/cli-contract.md) · [capabilities](https://github.com/QMahyar/tele-cli/blob/main/docs/capabilities.md) · [security](https://github.com/QMahyar/tele-cli/blob/main/docs/security.md)
