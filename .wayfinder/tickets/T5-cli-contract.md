# T5 — fix/cli-contract-consistency

Labels: wayfinder:task
Branch: fix/t5-cli-contract
Blocked by: T4

## Question

Command-layer behavior diverges from docs/cli-contract.md and from itself in six places — which divergences are bugs vs contract text to update?

## Scope

1. `listen --raw` help text is false (listen.rs:26): says "instead of" but code appends Raw to allowlist (listen.rs:44-46). DECISION: make help say "also emit raw TL updates" (keep append behavior — changing semantics breaks users).
2. dialog.rs:253-294, :296-327 (archive/delete) skip `require_chat_target` → empty `--chat` exits 3 instead of 1. Add the guard like chat.rs does.
3. Manual mutual-exclusion ifs → clap `conflicts_with`: main.rs:169 (`--json`/`--jsonl`), chat.rs:535-547 (`--promote`/`--demote`, `--preset`/`--rights`). Keep any manual JSON-envelope emission for json/jsonl error path.
4. Dedup helpers: raw.rs:320-404 re-implements `peer_id`, `stats_period`, `stats_abs`, `stats_percent` — import from commands/helpers.rs instead.
5. raw.rs:410-415 silent `as` casts i64→i32 → `i32::try_from` mapped to Usage error.
6. argv_command_hint mislabels envelope.command when non-option flag values present (main.rs:206-242) — make value-skipping flag-aware.
7. Help-text alignment for `--chat` targets across chat.rs/dialog.rs variants (mention @username, t.me link, numeric ID, invite link, me; +phone where classify_target accepts it).
8. docs/cli-contract.md: document drafts negated-id convention (dialog.rs:174-178 emits -chat_id/-channel_id vs positive user ids); document listen Ctrl+C→130, backoff 1→30s cap 5 consecutive, and `catch_up: true` backlog replay semantics.

## Done when

All exits match contract; clippy/fmt/tests green; contract doc updated in same change.
