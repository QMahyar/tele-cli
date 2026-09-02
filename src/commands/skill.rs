use clap::Subcommand;

use crate::error::TeleResult;

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

pub async fn run(cmd: SkillCmd, _flags: &crate::executor::GlobalFlags) -> TeleResult<i32> {
    match cmd.sub {
        None | Some(SkillSub::Print) => {
            print!("{SKILL_MD}");
            Ok(crate::error::EXIT_OK)
        }
        Some(SkillSub::Install { dir, force }) => install(dir, force),
    }
}

fn install(dir: Option<std::path::PathBuf>, force: bool) -> TeleResult<i32> {
    if let Some(dir) = dir {
        let target = dir.join("tele").join("SKILL.md");
        write_skill(&target, force)?;
        crate::output::log_line("info", &format!("installed skill to {}", target.display()));
        return Ok(crate::error::EXIT_OK);
    }
    let mut written = Vec::new();
    for dir in detected_agent_dirs() {
        let target = dir.join("tele").join("SKILL.md");
        if target.exists() && !force {
            crate::output::log_line(
                "warn",
                &format!("exists (use --force to overwrite): {}", target.display()),
            );
            continue;
        }
        match write_skill(&target, force) {
            Ok(()) => written.push(target.display().to_string()),
            Err(e) => crate::output::log_line("warn", &format!("skipped {}: {e}", dir.display())),
        }
    }
    if written.is_empty() {
        return Err(crate::error::TeleError::Other(
            "no agent skill directory detected; pass --dir PATH".to_string(),
        ));
    }
    for path in &written {
        crate::output::log_line("info", &format!("installed skill to {path}"));
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
