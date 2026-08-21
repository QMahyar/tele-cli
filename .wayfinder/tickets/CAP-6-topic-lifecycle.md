# CAP-6: Topic lifecycle

**Effort:** M · **Deps:** none · **Branch:** `feat/cap-6-topic-lifecycle`

## Goal

Topics are create/list only. Add close/reopen, edit (title/emoji — emoji stays deferred per M7), delete, pin via raw TL (`messages.EditForumTopic`, `DeleteForumTopic`+history, `UpdatePinnedForumTopic`).

## Acceptance criteria

- [ ] `tele topic close|reopen --chat X --topic <id>`, `tele topic edit --title <t> [--closed bool]`, `tele topic delete --chat X --topic <id>` (delete requires topic id resolution + history cleanup per TL semantics), `tele topic pin --topic <id>`.
- [ ] Reuse topic.rs create pattern (validation, peer resolve); list gains `closed` + `pinned` fields (feeds CAP-1 conventions).
- [ ] Errors map to existing taxonomy; offline tests for arg validation + shaping seams; docs + matrix row `chat.forum` note updated.
