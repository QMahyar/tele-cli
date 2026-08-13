# Changelog

Curated, consumer-facing summary per [docs/release.md](docs/release.md). Written with
each change, not at release archaeology time. Categories: Added / Changed / Fixed /
Deprecated / Removed / Security. Version numbers come from git tags (`v0.1.0` …).

## Unreleased

First release candidate. All shipped surface accumulated during development;
versioned as `0.1.0` at first tag.

### Added

- **Kernel**
  - Rust/grammers CLI skeleton (`cargo build`, clap derive, tokio runtime; exit
    codes 0/1/2/3/4).
  - Config: `TELE_API_ID` / `TELE_API_HASH` from `{app-dir}/.env` (process env
    overrides), TOML `config.toml` with accounts, tags, global + per-account proxy,
    flood and parallel settings.
  - Named sessions: one SQLite `sessions/{name}.session` per account under the app
    data dir (never CWD), never two clients on one session.
  - Account selection: `--account NAME` (defaults to all sessions), `--tag TAG`
    (tag ∩ sessions), empty selection is a usage error.
  - Executor: sequential default, `--parallel N` clamped 1–3, per-account results
    (success/error/flood) always returned; `command_finished`-style per-account
    errors on stderr.
  - Output: `--json` single envelope, `--jsonl` flag, `--dry-run` on all mutating
    commands (no network), comfy-table human mode, logs on stderr only.
  - Structured stderr logs gated by `TELE_LOG` (`error|warn|info|debug|trace`).
  - Capability-matrix contract tests (`tests/contract.rs`, 14 tests, offline).
- **Accounts** (`tele account`)
  - `add` (name + tags), `list` (sessions + tags), `status` (authorized per
    account), `login` with `--method code` (SMS prompt, 2FA via `--password`),
    `--method qr` (raw `auth.exportLoginToken` + QR rendered on stderr), `logout`
    (server `sign_out` + session delete), `remove` (local delete only).
- **Messages** (`tele msg`)
  - `send`: text, markdown (`--format markdown`), `--schedule` (RFC3339 or Unix
    timestamp), `--file` (images as photos, otherwise documents), `--caption`,
    `--reply`, `--silent`, link preview toggle; `edit`, `delete` (comma-separated
    `--ids` or `--all`), `forward` (`--from`/`--to`/`--ids`, `--silent`), `pin` /
    `unpin`, `get` (`--limit`, `--offset-id`, `--last`), `read` / `--mark-unread`,
    `react` (`--reaction` emoji or `--remove`), `search`, `download` (`--out`,
    extension from media type).
- **Chats & groups** (`tele chat`)
  - `join` (invite link or username/channel), `leave` (channel leave / group
    `DeleteChatUser`), `invite` (group or channel), `participants` (id, name,
    role), `kick`, `admin` (`--promote` / `--demote` / `--title`), `admin-log`
    (raw `channels.GetAdminLog`, table + JSON), `stats` (`--broadcast` for
    channels, otherwise megagroup), `create` with `--kind group|supergroup|channel`
    and `--forum` for supergroups.
- **Dialogs & topics** (`tele dialog`, `tele topic`)
  - `dialog list` (`--limit`, `--folder` filter, unread + draft + last message),
    `drafts` (raw `messages.GetAllDrafts`), `archive` / `--unarchive` (folders
    `EditPeerFolders`), `delete` (`delete_dialog`); `topic create` (`--emoji`),
    `topic list`.
- **Contacts, profile, privacy** (`tele contact`, `tele profile`, `tele privacy`)
  - `contact list` / `add` (`--user`, `--first`, `--last`, `--phone`) / `block` /
    `unblock` (raw `contacts.*`).
  - `profile get` (self or `--chat`, incl. phone, username, bio), `profile set`
    (`--name`, `--bio`, `--photo` upload + set).
  - `privacy get` (all or `--key`), `privacy set` (`--key`, `--allow`/`--deny`
    comma lists) for 9 keys (status, profile_photo, phone_number, calls,
    forwards, chat_invite, added_by_phone, voice_messages, about).
- **Takeout** (`tele takeout`)
  - `start` (`--contacts`/`--messages`/`--photos` → takeout session id), `export`
    (`--message-limit`, writes `contacts.json`, `dialogs.json`, `messages.jsonl`
    to `{app-dir}/export/{account}/`), `finish`.
- **Listen** (`tele listen`)
  - JSONL update stream on stdout: `NewMessage` (default), `MessageEdited`,
    `MessageDeleted`, `Raw` via `--events` allowlist (validated before connect),
    `--chat` filter, `--timeout-secs`, `--raw` raw update dumps.
- **Raw TL registry** (`tele raw`)
  - Typed registry: `account.UpdateProfile`, `contacts.Search`,
    `messages.ExportChatInvite` (usage_limit / expire_date / request_needed /
    title), `messages.GetAllDrafts`, `stats.GetBroadcastStats`,
    `stats.GetMegagroupStats`; `--args` JSON; unregistered names exit 1 before
    connect; honors `--dry-run`.
- **Proxy** — global and per-account SOCKS5 via `SenderPool::with_configuration`
  (`ConnectionParams.proxy_url`); per-account overrides global; unsupported proxy
  types error clearly.
- **Peer resolution** (`--chat` / `--user` targets) — numeric id (cached access
  hash), `@username`, `t.me/…` links, `me` (via `get_me`), `+phone` via raw
  `contacts.ImportContacts`.
- **JSON envelope** — `command` field populated with the invoked subcommand path
  (e.g. `"msg send"`); flood-wait errors carry `seconds` on the per-account
  `error` object (absent otherwise).

### Fixed

- `resolve_peer(InputPeerSelf)` in grammers 0.10 fails with a misleading
  `InvocationError::Dropped` ("request error: dropped (cancelled)"); `--chat me`
  now uses `client.get_me()`.
- Phone targets (`+98…`) parse as `i64`, so the phone branch runs before the
  numeric-id branch (a `+` prefix makes `parse::<i64>()` succeed).
- `tele listen --events Bogus` exits 1 before connecting (allowlist validated
  upfront); `tele raw <unregistered>` exits 1 before connecting.
- Unknown subcommands exit 2 (clap) instead of confusingly falling through.

### Security

- api_hash, session strings, phone numbers, and 2FA passwords are never logged;
  session files and `.env` are gitignored.
- One client per session file; `logout` revokes server-side before deleting the
  session.
