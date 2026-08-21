# CAP-10: Invite-link suite

**Effort:** M · **Deps:** none · **Branch:** `feat/cap-10-invite-links`

## Goal

ExportChatInvite currently lives only as a bare raw entry. Full management: `messages.{ExportChatInvite(with options), GetExportedChatInvite, SearchExportedChatInvites, GetChatInviteImporters, EditExportedChatInvite, DeleteRevokedExportedChatInvite, HideChatJoinRequest}`.

## Acceptance criteria

- [ ] `tele chat invite --chat X` exports default link (moves raw arm into friendly command).
- [ ] Options: `--title`, `--expire <ts|duration>`, `--usage-limit <n>`, `--request-approval bool`.
- [ ] `tele chat invite --list [--revoked] [--importers <link>]` lists links / importer rows (admin only).
- [ ] `tele chat invite --edit <link> ...` modify/revoke; `--delete-revoked` purge.
- [ ] Offline tests: arg validation, expiry parsing reuse (`parse_unixtime`), row shaping seams; docs + capabilities `chat.invite` cell updated.
