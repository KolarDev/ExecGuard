//! Best-effort secret redaction.
//!
//! Before a command is written to the history database, we scan its text
//! for a handful of well-known "this looks like a secret" shapes and
//! replace just the sensitive part with `***`. This is deliberately not
//! exhaustive — there is no way to catch every possible secret format from
//! text alone — it only catches the common, recognisable ones:
//!
//!   - `--password`/`--token`/`--api-key`/`--secret` flags, however they're
//!     spelled with `=` or a space, and whether the value is quoted.
//!   - An `Authorization: Bearer ...` (or `Basic`/`Token`) header value.
//!   - A password embedded in a URL (`scheme://user:PASSWORD@host`).
//!   - A few bare token shapes (AWS access key IDs, GitHub tokens, OpenAI
//!     API keys) that are recognisable even with no flag name next to them.
//!
//! The `regex` crate is used deliberately: unlike a typical backtracking
//! engine, it guarantees linear-time matching, so running it over an
//! arbitrary, user-typed command line can never hang.

use regex::Regex;
use std::sync::OnceLock;

/// Returns `text` with recognisable secrets replaced by `***`.
pub(crate) fn redact(text: &str) -> String {
    let mut redacted = text.to_string();
    for (pattern, replacement) in patterns() {
        redacted = pattern.replace_all(&redacted, *replacement).into_owned();
    }
    redacted
}

/// Each pattern paired with what to replace a match with. Capture groups
/// (`$1`, `$2`, ...) let a replacement keep the parts of the match that
/// aren't the secret itself, e.g. the flag name or the surrounding `@`.
fn patterns() -> &'static [(Regex, &'static str)] {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            // --password=secret, --password secret, --token "secret value"
            //
            // The bare (unquoted) alternative stops at a quote character,
            // not just whitespace, so it doesn't swallow the closing `"`
            // of an outer shell-quoted string like `-H "Authorization: ..."`.
            (
                Regex::new(
                    r#"(?i)(--password|--passwd|--token|--api-key|--apikey|--secret)([= ]+)("[^"]*"|'[^']*'|[^\s"']+)"#,
                )
                .unwrap(),
                "$1$2***",
            ),
            // curl -H "Authorization: Bearer <token>" (or Basic/Token)
            (
                Regex::new(r#"(?i)(Authorization:\s*(?:Bearer|Basic|Token)\s+)[^\s"']+"#).unwrap(),
                "$1***",
            ),
            // https://user:PASSWORD@host/path
            (Regex::new(r#"(://[^:/\s]+:)[^@\s]+(@)"#).unwrap(), "$1***$2"),
            // AWS access key IDs always start with AKIA and are 20 chars long.
            (Regex::new(r#"\bAKIA[0-9A-Z]{16}\b"#).unwrap(), "***"),
            // GitHub personal access tokens.
            (Regex::new(r#"\bghp_[A-Za-z0-9]{36}\b"#).unwrap(), "***"),
            // OpenAI-style API keys.
            (Regex::new(r#"\bsk-[A-Za-z0-9]{20,}\b"#).unwrap(), "***"),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_long_flag_with_equals() {
        assert_eq!(redact("mysql --password=secret123 -h localhost"), "mysql --password=*** -h localhost");
    }

    #[test]
    fn redacts_long_flag_with_space() {
        assert_eq!(redact("curl --token abc.def.ghi api.example.com"), "curl --token *** api.example.com");
    }

    #[test]
    fn redacts_quoted_value() {
        assert_eq!(redact(r#"tool --api-key "a b c""#), "tool --api-key ***");
    }

    #[test]
    fn redacts_authorization_header() {
        let cmd = r#"curl -H "Authorization: Bearer sk-abcdefghijklmnopqrstuvwxyz""#;
        assert_eq!(redact(cmd), r#"curl -H "Authorization: Bearer ***""#);
    }

    #[test]
    fn redacts_url_credentials() {
        assert_eq!(
            redact("git clone https://user:hunter2@github.com/org/repo.git"),
            "git clone https://user:***@github.com/org/repo.git"
        );
    }

    #[test]
    fn redacts_bare_aws_key() {
        assert_eq!(redact("export KEY=AKIAABCDEFGHIJKLMNOP"), "export KEY=***");
    }

    #[test]
    fn redacts_bare_github_token() {
        let token = "ghp_".to_string() + &"a".repeat(36);
        assert_eq!(redact(&format!("echo {token}")), "echo ***");
    }

    #[test]
    fn leaves_ordinary_commands_untouched() {
        for cmd in ["rm -rf /tmp/build", "git push origin main", "ls -la ~/project"] {
            assert_eq!(redact(cmd), cmd);
        }
    }
}
