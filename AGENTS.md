# Tele-Cli — agent context

Load this file every session. For a slice, also load **only** the matching `docs/capabilities.md` rows plus `docs/spec.md` sections you will touch. Do not load the entire Telegram API.

## Stack

- Rust stable, grammers-client 0.10 (MTProto, Lonami), clap, tokio, serde_json, comfy-table
- Build: `cargo`. Default `cargo test` has **no network**. Live: run manually with your sessions.

## Commands

```
cargo build
cargo run -- --help
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Conventions

- `src/client.rs` owns grammers. CLI commands resolve accounts and print; they never build a `Client` themselves.
- One `FileSession` per named account under the app data dir. Never CWD. Never two clients on one file.
- Friendly grammers methods first. Raw `tl::functions.*` only via `tele raw` (typed registry in `src/commands/raw.rs`) or when the matrix has no friendly path.
- Sequential default. `--parallel N` clamped to 1–3. Honor FloodWait / SlowModeWait (`flood_sleep_threshold=60`, `AutoSleep` retry).
- `--json` / JSONL is the public machine API. Change it only additively. See `docs/cli-contract.md`.
- Structured logs on **stderr**. Secrets, session strings, phone numbers, 2FA, api_hash never logged.
- No comments in code unless the user asks.

## Boundaries

**Always:** update `docs/capabilities.md` in the same change that ships a capability; clippy + tests before a slice is done; disconnect clients in `finally` / RAII guard.

**Ask first:** flip a matrix row to `never`; add a runtime dependency; introduce a daemon; change session backend; implement MCP/skill; publish to PyPI/crates.

**Never:** commit `.env`, sessions, phones, api_hash; share a session file; bot-token as the product path; voice/video, secret chats, Passport in v1; Telegram from unit tests; log secrets.

## Slice workflow

1. Pick one `tasks/todo.md` item and the matching capability ids.
2. RED: failing test in `tests/` (offline). Live behavior verified manually with real sessions.
3. GREEN: minimal kernel + command.
4. Mark matrix rows `done` if shipped.
5. Do not start the next slice until clippy + `cargo test` pass.

## Pointers

- Intent: `docs/ideas/tele-cli.md`
- Spec: `docs/spec.md`
- Matrix: `docs/capabilities.md`
- CLI contract: `docs/cli-contract.md`
- Threat model: `docs/security.md`
- Logs: `docs/observability.md`
- Release: `docs/release.md`
- ADRs: `docs/decisions/` (006 = Rust/grammers pivot)
- Plan: `tasks/plan.md`