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
  execguard check [--shell ...] [--elevated] --command-from-env
  execguard record --shell <s> --cwd <path> --exit-code <n> --duration-ms <n>
                    --outcome ALLOW|WARN|CONFIRM|REJECT [--] \"<command>\"
  execguard record [...] --command-from-env
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
        "record" => record(rest),
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

/// Records a command that already finished running: where/how long it ran,
/// what it exited with, and what verdict ExecGuard gave it. Called by the
/// shell hooks' post-exec step, once those things are actually known.
///
/// A storage failure here is never a reason to fail: the command already
/// ran, so the only thing at stake is whether its history gets logged.
fn record(args: &[String]) -> Result<ExitCode, String> {
    let args = parse_record_args(args)?;

    let outcome = execguard_storage::Store::default_path()
        .map_err(|e| e.to_string())
        .and_then(|path| execguard_storage::Store::open(&path).map_err(|e| e.to_string()))
        .and_then(|store| {
            store
                .record(execguard_storage::NewExecution {
                    command: &args.command,
                    cwd: &args.cwd,
                    shell: &args.shell,
                    exit_code: args.exit_code,
                    duration_ms: args.duration_ms,
                    outcome: &args.outcome,
                })
                .map_err(|e| e.to_string())
        });

    if let Err(message) = outcome {
        eprintln!("execguard: could not record history: {message}");
    }

    Ok(ExitCode::SUCCESS)
}

struct RecordArgs {
    shell: String,
    cwd: String,
    exit_code: i32,
    duration_ms: i64,
    outcome: String,
    command: String,
}

fn parse_record_args(args: &[String]) -> Result<RecordArgs, String> {
    let mut shell = None;
    let mut cwd = None;
    let mut exit_code = None;
    let mut duration_ms = None;
    let mut outcome = None;
    let mut command_from_env = false;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--shell" => shell = Some(iter.next().ok_or("--shell needs a value")?.clone()),
            "--cwd" => cwd = Some(iter.next().ok_or("--cwd needs a value")?.clone()),
            "--exit-code" => {
                let value = iter.next().ok_or("--exit-code needs a value")?;
                exit_code = Some(
                    value.parse::<i32>().map_err(|_| format!("--exit-code: '{value}' is not a number"))?,
                );
            }
            "--duration-ms" => {
                let value = iter.next().ok_or("--duration-ms needs a value")?;
                duration_ms = Some(
                    value.parse::<i64>().map_err(|_| format!("--duration-ms: '{value}' is not a number"))?,
                );
            }
            "--outcome" => {
                let value = iter.next().ok_or("--outcome needs a value")?;
                outcome = Some(parse_outcome(value)?);
            }
            "--command-from-env" => command_from_env = true,
            "--" => {
                let rest: Vec<&str> = iter.by_ref().map(String::as_str).collect();
                return finish_record_args(shell, cwd, exit_code, duration_ms, outcome, rest.join(" "));
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option '{other}'"));
            }
            _ => {
                let rest: Vec<&str> = std::iter::once(arg.as_str())
                    .chain(iter.by_ref().map(String::as_str))
                    .collect();
                return finish_record_args(shell, cwd, exit_code, duration_ms, outcome, rest.join(" "));
            }
        }
    }

    if command_from_env {
        let command = env::var("EXECGUARD_COMMAND").map_err(|_| {
            "--command-from-env: EXECGUARD_COMMAND is not set or not valid text".to_string()
        })?;
        return finish_record_args(shell, cwd, exit_code, duration_ms, outcome, command);
    }

    Err("record: missing command".to_string())
}

/// Fills in a `RecordArgs` once the command text is known, checking that
/// every other flag (all required) was actually given.
fn finish_record_args(
    shell: Option<String>,
    cwd: Option<String>,
    exit_code: Option<i32>,
    duration_ms: Option<i64>,
    outcome: Option<String>,
    command: String,
) -> Result<RecordArgs, String> {
    Ok(RecordArgs {
        shell: shell.ok_or("record: --shell is required")?,
        cwd: cwd.ok_or("record: --cwd is required")?,
        exit_code: exit_code.ok_or("record: --exit-code is required")?,
        duration_ms: duration_ms.ok_or("record: --duration-ms is required")?,
        outcome: outcome.ok_or("record: --outcome is required")?,
        command,
    })
}

fn parse_outcome(value: &str) -> Result<String, String> {
    match value.to_ascii_uppercase().as_str() {
        // UNCHECKED: ExecGuard itself failed, so the command ran without a
        // real verdict (see the shell hooks' fail-open fallback).
        outcome @ ("ALLOW" | "WARN" | "CONFIRM" | "REJECT" | "UNCHECKED") => Ok(outcome.to_string()),
        other => {
            Err(format!("--outcome: unknown outcome '{other}' (want ALLOW, WARN, CONFIRM, REJECT, or UNCHECKED)"))
        }
    }
}

struct CheckArgs {
    shell: Shell,
    elevated: bool,
    command: String,
}

fn parse_check_args(args: &[String]) -> Result<CheckArgs, String> {
    let mut shell = Shell::Unknown;
    let mut elevated = false;
    let mut command_from_env = false;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--shell" => {
                let name = iter.next().ok_or("--shell needs a value")?;
                shell = Shell::from_name(name).ok_or_else(|| format!("unknown shell '{name}'"))?;
            }
            "--elevated" => elevated = true,
            "--command-from-env" => command_from_env = true,
            "--" => {
                let rest: Vec<&str> = iter.by_ref().map(String::as_str).collect();
                return Ok(CheckArgs { shell, elevated, command: rest.join(" ") });
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option '{other}'"));
            }
            _ => {
                let rest: Vec<&str> = std::iter::once(arg.as_str())
                    .chain(iter.by_ref().map(String::as_str))
                    .collect();
                return Ok(CheckArgs { shell, elevated, command: rest.join(" ") });
            }
        }
    }

    if command_from_env {
        let command = env::var("EXECGUARD_COMMAND").map_err(|_| {
            "--command-from-env: EXECGUARD_COMMAND is not set or not valid text".to_string()
        })?;
        return Ok(CheckArgs { shell, elevated, command });
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

    fn full_record_args(command: &str) -> Vec<String> {
        args(&[
            "--shell",
            "bash",
            "--cwd",
            "/home/ada/project",
            "--exit-code",
            "0",
            "--duration-ms",
            "12",
            "--outcome",
            "warn",
            "--",
            command,
        ])
    }

    #[test]
    fn record_reads_every_flag() {
        let parsed = parse_record_args(&full_record_args("rm -rf build")).unwrap();
        assert_eq!(parsed.shell, "bash");
        assert_eq!(parsed.cwd, "/home/ada/project");
        assert_eq!(parsed.exit_code, 0);
        assert_eq!(parsed.duration_ms, 12);
        assert_eq!(parsed.outcome, "WARN"); // normalized to uppercase
        assert_eq!(parsed.command, "rm -rf build");
    }

    #[test]
    fn record_requires_every_flag() {
        for missing in ["--shell", "--cwd", "--exit-code", "--duration-ms", "--outcome"] {
            let mut required = full_record_args("ls");
            // Drop the flag and the value right after it.
            let i = required.iter().position(|a| a == missing).unwrap();
            required.drain(i..i + 2);
            assert!(parse_record_args(&required).is_err(), "should have required {missing}");
        }
    }

    #[test]
    fn record_rejects_an_unknown_outcome() {
        let mut bad = full_record_args("ls");
        let i = bad.iter().position(|a| a == "warn").unwrap();
        bad[i] = "maybe".to_string();
        assert!(parse_record_args(&bad).is_err());
    }

    #[test]
    fn record_rejects_non_numeric_exit_code_or_duration() {
        let mut bad = full_record_args("ls");
        let i = bad.iter().position(|a| a == "0").unwrap();
        bad[i] = "soon".to_string();
        assert!(parse_record_args(&bad).is_err());
    }
}