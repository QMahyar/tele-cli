mod client;
mod commands;
mod config;
mod entities;
mod error;
mod executor;
mod fs_util;
mod logging;
mod output;
mod serialize;
mod session;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use commands::*;
use error::TeleResult;
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
        help = "parallel accounts (1-3; default from config parallel_max)"
    )]
    parallel: Option<u32>,
    #[arg(long, global = true, help = "machine output: single JSON envelope")]
    json: bool,
    #[arg(long, global = true, help = "machine output: JSON lines")]
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
    /// Chats: join, leave, invite, participants, kick, admin, adminlog, stats, create
    #[command(subcommand)]
    Chat(chat::ChatCmd),
    /// Dialogs: list, drafts, archive, delete
    #[command(subcommand)]
    Dialog(dialog::DialogCmd),
    /// Forum topics
    #[command(subcommand)]
    Topic(topic::TopicCmd),
    /// Contacts: list, add, block, unblock
    #[command(subcommand)]
    Contact(contact::ContactCmd),
    /// Profile: get, set
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

fn main() {
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
    if flags.json && flags.jsonl {
        output::log_line(
            "error",
            "--json and --jsonl are mutually exclusive; pick one",
        );
        std::process::exit(error::EXIT_USAGE);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = runtime.block_on(async { run_command(cli.command, &flags).await });
    std::process::exit(code);
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

async fn run_command(command: Command, flags: &GlobalFlags) -> i32 {
    let result: TeleResult<i32> = match command {
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
            output::log_line("error", e.message());
            e.exit_code()
        }
    }
}
