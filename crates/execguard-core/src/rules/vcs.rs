//! Catches Git operations that can discard work: force-pushing (overwrites
//! the remote branch, and anyone else's unpushed commits on it), a hard
//! reset (discards uncommitted local changes), and a forced clean
//! (deletes untracked files, and ignored ones too with `-d`/`-x`).
//!
//! Known gap: this only recognises `git <subcommand>` as the very first
//! word after the program name. A global option before the subcommand,
//! like `git -C some/path push --force`, is not recognised.

use super::program::is_program;
use crate::{Action, Context, Finding, Rule, SimpleCommand};

pub struct HistoryRewrite;

impl Rule for HistoryRewrite {
    fn id(&self) -> &'static str {
        "vcs.history-rewrite"
    }

    fn evaluate(&self, command: &SimpleCommand, _ctx: &Context) -> Option<Finding> {
        if !is_program(&command.program, "git") {
            return None;
        }

        let args: Vec<&str> = command.args.iter().map(String::as_str).collect();
        let (subcommand, rest) = args.split_first()?;

        match *subcommand {
            "push" if has_force_flag(rest) => Some(Finding::new(
                self.id(),
                Action::Confirm,
                "force-pushing overwrites the remote branch's history; anyone else's unpushed work on it is lost",
            )),
            "reset" if rest.contains(&"--hard") => Some(Finding::new(
                self.id(),
                Action::Warn,
                "a hard reset discards uncommitted changes; they cannot be recovered afterwards",
            )),
            "clean" if has_force_flag(rest) => {
                let aggressive = has_short_flag(rest, 'd') || has_short_flag(rest, 'x') || has_short_flag(rest, 'X');
                Some(Finding::new(
                    self.id(),
                    if aggressive { Action::Confirm } else { Action::Warn },
                    if aggressive {
                        "a forced clean with -d/-x deletes untracked directories and files Git is told to ignore; \
                         anything you forgot to `git add`, including ignored config or secrets, goes with them"
                    } else {
                        "a forced clean deletes untracked files; anything you forgot to `git add` goes with them"
                    },
                ))
            }
            _ => None,
        }
    }
}

/// A single-dash option bundles its letters (`-fd` is `-f` and `-d`), so
/// this checks for `c` among them, not just a lone `-c`.
fn has_short_flag(args: &[&str], c: char) -> bool {
    args.iter().any(|&a| a.starts_with('-') && !a.starts_with("--") && a[1..].contains(c))
}

/// `--force`/`-f`, but not the deliberately safer `--force-with-lease`.
fn has_force_flag(args: &[&str]) -> bool {
    args.contains(&"--force") || has_short_flag(args, 'f')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx() -> Context {
        Context { cwd: PathBuf::from("/home/ada/project"), home: None, shell: crate::Shell::Bash, is_elevated: false }
    }

    fn check(cmd: &str) -> Option<Action> {
        let simple = execguard_parser::parse(cmd, execguard_parser::Syntax::Posix).into_iter().next().unwrap();
        HistoryRewrite.evaluate(&simple, &ctx()).map(|f| f.action)
    }

    #[test]
    fn force_push_needs_confirmation() {
        assert_eq!(check("git push --force origin main"), Some(Action::Confirm));
        assert_eq!(check("git push -f origin main"), Some(Action::Confirm));
        assert_eq!(check("git push -fu origin main"), Some(Action::Confirm));
    }

    #[test]
    fn plain_push_is_ignored() {
        assert_eq!(check("git push origin main"), None);
    }

    #[test]
    fn force_with_lease_is_the_safer_choice_and_ignored() {
        assert_eq!(check("git push --force-with-lease origin main"), None);
    }

    #[test]
    fn hard_reset_warns() {
        assert_eq!(check("git reset --hard"), Some(Action::Warn));
        assert_eq!(check("git reset --hard HEAD~3"), Some(Action::Warn));
    }

    #[test]
    fn soft_or_plain_reset_is_ignored() {
        assert_eq!(check("git reset --soft HEAD~1"), None);
        assert_eq!(check("git reset"), None);
    }

    #[test]
    fn forced_clean_without_directories_or_ignored_files_warns() {
        assert_eq!(check("git clean -f"), Some(Action::Warn));
        assert_eq!(check("git clean --force"), Some(Action::Warn));
    }

    #[test]
    fn forced_clean_with_directories_or_ignored_files_needs_confirmation() {
        assert_eq!(check("git clean -fd"), Some(Action::Confirm));
        assert_eq!(check("git clean -fx"), Some(Action::Confirm));
        assert_eq!(check("git clean -fdx"), Some(Action::Confirm));
    }

    #[test]
    fn clean_without_force_is_ignored() {
        assert_eq!(check("git clean -n"), None);
        assert_eq!(check("git clean -d"), None);
    }

    #[test]
    fn other_git_subcommands_and_other_programs_are_ignored() {
        assert_eq!(check("git status"), None);
        assert_eq!(check("git log --oneline"), None);
        assert_eq!(check("rm -rf build"), None);
    }
}
