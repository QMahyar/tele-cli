# CLI contract

Public interface for humans and agents. Hyrum: `--json` shape and exit codes are commitments. Add fields; do not rename or remove without a major version. Stderr carries freeform `[level] message` lines only (see `docs/observability.md`), never machine output.

## Invocation

```
tele [GLOBAL] GROUP COMMAND [ARGS]

Globals (root callback, inherited):
  --account NAME     repeatable; NAME or all
  --tag TAG          repeatable; union with --account
  --parallel N       default 1; max 32 (values outside 1..=32 are clamped with a warning)
  --json             machine output on stdout
  --jsonl            machine output: JSON lines (one-shot commands emit a single envelope line; only `tele listen` emits one record per event)
  --quiet / -q
  --verbose / -v     maps to log level
  --dry-run
  --config PATH
```

Empty selection is an error except `tele account list|add`.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | All selected accounts succeeded (or dry-run) |
| 1 | Usage / validation (bad flags, unknown account, bad JSON args) |
| 2 | Partial: some accounts succeeded, some failed — or an account operation partially completed (e.g. `msg delete` removed fewer than requested) |
| 3 | All selected accounts failed (Telegram / IO) |
| 4 | Auth required (not logged in, 2FA needed and not supplied) |
| 130 | Interrupted (SIGINT) |

Do not overload 1 for Telegram errors.

## `--json` envelope (one-shot)

Stdout is **one JSON object** (pretty=false, UTF-8). No logs on stdout.

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

`error` when `ok` is false:

```json
{
  "type": "InvocationError",
  "message": "A wait of 17 seconds is required",
  "seconds": 17
}
```

`seconds` is present only for flood-wait errors (FLOOD_WAIT / SLOWMODE_WAIT, RPC 420) and carries the wait duration in seconds.

RPC-backed `InvocationError` errors additionally carry the raw Telegram RPC
identity as additive keys:

```json
{
  "type": "InvocationError",
  "message": "rpc error 400: CHAT_INVALID",
  "code": 400,
  "name": "CHAT_INVALID"
}
```

`code` and `name` are present only when the failure maps to a Telegram RPC
error; scripts should match on these instead of parsing `message`.

Pre-flight failures (usage validation, config load/parse, account selection)
happen before any account runs. In `--json`/`--jsonl` mode they still emit one
envelope on stdout: `ok: false`, empty `results`, and a top-level `error`
object with the same fields as `results[].error`:
Clap parse errors (unknown subcommand, missing required flag) and the
`--json`/`--jsonl` conflict also emit this envelope on stdout when `--json` or
`--jsonl` is present.

`--jsonl` for one-shot commands is identical to `--json`: exactly one envelope
line on stdout (valid JSONL). Only `tele listen` emits one record per event
(see below). Envelope `error.message` strings are stripped of ANSI escapes.

```json
{
  "ok": false,
  "command": "account list",
  "dry_run": false,
  "results": [],
  "error": {
    "type": "ConfigError",
    "message": "failed to parse C:\\path\\config.toml: ..."
  }
}
```

Rules:

- `data` is additive per command. Document new keys in this file when added.
- `account list --json` also emits a top-level `accounts` key: an array of the
  same rows as `results[].data` (each `{"name","tags","session"}`). It duplicates
  the data — consumers should prefer `results`.
- Telegram objects are serialized via an allowlist (`id`, `date`, `message`, `peer`, …). Never dump raw `api_hash`, session, or auth keys.
- `--dry-run`: `ok=true`, `dry_run=true`, no network. Every dry-run
  `results[].data` envelope carries `dry_run`, a human-readable `would`
  describing the exact intended action (using the command's argument values),
  and the command's own argument keys — all additive. `account add` and
  `tele listen` follow the same `would` convention where applicable.
- Message objects may carry `media_kind` (`photo`, `document`, `sticker`,
  `poll`, …) and `media_label` (filename / emoji / question; `null` when the
  kind has no label) alongside the legacy colon-joined `media` string.
- Message objects may also carry, when present on the Telegram message:
  `grouped_id`, `views`, `forwards`, `edit_date` (RFC 3339), `reply_to`
  (replied-to message id), and `via_bot` (inline bot user id). Absent
  enrichment keys are omitted, like the media block.
- `dialog list` rows additionally carry `pinned` (bool), `unread_mark`
  (bool), `unread_mentions`, `unread_reactions`, and `last_message_date`
  (RFC 3339; `null` when the dialog has no last message).
- `dialog drafts` keys drafts by chat id: positive for users,
  `-chat_id`/`-channel_id` (negated) for basic groups/channels — matching the
  Telegram bare-id convention used by `--chat` numeric targets.

## `chat participants` / `chat kick` / `chat admin`

- `chat participants --chat X` accepts additive `--role admin|banned|kicked|recent`
  and `--search <q>` filters (channels/supergroups; they map to the grammers
  `iter_participants` filter param: `ChannelParticipantsAdmins`,
  `ChannelParticipantsBanned{q}`, `ChannelParticipantsKicked{q}`,
  `ChannelParticipantsSearch{q}` for bare search, `ChannelParticipantsRecent`
  otherwise). Unknown roles are a Usage error before connect. On basic groups
  the filters are rejected with a clear Usage error instead of being ignored.
- `chat kick --chat X --user U` stays a plain friendly kick by default. With
  `--ban`, `--duration <secs|forever>`, or `--rights CSV`, it constructs
  `ChatBannedRights` via `set_banned_rights` (restrict / ban with optional
  duration) instead. `--duration` requires `--ban` or `--rights`. `--rights`
  takes comma-separated `name:true|false` pairs where `true` means the user
  keeps the right; names: `view_messages,send_messages,send_media,
  send_stickers,send_gifs,send_games,send_inline,embed_links,send_polls,
  change_info,invite_users,pin_messages`. Success rows keep legacy `kicked:
  true` and additively carry `banned` (bool), `until` (epoch seconds, only when
  `--duration` was given) and `restricted` (denied right names, only when
  rights were revoked). Dry-run rows echo `ban` plus `duration`/`rights` when
  present.
- `chat admin --rights CSV` additionally accepts `anonymous`, `other`, and
  `manage_topics`. Presets now cover them too (`admin` = everything except
  `anonymous`; `moderator`/`editor` include `manage_topics`). When any of
  `other`/`manage_topics` is requested, the command uses raw
  `channels.EditAdmin` (the grammers builder has no setters for those flags);
  otherwise it stays on the friendly `set_admin_rights` chain.

## `chat settings`

- `tele chat settings --chat X` with no toggle flags reads the current values
  from raw `channels.GetFullChannel`: rows carry `slow_mode` (seconds; `0`
  when off), `noforwards`, `signatures`, `join_request` (from the channel
  object in the response; may be `null` when the server omits it),
  `pre_history_hidden`, and `linked_chat_id` (`null` when unset).
- Toggles: `--slow-mode <secs|off>` via raw `channels.toggleSlowMode`
  (`off` sends 0), `--signatures on|off` via `channels.toggleSignatures`,
  `--pre-history on|off` ("on" hides pre-join history) via
  `channels.togglePreHistoryHidden`, and `--join-request on|off` via
  `channels.toggleJoinRequest` (`apply_to_invites` follows the requested
  state, so existing invite links require approval when enabled).
- Success rows carry `"applied": [flag names]` in application order.
- `--noforwards on|off` is a Usage error before any RPC: the toggle method is
  not part of this TL layer (grammers vendored schema); read-back still reports
  the current value.
- Basic groups: the whole command errors with a clear message naming that these
  settings apply to channels/supergroups only. Values validate offline
  (`slow_mode` 0–3600 or `off`; strict `on|off`) before connect.

## `chat edit` / `chat link`

- `tele chat edit --chat X` requires at least one of `--title`, `--about`,
  `--photo`. Title is trimmed and capped at 128 chars; about is trimmed and
  capped at 255 chars (`--about ""` clears the description). Success rows carry
  `"applied": [flag names]` in application order; dry-run rows echo the
  requested values.
- Title routes through raw `channels.editTitle`; basic groups use raw
  `messages.editChatTitle` with the bare chat id. About always uses raw
  `messages.editChatAbout` (works for channels/supergroups and basic groups;
  this TL layer has no `channels.editAbout`).
- `--photo <path>` reuses the msg upload path validation (sensitive basenames,
  app-data dir, size caps) then uploads via `upload_file` and raw
  `channels.editPhoto` / `messages.editChatPhoto`. `--photo remove` reads the
  current photo from full chat info and deletes it via raw
  `photos.deletePhotos`; a chat without a photo errors clearly.
- `tele chat link --chat X` with no `--to` prints the current discussion link:
  rows carry `linked_chat_id` (`null` when unlinked) from raw
  `channels.getFullChannel`.
- `tele chat link --chat X --to CHANNEL` links via raw
  `channels.setDiscussionGroup`; one side must be a broadcast channel and the
  other a supergroup (order-independent — the command classifies both peers).
  `--to remove` is an honest Usage error before connect: this API layer has no
  unlink method.

## `chat invite`

- Default mode invites a user: `chat invite --chat X --user U` keeps its legacy
  behavior and JSON shape (`channels.InviteToChannel`, `messages.AddChatUser`
  for basic groups). Omitting `--user` exports a default invite link via raw
  `messages.exportChatInvite`.
- Export options: `--title`, `--expire <unix-ts|RFC3339|duration>` (durations:
  `90s/30m/24h/7d/2w`; must be in the future; stored as epoch seconds),
  `--usage-limit <n>` (>0), `--request-approval true|false`. Success rows carry
  `link,title,revoked,permanent,request_needed,start_date,expire_date,
  usage_limit,usage,requested,admin_id,date`.
- `--list [--revoked] [--importers LINK]` lists links exported by this account
  (`messages.getExportedChatInvites`, admin_id = self) or who joined LINK
  (`messages.getChatInviteImporters`; importer rows carry
  `id,name,date,requested,approved_by`). `SearchExportedChatInvites` does not
  exist in this TL layer; `getExportedChatInvites` covers it.
- `--edit LINK` modifies one link via raw `messages.editExportedChatInvite`
  with any of the export options plus `--revoke` to revoke; at least one change
  is required. When Telegram replaces a permanent link the response carries two
  rows (old + new).
- `--delete-revoked` purges every revoked link of this account via raw
  `messages.deleteRevokedExportedChatInvites`; row reports `deleted_revoked`.
- Modes are mutually exclusive (`--user`, plain export/options, `--list`,
  `--edit`, `--delete-revoked`); `--revoke` requires `--edit`, `--revoked` /
  `--importers` require `--list`, and link options are rejected outside
  export/edit modes. All validation happens offline before any connection.
  The raw registry entry `tele raw messages.ExportChatInvite` stays available.

## `chat admin-log`

- `chat admin-log --chat X [--limit N]` streams raw `channels.getAdminLog`
  pages. Rows keep legacy keys `id,date,action` and additively gain `actor`
  (`{"id","name"}`; the name resolves from the response's attached users,
  falling back to the numeric id).
- Action payloads got additive depth: `change_title/about/username` carry
  `prev_*` + new value; `toggle_ban` carries `ban`/`prev_ban`
  (`left`,`denied` right names,`until_date` epoch when timed,`rank`);
  `toggle_admin` carries `admin`/`prev_admin` (`granted` right names,
  `anonymous`,`rank`); `change_photo` carries `photo`/`prev_photo`
  (`id`,`date`,`sizes` or `empty`); `update_pinned` and `delete_message` carry
  the message `id`; `join_by_invite` / `join_by_request` carry `invite_link`;
  `edit_message` adds `prev_text`. Also shaped: slow-mode/pre-history/
  noforwards toggles, default banned rights, linked chat, exported invite
  delete/revoke/edit, edit_rank. Unknown actions stay `{"kind":"other"}`.
- Filters: `--admin <user>` maps to the `admins` param (resolved like other
  user targets; `me` works), `--search <q>` to the server-side `q` string, and
  `--events <csv>` to `channel.AdminLogEventsFilter` flags — valid flags:
  join,leave,invite,ban,unban,kick,unkick,promote,demote,info,settings,pinned,
  edit,delete,group_call,invites,send,forums,sub_extend,edit_rank (unknown
  names are a Usage error before connect). `--since/--until <ts|RFC3339>`
  filter client-side on event dates (the API only exposes event-id bounds);
  `--since` after `--until` is rejected.
- Human table columns are `id,date,actor,action`; the action column keeps the
  existing char-safe 60-char truncation policy. JSON stays additive: no
  legacy key changed shape or meaning.

## `dialog draft` / `dialog pin` / `dialog delete`

- `dialog draft --chat X --text T` saves a draft via raw
  `messages.saveDraft`; `--clear` removes it (empty message). `--text` and
  `--clear` are mutually exclusive; passing neither is a Usage error. Success
  rows carry `cleared` (bool) and echo `draft` (the saved text, or `""` after
  a clear).
- `dialog pin --chat X [--unpin]` toggles dialog pinning via raw
  `messages.toggleDialogPin`; rows carry `pinned` reflecting the requested
  state. Reordering pinned dialogs (`messages.reorderPinnedDialogs`) is
  deferred.
- `dialog delete` reports honest per-kind outcome keys additively alongside
  the legacy `deleted: true`: `left` is true for channels/supergroups and
  basic groups (the dialog is left), `cleared` is true for private chats (the
  dialog entry is removed; history stays on both sides unless `--revoke`).
  `--revoke` routes user chats through raw `messages.deleteHistory` with
  `revoke: true`; for groups/channels it has no effect. Dry-run `would`
  describes the leave/clear semantics.

## `account login`

- Code login takes the phone from `--phone` or, when absent, the `TELE_PHONE`
  env var (trimmed; empty values ignored). The argv-exposure warning fires only
  when `--phone` was used.
- Invalid code: prompted again, up to 3 attempts on the same login token;
  exhaustion exits Usage and requires a fresh `tele account login`.
- Wrong 2FA password: re-prompted, up to 3 attempts (token refreshed via
  `account.GetPassword` between attempts); no new SMS/code is sent.
- `--qr-timeout-secs <n>` (default 300, must be > 0): overall QR-login deadline.
  Transient update-stream errors during QR polling are retried (backoff), up to
  3, before failing. On timeout the command fails with a clear error instead of
  polling forever.
- If the account was not authorized at entry and the login fails or is aborted,
  session files created during the attempt (session + lock + SQLite sidecars)
  are removed automatically, so `account list` never shows a phantom entry.

Human mode (no `--json`): Rich tables on stdout. Same exit codes.

## `msg pin` / `msg read` / `msg download`

- `msg pin --show` emits `results[].data.pinned_message` (message object or
  null). `msg pin --all` emits `{"unpinned_all": true}`. Both are mutually
  exclusive with `--id`, `--unpin`, and each other.
- `msg pin --notify` pins with a member notification (default stays silent);
  it uses the raw `messages.updatePinnedMessage` path.
- `msg read --mentions` clears only the mention badge
  (`{"mentions_cleared": true}`); mutually exclusive with `--mark-unread`.
- `msg download --chunk-size-kb <4-512, multiple of 4>` streams the media via
  chunked `iter_download` into the same temp+commit flow; without it the
  default one-shot download is unchanged.

## `msg search`

`--global` searches across all dialogs (`messages.searchGlobal`) instead of
one chat; with it, `--chat` is not required and dry-run `data.chat` is null
while `data.global` is `true`. Rows use the same message object shape.

## `contact add`

`results[].data` carries additive `contact` (bool) and `mutual` (bool)
reflecting the post-add state parsed from the RPC response. When the peer's
privacy settings prevent saving the contact, the account row fails with a
clear error instead of a false `"added": true`. A warn is logged when the add
updated the display name of an existing contact.

## `contact remove`

`tele contact remove --user X` removes X from the account's contact list via
raw `contacts.DeleteContacts`; success rows carry `{"user", "removed": true}`.
Targets a user — chat/channel peers are rejected with a Usage error.

## `contact list`

Rows gain additive `"username"` (string, empty when none). The human table
appends a matching `username` column; existing column order is unchanged.

## `msg send`

- `--file` is repeatable: one path sends a single media, 2-10 paths send an
  album (`{"album": [message objects]}`); albums do not support `--schedule`
  (including `--schedule online`) or `--thumbnail`.
- `--schedule online` schedules delivery for when the peer comes online
  (dry-run `data.schedule` is `0` for online).
- `--media-ttl <secs>` sets an auto-destruct timer on sent media.
- `--thumbnail <path>` attaches a custom thumbnail to single-document uploads.
- `--url <url> --kind photo|document` uploads remote media by URL instead of a
  local file.
- `--copy-from <chat> --copy-id <id>` re-sends an existing message's media
  without the forward header.
- `--topic <id>` posts into a forum topic (mutually exclusive with `--reply`;
  both set the reply-to header, and replying to the topic root lands the message
  in that topic). Reads scoped to a topic are available via
  `tele raw messages.Search` (`top_msg_id`), since grammers 0.10 does not expose
  topic filters on its history/search iterators.

## `msg delete`

`results[].data` carries `requested` (number asked to delete) and `deleted`
(number actually removed server-side). When `deleted < requested` (already-deleted
ids, others' messages, no permission) the row also carries `"partial": true` and
the process exits 2. `--self-only` deletes only for yourself (private chats and
basic groups; rejected for channels) via `messages.deleteMessages { revoke: false }`.
Mutually exclusive with `--all`.

## `profile set --username`

- `--username <value|remove>` sets or clears the account username via raw
  `account.updateUsername`. Values accept `@name`, bare `name`, or a
  `t.me/…` / `telegram.me/…` link; the literal value `remove` (any case)
  clears the username. Client-side shape validation (5-32 chars, letters,
  digits, underscore, must contain a letter, no leading digit or trailing
  underscore) runs before connect.
- Success rows carry additive `"username"`: the applied name, or
  `"removed"` after a clear.
- Server RPC errors map to Usage: `USERNAME_NOT_ALLOWED`,
  `USERNAME_INVALID` / `USERNAME_BAD_SYNTAX`, `USERNAME_OCCUPIED`.

## `profile photo --remove`

Removes the current profile photo: reads the photo id from
`users.getFullUser` (`full_user.profile_photo`) and calls raw
`photos.deletePhotos`. Fails honestly when no photo is set. Setting a photo
remains `profile set --photo <path>`.

## `profile emoji-status`

`tele profile emoji-status [--emoji <document-id> | --remove]` sets or clears
the emoji status via raw `account.updateEmojiStatus` (the TL request takes an
`EmojiStatus`: `emojiStatus{document_id}` to set, `emojiStatusEmpty` to
clear — there is no separate Input constructor in this layer). `--emoji` and
`--remove` are mutually exclusive and one of them is required. Success rows
carry `{"emoji_status": <id>|null, "removed": bool}`.

## `privacy set` keys and chat rules

- Key list grows additively to 14: `status profile_photo phone_number calls
  forwards chat_invite added_by_phone voice_messages about phone_p2p
  birthday star_gifts_auto_save no_paid_messages saved_music` (mapped both
  directions in get/set). Unknown keys still exit with a Usage error listing
  all valid keys.
- `--allow-chat <id,id>` / `--deny-chat <id,id>` add chat-participant rules
  (`InputPrivacyValueAllowChatParticipants` /
  `InputPrivacyValueDisallowChatParticipants`); ids must be positive.
  Existing base chat rules are replaced only when matching chat flags are
  given, preserved otherwise (same semantics as user rules).
- The same target on both sides is rejected before connect with a Usage
  error. Matching is normalized: case-insensitive, `@` / `t.me` prefixes
  stripped, numeric ids compared numerically across `--allow/--allow-chat`
  vs `--deny/--deny-chat`.

## `topic close|reopen|edit|delete|pin`

- Lifecycle commands take `--chat <target> --topic <id>` (`id` is the positive
  integer topic id shown by `tele topic list`). All go through raw TL:
  close/reopen via `messages.EditForumTopic { closed: true/false }`, edit via
  `messages.EditForumTopic { title?, closed? }` (at least one of `--title`,
  `--closed <bool>` required), pin via `messages.updatePinnedForumTopic
  { pinned: true }`, and delete via `messages.deleteTopicHistory`
  (`top_msg_id` = topic id) which removes the whole topic history.
- `topic edit --emoji` is not offered; emoji icon changes stay deferred (M7).
- Success rows carry `{"chat", "topic", "ok": true}` plus additive `"title"` /
  `"closed"` on `edit` reflecting exactly what was requested. Dry-run rows add
  `"would": "<action> topic <id> in chat <chat>"`.
- `topic list` rows gain additive `"closed"` and `"pinned"` booleans per topic;
  the human table appends matching columns (existing columns keep their order).

## Listen / stream

`tele listen` always streams **JSON Lines** on stdout, one event per line; `--json`
is accepted as a no-op for symmetry. Stdout writes are backpressured — `listen`
pauses on a slow reader instead of dropping events:

```json
{"event":"NewMessage","account":"work","id":123,"chat_id":456,"text":"...","date":"2026-08-13T12:00:00+00:00"}
```

`Raw` rows (from `--events Raw`, or `--raw` which implies it) carry the raw update
base64-encoded in a `raw` field plus a `state` object with `date`/`seq` and, per
the message-box variant, `pts` (common/channel box), `qts` (secondary box), or
`channel_id` + `pts` (channel box):

```json
{"event":"Raw","account":"work","raw":"<base64 TL serialization>","state":{"date":123,"seq":456,"pts":42}}
```

Default event type: `NewMessage` only. `--events` is an allowlist that gates all
rows, including `Raw`. Unknown event names → exit 1 before connect.

Valid names: `NewMessage`, `MessageEdited`, `MessageDeleted`, `Raw`, `Album`,
`Gap`.

### `Album` rows

With `--events Album`, consecutive `NewMessage`s that share a non-null
`grouped_id` and the same chat coalesce into a single `Album` row. Album
members never also appear as individual `NewMessage` rows; ungrouped messages
behave as before (emitted only when `NewMessage` is also in the allowlist):

```json
{"event":"Album","account":"work","chat_id":456,"grouped_id":9001,"ids":[1,2],"date":"2026-08-13T12:00:00+00:00","messages":[{"id":1,"text":"a"},{"id":2,"text":"b"}]}
```

`messages` holds full message payloads with the same shape as the body of a
`NewMessage` row; `ids` is a convenience list in member order; `date` is the
first member's date. The album flushes ~500 ms after the last member arrives,
so a straggler cannot hang the stream — an `Album` row may therefore appear up
to ~500 ms after other events that arrived in between.

### `Gap` rows

With `--events Gap`, `listen` tracks update sequence numbers (`pts`) per
message stream (common box, plus one box per channel). When an update reports
a `pts` higher than the previously observed `pts` + its `pts_count` — i.e.
updates were dropped because `update_queue_limit` was exceeded, or a
difference fetch ended prematurely (channel banned / server issues) — a
synthetic `Gap` row precedes the next events:

```json
{"event":"Gap","account":"work","reason":"pts_jump","expected_pts":11,"observed_pts":15,"state":{"date":123,"seq":456,"pts":15}}
```

The `state` object has exactly the shape documented for `Raw` rows. Channel
gaps add a top-level `channel_id`. Healed difference fetches (backfill that
replays every missed update) do **not** produce `Gap` rows, because no events
were lost from the stream.

### Deletion matching under `--chat`

Channel deletions carry a channel id and match directly. DM and basic-group
deletions (`UpdateDeleteMessages`) do not identify their chat; when `--chat`
targets a user or basic group, `listen` matches them through a local
id→peer map built from observed `NewMessage`/`MessageEdited` rows under the
same filter. Consequences:

- A deletion is matched only if the deleted message id was observed earlier in
  the same stream session; deletions of never-seen ids are suppressed while
  `--chat` is active (without `--chat` all deletion events pass through).
- The map is bounded at 10,000 entries (<1 MB); the oldest entries are evicted
  first, so very old message ids may stop matching after long sessions.

`tele listen --dry-run --json`/`--jsonl` emits one JSONL row per selected
account describing the intended stream, following the `would` convention
(`event` holds the configured event allowlist, comma-joined):

```json
{"event":"NewMessage","account":"work","dry_run":true,"would":"stream NewMessage updates from account work"}
```

Runtime semantics:

- **Ctrl+C** exits `130`; the stream task is aborted with the process.
- On stream failure, `listen` reconnects with exponential backoff (1s → 30s,
  capped at 5 consecutive attempts); an auth failure fails fast instead of
  retrying.
- The underlying update stream uses `catch_up: true`: after downtime, backlog
  accumulated while offline is replayed before live events. Consumers that
  require live-only behavior must filter by `date`.

## `tele serve`

```
tele serve --account NAME [--events NewMessage,MessageEdited] [--catch-up]
```

Duplex control plane for embedding scripts: `tele serve` is spawned as a child
process that owns **one** account session and speaks newline-delimited JSON
over stdio. Stdout carries server frames (handshake, responses, events); stdin
carries driver frames (hello, requests); stderr carries the usual freeform log
lines. The driving script is the supervisor — it spawns, feeds, parses, and
terminates the process.

Process model:

- Exactly one account: selection must resolve to a single session, otherwise
  exit 1 before anything connects.
- Exclusive ownership: the standard OS-level per-account session lock applies.
  While `tele serve` holds the session no other tele process can open it, and
  vice versa ("session <name> is in use by another process").
- Frames are single-line JSON objects terminated by `\n`; writers must flush
  per line.
- EOF on stdin shuts down cleanly: queued jobs drain, pending responses are
  flushed to stdout, the client disconnects, exit code 0.
- `--events` allowlist: `NewMessage`, `MessageEdited` (default
  `NewMessage`). Any other name exits 1 before connect.
- Default mode is live-only (updates from now on, no replay, no history);
  `--catch-up` first replays what was missed since the persisted update state.
- Stream failures reconnect automatically with exponential backoff (1s→30s, up
  to 5 consecutive attempts); auth failures fail fast (exit 4).
- `tele serve --dry-run` validates selection offline and prints a `Serve` row
  following the `would` convention; nothing connects.

### Handshake

Immediately after connecting (before reading any input) the server emits its
hello:

```json
{"type":"hello","protocol":1,"min_protocol":1,"max_protocol":1,"account":"work","identity":{"user_id":1234567,"username":"work","first_name":"Work","phone_masked":"+7***456"},"last_seq":null}
```

The driver answers with its own hello to complete negotiation:

```json
{"type":"hello","protocol":1}
```

- The driver's `protocol` must be an integer inside `[min_protocol,
  max_protocol]` (inclusive). A valid value re-emits the server hello; a
  missing/non-integer/out-of-range one yields a `VersionMismatch` error
  envelope (no `id`; the parse never produced a request) and the driver should
  stop.
- `identity` describes the logged-in account: bare `user_id`, `username`,
  `first_name`, and `phone_masked` (masked, or `null` when the account has no
  phone on file). Secrets never appear in hello or anywhere else on the wire.
- `last_seq` is the highest event `seq` assigned in this process so far
  (`null` before the first event).
- Sending another hello later re-emits the server hello with current
  identity/`last_seq` values.
- After an internal stream rebuild the server does not re-handshake; it emits
  a `Reconnected` event row instead.

### Requests and correlation

```json
{"id":17,"op":"msg send","params":{"chat":"@team","text":"hi"}}
```

- `id` is a driver-chosen unsigned integer, echoed on every response for
  correlation.
- `op` is either `"<group> <action>"` from the table below or a dotted
  transport op (`ping`, `ops.list`, `stream.resync`). Names mirror the CLI
  command they wrap (`"msg send"` ⇔ `tele msg send`) and share the same core
  implementation, validation rules, and chat-target syntax (`@user`,
  `t.me/…` links, numeric ids, `me`, `+phone` — phone branch wins over numeric
  parse).
- `params` mirrors the command's flags in snake_case without the dashes. It
  must be a JSON object; omitted means `{}`. Params parse with
  `deny_unknown_fields`: a typo'd key is a `ServeError` naming the field in
  `error.param` (missing required fields and wrong-typed values carry
  `error.param` too when the offender can be identified).
- An unknown op yields `NotImplemented`; its message lists every supported op
  — but prefer `ops.list` over parsing that message.

### Responses

```json
{"type":"response","id":17,"ok":true,"data":{}}
{"type":"response","id":18,"ok":false,"error":{"type":"InvocationError","message":"rpc error 420: FLOOD_WAIT (value: 17)","code":420,"name":"FLOOD_WAIT","seconds":17}}
```

`ok:false` envelopes carry the same error objects as one-shot `--json`
(`type`, `message`, plus the additive `seconds` / `code` / `name` keys
documented above). `id` is omitted when the failure cannot be attributed to a
request, such as a malformed input line.

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

Rare kinds from the shared error model may also surface additively:
`ConfigError`, `TaskPanicError`, `Error`.

### Events

Between requests the server polls the update stream and emits event rows on
stdout. Rows reuse the `tele listen` serializers (poll/event parity): a
`NewMessage` / `MessageEdited` row is the flattened message object — the same
shape as a `msg get` row — with these top-level keys merged in:

```json
{"event":"NewMessage","account":"work","chat_id":456,"seq":12,"id":789,"date":"2026-08-24T10:00:00+00:00","text":"..."}
```

- `event` is one of the configured `--events` kinds; updates outside the
  allowlist are consumed for state tracking but never emitted.
- `chat_id` appears when the update identifies its chat.
- `seq` is stamped on every emitted frame and counts monotonically from 1 for
  the lifetime of the `tele serve` process, continuing uninterrupted across
  internal reconnects and resyncs (it restarts at 1 only when the process is
  restarted).
- Each successful internal stream rebuild emits a `Reconnected` row:
  `{"event":"Reconnected","account":"work","seq":N}`.
- Gap detection: track the last seen `seq`; a skipped number means stdout rows
  were lost (likewise, a hello whose `last_seq` exceeds what you have read).
  Send `stream.resync` to force a rebuild with catch-up replay. Replays are
  deduplicated server-side via a bounded (chat, message-id, pts) key
  (capacity 10,000, oldest evicted first), so replayed updates stay invisible
  unless they fall outside that window.

### Transport ops

Inline ops handled by the serve loop itself (no route entry):

| op | params | `data` | notes |
|---|---|---|---|
| `ping` | ignored | `{"pong":true}` | Liveness probe. |
| `ops.list` | must be empty | `{"ops":[…]}` | Self-description, sorted by `op`. |
| `stream.resync` | must be empty | `{"resync":"started"}` | Rebuilds the update stream with catch-up; the response is sent before the rebuild starts. |

`ping` and `stream.resync` respond immediately; `ops.list` responds
immediately; none occupy an op lane. Each `ops.list` entry has the schema
`{"op","summary","group","read_only","destructive","retry_safe"}` where
`group` is the leading word of a spaced op (`msg`, `dialog`, `topic`,
`profile`, `privacy`, `contact`, `sticker`, `story`, `raw`) or `transport`
for the three inline ops. The list covers all 49 routed ops plus the 3
inline ops (52 entries).

### Two-lane execution and timeouts

- **Mutate lane**: one worker; jobs run strictly in submission order (queue
  depth 64).
- **Read lane**: two workers; read jobs run concurrently and their responses
  may interleave with each other and with mutate responses (queue depth 64).
- Per-op timeouts are enforced per job; expiry yields a `Timeout` error
  envelope correlated by request `id`:

| Class | Limit | Applies to |
|---|---|---|
| simple | 30s | mutating ops and short reads |
| paginated | 120s | list/search/get-style reads |
| story send | 600s | `story send` (media upload) |
| download | none | `msg download` streams until completion |
| raw | 120s | `raw` |

### Confirm gate

Destructive ops refuse to execute until the driver proves intent. Submitting
one without `"confirm":true` returns a `ConfirmRequired` error whose `would`
is the dry-run preview computed from the submitted params:

```json
{"type":"response","id":21,"ok":false,"error":{"type":"ConfirmRequired","message":"op msg delete is destructive and requires confirm:true","would":{"dry_run":true,"ids":[],"self_only":false,"would":"delete all messages in chat @game"}}}
```

- The destructive set today: `contact remove`, `dialog delete`,
  `msg delete`, `sticker remove`, `story delete`, `topic delete`.
- Resubmit the same params plus `"confirm":true` to proceed;
  `"confirm":false` does not unlock the gate.
- The gate applies even to `"dry_run":true` submissions of destructive ops.
- Once accepted, the `confirm` key is stripped before params parsing (it never
  trips `deny_unknown_fields`). Non-destructive ops strip a stray `confirm`
  the same way; sending `confirm` inside `params` without passing the gate is
  an unknown-field `ServeError`.

### Dry-run

Any routed op accepts `"dry_run":true` in params. The request is validated and
answered inline (no lane, no timeout) with the same payload shape as the
CLI's `--dry-run`: `dry_run:true`, a human-readable `would` describing the
exact intended action, and the operation's own argument keys. No network call
is made.

### Backpressure

Intake is bounded end to end: a 64-line stdin queue and 64-job op queues. A
slow consumer stalls the pipeline instead of growing memory without bound —
drivers should read stdout continuously.

### Op table (49 routes)

Lane `mutate` = ordered lane, `read` = concurrent lane. The hints column lists
only the non-default flags: `read_only` performs no state change,
`destructive` sits behind the confirm gate, `retry_unsafe` means a blind
retry can duplicate an effect. Absent hint ⇒ mutating / non-destructive /
retry-safe respectively.

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

### Serve stability

Serve frames follow the same rule as everything in this file: additive changes
only. New ops appear in `ops.list` before scripts rely on them; new hint or
row fields are additive; existing `op` names, envelope shapes, and error
`type` strings do not change meaning within a negotiated protocol range.
Discover ops dynamically via `ops.list` instead of hardcoding the table above.

## `tele takeout`

```
tele takeout start [--contacts] [--messages] [--photos]
tele takeout export [--message-limit <n>]
tele takeout finish [--abandon]
```

All three require explicit account selection. Per-account export artifacts live
under `<app data>/export/<account>/`: `contacts.json`, `messages.jsonl`,
`dialogs.json`, and the state file `takeout.json`.

**Progress (human mode):** without `--json`/`--jsonl`, export reports each
dialogs page (`dialogs page 2: +100 dialogs`) and each history page
(`dialog 3/57 Alice msgs=120`, style `dialog i/N <name> msgs=<n>`) on **stderr**
via the standard log line channel; stdout stays empty until the final envelope.
Machine mode emits no progress lines.

**Cursor resume:** `takeout.json` carries per-dialog checkpoints
(`{"takeout_id":1,"checkpoints":{"-1001234":4567,"42":-1}}`). A checkpoint
value is the oldest message id written for that dialog; `-1` marks a completed
dialog. On re-run, `export` appends to `messages.jsonl` instead of truncating:
completed dialogs are skipped, partially exported dialogs continue from their
checkpoint cursor, and pages are flushed + synced to disk before their
checkpoint is saved. A crash between page write and checkpoint save can
duplicate at most one page of one dialog on resume — never lose data.
Checkpoints assume the same `--message-limit`; delete the export dir to redo an
export with different settings. The failure message for partial exports points
at this automatic resume.

**Finish:** default `finish` ends the session as successful (`success:true`);
`finish --abandon` sends `success:false` so the server treats the session as
abandoned. Both clear the local state file afterwards and echo the server
boolean in `"finished"`.

## `tele raw`

```
tele raw TL_NAME --args JSON
```

`TL_NAME` is a **registry name** from `src/commands/raw.rs`. Rust TL types are
static, so the registry is a typed match: each supported method has a handler
arm and documented `--args` shape. Unregistered names exit 1 with the message
`raw method not in registry; add an arm in src/commands/raw.rs`.
`--args` is a JSON object of constructor kwargs. Result goes in `results[].data`.
Destructive raw calls still require `--account` and honor `--dry-run` (dry-run does
not invoke).

Registry names (18):

- Read-only, no args: `messages.GetAllDrafts`, `account.GetAuthorizations`,
  `messages.GetDialogUnreadMarks`.
- Peer-targeted read-only (`--args` key `chat`, same target syntax as `--chat`;
  `channels.GetFullChannel` uses key `channel`):
  `channels.GetFullChannel`, `users.GetUsers` (`id`: array of targets),
  `messages.GetHistory`, `messages.GetScheduledHistory`,
  `messages.Search` (`q` required; `filter` one of `empty|photos|video|gif|documents|urls|audio|voice`;
  optional `from_id`, `top_msg_id`),
  `messages.GetMessagesViews` (`id`: array of message ids, `increment`: bool),
  `messages.ReadReactions`, `messages.ReadMentions` (optional `top_msg_id`).
- Mutating (explicit `--account` required, honors `--dry-run`):
  `account.UpdateProfile`, `account.SetAuthorizationTTL`
  (`authorization_ttl_days`: integer), `contacts.DeleteByPhones`
  (`phones`: array of phone numbers), `messages.ExportChatInvite`.

Shaping notes: history/search/scheduled results carry
`count`/`messages[]`/`chats`/`users`; `NotModified` adds `"not_modified": true`.
Read-reactions/read-mentions return `{pts, pts_count, offset}`;
`GetAuthorizations` returns `{authorization_ttl_days, authorizations[]}` rows with
no secrets; boolean results return `{"ok": true|false}`. Numeric `--args` fields
(`int`/`long`) may be omitted and default to `0` at dispatch (limit defaults to
10); omitted non-numeric required fields fail validation before connect.

## `tele completions`

```
tele completions bash|zsh|fish|powershell
```

Prints shell completion script for the `tele` binary to **stdout**, exits 0.
No account selection or network involved.

## Stability

- New commands and new optional JSON keys = MINOR.
- Exit code meaning change, renamed JSON keys, or removed commands = MAJOR.
- Changelog is consumer-facing (`CHANGELOG.md`), not `git log`.
