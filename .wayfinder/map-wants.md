# Wayfinder Map — implement-every-want

Label: wayfinder:map
Tracker: local markdown (`.wayfinder/tickets/`)
Status: ACTIVE — Wave 1 launched

## Destination

Every `want` row in `docs/capabilities.md` flipped `done` (except mcp/skill, gated on explicit user go), with clippy/fmt/tests green per ticket and additive-only CLI contract.

## Notes

- Rust CLI, grammers-client 0.10 (TL layer 227). Read AGENTS.md before any ticket.
- Conventions: no comments in code; gates per ticket = `cargo fmt --all` + `cargo clippy --all-targets -- -D warnings` + `cargo test`; commit prefixes `feat|fix|refactor|test|docs:`; never push.
- EXECUTION OVERRIDE: this map carries implementation, not just decisions (user direction 2026-08-23: manager-agent delegates, never codes).
- PARALLEL PROTOCOL (overrides previous single-checkout decision): one git worktree + branch per ticket under `%TEMP%\opencode\tele-wt\`; shared warm build cache via `$env:CARGO_TARGET_DIR='E:\Code\Tele-Cli\target'`. File sets strictly disjoint per concurrent wave.
- Manager owns: docs/capabilities.md flips, tasks/todo.md, merges (`--no-ff` into `main`, suite re-run per merge), review, QA delegation. Agents NEVER edit docs/, tasks/, .wayfinder/, tests/contract.rs, Cargo.toml.
- business.*/stars.* = never (product decision 2026-08-23). mcp/skill = want but Phase-6 ask-first gate stands.
- TERMINOLOGY LOCK (audited 2026-08-23 against full flag/subcommand inventory): JSON keys mirror TL snake_case terms (grouped_id, edit_date, via_bot, reply_markup) — additive only per cli-contract.md; flags kebab-case; prefer flat-subcommand + mode-flags over nested groups (profile photo --remove style); secrets never via argv/env (no-echo prompts like login); serve ops will use command-path form ("msg send"), matching envelope.command, NOT dot form.

## Decisions so far

- [Gap triage](../tasks/todo.md) — 13 new want rows recorded; FLOOD_WAIT envelope partial done; silent/scheduled/album-receive already shipped.
- [kernel.serve research](../tasks/todo.md) — three-agent verdict: single-owner duplex stdio runtime (serve-A shipped on main `bdbfd1f`); serve-B `_core` extraction; serve-C hardening.

## Tickets — Wave 1 (parallel, disjoint)

- [W1-1 msg.buttons](tickets/W1-1-msg-buttons.md) — `src/serialize.rs`
- [W1-2 listen.filters](tickets/W1-2-listen-filters.md) — `src/commands/listen.rs`
- [W1-3 invite-check + join-requests](tickets/W1-3-chat-invite-requests.md) — `src/commands/chat.rs`
- [W1-4 password + session management](tickets/W1-4-auth-password-sessions.md) — `src/commands/account.rs`
- [W1-5 device identity](tickets/W1-5-device-id.md) — `src/config.rs`, `src/client.rs`
- [W1-6 link resolver](tickets/W1-6-link-resolve.md) — `src/entities.rs`

## Blocked / sequenced (tickets written, edges below)

- W2-1 msg batch (poll/typing/album-send/send-mods/click) — `msg.rs`+`raw.rs`; blocks on W1 merges touching serialize shape review
- W2-2 session-port + Telethon converter — `session.rs`
- W2-3 login-staged — `account.rs`; blocked by W1-4
- W3-1 serve-B cores + dispatch — multi-file; SOLO wave after W2
- W3-2 serve-C hardening — blocked by W3-1
- W4-1 raw-registry batch (effect/checklist/translate/transcribe/ai-compose/schedule-repeat) — `raw.rs`
- W4-2 stickers.manage; W4-3 stories.*

## Not yet specified

- Live verification checklist expansion for all network features (user-assisted, real sessions).
- cli-contract.md additive doc sync per merged feature (manager does at merge time).

## Out of scope

- business.*, stars.* (never). MCP/skill without explicit user go. Voice/video, secret chats, Passport (standing nevers).
