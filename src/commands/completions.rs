use std::io::Write;

use clap::Subcommand;

use crate::error::{TeleError, TeleResult};
use crate::GlobalFlags;

#[derive(Subcommand)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Powershell,
}

fn completion_bin_name() -> String {
    std::env::args()
        .next()
        .and_then(|p| {
            std::path::PathBuf::from(p)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_owned())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| env!("CARGO_BIN_NAME").to_string())
}

#[allow(dead_code)]
fn bin_name_from_arg(arg: Option<&str>) -> String {
    arg.and_then(|p| {
        std::path::Path::new(p)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_owned())
    })
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| env!("CARGO_BIN_NAME").to_string())
}

pub async fn run(shell: Shell, _flags: &GlobalFlags) -> TeleResult<i32> {
    let bin = completion_bin_name();
    let mut cmd = crate::command_for_completions();
    let mut buf = Vec::new();
    match shell {
        Shell::Bash => {
            clap_complete::generate(clap_complete::Shell::Bash, &mut cmd, bin.clone(), &mut buf);
        }
        Shell::Zsh => {
            clap_complete::generate(clap_complete::Shell::Zsh, &mut cmd, bin.clone(), &mut buf);
        }
        Shell::Fish => {
            clap_complete::generate(clap_complete::Shell::Fish, &mut cmd, bin.clone(), &mut buf);
        }
        Shell::Powershell => {
            clap_complete::generate(
                clap_complete::Shell::PowerShell,
                &mut cmd,
                bin.clone(),
                &mut buf,
            );
        }
    }
    let mut out = std::io::stdout();
    match out.write_all(&buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Err(TeleError::BrokenPipe),
        Err(e) => return Err(e.into()),
    }
    match out.flush() {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Err(TeleError::BrokenPipe),
        Err(e) => return Err(e.into()),
    }
    Ok(crate::error::EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::{bin_name_from_arg, completion_bin_name};

    fn gen(shell: clap_complete::Shell, bin: &str) -> String {
        let mut cmd = crate::command_for_completions();
        let mut buf = Vec::new();
        clap_complete::generate(shell, &mut cmd, bin, &mut buf);
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn bin_name_from_arg_prefers_file_stem() {
        assert_eq!(bin_name_from_arg(Some("telecli")), "telecli");
        assert_eq!(bin_name_from_arg(Some("/usr/local/bin/telecli")), "telecli");
        assert_eq!(
            bin_name_from_arg(Some("/usr/local/bin/telecli.exe")),
            "telecli"
        );
        #[cfg(windows)]
        assert_eq!(bin_name_from_arg(Some("C:\\tools\\telecli.exe")), "telecli");
    }

    #[test]
    fn bin_name_from_arg_falls_back_to_cargo_bin_name() {
        assert_eq!(bin_name_from_arg(None), env!("CARGO_BIN_NAME"));
        assert_eq!(bin_name_from_arg(Some("")), env!("CARGO_BIN_NAME"));
    }

    #[test]
    fn completion_bin_name_defaults_to_telecli() {
        assert_eq!(env!("CARGO_BIN_NAME"), "telecli");
        let name = completion_bin_name();
        assert!(!name.is_empty());
    }

    #[test]
    fn bash_completions_reference_real_bin() {
        let bin = "telecli";
        let out = gen(clap_complete::Shell::Bash, bin);
        assert!(out.contains(&format!("complete -F _{bin}")));
        assert!(out.contains(bin));
    }

    #[test]
    fn zsh_completions_have_compdef_for_real_bin() {
        let bin = "telecli";
        let out = gen(clap_complete::Shell::Zsh, bin);
        assert!(out.contains(&format!("#compdef {bin}")));
    }

    #[test]
    fn fish_completions_have_complete_for_real_bin() {
        let bin = "telecli";
        let out = gen(clap_complete::Shell::Fish, bin);
        assert!(out.contains(&format!("complete -c {bin}")));
    }

    #[test]
    fn powershell_completions_register_for_real_bin() {
        let bin = "telecli";
        let out = gen(clap_complete::Shell::PowerShell, bin);
        assert!(out.contains("Register-ArgumentCompleter"));
        assert!(out.contains(bin));
    }

    #[test]
    fn broken_pipe_maps_to_tele_error() {
        let io_err = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
        let err: crate::error::TeleError = io_err.into();
        assert!(err.is_broken_pipe());
        assert_eq!(err.exit_code(), crate::error::EXIT_OK);
    }
}
