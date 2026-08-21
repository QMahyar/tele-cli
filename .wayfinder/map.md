# Wayfinder Map — hardening-and-capabilities

Label: wayfinder:map
Tracker: local markdown (`.wayfinder/tickets/`)
Plan: `tasks/plan.md`
Status: ACTIVE — Phase 1 (BUG tickets) in execution

## Destination

All 9 confirmed bugs from the 2026-08-21 deep-dive fixed, then the merged grammers-unused + feature-gap capability set implemented: clippy/fmt/tests green per ticket, contract additive-only, docs synced, shipped to `main` after user review.

## Notes

- Rust CLI, grammers-client 0.10.0 (TL layer 227). Read AGENTS.md before any ticket.
- Conventions: no comments in code; clippy `--all-targets -- -D warnings` + fmt + full test suite green per ticket; commit prefixes `fix|feat|refactor|test|docs:`; never push; never touch `main`.
- Branch flow (Phase 1): single checkout, NO worktrees (user decision — disk/build cost). Integration branch `fix/hardening` off `main`. Agents run SEQUENTIALLY; each cuts its ticket branch from `fix/hardening`, gates, merges back with `--no-ff`, leaving repo on `fix/hardening`.
- Phase 2 will use the same pattern on integration branch `feat/capabilities`.
- Build cache: normal `target/` (warm across tickets — no CARGO_TARGET_DIR override).

## Decisions so far

- [Merge feature gaps into capabilities phase] — user direction; grammers-unused friendly methods and feature gaps are one workstream organized by domain (recorded in tasks/plan.md).

## Tickets — Phase 1 (bugs; parallel-ready, file-disjoint)

- [BUG-1 rate-limiter zero-budget hang](tickets/BUG-1-rate-limiter-zero-budget.md) — `rate_limiter.rs`
- [BUG-2 per-RPC tokens on paginated iteration](tickets/BUG-2-per-rpc-tokens.md) — `dialog.rs` `privacy.rs` `msg.rs`
- [BUG-3 contact add result parsing](tickets/BUG-3-contact-add-result.md) — `contact.rs`
- [BUG-4 login UX cluster](tickets/BUG-4-login-ux-cluster.md) — `account.rs` `client.rs` `session.rs`
- [BUG-5 config tolerance](tickets/BUG-5-config-tolerance.md) — `config.rs`
- [BUG-6 stale-cache evict-retry-hint](tickets/BUG-6-stale-cache-retry.md) — `entities.rs`
- [BUG-7 upload flood keys + name-split](tickets/BUG-7-upload-flood-namesplit.md) — `msg.rs` `profile.rs`

## Tickets — Phase 2 (capabilities; blocking edges)

Frontier order (respect deps): CAP-1 → {CAP-3, CAP-4, CAP-5}; CAP-2 independent; CAP-6..CAP-16 independent of CAP-1.
Blocking: CAP-3 blocks nothing but shares msg.rs send path with CAP-2/CAP-4 — serialize those sequentially at merge time.
Full list: see `tasks/plan.md` index; ticket files `tickets/CAP-*.md`.

## Not yet specified

- macOS CI job (carried from previous effort): release.yml ships mac binaries ci.yml never tests.
- Live verification checklist expansion for new network features (needs real sessions; user-assisted).
- `toml 0.8→1.x` dependency bump effort.

## Out of scope

- Matrix `later` rows: polls, effects, checklists, translate, transcribe, ai-compose, listen.action/user/album full typing, stories, stickers.manage, business, stars.
- Matrix `never` rows unchanged. MCP/skill (Phase 6, ask first).
- raw.rs dev-facing error text change (contract-mandated verbatim).
