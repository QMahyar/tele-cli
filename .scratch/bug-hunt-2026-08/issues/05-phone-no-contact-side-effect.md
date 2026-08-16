# 05 — +phone targets must not mutate the contact list

**What to build:** No command may permanently add a phone number to the account's contact list as a side effect of peer resolution. `msg send --chat +<phone>`, `contact block/unblock`, `privacy allow/deny`, `chat invite/kick`, `profile get`, `listen --chat` must resolve the number without `contacts.importContacts` persisting it. Only `tele contact add` may create a contact entry.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Offline test: phone-target resolution no longer invokes `contacts.ImportContacts` (or immediately cleans up the imported contact after extracting the user id)
- [ ] Offline test: `contact add --user +<phone>` still creates the contact (the one allowed import path)
- [ ] Offline test: phone privacy-blocked resolution still errors cleanly (`USER_NOT_FOUND`) without side effects
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` pass
- [ ] Source: docs/bug-hunt-2026-08.md findings 4.F3 / 8.H4 / 15.M2 (ImportContacts in resolve_peer, no cleanup)