# CAP-1: Serialize enrichment (additive JSON)

**Effort:** S/M · **Deps:** none (do first — other CAP tickets build on richer JSON) · **Branch:** `feat/cap-1-serialize`

## Goal

Message and dialog JSON carry the fields machine consumers already expect from Telegram.

## Acceptance criteria

- [ ] `message_to_json` gains additive fields when present on the message: `grouped_id`, `views`, `forwards`, `edit_date`, `reply_to` (id), `via_bot`. Absent → key omitted or null, consistent with existing media-field convention.
- [ ] Dialog rows gain: `pinned` (bool), `unread_mentions`, `unread_reactions`, `unread_mark` (bool), `last_message_date`.
- [ ] Contract tests lock every new field; proptest for message_to_json extended to new fields (never panics).
- [ ] `docs/cli-contract.md` updated additively.

## Files

`src/serialize.rs`, `src/commands/dialog.rs` (row shaping), contract tests, docs.
