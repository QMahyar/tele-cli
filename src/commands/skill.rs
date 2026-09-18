use clap::Subcommand;

use crate::error::TeleResult;
use crate::output::LogLevel;

pub(crate) const SKILL_MD: &str = include_str!("skill.md");

#[derive(clap::Args)]
pub struct SkillCmd {
    #[command(subcommand)]
    pub sub: Option<SkillSub>,
}

#[derive(Subcommand)]
pub enum SkillSub {
    /// Print the agent skill (SKILL.md) to stdout
    Print,
    /// Install the skill into detected agent skill directories
    Install {
        #[arg(
            long,
            help = "install into this directory instead of the detected agent dirs"
        )]
        dir: Option<std::path::PathBuf>,
        #[arg(long, help = "overwrite an existing skill without asking")]
        force: bool,
    },
}

pub async fn run(cmd: SkillCmd, flags: &crate::executor::GlobalFlags) -> TeleResult<i32> {
    match cmd.sub {
        None | Some(SkillSub::Print) => print_skill(flags),
        Some(SkillSub::Install { dir, force }) => install(dir, force, flags),
    }
}

fn local_data(
    data: serde_json::Value,
    flags: &crate::executor::GlobalFlags,
) -> crate::output::Envelope {
    crate::output::Envelope::new(
        vec![crate::output::AccountOutcome {
            account: "local".to_string(),
            ok: true,
            error: None,
            data: Some(data),
            exit_code: None,
        }],
        flags.dry_run,
        &flags.command,
    )
}

fn print_data(dry_run: bool) -> serde_json::Value {
    if dry_run {
        serde_json::json!({
            "dry_run": true,
            "skill": SKILL_MD,
            "would": "print agent skill to stdout",
        })
    } else {
        serde_json::json!({"skill": SKILL_MD})
    }
}

fn print_skill(flags: &crate::executor::GlobalFlags) -> TeleResult<i32> {
    if crate::output::machine_mode(flags.json, flags.jsonl) {
        let envelope = local_data(print_data(flags.dry_run), flags);
        return crate::executor::finish(flags, &envelope);
    }
    crate::output::print_raw(SKILL_MD)?;
    Ok(crate::error::EXIT_OK)
}

fn installed_freshness_note(target: &std::path::Path) {
    if let Ok(existing) = std::fs::read_to_string(target) {
        let stamp = format!("compatibility: tele {}+", env!("CARGO_PKG_VERSION"));
        if !existing.contains(&stamp) {
            crate::output::log_level(
                LogLevel::Warn,
                &format!(
                    "existing skill at {} predates tele {}; reinstalling updates it (recipes may have drifted)",
                    target.display(),
                    env!("CARGO_PKG_VERSION")
                ),
            );
        }
    }
}

fn install(
    dir: Option<std::path::PathBuf>,
    force: bool,
    flags: &crate::executor::GlobalFlags,
) -> TeleResult<i32> {
    if let Some(dir) = dir {
        let target = dir.join("tele").join("SKILL.md");
        if flags.dry_run {
            return install_dry_run_output(std::slice::from_ref(&target), force, flags);
        }
        installed_freshness_note(&target);
        write_skill(&target, force)?;
        let written = vec![target.display().to_string()];
        return finish_install(&written, force, flags);
    }
    let dirs = detected_agent_dirs();
    if flags.dry_run {
        if dirs.is_empty() {
            return Err(crate::error::TeleError::Other(
                "no agent skill directory detected; pass --dir PATH".to_string(),
            ));
        }
        let targets: Vec<std::path::PathBuf> = dirs
            .iter()
            .map(|dir| dir.join("tele").join("SKILL.md"))
            .collect();
        return install_dry_run_output(&targets, force, flags);
    }
    let mut written = Vec::new();
    for dir in &dirs {
        let target = dir.join("tele").join("SKILL.md");
        installed_freshness_note(&target);
        if target.exists() && !force {
            crate::output::log_level(
                LogLevel::Warn,
                &format!("exists (use --force to overwrite): {}", target.display()),
            );
            continue;
        }
        match write_skill(&target, force) {
            Ok(()) => written.push(target.display().to_string()),
            Err(e) => {
                crate::output::log_level(LogLevel::Warn, &format!("skipped {}: {e}", dir.display()))
            }
        }
    }
    if written.is_empty() {
        return Err(crate::error::TeleError::Other(
            "no agent skill directory detected; pass --dir PATH".to_string(),
        ));
    }
    finish_install(&written, force, flags)
}

fn install_targets_display(targets: &[std::path::PathBuf]) -> Vec<String> {
    targets.iter().map(|t| t.display().to_string()).collect()
}

fn install_dry_run_data(targets: &[String], force: bool) -> serde_json::Value {
    serde_json::json!({
        "dry_run": true,
        "force": force,
        "targets": targets,
        "would": format!("install skill to {}", targets.join(", ")),
    })
}

fn install_dry_run_output(
    targets: &[std::path::PathBuf],
    force: bool,
    flags: &crate::executor::GlobalFlags,
) -> TeleResult<i32> {
    let display = install_targets_display(targets);
    if crate::output::machine_mode(flags.json, flags.jsonl) {
        let envelope = local_data(install_dry_run_data(&display, force), flags);
        return crate::executor::finish(flags, &envelope);
    }
    for path in &display {
        crate::output::log_level(LogLevel::Info, &format!("would install skill to {path}"));
    }
    Ok(crate::error::EXIT_OK)
}

fn finish_install(
    written: &[String],
    force: bool,
    flags: &crate::executor::GlobalFlags,
) -> TeleResult<i32> {
    if crate::output::machine_mode(flags.json, flags.jsonl) {
        let envelope = local_data(
            serde_json::json!({"installed": written, "force": force}),
            flags,
        );
        return crate::executor::finish(flags, &envelope);
    }
    for path in written {
        crate::output::log_level(LogLevel::Info, &format!("installed skill to {path}"));
    }
    Ok(crate::error::EXIT_OK)
}

fn write_skill(target: &std::path::Path, force: bool) -> TeleResult<()> {
    if target.exists() && !force {
        return Err(crate::error::TeleError::Other(format!(
            "refusing to overwrite {} without --force",
            target.display()
        )));
    }
    let parent = target
        .parent()
        .ok_or_else(|| crate::error::TeleError::Other("skill path has no parent".to_string()))?;
    std::fs::create_dir_all(parent).map_err(|e| {
        crate::error::TeleError::Other(format!("cannot create {}: {e}", parent.display()))
    })?;
    std::fs::write(target, SKILL_MD).map_err(|e| {
        crate::error::TeleError::Other(format!("cannot write {}: {e}", target.display()))
    })?;
    Ok(())
}

fn detected_agent_dirs() -> Vec<std::path::PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from);
    let Some(home) = home else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    if home.join(".claude").is_dir() {
        dirs.push(home.join(".claude").join("skills"));
    }
    if home.join(".config").join("opencode").is_dir() {
        dirs.push(home.join(".config").join("opencode").join("skills"));
    }
    if home.join(".cursor").is_dir() {
        dirs.push(home.join(".cursor").join("skills"));
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dry_flags(command: &str) -> crate::executor::GlobalFlags {
        crate::executor::GlobalFlags {
            account: Vec::new(),
            tag: Vec::new(),
            parallel: None,
            json: false,
            jsonl: false,
            dry_run: true,
            quiet: false,
            config_path: None,
            command: command.to_string(),
        }
    }

    #[test]
    fn print_data_human_carries_skill_bytes() {
        let value = print_data(false);
        assert_eq!(value["skill"], serde_json::json!(SKILL_MD));
        assert!(value.get("dry_run").is_none());
        assert!(value.get("would").is_none());
    }

    #[test]
    fn print_data_dry_run_carries_would() {
        let value = print_data(true);
        assert_eq!(value["dry_run"], serde_json::json!(true));
        assert_eq!(value["skill"], serde_json::json!(SKILL_MD));
        assert!(
            value["would"]
                .as_str()
                .unwrap_or_default()
                .contains("print agent skill"),
            "would: {value}"
        );
    }

    #[test]
    fn install_dry_run_data_lists_targets_and_would() {
        let value = install_dry_run_data(&["/tmp/a/tele/SKILL.md".to_string()], false);
        assert_eq!(value["dry_run"], serde_json::json!(true));
        assert_eq!(value["force"], serde_json::json!(false));
        assert_eq!(
            value["targets"],
            serde_json::json!(["/tmp/a/tele/SKILL.md"])
        );
        assert!(
            value["would"]
                .as_str()
                .unwrap_or_default()
                .contains("/tmp/a/tele/SKILL.md"),
            "would: {value}"
        );
    }

    #[test]
    fn install_dir_dry_run_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("telecli-skill-dry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let code = install(Some(dir.clone()), false, &dry_flags("skill install")).unwrap();
        assert_eq!(code, crate::error::EXIT_OK);
        assert!(
            !dir.join("tele").join("SKILL.md").exists(),
            "dry-run must not write"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
