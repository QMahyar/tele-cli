# Tele-Cli — agent context

Load this file every session. For a slice, also load **only** the matching `docs/capabilities.md` rows plus `docs/spec.md` sections you will touch. Do not load the entire Telegram API.

## Skills (always load, in this order)

1. `using-agent-skills` — route every task to its workflow skill (spec/test/review/release…); follow its operating behaviors.
2. `rust-engineering` — all Rust work: idioms, guardrails, verify with `cargo fmt --check`, `clippy -D warnings`, `cargo test`.

Load additional workflow skills per task as routed (e.g. `test-driven-development` for slices, `documentation-and-adrs` when docs/ADRs change, `ci-cd-and-automation` if CI is ever added).

## Status (2026-08)

- Capability matrix fully resolved: no `want` rows left (`docs/capabilities.md` — all `done`/`later`/`never`). Per ADR-005 the release gate is met.
- Remaining product surface: Phase 6 — MCP server + agent skill (ask first). Release readiness is open work: CI is live (fmt/clippy/test on push+PR). `docs/release.md` and `CHANGELOG.md` were rewritten to the Rust era by the docs-agent pass (2026-08); keep them current.
- Live-verified against real sessions: send/get/edit/delete, cross-account listen, profile get, takeout export, raw registry, dry-runs, proxy negative path. Remaining live items are user-side (2FA/QR login, chat participants/adminlog on a real group, socks5 positive path, `--file`/`--schedule`, listen MessageEdited/MessageDeleted/Raw, takeout finish, logout).

## Stack

- Rust stable, grammers-client 0.10 (MTProto, Lonami), clap, tokio, serde_json, comfy-table, log (env-gated)
- Build: `cargo`. Default `cargo test` has **no network**. Live: run manually with your sessions.

## Commands

```
cargo build
cargo run -- --help
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check        (cargo fmt --all to fix)
$env:TELE_LOG="debug"; cargo run ...   (structured stderr logs; trace = grammers internals)
```

## Conventions

- `src/client.rs` owns grammers. CLI commands resolve accounts and print; they never build a `Client` themselves.
- One `FileSession` per named account under the app data dir. Never CWD. Never two clients on one file.
- Friendly grammers methods first. Raw `tl::functions.*` only via `tele raw` (typed registry in `src/commands/raw.rs`) or when the matrix has no friendly path.
- Sequential default. `--parallel N` clamped to 1–3. Honor FloodWait / SlowModeWait (`flood_sleep_threshold=60`, `AutoSleep` retry).
- `--json` / JSONL is the public machine API. Change it only additively. See `docs/cli-contract.md`.
- Structured logs on **stderr**. Secrets, session strings, phone numbers, 2FA, api_hash never logged.
- `--chat` targets (`src/entities.rs`, kernel.peers): numeric id (needs cached access_hash), `@username`, `t.me/...` link, `me`, `+phone` (raw `contacts.ImportContacts` — no friendly path). Phone branch must stay BEFORE the numeric parse: `"+98...".parse::<i64>()` succeeds.
- grammers 0.10 gotchas: `client.resolve_peer(InputPeerSelf)` fails with a misleading `InvocationError::Dropped` — use `client.get_me()` for "me". Any "request error: dropped (cancelled)" during peer resolution means the RPC response lacked the requested peer, not a network failure. `contacts.ImportContacts` adds the number to the account's contact list and returns the user only when their phone-number privacy permits.
- No comments in code unless the user asks.

## Live environment

- Real credentials/sessions live OUTSIDE the repo: `%APPDATA%\telecli\.env` (`TELE_API_ID`/`TELE_API_HASH`), `config.toml` (accounts 1, 2), `sessions\{name}.session`.
- `tele account login` prompts interactively (no newline + flush). For non-TTY automation there is a helper pattern: spawn with piped stdin, detect the prompt, poll a code file — see `C:\Users\qmahyar\AppData\Local\Temp\opencode\live_login.py`.

## Boundaries

**Always:** update `docs/capabilities.md` in the same change that ships a capability; clippy + tests before a slice is done; disconnect clients in `finally` / RAII guard; scrub real phone numbers and hashes from docs before committing.

**Ask first:** flip a matrix row to `never`; add a runtime dependency; introduce a daemon; change session backend; implement MCP/skill (Phase 6); publish/tag/CI setup; push (unless asked).

**Never:** commit `.env`, sessions, phones, api_hash; share a session file; bot-token as the product path; voice/video, secret chats, Passport in v1; Telegram from unit tests; log secrets.

## Slice workflow

1. Pick one `tasks/todo.md` item and the matching capability ids.
2. RED: failing test in `tests/` (offline). Live behavior verified manually with real sessions.
3. GREEN: minimal kernel + command.
4. Mark matrix rows `done` if shipped.
5. Do not start the next slice until clippy + `cargo test` pass.
6. Commit style: `feat|fix|refactor|test|docs|chore:` prefix, one logical change (per `docs/release.md`); commit/push only when asked.

## Pointers

- Intent: `docs/ideas/tele-cli.md`
- Spec: `docs/spec.md`
- Matrix: `docs/capabilities.md`
- CLI contract: `docs/cli-contract.md`
- Threat model: `docs/security.md`
- Logs: `docs/observability.md`
- Release: `docs/release.md` — versioning, CI plan, publish gate (Rust era; keep current)
- ADRs: `docs/decisions/` (006 = Rust/grammers pivot; 005 = no release until no `want` rows)
- Plan: `tasks/plan.md` (checkboxes stale; `tasks/todo.md` is the live tracker)
