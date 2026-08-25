# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Added
- SERVE-PLATFORM phase 0+1: duplex control-plane hardening (two-lane executor, event seq/resync, negotiated handshake v2 + identity, ops.list self-description, confirm gate, deny_unknown_fields, poll event parity) + pipe ops across msg/dialog/topic/profile/privacy/contact/stickers/story/raw groups

## [0.5.0] - 2026-08-23

### Added
- 	ele account ttl get|set --days N (inactive-account self-destruct timer) and 	ele account delete --reason R --yes (SRP-protected, explicit --account + --yes required).
- Web-session management: sessions --web, --terminate-web HASH, --terminate-all-web, --change-flags HASH --disable-encrypted/--disable-call-requests.
- 	ele account phone --change-phone +XXX / phone --confirm-code NNN --phone-hash H (staged phone change).
- Staged login --stage resend (auth.resendCode) and --stage cancel-code (auth.cancelCode + local state clear).

- Recovery-email lifecycle for cloud password: `--confirm-email CODE`, `--resend-email`, `--cancel-email`; set/change auto-prompt for the emailed code on `EMAIL_UNCONFIRMED` (max 3 attempts) after showing the masked inbox pattern. Plus `--status` (has_password / has_recovery / hint / pending_reset_date), `--reset-start` and `--decline-reset.
- 	ele account password --set and --change via local PH2 (pbkdf2-hmac-sha512 ×100000, salt1 extended with 32-byte secure_random, `PasswordKdfAlgo` + `new_password_hash = pow(g, PH2) mod p` replicated from `grammers-crypto 0.10` `src/two_factor_auth.rs:134-154`; `hint`/`recovery_email` ride same `account.UpdatePasswordSettings` call, no extra RPC; `--remove` remains via SRP proof). Dry-run returns honest `would` + `hint`/`recovery_email` booleans and never prompts.

## [0.4.0] - 2026-08-22

### Added

Messaging:
- `msg send --file` is repeatable: 2-10 paths send as one album (`client.send_album`), returning `{"album": [...]}`.
- `msg send --media-ttl <secs>` sets an auto-destruct timer on sent media.
- `msg send --thumbnail <path>` attaches a custom thumbnail to single-document uploads (reuses upload path guards).
- `msg send --url <url> --kind photo|document` uploads remote media by URL (`photo_url`/`document_url`).
- `msg send --copy-from <chat> --copy-id <id>` re-sends an existing message's media without the forward header.
- `msg send --schedule online` schedules delivery for when the peer comes online.
- `msg send --topic <id>` posts into a forum topic.
- `msg search --global` searches across all dialogs (`messages.searchGlobal`); `--chat` becomes optional with it.
- `msg pin --show` prints the current pinned message; `--all` unpins all; `--notify` pins with notification.
- `msg read --mentions` clears only the mention badge.
- `msg download --chunk-size-kb <4-512>` streams via chunked `iter_download`.
- Message JSON gains additive `grouped_id`, `views`, `forwards`, `edit_date`, `reply_to`, `via_bot` when present.

Chats:
- `chat participants --role admin|banned|kicked|recent` and `--search Q` (server-side filters).
- Admin rights completed: `anonymous`, `other`, `manage_topics` join the `--rights` CSV and presets.
- `chat kick --ban --duration <secs|forever> [--rights CSV]` builds full banned-rights restrictions.
- `tele chat settings` reads slow-mode/signatures/pre-history/join-request/noforwards/linked-chat and applies toggles available in the API layer.
- `tele chat edit [--title] [--about] [--photo PATH|remove]` edits existing chats; `tele chat link [--to CHANNEL|remove]` manages discussion-group links.
- `tele chat invite` grows into a suite: export with options (`--title/--expire/--usage-limit/--request-approval`), `--list [--revoked|--importers LINK]`, `--edit LINK [+--revoke]`, `--delete-revoked`.
- `chat admin-log` rows gain `actor {id,name}` and old/new value payloads across event kinds; new filters `--admin/--search/--events/--since/--until`.

Topics:
- `tele topic close|reopen|edit|delete|pin`; `topic list` rows gain `closed` + `pinned`.

Dialogs:
- `tele dialog draft --chat X [--text T|--clear]` sets or clears drafts.
- `tele dialog pin --chat X [--unpin]` pins/unpins dialogs.
- `dialog delete` reports honest per-kind `left`/`cleared` semantics and gains `--revoke`.
- Dialog list rows gain `pinned`, `unread_mark`, `unread_mentions`, `unread_reactions`, `last_message_date`.

Contacts/profile/privacy:
- `tele contact remove --user X`; contact rows carry `username`.
- `profile set --username <u|remove>`; `tele profile photo --remove`; `tele profile emoji-status [--emoji ID|--remove]`.
- Privacy covers all 14 layer keys (adds PhoneP2P, Birthday, StarGiftsAutoSave, NoPaidMessages, SavedMusic); `--allow-chat`/`--deny-chat` target chat participant lists; allow/deny overlap is rejected pre-connect.

Listen/takeout:
- `listen --events Gap` emits a synthetic loss marker when update-state jumps are detected.
- `listen --events Album` coalesces consecutive grouped messages into one Album row.
- DM/basic-group deletions now match under `--chat` via a bounded observed-peer map.
- `takeout export` prints human-mode progress lines and supports cursor-based resume after interruption; `takeout finish --abandon` ends the session as unsuccessful.

Raw/auth:
- `tele raw` registry grows from 6 to 18 methods (channels.GetFullChannel, users.GetUsers, messages.GetHistory/Search/GetScheduledHistory/GetMessagesViews/ReadReactions/ReadMentions/GetDialogUnreadMarks, account.GetAuthorizations/SetAuthorizationTTL, contacts.DeleteByPhones). Mutating entries require explicit `--account`.
- `account login --show-token`: prints the raw `tg://login?token=…` URI to stderr even when stderr is redirected to a log. Without the flag the URI is printed only on an interactive terminal.
- RPC errors in `--json` envelopes carry additive `code` and `name` keys — scripts should match on these instead of parsing the human `message` string.
- Message objects may carry additive `media_kind` and `media_label` fields alongside the legacy colon-joined `media` string.
- `account login`: `TELE_PHONE` env var supplies the phone for code login; invalid codes and wrong 2FA passwords retry in-process (3 attempts) instead of forcing a fresh login; QR login gained `--qr-timeout-secs` (default 300) and tolerates transient stream errors.
- Failed logins clean up phantom session files so `account list` stays truthful.

### Fixed
- A zero or negative `rpc_per_minute` budget no longer stalls every command for that account (treated as unlimited).
- Paginated iterations (dialog list, msg get, privacy get) consume rate-limiter tokens per fetched page, honoring ADR-008 for large limits.
- `contact add` parses the RPC result: additive `contact`/`mutual` fields, honest failure when privacy blocks the add, warn on silent rename-overwrite (was always `"added": true`).
- Aborted logins no longer leave phantom sessions that fanout then fails on.
- Unknown keys inside `[accounts.X]` TOML tables survive config rewrites; an empty per-account `[accounts.X.proxy]` table falls back to the global proxy instead of erroring.
- Stale cached access hashes fall through to uncached resolution and append the peer-cache hint on final failure; corrupt cache rows log a warning instead of failing silently.
- Upload-time FLOOD_WAIT now carries `code`/`name` JSON keys like the send path (the old mapping was dead code — `io::Error::source()` never exposed the wrapped RPC error).
- `profile set --name "John"` no longer wipes an existing last name; first/last names are capped at 64 chars and bio at 140 client-side.
- Peer-resolution failures misreported as "request error: dropped (cancelled)" are translated to an actionable hint about the peer cache.
- Oversized numeric values in `raw --args` and `--chat` targets fail with a usage error instead of a silent i32 wraparound.
- `listen --raw` help text corrected: raw TL updates are emitted *in addition to* the parsed event allowlist, not instead of it.

### Changed
- `msg send --file` is repeatable (was single-value).
- Broken stdout pipes exit 0 silently (`tele chat stats | head` no longer panics with exit code 101); stderr write failures are ignored.
- Ctrl+C structurally aborts pending per-account tasks instead of waiting for in-flight RPCs to finish.
- Mixed usage errors + Telegram failures now exit 1 uniformly across fanout commands and `tele listen` (previously `listen` could exit 2).
- Mutually exclusive flag pairs (`--json`/`--jsonl`, `--promote`/`--demote`, `--preset`/`--rights`) are rejected by clap at parse time.
- Stale `.session.lock` marker files persist after disconnect (supersedes 0.3.1's removal behavior); session exclusivity is guarded by the OS-level file lock, not the marker.
- `dialog archive` / `dialog delete` validate `--chat` before connecting, so missing targets exit 1 (usage) instead of 3.
- Config-related errors show the leaf filename only, not the full app-data path.

### Security
- Windows app-data directories and session files carry explicit owner-only DACLs (protected from inheritance) instead of relying on profile defaults.
- Upload guard refuses private-key material (`id_rsa` family basenames, `*.pem`, `*.key`, `*.p12`, `*.pfx`, `*.kdbx`, `.netrc`, `.git-credentials`, bare `credentials`).
- A failed config-file write can no longer clobber the previous config (tmp file is written and verified first).
- A failed `.env` restriction is warned about at startup; SQLite sidecars are permission-restricted like sessions.
- The 2FA password prompt no longer echoes typed characters on Windows terminals.

### Removed
- Never-wired flood-cooldown API in the per-account rate limiter; FloodWait/SlowModeWait handling is unchanged (grammers AutoSleep retry policy).

## [0.3.1] - 2026-08-18

### Added
- `msg send --no-preview`: disables link previews on the live send; the effective value is shown in `--dry-run` output. Conflicts with `--file`.
- `account list` honors `--account`/`--tag` filters; the `--json` envelope reports the filtered `accounts` set.

### Changed
- `tele listen` and `tele takeout start|export|finish` now require an explicit `--account`/`--tag` (or `all`) instead of silently acting on every session.
- Multi-account human output labels each account's table with `== name ==` so fanout results are attributable.
- `--parallel` help text and docs aligned to the real range (1–32).
- `--jsonl` documented: one-shot commands emit a single envelope line; only `tele listen` streams one record per event.
- `--config <dir>` now fails with a clear error instead of misreading a directory as a config file.

### Fixed
- Empty `--chat` values rejected with a usage error across msg/chat commands.
- `profile set --name ""` (or whitespace-only) rejected; `--bio ""` remains allowed.
- `msg react --reaction X --remove` conflict is now a usage error.
- JSON error output no longer contains ANSI escape sequences.
- Session lock files are removed when the client disconnects (no stale `.session.lock`).

## [0.3.0] - 2026-08-18

### Added
- Admin rights granularity: `chat admin` now supports `--preset moderator|editor|admin` and `--rights "pin,invite,ban"` for fine-grained permission control
- Download force flag: `msg download` now supports `--force` to overwrite existing files
- Emoji validation: proper support for multi-codepoint emojis (family emojis, skin tone modifiers) using grapheme cluster detection

### Changed
- `msg forward` no longer accepts `--silent` (forward path unified on grammers `forward_messages`; per-message id mapping is request-ordered and confirms can no longer be silently lost).
- `msg pin` no longer accepts `--silent` (grammers pins are always silent).
- Moved `tele_invocation` re-export from account.rs to error.rs
- Refactored `TeleError::Other` into specific variants for better error handling
- Dependencies: Added `windows` crate for Windows ACL support (Windows-only)
- Dependencies: Added `unicode-segmentation` for proper emoji validation
- Dependencies: Added `indicatif` for progress indicators (prepared for future use)

### Fixed
- **Critical:** Fixed takeout pagination infinite loop bug (was checking message ID instead of message count)
- Security: Windows file permissions now properly restricted (owner-only ACLs for .session and .env files)
- Security: Mutex poisoning recovery (no longer crashes on poisoned mutex)
- Security: Windows atomic rename now handles existing target files
- Security: Phone numbers redacted in login confirmations (format: +1***456)
- Performance: Config file now loaded once per fanout (was loading twice)
- Performance: Eliminated duplicate `effective_parallel` function
- Code quality: Extracted `is_sensitive_file` helper function
- Code quality: Better error taxonomy (FileSystem, TaskPanic variants)
- Code quality: Fixed exit code truncation (now properly clamped 0-255)
- Code quality: Safe date parsing (no panic on invalid timestamps)
- Documentation: Added comprehensive docs for phone resolution side-effects
- Fixed: download/upload path guards now canonicalize the app-data guard dir too — on volumes with 8.3 short names (e.g. GitHub Actions runners, `RUNNER~1`), a download/upload path written with the short alias could bypass the guard. `resolve_for_guard` tests canonicalize their expectation.
- Fixed: `msg delete` reports the number of messages actually sent for deletion (batch sizes), not the server's PTS delta — multi-id deletes no longer report `partial` and exit 2 on the happy path.
- Fixed: `msg --file` captions now honor `--format markdown`.
- Fixed: `msg get --last` with `--offset-id` is rejected (the combination was ambiguous).

## [0.2.0] - 2026-08-17

### Changed
- Docs: removed superseded Python/Telethon spec (`docs/ideas/tele-cli.md`); folded v1 scope and non-functional requirements into ADR-007.
- Docs: fixed dangling references to deleted spec in AGENTS.md.
- Docs: replaced hardcoded test count in README with version-agnostic wording.
- CI: automated release workflow builds cross-platform binaries (win-x64, linux-x64, mac-arm64, mac-x64), generates SHA-256 checksums, and creates GitHub Releases on `v*` tag push. npm publish is gated on the `NPM_TOKEN` secret.
- Refactor: `tele raw` registry, validation, and arg metadata are now generated at build time from the vendored TL schema (`tl/api.tl`) via `grammers-tl-parser`. Adding a new raw method requires only a TL entry in the schema + a hand-written dispatch arm; the registry, validation, and help text are derived automatically. No new runtime dependencies.
- Per-account flood weights: each account now gets its own token-bucket rate limiter
  (`rpc_per_minute`) and flood cooldown (`flood_sleep_threshold`), layered under the
  account-concurrency cap (`--parallel`, default 1). A flooded account no longer
  blocks siblings.
- `--parallel` clamp raised to 1..=32 (was 1..=3); default remains 1.
- New config keys under `[accounts.<name>]`: `rpc_per_minute: f64` (token-bucket
  budget; `None` = unlimited) and `flood_sleep_threshold: u64` (per-account
  AutoSleep threshold; `None` = global default).
- ADR-008 supersedes ADR-004 for flood/parallel design.


## [0.1.2] - 2026-08-17

### Added
- Docs: matrix, contract, and README synced with implementation (test counts, completions, listen JSONL).
- Docs: Windows permission model documented (relies on user-profile ACLs).
- Test: completions output, chat stats JSON shape, QR base64 encoding (net +9 tests; suite now 578: 514 unit + 44 contract + 20 selection).

### Fixed
- Fixed: --dry-run payloads now include the command's own argument keys (additive JSON).
- Fixed: chat stats and takeout start --dry-run rows now carry their argument keys (additive JSON).
- Fixed: QR login fallback warns when the one-time token is printed to a non-terminal.
- Fixed: `set_flags` logging tests serialize on the shared test lock (they mutate the process-global log level); the config clamp test writes distinct file contents so the config cache cannot serve a stale hit.
- Fixed: help text says admin-log; out-of-range --parallel now warns.
- usage errors now emit the JSON error envelope on stdout in machine mode (was: empty stdout, exit 1)
- `msg forward` now exits 2 (partial) when some or all chunks fail; was exit 0.
- Fixed: account remove refuses to delete a session file in use by another process.
- remove no longer errors when the session file was never created.
- Fixed: npm wrapper error message and README reference the correct scoped package.
- Fixed: takeout export writes one batch per page; listen stdout writes are backpressured.
- Fixed: uploads of config.toml are refused; uploads over 2 GiB are refused; download dirs re-checked after creation (junction TOCTOU).
- Fixed: raw messages.GetAllDrafts never dumps Debug strings into JSON output.
- Fixed: malformed phone chat targets are usage errors (exit 1); upload flood waits carry seconds in the JSON error.

## [0.1.1] - 2026-08-17

### Fixed
- `--json` command failures are wrapped in the standard envelope (`ok: false` + additive `error` field) instead of a bare stderr line
- config-load failures exit 1 on every command, not only `listen`
- `account remove --name all` and `account login --name all` are rejected with a usage error (exit 1)
- `listen --dry-run` emits one JSONL row per selected account
- numeric chat ids of uncached basic groups resolve via a chat-kind retry (fixes `request error: dropped (cancelled)` on group commands from accounts without the group cached)
- `takeout finish` wraps `FinishTakeoutSession` in `InvokeWithTakeout` (fixes `TAKEOUT_REQUIRED` and orphaned takeout sessions)
- `msg forward --silent` no longer errors after a successful RPC (extracts ids from `updatesCombined`; bounds-safe chunk indexing)
- `msg delete` reports partial deletions honestly (`deleted < requested` exits 2)
- `msg send`/`msg edit` reject empty or whitespace-only `--text` with a usage error (exit 1) before connect
- `msg send --file`/`profile set --photo` reject nonexistent paths (exit 1) before connect; upload/download path guards are case-insensitive on Windows with no raw-path fallback
- `validate_markdown` accepts URLs that merely contain `tg://user?id=`; only genuine mentions are validated
- `chat join` accepts scheme-less `t.me/...` and `telegram.me/...` invite links, and caches the joined chat's access_hash for follow-up id commands
- `chat participants` on basic groups no longer panics on members with missing user data
- `chat create --kind group` ids are resolvable via `--chat <id>` immediately after creation
- `+phone` peer resolution no longer persists a contact-list side effect
- `listen` skips `MessageEmpty` updates (no stream panic), probes `updates.GetState` before streaming (fail-fast on takeout/auth errors), exits 1 on config/credential failures, and reconnects with bounded backoff
- `takeout export` wraps `GetContacts` in `InvokeWithTakeout`
- `write_config` preserves comments and unknown keys (via `toml_edit`)
- `raw` rejects empty/whitespace `--args` values with a usage error
- empty `TELE_API_HASH=` is rejected instead of silently accepted

## [0.1.0] - 2026-08-13

### Added
- Account management: add, login (code + QR), logout, remove, status, list
- Messages: send (text, files, scheduled), get, edit, delete, forward, search, react, download, read, pin
- Chats: join, create, leave, participants, kick, admin, admin-log, stats, invite
- Dialogs: list, drafts, archive/unarchive, delete
- Topics: list, create
- Contacts: list, add, block/unblock
- Profile: get, set (name, bio, photo)
- Privacy: get, set (9 keys)
- Takeout: start, export, finish
- Listen: real-time JSONL streaming (NewMessage, MessageEdited, MessageDeleted, Raw)
- Raw TL: typed registry for supported TL methods
- Shell completions: bash, zsh, fish, powershell
- Multi-account with tag-based selection
- Parallel fan-out (1-3 accounts)
- SOCKS5 proxy support (global and per-account)
- JSON/JSONL machine output with structured envelope
- Dry-run mode for all commands
- Comprehensive test suite (268 tests)
