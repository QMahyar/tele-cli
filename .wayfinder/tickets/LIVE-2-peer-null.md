# LIVE-2: streamed row peer/sender null for brand-new peer (MED)

**Branch:** `feat/live2-peer-null` · **Files:** `src/serialize.rs` · **Deps:** none (solo — no other agent touches serialize.rs concurrently)

## Goal

Fresh DMs from a peer never seen before stream with `peer:null, sender:null` inside the nested message JSON while outer `chat_id` is correct.

## Acceptance

- [ ] Investigate `message_to_json` peer/sender resolution: why `msg.peer()` / `msg.sender()` return None for a freshly received NewMessage from account 2 → account 1 (out:false, text present, chat_id 8552872518 present). Check whether peer cache population lags behind update delivery, whether `Peer::from` mapping fails for certain `PeerId` kinds, or whether `Message::peer()` requires explicit session cache hydration
- [ ] Fix: ensure streamed rows always carry peer/sender when the outer event's `chat_id` is known; fallback to outer `chat_id`/`sender` derived from update's raw peer if message-level fields are None (never fabricate, only derive from already-available update metadata)
- [ ] Verify against live capture: `DUPLEX-SECOND` row previously had `peer:null, sender:null`; after fix, same scenario yields non-null peer/sender with correct kind/id/name where available
- [ ] Offline tests: fixture Message with peer/sender None but outer chat_id known → enriched JSON contains peer/sender; existing 1136 tests stay green; gates green

## Boundaries

Only `src/serialize.rs` (and its inline tests). Do not touch listen.rs/serve.rs — manager wires any cross-file call site if needed.
