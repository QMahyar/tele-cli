# CAP-3: Album send + InputMessage builder coverage

**Effort:** M · **Deps:** merge-sequentially with CAP-2/CAP-4 (shared msg.rs) · **Branch:** `feat/cap-3-send-media`

## Goal

Expose unused grammers send-side surface: `client.send_album`, `InputMessage::{copy_media, media_ttl, schedule_once_online, thumbnail, photo_url, document_url}`, `client.upload_stream` (stdin piping).

## Acceptance criteria

- [ ] `tele msg send --file a.jpg --file b.jpg ...` (2–10 files) uses `send_album` (grouped album); single file keeps existing path.
- [ ] `--copy-of <chat> <id>`: forward media without forward header via copy_media path.
- [ ] `--media-ttl <secs>` sets auto-destruct timer; `--schedule online` = schedule_once_online; remote fetch via `photo_url`/`document_url` (`--url <url> --kind photo|document`).
- [ ] `--stdin` + `--file-name`/mime flags pipe stdin as document upload via upload_stream.
- [ ] Custom `--thumbnail <path>` for document sends (reuse upload path validation incl. private-key blocklist).
- [ ] Validation errors are Usage exit 1 pre-connect; dry-run covers all new flags; contract tests per flag; docs updated.

## Files

`src/commands/msg.rs`, `docs/cli-contract.md`, tests.
