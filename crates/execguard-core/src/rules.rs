//! Built-in safety rules.

use crate::{Action, Context, Finding, Rule, Shell};
use std::path::Path;

/// Every built-in rule, ready to hand to the engine.
pub fn builtin() -> Vec<Box<dyn Rule>> {
    vec![Box::new(RecursiveDelete)]
}

/// Catches recursive deletes in Unix shells and PowerShell.
pub struct RecursiveDelete;

impl Rule for RecursiveDelete {
    fn id(&self) -> &'static str {
        "fs.recursive-delete"
    }

    fn evaluate(&self, command: &str, ctx: &Context) -> Option<Finding> {
        let mut tokens = command.split_whitespace().peekable();

        // `sudo rm ...`: skip the prefix, but remember that it means root.
        let mut elevated = ctx.is_elevated;
        while let Some(&word) = tokens.peek() {
            if word == "sudo" || word == "doas" {
                elevated = true;
                tokens.next();
            } else {
                break;
            }
        }

        let program = tokens.next()?;
        let args: Vec<&str> = tokens.collect();

        // Judge the command under every meaning it could have,
        // and keep the most severe verdict.
        dialects_for(program, ctx.shell)
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
                Action::Reject,
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

/// Which command-line language a delete command is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialect {
    Unix,
    PowerShell,
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

/// Is this target a filesystem root or the user's home directory?
fn is_dangerous_target(target: &str, dialect: Dialect, ctx: &Context) -> bool {
    let unquoted = target.trim_matches(|c| c == '"' || c == '\'');

    // PowerShell paths are case-insensitive and accept `\` or `/`.
    // Convert them to one canonical form so a single table covers every spelling.
    let normalized = match dialect {
        Dialect::Unix => unquoted.to_string(),
        Dialect::PowerShell => unquoted.to_ascii_lowercase().replace('\\', "/"),
    };
    let path = normalized.trim_end_matches('/');

    match path {
        // `/` and `//` become "" once trailing slashes are trimmed.
        "" | "/*" | "~" | "~/*" => true,
        "$HOME" | "${HOME}" | "$HOME/*" | "${HOME}/*" => dialect == Dialect::Unix,
        "$home" | "$home/*" | "$env:userprofile" | "$env:userprofile/*" => dialect == Dialect::PowerShell,

        // Relative targets: harmless in a project folder, fatal in ~ or /.
        "*" | "." | "./*" => ctx.cwd_is_root() || ctx.cwd_is_home(),
        ".." => match ctx.cwd.parent() {
            None => true,
            Some(parent) => parent.parent().is_none() || ctx.home.as_deref() == Some(parent),
        },

        _ if dialect == Dialect::PowerShell && is_drive_root(path) => true,
        _ => is_home_path(path, dialect, ctx),
    }
}

/// `c:` or `c:/*` (already normalized): the root of a Windows drive.
fn is_drive_root(path: &str) -> bool {
    let path = path.strip_suffix("/*").unwrap_or(path);
    let bytes = path.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// An absolute path that happens to be the home directory.
fn is_home_path(path: &str, dialect: Dialect, ctx: &Context) -> bool {
    let Some(home) = ctx.home.as_deref() else {
        return false;
    };

    match dialect {
        Dialect::Unix => Path::new(path) == home,
        Dialect::PowerShell => {
            let home = home.to_string_lossy().to_ascii_lowercase().replace('\\', "/");
            path == home.trim_end_matches('/')
        }
    }
}


     




#[cfg(test)]
mod tests {
    use super::*;
    use crate::Shell;
    use std::path::PathBuf;

    fn ctx() -> Context {
        Context {
            cwd: PathBuf::from("/home/ada/project"),
            home: Some(PathBuf::from("/home/ada")),
            shell: Shell::Bash,
            is_elevated: false,
        }
    }

    fn check(cmd: &str) -> Option<Action> {
        RecursiveDelete.evaluate(cmd, &ctx()).map(|f| f.action)
    }

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

        fn check_in(cwd: &str, cmd: &str) -> Option<Action> {
        let mut c = ctx();
        c.cwd = PathBuf::from(cwd);
        RecursiveDelete.evaluate(cmd, &c).map(|f| f.action)
    }

    #[test]
    fn wiping_root_or_home_is_rejected() {
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
            assert_eq!(check(cmd), Some(Action::Reject), "not rejected: {cmd}");
        }
    }

    #[test]
    fn force_is_not_needed_to_reject() {
        assert_eq!(check("rm -r /"), Some(Action::Reject));
    }

    #[test]
    fn one_dangerous_target_is_enough() {
        assert_eq!(check("rm -rf build ~ dist"), Some(Action::Reject));
    }

    #[test]
    fn star_depends_on_where_you_are() {
        assert_eq!(check_in("/home/ada/project/build", "rm -rf *"), Some(Action::Warn));
        assert_eq!(check_in("/home/ada", "rm -rf *"), Some(Action::Reject));
        assert_eq!(check_in("/", "rm -rf ."), Some(Action::Reject));
    }

    #[test]
    fn folders_inside_home_are_only_warned() {
        assert_eq!(check("rm -rf ~/project/build"), Some(Action::Warn));
        assert_eq!(check("rm -rf /home/ada/project"), Some(Action::Warn));
    }

    /// The `sudo` prefix is treated as a flag that makes the command more dangerous.
    #[test]
    fn program_paths_are_recognised() {
        assert_eq!(check("/bin/rm -rf build"), Some(Action::Warn));
        assert_eq!(check("/usr/bin/rm -rf build"), Some(Action::Warn));
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
        let action = RecursiveDelete.evaluate("rm -r build", &c).map(|f| f.action);
        assert_eq!(action, Some(Action::Confirm));
    }

    #[test]
    fn dangerous_targets_are_still_rejected_under_sudo() {
        assert_eq!(check("sudo rm -rf /"), Some(Action::Reject));
        assert_eq!(check("sudo rm -r ~"), Some(Action::Reject));
    }

    #[test]
    fn sudo_without_a_recursive_rm_is_ignored() {
        assert_eq!(check("sudo"), None);
        assert_eq!(check("sudo apt update"), None);
        assert_eq!(check("sudo rm notes.txt"), None);
    }

    /// PowerShell is case-insensitive and allows `\` or `/` in paths.
        fn check_ps(cmd: &str) -> Option<Action> {
        let c = Context {
            cwd: PathBuf::from(r"C:\Users\Ada\project"),
            home: Some(PathBuf::from(r"C:\Users\Ada")),
            shell: Shell::PowerShell,
            is_elevated: false,
        };
        RecursiveDelete.evaluate(cmd, &c).map(|f| f.action)
    }

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
    fn powershell_dangerous_targets_are_rejected() {
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
            assert_eq!(check_ps(cmd), Some(Action::Reject), "not rejected: {cmd}");
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
}