# W4-2: listen parsed events — action/user/service + poll rows

**Branch:** `feat/w4-2-listen-events` · **Files:** `src/commands/listen.rs` · **Deps:** none

## Goal

Close listen.action, listen.user, listen.service + carry the additive `poll` object into streamed rows.

## Acceptance

- [ ] `--events Service`: NewMessage carrying `MessageService` parses to an event row with additive `service_action` field (kind name from TL action enum: messageChatAddMember, messageChatJoinedByLink, messageChatDeleteMember, messagePinMessage, etc. — map common ones to friendly labels join/leave/join-invite/pin, unknown kinds keep raw variant name); composes with existing filters
- [ ] `--events ChatAction`: user typing/chat-level actions arriving as updates (inspect grammers Update enum + raw variants UpdateUserTyping/UpdateChatUserTyping at this layer); emit rows where constructible; if grammers' typed Update enum never surfaces them, document Raw as the path and say so honestly in report
- [ ] `--events UserUpdate`: Update::UserStatus/User updates → slim presence/status rows if surfaced; same honesty clause
- [ ] Streamed NewMessage/MessageEdited rows gain additive `poll` object mirroring msg.rs get/search enrichment (reuse the SAME shaping logic — extract/share rather than duplicate if a pub(crate) helper can live in serialize.rs… NOTE: serialize.rs edits allowed ONLY for moving existing msg.rs poll code to a shared home; coordinate shape exactly)
- [ ] Extend VALID_EVENTS + help text; offline tests per event kind incl. filter composition; gates green

## Boundaries

Only `src/commands/listen.rs` (+ minimal serialize.rs move for poll helper if needed — call it out).
