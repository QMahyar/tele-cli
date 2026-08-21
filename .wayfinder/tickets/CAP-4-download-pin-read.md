# CAP-4: Download streaming + pin/read extras

**Effort:** S/M · **Deps:** none · **Branch:** `feat/cap-4-download-pin-read`

## Goal

Unused read-side grammers surface: `iter_download` (chunked streaming control), `get_pinned_message`, `unpin_all_messages`, pin notify flag, `clear_mentions`, dialog mark-unread (`messages.markDialogUnread` raw).

## Acceptance criteria

- [ ] `tele msg download --chunk-size <kb>` streams via `iter_download` (default behavior unchanged when flag absent).
- [ ] `tele msg pin --notify` passes notify=true; default stays silent (current behavior locked by test).
- [ ] `tele msg pin --show` prints current pinned message (get_pinned_message); `--all` unpins all (unpin_all_messages, confirm-gated like other destructive ops? no — unpin is low-risk, no gate).
- [ ] `tele msg read --mentions` clears mention badge only (clear_mentions); `tele dialog unread --chat X [--on|--off]` via raw markDialogUnread.
- [ ] Contract tests per flag; docs/cli-contract.md additive.

## Files

`src/commands/msg.rs`, `src/commands/dialog.rs`, `src/commands/raw.rs` (if markDialogUnread registered), docs, tests.
