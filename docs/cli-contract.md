# CLI contract

This file defines the public interface for humans and agents. The `--json` shapes and exit codes documented here are commitments: new fields may appear, but a renamed key, a removed command, or a changed exit-code meaning requires a major version. Stderr carries freeform `[level] message` log lines only (see `docs/observability.md`); machine output goes to stdout only.

## Invocation

```
tele [GLOBAL] GROUP COMMAND [ARGS]

Globals (root callback, inherited):
  --account NAME     repeatable; NAME or all
  --tag TAG          repeatable; union with --account
  --parallel N       default 1; max 32 (values outside 1..=32 exit with usage error)
  --json             machine output on stdout
  --jsonl            machine output: JSON lines (one-shot commands emit a single envelope line; only `tele listen` emits one record per event)
  --quiet / -q
  --verbose / -v     maps to log level
  --dry-run
  --config PATH
```

An empty account selection is an error, except for `tele account list` and `tele account add`.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | All selected accounts succeeded (or dry-run) |
| 1 | Usage / validation (bad flags, unknown account, bad JSON args) |
| 2 | Partial: some accounts succeeded, some failed — or an account operation partially completed (e.g. `msg delete` removed fewer than requested) |
| 3 | All selected accounts failed (Telegram / IO) |
| 4 | Auth required (not logged in, 2FA needed and not supplied) |
| 130 | Interrupted (SIGINT) |

Telegram errors never produce exit code 1. They produce 3, or 4 when login is required. When every selected account fails, precedence is: auth (4) outranks Telegram/IO (3), which outranks usage (1). Exit 1 means all failures were usage failures.

## `--json` envelope (one-shot)

In `--json` mode stdout carries exactly one JSON object, UTF-8 encoded, not pretty-printed. Logs never appear on stdout.

```json
{
  "ok": true,
  "command": "msg send",
  "dry_run": false,
  "results": [
    {
      "account": "work",
      "ok": true,
      "data": {},
      "error": null
    }
  ]
}
```

When `ok` is false, `error` carries this shape:

```json
{
  "type": "InvocationError",
  "message": "A wait of 17 seconds is required",
  "seconds": 17
}
```

`seconds` appears only on flood-wait errors (`FLOOD_WAIT` or `SLOWMODE_WAIT`, RPC 420) and carries the wait length in seconds.

RPC-backed `InvocationError` objects also carry the raw Telegram RPC identity as additive keys:

```json
{
  "type": "InvocationError",
  "message": "rpc error 400: CHAT_INVALID",
  "code": 400,
  "name": "CHAT_INVALID"
}
```

`code` and `name` appear only when the failure maps to a Telegram RPC error. Match your script on these keys, not on `message` text.

Pre-flight failures happen before any account runs. They cover usage validation, config loading and parsing, and account selection. With `--json` or `--jsonl`, they still emit one envelope on stdout: `ok` is false, `results` is empty, and a top-level `error` object carries the same fields as `results[].error`. Clap parse errors (an unknown subcommand or a missing required flag) and the `--json`/`--jsonl` conflict emit the same envelope on stdout when either flag is present.

For one-shot commands, `--jsonl` behaves exactly like `--json`: one envelope line on stdout, which is valid JSONL. Only `tele listen` emits one record per event (see below). Envelope `error.message` strings contain no ANSI escapes.

```json
{
  "ok": false,
  "command": "account list",
  "dry_run": false,
  "results": [],
  "error": {
    "type": "ConfigError",
    "message": "failed to parse config.toml: ..."
  }
}
```

Rules:

- `data` grows additively per command. Document new keys in this file when you add them.
- `account list --json` also emits a top-level `accounts` array holding the same rows as `results[].data` (each `{"name","tags","session"}`). The array duplicates the data, so consumers should read `results`.
- Telegram objects are serialized through an allowlist (`id`, `date`, `message`, `peer`, and more). Raw `api_hash`, session data, and auth keys never appear in output.
- With `--dry-run`: `ok` is true, `dry_run` is true, and no network call is made. Every dry-run `results[].data` carries additive `dry_run`, a human-readable `would` string describing the exact intended action (built from the command's argument values), and the command's own argument keys. `account add` and `tele listen` follow the same `would` convention where applicable.
- Message objects may carry `media_kind` (`photo`, `document`, `sticker`, `poll`, and more) and `media_label` (a filename, emoji, or poll question; null when the kind has none), alongside the legacy colon-joined `media` string.
- When present on the Telegram message, message objects may also carry `grouped_id`, `views`, `forwards`, `edit_date` (RFC 3339), `reply_to` (the replied-to message id), and `via_bot` (the inline bot user id). Absent keys are omitted, like the media block.
- `dialog list` rows also carry `pinned` (bool), `unread_mark` (bool), `unread_mentions`, `unread_reactions`, and `last_message_date` (RFC 3339; null when the dialog has no last message).
- `dialog drafts` keys drafts by chat id: positive for users, negated (`-chat_id` or `-channel_id`) for basic groups and channels. This matches the Telegram bare-id convention used by numeric `--chat` targets.

## `chat participants`, `chat kick`, and `chat admin`

- `chat participants --chat X` accepts the additive filters `--role admin|banned|kicked|recent` and `--search <q>` on channels and supergroups. They map onto the grammers `iter_participants` filter parameter: `ChannelParticipantsAdmins`, `ChannelParticipantsBanned{q}`, `ChannelParticipantsKicked{q}`, `ChannelParticipantsSearch{q}` for a bare search, and `ChannelParticipantsRecent` otherwise. An unknown role is a Usage error before connect. On basic groups the filters fail with a clear Usage error instead of being ignored.
- By default, `chat kick --chat X --user U` performs a plain friendly kick. With `--ban`, `--duration <secs|forever>`, or `--rights CSV`, the command builds `ChatBannedRights` through `set_banned_rights` instead (restrict or ban, with optional duration). `--duration` requires `--ban` or `--rights`. `--rights` takes comma-separated `name:true|false` pairs where `true` means the user keeps the right; names: `view_messages,send_messages,send_media,
  send_stickers,send_gifs,send_games,send_inline,embed_links,send_polls,
  change_info,invite_users,pin_messages`. Success rows keep legacy `kicked: true` and additively carry `banned` (bool), `until` (epoch seconds, only when `--duration` was given), and `restricted` (denied right names, only when rights were revoked). Dry-run rows echo `ban` plus `duration` and `rights` when present.
- `chat admin --rights CSV` additionally accepts `anonymous`, `other`, and `manage_topics`; the presets cover them too (`admin` grants everything except `anonymous`; `moderator` and `editor` include `manage_topics`). When `other` or `manage_topics` is requested, the command uses raw `channels.EditAdmin`, because the grammers builder has no setters for those flags. Otherwise it stays on the friendly `set_admin_rights` chain.

## `chat settings`

- `tele chat settings --chat X` with no toggle flags reads the current values from raw `channels.GetFullChannel`. Rows carry `slow_mode` (seconds; `0` when off), `noforwards`, `signatures`, `join_request` (from the channel object in the response; may be null when the server omits it), `pre_history_hidden`, and `linked_chat_id` (null when unset).
- Each toggle maps to a raw method: `--slow-mode <secs|off>` to `channels.toggleSlowMode` (`off` sends 0), `--signatures on|off` to `channels.toggleSignatures`, `--pre-history on|off` to `channels.togglePreHistoryHidden` (`on` hides pre-join history), and `--join-request on|off` to `channels.toggleJoinRequest` (`apply_to_invites` follows the requested state, so existing invite links require approval once you enable it).
- Success rows carry `"applied": [flag names]` in application order.
- `--noforwards on|off` fails with a Usage error before any RPC: this TL layer (the grammers vendored schema) has no toggle method for it. Read-back still reports the current value.
- On basic groups, the whole command fails with a clear message: these settings apply to channels and supergroups only. Values validate offline before connect (`slow_mode` between 0 and 3600 or `off`; strict `on|off`).

## `chat edit` and `chat link`

- `tele chat edit --chat X` requires at least one of `--title`, `--about`, `--photo`. The command trims the title and caps it at 128 chars, and trims the about text and caps it at 255 chars (`--about ""` clears the description). Success rows carry `"applied": [flag names]` in application order; dry-run rows echo the requested values.
- Titles go through raw `channels.editTitle`; basic groups use raw `messages.editChatTitle` with the bare chat id. About always uses raw `messages.editChatAbout`, which works for channels, supergroups, and basic groups (this TL layer has no `channels.editAbout`).
- `--photo <path>` reuses the msg upload path validation (sensitive basenames, the app-data dir, size caps), then uploads via `upload_file` and raw `channels.editPhoto` or `messages.editChatPhoto`. `--photo remove` reads the current photo from full chat info and deletes it via raw `photos.deletePhotos`. A chat without a photo fails with a clear error.
- `tele chat link --chat X` with no `--to` prints the current discussion link. Rows carry `linked_chat_id` (null when unlinked) from raw `channels.getFullChannel`.
- `tele chat link --chat X --to CHANNEL` links via raw `channels.setDiscussionGroup`. One side must be a broadcast channel and the other a supergroup, in either order (the command classifies both peers). `--to remove` fails with an honest Usage error before connect: this API layer has no unlink method.

## `chat invite`

- The default mode invites a user: `chat invite --chat X --user U` keeps its legacy behavior and JSON shape (`channels.InviteToChannel`, or `messages.AddChatUser` for basic groups). Omitting `--user` exports a default invite link via raw `messages.exportChatInvite`.
- Export options: `--title`, `--expire <unix-ts|RFC3339|duration>` (durations: `90s/30m/24h/7d/2w`; the value must lie in the future and is stored as epoch seconds), `--usage-limit <n>` (above 0), and `--request-approval true|false`. Success rows carry `link,title,revoked,permanent,request_needed,start_date,expire_date,
  usage_limit,usage,requested,admin_id,date`.
- `--list [--revoked] [--importers LINK]` lists links exported by this account (`messages.getExportedChatInvites`, admin_id = self) or the joiners of LINK (`messages.getChatInviteImporters`; importer rows carry `id,name,date,requested,approved_by`). `SearchExportedChatInvites` does not exist in this TL layer; `getExportedChatInvites` covers that need.
- `--edit LINK` modifies one link via raw `messages.editExportedChatInvite` with any of the export options, plus `--revoke` to revoke. At least one change is required. When Telegram replaces a permanent link, the response carries two rows (old + new).
- `--delete-revoked` purges every revoked link of this account via raw `messages.deleteRevokedExportedChatInvites`; the row reports `deleted_revoked`.
- Modes are mutually exclusive (`--user`, plain export with options, `--list`, `--edit`, `--delete-revoked`). `--revoke` requires `--edit`. `--revoked` and `--importers` require `--list`. Link options fail outside export and edit modes. All validation happens offline before any connection. The raw registry entry `tele raw messages.ExportChatInvite` stays available.

## `chat admin-log`

- `chat admin-log --chat X [--limit N]` streams raw `channels.getAdminLog` pages. Rows keep legacy keys `id,date,action` and additively gain `actor` (an object with `id` and `name`; the name resolves from the response's attached users and falls back to the numeric id).
- Action payloads gained additive depth. `change_title`, `change_about`, and `change_username` carry the previous value under a `prev_*` key plus the new value. `toggle_ban` carries `ban`/`prev_ban` (`left`, denied right names, `until_date` epoch when timed, and `rank`). `toggle_admin` carries `admin`/`prev_admin` (granted right names, `anonymous`, and `rank`). `change_photo` carries `photo`/`prev_photo` (`id`, `date`, `sizes` or `empty`). `update_pinned` and `delete_message` carry the message `id`. `join_by_invite` and `join_by_request` carry `invite_link`. `edit_message` adds `prev_text`. Also shaped: slow-mode, pre-history, and noforwards toggles, default banned rights, linked chat, exported invite delete/revoke/edit, and edit_rank. Unknown actions stay `{"kind":"other"}`.
- Filters: `--admin <user>` maps to the `admins` parameter (resolved like other user targets; `me` works). `--search <q>` maps to the server-side `q` string. `--events <csv>` maps to `channel.AdminLogEventsFilter` flags. Valid flags: join,leave,invite,ban,unban,kick,unkick,promote,demote,info,settings,pinned,
  edit,delete,group_call,invites,send,forums,sub_extend,edit_rank (unknown names are a Usage error before connect). `--since/--until <ts|RFC3339>` filter client-side on event dates (the API exposes event-id bounds only); a `--since` after `--until` fails.
- The human table columns are `id,date,actor,action`, and the action column keeps the existing char-safe 60-char truncation. JSON stays additive: no legacy key changed shape or meaning.

## `dialog draft`, `dialog pin`, `dialog delete`

- `dialog draft --chat X --text T` saves a draft via raw `messages.saveDraft`; `--clear` removes it (an empty message). `--text` and `--clear` are mutually exclusive, and passing neither is a Usage error. Success rows carry `cleared` (bool) and echo `draft` (the saved text, or `""` after a clear).
- `dialog pin --chat X [--unpin]` toggles dialog pinning via raw `messages.toggleDialogPin`. Rows carry `pinned`, reflecting the requested state. Reordering pinned dialogs (`messages.reorderPinnedDialogs`) is deferred.
- `dialog delete` reports honest per-kind outcome keys additively alongside the legacy `deleted: true`. `left` is true for channels, supergroups, and basic groups (the dialog is left). `cleared` is true for private chats (the dialog entry is removed; history stays on both sides unless `--revoke`). `--revoke` routes user chats through raw `messages.deleteHistory` with `revoke: true`; for groups and channels it has no effect. Dry-run `would` describes the leave or clear semantics.

## `account login`

- Code login takes the phone from `--phone` or, when that flag is absent, from the `TELE_PHONE` env var (trimmed; empty values are ignored). The argv-exposure warning fires only when `--phone` was used.
- An invalid code triggers a re-prompt, up to 3 attempts on the same login token. Exhausting them exits with Usage and requires a fresh `tele account login`.
- A wrong 2FA password triggers a re-prompt, up to 3 attempts (the token refreshes via `account.GetPassword` between attempts). No new SMS or code is sent.
- `--qr-timeout-secs <n>` (default 300, must be above 0) sets the overall QR-login deadline. Transient update-stream errors during QR polling are retried with backoff, up to 3 times, before the command fails. On timeout, the command fails with a clear error instead of polling forever.
- If the account was not authorized at entry and the login fails or is aborted, the command removes the session files it created (session, lock, and SQLite sidecars). `account list` therefore never shows a phantom entry.

Human mode (no `--json`) prints rich tables on stdout and uses the same exit codes.

## `msg pin`, `msg read`, `msg download`

- `msg pin --show` emits `results[].data.pinned_message` (a message object or null). `msg pin --all` emits `{"unpinned_all": true}`. Both flags are mutually exclusive with each other and with `--id` and `--unpin`.
- `msg pin --notify` pins with a member notification (the default stays silent) through the raw `messages.updatePinnedMessage` path.
- `msg read --mentions` clears only the mention badge (`{"mentions_cleared": true}`) and is mutually exclusive with `--mark-unread`.
- `msg download --chunk-size-kb <4-512, multiple of 4>` streams the media through chunked `iter_download` into the same temp+commit flow. Without the flag, the default one-shot download behaves as before.

## `msg search`

With `--global`, the search runs across all dialogs (`messages.searchGlobal`) instead of one chat. `--chat` becomes optional, dry-run `data.chat` is null, and `data.global` is true. Rows use the same message object shape.

## `contact add`

`results[].data` carries additive `contact` (bool) and `mutual` (bool), reflecting the post-add state parsed from the RPC response. When the peer's privacy settings prevent saving the contact, the account row fails with a clear error instead of reporting a false `"added": true`. The command logs a warning when the add updates the display name of an existing contact.

## `contact remove`

`tele contact remove --user X` removes X from the account's contact list via raw `contacts.DeleteContacts`. Success rows carry `{"user", "removed": true}`. The target must be a user; chat and channel peers fail with a Usage error.

## `contact list`

Rows gain additive `"username"` (a string, empty when none). The human table appends a matching `username` column; existing column order is unchanged.

## `msg send`

- `--file` is repeatable: one path sends a single media, and 2-10 paths send an album (`{"album": [message objects]}`). Albums do not support `--schedule` (including `--schedule online`) or `--thumbnail`.
- `--schedule online` schedules delivery for when the peer comes online (dry-run `data.schedule` is `0` for online).
- `--media-ttl <secs>` sets an auto-destruct timer on sent media.
- `--thumbnail <path>` attaches a custom thumbnail to single-document uploads.
- `--url <url> --kind photo|document` uploads remote media by URL instead of a local file.
- `--copy-from <chat> --copy-id <id>` re-sends an existing message's media without the forward header.
- `--topic <id>` posts into a forum topic. It is mutually exclusive with `--reply`: both set the reply-to header, and replying to the topic root lands the message in that topic. Reads scoped to a topic are available through `tele raw messages.Search` (`top_msg_id`), because grammers 0.10 exposes no topic filters on its history or search iterators.

## `msg delete`

`results[].data` carries `requested` (how many ids you asked to delete) and `deleted` (how many the server actually removed). When `deleted < requested` (already-deleted ids, other people's messages, or missing permission), the row also carries `"partial": true` and the process exits 2. `--self-only` deletes only for yourself (private chats and basic groups; rejected for channels) via `messages.deleteMessages { revoke: false }`. It is mutually exclusive with `--all`.

## `msg click`

- `tele msg click --chat X --id N [--button TEXT | --button-index N | --button-contains SUBSTRING]` clicks an inline button on a bot message (`messages.getBotCallbackAnswer`). Only callback buttons can be clicked; reply-keyboard buttons error with a `tele msg send --chat <chat> --text "…" ` hint.
- Selector precedence: `--button-index` > `--button-contains` > `--button`. The three flags are mutually exclusive (clap `conflicts_with`).
- `--button-index N` is 1-based across all rows (row-major flatten). Example: a 2×2 inline keyboard has positions 1..4.
- `--button TEXT` is an exact match (case-sensitive, then case-insensitive fallback). On miss, the error keeps `no button named …` and appends `Did you mean #i "text"? Available: [#1 "…", #2 "…"]` with real 1-based texts.
- `--button-contains SUBSTRING` is a case-insensitive substring match against button `text`. It picks the first match; on ambiguous (≥2 hits) it exits 1 with `Did you mean #i "text" or #j "text"? Available: [#1 "…", #2 "…"]` (Persian+emoji resilient, substring is lowercased on both sides). On no match it suggests the available list like `--button`.
- Dry-run (`--dry-run` or `"dry_run": true` via `msg click` serve/MCP `ClickParams.button_contains`) validates the selector and reports `would: "click button … on message N"` without network. `ClickParams` carries `button_contains` alongside `button` and `button_index` (`deny_unknown_fields`, `additionalProperties: false`).

### Bot QA recipe (Windows pwsh)

Copy-pasteable bot loop (`/start` → inspect buttons → click → watch edited message):

```pwsh
tele msg send --chat @BOT --text "/start" --json | python -m json.tool
tele msg get --chat @BOT --id 123 --json | python -m json.tool   # see reply_markup rows[].type markers
tele msg click --chat @BOT --id 123 --button-index 1 --json
tele msg click --chat @BOT --id 123 --button-data "force_sub:refresh" --json   # match decoded callback data exactly
# watch edited bot message (progress bar):
tele msg get --chat @BOT --id 123 --watch --timeout-secs 60 --json
```

`jq` alternatives (same envelope):

```bash
tele msg get --chat @BOT --id 123 --json | jq .results[0].data.messages[0].text
tele msg get --chat @BOT --id 123 --json | jq '.results[0].data.messages[0].reply_markup.rows[].buttons[]'
```

Notes:

- stdout is UTF-8 JSON; on pwsh set `chcp 65001` or `$OutputEncoding=[Console]::OutputEncoding=[Text.UTF8Encoding]::new()` if Persian mangles; prefer `target\debug\telecli.exe` over `cargo run --` for hot loops (0.5s compile tax).
- For hot loops prefer `target\debug\telecli.exe` over `cargo run --` (0.5s compile tax) — hot loops avoid the ~0.5s `cargo run` check.

## `profile set --username`

- `--username <value|remove>` sets or clears the account username via raw `account.updateUsername`. Values accept `@name`, bare `name`, or a `t.me/…` or `telegram.me/…` link; the literal value `remove` (any case) clears the username. Client-side shape validation runs before connect: 5-32 chars, letters, digits, underscore, at least one letter, no leading digit, no trailing underscore.
- Success rows carry additive `"username"`: the applied name, or `"removed"` after a clear.
- Server RPC errors map to Usage: `USERNAME_NOT_ALLOWED`, `USERNAME_INVALID` / `USERNAME_BAD_SYNTAX`, and `USERNAME_OCCUPIED`.

## `profile photo --remove`

This command removes the current profile photo. It reads the photo id from `users.getFullUser` (`full_user.profile_photo`) and calls raw `photos.deletePhotos`. It fails honestly when no photo is set. Setting a photo stays on `profile set --photo <path>`.

## `profile emoji-status`

`tele profile emoji-status [--emoji <document-id> | --remove]` sets or clears the emoji status via raw `account.updateEmojiStatus`. The TL request takes an `EmojiStatus`: `emojiStatus{document_id}` to set, `emojiStatusEmpty` to clear (this layer has no separate Input constructor). `--emoji` and `--remove` are mutually exclusive, and one of them is required. Success rows carry `{"emoji_status": <id>|null, "removed": bool}`.

## `privacy set` keys and chat rules

- The key list grows additively and currently holds 14 keys: `status profile_photo phone_number calls
  forwards chat_invite added_by_phone voice_messages about phone_p2p
  birthday star_gifts_auto_save no_paid_messages saved_music` (mapped in both directions for get/set). An unknown key exits with a Usage error listing all valid keys.
- `--allow-chat <id,id>` and `--deny-chat <id,id>` add chat-participant rules (`InputPrivacyValueAllowChatParticipants` / `InputPrivacyValueDisallowChatParticipants`). Ids must be positive. Existing base chat rules are replaced only when matching chat flags are given, and preserved otherwise (the same semantics as user rules).
- The same target on both sides fails with a Usage error before connect. Matching is normalized: case-insensitive, with leading `@` or `t.me` prefixes stripped, and numeric ids compared numerically across `--allow/--allow-chat` versus `--deny/--deny-chat`.

## Topic lifecycle commands

- `topic close`, `topic reopen`, `topic edit`, `topic delete`, and `topic pin` take `--chat <target> --topic <id>`. The id is the positive integer topic id shown by `tele topic list`. All five go through raw TL: close and reopen via `messages.EditForumTopic { closed: true/false }`, edit via `messages.EditForumTopic { title?, closed? }` (at least one of `--title` or `--closed <bool>` required), pin via `messages.updatePinnedForumTopic { pinned: true }`, and delete via `messages.deleteTopicHistory` (`top_msg_id` = topic id), which removes the whole topic history.
- `topic edit --emoji` is not offered; emoji icon changes stay deferred (M7).
- Success rows carry `{"chat", "topic", "ok": true}` plus additive `"title"` and `"closed"` on `edit`, reflecting exactly what was requested. Dry-run rows add `"would": "<action> topic <id> in chat <chat>"`.
- `topic list` rows gain additive `"closed"` and `"pinned"` booleans per topic; the human table appends matching columns (existing columns keep their order).

## `tele listen` streaming

`tele listen` always streams JSON Lines on stdout, one event per line; `--json` is accepted as a no-op for symmetry. Stdout writes are backpressured: `listen` pauses on a slow reader instead of dropping events.

```json
{"event":"NewMessage","account":"work","id":123,"chat_id":456,"text":"...","date":"2026-08-13T12:00:00+00:00"}
```

`Raw` rows (from `--events Raw`, or from `--raw`, which implies it) carry the raw update base64-encoded in a `raw` field plus a `state` object with `date`/`seq` and, depending on the message-box variant, `pts` (common and channel box), `qts` (secondary box), or `channel_id` plus `pts` (channel box):

```json
{"event":"Raw","account":"work","raw":"<base64 TL serialization>","state":{"date":123,"seq":456,"pts":42}}
```

The default event type is `NewMessage` only. `--events` is an allowlist that gates all rows, including `Raw`. An unknown event name exits 1 before connect.

Valid names: `NewMessage`, `MessageEdited`, `MessageDeleted`, `Raw`, `Album`,
`Gap`, `Service`, `ChatAction`, `UserUpdate`.

### `Album` rows

With `--events Album`, consecutive `NewMessage` updates that share a non-null `grouped_id` and the same chat coalesce into a single `Album` row. Album members never also appear as individual `NewMessage` rows. Ungrouped messages behave as before: they are emitted only when `NewMessage` is also in the allowlist.

```json
{"event":"Album","account":"work","chat_id":456,"grouped_id":9001,"ids":[1,2],"date":"2026-08-13T12:00:00+00:00","messages":[{"id":1,"text":"a"},{"id":2,"text":"b"}]}
```

`messages` holds full message payloads with the same shape as the body of a `NewMessage` row. `ids` is a convenience list in member order, and `date` is the first member's date. The album flushes about 500 ms after the last member arrives, so a straggler cannot hang the stream. An `Album` row may therefore appear up to 500 ms after other events that arrived in between.

### `Gap` rows

With `--events Gap`, `listen` tracks update sequence numbers (`pts`) per message stream: one common box, plus one box per channel. When an update reports a `pts` higher than the previously observed `pts` plus its `pts_count`, updates were dropped (the `update_queue_limit` was exceeded) or a difference fetch ended prematurely (the channel was banned, or the server had issues). A synthetic `Gap` row then precedes the next events:

```json
{"event":"Gap","account":"work","reason":"pts_jump","expected_pts":11,"observed_pts":15,"state":{"date":123,"seq":456,"pts":15}}
```

The `state` object has exactly the shape documented for `Raw` rows. Channel gaps add a top-level `channel_id`. Healed difference fetches (a backfill that replays every missed update) do not produce `Gap` rows, because no events were lost from the stream.

### Deletion matching under `--chat`

Channel deletions carry a channel id and match directly. DM and basic-group deletions (`UpdateDeleteMessages`) do not identify their chat. When `--chat` targets a user or basic group, `listen` matches them through a local id-to-peer map built from observed `NewMessage` and `MessageEdited` rows under the same filter. Consequences:

- A deletion matches only when the deleted message id was observed earlier in the same stream session. While `--chat` is active, deletions of never-seen ids are suppressed (without `--chat`, all deletion events pass through).
- The map is bounded at 10,000 entries (under 1 MB), and the oldest entries are evicted first, so very old message ids may stop matching after long sessions.

`tele listen --dry-run --json`/`--jsonl` emits one JSONL row per selected account describing the intended stream, following the `would` convention (`event` holds the configured event allowlist, comma-joined):

```json
{"event":"NewMessage","account":"work","dry_run":true,"would":"stream NewMessage updates from account work"}
```

Runtime semantics:

- Ctrl+C exits `130`; the stream task is aborted with the process.
- On stream failure, `listen` reconnects with exponential backoff (1 s to 30 s, capped at 5 consecutive attempts). An auth failure fails fast instead of retrying.
- The underlying update stream uses `catch_up: true`: after downtime, the backlog accumulated while offline is replayed before live events. Consumers that require live-only behavior must filter by `date`.

## `tele serve`

```
tele serve --account NAME [--account NAME ...] [--events NewMessage,MessageEdited] [--catch-up]
```

`tele serve` is a duplex control plane for embedding scripts. It runs as a child process that owns one account session and speaks newline-delimited JSON over stdio. Stdout carries server frames (handshake, responses, events); stdin carries driver frames (hello, requests); stderr carries the usual freeform log lines. The driving script is the supervisor: it spawns, feeds, parses, and terminates the process.

Process model:

- One or more accounts: selection resolves to at least one and at most 32 sessions, otherwise the process exits 1 before anything connects. Each account session is locked exclusively (OS-level per-account lock); other tele processes cannot open the same sessions while serve runs.
- Exclusive ownership: the standard OS-level per-account session lock applies. While `tele serve` holds the sessions, no other tele process can open them, and the reverse holds too ("session <name> is in use by another process").
- Frames are single-line JSON objects terminated by `\n`; writers must flush per line.
- EOF on stdin shuts down cleanly: queued jobs drain, pending responses are flushed to stdout, the client disconnects, and the exit code is 0.
- `--events` allowlist: `NewMessage` and `MessageEdited` (default `NewMessage`). Any other name exits 1 before connect.
- The default mode is live-only (updates from now on; no replay, no history). `--catch-up` first replays what was missed since the persisted update state.
- Stream failures reconnect automatically with exponential backoff (1 s to 30 s, up to 5 consecutive attempts); auth failures fail fast (exit 4).
- `tele serve --dry-run` validates the selection offline and prints a `Serve` row following the `would` convention; nothing connects.

### Handshake

Immediately after connecting, before reading any input, the server emits its hello:

```json
{"type":"hello","protocol":1,"min_protocol":1,"max_protocol":1,"account":"work","identity":{"user_id":1234567,"username":"work","first_name":"Work","phone_masked":"+7***456"},"last_seq":null}
```

The driver answers with its own hello to complete negotiation:

```json
{"type":"hello","protocol":1}
```

- The driver's `protocol` must be an integer inside `[min_protocol, max_protocol]` (inclusive). A valid value makes the server re-emit its hello. A missing, non-integer, or out-of-range value yields a `VersionMismatch` error envelope with no `id` (the parse never produced a request), and the driver should stop.
- `identity` describes the logged-in account: bare `user_id`, `username`, `first_name`, and `phone_masked` (masked, or null when the account has no phone on file). Secrets never appear in hello or anywhere else on the wire.
- `last_seq` is the highest event `seq` assigned in this process so far (null before the first event).
- Sending another hello later re-emits the server hello with current identity and `last_seq` values.
- After an internal stream rebuild, the server does not re-handshake; it emits a `Reconnected` event row instead.

### Multi-account serve

`tele serve --account a --account b` (or `--account all`, up to 32 accounts) serves every selected account from one process and one stdio connection. All of the above protocol rules are unchanged; the additions are:

- The hello gains an additive `accounts` array: one entry per served account, `{"name":..., "identity":{...}}`, sorted by name. The legacy `account` and `identity` fields stay and describe the first account (sorted alphabetically).
- Every account has its own connection, session lock, rate limiter, and update stream. Each account reconnects independently; a rebuild of one account does not disturb the others. Event rows carry the `account` field of the emitting account. The mutate lane stays a single ordered queue across all accounts.
- Requests may target an account by adding a top-level `"account": "<name>"` key to the params object (stripped by the server before op parsing). With one account served, omitting it keeps working exactly as before. With multiple accounts, omitting `"account"` on a routed op returns a `ServeError` naming the served accounts; an unknown name returns a `ServeError` listing the valid names.
- `stream.resync` follows the same rule: `"account"` resyncs that account only; omitted with multiple accounts resyncs every account. The response is still `{"resync":"started"}` and the server emits a `Reconnected` row per rebuilt account afterward.
- A fatal account error (auth failure, or 5 consecutive stream failures) exits the process with that error, same as single-account mode.

### Requests and correlation

```json
{"id":17,"op":"msg send","params":{"chat":"@team","text":"hi"}}
```

- `id` is a driver-chosen unsigned integer, echoed on every response for correlation.
- `op` is either a `"<group> <action>"` pair from the table below or a dotted transport op (`ping`, `ops.list`, `stream.resync`). Names mirror the CLI command each op wraps: `"msg send"` runs the same core behind `tele msg send`, with the same implementation, validation rules, and chat-target syntax (`@user`, `t.me/…` links, numeric ids, `me`, `+phone`; the phone branch wins over the numeric parse).
- `params` mirrors the command's flags in snake_case without the dashes. It must be a JSON object; omitting it means `{}`. Params parse with `deny_unknown_fields`: a typo'd key produces a `ServeError` naming the field in `error.param`. Missing required fields and wrong-typed values carry `error.param` too when the offender can be identified.
- An unknown op yields `NotImplemented`; its message lists every supported op. Prefer `ops.list` over parsing that message.

### Responses

```json
{"type":"response","id":17,"ok":true,"data":{}}
{"type":"response","id":18,"ok":false,"error":{"type":"InvocationError","message":"rpc error 420: FLOOD_WAIT (value: 17)","code":420,"name":"FLOOD_WAIT","seconds":17}}
```

`ok:false` envelopes carry the same error objects as one-shot `--json`: `type`, `message`, plus the additive `seconds`, `code`, and `name` keys documented above. `id` is omitted when the failure cannot be attributed to a request, such as a malformed input line.

Error taxonomy on the serve wire:

| `error.type` | When | Extra keys |
|---|---|---|
| `ParseError` | Input line is not valid JSON / not an object | — |
| `ServeError` | Framing or params problem: bad `id`/`op`/`params` shape, unknown field, wrong param type, non-empty params where none are allowed | `param` (offending field name, when identified) |
| `UsageError` | Command-level validation failed (identical rules and wording to the CLI flags) | — |
| `AuthError` | Session invalid or logged out; fatal to the connection | — |
| `InvocationError` | Telegram RPC or transport failure | `code`, `name` (RPC-backed); `seconds` (wait value for RPC 420: `FLOOD_WAIT`, `SLOWMODE_WAIT`, …) |
| `Timeout` | Op exceeded its lane timeout; message names the op and the limit | — |
| `ConfirmRequired` | Destructive op submitted without `"confirm":true` | `would` (the dry-run preview payload) |
| `NotImplemented` | Unknown op | message lists all supported ops |
| `VersionMismatch` | hello protocol outside the negotiated range | — |

Rare kinds from the shared error model may also surface additively: `ConfigError`, `TaskPanicError`, `Error`.

### Events

Between requests, the server polls the update stream and emits event rows on stdout. Rows reuse the `tele listen` serializers (poll and event output match): a `NewMessage` or `MessageEdited` row is the flattened message object, the same shape as a `msg get` row, with these top-level keys merged in:

```json
{"event":"NewMessage","account":"work","chat_id":456,"seq":12,"id":789,"date":"2026-08-24T10:00:00+00:00","text":"..."}
```

- `event` is one of the configured `--events` kinds. Updates outside the allowlist are consumed for state tracking but never emitted.
- `chat_id` appears when the update identifies its chat.
- `seq` is stamped on every emitted frame and counts monotonically from 1 for the lifetime of the `tele serve` process. It continues uninterrupted across internal reconnects and resyncs, and restarts at 1 only when the process restarts.
- Each successful internal stream rebuild emits a `Reconnected` row: `{"event":"Reconnected","account":"work","seq":N}`.
- Gap detection: track the last seen `seq`. A skipped number means stdout rows were lost (likewise, a hello whose `last_seq` exceeds what you have read). Send `stream.resync` to force a rebuild with catch-up replay. Replays are deduplicated server-side through a bounded (chat, message-id, pts) key with a capacity of 10,000 (oldest evicted first), so replayed updates stay invisible unless they fall outside that window.

### Transport ops

Inline ops handled by the serve loop itself (no route entry):

| op | params | `data` | notes |
|---|---|---|---|
| `ping` | ignored | `{"pong":true}` | Liveness probe. |
| `ops.list` | must be empty | `{"ops":[…]}` | Self-description, sorted by `op`. |
| `stream.resync` | must be empty | `{"resync":"started"}` | Rebuilds the update stream with catch-up; the response is sent before the rebuild starts. |

`ping` and `stream.resync` respond immediately, and so does `ops.list`; none of the three occupies an op lane. Each `ops.list` entry has the schema
`{"op","summary","group","read_only","destructive","retry_safe"}` where
`group` is the leading word of a spaced op (`msg`, `dialog`, `topic`,
`profile`, `privacy`, `contact`, `sticker`, `story`, `raw`) or `transport`
for the three inline ops. The list covers all 67 routed ops plus the 3 inline ops, so it holds 70 entries. Recount with `(Select-String -Path src\commands\*.rs -Pattern 'serve_route!\(').Count`.

### Two-lane execution and timeouts

- Mutate lane: one worker; jobs run strictly in submission order (queue depth 64).
- Read lane: two workers; read jobs run concurrently, and their responses may interleave with each other and with mutate responses (queue depth 64).
- Per-op timeouts are enforced per job. Expiry yields a `Timeout` error envelope correlated by request `id`:

| Class | Limit | Applies to |
|---|---|---|
| simple | 30s | mutating ops and short reads |
| paginated | 120s | list/search/get-style reads |
| story send | 600s | `story send` (media upload) |
| download | none | `msg download` streams until completion |
| raw | 120s | `raw` |

### Confirm gate

Destructive ops refuse to execute until the driver proves intent. Submitting one without `"confirm":true` returns a `ConfirmRequired` error whose `would` is the dry-run preview computed from the submitted params:

```json
{"type":"response","id":21,"ok":false,"error":{"type":"ConfirmRequired","message":"op msg delete is destructive and requires confirm:true","would":{"dry_run":true,"ids":[],"self_only":false,"would":"delete all messages in chat @game"}}}
```

- The destructive set today: `chat kick`, `chat leave`, `contact remove`, `dialog delete`,
  `msg delete`, `sticker remove`, `story delete`, `topic delete`.
- Resubmit the same params plus `"confirm":true` to proceed. `"confirm":false` does not unlock the gate.
- The gate applies even to `"dry_run":true` submissions of destructive ops.
- Once accepted, the `confirm` key is stripped before params parsing (it never trips `deny_unknown_fields`). Non-destructive ops strip a stray `confirm` the same way; sending `confirm` inside `params` without passing the gate is an unknown-field `ServeError`.

### Dry-run

Any routed op accepts `"dry_run":true` in its params. The request is validated and answered inline (no lane, no timeout) with the same payload shape as the CLI's `--dry-run`: `dry_run:true`, a human-readable `would` describing the exact intended action, and the operation's own argument keys. No network call is made.

### Backpressure

Intake is bounded end to end: a 64-line stdin queue and 64-job op queues. A slow consumer stalls the pipeline instead of growing memory without bound. Read stdout continuously.

### Op table (67 routes)

Lane `mutate` is the ordered lane; `read` is the concurrent lane. The hints column lists only non-default flags: `read_only` performs no state change, `destructive` sits behind the confirm gate, and `retry_unsafe` means a blind retry can duplicate an effect. An absent hint means mutating, non-destructive, or retry-safe respectively. Recount the routes with `(Select-String -Path src\commands\*.rs -Pattern 'serve_route!\(').Count`.

`contact` group:

| op | summary | lane | timeout | hints |
|---|---|---|---|---|
| `contact add` | add a contact by phone number | mutate | 30s | |
| `contact block` | block a user | mutate | 30s | |
| `contact list` | list contacts | read | 120s | read_only |
| `contact remove` | remove a contact | mutate | 30s | destructive |
| `contact unblock` | unblock a user | mutate | 30s | |

`dialog` group:

| op | summary | lane | timeout | hints |
|---|---|---|---|---|
| `dialog archive` | archive or unarchive a dialog | mutate | 30s | |
| `dialog delete` | remove a dialog from the chat list | mutate | 30s | destructive |
| `dialog draft` | save or clear a chat draft | mutate | 30s | |
| `dialog drafts` | list chats holding unsent drafts | read | 120s | read_only |
| `dialog list` | list recent dialogs | read | 120s | read_only |
| `dialog pin` | pin or unpin a dialog in the chat list | mutate | 30s | |

`msg` group:

| op | summary | lane | timeout | hints |
|---|---|---|---|---|
| `msg click` | click an inline button on a bot message | mutate | 30s | retry_unsafe |
| `msg delete` | delete a message or all my messages in a chat | mutate | 30s | destructive |
| `msg download` | download message media to disk | read | none | read_only retry_unsafe |
| `msg edit` | edit the text of an outgoing message | mutate | 30s | |
| `msg forward` | forward messages between chats | mutate | 30s | retry_unsafe |
| `msg get` | fetch messages from a chat by recency or id | read | 120s | read_only |
| `msg pin` | pin or unpin a message in a chat | mutate | 30s | |
| `msg react` | add or remove a reaction on a message | mutate | 30s | |
| `msg read` | mark a chat read up to a message | mutate | 30s | |
| `msg search` | search messages in a chat or globally | read | 120s | read_only |
| `msg send` | send a text message to a chat | mutate | 30s | retry_unsafe |
| `msg typing` | send a chat action such as typing | mutate | 30s | |
| `msg vote` | vote in a poll attached to a message | mutate | 30s | retry_unsafe |

`privacy` group:

| op | summary | lane | timeout | hints |
|---|---|---|---|---|
| `privacy get` | show one privacy setting | read | 120s | read_only |
| `privacy set` | set one privacy setting | mutate | 30s | |

`profile` group:

| op | summary | lane | timeout | hints |
|---|---|---|---|---|
| `profile emoji-status` | set or clear my emoji status | mutate | 30s | |
| `profile get` | show my profile | read | 120s | read_only |
| `profile photo` | set or clear my profile photo | mutate | 30s | |
| `profile set` | update my name or bio | mutate | 30s | |

`raw` group:

| op | summary | lane | timeout | hints |
|---|---|---|---|---|
| `raw` | invoke one raw TL method by name | mutate | 120s | retry_unsafe |

`sticker` group:

| op | summary | lane | timeout | hints |
|---|---|---|---|---|
| `sticker install` | install a sticker set | mutate | 30s | |
| `sticker list` | list installed sticker sets | read | 120s | read_only |
| `sticker remove` | uninstall a sticker set | mutate | 30s | destructive |
| `sticker search` | search sticker sets by keyword | read | 120s | read_only |
| `sticker show` | list stickers in an installed set | read | 120s | read_only |

`story` group:

| op | summary | lane | timeout | hints |
|---|---|---|---|---|
| `story delete` | delete one of my stories | mutate | 30s | destructive |
| `story list` | list stories for peers | read | 120s | read_only |
| `story pin` | pin one of my stories | mutate | 30s | |
| `story read` | mark a peer's stories as read | mutate | 30s | |
| `story send` | post a new story | mutate | 600s | |
| `story unpin` | unpin one of my stories | mutate | 30s | |

`topic` group:

| op | summary | lane | timeout | hints |
|---|---|---|---|---|
| `topic close` | close a forum topic | mutate | 30s | |
| `topic create` | create a forum topic in a channel | mutate | 30s | |
| `topic delete` | delete a forum topic | mutate | 30s | destructive |
| `topic edit` | rename or re-icon a forum topic | mutate | 30s | |
| `topic list` | list forum topics in a channel | read | 120s | read_only |
| `topic pin` | pin or unpin a forum topic | mutate | 30s | |
| `topic reopen` | reopen a closed forum topic | mutate | 30s | |

`account` group:

| op | summary | lane | timeout | hints |
|---|---|---|---|---|
| `account sessions list` | list active device sessions | read | 120s | read_only |
| `account sessions web` | list active web login sessions | read | 120s | read_only |
| `account status` | probe authorization and account-level API health | read | 120s | read_only |
| `account ttl get` | show the inactive-account self-destruct TTL | read | 120s | read_only |
| `account ttl set` | set the inactive-account self-destruct TTL | mutate | 30s | |

`chat` group:

| op | summary | lane | timeout | hints |
|---|---|---|---|---|
| `chat admin` | promote or demote a chat admin | mutate | 30s | |
| `chat admin-log` | list recent chat admin log events | read | 120s | read_only |
| `chat create` | create a group, supergroup, or channel | mutate | 30s | |
| `chat edit` | edit chat title, about, or photo | mutate | 30s | |
| `chat invite` | invite users or manage invite links | mutate | 30s | |
| `chat join` | join a chat by target or invite link | mutate | 30s | |
| `chat kick` | kick, ban, or restrict a chat participant | mutate | 30s | destructive |
| `chat leave` | leave a chat or channel | mutate | 30s | destructive |
| `chat link` | show or set the discussion link of a channel | mutate | 30s | |
| `chat participants` | list chat participants | read | 120s | read_only |
| `chat requests` | list or act on pending join requests | mutate | 30s | |
| `chat settings` | update or read channel settings | mutate | 30s | |
| `chat stats` | show broadcast or megagroup stats | read | 120s | read_only |

### Serve stability

Serve frames follow the same rule as everything in this file: additive changes only. New ops appear in `ops.list` before scripts rely on them; new hint or row fields are additive. Existing `op` names, envelope shapes, and error `type` strings do not change meaning within a negotiated protocol range. Discover ops dynamically via `ops.list` instead of hardcoding the table above.

## `tele mcp`

```
tele mcp --account NAME [--read-only] [--groups g1,g2]
```

`tele mcp` is an MCP stdio server for LLM agents. It exposes every routed `tele serve` op as an MCP tool, so an agent drives one Telegram account through the same planner/runner core as the CLI, without shelling out or parsing CLI tables. It is implemented on rmcp 3.1 (the official Rust SDK) and speaks JSON-RPC 2.0 over stdio per the Model Context Protocol. An MCP client spawns the process (client config snippets below); you do not run it interactively.

Process model:

- Exactly one account: `--account NAME` is fixed for the server's lifetime, and every tool runs as that account. The standard OS-level session lock applies: while `tele mcp` holds the session, no other tele process can open it, and the reverse holds too ("session <name> is in use by another process").
- Logs stay on stderr; stdout is transport only. Startup emits
  `mcp: serving N tools (full|read-only) over stdio for account NAME`.
- `--read-only` omits every mutating tool from `tools/list`; only the 20 read-only tools of the table below are discoverable.
- `--groups msg,dialog` keeps only tools whose op group matches (comma-delimited, case-insensitive). Combined with `--read-only`, the two filters AND together. Both flags are discovery filters: a hidden tool stays invokable by its exact name, so treat them as curation, not as a hard security gate.
- EOF on stdin shuts down cleanly: the Telegram client disconnects and the exit code is 0.

### Transport and lifecycle

The rmcp stdio transport uses the legacy initialize handshake:

- The server declares protocol version `2025-11-25` and negotiates down to
  `2025-06-18` if the client offers it. The base spec revision 2026-07-28
  removed `initialize`, but no shipped client requires it yet, so tele pins
  the legacy handshake for maximum compatibility.
- Lifecycle: `initialize`, then `notifications/initialized`, then any number of
  `tools/list`, `tools/call`, and `ping`.
- Server capabilities advertise tools only; there is no `listChanged` yet.
- The `initialize` result carries `serverInfo` (`name: "tele"`, crate version) plus an instructions string naming the bound account, the universal `dry_run:true` convention, and the ConfirmRequired / `arguments.confirm=true` flow (plus a note that mutating tools are hidden when started with `--read-only`). Surface this string to the model.

### Tool naming

Op names map to tool names deterministically: spaces become underscores.

- `msg send` becomes `msg_send`, `account sessions list` becomes
  `account_sessions_list`, and `chat admin-log` becomes `chat_admin-log`.
- Every name matches `^[A-Za-z0-9_-]{1,128}$`.
- Names are unique across the table; `tools/list` returns them sorted
  alphabetically.

### Tool descriptor

Each entry in `tools/list` carries:

| field | value |
|---|---|
| `name` | mapped tool name |
| `description` | op summary; destructive ops append: "Destructive: the first call rejects with ConfirmRequired; resend with arguments.confirm=true to run it." |
| `inputSchema` | JSON Schema draft 2020-12 object: `type: object`, `additionalProperties: false`, properties derived from each op's Params structs via schemars (doc comments become property descriptions) |
| `annotations` | `readOnlyHint` = route read_only flag; `destructiveHint` = route destructive flag; `idempotentHint` = true when read-only, else the route retry_safe flag |

No separate `title` field is emitted; clients fall back to `name`. Annotations are hints for clients, not enforcement. The confirm gate below enforces.

### Safety model

- Dry-run everywhere: every tool accepts `"dry_run": true` and answers offline with the CLI-shaped would payload (`dry_run:true`, a human-readable `would`, and the operation's own argument keys). No network call is made.
- Confirm gate: destructive tools reject the first call with an `isError:true` result whose text contains the `ConfirmRequired` envelope including the computed `would` preview. Resend the same arguments plus `"confirm": true` to run (`"confirm": false` does not unlock). The destructive set today is 8 tools: `chat_kick`, `chat_leave`, `contact_remove`,
  `dialog_delete`, `msg_delete`, `sticker_remove`, `story_delete`,
  and `topic_delete`. All of them carry `destructiveHint: true`.
- `--read-only`: mutating tools are absent from `tools/list`.
- `--groups`: least-privilege curation before the first request (for example,
  `--groups msg,dialog` exposes only messaging and dialog tools).

### Error taxonomy

Only one failure class is protocol-level; everything else arrives in-band so the model can self-correct:

| failure | wire shape |
|---|---|
| unknown tool name | JSON-RPC error `-32602` invalid_params: `unknown tool X; use tools/list to see the 67 available tele tools` |
| everything else | normal response whose text carries a tele envelope; `isError: true` marks failure, `isError` absent/false on success |

Envelope `type` strings inside `isError:true` text reuse the serve taxonomy:

| envelope `type` | when | extra keys |
|---|---|---|
| `ConfirmRequired` | destructive tool called without `"confirm":true` | `would` (the dry-run preview payload) |
| `ServeError` | params failed deserialization/validation (`deny_unknown_fields`: typo'd, missing, wrong-typed keys) | `param` (offending field when identified) |
| `UsageError` | command-level validation failed (identical rules and wording to the CLI flags) | — |
| `AuthError` | session invalid or logged out; restart the server after re-login | — |
| `InvocationError` | Telegram RPC or transport failure | `code`, `name`; `seconds` on RPC 420 waits (`FLOOD_WAIT`, `SLOWMODE_WAIT`, …) |
| `ConfigError`, `TaskPanicError`, `Error` | rare kinds from the shared error model | — |

Self-correction loop: fix params per `ServeError.param`, add
`"confirm":true` after `ConfirmRequired`, wait out `seconds` on an
`InvocationError` with `name: FLOOD_WAIT`, or switch to `"dry_run":true`
to preview anything first. There are no lane timeouts over MCP; requests run one at a time in arrival order.

### Not exposed over MCP

`tele serve`'s three inline transport ops are serve-loop concepts, not MCP tools: `ping` (MCP has its own protocol-level ping), `ops.list` (replaced by `tools/list`), and `stream.resync` (no event streaming over MCP yet). They appear in neither the tool table nor `tools/list`.

### Tool table (67)

Same hints notation as the serve table: listed values mark non-defaults, and an absent hints cell means mutating, non-destructive, or retry-safe. All 67 tools are discoverable in full mode; the 20 rows carrying `read_only` survive `--read-only`.

`account` group (5):

| tool | summary | hints |
|---|---|---|
| `account_sessions_list` | list active device sessions | read_only |
| `account_sessions_web` | list active web login sessions | read_only |
| `account_status` | probe authorization and account-level API health | read_only |
| `account_ttl_get` | show the inactive-account self-destruct TTL | read_only |
| `account_ttl_set` | set the inactive-account self-destruct TTL | |

`chat` group (13):

| tool | summary | hints |
|---|---|---|
| `chat_admin` | promote or demote a chat admin | |
| `chat_admin-log` | list recent chat admin log events | read_only |
| `chat_create` | create a group, supergroup, or channel | |
| `chat_edit` | edit chat title, about, or photo | |
| `chat_invite` | invite users or manage invite links | |
| `chat_join` | join a chat by target or invite link | |
| `chat_kick` | kick, ban, or restrict a chat participant | destructive |
| `chat_leave` | leave a chat or channel | destructive |
| `chat_link` | show or set the discussion link of a channel | |
| `chat_participants` | list chat participants | read_only |
| `chat_requests` | list or act on pending join requests | |
| `chat_settings` | update or read channel settings | |
| `chat_stats` | show broadcast or megagroup stats | read_only |

`contact` group (5):

| tool | summary | hints |
|---|---|---|
| `contact_add` | add a contact by phone number | |
| `contact_block` | block a user | |
| `contact_list` | list contacts | read_only |
| `contact_remove` | remove a contact | destructive |
| `contact_unblock` | unblock a user | |

`dialog` group (6):

| tool | summary | hints |
|---|---|---|
| `dialog_archive` | archive or unarchive a dialog | |
| `dialog_delete` | remove a dialog from the chat list | destructive |
| `dialog_draft` | save or clear a chat draft | |
| `dialog_drafts` | list chats holding unsent drafts | read_only |
| `dialog_list` | list recent dialogs | read_only |
| `dialog_pin` | pin or unpin a dialog in the chat list | |

`msg` group (13):

| tool | summary | hints |
|---|---|---|
| `msg_click` | click an inline button on a bot message | retry_unsafe |
| `msg_delete` | delete a message or all my messages in a chat | destructive |
| `msg_download` | download message media to disk | read_only retry_unsafe |
| `msg_edit` | edit the text of an outgoing message | |
| `msg_forward` | forward messages between chats | retry_unsafe |
| `msg_get` | fetch messages from a chat by recency or id | read_only |
| `msg_pin` | pin or unpin a message in a chat | |
| `msg_react` | add or remove a reaction on a message | |
| `msg_read` | mark a chat read up to a message | |
| `msg_search` | search messages in a chat or globally | read_only |
| `msg_send` | send a text message to a chat | retry_unsafe |
| `msg_typing` | send a chat action such as typing | |
| `msg_vote` | vote in a poll attached to a message | retry_unsafe |

`privacy` group (2):

| tool | summary | hints |
|---|---|---|
| `privacy_get` | show one privacy setting | read_only |
| `privacy_set` | set one privacy setting | |

`profile` group (4):

| tool | summary | hints |
|---|---|---|
| `profile_emoji-status` | set or clear my emoji status | |
| `profile_get` | show my profile | read_only |
| `profile_photo` | set or clear my profile photo | |
| `profile_set` | update my name or bio | |

`raw` group (1):

| tool | summary | hints |
|---|---|---|
| `raw` | invoke one raw TL method by name | retry_unsafe |

`sticker` group (5):

| tool | summary | hints |
|---|---|---|
| `sticker_install` | install a sticker set | |
| `sticker_list` | list installed sticker sets | read_only |
| `sticker_remove` | uninstall a sticker set | destructive |
| `sticker_search` | search sticker sets by keyword | read_only |
| `sticker_show` | list stickers in an installed set | read_only |

`story` group (6):

| tool | summary | hints |
|---|---|---|
| `story_delete` | delete one of my stories | destructive |
| `story_list` | list stories for peers | read_only |
| `story_pin` | pin one of my stories | |
| `story_read` | mark a peer's stories as read | |
| `story_send` | post a new story | |
| `story_unpin` | unpin one of my stories | |

`topic` group (7):

| tool | summary | hints |
|---|---|---|
| `topic_close` | close a forum topic | |
| `topic_create` | create a forum topic in a channel | |
| `topic_delete` | delete a forum topic | destructive |
| `topic_edit` | rename or re-icon a forum topic | |
| `topic_list` | list forum topics in a channel | read_only |
| `topic_pin` | pin or unpin a forum topic | |
| `topic_reopen` | reopen a closed forum topic | |

### Backpressure

Not applicable: rmcp processes requests one at a time on the stdio transport, so there are no queue bounds to honor (contrast `tele serve`'s bounded intake).

### Client configuration

Claude Desktop reads `%APPDATA%\Claude\claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "tele": {
      "command": "tele",
      "args": ["mcp", "--account", "work"]
    }
  }
}
```

Claude Code reads project `.mcp.json` (same `mcpServers` shape as above), or you can run:

```
claude mcp add --transport stdio tele -- tele mcp --account work
```

Cursor reads `.cursor/mcp.json` in the project root:

```json
{
  "mcpServers": {
    "tele": {
      "command": "tele",
      "args": ["mcp", "--account", "work", "--read-only"]
    }
  }
}
```

Adjust `--account`, `--groups`, and the binary path for your setup. Absolute binary paths are safer in client configs.

### MCP stability

Tool names, the descriptor fields above, and the `-32602` unknown-tool shape are stable. New tools appear additively in `tools/list`; new inputSchema properties are additive; existing property types do not change meaning. Discover tools via `tools/list` instead of hardcoding the table above.

## `tele takeout`

```
tele takeout start [--contacts] [--messages] [--photos]
tele takeout export [--message-limit <n>]
tele takeout finish [--abandon]
```

All three subcommands require explicit account selection. Per-account export artifacts live under `<app data>/export/<account>/`: `contacts.json`, `messages.jsonl`, `dialogs.json`, and the state file `takeout.json`.

**Progress (human mode):** without `--json`/`--jsonl`, export reports each dialogs page (`dialogs page 2: +100 dialogs`) and each history page (`dialog 3/57 Alice msgs=120`, style `dialog i/N <name> msgs=<n>`) on stderr through the standard log line channel. Stdout stays empty until the final envelope. Machine mode emits no progress lines.

**Cursor resume:** `takeout.json` carries per-dialog checkpoints
(`{"takeout_id":1,"checkpoints":{"-1001234":4567,"42":-1}}`). A checkpoint value is the oldest message id written for that dialog; `-1` marks a completed dialog. On a re-run, `export` appends to `messages.jsonl` instead of truncating: completed dialogs are skipped, partially exported dialogs continue from their checkpoint cursor, and pages are flushed and synced to disk before their checkpoint is saved. A crash between a page write and a checkpoint save can duplicate at most one page of one dialog on resume; it never loses data. Checkpoints assume the same `--message-limit`; delete the export dir to redo an export with different settings. The failure message for partial exports points at this automatic resume.

**Finish:** the default `finish` ends the session as successful (`success:true`). `finish --abandon` sends `success:false`, so the server treats the session as abandoned. Both clear the local state file afterwards and echo the server boolean in `"finished"`.

## `tele raw`

```
tele raw TL_NAME --args JSON
```

`TL_NAME` is a registry name from `src/commands/raw.rs`. Rust TL types are static, so the registry is a typed match: each supported method has a handler arm and a documented `--args` shape. An unregistered name exits 1 with the message
`raw method not in registry; add an arm in src/commands/raw.rs`.
`--args` is a JSON object of constructor kwargs. The result lands in `results[].data`. Mutating raw calls still require `--account` and honor `--dry-run` (dry-run does not invoke).

Registry names (25):

- Read-only, no args: `messages.GetAllDrafts`, `account.GetAuthorizations`,
  `messages.GetDialogUnreadMarks`, `messages.GetAvailableEffects`.
- Peer-targeted read-only (`--args` key `chat`, same target syntax as `--chat`;
  `channels.GetFullChannel` and the stats methods use key `channel`):
  `channels.GetFullChannel`, `users.GetUsers` (`id`: array of targets),
  `messages.GetHistory`, `messages.GetScheduledHistory`,
  `messages.Search` (`q` required; `filter` one of `empty|photos|video|gif|documents|urls|audio|voice`;
  optional `from_id`, `top_msg_id`),
  `messages.GetMessagesViews` (`id`: array of message ids, `increment`: bool),
  `messages.ReadReactions`, `messages.ReadMentions` (optional `top_msg_id`),
  `stats.GetBroadcastStats`, `stats.GetMegagroupStats`.
- Other read-only: `contacts.Search` (`q` required, optional `limit`),
  `messages.TranslateText` (`to_lang`, `text`),
  `messages.TranscribeAudio` (`chat`, `msg_id`),
  `messages.ComposeMessageWithAI` (`text`).
- Mutating (explicit `--account` required, honors `--dry-run`):
  `account.UpdateProfile`, `account.SetAuthorizationTTL`
  (`authorization_ttl_days`: integer), `contacts.DeleteByPhones`
  (`phones`: array of phone numbers), `messages.ExportChatInvite`,
  `messages.AppendTodoList` (`chat`, `msg_id`, `list`),
  `messages.SendScheduledMessages` (`chat`, `id`),
  `messages.ToggleTodoCompleted` (`chat`, `msg_id`, `completed`, `incompleted`).

Shaping notes: history/search/scheduled results carry
`count`/`messages[]`/`chats`/`users`; `NotModified` adds `"not_modified": true`.
Read-reactions/read-mentions return `{pts, pts_count, offset}`;
`GetAuthorizations` returns `{authorization_ttl_days, authorizations[]}` rows with
no secrets; boolean results return `{"ok": true|false}`. Numeric `--args` fields
(`int`/`long`) may be omitted and default to `0` at dispatch (limit defaults to
10); omitted non-numeric required fields fail validation before connect. Recount the registry with:
`$s = Get-Content src/commands/raw.rs -Raw; $i = $s.IndexOf('pub const REGISTERED'); $j = $s.IndexOf('];', $i); ([regex]::Matches($s.Substring($i, $j - $i), '"[^"]+"')).Count`

## `tele completions`

```
tele completions bash|zsh|fish|powershell
```

Prints a shell completion script for the `tele` binary to stdout and exits 0. No account selection or network is involved.

## Stability

- New commands and new optional JSON keys are MINOR releases.
- A changed exit-code meaning, a renamed JSON key, or a removed command is MAJOR.
- Consumers read `CHANGELOG.md`; git log is not the changelog.
