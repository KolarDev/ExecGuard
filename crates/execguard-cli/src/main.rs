//! `execguard` command-line entry point.
//!
//! Exit codes (the contract with the shell hooks):
//!   0 ALLOW, 10 WARN, 20 CONFIRM, 30 REJECT,
//!   64 usage error, 70 internal error. Anything else: ExecGuard broke.

use std::env;
use std::panic;
use std::path::PathBuf;
use std::process::ExitCode;

use execguard_core::{Context, Engine, Shell};

const EXIT_USAGE: u8 = 64;
const EXIT_INTERNAL: u8 = 70;

const USAGE: &str = "\
usage:
  execguard check [--shell bash|zsh|powershell] [--elevated] [--] \"<command>\"
  execguard help";

fn main() -> ExitCode {
    let outcome = panic::catch_unwind(|| run(env::args().skip(1).collect()));

    match outcome {
        Ok(Ok(code)) => code,
        Ok(Err(message)) => {
            eprintln!("execguard: {message}");
            ExitCode::from(EXIT_USAGE)
        }
        Err(_) => {
            eprintln!("execguard: internal error; this command was NOT checked");
            ExitCode::from(EXIT_INTERNAL)
        }
    }
}

fn run(args: Vec<String>) -> Result<ExitCode, String> {
    let Some((subcommand, rest)) = args.split_first() else {
        return Err(USAGE.to_string());
    };

    match subcommand.as_str() {
        "check" => check(rest),
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown command '{other}'\n{USAGE}")),
    }
}

fn check(args: &[String]) -> Result<ExitCode, String> {
    let args = parse_check_args(args)?;

    let ctx = Context {
        cwd: env::current_dir().unwrap_or_default(),
        home: env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .map(PathBuf::from),
        shell: args.shell,
        is_elevated: args.elevated,
    };

    let decision = Engine::with_builtin_rules().evaluate(&args.command, &ctx);

    for finding in decision.findings() {
        eprintln!("execguard {finding}");
    }

    Ok(ExitCode::from(decision.action().exit_code()))
}

struct CheckArgs {
    shell: Shell,
    elevated: bool,
    command: String,
}

fn parse_check_args(args: &[String]) -> Result<CheckArgs, String> {
    let mut shell = Shell::Unknown;
    let mut elevated = false;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--shell" => {
                let name = iter.next().ok_or("--shell needs a value")?;
                shell = Shell::from_name(name).ok_or_else(|| format!("unknown shell '{name}'"))?;
            }
            "--elevated" => elevated = true,
            "--" => {
                let rest: Vec<&str> = iter.by_ref().map(String::as_str).collect();
                return Ok(CheckArgs { shell, elevated, command: rest.join(" ") });
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option '{other}'"));
            }
            _ => {
                // The first word that isn't an option starts the command.
                let rest: Vec<&str> = std::iter::once(arg.as_str())
                    .chain(iter.by_ref().map(String::as_str))
                    .collect();
                return Ok(CheckArgs { shell, elevated, command: rest.join(" ") });
            }
        }
    }

    Err("check: missing command".to_string())
}







#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reads_options_and_command() {
        let parsed = parse_check_args(&args(&["--shell", "zsh", "--elevated", "--", "rm -rf build"])).unwrap();
        assert_eq!(parsed.shell, Shell::Zsh);
        assert!(parsed.elevated);
        assert_eq!(parsed.command, "rm -rf build");
    }

    #[test]
    fn double_dash_is_optional() {
        let parsed = parse_check_args(&args(&["--shell", "bash", "rm -rf build"])).unwrap();
        assert_eq!(parsed.command, "rm -rf build");
    }

    #[test]
    fn split_command_words_are_joined() {
        let parsed = parse_check_args(&args(&["--", "rm", "-rf", "build"])).unwrap();
        assert_eq!(parsed.command, "rm -rf build");
        assert_eq!(parsed.shell, Shell::Unknown);
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(parse_check_args(&args(&[])).is_err());
        assert!(parse_check_args(&args(&["--shell"])).is_err());
        assert!(parse_check_args(&args(&["--shell", "fish", "ls"])).is_err());
        assert!(parse_check_args(&args(&["--verbose", "ls"])).is_err());
    }
}