//! Recognising a program by name, regardless of what directory it was
//! called through or whether a Windows `.exe` extension was spelled out.
//!
//! `recursive_delete`'s own program-name handling is deliberately separate
//! from this: it treats a leading `\` as an escaped command name (`\rm`),
//! not a path separator, which would be the wrong call for every other rule.

/// The bare program name: strips any directory prefix (`/` or `\`) and,
/// if present, a trailing `.exe` (case-insensitive).
pub(super) fn base_name(program: &str) -> &str {
    let base = program.rsplit(['/', '\\']).next().unwrap_or(program);
    base.strip_suffix(".exe").or_else(|| base.strip_suffix(".EXE")).unwrap_or(base)
}

/// Is `program` the tool named `name`, under any spelling `base_name`
/// normalizes away? Comparison is case-insensitive.
pub(super) fn is_program(program: &str, name: &str) -> bool {
    base_name(program).eq_ignore_ascii_case(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_directories_and_exe() {
        assert_eq!(base_name("git"), "git");
        assert_eq!(base_name("/usr/bin/git"), "git");
        assert_eq!(base_name(r"C:\Program Files\Git\bin\git.exe"), "git");
    }

    #[test]
    fn comparison_is_case_insensitive() {
        assert!(is_program("GIT", "git"));
        assert!(is_program(r"C:\tools\Format-Volume.exe", "format-volume"));
        assert!(!is_program("github", "git"));
    }
}
