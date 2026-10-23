//! Catches recursive deletes in Unix shells and PowerShell.

use super::targets::{is_dangerous_target, Dialect};
use crate::{Action, Context, Finding, Rule, Shell, SimpleCommand};

pub struct RecursiveDelete;

impl Rule for RecursiveDelete {
    fn id(&self) -> &'static str {
        "fs.recursive-delete"
    }

    fn evaluate(&self, command: &SimpleCommand, ctx: &Context) -> Option<Finding> {
        // `sudo`/`doas` are already stripped from `command` by the parser;
        // it just tells us they were there.
        let elevated = ctx.is_elevated || command.elevated;
        let args: Vec<&str> = command.args.iter().map(String::as_str).collect();

        // Judge the command under every meaning it could have,
        // and keep the most severe verdict.
        dialects_for(&command.program, ctx.shell)
            .iter()
            .filter_map(|&dialect| self.judge(dialect, &args, elevated, ctx))
            .max_by_key(|finding| finding.action)
    }
}

impl RecursiveDelete {
    /// Judge the arguments under one dialect's rules.
    fn judge(&self, dialect: Dialect, args: &[&str], elevated: bool, ctx: &Context) -> Option<Finding> {
        let parsed = match dialect {
            Dialect::Unix => parse_rm_args(args),
            Dialect::PowerShell => parse_powershell_args(args),
        };

        if !parsed.recursive {
            return None;
        }

        if let Some(target) = parsed.targets.iter().find(|t| is_dangerous_target(t, dialect, ctx)) {
            return Some(Finding::new(
                self.id(),
                Action::Confirm,
                format!("recursive delete of '{target}' would wipe a root or home directory"),
            ));
        }

        if elevated {
            return Some(Finding::new(
                self.id(),
                Action::Confirm,
                "recursive delete with root or administrator privileges; a mistake here can damage the system",
            ));
        }

        if parsed.force {
            return Some(Finding::new(
                self.id(),
                Action::Warn,
                "forced recursive delete; files skip the trash and cannot be recovered",
            ));
        }

        None
    }
}

/// Every dialect a program name could mean in this shell.
/// Empty means "not a delete command".
fn dialects_for(program: &str, shell: Shell) -> &'static [Dialect] {
    match shell {
        Shell::PowerShell => {
            let name = program
                .rsplit(|c| c == '/' || c == '\\')
                .next()
                .unwrap_or(program)
                .to_ascii_lowercase();

            match name.as_str() {
                // An explicit .exe skips aliases: Git for Windows ships a real Unix rm.exe.
                "rm.exe" => &[Dialect::Unix],
                // Remove-Item on Windows, but the real /bin/rm in PowerShell 7 on Linux/macOS.
                "rm" => &[Dialect::PowerShell, Dialect::Unix],
                "remove-item" | "del" | "erase" | "rd" | "rmdir" | "ri" => &[Dialect::PowerShell],
                _ => &[],
            }
        }
        _ => {
            if program_name(program) == "rm" {
                &[Dialect::Unix]
            } else {
                &[]
            }
        }
    }
}

/// The bare Unix program name: `/bin/rm` and `\rm` become `rm`.
fn program_name(program: &str) -> &str {
    let base = program.rsplit('/').next().unwrap_or(program);
    base.trim_start_matches('\\')
}

/// What we learned from a delete command's arguments.
struct RmArgs<'a> {
    recursive: bool,
    force: bool,
    targets: Vec<&'a str>,
}

/// Reads Unix `rm` arguments: flags (however they are spelled) and targets.
fn parse_rm_args<'a>(args: &[&'a str]) -> RmArgs<'a> {
    let mut parsed = RmArgs { recursive: false, force: false, targets: Vec::new() };
    let mut end_of_options = false;

    for &arg in args {
        if end_of_options || arg == "-" || !arg.starts_with('-') {
            parsed.targets.push(arg);
            continue;
        }

        if arg == "--" {
            end_of_options = true;
        } else if let Some(long) = arg.strip_prefix("--") {
            match long {
                "recursive" => parsed.recursive = true,
                "force" => parsed.force = true,
                _ => {}
            }
        } else {
            for c in arg[1..].chars() {
                match c {
                    'r' | 'R' => parsed.recursive = true,
                    'f' => parsed.force = true,
                    _ => {}
                }
            }
        }
    }

    parsed
}

/// Reads PowerShell `Remove-Item` arguments.
/// Parameters are case-insensitive and may be shortened to any
/// unambiguous prefix: `-Recurse`, `-rec`, `-r` all work.
fn parse_powershell_args<'a>(args: &[&'a str]) -> RmArgs<'a> {
    let mut parsed = RmArgs { recursive: false, force: false, targets: Vec::new() };

    for &arg in args {
        let Some(param) = arg.strip_prefix('-') else {
            parsed.targets.push(arg);
            continue;
        };

        // `-Recurse:$true` -> `recurse`
        let name = param.split(':').next().unwrap_or(param).to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }

        if "recurse".starts_with(name.as_str()) {
            parsed.recursive = true;
        } else if name.len() >= 2 && "force".starts_with(name.as_str()) {
            // `-f` alone is ambiguous (-Filter or -Force), so Force needs `-fo` or longer.
            parsed.force = true;
        }
    }

    parsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx() -> Context {
        Context {
            cwd: PathBuf::from("/home/ada/project"),
            home: Some(PathBuf::from("/home/ada")),
            shell: Shell::Bash,
            is_elevated: false,
        }
    }

    /// Parses `cmd` as the given shell would and returns its first simple
    /// command, the way `Engine` does internally. Tests use this to call
    /// `RecursiveDelete` directly without going through `Engine`.
    fn parse_first(cmd: &str, shell: Shell) -> SimpleCommand {
        let syntax = match shell {
            Shell::PowerShell => execguard_parser::Syntax::PowerShell,
            _ => execguard_parser::Syntax::Posix,
        };
        execguard_parser::parse(cmd, syntax).into_iter().next().unwrap_or_default()
    }

    fn check(cmd: &str) -> Option<Action> {
        RecursiveDelete.evaluate(&parse_first(cmd, Shell::Bash), &ctx()).map(|f| f.action)
    }

    fn check_in(cwd: &str, cmd: &str) -> Option<Action> {
        let mut c = ctx();
        c.cwd = PathBuf::from(cwd);
        RecursiveDelete.evaluate(&parse_first(cmd, Shell::Bash), &c).map(|f| f.action)
    }

    fn check_ps(cmd: &str) -> Option<Action> {
        let c = Context {
            cwd: PathBuf::from(r"C:\Users\Ada\project"),
            home: Some(PathBuf::from(r"C:\Users\Ada")),
            shell: Shell::PowerShell,
            is_elevated: false,
        };
        RecursiveDelete.evaluate(&parse_first(cmd, Shell::PowerShell), &c).map(|f| f.action)
    }

    // ---- Unix: flags ----

    #[test]
    fn all_flag_spellings_are_caught() {
        for cmd in [
            "rm -rf build",
            "rm -fr build",
            "rm -r -f build",
            "rm -Rf build",
            "rm -rfv build",
            "rm --recursive --force build",
        ] {
            assert_eq!(check(cmd), Some(Action::Warn), "missed: {cmd}");
        }
    }

    #[test]
    fn plain_or_partial_deletes_are_ignored() {
        for cmd in ["rm notes.txt", "rm -r build", "rm -f notes.txt", "ls -rf"] {
            assert_eq!(check(cmd), None, "false alarm: {cmd}");
        }
    }

    #[test]
    fn nothing_after_double_dash_is_a_flag() {
        assert_eq!(check("rm -- -rf"), None);
        assert_eq!(check("rm -r -- -f"), None);
    }

    #[test]
    fn empty_commands_are_ignored() {
        assert_eq!(check(""), None);
        assert_eq!(check("   "), None);
    }

    // ---- Unix: dangerous targets ----

    #[test]
    fn wiping_root_or_home_needs_confirmation() {
        for cmd in [
            "rm -rf /",
            "rm -rf //",
            "rm -rf /*",
            "rm -rf ~",
            "rm -rf ~/",
            "rm -rf ~/*",
            "rm -rf $HOME",
            "rm -rf \"$HOME\"",
            "rm -rf /home/ada",
            "rm -rf -- /",
        ] {
            assert_eq!(check(cmd), Some(Action::Confirm), "not confirmed: {cmd}");
        }
    }

    #[test]
    fn force_is_not_needed_to_confirm() {
        assert_eq!(check("rm -r /"), Some(Action::Confirm));
    }

    #[test]
    fn one_dangerous_target_is_enough() {
        assert_eq!(check("rm -rf build ~ dist"), Some(Action::Confirm));
    }

    #[test]
    fn star_depends_on_where_you_are() {
        assert_eq!(check_in("/home/ada/project/build", "rm -rf *"), Some(Action::Warn));
        assert_eq!(check_in("/home/ada", "rm -rf *"), Some(Action::Confirm));
        assert_eq!(check_in("/", "rm -rf ."), Some(Action::Confirm));
    }

    #[test]
    fn deleting_the_parent_of_a_project_needs_confirmation() {
        assert_eq!(check_in("/home/ada/project", "rm -rf .."), Some(Action::Confirm));
        assert_eq!(check_in("/home/ada/project/build", "rm -rf .."), Some(Action::Warn));
    }

    #[test]
    fn folders_inside_home_are_only_warned() {
        assert_eq!(check("rm -rf ~/project/build"), Some(Action::Warn));
        assert_eq!(check("rm -rf /home/ada/project"), Some(Action::Warn));
    }

    // ---- Unix: program paths and privilege ----

    #[test]
    fn program_paths_are_recognised() {
        assert_eq!(check("/bin/rm -rf build"), Some(Action::Warn));
        assert_eq!(check("/usr/bin/rm -rf build"), Some(Action::Warn));
        assert_eq!(check("\\rm -rf build"), Some(Action::Warn));
    }

    #[test]
    fn sudo_escalates_to_confirm() {
        assert_eq!(check("sudo rm -rf build"), Some(Action::Confirm));
        assert_eq!(check("sudo rm -r build"), Some(Action::Confirm));
        assert_eq!(check("doas /bin/rm -rf build"), Some(Action::Confirm));
        assert_eq!(check("sudo sudo rm -rf build"), Some(Action::Confirm));
    }

    #[test]
    fn root_shell_escalates_to_confirm() {
        let mut c = ctx();
        c.is_elevated = true;
        let action = RecursiveDelete
            .evaluate(&parse_first("rm -r build", Shell::Bash), &c)
            .map(|f| f.action);
        assert_eq!(action, Some(Action::Confirm));
    }

    #[test]
    fn dangerous_targets_under_sudo_show_the_danger_message() {
        let finding = RecursiveDelete
            .evaluate(&parse_first("sudo rm -rf /", Shell::Bash), &ctx())
            .expect("should object");
        assert_eq!(finding.action, Action::Confirm);
        assert!(finding.message.contains("would wipe"), "wrong message: {}", finding.message);
    }

    #[test]
    fn sudo_without_a_recursive_rm_is_ignored() {
        assert_eq!(check("sudo"), None);
        assert_eq!(check("sudo apt update"), None);
        assert_eq!(check("sudo rm notes.txt"), None);
    }

    // ---- PowerShell ----

    #[test]
    fn powershell_spellings_are_caught() {
        for cmd in [
            "Remove-Item -Recurse -Force build",
            "remove-item -recurse -force build",
            "Remove-Item -Path build -Recurse -Force",
            "ri build -Recurse -Force",
            "del -Rec -Force build",
            "rm -r -fo build",
        ] {
            assert_eq!(check_ps(cmd), Some(Action::Warn), "missed: {cmd}");
        }
    }

    #[test]
    fn powershell_dangerous_targets_need_confirmation() {
        for cmd in [
            r"Remove-Item -Recurse -Force C:\",
            r"Remove-Item -Recurse C:\*",
            r"rm -r -fo D:\",
            "Remove-Item -Recurse -Force ~",
            "Remove-Item -Recurse -Force $HOME",
            "Remove-Item -Recurse -Force $env:USERPROFILE",
            r"Remove-Item -Recurse -Force C:\Users\Ada",
            r"Remove-Item -Recurse -Force c:\users\ada\",
        ] {
            assert_eq!(check_ps(cmd), Some(Action::Confirm), "not confirmed: {cmd}");
        }
    }

    #[test]
    fn powershell_parameter_rules_are_respected() {
        // `-rf` is one (invalid) parameter name in PowerShell, not two flags.
        assert_eq!(check_ps("Remove-Item -rf build"), None);
        // `-f` alone is ambiguous (-Filter or -Force), so it is not Force.
        assert_eq!(check_ps("Remove-Item -r -f build"), None);
    }

    #[test]
    fn rm_in_powershell_is_judged_both_ways() {
        // PowerShell 7 on Linux/macOS runs the real /bin/rm.
        assert_eq!(check_ps("rm -rf build"), Some(Action::Warn));
        // Git for Windows' rm.exe is a Unix rm, alias or not.
        assert_eq!(check_ps("rm.exe -rf build"), Some(Action::Warn));
    }

    #[test]
    fn unix_shells_do_not_know_powershell_names() {
        assert_eq!(check("Remove-Item -Recurse -Force build"), None);
    }

    // ---- Gaps fixed by the real parser (dev-doc/HANDOFF.md 6.4) ----

    #[test]
    fn a_quoted_target_with_spaces_is_one_argument_not_two() {
        // Before the parser, "my folder" split on whitespace into two
        // separate targets, "my" and "folder" -- neither a real path.
        assert_eq!(check(r#"rm -rf "my folder""#), Some(Action::Warn));
    }

    #[test]
    fn a_chained_command_is_still_checked() {
        // Before the parser, only the first word of the whole line ("cd")
        // was ever looked at, so this rm was invisible.
        let engine = crate::Engine::with_builtin_rules();
        let mut c = ctx();
        c.cwd = PathBuf::from("/home/ada");
        let action = engine.evaluate("cd ~ && rm -rf *", &c).action();
        assert_eq!(action, Action::Confirm);
    }

    #[test]
    fn wrapper_commands_no_longer_hide_the_real_program() {
        for cmd in ["env rm -rf build", "nohup rm -rf build", "command rm -rf build"] {
            assert_eq!(check(cmd), Some(Action::Warn), "missed: {cmd}");
        }
    }

    // ---- The promise: built-in rules never reject ----

    #[test]
    fn built_in_rules_never_reject() {
        let engine = crate::Engine::with_builtin_rules();
        let mut c = ctx();
        c.is_elevated = true;
        for cmd in ["rm -rf /", "rm -rf ~", "sudo rm -rf /*", "rm -rf $HOME", "rm -rf .."] {
            let action = engine.evaluate(cmd, &c).action();
            assert_ne!(action, Action::Reject, "built-in rules rejected: {cmd}");
        }
    }
}
