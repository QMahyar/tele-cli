# W2-1: messaging batch — poll/typing/album-send/send-mods/click

**Branch:** `feat/w2-1-msg-batch` · **Files:** `src/commands/msg.rs`, `src/commands/raw.rs` · **Deps:** none

## Goal

Close the five remaining messaging want rows: msg.poll, msg.typing, msg.album-send, msg.send-mods, msg.click.

## Acceptance (verify every TL name against tl/api.tl at this layer before coding)

- [ ] msg.poll — render polls additively in message JSON when media is a poll (question/options/voters where present); `tele msg vote --chat X --id N --option 1[,2]` wrapping the sendVote-family method (raw if no friendly path); closing a poll = sendVote close flag if the layer has it, else document honestly
- [ ] msg.typing — `tele msg typing --chat X [--action typing|upload-photo|upload-file|cancel]` via messages.setTyping (default action typing; auto-expiring)
- [ ] msg.album-send — `--file A --file B` already sends albums via send_album (CAP-3): VERIFY what exists; only fill true gaps (e.g. >10 guard already there?) — do not duplicate shipped behavior
- [ ] msg.send-mods — `--noforwards` + `--background` on msg send (sendMessage flags at this layer; grammers builder may lack them → raw fallback arm like forward/pin did); silent already shipped
- [ ] msg.click — `tele msg click --chat X --id N --button TEXT-or-INDEX [--data B64]`: locate button in the message's reply_markup (serialize.rs now emits it — reuse shapes), then messages.getBotCallbackAnswer with the button's callback_data; alert/answer fields surfaced in output; reply-keyboard buttons are NOT clickable — honest error suggesting msg send of their text
- [ ] Mutators honor explicit --account + --dry-run gates; error taxonomy respected (exit codes per cli-contract.md)
- [ ] Offline tests per feature (validation matrices + row shaping); gates green (fmt/clippy -D warnings/cargo test)

## Boundaries

Only `src/commands/msg.rs` + `src/commands/raw.rs`. serialize.rs is merged/stable — read it, don't edit it (report if a gap forces it). Terminology lock applies (map-wants.md Notes).
