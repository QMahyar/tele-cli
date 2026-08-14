use clap::Subcommand;

use crate::error::TeleResult;
use crate::GlobalFlags;

#[derive(Subcommand)]
pub enum Shell {
    /// Bash completions
    Bash,
    /// Zsh completions
    Zsh,
    /// Fish completions
    Fish,
    /// PowerShell completions
    Powershell,
}

pub async fn run(shell: Shell, _flags: &GlobalFlags) -> TeleResult<i32> {
    let mut cmd = crate::command_for_completions();
    match shell {
        Shell::Bash => {
            clap_complete::generate(
                clap_complete::Shell::Bash,
                &mut cmd,
                "tele",
                &mut std::io::stdout(),
            );
        }
        Shell::Zsh => {
            clap_complete::generate(
                clap_complete::Shell::Zsh,
                &mut cmd,
                "tele",
                &mut std::io::stdout(),
            );
        }
        Shell::Fish => {
            clap_complete::generate(
                clap_complete::Shell::Fish,
                &mut cmd,
                "tele",
                &mut std::io::stdout(),
            );
        }
        Shell::Powershell => {
            clap_complete::generate(
                clap_complete::Shell::PowerShell,
                &mut cmd,
                "tele",
                &mut std::io::stdout(),
            );
        }
    }
    Ok(crate::error::EXIT_OK)
}
