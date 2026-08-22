# CAP-15: Takeout upgrades — progress, resume, abandon

**Effort:** M · **Deps:** none · **Branch:** `feat/cap-15-takeout`

## Goal

Exports are silent for hours, resume = full re-export (truncates messages.jsonl), and there is no abandon path.

## Acceptance criteria

- [ ] stderr progress via log_line per dialogs/history page: `dialog i/N <name> msgs=<n>` style, machine-mode unaffected.
- [ ] Cursor resume: state file gains `(dialog_id, last_min_id)` checkpoints; re-run appends + skips completed dialogs; partial-file heuristic error text updated to point at automatic resume.
- [ ] `tele takeout finish --abandon` sends success:false; default finish stays success:true.
- [ ] Crash-safety: append mode + fsync-per-page acceptable (perf note in docs); offline tests for checkpoint read/write + skip logic with fixture iterators.
