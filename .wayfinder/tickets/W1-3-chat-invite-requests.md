# W1-3: chat.invite-check + join-request management

**Branch:** `feat/w1-3-invite-requests` · **Files:** `src/commands/chat.rs` only · **Deps:** none

## Goal

Preview invite links without joining; manage pending join requests.

## Acceptance

- [ ] `tele chat invite --check <LINK>` — NEW MODE on the existing five-mode invite command (house style: modes not extra subcommands); raw `messages.checkChatInvite`; handle every ChatInvite variant honestly (Already → resolved chat row; Peek/Invite → title, members/participants count where present, request_needed flag); reuse normalize_invite_link for input forms including bare +hash
- [ ] `tele chat requests --chat X` list mode via getChatJoinRequests (rows: from user, date, request text if any)
- [ ] Same command mutate modes: `[--approve|--dismiss]` with `[--user USER | --all]` wrapping hideChatJoinRequest / hideAllChatJoinRequests; clap conflicts_with between --all and --user; approve/dismiss require explicit --account and honor --dry-run like other mutators
- [ ] Error honesty: INVITE_HASH_INVALID etc. map through the existing error taxonomy (exit codes per docs)
- [ ] Offline tests first (TDD): arg-validation matrix (missing chat; approve without user/all; --all with --user conflict), link normalization cases, row-shaping fixtures with constructed TL values where feasible

## Boundaries

Only `src/commands/chat.rs`. Subcommand enums live in this file (no main.rs edit needed).
