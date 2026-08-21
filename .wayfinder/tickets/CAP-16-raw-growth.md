# CAP-16: Raw registry growth

**Effort:** M · **Deps:** none · **Branch:** `feat/cap-16-raw-growth`

## Goal

Registry has 6 methods; the escape hatch is nearly shut. Growth = allowlist entries + hand-written shaping arms (build.rs generator already handles params/validation/help).

## Acceptance criteria

- [ ] Add read-only methods (shaping arms + NEEDS_PEER_RESOLVE where applicable):
  - `channels.GetFullChannel`, `users.GetUsers`, `messages.Search`, `messages.GetHistory`
  - `messages.GetScheduledHistory`, `messages.GetMessagesViews`, `messages.ReadReactions`, `messages.ReadMentions`
  - `account.GetAuthorizations`, `account.SetAuthorizationTTL` (mutating gate applies)
  - `folders.GetChatFolders`, `messages.GetDialogUnreadMarks`
  - `contacts.Search` exists — keep; add `contacts.DeleteByPhones` (mutating gate)
- [ ] Each arm: human-mode lines or table, JSON envelope verbatim, offline test for shaping with fixture response.
- [ ] Matrix rows updated: auth.session-ttl → done when SetAuthorizationTTL ships; msg.schedule-repeat stays later (send-side only).
- [ ] Registry count assertion in contract tests updated.
