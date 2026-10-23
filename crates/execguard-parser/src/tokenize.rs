//! Turns one line of shell input into words and separators.
//!
//! This step only understands quoting and punctuation. It does not know
//! what `sudo` or `;` *mean* for splitting commands apart — that happens
//! one layer up, in `lib.rs`.

use crate::Syntax;

/// One piece of a tokenized line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Token {
    /// A word, with quotes already removed. `rm -rf "my folder"` produces
    /// the words `rm`, `-rf`, `my folder`.
    Word(String),
    /// A point where one command ends and another begins: `;`, `&&`,
    /// `||`, `|`, or a newline.
    Separator,
}

pub(crate) fn tokenize(line: &str, syntax: Syntax) -> Vec<Token> {
    match syntax {
        Syntax::Posix => tokenize_posix(line),
        Syntax::PowerShell => tokenize_powershell(line),
    }
}

/// Bash/Zsh-style tokenizing: `\` escapes the next character, `'...'` is
/// fully literal, `"..."` is literal except `\` can still escape `"`, `\`,
/// `$`, and `` ` `` inside it.
fn tokenize_posix(line: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => end_word(&mut tokens, &mut word, &mut in_word),

            '\n' | ';' => {
                end_word(&mut tokens, &mut word, &mut in_word);
                tokens.push(Token::Separator);
            }

            '|' | '&' => {
                end_word(&mut tokens, &mut word, &mut in_word);
                if chars.peek() == Some(&c) {
                    chars.next(); // the second half of `||` or `&&`
                }
                tokens.push(Token::Separator);
            }

            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }

            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if matches!(chars.peek(), Some('"' | '\\' | '$' | '`')) => {
                            word.push(chars.next().unwrap());
                        }
                        other => word.push(other),
                    }
                }
            }

            '\\' => {
                in_word = true;
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }

            other => {
                in_word = true;
                word.push(other);
            }
        }
    }
    end_word(&mut tokens, &mut word, &mut in_word);

    tokens
}

/// PowerShell-style tokenizing: `` ` `` is the escape character, and a
/// doubled quote (`''` or `""`) inside a string of the same kind means one
/// literal quote character, not the end of the string.
///
/// A lone `&` (PowerShell's call operator) ends the current word but is not
/// a separator — unlike Bash, it is not used to chain commands.
fn tokenize_powershell(line: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => end_word(&mut tokens, &mut word, &mut in_word),

            '\n' | ';' => {
                end_word(&mut tokens, &mut word, &mut in_word);
                tokens.push(Token::Separator);
            }

            '|' => {
                end_word(&mut tokens, &mut word, &mut in_word);
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                tokens.push(Token::Separator);
            }

            '&' => {
                end_word(&mut tokens, &mut word, &mut in_word);
                if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Separator);
                }
                // A lone `&` is dropped: it is not a separator in PowerShell.
            }

            '\'' => {
                in_word = true;
                read_quoted(&mut chars, &mut word, '\'');
            }

            '"' => {
                in_word = true;
                read_quoted_with_escapes(&mut chars, &mut word, '"');
            }

            '`' => {
                in_word = true;
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }

            other => {
                in_word = true;
                word.push(other);
            }
        }
    }
    end_word(&mut tokens, &mut word, &mut in_word);

    tokens
}

/// Reads the inside of a `'...'` string: `quote` doubled (`''`) is one
/// literal quote and keeps the string going; a single `quote` ends it.
fn read_quoted(chars: &mut std::iter::Peekable<std::str::Chars>, word: &mut String, quote: char) {
    while let Some(c) = chars.next() {
        if c == quote {
            if chars.peek() == Some(&quote) {
                chars.next();
                word.push(quote);
            } else {
                break;
            }
        } else {
            word.push(c);
        }
    }
}

/// Like `read_quoted`, but also lets `` ` `` escape the next character —
/// used for `"..."` strings, where PowerShell still allows backtick escapes.
fn read_quoted_with_escapes(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    word: &mut String,
    quote: char,
) {
    while let Some(c) = chars.next() {
        if c == quote {
            if chars.peek() == Some(&quote) {
                chars.next();
                word.push(quote);
            } else {
                break;
            }
        } else if c == '`' {
            if let Some(next) = chars.next() {
                word.push(next);
            }
        } else {
            word.push(c);
        }
    }
}

/// Pushes the word built up so far as a token, if there is one.
fn end_word(tokens: &mut Vec<Token>, word: &mut String, in_word: &mut bool) {
    if *in_word {
        tokens.push(Token::Word(std::mem::take(word)));
        *in_word = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(tokens: &[Token]) -> Vec<&str> {
        tokens
            .iter()
            .filter_map(|t| match t {
                Token::Word(w) => Some(w.as_str()),
                Token::Separator => None,
            })
            .collect()
    }

    #[test]
    fn posix_splits_on_whitespace() {
        let tokens = tokenize_posix("rm -rf build");
        assert_eq!(words(&tokens), ["rm", "-rf", "build"]);
    }

    #[test]
    fn posix_single_quotes_keep_spaces_and_are_fully_literal() {
        let tokens = tokenize_posix(r#"rm -rf 'my folder' '$HOME'"#);
        assert_eq!(words(&tokens), ["rm", "-rf", "my folder", "$HOME"]);
    }

    #[test]
    fn posix_double_quotes_keep_spaces() {
        let tokens = tokenize_posix(r#"rm -rf "my folder""#);
        assert_eq!(words(&tokens), ["rm", "-rf", "my folder"]);
    }

    #[test]
    fn posix_double_quotes_allow_escaped_quote() {
        let tokens = tokenize_posix(r#"echo "say \"hi\"""#);
        assert_eq!(words(&tokens), ["echo", r#"say "hi""#]);
    }

    #[test]
    fn posix_backslash_escapes_one_character() {
        let tokens = tokenize_posix(r"rm -rf my\ file");
        assert_eq!(words(&tokens), ["rm", "-rf", "my file"]);
    }

    #[test]
    fn posix_quotes_can_sit_inside_a_word() {
        let tokens = tokenize_posix(r#"echo a"b c"d"#);
        assert_eq!(words(&tokens), ["echo", "ab cd"]);
    }

    #[test]
    fn posix_separators_are_recognised() {
        let tokens = tokenize_posix("cd ~ && rm -rf *");
        assert_eq!(
            tokens,
            vec![
                Token::Word("cd".into()),
                Token::Word("~".into()),
                Token::Separator,
                Token::Word("rm".into()),
                Token::Word("-rf".into()),
                Token::Word("*".into()),
            ]
        );
    }

    #[test]
    fn posix_semicolon_pipe_and_background_all_separate() {
        for line in ["ls; rm -rf ~", "ls | rm -rf ~", "ls & rm -rf ~", "ls || rm -rf ~"] {
            let tokens = tokenize_posix(line);
            assert!(tokens.contains(&Token::Separator), "no separator found in: {line}");
            assert!(words(&tokens).contains(&"rm"), "lost the rm in: {line}");
        }
    }

    #[test]
    fn powershell_splits_on_whitespace() {
        let tokens = tokenize_powershell("Remove-Item -Recurse build");
        assert_eq!(words(&tokens), ["Remove-Item", "-Recurse", "build"]);
    }

    #[test]
    fn powershell_single_quotes_are_literal_with_doubling() {
        let tokens = tokenize_powershell("Remove-Item 'it''s a folder'");
        assert_eq!(words(&tokens), ["Remove-Item", "it's a folder"]);
    }

    #[test]
    fn powershell_double_quotes_allow_doubling_and_backtick_escapes() {
        let tokens = tokenize_powershell(r#"Remove-Item "say ""hi"" `$x""#);
        assert_eq!(words(&tokens), ["Remove-Item", r#"say "hi" $x"#]);
    }

    #[test]
    fn powershell_double_ampersand_separates() {
        let tokens = tokenize_powershell("cd ~ && Remove-Item -Recurse *");
        assert!(tokens.contains(&Token::Separator));
        assert!(words(&tokens).contains(&"Remove-Item"));
    }

    #[test]
    fn powershell_lone_ampersand_is_not_a_separator() {
        let tokens = tokenize_powershell("& C:\\tools\\run.exe");
        assert!(!tokens.contains(&Token::Separator));
        assert_eq!(words(&tokens), [r"C:\tools\run.exe"]);
    }
}
