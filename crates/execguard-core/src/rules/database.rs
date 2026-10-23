//! Catches destructive SQL passed straight to a database client's command
//! line: `DROP DATABASE`, `DROP TABLE`, or `TRUNCATE`.
//!
//! This only looks for the client program by name, deliberately -- a bare
//! text search for these words would also flag `grep -r TRUNCATE` or
//! `cat migration.sql`, which don't run anything themselves.

use super::program::is_program;
use crate::{Action, Context, Finding, Rule, SimpleCommand};

const CLIENTS: &[&str] = &["psql", "mysql", "mariadb", "sqlite3", "sqlcmd", "redis-cli", "mongosh", "mongo"];

pub struct DestructiveStatement;

impl Rule for DestructiveStatement {
    fn id(&self) -> &'static str {
        "db.destructive-statement"
    }

    fn evaluate(&self, command: &SimpleCommand, _ctx: &Context) -> Option<Finding> {
        if !CLIENTS.iter().any(|&client| is_program(&command.program, client)) {
            return None;
        }

        let statement = destructive_statement_in(&command.args)?;
        Some(Finding::new(
            self.id(),
            Action::Confirm,
            format!("this command runs a {statement} statement; once it commits, the data is gone"),
        ))
    }
}

/// Looks for `TRUNCATE`, `DROP DATABASE`, or `DROP TABLE` as whole words
/// (so `TRUNCATED`, or a table literally named `DROP_DATABASE_LOG`, don't
/// match), anywhere across `args` joined back into one line of text.
fn destructive_statement_in(args: &[String]) -> Option<&'static str> {
    let text = args.join(" ").to_ascii_uppercase();
    let words: Vec<&str> = text.split_whitespace().collect();

    for (i, &word) in words.iter().enumerate() {
        match word {
            "TRUNCATE" => return Some("TRUNCATE"),
            "DROP" if words.get(i + 1) == Some(&"DATABASE") => return Some("DROP DATABASE"),
            "DROP" if words.get(i + 1) == Some(&"TABLE") => return Some("DROP TABLE"),
            _ => {}
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx() -> Context {
        Context { cwd: PathBuf::from("/home/ada"), home: None, shell: crate::Shell::Bash, is_elevated: false }
    }

    fn check(cmd: &str) -> Option<Action> {
        let simple = execguard_parser::parse(cmd, execguard_parser::Syntax::Posix).into_iter().next().unwrap();
        DestructiveStatement.evaluate(&simple, &ctx()).map(|f| f.action)
    }

    #[test]
    fn drop_database_via_a_known_client_needs_confirmation() {
        assert_eq!(check(r#"psql -c "DROP DATABASE prod""#), Some(Action::Confirm));
        assert_eq!(check(r#"psql -c "drop database prod""#), Some(Action::Confirm)); // case-insensitive
    }

    #[test]
    fn drop_table_and_truncate_are_also_caught() {
        assert_eq!(check(r#"mysql -e "DROP TABLE users""#), Some(Action::Confirm));
        assert_eq!(check(r#"sqlite3 app.db "TRUNCATE sessions""#), Some(Action::Confirm));
    }

    #[test]
    fn safe_statements_are_ignored() {
        assert_eq!(check(r#"psql -c "SELECT * FROM users""#), None);
        assert_eq!(check("psql --version"), None);
    }

    #[test]
    fn the_same_text_from_a_non_client_program_is_ignored() {
        assert_eq!(check(r#"echo "DROP DATABASE prod""#), None);
        assert_eq!(check("grep -r TRUNCATE ."), None);
    }

    #[test]
    fn a_word_that_merely_contains_the_keyword_is_not_a_match() {
        assert_eq!(check(r#"mysql -e "SELECT * FROM DROP_DATABASE_LOG""#), None);
        assert_eq!(check(r#"mysql -e "the data was already truncated""#), None);
    }
}
