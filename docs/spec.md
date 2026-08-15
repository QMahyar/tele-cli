# Spec: Tele-Cli

> **SUPERSEDED.** This spec is the original intent document. It is superseded by
> the living capability matrix (`docs/capabilities.md`) and
> [ADR-006: Rust CLI with grammers (drop Python)](decisions/006-rust-grammers.md);
> where they conflict, the matrix and the ADRs win. Stale details below (e.g.
> `src/accounts.rs`, `msg poll`, `chat permissions`) describe plans that were
> re-scoped or dropped — check the matrix before relying on them.

## Objective

Build `tele`: a long-lived Rust CLI that operates **real phone-number user accounts** at
native-user depth, for a human in a terminal and for AI agents via `--json` / JSONL.

Success:

- Login once per named account (SMS/app code, 2FA, QR). Session persists as one
  session file per name.
- Fan-out any wrapped command across `--account NAME` (repeatable), `--tag TAG`,
  `--account all`. Sequential default; `--parallel N` with `N<=3`.
- A living capability matrix lists every Telegram user-client domain we care
  about, its grammers path (friendly method | raw `tl::functions.*`), the CLI
  command, and status (`want` | `later` | `never` | `done`). No capability is
  silently skipped.
- Agents reach unwrapped edge cases via `tele raw` (typed registry). MCP server +
  agent skill ship last, same kernel.
- Unpublished until the `want` matrix is `done`.

Users: you, plus your agents running the same binary.

## Tech Stack

| Piece | Choice | Why |
|---|---|---|
| Language | Rust (edition 2021, stable) | User requirement; single binary |
| Telegram | `grammers-client` 0.10 (Lonami, same author as Telethon) | Full MTProto user surface; typed TL |
| CLI | clap (derive) | Help, completion, exit codes |
| JSON | serde / serde_json | Machine API |
| Config | TOML (`config.toml`) + `.env` (manual parse) | Same layout as original spec |
| Tables | `comfy-table` (human mode) | Clean stdout |
| Async | tokio | grammers is tokio-based |
| Tests | `cargo test` (unit, no network) | Contract + config + selection |

Not in v1 runtime: TUI libs, MCP SDK (added when MCP ships).

## Commands

```
# build / quality
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check

tele [GLOBAL] COMMAND ...

  --account NAME     Repeatable. NAME or "all"
  --tag TAG          Repeatable. Union with --account
  --parallel N       Default 1 (sequential). Max 3
  --json             Machine JSON (single object or JSONL)
  --quiet / -q
  --verbose / -v
  --dry-run
  --config PATH      Override config file
```

Command groups (gh-style):

```
tele account login|logout|list|add|remove|status
tele msg    send|edit|delete|forward|pin|unpin|get|react|download   # msg poll: deferred (later), no friendly path in grammers 0.10
tele chat   join|leave|invite|participants|kick|admin|admin-log|stats|create   # chat permissions: deferred (later)
tele topic  create|list|...
tele dialog list|archive|delete|drafts
tele contact list|add|block|unblock
tele profile get|set
tele privacy get|set
tele takeout
tele listen
tele raw METHOD [--args JSON]
tele mcp serve          # last; not in first slices
```

## Project Structure

```
docs/
  spec.md                 This spec
  ideas/tele-cli.md       Confirmed intent / one-pager
  capabilities.md         Living matrix (spine)
src/
  main.rs                 clap root, dispatch, exit codes
  config.rs               TOML + .env load, app dir policy
  accounts.rs             (superseded — never shipped; account/tag selection lives in src/executor.rs select_accounts)
  session.rs              Session path policy (never CWD)
  client.rs               grammers Client factory (flood, proxy, reconnect)
  executor.rs             Sequential / parallel fan-out + flood surfacing
  entities.rs             Resolve @user / t.me / invite / id
  serialize.rs            Message / TL -> JSON (allowlist)
  error.rs                Error type + exit code mapping
  output.rs               --json envelope, tables, stderr logs
  commands/
    account.rs
    msg.rs
    chat.rs
    dialog.rs
    topic.rs
    contact.rs
    profile.rs
    privacy.rs
    takeout.rs
    listen.rs
    raw.rs                Typed TL registry
tests/                    Integration tests (contract, config; no network)
Cargo.toml
.env.example
```

App data (not in repo):

```
Windows: %APPDATA%/telecli/
Unix:    $XDG_CONFIG_HOME/telecli/  or ~/.config/telecli/

  config.toml
  sessions/<account-name>.session
```

## Testing Strategy

| Level | Where | What |
|---|---|---|
| Unit | `tests/` (Rust) | Config merge, account/tag selection, session paths, executor ordering, JSON serialize, dry-run short-circuit |
| Contract | `tests/` | Every `want` matrix row has a CLI command or raw-registry entry; `tele --help` lists declared groups |
| Live | manual / `cargo run` | Real grammers against sessions you provide. Default test suite is offline. Live mutation only against a chat you name (`--to me`) |

`--dry-run` is required on mutating commands and must not send.

## Boundaries

**Always**

- Update `docs/capabilities.md` in the same change that adds or ships a capability.
- Honor FloodWait / SlowModeWait; default sequential; `--parallel` capped at 3.
- Keep `api_id` / `api_hash` out of git; sessions out of git.
- Disconnect clients in `finally` (RAII / guard).
- Run `cargo test` + `cargo clippy` before calling a slice done.

**Ask first**

- Flipping a matrix row to `never` or adding a new `want` domain.
- Adding a runtime dependency.
- Introducing a background daemon (listen may force this later).
- Changing session storage backend.
- Implementing MCP or the agent skill.

**Never**

- Share one session file across two running clients.
- Commit `.env`, session files, or phone numbers.
- Treat bot-token login as the primary product path.
- Implement voice/video calls, secret chats, or Passport in v1.
- Ship MCP/skill before the CLI kernel + first `want` rows.
- Drop a failing test to go green.
- Call Telegram from unit tests.
- Log api_hash, session strings, phone numbers, or 2FA passwords.

## Success Criteria

1. `tele --help` shows groups; no args shows help.
2. `tele account login --account work` completes code + 2FA + QR paths and
   writes `sessions/work.session`.
3. `tele account logout` calls `sign_out` (server logout + delete session).
   Local file delete is a separate `remove --keep-remote`.
4. `tele msg send --account work --to me "hi"` and `--schedule` work; `--json`
   emits parseable output; `--dry-run` does not send.
5. `tele chat join --tag iran --invite <url>` fans out sequentially and reports
   per-account success/FloodWait.
6. `tele listen --account work` streams NewMessage JSONL; `--events` is an
   allowlist; default is NewMessage only.
7. `tele raw <registry-name> --args '{...}'` invokes the TL function.
8. `docs/capabilities.md` has no `want` row without a command path or a raw
   registry entry.
9. Default `cargo test` is green with no network.

## Config sketch

```toml
# %APPDATA%/telecli/config.toml
[app]
flood_sleep_threshold = 60
parallel_max = 3

[proxy]          # optional global
# type = "socks5"   # socks5 | http | mtproto
# host = ""
# port = 0

[accounts.work]
tags = ["iran", "work"]
# proxy override optional
```

```
# .env (never committed)
TELE_API_ID=
TELE_API_HASH=
```

## Related docs

- `docs/cli-contract.md` — exit codes, `--json`, listen JSONL, raw registry
- `docs/security.md` — threat model
- `docs/observability.md` — stderr events
- `docs/release.md` — semver, CI, publish gate
- `docs/decisions/` — ADRs 001–006
- `AGENTS.md` — per-session context pack
