# CAP-13: Contacts/profile/privacy completion

**Effort:** M · **Deps:** none · **Branch:** `feat/cap-13-identity-surface`

## Goal

Close the gaps the matrix overstates: contact remove + username in JSON; profile username/photo-delete/emoji-status; privacy 5 missing keys + chat-participant rules + allow/deny overlap rejection.

## Acceptance criteria

- [ ] `tele contact remove --user X` (DeleteContacts); contacts list rows gain `username`.
- [ ] `tele profile set --username <u|remove>` (account.updateUsername incl. USERNAME_NOT_ALLOWED error mapping); `tele profile photo --remove` (photos.deletePhotos on current); `tele profile emoji-status [--emoji <id>|--remove]` (account.updateEmojiStatus).
- [ ] Privacy: add PhoneP2P, Birthday, StarGiftsAutoSave, NoPaidMessages, SavedMusic to keys() mapping both directions; `--allow-chat <id,id>` / `--deny-chat` via InputPrivacyValueAllowChatParticipants; reject same target in allow+deny (Usage).
- [ ] Fix matrix row `profile.*` cell text to match reality after ship.
- [ ] Offline tests: key mapping tables, overlap rejection, username validation; docs updated.
