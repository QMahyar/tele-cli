# Getting started with tele

This guide walks you through installing tele, setting up your first account, and running your first commands.

## Prerequisites

- A Telegram account (user account, not a bot)
- API credentials from [my.telegram.org](https://my.telegram.org)
- Node.js (for npm install) or Rust 1.89+ (to build from source)

## Step 1: Get API credentials

1. Go to [my.telegram.org](https://my.telegram.org)
2. Log in with your phone number
3. Go to "API development tools"
4. Create an application
5. Note your `api_id` (numeric) and `api_hash` (string)

## Step 2: Install tele

### Option A: npm (recommended)

```bash
npm install -g @qmahyar/telecli
tele --version
```

### Option B: binary download

Download a binary from [Releases](https://github.com/QMahyar/tele-cli/releases) for your platform.

### Option C: build from source

Requires [Rust 1.89+](https://www.rust-lang.org/tools/install).

```bash
git clone https://github.com/QMahyar/tele-cli.git
cd tele-cli
cargo build --release
target/release/tele --version
```

## Step 3: Configure credentials

Create the config directory and add your API credentials:

```bash
# Linux/macOS
mkdir -p ~/.config/tele
echo 'TELE_API_ID=1234567' > ~/.config/tele/.env
echo 'TELE_API_HASH=0123456789abcdef0123456789abcdef' >> ~/.config/tele/.env

# Windows (PowerShell)
mkdir -p "$env:APPDATA\tele"
Set-Content "$env:APPDATA\tele\.env" "TELE_API_ID=1234567`nTELE_API_HASH=0123456789abcdef0123456789abcdef"
```

## Step 4: Add an account

```bash
tele account add --name work
```

This prompts for your phone number. Then login:

```bash
# Code login (SMS)
tele account login --name work --method code --phone +1XXXXXXXXXX

# QR login
tele account login --name work --method qr
```

## Step 5: Start using tele

```bash
# Send yourself a message
tele msg send --chat me --text "hello from tele"

# List your recent dialogs
tele dialog list

# Get your profile
tele profile get

# Try a dry-run (no network calls)
tele msg send --chat me --text "test" --dry-run
```

## Multi-account usage

```bash
# Add multiple accounts
tele account add --name personal
tele account add --name work

# List all accounts
tele account list

# Target a specific account
tele msg send --account work --chat me --text "from work"

# Target by tag
tele msg send --tag work --chat me --text "from all work accounts"

# Parallel execution across tagged accounts
tele msg send --tag work --chat me --text "broadcast" --parallel 3
```

## JSON output for scripts

```bash
# Machine-readable JSON
tele msg send --chat me --text "test" --json

# JSON Lines for streaming
tele listen --events NewMessage --chat me --timeout-secs 10 --jsonl

# Use with jq
tele dialog list --json | jq '.results[].data[].name'
```

## MCP for AI agents

```bash
# Start MCP server for Claude/Cursor
tele mcp --account work

# Read-only mode
tele mcp --account work --read-only

# Filter tool groups
tele mcp --account work --groups msg,dialog
```

## Common patterns

### Dry-run before real commands

```bash
# Preview what would happen
tele msg delete --chat @group --ids 123 --dry-run

# Then execute
tele msg delete --chat @group --ids 123
```

### Filter listen events

```bash
# Only new messages from a specific chat
tele listen --events NewMessage --chat @team

# Messages matching a regex
tele listen --events NewMessage --pattern "deploy|release"

# Only incoming messages
tele listen --events NewMessage --in

# From a specific user
tele listen --events NewMessage --from @alice
```

### Export data

```bash
# Start takeout export
tele takeout start --account work

# Export data
tele takeout export --account work

# Finish takeout
tele takeout finish --account work
```

## Troubleshooting

### "session is in use by another process"

Another tele process is using this account. Wait for it to finish, or check for hanging processes.

### "auth required"

Run `tele account login --name NAME` to re-authenticate.

### "FLOOD_WAIT"

Telegram is rate-limiting. tele handles this automatically with exponential backoff. Wait the specified seconds.

### Verbose logging

```bash
# See what's happening
tele -v msg send --chat me --text "test"

# Debug level
tele -vv msg send --chat me --text "test"
```

## Next steps

- Read the [CLI Contract](cli-contract.md) for the full machine API reference
- Check [Security](security.md) for the threat model
- See [Contributing](CONTRIBUTING.md) if you want to add features
