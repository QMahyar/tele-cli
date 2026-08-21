# CAP-11: Admin-log depth

**Effort:** S/M · **Deps:** none · **Branch:** `feat/cap-11-admin-log`

## Goal

`chat admin-log` drops the acting admin (`event.user_id`) entirely, old/new value pairs, ban-until/rights detail, pinned ids, and offers no filters.

## Acceptance criteria

- [ ] Rows gain additive `actor` (user id + resolvable name via cached peers) and richer `action` payloads: old/new values for title/about/username, ban-until timestamp + rights diff on toggle_ban/admin, photo payload summary, pinned message id, invite link on join_by_invite.
- [ ] Filters: `--admin <user>` (admins list param), `--search <q>`, `--since/--until <ts>`, `--events <csv>` mapped to channel.AdminLogEventsFilter flags.
- [ ] Human table stays readable (truncate policy like existing char-safe truncation); JSON additive only; offline shaping tests with fixtures; docs updated.
