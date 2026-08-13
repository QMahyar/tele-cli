# Tele-Cli (`tele`)

A Rust CLI that operates **real Telegram phone accounts** at native-user depth — one
session per account, messages, chats, dialogs, contacts, privacy, takeout, and live
update streaming. Built on [grammers](https://docs.rs/grammers-client) 0.10 (MTProto).

- **Many accounts, one tool.** Name them, tag them, fan out a command across all of
  them — sequentially by default, `--parallel 1–3` when you need speed.
- **For humans and agents.** comfy-tables for you; one JSON envelope (or JSONL from
  `listen`) on stdout for scripts and AI agents. Logs go to stderr only.
- **No bot tokens.** Full user-client surface: scheduled sends, forum topics, admin
  log, takeout export, raw TL calls via a typed registry.

**Status:** pre-release `0.1.0`. Capability matrix has no open `want` rows (release
gate met, see [ADR-005](docs/decisions/005-unpublished-until-want-done.md)); the
remaining product surface is [Phase 6](tasks/plan.md) (MCP server + agent skill, ask
first). CI and publishing are not set up yet — see [docs/release.md](docs/release.md).

## Quickstart

Requires a stable Rust toolchain. Build once:

```
cargo build --release
```

The binary is `target/release/telecli` (`telecli.exe` on Windows); rename it to
`tele` if you like — that's the name used throughout this README.

**1. Set up the app data dir.** Everything lives here — never in the repo:

PowerShell (Windows):

```powershell
$dir = Join-Path $env:APPDATA "telecli"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item .env.example (Join-Path $dir ".env")
```

Bash (Linux/macOS):

```bash
mkdir -p ~/.config/telecli
cp .env.example ~/.config/telecli/.env
```

**2. Fill in your API credentials** in `{app-dir}/.env` (from
my.telegram.org — **never commit these**):

```
TELE_API_ID=1234567
TELE_API_HASH=0123456789abcdef0123456789abcdef
```

Process environment overrides the file, so `TELE_API_ID=... tele ...` works too.

**3. Register and log in an account** (prompts for the SMS code; `--password` for
2FA; `--method qr` to scan a QR instead):

```bash
tele account add --name work --tags iran,work
tele account login --name work --method code --phone +1XXXXXXXXXX
tele account status --account work
```

**4. Send your first message** (try `--dry-run` first — it touches nothing):

```bash
tele msg send --account work --chat me --text "hello from tele" --dry-run
tele msg send --account work --chat me --text "hello from tele"
```

## Command tour

Account selection is global: `--account NAME` (defaults to **all** sessions) or
`--tag TAG` (accounts having that tag *and* a session). All commands accept
`--json`, `--dry-run`, `--config PATH`.

### Accounts

```
tele account list                    # sessions + tags
tele account login --name work --method code --phone +1XXXXXXXXXX
tele account login --name work --method qr            # QR on stderr
tele account logout --name work      # server sign-out + delete session
tele account remove --name work      # local delete only
```

### Messages

```
tele msg send --chat @username --text "hi"                    # --format markdown, --silent, --reply ID, --no-preview
tele msg send --chat me --file ./doc.pdf --caption "here"     # images go as photos
tele msg send --chat me --text "later" --schedule 2026-08-14T09:00:00Z
tele msg get  --chat me --limit 5 --json                      # id, date, sender, text
tele msg edit --chat me --id 42 --text "v2"
tele msg delete --chat me --ids 42,43     # or --all
tele msg forward --from @news --to me --ids 100,101
tele msg react --chat me --id 42 --reaction 👍    # --remove
tele msg search --chat me --query "invoice"
tele msg download --chat me --id 42 --out ./media
tele msg read --chat me                      # --mark-unread to invert
```

`--schedule` accepts a Unix timestamp or RFC3339. Pinning is `tele msg pin --chat me
--id 42` (`--unpin` to remove).

### Chats & groups

```
tele chat join --chat https://t.me/joinchat/AbCdEf           # invite link or @username
tele chat create --kind channel --title "Announcements" --description "..."
tele chat participants --chat @somegroup --limit 100
tele chat kick --chat @somegroup --user @spammer
tele chat admin --chat @somegroup --user @mod --promote --title "Mod"
tele chat admin-log --chat @somegroup                       # admin actions
tele chat stats --chat @somegroup                           # --broadcast for channels
tele chat invite --chat @somegroup --user @friend
tele chat leave --chat @somegroup
```

### Dialogs & topics

```
tele dialog list --limit 20                # --folder 1 = archive
tele dialog drafts
tele dialog archive --chat @old           # --unarchive to restore
tele dialog delete --chat @old            # leave + delete history
tele topic create --chat @forum --title "Off-topic" --emoji 🎮
tele topic list --chat @forum
```

### Contacts, profile, privacy

```
tele contact list --limit 50
tele contact add --user @friend --first "Jane" --last "Doe"
tele contact block --user @spammer        # --unblock to undo
tele profile get                          # your own profile (--chat me)
tele profile get --chat @friend           # any user
tele profile set --name "New Name" --bio "…" --photo ./pic.jpg
tele privacy get --key phone_number
tele privacy set --key status --allow @friend,@colleague --deny @rival
```

Privacy keys: `status`, `profile_photo`, `phone_number`, `calls`, `forwards`,
`chat_invite`, `added_by_phone`, `voice_messages`, `about`.

### Takeout (data export)

```
tele takeout start --contacts --messages --photos    # returns a takeout session id
tele takeout export --message-limit 1000             # writes to {app-dir}/export/{account}/
tele takeout finish
```

Exports `contacts.json`, `dialogs.json`, and `messages.jsonl` per account.

### Listen (live update stream)

`listen` streams **JSON Lines on stdout**, one update per line, until Ctrl-C
(`--timeout-secs N` to bound it):

```
tele listen --account work --events NewMessage,MessageEdited,MessageDeleted --chat me
```

`--events` is an allowlist (default `NewMessage`); unknown names exit before
connecting. `--raw` switches to raw `Update` dumps. Events:

| event | JSONL `type` | payload |
|---|---|---|
| `NewMessage` | `new_message` | message JSON + `account` |
| `MessageEdited` | `message_edited` | message JSON + `account` |
| `MessageDeleted` | `message_deleted` | `chat_id`, `ids` |
| `Raw` | `update` / `raw` | debug dump |

### Raw TL

The typed registry — one handler per supported TL method; unregistered names fail
with a pointer to add an arm (see [docs/cli-contract.md](docs/cli-contract.md)):

```
tele raw messages.ExportChatInvite --args '{"chat": "@somegroup", "usage_limit": 100}'
tele raw contacts.Search --args '{"q": "Jane", "limit": 5}'
tele raw messages.GetAllDrafts
```

Registered: `account.UpdateProfile`, `contacts.Search`, `messages.ExportChatInvite`,
`messages.GetAllDrafts`, `stats.GetBroadcastStats`, `stats.GetMegagroupStats`.

## Machine output

`--json` on one-shot commands prints a single JSON object on stdout:

```json
{"ok":true,"accounts":[{"account":"work","ok":true,"data":{"id":928,"date":"2026-08-13T12:00:00+00:00","out":true,"peer":{"id":123,"kind":"user","name":"me"},"sender":{"id":123,"kind":"user","name":"me"},"text":"hello from tele"}}]}
```

`data` is per-command and additive; failures carry `"error": "..."` and exit codes
reflect per-account results. Dry runs return `ok: true, data.dry_run: true` and make
**no network calls**.

Exit codes: `0` all succeeded · `1` usage/validation (bad selection, bad JSON args,
unknown raw name) · `2` clap parse errors, or partial success (some accounts failed)
· `3` all accounts failed (Telegram/IO) · `4` auth required.

## Configuration

`{app-dir}/config.toml` (created by `tele account add`; `--config PATH` overrides):

```toml
flood_sleep_threshold = 60
parallel_max = 3

[proxy]                              # optional global proxy (socks5 only)
type = "socks5"
host = "127.0.0.1"
port = 9050

[accounts.work]
tags = ["iran", "work"]

[accounts.work.proxy]                # per-account override
type = "socks5"
host = "127.0.0.1"
port = 1080
```

- Proxy is **socks5-only** (grammers 0.10); `type = "http"` fails with a clear
  error. Empty host/port = no proxy.
- `--parallel` is clamped to 1–3 at runtime.
- App data dir: `%APPDATA%\telecli` on Windows, `$XDG_CONFIG_HOME/telecli` or
  `~/.config/telecli` elsewhere. Override with `TELE_APP_DIR`.

## Security notes

- **Secrets live outside the repo**: `.env` (api_id/api_hash), `config.toml`
  (proxy, tags), and `sessions/{name}.session` all live under the app data dir.
  `.env`, sessions, and Cargo.lock are gitignored; never commit them.
- One session file per account, one client per session. Never share a session
  across processes.
- Structured logs on stderr only; secrets, phone numbers, and session data are
  never logged. Set `TELE_LOG=debug` (or `trace` for grammers internals) to see
  them.
- `--chat` accepts numeric id (cached access hash required), `@username`,
  `t.me/...` links, `me`, or `+phone` (imports the number as a contact — only
  works if the target's phone privacy allows it). See
  [docs/security.md](docs/security.md) for the full threat model.
- `tele raw` is full account power: it can call anything the account can. `--dry-run`
  is honored (no invocation), but treat raw calls with care.

## Live verification status

Verified 2026-08-13 against real sessions: account status/list, send/get/edit/delete
round-trip, cross-account `listen` (account 2 → account 1, JSONL received), profile
get, takeout export, raw registry (registered → ok, unknown → exit 1), all dry-runs,
and the proxy negative path. Still user-side to verify: 2FA/QR login, chat
participants/admin-log on a real group, socks5 positive path, `--file`/`--schedule`,
MessageEdited/MessageDeleted/Raw listen events, takeout finish, logout.

## Docs

- [Capability matrix](docs/capabilities.md) — the spine: every Telegram domain, its
  grammers path, CLI command, and status
- [CLI contract](docs/cli-contract.md) — exit codes, JSON envelope, listen JSONL, raw
  registry (the machine API)
- [Security](docs/security.md) — threat model and boundaries
- [Observability](docs/observability.md) — stderr logging
- [Release](docs/release.md) — versioning, CI plan, publish path
- [Spec](docs/spec.md) and [product intent](docs/ideas/tele-cli.md)
- [ADRs](docs/decisions/) — 001–006
- [Tasks](tasks/todo.md) and [plan](tasks/plan.md)
