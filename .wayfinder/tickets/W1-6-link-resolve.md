# W1-6: kernel.link-resolve — t.me/<chat>/<id> deep links

**Branch:** `feat/w1-6-link-resolve` · **Files:** `src/entities.rs` only · **Deps:** none

## Goal

`t.me/<username>/<msgid>` (and `t.me/c/<internal>/<msgid>` private form) resolve to chat id + message id so scripts can act on links directly.

## Acceptance

- [ ] Extend `classify_target` family: new target kind carrying (peer_ref, Option<msg_id>); pure parser with exhaustive tests: username/msgid, `t.me/c/1234567/456`, trailing slashes, query strings stripped, invalid msg_id (non-numeric/zero/negative) = invalid target
- [ ] Peer-resolution path returns the parsed msg id alongside PeerId without breaking every existing caller (keep old function signature working or update call sites INSIDE entities.rs only)
- [ ] Public accessor other modules can consume next wave: e.g. `ResolvedTarget { peer: PeerId, msg_id: Option<i32> }` — design it cleanly since msg.rs integration lands later (W2)
- [ ] Existing proptest/classify_target tests extended, none broken; gates green

## Boundaries

Only `src/entities.rs`. msg.rs/dialog.rs integration is explicitly OUT of scope (later ticket owns it).
