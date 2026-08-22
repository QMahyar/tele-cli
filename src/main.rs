mod client;
mod commands;
mod config;
mod entities;
mod error;
mod executor;
mod fs_util;
mod logging;
mod output;
mod rate_limiter;
mod serialize;
mod session;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use commands::*;
use executor::GlobalFlags;

#[derive(Parser)]
#[command(
    name = "tele",
    version,
    about = "Telegram user-account CLI (grammers)",
    disable_help_subcommand = true
)]
struct Cli {
    #[arg(
        long,
        global = true,
        action = clap::ArgAction::Append,
        help = "account name (NAME or all; repeatable)"
    )]
    account: Vec<String>,
    #[arg(
        long,
        global = true,
        action = clap::ArgAction::Append,
        help = "select accounts by config tag (repeatable)"
    )]
    tag: Vec<String>,
    #[arg(
        long,
        global = true,
        help = "parallel accounts (1-32; default from config parallel_max)"
    )]
    parallel: Option<u32>,
    #[arg(
        long,
        global = true,
        conflicts_with = "jsonl",
        help = "machine output: single JSON envelope"
    )]
    json: bool,
    #[arg(
        long,
        global = true,
        conflicts_with = "json",
        help = "machine output: JSON lines (one-shot commands emit a single envelope line)"
    )]
    jsonl: bool,
    #[arg(long, global = true, help = "validate without touching Telegram")]
    dry_run: bool,
    #[arg(long, short = 'q', global = true, help = "suppress stderr logs")]
    quiet: bool,
    #[arg(
        long,
        short = 'v',
        global = true,
        action = clap::ArgAction::Count,
        help = "verbose stderr logs (-vv = debug)"
    )]
    verbose: u8,
    #[arg(long, global = true, help = "config.toml path")]
    config: Option<std::path::PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage accounts (sessions, login, logout)
    #[command(subcommand)]
    Account(account::AccountCmd),
    /// Messages: send, edit, delete, forward, pin, get, read, react, search, download
    #[command(subcommand)]
    Msg(msg::MsgCmd),
    /// Chats: join, leave, invite, participants, kick, admin, admin-log, stats, create
    #[command(subcommand)]
    Chat(chat::ChatCmd),
    /// Dialogs: list, drafts, draft, archive, pin, delete
    #[command(subcommand)]
    Dialog(dialog::DialogCmd),
    /// Forum topics
    #[command(subcommand)]
    Topic(topic::TopicCmd),
    /// Contacts: list, add, remove, block, unblock
    #[command(subcommand)]
    Contact(contact::ContactCmd),
    /// Profile: get, set, photo, emoji-status
    #[command(subcommand)]
    Profile(profile::ProfileCmd),
    /// Privacy rules
    #[command(subcommand)]
    Privacy(privacy::PrivacyCmd),
    /// Account export (takeout)
    #[command(subcommand)]
    Takeout(takeout::TakeoutCmd),
    /// Stream updates as JSONL
    Listen(listen::ListenArgs),
    /// Raw TL invocation (typed registry)
    Raw(raw::RawArgs),
    /// Generate shell completions
    #[command(subcommand)]
    Completions(completions::Shell),
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn main() -> std::process::ExitCode {
    logging::init();
    let matches = match Cli::command().try_get_matches() {
        Ok(matches) => matches,
        Err(e) => {
            let code = if e.use_stderr() {
                error::EXIT_USAGE
            } else {
                error::EXIT_OK
            };
            let _ = e.print();
            if e.use_stderr() && std::env::args_os().any(|a| a == "--json" || a == "--jsonl") {
                let hint = argv_command_hint().unwrap_or_default();
                let error_json = serde_json::json!({"type": "UsageError", "message": strip_ansi(&e.to_string())});
                let envelope = output::Envelope::failed(false, &hint, error_json);
                if let Ok(v) = serde_json::to_value(&envelope) {
                    let _ = output::print_json(&v);
                }
            }
            std::process::exit(code);
        }
    };
    let cli = Cli::from_arg_matches(&matches).expect("clap matches parse");
    let flags = GlobalFlags {
        account: cli.account,
        tag: cli.tag,
        parallel: cli.parallel,
        json: cli.json,
        jsonl: cli.jsonl,
        dry_run: cli.dry_run,
        quiet: cli.quiet,
        config_path: cli.config,
        command: invoked_path(&matches),
    };
    logging::set_flags(cli.verbose, flags.quiet);
    if let Some(p) = flags.parallel {
        if !(1..=32).contains(&p) {
            output::log_line("warn", &format!("--parallel {p} is outside 1-32; clamped"));
        }
    }
    if flags.json && flags.jsonl {
        let message = "--json and --jsonl are mutually exclusive; pick one";
        output::log_line("error", message);
        let error_json = serde_json::json!({"type": "UsageError", "message": message});
        let envelope = output::Envelope::failed(false, &flags.command, error_json);
        if let Ok(v) = serde_json::to_value(&envelope) {
            let _ = output::print_json(&v);
        }
        std::process::exit(error::EXIT_USAGE);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = runtime.block_on(async {
        tokio::select! {
            code = run_command(cli.command, &flags) => code,
            _ = tokio::signal::ctrl_c() => error::EXIT_INTERRUPTED,
        }
    });
    std::process::ExitCode::from(code.clamp(0, 255) as u8)
}

pub(crate) fn command_for_completions() -> clap::Command {
    Cli::command()
}

fn invoked_path(matches: &clap::ArgMatches) -> String {
    let mut parts = Vec::new();
    let mut m = matches;
    while let Some((name, sub)) = m.subcommand() {
        parts.push(name.to_string());
        m = sub;
    }
    parts.join(" ")
}

fn argv_command_hint() -> Option<String> {
    const GLOBAL_VALUE_FLAGS: [&str; 4] = ["--account", "--tag", "--parallel", "--config"];
    const GLOBAL_BOOL_FLAGS: [&str; 7] = [
        "--json",
        "--jsonl",
        "--dry-run",
        "--quiet",
        "-q",
        "--verbose",
        "-v",
    ];
    let mut parts: Vec<String> = Vec::new();
    let mut skip_value = false;
    for arg in std::env::args_os().skip(1) {
        let s = arg.to_string_lossy().into_owned();
        if skip_value {
            skip_value = false;
            continue;
        }
        if GLOBAL_VALUE_FLAGS.contains(&s.as_str()) {
            skip_value = true;
            continue;
        }
        if GLOBAL_VALUE_FLAGS
            .iter()
            .any(|f| s.starts_with(&format!("{f}=")))
        {
            continue;
        }
        if GLOBAL_BOOL_FLAGS.contains(&s.as_str()) || s == "--" {
            continue;
        }
        if s.starts_with('-') {
            break;
        }
        parts.push(s);
        if parts.len() == 2 {
            break;
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

async fn run_command(command: Command, flags: &GlobalFlags) -> i32 {
    let result = match command {
        Command::Account(c) => account::run(c, flags).await,
        Command::Msg(c) => msg::run(c, flags).await,
        Command::Chat(c) => chat::run(c, flags).await,
        Command::Dialog(c) => dialog::run(c, flags).await,
        Command::Topic(c) => topic::run(c, flags).await,
        Command::Contact(c) => contact::run(c, flags).await,
        Command::Profile(c) => profile::run(c, flags).await,
        Command::Privacy(c) => privacy::run(c, flags).await,
        Command::Takeout(c) => takeout::run(c, flags).await,
        Command::Listen(c) => listen::run(&c, flags).await,
        Command::Raw(c) => raw::run(&c, flags).await,
        Command::Completions(s) => completions::run(s, flags).await,
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            if e.is_broken_pipe() {
                return error::EXIT_OK;
            }
            output::log_line("error", e.message());
            if output::machine_mode(flags.json, flags.jsonl) {
                let envelope = output::Envelope::failed(flags.dry_run, &flags.command, e.as_json());
                let value = serde_json::to_value(&envelope).expect("envelope serializes");
                let _ = output::print_json(&value);
            }
            e.exit_code()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::strip_ansi;

    #[test]
    fn strip_ansi_leaves_plain_text_unchanged() {
        assert_eq!(
            strip_ansi("error: unexpected argument"),
            "error: unexpected argument"
        );
    }

    #[test]
    fn strip_ansi_removes_color_code() {
        assert_eq!(strip_ansi("\x1b[31merror\x1b[0m"), "error");
    }

    #[test]
    fn strip_ansi_removes_multiple_escapes() {
        assert_eq!(strip_ansi("a\x1b[1;31mb\x1b[0mc"), "abc");
    }

    #[test]
    fn strip_ansi_removes_escape_at_start_and_end() {
        assert_eq!(strip_ansi("\x1b[32mstart"), "start");
        assert_eq!(strip_ansi("end\x1b[0m"), "end");
    }

    #[test]
    fn exit_code_clamping_preserves_valid_codes() {
        assert_eq!(0_i32.clamp(0, 255), 0);
        assert_eq!(1_i32.clamp(0, 255), 1);
        assert_eq!(255_i32.clamp(0, 255), 255);
    }

    #[test]
    fn exit_code_clamping_limits_out_of_range() {
        assert_eq!(256_i32.clamp(0, 255), 255);
        assert_eq!(1000_i32.clamp(0, 255), 255);
        assert_eq!((-1_i32).clamp(0, 255), 0);
    }
}
