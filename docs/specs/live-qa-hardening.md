# Spec: Live QA Hardening (ZEUS panel test)

Source: 2 live agents deployed real Cloudflare Worker panels via @ZEUS_PANEL_BOT (tele 0.6.8, accounts 1+2). Both reported the same 5 friction points. This spec fixes them.

## Objective
Bot-driven QA is the hardest Tele-Cli flow: send / get / click / poll edited messages, across Persian+emoji buttons, on Windows pwsh, with 14 `cargo run` invocations. Make it one-command discoverable and copy-pasteable.

## Findings (from agents, deduplicated)

| # | Title | Severity |
|---|-------|----------|
| F1 | No discovery for buttons — `callback_data` is opaque base64, no decoder | Must-fix |
| F2 | Polling is manual — bot edits same message (progress bar), no `--watch` | Must-fix |
| F3 | `--button` exact Persian+emoji match brittle — `--button-index` works but undocumented | Must-fix |
| F4 | JSON envelope ergonomics — deep nesting, no jq recipe | Must-fix |
| F5 | Unicode mangles in pwsh + `cargo run` overhead tax | Must-fix |
| F6 | Spec drift — bot's `workers_subdomain` perm no longer exists (395 groups, 0 hits) | Low |

F6 is docs/notes only; no code change.

## Tickets

### T1 — Button discovery
**Acceptance:**
- `tele msg get --chat @BOT --id N --json` already returns `reply_markup.rows[].buttons[].text` and `data` (base64 url/callback). Add `data_str`: base64-decoded UTF-8 string alongside `data` (empty when not valid UTF-8). Additive only — existing `data` unchanged.
- Human table for `msg get` prints a compact `buttons` column (e.g. `[1] 🚀 ساخت پنل (callback) [2] 🔗 Link (url)`), so `--help` users see buttons without `--json`.
- Tests: `cargo test` offline, `msg get --dry-run --json` envelope contains `data_str` for messages with buttons.

### T2 — Polling helper for edited bot messages
**Acceptance:**
- `tele msg get --chat @BOT --id N --watch --timeout-secs 60 --poll-interval 2 --json` polls the same message until `edit_date` changes or a new message appears after N, then prints one envelope and exits 0. On timeout exits 1 with `{"type":"Timeout"}` envelope in `--json` mode. Additive flags — existing `msg get` unchanged.
- Implementation reuses `get_core` loop (no new RPC type); sleeps `poll_interval` between fetches. Caps at `timeout_secs`.
- Tests: offline contract tests for new flags (clap parse, timeout=0 rejected), dry-run shape.

### T3 — Click resilience
**Acceptance:**
- `tele msg click` gains `--button-contains <substring>`: case-insensitive substring match against button `text`. Mutually exclusive with `--button` and `--button-index`. Picks first match; ambiguous (≥2 hits) exits 1 with `Did you mean #i "text" or #j "text"? Available: [#1 "…", #2 "…"]`.
- Exact `--button` failure also suggests `Did you mean #i "…"? Available: [#1 "…", …]` instead of bare "button not found".
- `--help` for `msg click` documents precedence: `--button-index` > `--button-contains` > `--button`, and that `--button-index` is 1-based across all rows.
- Tests: offline (mock reply_markup): exact miss suggests, contains picks first, ambiguous lists, precedence.

### T4 — Docs: JSON envelope + jq + binary tip
**Acceptance:**
- `docs/cli-contract.md` gains a "Bot QA recipe" section: `msg send → msg get → msg click --button-index` loop with `jq` and `python -m json.tool` examples, and the `target\debug\telecli.exe` tip for repeated calls.
- `docs/examples.md` gains a "Bot buttons" example: `tele msg click --chat @bot --id 123 --button-index 1` and `--button-contains`.
- `AGENTS.md` Commands section notes `cargo run` overhead and `target\debug\telecli.exe` for hot loops.

### T5 — Windows UTF-8
**Acceptance:**
- `src/output.rs` and `src/serialize.rs` already emit UTF-8 JSON. Fix is docs + one code guard: `main.rs` on Windows calls `SetConsoleOutputCP(CP_UTF8)` at startup so `cargo run` + `python -c` piping preserves Persian. Verified by `msg get` on a Persian bot message rendering correctly in pwsh without `chcp 65001`.
- `docs/cli-contract.md` notes that stdout is UTF-8 and pwsh users should prefer `target\debug\telecli.exe` or set `chcp 65001` / `$OutputEncoding = [Console]::OutputEncoding = [Text.UTF8Encoding]::new()`.

### T6 — Spec drift (docs only)
**Acceptance:**
- Remove `workers_subdomain` from any Tele-Cli docs/examples that mention it. Cloudflare has 395 permission groups, 0 `subdomain` hits; the 5 remaining perms (Workers Scripts Write e086…, KV Write f7f0…, D1 Write 09b…, Account Settings Read c1fd…, Account Analytics Read b89…) are sufficient — bot accepted them on 2 live deploys.
- If `E:\vault\Platforms\cloudflare\platform.md` is in scope, note the fallback; otherwise leave a code comment near any hard-coded perm list.

**Verification 2026-08-29 (T6, docs/spec-drift):** `rg -n "workers_subdomain|subdomain"` found 0 hits in Tele-Cli `docs/` and `src/` outside this spec; the 5 perms above are sufficient. No hard-coded perm list exists in `src/` — no comment needed. `E:\vault\Platforms\cloudflare\platform.md` out of scope per task; checked and it contains no `workers_subdomain` — not touched.

## Tech stack
Rust stable, `grammers-client 0.10`, `grammers-session 0.10 (sqlite-storage)`, `tokio`, `clap 4`, `serde_json`, `comfy-table`.

## Commands
```
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -- --help
cargo run -- msg get --help
cargo run -- msg click --help
```

## Project structure
```
src/commands/msg/    → T1 (serialize reply_markup), T2 (watch loop), T3 (click matching)
src/serialize.rs     → T1 (data_str)
src/output.rs        → T5 (UTF-8 guard)
src/main.rs          → T5 (Windows codepage)
docs/cli-contract.md → T4, T5, T6
docs/examples.md     → T4
AGENTS.md            → T4
```

## Testing strategy
- `cargo test` (offline, 1399 tests) + `cargo clippy` gate per ticket.
- Contract tests in `tests/contract.rs` cover new flags (parse, mutual exclusion, timeout bounds).
- Unit tests for new helpers (base64→data_str, button substring match, Did-you-mean).
- Live verification deferred: re-run one ZEUS deploy after ship.

## Boundaries
- Always: additive changes only (cli-contract, envelope). No renamed keys.
- Ask first: new runtime dependency.
- Never: commit .env / sessions / phones / api_hash; change session backend.

## Success criteria
- [ ] T1: `msg get --json` envelope has `data_str` alongside `data` for bot buttons; human table shows buttons.
- [ ] T2: `msg get --watch --timeout-secs 60 --poll-interval 2 --json` polls until edit_date changes, then exits 0; timeout exits 1.
- [ ] T3: `msg click --button-contains` works; exact miss suggests `Did you mean`; --help documents precedence.
- [ ] T4: cli-contract + examples + AGENTS updated with bot QA recipe, jq, binary tip.
- [ ] T5: Windows UTF-8 guard in main.rs; docs note pwsh fix.
- [x] T6: workers_subdomain removed from docs — verified 2026-08-29: 0 Tele-Cli hits outside this spec; vault out of scope.
- [ ] cargo test + clippy pass, no branches left, map closed.

## Open questions
- None — all findings have concrete fixes above.
