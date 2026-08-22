# W1-1: msg.buttons — reply markup in message JSON

**Branch:** `feat/w1-1-msg-buttons` · **Files:** `src/serialize.rs` only · **Deps:** none

## Goal

Inline keyboards / reply markup visible to machine consumers (game scripts need button text + callback_data to click later).

## Acceptance

- [ ] `message_to_json` gains additive `reply_markup` key (omitted when absent — follow existing optional-key pattern like grouped_id)
- [ ] Shape: `{"kind":"inline"|"reply"|"hide"|"force_reply","rows":[[button,...],...]}` where button objects carry `text` plus exactly one of `callback_data` (base64), `url`, `switch_inline_query`/`switch_inline_query_current_chat`, `buy`; unknown TL variants serialize as `{"text":..., "raw_kind":"<variant-name>"}` — never panic on new layer variants
- [ ] Works for every consumer of message_to_json (msg get/listen/serve/takeout share it) with zero shape change when markup absent
- [ ] Unit tests: inline keyboard fixture (callback+url mixed rows), reply keyboard kind mapping, unknown-variant fallback, absence = key omitted; extend the existing proptest if one covers message_to_json; gates green

## Boundaries

Only `src/serialize.rs`. grammers entry point: `Message::reply_markup()` returning TL `ReplyMarkup` enum — inspect vendored `tl/api.tl` for exact variant names at this layer.
