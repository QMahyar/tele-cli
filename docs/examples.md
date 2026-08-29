# Examples

Practical examples of using tele for common tasks.

## Messaging

### Send a text message

```bash
tele msg send --chat me --text "hello from tele"
tele msg send --chat @team --text "deploy complete"
tele msg send --chat 123456789 --text "hello"
```

### Send a file

```bash
tele msg send --chat me --file ./photo.jpg
tele msg send --chat me --file ./doc.pdf --text "here's the document"
```

### Send an album (2-10 files)

```bash
tele msg send --chat me --file ./a.jpg --file ./b.jpg --file ./c.jpg
```

### Schedule a message

```bash
tele msg send --chat @team --text "good morning" --schedule 1723500000
tele msg send --chat @team --text "wake up" --schedule online
```

### Send with modifiers

```bash
tele msg send --chat me --text "protected" --noforwards
tele msg send --chat me --text "background" --background
```

### Edit a message

```bash
tele msg edit --chat @team --id 123 --text "updated text"
```

### Delete messages

```bash
tele msg delete --chat @team --id 123
tele msg delete --chat @team --id 123,456,789
tele msg delete --chat me --id 123 --self-only
```

### Forward messages

```bash
tele msg forward --chat @team --id 123 --to @other
```

### Search messages

```bash
tele msg search --chat @team --query "deploy"
tele msg search --global --query "important"
tele msg search --chat @team --query "error" --limit 10
```

### React to a message

```bash
tele msg react --chat @team --id 123 --emoji "👍"
tele msg react --chat @team --id 123 --emoji "🔥"
```

### Download media

```bash
tele msg download --chat @team --id 123
tele msg download --chat @team --id 123 --chunk-size-kb 256
```

### Pin a message

```bash
tele msg pin --chat @team --id 123
tele msg pin --chat @team --id 123 --notify
tele msg pin --chat @team --unpin
```

### Vote in a poll

```bash
tele msg vote --chat @team --id 123 --option 1
tele msg vote --chat @team --id 123 --option 1,3
```

### Click an inline button

```bash
tele msg click --chat @bot --id 123 --button "OK"
tele msg click --chat @bot --id 123 --button-index 1
```

### Bot buttons

```bash
tele msg click --chat @bot --id 123 --button-index 1
tele msg click --chat @bot --id 123 --button-contains "ساخت پنل"
```

On ambiguous substring (≥2 matches) the command exits 1 with `Did you mean #i "text" or #j "text"? Available: [#1 "…", #2 "…"]` — use the shown 1-based index.

## Chat management

### Join a chat

```bash
tele chat join --target @publicgroup
tele chat join --target https://t.me/joinchat/ABC123
```

### Leave a chat

```bash
tele chat leave --target @group
```

### Create a chat

```bash
tele chat create --title "My Group" --type group
tele chat create --title "My Channel" --type channel
```

### List participants

```bash
tele chat participants --target @group
tele chat participants --target @group --role admin
tele chat participants --target @group --search "alice"
```

### Kick or ban a user

```bash
tele chat kick --target @group --user @spammer
tele chat kick --target @group --user @spammer --ban
tele chat kick --target @group --user @spammer --ban --duration 3600
```

### Manage admins

```bash
tele chat admin --target @group --user @alice --rights admin
tele chat admin --target @group --user @alice --rights moderator
```

### View admin log

```bash
tele chat admin-log --target @group
tele chat admin-log --target @group --limit 20
tele chat admin-log --target @group --admin @alice
```

### Chat statistics

```bash
tele chat stats --target @channel
```

### Manage invite links

```bash
tele chat invite --target @group --user @alice
tele chat invite --target @group --title "Team Link" --expire 7d
tele chat invite --target @group --list
tele chat invite --target @group --delete-revoked
```

### Chat settings

```bash
tele chat settings --target @channel
tele chat settings --target @channel --slow-mode 300
tele chat settings --target @channel --signatures on
tele chat settings --target @channel --join-request on
```

### Edit chat info

```bash
tele chat edit --target @group --title "New Name"
tele chat edit --target @group --about "Updated description"
tele chat edit --target @group --photo ./new-photo.jpg
tele chat edit --target @group --photo remove
```

## Dialogs

### List dialogs

```bash
tele dialog list
tele dialog list --limit 20
tele dialog list --json
```

### Work with drafts

```bash
tele dialog drafts
tele dialog draft --chat @team --text "TODO: fix bug"
tele dialog draft --chat @team --clear
```

### Pin/unpin dialogs

```bash
tele dialog pin --chat @team
tele dialog pin --chat @team --unpin
```

### Archive/unarchive

```bash
tele dialog archive --chat @oldgroup
tele dialog archive --chat @oldgroup --unarchive
```

### Delete dialogs

```bash
tele dialog delete --chat @oldgroup
tele dialog delete --chat @alice --revoke
```

## Forum topics

### List topics

```bash
tele topic list --chat @forum
```

### Create a topic

```bash
tele topic create --chat @forum --title "New Discussion"
```

### Manage topics

```bash
tele topic close --chat @forum --topic 1
tele topic reopen --chat @forum --topic 1
tele topic edit --chat @forum --topic 1 --title "Updated Title"
tele topic delete --chat @forum --topic 1
tele topic pin --chat @forum --topic 1
```

## Contacts

```bash
tele contact list
tele contact add --user @alice
tele contact remove --user @alice
tele contact block --user @spammer
tele contact unblock --user @alice
```

## Profile

```bash
tele profile get
tele profile set --first-name "New Name"
tele profile set --bio "Updated bio"
tele profile set --username newname
tele profile set --username remove
tele profile photo --remove
tele profile emoji-status --emoji 1234567890
tele profile emoji-status --remove
```

## Privacy

```bash
tele privacy get --key phone_number
tele privacy get --key profile_photo
tele privacy set --key phone_number --rule nobody
tele privacy set --key forwards --rule contacts
```

## Stories

```bash
tele story list
tele story send --file ./photo.jpg --caption "My story"
tele story send --file ./video.mp4 --privacy contacts --period 86400
tele story read --max-id 100
tele story delete --ids 100
tele story pin --ids 100
tele story unpin --ids 100
```

## Stickers

```bash
tele sticker list
tele sticker search --query "cat"
tele sticker show --set AnimalPaws
tele sticker install --set AnimalPaws
tele sticker remove --set AnimalPaws
```

## Live streaming

### Basic listening

```bash
tele listen
tele listen --events NewMessage --timeout-secs 30
```

### Filter events

```bash
tele listen --events NewMessage,MessageEdited
tele listen --events NewMessage --chat @team
tele listen --events NewMessage --from @alice
tele listen --events NewMessage --in
tele listen --events NewMessage --out
```

### Filter by pattern

```bash
tele listen --events NewMessage --pattern "deploy|release"
tele listen --events NewMessage --pattern "error|fail" --chat @alerts
```

### Album and service messages

```bash
tele listen --events Album --chat @media
tele listen --events Service --chat @group
tele listen --events ChatAction --chat @group
```

### Raw updates

```bash
tele listen --events Raw
tele listen --raw
```

## Raw TL

```bash
# List available methods
tele raw --help

# Call a method
tele raw contacts.Search --args '{"q":"alice","limit":10}'
tele raw messages.GetAllDrafts
tele raw users.GetUsers --args '{"id":[{"user_id":123456}]}'
```

## Multi-account

```bash
# List accounts
tele account list

# Target by name
tele msg send --account work --chat me --text "from work"

# Target by tag
tele msg send --tag bulk --chat @group --text "broadcast"

# Parallel execution
tele msg send --tag bulk --chat @group --text "fast" --parallel 5
```

## Machine output

```bash
# JSON envelope
tele msg send --chat me --text "test" --json

# JSON Lines
tele listen --events NewMessage --chat me --jsonl

# Dry-run with JSON
tele msg send --chat me --text "test" --json --dry-run

# Pipe to jq
tele dialog list --json | jq '.results[].data[].name'
```

## MCP integration

### Claude Desktop

Add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "tele": {
      "command": "tele",
      "args": ["mcp", "--account", "work"]
    }
  }
}
```

### Claude Code

Add to `.mcp.json`:

```json
{
  "mcpServers": {
    "tele": {
      "command": "tele",
      "args": ["mcp", "--account", "work"]
    }
  }
}
```

### Read-only mode

```bash
tele mcp --account work --read-only
```

### Filter tool groups

```bash
tele mcp --account work --groups msg,dialog
```
