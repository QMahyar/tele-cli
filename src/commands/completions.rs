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

#[cfg(test)]
mod tests {
    fn gen(shell: clap_complete::Shell) -> String {
        let mut cmd = crate::command_for_completions();
        let mut buf = Vec::new();
        clap_complete::generate(shell, &mut cmd, "tele", &mut buf);
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn bash_completions_reference_tele() {
        assert!(gen(clap_complete::Shell::Bash).contains("complete -F _tele"));
    }

    #[test]
    fn zsh_completions_have_compdef() {
        assert!(gen(clap_complete::Shell::Zsh).contains("#compdef tele"));
    }

    #[test]
    fn fish_completions_have_complete() {
        assert!(gen(clap_complete::Shell::Fish).contains("complete -c tele"));
    }

    #[test]
    fn powershell_completions_register() {
        assert!(gen(clap_complete::Shell::PowerShell).contains("Register-ArgumentCompleter"));
    }
}
