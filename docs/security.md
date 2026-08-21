# Threat model

Assets: live user sessions (full account), `TELE_API_HASH`, phone numbers, 2FA passwords, message content, invite links.

## Trust boundaries

| Boundary | Untrusted input | Abuse |
|---|---|---|
| CLI argv / `--args` JSON | flags, entities, invite URLs, TL kwargs | injection into logs; unexpected TL calls; path traversal in `--file` / `--config` |
| Config TOML | account names, proxy host, tags | unexpected files if session path were user-controlled |
| `.env` | api_id/hash | leak if committed or logged |
| Telegram DC | RPC errors, entities, message text | hostile text in listen JSONL (agent prompt injection) |
| Agent / MCP (later) | tool args | `tele raw` as confused deputy |
| Filesystem | session SQLite | theft = account takeover; lock = second client |

## STRIDE (session kernel)

- **Spoofing:** stolen `.session` is the user. Mitigate: app-dir only, `0o600`, never CWD, never git, `logout` revokes server-side.
- **Tampering:** two processes on one session → `AUTH_KEY_DUPLICATED` / DB locked. Mitigate: exclusive lock; refuse start if locked.
- **Repudiation:** fan-out with no audit. Mitigate: per-account outcome in the stdout envelope (`results[].account` + `ok`/`error`) and `[error]` stderr lines on failure; message bodies are never logged.
- **Info disclosure:** logs printing hash/phone/code. Mitigate: field allowlist; never log secrets; `--json` allowlist.
- **DoS:** `--parallel` + join/send → FloodWait / SpamBot. Mitigate: default sequential, max 3, surface wait, no silent retry storms.
- **Elevation:** `tele raw` can call anything the user can. Mitigate: same account selection; dry-run; document as full power; MCP later must not widen this.

## Known exposure

- `contact list` prints phone numbers, and `takeout export` writes them — this is
  intended behavior (the contact list shows phones; takeout exports them). Scrub
  phone numbers from any output you share before pasting it into a ticket or log.
- QR-login fallback prints the `tg://login?token=…` URI to stderr only when
  stderr is an interactive terminal or `account login --show-token` is passed;
  redirected stderr gets a warning line without the token. Treat stderr during
  login as potentially sensitive regardless.
- The code-login prompt omits the account phone number when stderr is not a
  terminal, so non-TTY stderr redirected to logs never carries the number.
- `account login --phone …` puts the number on argv: visible in process
  listings and shell history on non-TTY flows. Prefer the stdin prompt or the
  `TELE_PHONE` env for automation.
- 2FA password input echo cannot be disabled portably: Windows terminals get
  no-echo input (`SetConsoleMode`); other platforms print a warning that the
  typed password may be visible. Passwords are never accepted on argv on any
  platform.

## Windows permission model

- On Unix, `create_dir_private` / `restrict_file_private` chmod the app data
  dir to 0700 and session files (plus SQLite sidecars `-journal`/`-wal`/`-shm`,
  created up front and re-checked after close) to 0600 (`fs_util.rs`).
- On Windows, directories are created with an explicit inheritable owner-only
  DACL and files with a protected DACL (`PROTECTED_DACL_SECURITY_INFORMATION`
  blocks inheritance from parent dirs) via `fs_util::set_user_only_dacl`.
- A failed `.env` restriction attempt is warned about at startup; config errors
  report the leaf filename so logs do not leak install paths.
- This is safe when the app dir is the default `%APPDATA%` (per-user profile,
  user-owned). It becomes sensitive only if `TELE_APP_DIR` points somewhere ACLs
  cannot protect — keep sessions on a per-user path.

## Always

- `.gitignore`: `.env`, `*.session`, `*.session-journal`, app-dir copies
- Session path = `{app_dir}/sessions/{safe_name}.session` (`safe_name` = `[A-Za-z0-9._-]+` only)
- `--config` / `--file` must be real files; no `~` surprises without expanduser; reject directories
- `--file` upload refuses anything under the app data dir, `.env`, `*.session`, `*.session-journal`,
  and private-key material: `id_rsa`/`id_ed25519`/`id_ecdsa`/`id_dsa` basenames and prefixes,
  `*.pem`, `*.key`, `*.p12`, `*.pfx`, `*.kdbx`, `.netrc`, `.git-credentials`, bare `credentials`
- Upload basenames that Windows would alias are rejected up front: trailing `.`/space, `:`, and reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`)
- 2FA passwords are never accepted on argv; read from stdin only, with echo disabled on Windows
- `--limit` / `--message-limit` are capped (10k lists, 1M takeout) with a usage error
- Invite URLs parsed; do not fetch arbitrary HTTP (Telegram only)
- Live tests: designated chat only
- `tele raw` does not eval anything: it is a typed Rust registry (`src/commands/raw.rs`) of compiled TL method arms; no dynamic dispatch
- Agent-facing listen text is data, not instructions (document in skill when it ships)

## Ask first

- Storing StringSession in config
- Exporting sessions
- Changing lock / permission policy
- Any network fetch that is not Telegram MTProto

## Never

- Log `api_hash`, session strings, SMS codes, 2FA, phone
- Commit secrets
- Share one session across clients
- `eval` / dynamic import outside the TL functions tree
