# Implementation Plan: Hardening + Capabilities (2026-08)

## Overview

Two workstreams from the 10-agent deep-dive (2026-08-21), executed systematically via wayfinder tickets, then tested and shipped:

1. **Phase 1 — Bug fixes** (7 execution tickets, all file-disjoint → fully parallel).
2. **Phase 2 — Capabilities**: unused grammers 0.10 friendly surface MERGED with feature gaps (same implementation work; e.g. `send_album` = album send). 16 tickets organized by domain.
3. **Phase 3 — Integration test pass** + live verification checklist.
4. **Phase 4 — Ship**: docs sync, CHANGELOG, matrix updates, merge to `main`, push/tag on explicit user confirm.

Tracker: wayfinder local markdown (`.wayfinder/tickets/`). Map: `.wayfinder/map.md`.

## Architecture Decisions

- **Merge grammers-unused + feature gaps** into one capability phase (user-approved direction; distinction dissolves at implementation level).
- **Branch-per-ticket off integration branch, single checkout** (user decision: no worktrees — disk + cold-build cost). Agents run sequentially; each cuts its ticket branch from the integration head, gates (clippy/fmt/test), merges back `--no-ff`. Integration branches: `fix/hardening` (Phase 1), `feat/capabilities` (Phase 2).
- JSON/CLI contract changes are **additive only** (`docs/cli-contract.md`). Matrix rows updated in the same change that ships a capability.
- Polls/effects/checklists/translate/transcribe/stories/business/stars/MCP stay `later` per matrix — NOT in scope.
- RED-first where practical: failing offline test before fix.

## Task List (index — detail lives in tickets)

### Phase 1: Bug fixes (parallel agents)
- [ ] BUG-1 rate-limiter zero-budget hang — `rate_limiter.rs`
- [ ] BUG-2 per-RPC tokens for paginated iteration — `dialog.rs`, `privacy.rs`, `msg.rs`
- [ ] BUG-3 contact add parses RPC result — `contact.rs`
- [ ] BUG-4 login UX cluster (TELE_PHONE, retries, phantom session, QR deadline, tags wipe) — `account.rs`, `client.rs`, `session.rs`
- [ ] BUG-5 config tolerance (unknown account keys, empty proxy table) — `config.rs`
- [ ] BUG-6 stale-cache evict/retry/hint — `entities.rs`
- [ ] BUG-7 upload FLOOD_WAIT keys + name-split — `msg.rs`, `profile.rs`

### Checkpoint: Phase 1
- [ ] All 7 branches merged into `fix/hardening`; clippy+fmt+full tests green
- [ ] No contract regressions (47 contract tests)

### Phase 2: Capabilities (see CAP tickets for acceptance criteria)
- [ ] CAP-1 serialize enrichment (message + dialog JSON, additive)
- [ ] CAP-2 msg --topic scoping (send/get/search)
- [ ] CAP-3 album send + InputMessage builders (copy/ttl/online/thumbnail/url/stream)
- [ ] CAP-4 download streaming + pin/read extras (iter_download, get_pinned, unpin_all, --notify, clear_mentions, mark-unread)
- [ ] CAP-5 global search (search_all_messages)
- [ ] CAP-6 topic lifecycle (close/edit/delete/pin)
- [ ] CAP-7 moderation depth (participant filters, admin rights completion, ban duration/rights)
- [ ] CAP-8 chat settings toggles (slow-mode, noforwards, signatures, pre-history)
- [ ] CAP-9 chat metadata edit (title/about/photo, linked channel)
- [ ] CAP-10 invite-link suite (export options/list/revoke/edit)
- [ ] CAP-11 admin-log depth (actor, old/new values, filters)
- [ ] CAP-12 dialog extras (draft set/clear, dialog pin/unpin, delete semantics + --revoke)
- [ ] CAP-13 contacts/profile/privacy completion (contact remove+username, profile username/photo-delete/emoji-status, privacy 5 keys + chat-participant rules)
- [ ] CAP-14 listen upgrades (Gap event, album grouping, DM-deletion matching)
- [ ] CAP-15 takeout upgrades (progress lines, cursor resume, abandon)
- [ ] CAP-16 raw registry growth (~15 read-only methods)

### Checkpoint: Phase 2
- [ ] Full suite green; new contract tests for every new flag/JSON field
- [ ] docs/capabilities.md rows flipped to done only where shipped

### Phase 3: Final verification
- [ ] cargo test + clippy -D warnings + fmt check clean at merge head
- [ ] Manual live checklist re-run (real sessions; user-assisted) for network-touching features

### Phase 4: Ship
- [ ] CHANGELOG + docs synced (capabilities.md, cli-contract.md, security.md if touched)
- [ ] Merge `fix/hardening` → `main` (fast-forward or merge commit per repo history)
- [ ] Push + tag ONLY on user confirm (AGENTS.md boundary)

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Parallel agents conflict on msg.rs (BUG-2 vs BUG-7) | Merge friction | Different regions (~908 vs ~483); trivial resolution expected |
| grammers friendly gaps discovered mid-ticket | Rework | Scout report verified method existence against 0.10.0 sources; raw fallback documented per item |
| Contract drift from many additive fields | Machine-API breakage | Every ticket adds contract tests; envelope shape locked by existing tests |
| Live-only behavior untestable offline | False confidence | Unit tests gate logic; live checklist covers RPC paths before ship |
| Worktree cold builds slow | Time | Shared CARGO_TARGET_DIR under %TEMP%\opencode |

## Open Questions

1. Push/tag at ship time — needs explicit user go (Phase 4).
2. macOS CI job (map "Not yet specified" carry-over) — runner/budget decision, not in this effort.
3. `toml 0.8→1.x` bump — defer to separate effort (breaking, touches write_config paths just hardened).
