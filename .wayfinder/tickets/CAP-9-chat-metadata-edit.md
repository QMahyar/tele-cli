# CAP-9: Chat metadata edit

**Effort:** M · **Deps:** none · **Branch:** `feat/cap-9-chat-edit`

## Goal

Edit an existing chat's title/about/photo and linked discussion group. Today title/description exist only at creation.

## Acceptance criteria

- [ ] `tele chat edit --chat X [--title <t>] [--about <a>]` via `channels.EditTitle`/`EditAbout` (basic groups: raw equivalents).
- [ ] `tele chat edit --chat X --photo <path>` reuses msg upload path validation + `channels.EditPhoto`; `--photo remove` deletes.
- [ ] `tele chat link --chat X [--to <channel>|remove]` get/set discussion linkage (`channels.GetFullChannel linked_chat` read; SetDiscussionGroup set).
- [ ] Length validation mirrors profile caps; dry-run support; offline tests for arg validation; docs updated.
