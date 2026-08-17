# telecli

Telegram user-account CLI — messages, chats, groups, contacts, privacy, live streaming. No bot tokens.

## Install

```sh
npm install -g @qmahyar/telecli
```

Windows x64 only (the Rust binary is published for win32-x64). The npm package
bundles the binary; integrity is enforced by npm's tarball checksum.

Other platforms / manual install: see the [GitHub Releases](https://github.com/QMahyar/tele-cli/releases)
or `cargo install --locked telecli`.

## Setup

Credentials live outside the repo:

- `%APPDATA%\telecli\.env` with `TELE_API_ID` and `TELE_API_HASH` (from https://my.telegram.org)
- `tele account login` prompts interactively for the verification code or QR

## Usage

```sh
telecli account status
telecli msg send --chat @username --text "hi"
telecli msg get --chat @username
telecli chat participants --chat <id>
telecli listen --jsonl
telecli --help
```

`--json` / `--jsonl` is the machine API; human tables otherwise. Exit codes:
0 ok, 1 usage, 2 partial, 3 all-failed, 4 auth, 130 interrupted.

## Documentation

- [capabilities matrix](https://github.com/QMahyar/tele-cli/blob/main/docs/capabilities.md)
- [CLI contract](https://github.com/QMahyar/tele-cli/blob/main/docs/cli-contract.md)
- [security](https://github.com/QMahyar/tele-cli/blob/main/docs/security.md)

## License

MIT
