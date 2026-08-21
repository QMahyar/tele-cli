# CAP-7: Moderation depth

**Effort:** M · **Deps:** none · **Branch:** `feat/cap-7-moderation`

## Goal

Participant filtering and rights completeness.

## Acceptance criteria

- [ ] `tele chat participants --role admin|banned|kicked|recent [--search <q>]` — pass grammers `iter_participants` filter param (currently unused).
- [ ] Admin rights completion: add missing `anonymous`, `other`, `manage_topics` flags to the builder chain + presets (`chat.rs:737-750`); `--rights` CSV accepts them.
- [ ] `tele chat kick --ban --duration <secs|forever> --rights view_messages:false,...`: construct ChatBannedRights for restrict/ban with duration instead of bare friendly kick when flags present (friendly kick stays default).
- [ ] Validation pre-connect (Usage) for bad roles/rights; offline table tests for rights mapping incl. presets; docs updated.
