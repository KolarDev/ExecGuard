//! Shared logic for recognising a filesystem root or the user's home
//! directory, however a Unix shell or PowerShell spells it. Any rule that
//! judges whether a path argument is dangerous can reuse this.

use crate::Context;
use std::path::Path;

/// Which command-line language a path argument is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Dialect {
    Unix,
    PowerShell,
}

/// Is this target a filesystem root or the user's home directory?
/// `target` has already had its quotes removed by the parser.
pub(super) fn is_dangerous_target(target: &str, dialect: Dialect, ctx: &Context) -> bool {
    // PowerShell paths are case-insensitive and accept `\` or `/`.
    // Convert them to one canonical form so a single table covers every spelling.
    let normalized = match dialect {
        Dialect::Unix => target.to_string(),
        Dialect::PowerShell => target.to_ascii_lowercase().replace('\\', "/"),
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

    #[test]
    fn root_and_home_spellings_are_dangerous() {
        for target in ["/", "//", "/*", "~", "~/", "~/*", "$HOME", "${HOME}", "/home/ada"] {
            assert!(is_dangerous_target(target, Dialect::Unix, &ctx()), "missed: {target}");
        }
    }

    #[test]
    fn project_subfolders_are_not_dangerous() {
        assert!(!is_dangerous_target("build", Dialect::Unix, &ctx()));
        assert!(!is_dangerous_target("~/project/build", Dialect::Unix, &ctx()));
    }
}
