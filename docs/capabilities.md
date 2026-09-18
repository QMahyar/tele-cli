# Capability matrix

This table tracks every product capability from request to ship. `Status` is one of `want`, `later`, `never`, or `done`.

Sources: the [Telegram API overview](https://core.telegram.org/api), the [method reference](https://core.telegram.org/methods), the [grammers docs](https://docs.rs/grammers-client/latest/grammers_client/), and the [full method listing](https://docs.telegram.org/methods). Rows track grammers 0.10.

When you bump grammers, diff its client methods and TL-layer changelog against this table, then add rows for what changed. Do not delete rows silently.

RPCs without a friendly wrapper stay reachable through `tele raw <registry-name>`. The typed registry in `src/commands/raw.rs` maps each supported method to a handler, because Rust TL types are static. An unregistered method fails with an error that names the registry. Adding one takes one match arm and one row here.

## Auth and account

| id | Capability | Telegram | grammers | CLI | Status |
|---|---|---|---|---|---|
| auth.code | Phone login code | `auth.sendCode` / `auth.signIn` | `request_login_code`, `sign_in` | `tele account login` | done |
| auth.2fa | 2FA password | `auth.checkPassword` | `check_password` | `tele account login` (interactive 2FA prompt; no-echo input on Windows) | done |
| auth.qr | QR login | `auth.exportLoginToken` | raw flow (no friendly helper) | `tele account login --method qr` (raw `tg://login` URI printed only on TTY stderr or `--show-token`) | done |
| auth.logout | Logout + delete session | `auth.logOut` | `sign_out` | `tele account logout` | done |
| auth.session-ttl | Session TTL / auth settings | `account.setAuthorizationTTL` | raw | `tele raw` account.SetAuthorizationTTL | done |
| auth.passkey | Passkeys | `/api/passkeys` (Layer 219, present in vendored 227) — WebAuthn ceremony + RP-ID=telegram.org ban for unofficial apps | none friendly | impractical: requires browser/platform authenticator; raw TL alone insufficient | never |
| auth.bot-token | Bot token login | `auth.importBotAuthorization` | `bot_sign_in` | not a product path | never |
| auth.password-manage | Cloud password lifecycle: set/change/remove via local PH2 (pbkdf2-hmac-sha512 ×100k replicated from grammers-crypto 0.10) + SRP proofs; recovery-email confirm/resend/cancel with auto-prompt on EMAIL_UNCONFIRMED; status display; 7-day reset start/decline. KNOWN LIMITATION: `--remove`/`--change` against live server return NEW_SETTINGS_EMPTY / INPUT_FETCH_ERROR (grammers TL serialization defect for UpdatePasswordSettings with SRP proof — upstream issue); disable/change via official app until fixed | `account.{GetPassword,UpdatePasswordSettings,ConfirmPasswordEmail,ResendPasswordEmail,CancelPasswordEmail,ResetPassword,DeclinePasswordReset}` + `grammers_crypto` SRP + local PH2 | `tele account password` with `--set` / `--change` / `--remove` / `--confirm-email CODE` / `--resend-email` / `--cancel-email` / `--status` / `--reset-start` / `--decline-reset` (no-echo prompts; dry-run never prompts; secrets never in output) | done |
| auth.sessions-manage | List + terminate other sessions/devices; web sessions list/terminate/flags | `account.{GetAuthorizations,ResetAuthorization,GetWebAuthorizations,ResetWebAuthorization(s),ChangeAuthorizationSettings}` | raw | `tele account sessions [--terminate HASH]` / `sessions --web` / `--terminate-web HASH` / `--terminate-all-web` / `--change-flags HASH …`; own current-hash refusal | done |
| auth.account-ttl | Inactive-account self-destruct timer get/set (1–365 days) | `account.{GetAccountTTL,SetAccountTTL}` | raw | `tele account ttl --get` / `--set --days N` | done |
| auth.delete-account | Account deletion with reason + password SRP + --yes guard | `account.DeleteAccount` | raw | `tele account delete --reason R --yes` (explicit --account required) | done |
| auth.phone-change | Change account phone (staged: send code → confirm) | `account.{SendChangePhoneCode,ChangePhone}` | raw | `tele account phone --change-phone +XXX [--allow-flashcall]` then `phone --confirm-code NNN --phone-hash H`; dry-run `would` strings redact the OTP code and phone_code_hash (length-only hint) | done |

## Messages

| id | Capability | Telegram | grammers | CLI | Status |
|---|---|---|---|---|---|
| msg.send | Send text | `messages.sendMessage` | `send_message` | `tele msg send` (incl. `--as voice`/`video-note` for voice/round-video notes, `--poll Q --option A --option B [--poll-mode quiz --poll-quiz-option N]` for poll creation via `InputMediaPoll`; `--split N` chunks oversized text into sequential ≤4096-UTF-16-unit messages) | done |
| msg.schedule | Scheduled send | `schedule_date` | raw (no friendly param) | `tele msg send --schedule` | done |
| msg.schedule-repeat | Repeating scheduled messages | `messages.{SendMessage,editMessage}` carry `schedule_repeat_period` at the vendored layer 227 | raw | `tele raw` messages.SendMessage / messages.editMessage with `schedule_repeat_period`; no friendly `tele msg send --repeat` flag yet | later |
| msg.edit | Edit | `messages.editMessage` | `edit_message` | `tele msg edit` (text/caption via `--text`/`--caption` with `--format plain`/`markdown`, media swap via `--file`, `--no-preview` sets `no_webpage`) | done |
| msg.delete | Delete | `messages.deleteMessages` | `delete_messages` | `tele msg delete` (partial reporting + `--self-only`) | done |
| msg.forward | Forward | `messages.forwardMessages` | `forward_messages` | `tele msg forward` (no silent flag; grammers 0.10 does not set the TL silent flag, so forwarded messages notify recipients) | done |
| msg.history | Get / iter history | `messages.getHistory` | `get_messages_by_id`, `iter_messages`, `get_reply_to_message` | `tele msg get` (single `--id` accepts `--replied` to attach the additive `replied_to` message row via `get_reply_to_message`, incl. cross-chat discussion parents) | done |
| msg.pin | Pin / unpin | `messages.updatePinnedMessage` | `pin_message`, `unpin_message` | `tele msg pin` (unpin always silent via grammers; `--notify` controls the pin notification) | done |
| msg.read | Mark read | `messages.readHistory` | `mark_as_read` | `tele msg read` | done |
| msg.file | Send file | `messages.sendMedia` | `upload_file` + `send_message` (+ `upload_stream` for stdin) | `tele msg send --file` (`--file -` streams stdin with required `--file-size`/`--file-name`) | done |
| msg.download | Download media | upload/download API | `download_media`, `iter_download` | `tele msg download` (single `--id`, album siblings via `--id N --album`, whole-chat sweep via `--all [--since/--until] [--limit]` with per-chat checkpoint resume) | done |
| msg.react | Reactions | `/api/reactions` | `send_reactions` | `tele msg react` | done |
| msg.poll | Polls: create, render in message rows + vote, close, fresh results, voters, unread | `/api/poll`, `messages.sendVote`, `messages.editMessage` (close via `InputMediaPoll` with `poll.closed=true`), `messages.getPollResults` / `getPollVotes` / `getUnreadPollVotes` | `sendMedia` + `InputMediaPoll` for create; raw arm; `Media::Poll` answers | `tele msg send --poll Q --option A --option B`; `tele msg vote --chat X --id N --option 1[,2]`; `tele msg poll-close --chat X --id N`; `tele msg poll-results --chat X --id N`; `tele msg poll-votes --chat X --id N [--option N] [--limit N]`; `tele msg poll-unread --chat X [--top T] [--limit N]`; additive `poll` object on msg get/search rows | done |
| msg.export | Export chat history to file | client-side sweep | `iter_messages` + serialize | `tele msg export --chat X [--format txt\|jsonl] [--out FILE] [--since/--until] [--limit N] [--offset-id M]` (rows carry additive `link` t.me permalink for channels/supergroups; `--out` files get private permissions; single-account only) | done |
| msg.search | Search / filters | `/api/search` | `search_messages`, `search_all_messages` | `tele msg search` (`--from SENDER`, `--kind photo/video/gif/document/url/audio/voice`, `--since/--until` dates; per-chat `--from me` and `--kind` run server-side; `--from me` uses the server-side own-messages filter) | done |
| msg.draft | Drafts | `/api/drafts` | raw | `tele dialog drafts` | done |
| msg.effect | Animated effects: list available + apply on send | `/api/effects` | raw `messages.GetAvailableEffects` for list; raw `SendMessage`/`SendMedia` with `effect` for apply (grammers 0.10 builder carries no effect field) | `tele raw` messages.GetAvailableEffects; `tele msg send --effect ID` (text and single-file sends; albums/--url/--copy-from/--as/--poll rejected; Premium/1-on-1 limits surface as honest server errors) | done |
| msg.checklist | Checklists: append items, toggle completion | `/api/todo` | raw `messages.{AppendTodoList,ToggleTodoCompleted}` (creation rides inputMediaTodo in send media) | `tele raw` registry arms | done |
| msg.translate | Translation | `/api/translation`, `messages.TranslateText` | raw arm with result shaping | `tele raw` messages.TranslateText | done |
| msg.transcribe | Voice transcription | `/api/transcribe`, `messages.TranscribeAudio` | raw arm with transcribed-audio shaping | `tele raw` messages.TranscribeAudio | done |
| msg.ai-compose | AI compose | `messages.ComposeMessageWithAI` exists at layer 227 | raw arm with composed-message + tone shaping | `tele raw` messages.ComposeMessageWithAI | done |
| msg.buttons | Reply markup / inline buttons in message JSON | `ReplyMarkup` | `Message::reply_markup` | `tele msg get` / `listen` / `serve` rows carry additive `reply_markup`; kinds inline/reply/hide/force_reply; every button carries additive `type` (`text`/`url`/`callback`/`switch_inline`/`buy`/`copy_text`/`game`/`request_phone`/`request_location`/`request_poll`/`url_auth`/`unknown`), callback buttons carry `requires_password` when set, unknown variants degrade to `raw_kind`, never panic; `peer`/`sender` objects carry additive `username` (null when the peer has none; the id-only fallback object cannot resolve usernames) | done |
| msg.click | Inline button press (+ reply-keyboard fallback) | `messages.getBotCallbackAnswer` | raw arm (button located via serialize shapes) | `tele msg click --chat X --id N (--button TEXT / --button-index N / --button-contains SUBSTR / --button-data DATA)`; `--button-data` matches the decoded callback payload exactly; precedence `--button-index > --button-data > --button-contains > --button`; reply-keyboard buttons error with send-text hint | done |
| msg.album-send | Send media group together | `messages.sendMultiMedia` | `send_album` (CAP-3, 2–10 files) | `tele msg send --file A --file B` (grouped) | done |
| msg.typing | Chat action indicator | `messages.setTyping` | friendly `Client::action()` oneshot/cancel | `tele msg typing --chat X [--action typing/upload-photo/upload-file/cancel]` | done |
| msg.send-mods | Send modifiers: noforwards (protected), background send | sendMessage flags (grammers builder lacks noforwards → raw arm with markdown parse) | raw + builder `.background()` | `tele msg send` with `--noforwards` / `--background`; silent pre-existing | done |

## Chats

| id | Capability | Telegram | grammers | CLI | Status |
|---|---|---|---|---|---|
| chat.join | Join public / invite | `channels.joinChannel`, `messages.importChatInvite` | `join_chat`, `accept_invite_link` | `tele chat join` | done |
| chat.leave | Leave / delete dialog | `channels.leaveChannel`, `messages.deleteChatUser` | `delete_dialog` | `tele chat leave` | done |
| chat.invite | Export / edit invites | `messages.{exportChatInvite,getExportedChatInvites,editExportedChatInvite,deleteRevokedExportedChatInvites,getChatInviteImporters}`, `channels.InviteToChannel` / `messages.AddChatUser` | raw (friendly command wraps the raw family; raw registry entry stays) | `tele chat invite`: adds users (`--user`); exports links (+`--title/--expire/--usage-limit/--request-approval`); `--list [--revoked or --importers LINK]`; `--edit LINK (+--revoke +options)`; `--delete-revoked` | done |
| chat.participants | List members | `channels.getParticipants` | `iter_participants` (channels/supergroups; `--role admin/banned/kicked/recent` + `--search` pass the TL filter param); basic groups via raw `messages.GetFullChat` — members whose user data is missing are skipped, never a panic | `tele chat participants` | done |
| chat.kick | Kick / ban / restrict | `channels.editBanned` | `kick_participant`, `set_banned_rights` | `tele chat kick` (`--ban`, `--duration secs-or-forever`, `--rights CSV`) | done |
| chat.admin | Edit admin | `channels.editAdmin` | `set_admin_rights` (+ raw `channels.EditAdmin` when `other`/`manage_topics` requested) | `tele chat admin` (`--rights` incl. anonymous, other, manage_topics) | done |
| chat.permissions | Rights read-back for a participant | `channels.getParticipant` (+ `messages.getFullChat` role fallback for basic groups) | raw `channels.GetParticipant`; `get_permissions` for basic groups | `tele chat permissions --chat X --user U` (rows: role, full admin_rights/banned_rights flag maps incl. until_date, rank, can_edit, kicked_by, promoted_by; serve/MCP `chat permissions` op) | done |
| chat.adminlog | Admin log | `channels.getAdminLog` (+`AdminLogEventsFilter` flags) | raw | `tele chat admin-log` (`--admin USER`, `--search Q`, `--events CSV` server-side; `--since/--until` client-side; rows carry additive `actor` and old/new action payloads) | done |
| chat.stats | Channel / group stats | `/api/stats` | raw | `tele chat stats` | done |
| chat.settings | Slow mode, signatures, join-request, pre-history, noforwards | `channels.{toggleSlowMode,toggleSignatures,togglePreHistoryHidden,toggleJoinRequest}`, `messages.toggleNoForwards`, read-back via `channels.getFullChannel` | raw | `tele chat settings` (`--noforwards on/off` applies via `messages.toggleNoForwards`; value also reported by read-back) | done |
| chat.forum | Forums / topics | `/api/forum` | raw | `tele topic *` incl. lifecycle close / reopen / edit / delete / pin (`topic create --emoji` single-codepoint only, see note) | done |
| chat.folders | Folders / archive | `/api/folders` | raw | `tele dialog archive` | done |
| dialog.folders-manage | List/create/delete/reorder chat folders (dialog filters) | `messages.{getDialogFilters,updateDialogFilter,updateDialogFiltersOrder}` | raw | `tele dialog folders` / `folder-create --title X [--contacts --groups --broadcasts --bots --exclude-muted --exclude-read --exclude-archived]` / `folder-delete --id N` / `folder-reorder --order 1,2,3` | done |
| msg.scheduled-manage | List/delete/send-now scheduled messages | `messages.{getScheduledHistory,deleteScheduledMessages,sendScheduledMessages}` | raw | `tele msg scheduled --chat X` / `scheduled-delete --chat X --ids N` / `scheduled-send --chat X --ids N` | done |
| cache.local | Local SQLite message cache with FTS5 offline search | local SQLite + `iter_messages` sync | local | `tele cache sync --chat X` / `cache search --query Q` / `cache stats` / `cache clear` (per-account `{app}/cache/{name}.cache.db`, FTS5 on text/chat/sender) | done |
| chat.create | Create channel / group | `channels.createChannel` | raw | `tele chat create` | done |
| chat.edit | Edit title / about / photo | `channels.{editTitle,editPhoto}`, `messages.{editChatTitle,editChatPhoto,editChatAbout}`, `photos.deletePhotos` | raw | `tele chat edit` (`--title`, `--about`, `--photo path-or-remove`) | done |
| chat.link | Discussion group linkage | `channels.{getFullChannel,setDiscussionGroup}` | raw | `tele chat link` (`--to CHANNEL` set; unlink has no API method — honest error) | done |
| chat.invite-check | Preview invite link without joining (title/members/request flag) | `messages.checkChatInvite` | raw | `tele chat invite --check LINK` (Already/Peek/Invite variants; bare +hash accepted) | done |
| chat.join-requests | Approve/dismiss join requests, bulk | `messages.{hideChatJoinRequest,hideAllChatJoinRequests}` (+ list via GetChatInviteImporters requested:true — getChatJoinRequests absent at layer 227) | raw | `tele chat requests` list; approve/dismiss via `--approve` / `--dismiss` with `--user USER` or `--all` | done |

Note on topic icons: `tele topic create --emoji` accepts only a single-codepoint emoji (4 UTF-8 bytes). Empty values, non-emoji values, and multi-codepoint values fail with a Usage error before connect. The command sends the packed codepoint as `icon_emoji_id`, but Telegram expects a custom-emoji document ID (roughly 1e18) in that field, so the server currently rejects or ignores the value and the topic icon degrades. Full support waits on a `messages.searchCustomEmoji` document-ID lookup (open item M7). Topic lifecycle (`close`, `reopen`, `edit`, `delete`, `pin`) ships over raw TL: `messages.editForumTopic` sets the closed flag and the title, `messages.updatePinnedForumTopic` pins, and `messages.deleteTopicHistory` removes whole-topic history (it is not `messages.deleteHistory`). `topic list` rows carry additive `closed` and `pinned`.

## Dialogs and users

| id | Capability | Telegram | grammers | CLI | Status |
|---|---|---|---|---|---|
| dialog.list | Dialog list | `messages.getDialogs` | `iter_dialogs` | `tele dialog list` | done |
| dialog.draft | Set / clear draft | `messages.saveDraft` | raw | `tele dialog draft` (`--text` saves, `--clear` removes) | done |
| dialog.pin | Pin / unpin dialog | `messages.toggleDialogPin` | raw | `tele dialog pin` (`--unpin`; `reorderPinnedDialogs` deferred) | done |
| dialog.delete | Remove dialog (honest semantics) | `channels.leaveChannel`, `messages.deleteChatUser`, `messages.deleteHistory` | `delete_dialog` + raw | `tele dialog delete` (JSON `left`/`cleared`; `--revoke` deletes history on both sides for user chats) | done |
| contact.* | Contacts add/remove/list, block/unblock | `contacts.{GetContacts,AddContact,DeleteContacts}`, `contacts.{Block,Unblock}` | raw | `tele contact *` (`list` rows carry `username`; `remove --user` via DeleteContacts) | done |
| profile.photos | Profile photo history | `photos.getUserPhotos` / chat-photos sweep | `iter_profile_photos` | `tele profile photos [--user USER] [--limit N]` (rows: id/date/size/sizes/current; serve/MCP `profile photos` op) | done |
| profile.* | Profile get/set (name, bio, photo, username), photo remove, emoji status | `get_me`, `account.{UpdateProfile,UpdateUsername}`, `users.getFullUser`, `photos.{UploadProfilePhoto,UpdateProfilePhoto,DeletePhotos}`, `account.updateEmojiStatus` | `get_me` + raw | `tele profile *` (`get`; `set` incl. `--username <u>` and `--clear-username` with USERNAME errors mapped to Usage; `photo --remove`; `emoji-status --emoji <id>` or `--remove`; no colors commands exist) | done |
| privacy.* | Privacy rules (14 keys, user plus chat-participant rules) | `account.{GetPrivacy,SetPrivacy}` | raw | `tele privacy *` (keys incl. phone_p2p, birthday, star_gifts_auto_save, no_paid_messages, saved_music; `--allow-chat` / `--deny-chat`; same target on both sides rejected; merge-by-default with `--replace` for revocation — cross-side targets error instead of shipping contradictory rules) | done |
| takeout | Data export | `/api/takeout` | raw | `tele takeout` (requires explicit `--account`/`--tag`; `all` allowed; stderr progress in human mode; per-dialog checkpoint resume appends instead of truncating; `finish --abandon` = success:false) | done |
| stories.* | Story send/list/read/delete/pin/edit/views/reactions/link | `stories.{sendStory,editStory,getPeerStories,getStoriesArchive,getPinnedStories,readStories,deleteStories,togglePinned,getStoriesViews,getStoryViewsList,getStoryReactionsList,exportStoryLink}`, `upload_file` + `inputMediaUploadedPhoto`/`inputMediaUploadedDocument` | raw | `tele story *` (`send --file F [--caption C] [--privacy everyone/contacts/close-friends] [--period s] [--pinned] [--noforwards]`; `edit --id N [--file F] [--caption C] [--privacy P]`; `list [--archive] [--pinned]`; `read --max-id N` (API is max-id only, no per-id read); `views --ids 1,2` (view counts); `viewers --id N` (who viewed, own stories); `reactions --id N`; `link --id N` (deep link); `delete --ids`, `pin --ids`, `unpin --ids`; all mutators require explicit `--account` and honor `--dry-run`; send/edit validate sensitive paths like msg uploads; peer must be own account, a user whose privacy permits, or a channel you admin — server-enforced, not pre-checked offline) | done |

## Update streams

| id | Capability | Telegram | grammers | CLI | Status |
|---|---|---|---|---|---|
| listen.new | NewMessage | updates | `Update::NewMessage` | `tele listen` (default; requires explicit `--account`/`--tag`) | done |
| listen.edit | MessageEdited | updates | `Update::EditMessage` | `--events MessageEdited` | done |
| listen.delete | MessageDeleted | updates | `Update::DeleteMessages` | `--events MessageDeleted` (DM/basic-group deletions match under `--chat` via bounded observed-id map) | done |
| listen.action | ChatAction | updates (Raw wrapper: UserTyping/ChatUserTyping/ChannelUserTyping — typed enum carries none at this layer) | parsed-from-Raw | `--events ChatAction` rows `{action:{kind,label}}` | done |
| listen.user | UserUpdate | updates (updateUserStatus via Raw wrapper; other user updates stay on Raw path) | parsed-from-Raw | `--events UserUpdate` presence rows `{status:{kind,label,expires?/was_online?}}` | done |
| listen.callback-query | CallbackQuery (bot button presses) | updates (BotCallbackQuery/InlineBotCallbackQuery via Raw wrapper) | parsed-from-Raw | `--events CallbackQuery` rows `{user_id, chat_id?, message_id, data, data_b64}`; absent from allowlist falls back to Raw; answering not exposed | done |
| listen.album | Album | updates | `Update::NewMessage` (grouped) | `--events Album` (coalesce by grouped_id, ~500 ms quiescence flush) | done |
| listen.gap | Gap (update-loss marker) | updates | pts tracking per message box | `--events Gap` (synthetic row when updates were dropped/difference ended early) | done |
| listen.raw | Raw Update | updates | raw `Update` enum | `--events Raw` (base64 payload + state in row, allowlist-gated) | done |
| listen.filters | Sender / direction / regex / multi-chat filters, finite-stream exits | client-side | client-side | `tele listen` with `--from USER` / `--in` / `--out` / `--pattern RE` (case-sensitive) / repeatable `--chat`; AND across dimensions, OR within; `--count N` / `--until TS` / `--timeout-secs S` end the stream with exit 0 | done |
| listen.service | Parsed service messages (joins/leaves/pin + 63 more kinds) | updates `messageService` | typed `Message::Service` | `--events Service` rows with `service_action:{kind,label}`; composes chat/from/direction filters | done |
| listen.callback | CallbackQuery (bot-account updates) | bot | `Update::CallbackQuery` | — (see `listen.callback-query` above, which covers presses on this account's bot messages) | never |
| listen.inline | InlineQuery | bot | `Update::InlineQuery` | — | never |

## Other domains

| id | Domain | CLI | Status | Why |
|---|---|---|---|---|
| stickers.manage | Sticker pack management | `tele sticker` `list` / `search --query` / `show --set S` / `install --set S [--archive]` / `remove --set S` (`--set` accepts short name or t.me/addstickers link); raw arms in dedicated module for `messages.{getAllStickers,searchStickerSets,getStickerSet,installStickerSet,uninstallStickerSet}`; creator-side naming + archive-toggle via `tele raw` | done | |
| business.* | Telegram Business | — | never | Monetization extras — cut by product decision 2026-08-23 |
| stars.* | Stars, gifts, payments, boosts, giveaways | — | never | Monetization — cut by product decision 2026-08-23 |
| calls.* | 1:1 and group calls | — | never | Realtime media |
| secret.* | Secret chats / E2E | — | never | Separate protocol |
| passport.* | Telegram Passport | — | never | Not this product |
| ads.* | Sponsored messages | — | never | Official-client burden |
| collectibles.* | Fragment collectibles | — | never | Not this product |
| smsjobs.* | Official-client SMS jobs | — | never | Official only |
| mcp | MCP stdio server: tele mcp exposes 78 ops as tools via rmcp 3.1 (stdio, legacy handshake, inputSchema via schemars, annotations, confirm gate, read-only/groups filters) | `tele mcp --account NAME` | done | |
| skill.print | Print the embedded Agent-Skills-spec SKILL.md (usage rules, command map, envelope, recipes) to stdout | `tele skill` | done | |
| skill.install | Install SKILL.md into detected agent skill dirs (.claude, .config/opencode, .cursor) or --dir PATH; --force overwrites | `tele skill install [--dir PATH] [--force]` | done | |

## Kernel

Kernel rows cover the non-Telegram pieces the CLI needs before any RPC runs.

| id | Capability | CLI / module | Status |
|---|---|---|---|
| kernel.config | TOML + .env | `src/config.rs` | done |
| kernel.accounts | Names, tags, all | `src/executor.rs` | done |
| kernel.session | Per-account session path; OS-level exclusive lock (stale `.session.lock` marker persists by design); SQLite sidecars (`-journal`/`-wal`/`-shm`) permission-restricted like the session | `src/session.rs`, `src/fs_util.rs` | done |
| kernel.executor | Sequential by default; `--parallel` cap 1..=32; per-account token-bucket rate limiter layered on global semaphore (FloodWait/SlowModeWait handled by grammers AutoSleep retry policy); Ctrl+C aborts pending account tasks structurally; `listen`/`takeout` require explicit account selection | `src/executor.rs`, `src/rate_limiter.rs` | done |
| kernel.output | Broken stdout pipes exit 0 silently; stderr write failures ignored; tables use dynamic width arrangement | `src/output.rs`, `src/main.rs` | done |
| kernel.proxy | Global + per-account SOCKS5 (grammers 0.10 proxy feature is socks5-only) | `src/client.rs` | done |
| kernel.json | Serialize results | `src/serialize.rs` | done |
| kernel.raw | Build-time TL registry | `tele raw` (`src/commands/raw.rs` + `build.rs` + vendored `tl/api.tl`) — validation, registry, and help generated from TL schema at build time via `grammers-tl-parser`; dispatch arms hand-written for response shaping; human-readable output in non-machine mode (lines or table); JSON envelope in `--json`/`--jsonl`; mutating methods (`account.UpdateProfile`, `account.SetAuthorizationTTL`, `contacts.DeleteByPhones`, `messages.ExportChatInvite`, `messages.AppendTodoList`, `messages.SendScheduledMessages`, `messages.ToggleTodoCompleted`) require an explicit `--account` and honor `--dry-run`; the serve/MCP `raw` op is registered `destructive`, so calls pass through the `ConfirmRequired{would}` confirm gate; 25 registry names incl. read-only `channels.GetFullChannel`, `users.GetUsers`, `messages.{GetHistory,Search,GetScheduledHistory,GetMessagesViews,ReadReactions,ReadMentions,GetDialogUnreadMarks}`, `account.GetAuthorizations` | done |
| kernel.peers | Chat-target resolution: numeric id (cached auth; `chat create` caches the created chat's access_hash into the session so `--chat <id>` works immediately after; `-100…` bot-API dialog ids via `PeerId::from_bot_api_dialog_id`), `@username`, `t.me/` link, `me` (friendly `get_me`; `resolve_peer(InputPeerSelf)` is broken in grammers 0.10 — misleading `Dropped`), `+phone` (raw `contacts.ImportContacts`, no friendly path; the temporary import is deleted immediately after resolution — no contact side effect, and privacy settings may hide the account) | `src/entities.rs` | done |
| kernel.completions | Shell completions (bash, zsh, fish, powershell), printed to stdout, exit 0 | `tele completions` (`src/commands/completions.rs`) | done |
| kernel.device-id | Per-account device identity (`device_model`/`system_version`/`app_version`/`lang_code`) fed to the client builder; config `[accounts.<name>]` keys → `ConnectionParams`; all four wired at grammers 0.10, defaults neutral when unset | `src/config.rs`, `src/client.rs` | done |
| kernel.session-port | Session export/import across machines + Telethon `.session` converter (classic + version-table schemas via read-only libsql, same engine as grammers; native authoring via public `Session` trait; sha256 rows, locked-source refusal, `--force` semantics, restricted perms) | `tele account export-session` / `import-session [--from-telethon]`, `src/session.rs` | done |
| kernel.login-staged | Non-TTY staged login; pending auth state resumable across invocations (`--stage begin/code/status/cancel`; phone_code_hash persisted under `{app}/pending/`, secrets never stored; 303 DC migration handled); server-side `--stage resend` (auth.resendCode) and `--stage cancel-code` (auth.cancelCode + clears local state); code-staged method only — QR staging excluded | `tele account login --stage …` | done |
| kernel.link-resolve | Deep link → chat id + message id (`t.me/<chat>/<id>`, `t.me/c/<internal>/<id>`): `msg get --chat LINK` fetches that message (--id conflicts); other commands reject link-carried ids naming accepted consumers; parser exposed as `parse_target() -> ResolvedTarget` | `src/entities.rs`, `src/commands/msg/mod.rs` | done |
| kernel.serve | Duplex control plane: `tele serve` child owns one or more account sessions (1-32, each with an OS-level lock, EOF drains and exits 0) speaking newline-delimited JSONL over stdin/stdout with stderr logs; script is supervisor. Negotiated hello (driver `{"type":"hello","protocol":N}`; server carries `protocol`/`min_protocol`/`max_protocol`, `account`+`identity` for the first account, additive `accounts[]` entries `{"name","identity"}` for all served accounts sorted by name, `last_seq`; `VersionMismatch` outside the inclusive range). Requests correlate by integer `id`; params parse with deny_unknown_fields and failures carry `error.param`; routed ops target an account via a top-level `params.account` name (stripped before parsing; omitted defaults to the only account, ambiguity with multiple served accounts is a `ServeError`). Full error taxonomy incl. FLOOD_WAIT/SLOWMODE_WAIT `seconds`, per-class `Timeout` (30s simple, 120s paginated/raw, 600s story send, unbounded download), `ConfirmRequired{would}` confirm gate on the eight destructive ops (`msg delete`, `dialog delete`, `topic delete`, `story delete`, `contact remove`, `sticker remove`, `chat kick`, `chat leave`) requiring `"confirm":true` (stripped before parsing). Executor lanes: mutate lane strictly ordered single-worker shared across accounts, read lane dual-worker; bounded intake/queues (64) for backpressure; monotonic event `seq` per process with gap detection via `last_seq` + `stream.resync` per-account catch-up healing and bounded replay dedupe keyed by (account, chat, msg, pts); each account owns its connection + rate limiter and reconnects independently with a `Reconnected` row per account. Event rows share the listen serializers (`NewMessage`/`MessageEdited` flattened, `Reconnected`). Self-description via inline `ops.list`: all 78 routed ops across msg/dialog/topic/profile/privacy/contact/sticker/story/raw/chat/account/cache groups plus `ping`/`stream.resync`/`ops.list` (group `transport`), each entry `{op,summary,group,read_only,destructive,retry_safe}` | `tele serve --account NAME [--account NAME ...]` (`src/commands/serve.rs` + shared cores in command modules) | done |
| kernel.doctor | Local health check: app dir, config parse, credential presence (never values), session files, private permissions; no network, explicit `--account` never triggers RPC (single `local` outcome, exit 0 healthy / 3 findings / 1 usage) | `tele doctor` | done |
| kernel.wizard | Guided credential setup: api_id/api_hash prompts with keep-existing defaults, private .env + config writes, no network, no embedded credentials; `--dry-run` previews, `--no-input` fails closed | `tele wizard` | done |
| mcp.resources | Read-only MCP context resources | client-side | rmcp resources capability | `tele mcp` advertises `resources` with `tele://skill`, `tele://profile`, `tele://dialogs` (JSON/markdown; read-only mode included) | done |

## Counts

The numbers this file claims resolve as follows in the current tree:

- Raw registry names (`kernel.raw` row): 25. Recount in PowerShell:
  `$s = Get-Content src/commands/raw.rs -Raw; $i = $s.IndexOf('pub const REGISTERED'); $j = $s.IndexOf('];', $i); ([regex]::Matches($s.Substring($i, $j - $i), '"[^"]+"')).Count`
- Routed serve and MCP ops (`mcp` row): 81. Recount:
  `rg -c 'serve_route!\(' src/` (or `(Select-String -Path src\commands\*.rs,src\commands\*\*.rs -Pattern 'serve_route!\(').Count`)
- Every `done` row must expose a real CLI surface. The contract test enforces it: `cargo test --test contract -- done_rows_have_cli_surface`.

## Deliberate non-goals

- **Output i18n**: help, errors, and tables are English-only. `lang_code` config affects only the MTProto client identity sent to Telegram, never tele's output language. Revisit only if a real user need appears.
