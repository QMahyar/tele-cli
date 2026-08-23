# W4-1: raw-registry batch — six want rows

**Branch:** `feat/w4-1-raw-batch` · **Files:** `src/commands/raw.rs` · **Deps:** none

## Goal

Ship registry coverage for: msg.schedule-repeat, msg.effect, msg.checklist, msg.translate, msg.transcribe, msg.ai-compose.

## Acceptance

- [ ] For EACH row: verify the TL method actually exists in tl/api.tl at layer 227 BEFORE coding. Add a typed registry arm (validation + response shaping consistent with existing arms). If a method does NOT exist at this layer (likely for ai-compose/checklist), do NOT fake it — leave the row unshipped and list it under "absent from layer" in your report
- [ ] Candidates to check: messages.TranslateText (+TranslateTextResult), messages.TranscribeAudio (+TranscribedAudio), schedule/repeat surface (messages.SendScheduledMessage? repeating schedules?), effects (sendMessage effect flag is input-side — registry arm may target whatever read/apply methods exist), checklists (/api/todo constructors), AI compose
- [ ] Mutating arms keep the explicit-account + dry-run gate convention
- [ ] Offline tests per added arm: validate_params happy/sad paths, response shaping fixture
- [ ] Gates green (fmt/clippy -D warnings/cargo test)

## Boundaries

Only `src/commands/raw.rs`.
