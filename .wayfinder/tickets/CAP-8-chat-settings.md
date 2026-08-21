# CAP-8: Chat settings toggles

**Effort:** M · **Deps:** none · **Branch:** `feat/cap-8-chat-settings`

## Goal

Settings that require owning/admining a chat: slow mode, noforwards, signatures, join-by-request?, pre-history hide. Raw TL (`channels.{ToggleSlowMode,ToggleSignatures,ToggleJoinRequest,TogglePreHistoryHidden,ToggleNoforwards}`, `messages.SetChatAvailableReactions` out of scope).

## Acceptance criteria

- [ ] `tele chat settings --chat X [--slow-mode <secs|off>] [--noforwards on|off] [--signatures on|off] [--pre-history on|off] [--join-request on|off]`
- [ ] Read-back: `tele chat settings --chat X` (no toggles) prints current values from full-channel info (GetFullChannel raw or cached full entity fields).
- [ ] Basic groups: unsupported toggles → clear per-flag error, not silent success.
- [ ] Offline validation tests (mutually-exclusive combos, kind checks); docs + capabilities row note.
