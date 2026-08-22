# CAP-2: msg --topic scoping (forums end-to-end)

**Effort:** M · **Deps:** none · **Branch:** `feat/cap-2-topic-scope`

## Goal

Send/get/search inside forum topics. Today grep "topic" in msg.rs = zero matches; forums are unusable from the CLI.

## Acceptance criteria

- [ ] `tele msg send --topic <id>` passes reply-check-then-topic id into InputMessage (`topic_id` / reply-to-thread semantics per grammers 0.10 API).
- [ ] `tele msg get --topic <id>` filters history to the topic (iter_messages topic filter or raw GetHistory with topic params — verify grammers support; raw fallback allowed).
- [ ] `tele msg search --topic <id>` scopes search likewise.
- [ ] Non-forum chat + `--topic` → clear Usage error before connect.
- [ ] Offline unit tests for arg plumbing + validation; docs/cli-contract.md additive.

## Files

`src/commands/msg.rs`, possibly `raw.rs` if raw fallback needed, docs, tests.
