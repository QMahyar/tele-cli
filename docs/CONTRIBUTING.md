# Contributing to tele

This guide is for human developers and AI agents working on the tele codebase. It covers architecture, setup, conventions, and how to add a new command.

## Architecture

The codebase has three layers:

```
main.rs          clap derive, global flags, subcommand dispatch
commands/        one file per command group (msg.rs, chat.rs, etc.)
  mod.rs         shared validators (validate_limit, require_chat_target, parse_unixtime)
  helpers.rs     peer_id(), stats_*() shared utilities
  credentials.rs creds(), creds_api_id() shared across commands
  serve.rs       duplex JSONL server (owns one session, select! loop)
  mcp.rs         MCP stdio server (rmcp 3.1, tools/list + tools/call)
src/             kernel (config, session, client, executor, output, serialize, entities, error)
tests/
  contract.rs    integration tests that spawn the binary as a subprocess
  selection.rs   account-selection unit tests
docs/            specs, contract, security, architecture decisions
```

**Data flow for a command:**

1. `main.rs` parses CLI args via clap derive.
2. The subcommand enum dispatches to `commands/<group>.rs::run()`.
3. `run()` validates args offline (before any network call). Validation returns `TeleError::Usage` on bad input.
4. For dry-run, the command builds a `would` payload and returns without connecting.
5. For real runs, `run_fanout()` or `run_one()` from `executor.rs` resolves accounts, acquires session locks, and runs a per-account closure against a `ClientGuard`.
6. The closure connects via `client.rs`, makes RPC calls through grammers, and returns `TeleResult<Value>`.
7. `output.rs` formats the result as a human table or JSON envelope.

**Key abstractions:**

- `ClientGuard` (client.rs): owns the grammers `Client` and session lock. Disconnects on drop.
- `GlobalFlags` (executor.rs): parsed global args (`--json`, `--dry-run`, `--account`, etc.) threaded through every command.
- `TeleError` (error.rs): typed error enum. `Usage` = exit 1, `Telegram` = exit 3, `Auth` = exit 4.
- `Envelope` (output.rs): wraps per-account results into the JSON envelope on stdout.

## Getting started

```bash
git clone https://github.com/QMahyar/tele-cli.git
cd tele-cli
cargo build
cargo test                    # all offline, no network
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Run a smoke-test:

```bash
cargo run -- --help
cargo run -- account list
```

Tests never touch Telegram. They run in isolated temp dirs with fake configs. No credentials needed.

## Adding a new command

The pattern is the same across all 16 command groups. Here is the minimal recipe using `contact.rs` as a reference.

### 1. Define the subcommand enum and args

```rust
// src/commands/contact.rs
use clap::{Args, Subcommand};
use crate::error::{TeleError, TeleResult};
use crate::executor::{run_fanout, GlobalFlags};

#[derive(Subcommand)]
pub enum ContactCmd {
    List(ListArgs),
    Add(AddArgs),
}

#[derive(Args, Clone)]
pub struct ListArgs {
    #[arg(long, default_value_t = 100, help = "max contacts to list (1-10000)")]
    limit: u32,
}

#[derive(Args, Clone)]
pub struct AddArgs {
    #[arg(long, help = "user to add: @username, numeric ID, +phone, or me")]
    user: String,
}
```

### 2. Write the run function

```rust
pub async fn run(cmd: ContactCmd, flags: &GlobalFlags) -> TeleResult<i32> {
    match cmd {
        ContactCmd::List(a) => list(a, flags).await,
        ContactCmd::Add(a) => add(a, flags).await,
    }
}
```

### 3. Implement each subcommand

```rust
async fn list(args: ListArgs, flags: &GlobalFlags) -> TeleResult<i32> {
    // Validate offline (before any connect)
    validate_list(&args)?;

    // Clone what the closure needs
    let config_path = flags.config_path.clone();
    let dry_run = flags.dry_run;
    let json = flags.json;

    // run_fanout fans out across --account/--tag/all
    let envelope = run_fanout(flags, move |name| {
        let config_path = config_path.clone();
        let args = args.clone();
        Box::pin(async move {
            if dry_run {
                return list_dry_run(&args);
            }
            let (_guard, client, _sender) =
                crate::client::connect(&name, &config_path).await?;
            // ... RPC calls through `client` ...
            // Return Ok(Value) for JSON envelope
        })
    }).await?;

    crate::output::printEnvelope(&envelope, json, flags.jsonl)?;
    Ok(crate::output::exit_code(&envelope))
}
```

### 4. Wire it into mod.rs and main.rs

In `src/commands/mod.rs`, add:

```rust
pub mod contact;
```

In `src/main.rs`, add the subcommand to the `Cli` enum:

```rust
#[derive(Subcommand)]
enum Commands {
    // ... existing ...
    #[command/about = "Contacts: list, add, block/unblock")]
    Contact {
        #[command(subcommand)]
        cmd: contact::ContactCmd,
    },
}
```

And add a dispatch arm in the match:

```rust
Commands::Contact { cmd } => contact::run(cmd, &flags).await?,
```

### 5. Add tests

- Unit tests go in the same file inside `#[cfg(test)] mod tests {}`. Use `#[test]` for validation and dry-run shape tests. No network calls.
- If your command affects the CLI contract (JSON shapes, exit codes), add a contract test in `tests/contract.rs` that spawns the binary.

### 6. Update docs

- Add a row to `docs/capabilities.md` with status `want` (or `done` if shipped).
- If your command changes JSON shapes, update `docs/cli-contract.md`.

## Code conventions

- **No comments in code** unless someone explicitly asks for them.
- **No new dependencies** without checking the impact on build time and binary size. Ask first.
- **Validation before connect.** Every flag value validates offline. `TeleError::Usage` for bad input, never a raw string.
- **Dry-run is free.** The dry-run path builds a `would` payload and returns without connecting to Telegram.
- **`--json` is additive.** New fields may appear. Renamed or removed fields require a major version.
- **Secrets never logged.** Phone numbers, API keys, session strings, 2FA passwords, and QR tokens stay out of logs, JSON output, and process titles.
- **Stderr only for logs.** Machine output goes to stdout only.
- **Chat targets:** `--chat` accepts numeric id, `@username`, `t.me/...` link, `me`, or `+phone`. The phone branch parses before numeric id (because `"+98...".parse::<i64>()` succeeds).

## Testing strategy

| Test type | What | Where | How to run |
|---|---|---|---|
| Unit | Validation, dry-run shapes, serialization, helpers | `src/**/*.rs` (inline `#[cfg(test)]`) | `cargo test` |
| Contract | CLI binary behavior via subprocess: exit codes, JSON shapes, error messages | `tests/contract.rs` | `cargo test --test contract` |
| Selection | Account selection and tag matching | `tests/selection.rs` | `cargo test --test selection` |

All tests run offline. No Telegram connection is made. Tests use isolated temp dirs with fake configs.

## How AI agents should work here

AI agents should load `AGENTS.md` from the repo root every session. It contains:

- Tech stack and dependencies
- Project map with file-level descriptions
- Code conventions and patterns
- Common gotchas (grammers 0.10 quirks)
- The context hierarchy: load only what the task needs

Key rules for agents:

1. **Load AGENTS.md first.** It has the project map, conventions, and gotchas.
2. **Load the one spec slice you touch.** Do not paste all of `docs/capabilities.md`. Load only the rows matching your task.
3. **Load the file you will modify** and one example of the same pattern.
4. **Never guess an API.** If grammers has no friendly wrapper, use `tele raw` or check `docs/capabilities.md` for the registry entry.
5. **Validate before connect.** Test your validation offline before touching the network.
6. **Run `cargo clippy` and `cargo test`** before considering any task done.

## Release process

See [release.md](release.md) for the full process. Summary:

1. Confirm `main` is green (ci workflow passes).
2. Confirm `docs/capabilities.md` has no `want` rows.
3. Bump `version` in `Cargo.toml` and `Cargo.lock`.
4. Add a `## [X.Y.Z] - YYYY-MM-DD` section to `CHANGELOG.md`.
5. Commit, tag, push.

```bash
git tag -a -m "vX.Y.Z" vX.Y.Z
git push origin vX.Y.Z
```

The `release` workflow builds binaries, creates the GitHub Release, and publishes npm.
