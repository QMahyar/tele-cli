mod cache_db;
mod capped_map;
mod chat_target;
mod client;
mod commands;
mod config;
mod doctor;
mod entities;
mod error;
mod executor;
mod fs_util;
mod logging;
mod output;
mod pagination;
mod rate_limiter;
mod serialize;
mod session;
mod wizard;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use std::io::Write as _;

use commands::*;
use executor::GlobalFlags;

const MAIN_RUNTIME_STACK_SIZE: usize = 64 * 1024 * 1024;

#[derive(Parser)]
#[command(
    name = "tele",
    version,
    about = "Telegram user-account CLI",
    long_about = "Telegram user-account CLI\n\nReport bugs at: https://github.com/QMahyar/tele-cli/issues"
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
    #[arg(
        long,
        global = true,
        value_name = "FIELDS",
        help = "project machine-output rows to comma-separated dotted fields (requires --json/--jsonl; unknown fields are usage errors)"
    )]
    fields: Option<String>,
    #[arg(
        long,
        global = true,
        help = "fail closed instead of prompting for interactive input"
    )]
    no_input: bool,
    #[arg(
        long,
        global = true,
        help = "acknowledge placing app state outside a per-user directory (required when TELE_APP_DIR leaves per-user paths)"
    )]
    allow_insecure_app_dir: bool,
    #[arg(
        long,
        global = true,
        help = "acknowledge that TELE_LOG=trace bypasses secret scrubbing for grammers internals"
    )]
    allow_trace: bool,
    #[arg(long, global = true, help = "validate without touching Telegram")]
    dry_run: bool,
    #[arg(
        long,
        short = 'q',
        global = true,
        conflicts_with = "verbose",
        help = "quiet: only [error] lines on stderr (overrides -v/TELE_LOG)"
    )]
    quiet: bool,
    #[arg(
        long,
        short = 'v',
        global = true,
        conflicts_with = "quiet",
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
    /// Sticker packs: list, search, show, install, remove
    #[command(subcommand)]
    Sticker(stickers::StickerCmd),
    /// Stories: send, list, read, delete, pin, unpin
    #[command(subcommand)]
    Story(stories::StoryCmd),
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
    /// Local message cache: sync, search offline, stats, clear
    #[command(subcommand)]
    Cache(cache::CacheCmd),
    /// Stream updates as JSONL
    Listen(listen::ListenArgs),
    /// Duplex JSONL runtime: events out, actions in (1–32 accounts)
    Serve(serve::ServeArgs),
    /// MCP stdio server: tele ops as tools (owns one account)
    Mcp(mcp::McpArgs),
    /// Raw TL invocation (typed registry)
    Raw(raw::RawArgs),
    /// Check local setup health (config, credentials, sessions; no network)
    Doctor,
    /// Guided credential setup (api_id/api_hash prompts, .env + config writes; no network)
    Wizard(wizard::WizardArgs),
    /// Print (or install) the agent skill for driving tele
    Skill(skill::SkillCmd),
    /// Generate shell completions
    #[command(subcommand)]
    Completions(completions::Shell),
}

fn main() -> std::process::ExitCode {
    logging::init();
    let matches = match Cli::command().try_get_matches() {
        Ok(matches) => matches,
        Err(e) => return clap_error_exit(e),
    };
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(e) => return clap_error_exit(e),
    };
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
            let message = format!("--parallel {p} must be between 1 and 32");
            return std::process::ExitCode::from(emit_usage_error(
                UsageCtx {
                    machine: output::machine_mode(flags.json, flags.jsonl),
                    dry_run: flags.dry_run,
                },
                &flags.command,
                &message,
            ) as u8);
        }
    }
    if let Some(ref cfg_path) = flags.config_path {
        if !cfg_path.exists() {
            let message = format!(
                "config file not found: {}",
                config::config_display_name(cfg_path)
            );
            return std::process::ExitCode::from(emit_usage_error(
                UsageCtx {
                    machine: output::machine_mode(flags.json, flags.jsonl),
                    dry_run: flags.dry_run,
                },
                &flags.command,
                &message,
            ) as u8);
        }
    }
    if flags.json && flags.jsonl {
        let message = "--json and --jsonl are mutually exclusive; pick one";
        return std::process::ExitCode::from(emit_usage_error(
            UsageCtx {
                machine: true,
                dry_run: false,
            },
            &flags.command,
            message,
        ) as u8);
    }
    if let Some(spec) = cli.fields.as_deref() {
        if !cli.json && !cli.jsonl {
            return std::process::ExitCode::from(emit_usage_error(
                UsageCtx {
                    machine: false,
                    dry_run: flags.dry_run,
                },
                &flags.command,
                "--fields requires --json or --jsonl",
            ) as u8);
        }
        if let Err(e) = output::set_output_fields(spec) {
            let message = e.message();
            return std::process::ExitCode::from(emit_usage_error(
                UsageCtx {
                    machine: true,
                    dry_run: flags.dry_run,
                },
                &flags.command,
                &message,
            ) as u8);
        }
    }
    if cli.no_input && !cli.dry_run {
        if let Some(detail) = no_input_violation(&cli.command, &matches) {
            let message = format!(
                "{detail}; --no-input fails closed (re-run without --no-input or add --dry-run to preview)"
            );
            return std::process::ExitCode::from(emit_usage_error(
                UsageCtx {
                    machine: output::machine_mode(cli.json, cli.jsonl),
                    dry_run: false,
                },
                &flags.command,
                &message,
            ) as u8);
        }
    }
    if let Err(e) = logging::trace_ack_error(cli.allow_trace) {
        return std::process::ExitCode::from(emit_usage_error(
            UsageCtx {
                machine: output::machine_mode(cli.json, cli.jsonl),
                dry_run: cli.dry_run,
            },
            &flags.command,
            &e.message(),
        ) as u8);
    }
    if !cli.allow_insecure_app_dir {
        if let Some(override_dir) = config::app_dir_override() {
            if !config::app_dir_is_per_user(&override_dir) {
                let message = format!(
                    "TELE_APP_DIR={} is outside per-user paths; re-run with --allow-insecure-app-dir to acknowledge the weaker file-protection model",
                    override_dir.display()
                );
                return std::process::ExitCode::from(emit_usage_error(
                    UsageCtx {
                        machine: output::machine_mode(cli.json, cli.jsonl),
                        dry_run: cli.dry_run,
                    },
                    &flags.command,
                    &message,
                ) as u8);
            }
        }
    }
    if config::app_data_dir_checked().is_err() {
        let message = "cannot determine app data directory; set TELE_APP_DIR to choose a location";
        output::log_line("error", message);
        bug_report_footer();
        if output::machine_mode(flags.json, flags.jsonl) {
            let error_json = crate::error::TeleError::Config(message.to_string()).as_json();
            let envelope = output::Envelope::failed(flags.dry_run, &flags.command, error_json);
            if let Ok(v) = serde_json::to_value(&envelope) {
                let _ = output::print_json(&v);
            }
        }
        return std::process::ExitCode::from(error::EXIT_USAGE as u8);
    }
    config::migrate_app_data_dir();
    crate::session::sweep_tighten_session_files();
    let machine_mode = output::machine_mode(flags.json, flags.jsonl);
    let dry_run = flags.dry_run;
    let command_name = flags.command.clone();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = match std::thread::Builder::new()
        .stack_size(MAIN_RUNTIME_STACK_SIZE)
        .spawn(move || {
            runtime.block_on(async {
                tokio::select! {
                    code = run_command(cli.command, &flags) => code,
                    _ = tokio::signal::ctrl_c() => {
                        if output::machine_mode(flags.json, flags.jsonl) {
                            let error_json = serde_json::json!({"type": "Interrupted", "message": "interrupted by SIGINT"});
                            let envelope = output::Envelope::failed(flags.dry_run, &flags.command, error_json);
                            if let Ok(v) = serde_json::to_value(&envelope) {
                                let _ = output::print_json(&v);
                            }
                        }
                        error::EXIT_INTERRUPTED
                    }
                }
            })
        })
        .expect("spawn main runtime thread")
        .join()
    {
        Ok(code) => code,
        Err(payload) => {
            let mut msg = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "runtime thread panicked (non-string payload)".to_string()
            };
            let text = format!(
                "runtime thread panicked: {}",
                error::scrub(std::mem::take(&mut msg))
            );
            output::log_line("error", &text);
            if machine_mode {
                let error_json = serde_json::json!({
                    "type": "TaskPanicError",
                    "message": error::scrub(text),
                });
                let envelope = output::Envelope::failed(dry_run, &command_name, error_json);
                if let Ok(v) = serde_json::to_value(&envelope) {
                    let _ = output::print_json(&v);
                }
            }
            error::EXIT_ALL_FAILED
        }
    };
    std::process::ExitCode::from(clamp_exit_code(code))
}

struct UsageCtx {
    machine: bool,
    dry_run: bool,
}

pub(crate) const ISSUE_URL: &str = "https://github.com/QMahyar/tele-cli/issues";

fn bug_report_footer() {
    let _ = writeln!(std::io::stderr(), "report bugs at {ISSUE_URL}",);
}

fn clap_error_exit(e: clap::Error) -> std::process::ExitCode {
    let code = if e.use_stderr() {
        error::EXIT_USAGE
    } else {
        error::EXIT_OK
    };
    let _ = e.print();
    if e.use_stderr() {
        bug_report_footer();
    }
    if e.use_stderr() && std::env::args_os().any(|a| a == "--json" || a == "--jsonl") {
        let hint = argv_command_hint().unwrap_or_default();
        emit_usage_error(
            UsageCtx {
                machine: true,
                dry_run: false,
            },
            &hint,
            &output::strip_ansi(&e.to_string()),
        );
    }
    std::process::ExitCode::from(code as u8)
}

const EXIT_CODE_MIN: i32 = 0;
const EXIT_CODE_MAX: i32 = 255;

fn clamp_exit_code(code: i32) -> u8 {
    code.clamp(EXIT_CODE_MIN, EXIT_CODE_MAX) as u8
}

fn emit_usage_error(ctx: UsageCtx, command: &str, message: &str) -> i32 {
    output::log_line("error", message);
    bug_report_footer();
    if ctx.machine {
        let error_json = serde_json::json!({"type": "UsageError", "message": message});
        let envelope = output::Envelope::failed(ctx.dry_run, command, error_json);
        if let Ok(v) = serde_json::to_value(&envelope) {
            let _ = output::print_json(&v);
        }
    }
    error::EXIT_USAGE
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

fn flag_is_set(matches: &clap::ArgMatches, id: &str) -> bool {
    matches
        .try_get_one::<bool>(id)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}

fn password_requests_prompt(matches: &clap::ArgMatches) -> bool {
    let Some(("account", selected)) = matches.subcommand() else {
        return false;
    };
    let Some(("password", password)) = selected.subcommand() else {
        return false;
    };
    flag_is_set(password, "set")
        || flag_is_set(password, "change")
        || flag_is_set(password, "remove")
        || flag_is_set(password, "decline_reset")
}

fn login_may_prompt(args: &account::LoginArgs) -> bool {
    if let Some(stage) = args.stage.as_deref() {
        return stage == "code";
    }
    args.method != "qr"
}

fn no_input_violation(command: &Command, matches: &clap::ArgMatches) -> Option<String> {
    match command {
        Command::Account(account::AccountCmd::Login(args)) if login_may_prompt(args) => {
            if args.phone.as_deref().is_some_and(|p| !p.trim().is_empty()) {
                Some("--phone on argv under --no-input is rejected (visible in process listings and shell history); use TELE_PHONE instead".to_string())
            } else {
                Some("account login would prompt for the login code or 2FA password".to_string())
            }
        }
        Command::Account(account::AccountCmd::Phone(args))
            if args
                .change_phone
                .as_deref()
                .is_some_and(|p| !p.trim().is_empty()) =>
        {
            Some(
                "--change-phone on argv under --no-input is rejected; re-run without --no-input"
                    .to_string(),
            )
        }
        Command::Account(account::AccountCmd::Password(_)) if password_requests_prompt(matches) => {
            Some("account password would prompt for the cloud password".to_string())
        }
        Command::Account(account::AccountCmd::Delete(_)) => {
            Some("account delete may prompt for the cloud password".to_string())
        }
        Command::Wizard(_) => {
            Some("credential wizard would prompt for api_id/api_hash".to_string())
        }
        _ => None,
    }
}

fn argv_command_hint() -> Option<String> {
    argv_command_hint_from(std::env::args_os().skip(1))
}

fn argv_command_hint_from(args: impl IntoIterator<Item = std::ffi::OsString>) -> Option<String> {
    const GLOBAL_VALUE_FLAGS: [&str; 5] =
        ["--account", "--tag", "--parallel", "--config", "--fields"];
    const GLOBAL_BOOL_FLAGS: [&str; 10] = [
        "--json",
        "--jsonl",
        "--dry-run",
        "--quiet",
        "-q",
        "--verbose",
        "-v",
        "--no-input",
        "--allow-insecure-app-dir",
        "--allow-trace",
    ];
    let mut parts: Vec<String> = Vec::new();
    let mut skip_value = false;
    for arg in args {
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
        Command::Sticker(c) => stickers::run(c, flags).await,
        Command::Story(c) => stories::run(c, flags).await,
        Command::Contact(c) => contact::run(c, flags).await,
        Command::Profile(c) => profile::run(c, flags).await,
        Command::Privacy(c) => privacy::run(c, flags).await,
        Command::Takeout(c) => takeout::run(c, flags).await,
        Command::Cache(c) => cache::run(c, flags).await,
        Command::Listen(c) => listen::run(&c, flags).await,
        Command::Serve(c) => serve::run(&c, flags).await,
        Command::Mcp(c) => mcp::run(&c, flags).await,
        Command::Raw(c) => raw::run(&c, flags).await,
        Command::Doctor => doctor::run(flags).await,
        Command::Wizard(c) => wizard::run(&c, flags).await,
        Command::Skill(c) => skill::run(c, flags).await,
        Command::Completions(s) => completions::run(s, flags).await,
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            if e.is_broken_pipe() {
                return error::EXIT_OK;
            }
            output::log_line("error", &e.message());
            bug_report_footer();
            if output::machine_mode(flags.json, flags.jsonl) {
                let envelope = output::Envelope::failed(flags.dry_run, &flags.command, e.as_json());
                if let Ok(value) = serde_json::to_value(&envelope) {
                    let _ = output::print_json(&value);
                }
            }
            e.exit_code()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clamp_exit_code;
    use crate::output::strip_ansi;
    use clap::CommandFactory;

    fn login_args(method: &str, stage: Option<&str>) -> crate::commands::account::LoginArgs {
        crate::commands::account::LoginArgs {
            name: "work".to_string(),
            method: method.to_string(),
            phone: None,
            show_token: false,
            qr_timeout_secs: 300,
            stage: stage.map(str::to_string),
        }
    }

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
    fn clamp_exit_code_maps_bounds() {
        assert_eq!(clamp_exit_code(0), 0);
        assert_eq!(clamp_exit_code(1), 1);
        assert_eq!(clamp_exit_code(130), 130);
        assert_eq!(clamp_exit_code(255), 255);
        assert_eq!(clamp_exit_code(256), 255);
        assert_eq!(clamp_exit_code(1000), 255);
        assert_eq!(clamp_exit_code(-1), 0);
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

    #[test]
    fn login_may_prompt_covers_code_flows_only() {
        assert!(super::login_may_prompt(&login_args("code", None)));
        assert!(!super::login_may_prompt(&login_args("qr", None)));
        assert!(super::login_may_prompt(&login_args("code", Some("code"))));
        for stage in ["begin", "status", "cancel", "resend", "cancel-code"] {
            assert!(
                !super::login_may_prompt(&login_args("code", Some(stage))),
                "stage {stage} reads no stdin"
            );
        }
    }

    #[test]
    fn password_requests_prompt_covers_password_modes_only() {
        for mode in ["--set", "--change", "--remove", "--decline-reset"] {
            let matches = super::Cli::command()
                .try_get_matches_from(["tele", "account", "password", mode])
                .unwrap();
            assert!(
                super::password_requests_prompt(&matches),
                "mode {mode} must count as prompting"
            );
        }
        for mode in [
            "--status",
            "--resend-email",
            "--cancel-email",
            "--reset-start",
        ] {
            let matches = super::Cli::command()
                .try_get_matches_from(["tele", "account", "password", mode])
                .unwrap();
            assert!(
                !super::password_requests_prompt(&matches),
                "mode {mode} reads no stdin"
            );
        }
        let matches = super::Cli::command()
            .try_get_matches_from(["tele", "msg", "send", "--chat", "me", "--text", "hi"])
            .unwrap();
        assert!(!super::password_requests_prompt(&matches));
    }

    #[test]
    fn issue_url_points_at_tracker() {
        assert!(super::ISSUE_URL.starts_with("https://"));
        assert!(super::ISSUE_URL.contains("/issues"));
        assert!(!super::ISSUE_URL.contains(' '));
    }

    #[test]
    fn no_input_violation_names_prompting_commands() {
        use clap::FromArgMatches;
        let parsed = |argv: &[&str]| {
            let m = super::Cli::command().try_get_matches_from(argv).unwrap();
            let cli = super::Cli::from_arg_matches(&m).unwrap();
            (cli.command, m)
        };
        let (command, m) = parsed(&["tele", "account", "delete", "--reason", "x", "--yes"]);
        assert!(super::no_input_violation(&command, &m).is_some());
        let (command, m) = parsed(&["tele", "msg", "send", "--chat", "me", "--text", "hi"]);
        assert!(super::no_input_violation(&command, &m).is_none());
    }

    #[test]
    fn argv_hint_skips_all_global_flags() {
        let argv = [
            "--allow-insecure-app-dir",
            "--allow-trace",
            "--json",
            "msg",
            "send",
        ];
        assert_eq!(
            super::argv_command_hint_from(argv.map(std::ffi::OsString::from)),
            Some("msg send".to_string())
        );
        let argv = ["--allow-insecure-app-dir", "--json", "foobar"];
        assert_eq!(
            super::argv_command_hint_from(argv.map(std::ffi::OsString::from)),
            Some("foobar".to_string())
        );
    }

    #[test]
    fn no_input_violation_rejects_phone_on_argv() {
        use clap::FromArgMatches;
        let parsed = |argv: &[&str]| {
            let m = super::Cli::command().try_get_matches_from(argv).unwrap();
            let cli = super::Cli::from_arg_matches(&m).unwrap();
            (cli.command, m)
        };
        let (command, m) = parsed(&[
            "tele",
            "account",
            "login",
            "--name",
            "w",
            "--phone",
            "+10000000000",
        ]);
        let detail =
            super::no_input_violation(&command, &m).expect("phone on argv must fail closed");
        assert!(detail.contains("--phone"), "detail: {detail}");
        let (command, m) = parsed(&["tele", "account", "phone", "--change-phone", "+10000000001"]);
        let detail =
            super::no_input_violation(&command, &m).expect("change-phone on argv must fail closed");
        assert!(detail.contains("--change-phone"), "detail: {detail}");
        let (command, m) = parsed(&["tele", "account", "login", "--name", "w", "--method", "qr"]);
        assert!(super::no_input_violation(&command, &m).is_none());
    }
}
