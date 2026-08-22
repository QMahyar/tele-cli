# Capability matrix

Spine of development. Status: `want` | `later` | `never` | `done`.

Sources: [Telegram API](https://core.telegram.org/api), [methods](https://core.telegram.org/methods), [grammers docs](https://docs.rs/grammers-client/latest/grammers_client/), [Full API](https://docs.telegram.org/methods). Layer tracked against grammers 0.10.

On grammers bump: diff client methods + TL layer changelog; add rows; do not silently drop.

Escape hatch for unwrapped RPCs: `tele raw <registry-name>` — a **typed registry** in `src/commands/raw.rs` (Rust TL types are static; the registry maps each supported method to a handler). Unregistered methods fail with a clear error; adding one is a one-command change + matrix row.

## Auth

| id | Capability | Telegram | grammers | CLI | Status |
|---|---|---|---|---|---|
| auth.code | Phone login code | `auth.sendCode` / `auth.signIn` | `request_login_code`, `sign_in` | `tele account login` | done |
| auth.2fa | 2FA password | `auth.checkPassword` | `check_password` | `tele account login` (interactive 2FA prompt; no-echo input on Windows) | done |
| auth.qr | QR login | `auth.exportLoginToken` | raw flow (no friendly helper) | `tele account login --method qr` (raw `tg://login` URI printed only on TTY stderr or `--show-token`) | done |
| auth.logout | Logout + delete session | `auth.logOut` | `sign_out` | `tele account logout` | done |
| auth.session-ttl | Session TTL / auth settings | `account.setAuthorizationTTL` | raw | `tele raw` account.SetAuthorizationTTL | done |
| auth.passkey | Passkeys | `/api/passkeys` | none friendly | — | want |
| auth.bot-token | Bot token login | `auth.importBotAuthorization` | `bot_sign_in` | not a product path | never |
| auth.password-manage | Cloud password set/change/remove + recovery email | `account.{GetPassword,UpdatePasswordSettings}` | raw | `tele account password set|change|remove` | want |
| auth.sessions-manage | List + terminate other sessions/devices | `account.{GetAuthorizations,ResetAuthorization}` | raw (`GetAuthorizations` in raw registry today) | `tele account sessions [--terminate HASH]` | want |

## Messages

| id | Capability | Telegram | grammers | CLI | Status |
|---|---|---|---|---|---|
| msg.send | Send text | `messages.sendMessage` | `send_message` | `tele msg send` | done |
| msg.schedule | Scheduled send | `schedule_date` | raw (no friendly param) | `tele msg send --schedule` | done |
| msg.schedule-repeat | Repeating scheduled | layer 215+ | raw if present | `tele raw` registry | want |
| msg.edit | Edit | `messages.editMessage` | `edit_message` | `tele msg edit` | done |
| msg.delete | Delete | `messages.deleteMessages` | `delete_messages` | `tele msg delete` (partial reporting + `--self-only`) | done |
| msg.forward | Forward | `messages.forwardMessages` | `forward_messages` | `tele msg forward` (always silent via grammers; no `--silent` flag) | done |
| msg.history | Get / iter history | `messages.getHistory` | `get_messages_by_id`, `iter_messages` | `tele msg get` | done |
| msg.pin | Pin / unpin | `messages.updatePinnedMessage` | `pin_message`, `unpin_message` | `tele msg pin` (always silent via grammers; no `--silent` flag) | done |
| msg.read | Mark read | `messages.readHistory` | `mark_as_read` | `tele msg read` | done |
| msg.file | Send file | `messages.sendMedia` | `upload_file` + `send_message` | `tele msg send --file` | done |
| msg.download | Download media | upload/download API | `download_media`, `iter_download` | `tele msg download` | done |
| msg.react | Reactions | `/api/reactions` | `send_reactions` | `tele msg react` | done |
| msg.poll | Polls | `/api/poll` | `send_message(InputMessage::poll)` or raw | `tele msg poll` | want |
| msg.search | Search / filters | `/api/search` | `search_messages`, `search_all_messages` | `tele msg search` | done |
| msg.draft | Drafts | `/api/drafts` | raw | `tele dialog drafts` | done |
| msg.effect | Animated effects | `/api/effects` | raw | `tele raw` registry | want |
| msg.checklist | Checklists | `/api/todo` | raw | `tele raw` registry | want |
| msg.translate | Translation | `/api/translation` | raw | `tele raw` registry | want |
| msg.transcribe | Voice transcription | `/api/transcribe` | raw | `tele raw` registry | want |
| msg.ai-compose | AI compose | `/api/ai` | raw | `tele raw` registry | want |
| msg.buttons | Reply markup / inline buttons in message JSON | `ReplyMarkup` | `Message::reply_markup` | serialized additively as `reply_markup` in every message JSON row (msg get/listen/takeout) | want |
| msg.click | Inline button press (+ reply-keyboard fallback) | `messages.getBotCallbackAnswer` | raw | `tele msg click` (reply-keyboard buttons fall back to sending their text) | want |
| msg.album-send | Send media group together | `messages.sendMultiMedia` | raw | `tele msg send --file A --file B` (grouped) | want |
| msg.typing | Chat action indicator | `messages.setTyping` | raw | `tele msg typing [--action typing/upload_*]` | want |
| msg.send-mods | Send modifiers: noforwards (protected), background send | `sendMessage`/`sendMultiMedia` flags | raw (grammers exposes none) | `tele msg send --noforwards/--background` | want |

## Chats

| id | Capability | Telegram | grammers | CLI | Status |
|---|---|---|---|---|---|
| chat.join | Join public / invite | `channels.joinChannel`, `messages.importChatInvite` | `join_chat`, `accept_invite_link` | `tele chat join` | done |
| chat.leave | Leave / delete dialog | `channels.leaveChannel`, `messages.deleteChatUser` | `delete_dialog` | `tele chat leave` | done |
| chat.invite | Export / edit invites | `messages.{exportChatInvite,getExportedChatInvites,editExportedChatInvite,deleteRevokedExportedChatInvites,getChatInviteImporters}`, `channels.InviteToChannel` / `messages.AddChatUser` | raw (friendly command wraps the raw family; raw registry entry stays) | `tele chat invite`: adds users (`--user`); exports links (+`--title/--expire/--usage-limit/--request-approval`); `--list [--revoked or --importers LINK]`; `--edit LINK (+--revoke +options)`; `--delete-revoked` | done |
| chat.participants | List members | `channels.getParticipants` | `iter_participants` (channels/supergroups; `--role admin/banned/kicked/recent` + `--search` pass the TL filter param); basic groups via raw `messages.GetFullChat` — members whose user data is missing are skipped, never a panic | `tele chat participants` | done |
| chat.kick | Kick / ban / restrict | `channels.editBanned` | `kick_participant`, `set_banned_rights` | `tele chat kick` (`--ban`, `--duration secs-or-forever`, `--rights CSV`) | done |
| chat.admin | Edit admin | `channels.editAdmin` | `set_admin_rights` (+ raw `channels.EditAdmin` when `other`/`manage_topics` requested) | `tele chat admin` (`--rights` incl. anonymous, other, manage_topics) | done |
| chat.adminlog | Admin log | `channels.getAdminLog` (+`AdminLogEventsFilter` flags) | raw | `tele chat admin-log` (`--admin USER`, `--search Q`, `--events CSV` server-side; `--since/--until` client-side; rows carry additive `actor` and old/new action payloads) | done |
| chat.stats | Channel / group stats | `/api/stats` | raw | `tele chat stats` | done |
| chat.settings | Slow mode, signatures, join-request, pre-history | `channels.{toggleSlowMode,toggleSignatures,togglePreHistoryHidden,toggleJoinRequest}`, read-back via `channels.getFullChannel` | raw | `tele chat settings` (`--noforwards` rejected: no toggle method in this TL layer; value still reported by read-back) | done |
| chat.forum | Forums / topics | `/api/forum` | raw | `tele topic *` incl. lifecycle close / reopen / edit / delete / pin (`topic create --emoji` single-codepoint only, see note) | done |
| chat.folders | Folders / archive | `/api/folders` | raw | `tele dialog archive` | done |
| chat.create | Create channel / group | `channels.createChannel` | raw | `tele chat create` | done |
| chat.edit | Edit title / about / photo | `channels.{editTitle,editPhoto}`, `messages.{editChatTitle,editChatPhoto,editChatAbout}`, `photos.deletePhotos` | raw | `tele chat edit` (`--title`, `--about`, `--photo path-or-remove`) | done |
| chat.link | Discussion group linkage | `channels.{getFullChannel,setDiscussionGroup}` | raw | `tele chat link` (`--to CHANNEL` set; unlink has no API method — honest error) | done |
| chat.invite-check | Preview invite link without joining (title/members/request flag) | `messages.checkChatInvite` | raw | `tele chat invite-check LINK` | want |
| chat.join-requests | Approve/dismiss join requests, bulk | `messages.{hideChatJoinRequest,hideAllChatJoinRequests}` | raw | `tele chat requests [--approve|--dismiss] [--all]` | want |

Note: `tele topic create --emoji` accepts only a single-codepoint emoji (4 UTF-8 bytes); empty, non-emoji, or multi-codepoint values are rejected with a Usage error before connect. The accepted value is sent as the packed codepoint in `icon_emoji_id`, but Telegram expects a custom-emoji document ID (~1e18) there — the server currently rejects/ignores it, so the topic icon is degraded. Full support is deferred until a `messages.searchCustomEmoji` document-ID lookup is implemented (open item M7, tracked in `tasks/todo.md`). Topic lifecycle (`close`, `reopen`, `edit`, `delete`, `pin`) ships via raw TL: `messages.editForumTopic` (closed flag / title), `messages.updatePinnedForumTopic` (pin), and `messages.deleteTopicHistory` (whole-topic history removal; not `messages.deleteHistory`). `topic list` rows carry additive `closed` + `pinned`.

## Dialogs & users

| id | Capability | Telegram | grammers | CLI | Status |
|---|---|---|---|---|---|
| dialog.list | Dialog list | `messages.getDialogs` | `iter_dialogs` | `tele dialog list` | done |
| dialog.draft | Set / clear draft | `messages.saveDraft` | raw | `tele dialog draft` (`--text` saves, `--clear` removes) | done |
| dialog.pin | Pin / unpin dialog | `messages.toggleDialogPin` | raw | `tele dialog pin` (`--unpin`; `reorderPinnedDialogs` deferred) | done |
| dialog.delete | Remove dialog (honest semantics) | `channels.leaveChannel`, `messages.deleteChatUser`, `messages.deleteHistory` | `delete_dialog` + raw | `tele dialog delete` (JSON `left`/`cleared`; `--revoke` deletes history on both sides for user chats) | done |
| contact.* | Contacts add/remove/list, block/unblock | `contacts.{GetContacts,AddContact,DeleteContacts}`, `contacts.{Block,Unblock}` | raw | `tele contact *` (`list` rows carry `username`; `remove --user` via DeleteContacts) | done |
| profile.* | Profile get/set (name, bio, photo, username), photo remove, emoji status | `get_me`, `account.{UpdateProfile,UpdateUsername}`, `users.getFullUser`, `photos.{UploadProfilePhoto,UpdateProfilePhoto,DeletePhotos}`, `account.updateEmojiStatus` | `get_me` + raw | `tele profile *` (`get`; `set` incl. `--username <u or remove>` with USERNAME errors mapped to Usage; `photo --remove`; `emoji-status --emoji <id>` or `--remove`; no colors commands exist) | done |
| privacy.* | Privacy rules (14 keys, user plus chat-participant rules) | `account.{GetPrivacy,SetPrivacy}` | raw | `tele privacy *` (keys incl. phone_p2p, birthday, star_gifts_auto_save, no_paid_messages, saved_music; `--allow-chat` / `--deny-chat`; same target on both sides rejected) | done |
| takeout | Data export | `/api/takeout` | raw | `tele takeout` (requires explicit `--account`/`--tag`; `all` allowed; stderr progress in human mode; per-dialog checkpoint resume appends instead of truncating; `finish --abandon` = success:false) | done |

## Updates

| id | Capability | Telegram | grammers | CLI | Status |
|---|---|---|---|---|---|
| listen.new | NewMessage | updates | `Update::NewMessage` | `tele listen` (default; requires explicit `--account`/`--tag`) | done |
| listen.edit | MessageEdited | updates | `Update::EditMessage` | `--events MessageEdited` | done |
| listen.delete | MessageDeleted | updates | `Update::DeleteMessages` | `--events MessageDeleted` (DM/basic-group deletions match under `--chat` via bounded observed-id map) | done |
| listen.action | ChatAction | updates | `Update::UserStatus`/raw | `--events ChatAction` | want |
| listen.user | UserUpdate | updates | `Update::*` | `--events UserUpdate` | want |
| listen.album | Album | updates | `Update::NewMessage` (grouped) | `--events Album` (coalesce by grouped_id, ~500 ms quiescence flush) | done |
| listen.gap | Gap (update-loss marker) | updates | pts tracking per message box | `--events Gap` (synthetic row when updates were dropped/difference ended early) | done |
| listen.raw | Raw Update | updates | raw `Update` enum | `--events Raw` (base64 payload + state in row, allowlist-gated) | done |
| listen.filters | Sender / direction / regex / multi-chat filters | client-side | client-side | `listen --from USER --in --out --pattern RE --chat repeatable` | want |
| listen.service | Parsed service messages (joins/leaves/…) | updates `messageService` | `Update::NewMessage` (service) | parsed event rows with action kind | want |
| listen.callback | CallbackQuery | bot | `Update::CallbackQuery` | — | never |
| listen.inline | InlineQuery | bot | `Update::InlineQuery` | — | never |

## Explicitly later / never

| id | Domain | Status | Why |
|---|---|---|---|
| stories.* | Stories | want | Extra surface |
| stickers.manage | Sticker / GIF pack management | want | Send-as-file first |
| business.* | Telegram Business | never | Monetization extras — cut by product decision 2026-08-23 |
| stars.* | Stars, gifts, payments, boosts, giveaways | never | Monetization — cut by product decision 2026-08-23 |
| calls.* | 1:1 and group calls | never | Realtime media |
| secret.* | Secret chats / E2E | never | Separate protocol |
| passport.* | Telegram Passport | never | Not this product |
| ads.* | Sponsored messages | never | Official-client burden |
| collectibles.* | Fragment collectibles | never | Not this product |
| smsjobs.* | Official-client SMS jobs | never | Official only |
| mcp | MCP server | want | End of development |
| skill | Agent skill | want | End of development |

## Kernel (not Telegram, but blocking)

| id | Capability | CLI / module | Status |
|---|---|---|---|
| kernel.config | TOML + .env | `src/config.rs` | done |
| kernel.accounts | Names, tags, all | `src/executor.rs` | done |
| kernel.session | Per-account session path; OS-level exclusive lock (stale `.session.lock` marker persists by design); SQLite sidecars (`-journal`/`-wal`/`-shm`) permission-restricted like the session | `src/session.rs`, `src/fs_util.rs` | done |
| kernel.executor | Sequential by default; `--parallel` cap 1..=32; per-account token-bucket rate limiter layered on global semaphore (FloodWait/SlowModeWait handled by grammers AutoSleep retry policy); Ctrl+C aborts pending account tasks structurally; `listen`/`takeout` require explicit account selection | `src/executor.rs`, `src/rate_limiter.rs` | done |
| kernel.output | Broken stdout pipes exit 0 silently; stderr write failures ignored; tables use dynamic width arrangement | `src/output.rs`, `src/main.rs` | done |
| kernel.proxy | Global + per-account SOCKS5 (grammers 0.10 proxy feature is socks5-only) | `src/client.rs` | done |
| kernel.json | Serialize results | `src/serialize.rs` | done |
| kernel.raw | Build-time TL registry | `tele raw` (`src/commands/raw.rs` + `build.rs` + vendored `tl/api.tl`) — validation, registry, and help generated from TL schema at build time via `grammers-tl-parser`; dispatch arms hand-written for response shaping; human-readable output in non-machine mode (lines or table); JSON envelope in `--json`/`--jsonl`; mutating methods (`account.UpdateProfile`, `account.SetAuthorizationTTL`, `contacts.DeleteByPhones`, `messages.ExportChatInvite`) require an explicit `--account` and honor `--dry-run`; 18 registry names incl. read-only `channels.GetFullChannel`, `users.GetUsers`, `messages.{GetHistory,Search,GetScheduledHistory,GetMessagesViews,ReadReactions,ReadMentions,GetDialogUnreadMarks}`, `account.GetAuthorizations` | done |
| kernel.peers | Chat-target resolution: numeric id (cached auth; `chat create` caches the created chat's access_hash into the session so `--chat <id>` works immediately after; `-100…` bot-API dialog ids via `PeerId::from_bot_api_dialog_id`), `@username`, `t.me/` link, `me` (friendly `get_me`; `resolve_peer(InputPeerSelf)` is broken in grammers 0.10 — misleading `Dropped`), `+phone` (raw `contacts.ImportContacts`, no friendly path; the temporary import is deleted immediately after resolution — no contact side effect, and privacy settings may hide the account) | `src/entities.rs` | done |
| kernel.completions | Shell completions (bash, zsh, fish, powershell), printed to stdout, exit 0 | `tele completions` (`src/commands/completions.rs`) | done |
| kernel.device-id | Per-account device identity (`device_model`/`system_version`/`app_version`/`lang_code`) fed to the client builder | config `[accounts.<name>]` keys → `src/client.rs` | want |
| kernel.session-port | Session export/import across machines + Telethon `.session` converter | `tele account export-session/import-session`, `src/session.rs` | want |
| kernel.login-staged | Non-TTY staged login; pending auth state resumable across invocations | `tele account login --stage …` (state under app dir) | want |
| kernel.link-resolve | Deep link → chat id + message id (`t.me/<chat>/<id>`) | extend `classify_target` (`src/entities.rs`) + JSON id fields | want |
| kernel.serve | Single-owner runtime: `tele serve` child process holds the connected client per account, streams events on stdout, accepts action requests on stdin (duplex JSONL, LSP/MCP shape: versioned hello handshake, request-id correlation, stderr logs; script is supervisor). Solves listen+act-on-one-session (MTProto main sessions are full-duplex; `tmp_sessions` default 1 — one connection multiplexes RPC + updates) without session-lock contention | `tele serve --account X` (`src/commands/serve.rs`; tokio io-util; actions via in-process `*_core` fns) | want |
