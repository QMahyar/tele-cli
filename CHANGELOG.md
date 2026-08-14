# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [0.1.0] - 2026-08-13

### Added
- Account management: add, login (code + QR), logout, remove, status, list
- Messages: send (text, files, scheduled), get, edit, delete, forward, search, react, download, read, pin
- Chats: join, create, leave, participants, kick, admin, admin-log, stats, invite
- Dialogs: list, drafts, archive/unarchive, delete
- Topics: list, create
- Contacts: list, add, block/unblock
- Profile: get, set (name, bio, photo)
- Privacy: get, set (9 keys)
- Takeout: start, export, finish
- Listen: real-time JSONL streaming (NewMessage, MessageEdited, MessageDeleted, Raw)
- Raw TL: typed registry for supported TL methods
- Shell completions: bash, zsh, fish, powershell
- Multi-account with tag-based selection
- Parallel fan-out (1-3 accounts)
- SOCKS5 proxy support (global and per-account)
- JSON/JSONL machine output with structured envelope
- Dry-run mode for all commands
- Comprehensive test suite (268 tests)
- npm package for cross-platform installation
