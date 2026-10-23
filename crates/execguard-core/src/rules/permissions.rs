//! Catches recursive `chmod`/`chown` that either grant everyone write
//! access or touch a root/home directory.
//!
//! `chmod`'s mode is only checked precisely when it's numeric (`777`,
//! `0755`, ...); a symbolic mode (`o+w`, `a=rwx`, ...) isn't parsed, so it
//! is treated as unknown and, leaning cautious, warned about anyway.

use super::program::is_program;
use super::targets::{is_dangerous_target, Dialect};
use crate::{Action, Context, Finding, Rule, SimpleCommand};

pub struct PermissionBomb;

impl Rule for PermissionBomb {
    fn id(&self) -> &'static str {
        "fs.permission-bomb"
    }

    fn evaluate(&self, command: &SimpleCommand, ctx: &Context) -> Option<Finding> {
        let args: Vec<&str> = command.args.iter().map(String::as_str).collect();

        if is_program(&command.program, "chmod") {
            return self.judge_chmod(&args, ctx);
        }
        if is_program(&command.program, "chown") {
            return self.judge_chown(&args, ctx);
        }
        None
    }
}

impl PermissionBomb {
    fn judge_chmod(&self, args: &[&str], ctx: &Context) -> Option<Finding> {
        if !has_recursive_flag(args) {
            return None;
        }

        // The mode is the first non-flag argument; everything after it is targets.
        let (mode, targets) = first_non_flag(args)?;
        let world_writable = numeric_mode_is_world_writable(mode).unwrap_or(true); // unknown (symbolic): lean cautious

        if !world_writable {
            return None;
        }

        if targets.iter().any(|t| is_dangerous_target(t, Dialect::Unix, ctx)) {
            return Some(Finding::new(
                self.id(),
                Action::Confirm,
                "recursively making a root or home directory world-writable exposes the whole system",
            ));
        }

        Some(Finding::new(
            self.id(),
            Action::Warn,
            "recursively making files world-writable; anyone on the system can modify them",
        ))
    }

    fn judge_chown(&self, args: &[&str], ctx: &Context) -> Option<Finding> {
        if !has_recursive_flag(args) {
            return None;
        }

        let (_owner, targets) = first_non_flag(args)?;
        if targets.iter().any(|t| is_dangerous_target(t, Dialect::Unix, ctx)) {
            return Some(Finding::new(
                self.id(),
                Action::Confirm,
                "recursively changing ownership of a root or home directory can lock you, or the system, out",
            ));
        }

        None
    }
}

/// The first argument that isn't a flag (chmod/chown's mode or owner
/// spec), and everything after it (the targets).
fn first_non_flag<'a>(args: &'a [&'a str]) -> Option<(&'a str, &'a [&'a str])> {
    let i = args.iter().position(|a| !a.starts_with('-'))?;
    Some((args[i], &args[i + 1..]))
}

fn has_short_flag(args: &[&str], c: char) -> bool {
    args.iter().any(|&a| a.starts_with('-') && !a.starts_with("--") && a[1..].contains(c))
}

fn has_recursive_flag(args: &[&str]) -> bool {
    args.contains(&"--recursive") || has_short_flag(args, 'R')
}

/// Whether a numeric mode (`"777"`, `"0755"`, ...) grants write access to
/// "other" -- the last digit's write bit. `None` means `mode` isn't a
/// plain numeric mode at all.
fn numeric_mode_is_world_writable(mode: &str) -> Option<bool> {
    if mode.is_empty() || mode.len() > 4 || !mode.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let other = mode.as_bytes().last().unwrap() - b'0';
    if other > 7 {
        return None;
    }
    Some(other & 0b010 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx() -> Context {
        Context {
            cwd: PathBuf::from("/home/ada/project"),
            home: Some(PathBuf::from("/home/ada")),
            shell: crate::Shell::Bash,
            is_elevated: false,
        }
    }

    fn check(cmd: &str) -> Option<Action> {
        let simple = execguard_parser::parse(cmd, execguard_parser::Syntax::Posix).into_iter().next().unwrap();
        PermissionBomb.evaluate(&simple, &ctx()).map(|f| f.action)
    }

    #[test]
    fn recursive_world_writable_on_a_project_folder_warns() {
        assert_eq!(check("chmod -R 777 build"), Some(Action::Warn));
        assert_eq!(check("chmod --recursive 666 build"), Some(Action::Warn));
    }

    #[test]
    fn recursive_world_writable_on_root_or_home_needs_confirmation() {
        assert_eq!(check("chmod -R 777 /"), Some(Action::Confirm));
        assert_eq!(check("chmod -R 777 ~"), Some(Action::Confirm));
    }

    #[test]
    fn non_world_writable_modes_are_quiet() {
        assert_eq!(check("chmod -R 755 build"), None);
        assert_eq!(check("chmod -R 644 build"), None);
    }

    #[test]
    fn non_recursive_chmod_is_quiet_even_at_777() {
        assert_eq!(check("chmod 777 build"), None);
    }

    #[test]
    fn symbolic_modes_are_unknown_so_they_warn() {
        assert_eq!(check("chmod -R o+w build"), Some(Action::Warn));
    }

    #[test]
    fn chown_on_an_ordinary_target_is_quiet() {
        assert_eq!(check("chown -R ada:ada build"), None);
    }

    #[test]
    fn chown_on_root_or_home_needs_confirmation() {
        assert_eq!(check("chown -R root:root /"), Some(Action::Confirm));
        assert_eq!(check("chown -R ada:ada ~"), Some(Action::Confirm));
    }

    #[test]
    fn non_recursive_chown_is_quiet() {
        assert_eq!(check("chown root:root /"), None);
    }

    #[test]
    fn other_programs_are_ignored() {
        assert_eq!(check("rm -rf build"), None);
    }
}
