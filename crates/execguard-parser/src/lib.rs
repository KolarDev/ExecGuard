//! Splits one typed command line into the simple commands it actually runs.
//!
//! A shell hook hands ExecGuard the whole line the user typed, which can be
//! several commands glued together (`cd ~ && rm -rf *`) and can wrap the
//! real program in prefixes (`sudo`, `env`, `nohup`, `time`, `FOO=bar ...`).
//! [`parse`] turns that line into a list of [`SimpleCommand`]s — one per
//! piece — with the wrapping already peeled off, so a rule only ever has to
//! think about one real program and its arguments.
//!
//! This crate does not look at the filesystem, the environment, or the
//! shell's actual variable values: it only reads the text of the line.

mod tokenize;

use tokenize::Token;

/// Which shell's quoting and punctuation rules to use when splitting a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// Bash, Zsh, and anything unrecognised.
    Posix,
    PowerShell,
}

/// One real command, after wrapper stripping and quote removal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SimpleCommand {
    /// The program being run, e.g. `"rm"`. Wrappers like `sudo` and `env`
    /// are never the program — they're stripped before this is set.
    pub program: String,
    /// The program's arguments, with quotes already removed.
    pub args: Vec<String>,
    /// True if this command runs through `sudo` or `doas`.
    pub elevated: bool,
}

/// Wrapper commands that just run whatever follows them, without changing
/// what it means to run it (beyond `sudo`/`doas`, which also elevate).
const TRANSPARENT_WRAPPERS: &[&str] = &["env", "command", "nohup", "time"];

/// Splits `line` into the simple commands it runs, in the order they'd run.
/// `;`, `&&`, `||`, `|`, and newlines each start a new one.
pub fn parse(line: &str, syntax: Syntax) -> Vec<SimpleCommand> {
    let word_groups = group_by_separator(tokenize::tokenize(line, syntax));

    word_groups
        .into_iter()
        .map(|words| strip_wrappers(words, syntax))
        .collect()
}

/// Groups words into one list per command, starting a new list at every
/// separator. Two separators in a row (`a ;; b`) never produce an empty
/// group in between.
fn group_by_separator(tokens: Vec<Token>) -> Vec<Vec<String>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();

    for token in tokens {
        match token {
            Token::Word(word) => current.push(word),
            Token::Separator if !current.is_empty() => {
                groups.push(std::mem::take(&mut current));
            }
            Token::Separator => {}
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }

    groups
}

/// Peels wrapper words (`sudo`, `env`, a bare `FOO=bar`, ...) off the front
/// of `words`, leaving the real program and its arguments.
fn strip_wrappers(words: Vec<String>, syntax: Syntax) -> SimpleCommand {
    let mut elevated = false;
    let mut words = words.into_iter().peekable();

    while let Some(word) = words.peek() {
        if word == "sudo" || word == "doas" {
            elevated = true;
        } else if TRANSPARENT_WRAPPERS.contains(&word.as_str()) {
            // just unwrap it, nothing to record
        } else if syntax == Syntax::Posix && looks_like_assignment(word) {
            // `FOO=bar rm -rf x`: a one-off environment variable, Posix-only
        } else {
            break;
        }
        words.next();
    }

    SimpleCommand {
        program: words.next().unwrap_or_default(),
        args: words.collect(),
        elevated,
    }
}

/// True for a shell-style variable assignment like `FOO=bar` or `_X=1`:
/// a name made of letters, digits, and underscores, not starting with a
/// digit, followed by `=`.
fn looks_like_assignment(word: &str) -> bool {
    let Some((name, _value)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_command_has_no_wrapper() {
        let commands = parse("rm -rf build", Syntax::Posix);
        assert_eq!(
            commands,
            [SimpleCommand { program: "rm".into(), args: vec!["-rf".into(), "build".into()], elevated: false }]
        );
    }

    #[test]
    fn quoted_target_with_a_space_is_one_argument() {
        let commands = parse(r#"rm -rf "my folder""#, Syntax::Posix);
        assert_eq!(commands[0].args, vec!["-rf", "my folder"]);
    }

    #[test]
    fn a_chain_produces_one_simple_command_per_piece() {
        let commands = parse("cd ~ && rm -rf *", Syntax::Posix);
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].program, "cd");
        assert_eq!(commands[1].program, "rm");
        assert_eq!(commands[1].args, vec!["-rf", "*"]);
    }

    #[test]
    fn semicolons_and_pipes_also_split() {
        for line in ["ls; rm -rf ~", "ls | rm -rf ~", "ls && rm -rf ~"] {
            let commands = parse(line, Syntax::Posix);
            assert!(commands.iter().any(|c| c.program == "rm"), "lost the rm in: {line}");
        }
    }

    #[test]
    fn repeated_separators_do_not_produce_empty_commands() {
        let commands = parse("ls ;; rm -rf ~", Syntax::Posix);
        assert_eq!(commands.len(), 2);
    }

    #[test]
    fn sudo_and_doas_elevate_and_are_removed() {
        let commands = parse("sudo rm -rf /", Syntax::Posix);
        assert_eq!(commands[0].program, "rm");
        assert!(commands[0].elevated);

        let commands = parse("doas rm -rf /", Syntax::Posix);
        assert_eq!(commands[0].program, "rm");
        assert!(commands[0].elevated);
    }

    #[test]
    fn stacked_sudo_still_elevates_once() {
        let commands = parse("sudo sudo rm -rf /", Syntax::Posix);
        assert_eq!(commands[0].program, "rm");
        assert!(commands[0].elevated);
    }

    #[test]
    fn transparent_wrappers_are_unwrapped_without_elevating() {
        for line in ["env rm -rf /", "command rm -rf /", "nohup rm -rf /", "time rm -rf /"] {
            let commands = parse(line, Syntax::Posix);
            assert_eq!(commands[0].program, "rm", "failed on: {line}");
            assert!(!commands[0].elevated, "wrongly elevated: {line}");
        }
    }

    #[test]
    fn env_style_assignments_are_skipped() {
        let commands = parse("FOO=bar rm -rf /", Syntax::Posix);
        assert_eq!(commands[0].program, "rm");

        let commands = parse("env FOO=bar BAZ=1 rm -rf /", Syntax::Posix);
        assert_eq!(commands[0].program, "rm");
        assert_eq!(commands[0].args, vec!["-rf", "/"]);
    }

    #[test]
    fn assignment_lookalikes_are_posix_only() {
        // PowerShell has no such syntax: "FOO=bar" there is just a plain word.
        let commands = parse("FOO=bar Remove-Item -Recurse x", Syntax::PowerShell);
        assert_eq!(commands[0].program, "FOO=bar");
    }

    #[test]
    fn wrapper_with_nothing_after_it_is_an_empty_program() {
        let commands = parse("sudo", Syntax::Posix);
        assert_eq!(commands, [SimpleCommand { program: "".into(), args: vec![], elevated: true }]);
    }

    #[test]
    fn blank_input_produces_no_commands() {
        assert_eq!(parse("", Syntax::Posix), []);
        assert_eq!(parse("   ", Syntax::Posix), []);
    }
}
