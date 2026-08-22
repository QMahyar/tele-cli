# Wayfinder Map — hardening-and-capabilities

Label: wayfinder:map
Tracker: local markdown (`.wayfinder/tickets/`)
Plan: `tasks/plan.md`
Status: COMPLETE — all 23 tickets closed on `fix/hardening`; 837 tests green; ready for review/merge to main (push/tag pending user confirm)

## Destination

All 9 confirmed bugs from the 2026-08-21 deep-dive fixed, then the merged grammers-unused + feature-gap capability set implemented: clippy/fmt/tests green per ticket, contract additive-only, docs synced, shipped to `main` after user review.

## Notes

- Rust CLI, grammers-client 0.10.0 (TL layer 227). Read AGENTS.md before any ticket.
- Conventions: no comments in code; clippy `--all-targets -- -D warnings` + fmt + full test suite green per ticket; commit prefixes `fix|feat|refactor|test|docs:`; never push; never touch `main`.
- Branch flow (Phase 1): single checkout, NO worktrees (user decision — disk/build cost). Integration branch `fix/hardening` off `main`. Agents run SEQUENTIALLY; each cuts its ticket branch from `fix/hardening`, gates, merges back with `--no-ff`, leaving repo on `fix/hardening`.
- Phase 2 will use the same pattern on integration branch `feat/capabilities`.
- Build cache: normal `target/` (warm across tickets — no CARGO_TARGET_DIR override).

## Decisions so far

- [Merge feature gaps into capabilities phase] — user direction; grammers-unused friendly methods and feature gaps are one workstream organized by domain (recorded in tasks/plan.md).
- [BUG-1 rate-limiter zero-budget hang](tickets/BUG-1-rate-limiter-zero-budget.md) — budget <= 0 acts as unlimited; timeout-guarded tests.
- [BUG-2 per-RPC tokens](tickets/BUG-2-per-rpc-tokens.md) — acquire_for_items paces every 100 items in dialog list + msg get; privacy get acquires per key RPC; drafts was already 1:1 (single GetAllDrafts RPC, no change needed).
- [BUG-4 login UX cluster](tickets/BUG-4-login-ux-cluster.md) — TELE_PHONE env honored (warning truthful), 3-attempt code retry on same token, 3-attempt 2FA retry with PasswordToken refresh via account.GetPassword, phantom-session cleanup when unauthorized at entry, --qr-timeout-secs default 300 + transient stream tolerance, account add preserves tags unless --tags passed.
- [BUG-5 config tolerance](tickets/BUG-5-config-tolerance.md) — unknown per-account TOML keys survive rewrites (toml_edit key carry-over); empty per-account proxy table falls back to global.
- [BUG-6 stale-cache evict-retry-hint](tickets/BUG-6-stale-cache-retry.md) — PEER_ID_INVALID/CHANNEL_INVALID after cache hit falls through to uncached fallback then PEER_UNKNOWN_HINT; corrupt rows warn instead of silent miss. Note: no session API to delete a cached ref, so "evict" = fall-through retry; entry is overwritten on next successful write.
- [BUG-7 upload flood keys + name-split](tickets/BUG-7-upload-flood-namesplit.md) — root cause deeper than review claimed: io::Error::source() is always None for wrapped payloads so the old FLOOD_WAIT arm was dead code; fixed via get_ref() downcast, keys now parity with send path; split_full_name sends last_name only when present (server keeps existing otherwise), 64/64/140 caps as Usage errors.
- [BUG-3 contact add result parsing](tickets/BUG-3-contact-add-result.md) — parses returned User from AddContact Updates; additive `contact`/`mutual` JSON; honest failure on privacy block (row error instead of false added:true); warn on rename-overwrite. Note: grammers 0.10 has no UpdatesCombined variant name — it is `Updates::Combined`.
- [CAP-1 serialize enrichment](tickets/CAP-1-serialize-enrichment.md) — message JSON gains grouped_id/views/forwards/edit_date/reply_to/via_bot (omitted when absent); dialog rows gain pinned/unread_mark/unread_mentions/unread_reactions/last_message_date; contract doc updated.
- [CAP-4 download/pin/read extras](tickets/CAP-4-download-pin-read.md) — pin --show/--all/--notify (raw UpdatePinnedMessage for notify), read --mentions (clear_mentions), download --chunk-size-kb streaming via iter_download; `dialog unread` skipped as duplicate of existing msg read --mark-unread.
- [CAP-5 global search](tickets/CAP-5-global-search.md) — msg search --global via search_all_messages; per-chat path byte-identical.
- [CAP-2 topic scoping](tickets/CAP-2-topic-scope) — send-only: msg send --topic (reply-header semantics, conflicts --reply). get/search --topic NOT implemented: grammers Message::from_raw needs a PeerMap that cannot be constructed publicly, so raw Search results cannot be shaped as messages; reads documented to go through tele raw messages.Search (CAP-16).
- [CAP-3 album + builders](tickets/CAP-3-album-and-builders.md) — repeatable --file (2-10 = album via client.send_album), --media-ttl, --thumbnail (single doc), --url+--kind (photo_url/document_url), --copy-from/--copy-id (copy_media), --schedule online (sentinel 0 → schedule_once_online). upload_stream/stdin piping deferred (ergonomics need mime/name plumbing). Note: CAP-3 commit landed directly on fix/hardening (branch was lost to a dead agent); content identical to branch flow.
- [CAP-6 topic lifecycle](tickets/CAP-6-topic-lifecycle.md) — topic close/reopen/edit/delete/pin all raw (EditForumTopic, updatePinnedForumTopic, deleteTopicHistory — not deleteHistory as ticket guessed); list gains closed+pinned.
- [CAP-12 dialog extras](tickets/CAP-12-dialog-extras.md) — dialog draft set/clear (raw saveDraft), dialog pin/unpin (raw toggleDialogPin), delete gains left/cleared honesty + --revoke (raw deleteHistory for user peers).
- [CAP-7 moderation depth](tickets/CAP-7-moderation-depth.md) — participants --role/--search via friendly filters; admin rights anonymous/other/manage_topics completed (raw EditAdmin fallback where builder lacks setters); kick --ban/--duration/--rights via set_banned_rights.
- [CAP-8 chat settings](tickets/CAP-8-chat-settings.md) — settings toggles raw; read-back via GetFullChannel; --noforwards honest Usage error (no toggleNoforwards in layer 227).
- [CAP-9 chat metadata edit](tickets/CAP-9-chat-metadata-edit.md) — chat edit title/about/photo (raw, basic-group equivalents), photo remove via photos.deletePhotos, chat link get/set discussion group.
- [CAP-10 invite-link suite](tickets/CAP-10-invite-link-suite.md) — five-mode chat invite command wrapping raw messages.* constructors; SearchExportedChatInvites does not exist in this layer (getExportedChatInvites covers it).
- [CAP-11 admin-log depth](tickets/CAP-11-admin-log-depth.md) — actor resolution from response users vector, old/new payload depth across ~15 event kinds, server-side --admin/--search/--events filter, client-side --since/--until (TL has no timestamp bounds).
- [CAP-13 identity surface](tickets/CAP-13-contacts-profile-privacy.md) — contact remove + username in rows; profile username/photo-remove/emoji-status (EmojiStatus::Status/Empty — no InputEmojiStatus constructor); privacy 14/14 keys + --allow-chat/--deny-chat + overlap rejection; profile.* matrix row corrected.
- [CAP-14 listen upgrades](tickets/CAP-14-listen-upgrades.md) — Gap synthetic row via pts-delta tracking (--events Gap), Album coalescing with 500ms flush, bounded 10k id→peer map enables DM deletion matching under --chat. Dev-only tokio test-util feature added for paused-clock tests.
- [CAP-15 takeout upgrades](tickets/CAP-15-takeout-upgrades.md) — human-mode progress lines, per-dialog checkpoint resume (takeout.json checkpoints), finish --abandon success:false.
- [CAP-16 raw registry growth](tickets/CAP-16-raw-growth.md) — registry 6→18 methods; auth.session-ttl → done. folders.GetChatFolders skipped (constructor absent from layer 227).

## Tickets — Phase 1 (bugs; parallel-ready, file-disjoint)

- [BUG-1 rate-limiter zero-budget hang](tickets/BUG-1-rate-limiter-zero-budget.md) — `rate_limiter.rs`
- [BUG-2 per-RPC tokens on paginated iteration](tickets/BUG-2-per-rpc-tokens.md) — `dialog.rs` `privacy.rs` `msg.rs`
- [BUG-3 contact add result parsing](tickets/BUG-3-contact-add-result.md) — `contact.rs`
- [BUG-4 login UX cluster](tickets/BUG-4-login-ux-cluster.md) — `account.rs` `client.rs` `session.rs`
- [BUG-5 config tolerance](tickets/BUG-5-config-tolerance.md) — `config.rs`
- [BUG-6 stale-cache evict-retry-hint](tickets/BUG-6-stale-cache-retry.md) — `entities.rs`
- [BUG-7 upload flood keys + name-split](tickets/BUG-7-upload-flood-namesplit.md) — `msg.rs` `profile.rs`

## Tickets — Phase 2 (capabilities; blocking edges)

Frontier order (respect deps): CAP-1 → {CAP-3, CAP-4, CAP-5}; CAP-2 independent; CAP-6..CAP-16 independent of CAP-1.
Blocking: CAP-3 blocks nothing but shares msg.rs send path with CAP-2/CAP-4 — serialize those sequentially at merge time.
Full list: see `tasks/plan.md` index; ticket files `tickets/CAP-*.md`.

## Not yet specified

- macOS CI job (carried from previous effort): release.yml ships mac binaries ci.yml never tests.
- Live verification checklist expansion for new network features (needs real sessions; user-assisted).
- `toml 0.8→1.x` dependency bump effort.

## Out of scope

- Matrix `later` rows: polls, effects, checklists, translate, transcribe, ai-compose, listen.action/user/album full typing, stories, stickers.manage, business, stars.
- Matrix `never` rows unchanged. MCP/skill (Phase 6, ask first).
- raw.rs dev-facing error text change (contract-mandated verbatim).
