use std::io::Write;

use clap::Subcommand;

use crate::error::{TeleError, TeleResult};
use crate::GlobalFlags;

#[derive(Subcommand)]
pub enum Shell {
    /// Generate completions for bash
    Bash,
    /// Generate completions for zsh
    Zsh,
    /// Generate completions for fish
    Fish,
    /// Generate completions for PowerShell
    Powershell,
}

fn completion_bin_name() -> String {
    bin_name_from_arg(std::env::args().next().as_deref())
}

fn bin_name_from_arg(arg: Option<&str>) -> String {
    arg.and_then(|p| {
        std::path::Path::new(p)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(normalize_npm_stem)
    })
    .filter(|s| !s.is_empty())
    .unwrap_or_else(|| env!("CARGO_BIN_NAME").to_string())
}

/// The npm launcher spawns `tele-<target-triple>[.exe]` (legacy alias
/// `telecli-`), so completions must collapse platform-suffixed stems back to
/// the command name the user actually types.
fn normalize_npm_stem(stem: &str) -> String {
    for prefix in ["tele-", "telecli-"] {
        if let Some(rest) = stem.strip_prefix(prefix) {
            if looks_like_target_triple(rest) {
                return prefix.trim_end_matches('-').to_string();
            }
        }
    }
    stem.to_string()
}

fn looks_like_target_triple(rest: &str) -> bool {
    const ARCHES: &[&str] = &[
        "x86_64",
        "i686",
        "i586",
        "i386",
        "aarch64",
        "arm",
        "thumb",
        "riscv",
        "wasm",
        "loongarch",
        "mips",
        "powerpc",
        "s390x",
        "sparc",
        "hexagon",
        "xtensa",
    ];
    let segments: Vec<&str> = rest.split('-').collect();
    if segments.len() < 3
        || !segments.iter().all(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
    {
        return false;
    }
    ARCHES.iter().any(|a| segments[0].starts_with(a))
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
        assert_eq!(bin_name_from_arg(Some("tele")), "tele");
        assert_eq!(bin_name_from_arg(Some("/usr/local/bin/tele")), "tele");
        assert_eq!(bin_name_from_arg(Some("/usr/local/bin/tele.exe")), "tele");
        #[cfg(windows)]
        assert_eq!(bin_name_from_arg(Some("C:\\tools\\tele.exe")), "tele");
    }

    #[test]
    fn bin_name_from_arg_falls_back_to_cargo_bin_name() {
        assert_eq!(bin_name_from_arg(None), env!("CARGO_BIN_NAME"));
        assert_eq!(bin_name_from_arg(Some("")), env!("CARGO_BIN_NAME"));
    }

    #[test]
    fn bin_name_from_arg_normalizes_npm_platform_suffixed_stems() {
        assert_eq!(
            bin_name_from_arg(Some("tele-x86_64-pc-windows-msvc")),
            "tele"
        );
        assert_eq!(
            bin_name_from_arg(Some("telecli-x86_64-pc-windows-msvc")),
            "telecli"
        );
        assert_eq!(bin_name_from_arg(Some("tele-aarch64-apple-darwin")), "tele");
        assert_eq!(
            bin_name_from_arg(Some("tele-x86_64-unknown-linux-musl")),
            "tele"
        );
        #[cfg(windows)]
        assert_eq!(
            bin_name_from_arg(Some("C:\\npm\\tele-x86_64-pc-windows-msvc.exe")),
            "tele"
        );
    }

    #[test]
    fn bin_name_from_arg_keeps_alias_and_non_triple_stems() {
        assert_eq!(bin_name_from_arg(Some("tele")), "tele");
        assert_eq!(bin_name_from_arg(Some("tele.exe")), "tele");
        assert_eq!(bin_name_from_arg(Some("telecli")), "telecli");
        assert_eq!(bin_name_from_arg(Some("tele-server")), "tele-server");
    }

    #[test]
    fn completion_bin_name_defaults_to_cargo_bin() {
        assert!(matches!(env!("CARGO_BIN_NAME"), "tele" | "telecli"));
        let name = completion_bin_name();
        assert!(!name.is_empty());
    }

    #[test]
    fn bash_completions_reference_real_bin() {
        let bin = "tele";
        let out = gen(clap_complete::Shell::Bash, bin);
        assert!(out.contains(&format!("complete -F _{bin}")));
        assert!(out.contains(bin));
    }

    #[test]
    fn zsh_completions_have_compdef_for_real_bin() {
        let bin = "tele";
        let out = gen(clap_complete::Shell::Zsh, bin);
        assert!(out.contains(&format!("#compdef {bin}")));
    }

    #[test]
    fn fish_completions_have_complete_for_real_bin() {
        let bin = "tele";
        let out = gen(clap_complete::Shell::Fish, bin);
        assert!(out.contains(&format!("complete -c {bin}")));
    }

    #[test]
    fn powershell_completions_register_for_real_bin() {
        let bin = "tele";
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
