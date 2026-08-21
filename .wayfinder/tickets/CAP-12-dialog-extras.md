# CAP-12: Dialog extras — drafts, pin, delete semantics

**Effort:** M · **Deps:** CAP-1 conventions · **Branch:** `feat/cap-12-dialog-extras`

## Goal

Dialogs are read-mostly: no draft set/clear, no dialog pin/unpin, and `dialog delete` mislabels leave/clear as "deleted".

## Acceptance criteria

- [ ] `tele dialog draft --chat X [--text <t>|--clear]` via `messages.saveDraft` (raw or grammers set_draft if present in 0.10 — verify).
- [ ] `tele dialog pin --chat X [--unpin]` via `messages.toggleDialogPin`; optional `--order` reorder via reorderPinnedDialogs deferred if scope grows — note in docs.
- [ ] `dialog delete`: JSON gains honest per-kind fields (`left`, `cleared`) alongside additive `deleted` for compat; add `--revoke` flag → DeleteHistory revoke:true; help text corrected.
- [ ] Offline tests incl. contract additions; docs/cli-contract.md + capabilities rows updated.
