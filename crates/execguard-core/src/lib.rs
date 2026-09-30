use std::fmt;
use std::path::PathBuf;

/// The action ExecGuard will take when a command is run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    Allow,
    Warn,
    Confirm,
    Reject,
}

impl Action {
    /// The exit code the CLI returns for this action.
    /// The shell hook reads this number to decide what to do.
    pub fn exit_code(self) -> u8 {
        match self {
            Action::Allow => 0,
            Action::Warn => 10,
            Action::Confirm => 20,
            Action::Reject => 30,
        }
    }

    /// The name shown to users, e.g. "REJECT".
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Allow => "ALLOW",
            Action::Warn => "WARN",
            Action::Confirm => "CONFIRM",
            Action::Reject => "REJECT",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}


/// The shell a command will run in.
/// The same text can mean different things in different shells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    PowerShell,
    Unknown,
}

impl Shell {
    /// Turns a name like "bash" or "pwsh" into a `Shell`.
    /// Returns `None` if the name isn't recognised.
    pub fn from_name(name: &str) -> Option<Shell> {
        match name.to_ascii_lowercase().as_str() {
            "bash" => Some(Shell::Bash),
            "zsh" => Some(Shell::Zsh),
            "powershell" | "pwsh" => Some(Shell::PowerShell),
            _ => None,
        }
    }
}


/// Everything ExecGuard knows about where and how a command will run.
#[derive(Debug, Clone)]
pub struct Context {
    /// The directory the command runs in.
    pub cwd: PathBuf,
    /// The user's home directory, if it could be found.
    pub home: Option<PathBuf>,
    /// The shell that will run the command.
    pub shell: Shell,
    /// Running as root (Unix) or Administrator (Windows).
    pub is_elevated: bool,
}

impl Context {
    /// True if the current directory is a filesystem root, like `/` or `C:\`.
    /// An empty (unknown) directory also counts, to stay on the safe side.
    pub fn cwd_is_root(&self) -> bool {
        self.cwd.parent().is_none()
    }

    /// True if the current directory is the user's home directory.
    pub fn cwd_is_home(&self) -> bool {
        self.home.as_deref() == Some(self.cwd.as_path())
    }
}

/// The result of running a command through ExecGuard: an action plus the reasons for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Stable ID of the rule, e.g. "fs.recursive-delete".
    pub rule_id: &'static str,
    /// What this rule wants to happen.
    pub action: Action,
    /// Human-readable explanation shown to the user.
    pub message: String,
}

impl Finding {
    pub fn new(rule_id: &'static str, action: Action, message: impl Into<String>) -> Self {
        Finding { rule_id, action, message: message.into() }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {} ({})", self.action, self.message, self.rule_id)
    }
}

/// The final verdict for one command: an action plus every reason behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    action: Action,
    findings: Vec<Finding>,
}

impl Decision {
    /// Combines findings from many rules. The most severe action wins;
    /// no findings at all means Allow.
    pub fn from_findings(findings: Vec<Finding>) -> Self {
        let action = findings
            .iter()
            .map(|f| f.action)
            .max()
            .unwrap_or(Action::Allow);
        Decision { action, findings }
    }

    pub fn action(&self) -> Action {
        self.action
    }

    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }
}


/// A safety rule: looks at a command and its context, and may object.
///
/// Returning `None` means "no opinion": as far as this rule is
/// concerned, the command is fine.
pub trait Rule: Send + Sync {
    /// Stable ID, e.g. "fs.recursive-delete". Must never change once released.
    fn id(&self) -> &'static str;

    /// Examine a command. Return a `Finding` only if this rule objects.
    fn evaluate(&self, command: &str, ctx: &Context) -> Option<Finding>;
}

/// An engine that runs rules against commands and combines their findings.
pub struct Engine {
    rules: Vec<Box<dyn Rule>>,
}

impl Engine {
    pub fn new(rules: Vec<Box<dyn Rule>>) -> Self {
        Engine { rules }
    }

    /// Evaluate a command against every rule.
    pub fn evaluate(&self, command: &str, ctx: &Context) -> Decision {
        let findings = self
            .rules
            .iter()
            .filter_map(|rule| rule.evaluate(command, ctx))
            .collect();
        Decision::from_findings(findings)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Action; 4] = [Action::Allow, Action::Warn, Action::Confirm, Action::Reject];

    #[test]
    fn severity_order_is_allow_warn_confirm_reject() {
        assert!(Action::Allow < Action::Warn);
        assert!(Action::Warn < Action::Confirm);
        assert!(Action::Confirm < Action::Reject);
    }

    #[test]
    fn max_picks_the_most_severe_action() {
        let worst = [Action::Warn, Action::Reject, Action::Allow].into_iter().max();
        assert_eq!(worst, Some(Action::Reject));
    }

    #[test]
    fn exit_codes_match_the_protocol() {
        assert_eq!(Action::Allow.exit_code(), 0);
        assert_eq!(Action::Warn.exit_code(), 10);
        assert_eq!(Action::Confirm.exit_code(), 20);
        assert_eq!(Action::Reject.exit_code(), 30);
    }

    #[test]
    fn exit_codes_avoid_reserved_values() {
        let reserved = [1, 2, 101, 126, 127];
        for action in ALL {
            let code = action.exit_code();
            assert!(!reserved.contains(&code), "{action} uses reserved exit code {code}");
            assert!(code < 128, "{action} uses {code}, which looks like a signal exit");
        }
    }

    #[test]
    fn exit_codes_are_unique() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.exit_code(), b.exit_code(), "{a} and {b} share an exit code");
            }
        }
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(Action::Reject.to_string(), "REJECT");
        for action in ALL {
            assert_eq!(action.to_string(), action.as_str());
        }
    }

    /// Testing for the shell enum
    #[test]
    fn shell_names_are_case_insensitive() {
        assert_eq!(Shell::from_name("bash"), Some(Shell::Bash));
        assert_eq!(Shell::from_name("ZSH"), Some(Shell::Zsh));
        assert_eq!(Shell::from_name("PowerShell"), Some(Shell::PowerShell));
        assert_eq!(Shell::from_name("pwsh"), Some(Shell::PowerShell));
    }

    #[test]
    fn unrecognised_shell_names_are_rejected() {
        assert_eq!(Shell::from_name("bsh"), None);
        assert_eq!(Shell::from_name("cmd"), None);
        assert_eq!(Shell::from_name(""), None);
        assert_eq!(Shell::from_name("unknown"), None);
    }

    /// Tests for the context struct
        fn ctx_in(cwd: &str) -> Context {
        Context {
            cwd: PathBuf::from(cwd),
            home: Some(PathBuf::from("/home/ada")),
            shell: Shell::Bash,
            is_elevated: false,
        }
    }

    #[test]
    fn root_is_detected() {
        assert!(ctx_in("/").cwd_is_root());
        assert!(!ctx_in("/home/ada").cwd_is_root());
    }

    #[test]
    fn unknown_cwd_counts_as_root() {
        assert!(ctx_in("").cwd_is_root());
    }

    #[test]
    fn home_is_detected_even_with_trailing_slash() {
        assert!(ctx_in("/home/ada").cwd_is_home());
        assert!(ctx_in("/home/ada/").cwd_is_home());
        assert!(!ctx_in("/home/ada/project").cwd_is_home());
    }

    #[test]
    fn missing_home_never_matches() {
        let mut ctx = ctx_in("/home/ada");
        ctx.home = None;
        assert!(!ctx.cwd_is_home());
    }

    /// Tests for the decision struct
    #[test]
    fn no_findings_means_allow() {
        let d = Decision::from_findings(vec![]);
        assert_eq!(d.action(), Action::Allow);
        assert!(d.findings().is_empty());
    }

    #[test]
    fn most_severe_finding_decides() {
        let d = Decision::from_findings(vec![
            Finding::new("a", Action::Warn, "a warning"),
            Finding::new("b", Action::Reject, "a rejection"),
            Finding::new("c", Action::Confirm, "a confirmation"),
        ]);
        assert_eq!(d.action(), Action::Reject);
        assert_eq!(d.findings().len(), 3);
    }

    #[test]
    fn finding_display_shows_action_message_and_rule() {
        let f = Finding::new("fs.recursive-delete", Action::Warn, "forced recursive delete");
        assert_eq!(f.to_string(), "[WARN] forced recursive delete (fs.recursive-delete)");
    }

    /// A simple rule for testing the Rule trait.
    struct DropDatabase;

    impl Rule for DropDatabase {
        fn id(&self) -> &'static str {
            "test.drop-database"
        }

        fn evaluate(&self, command: &str, _ctx: &Context) -> Option<Finding> {
            if command.to_ascii_uppercase().contains("DROP DATABASE") {
                Some(Finding::new(self.id(), Action::Reject, "dropping a database"))
            } else {
                None
            }
        }
    }

    #[test]
    fn a_rule_objects_to_matching_commands() {
        let finding = DropDatabase.evaluate("psql -c 'drop database prod'", &ctx_in("/home/ada"));
        let finding = finding.expect("rule should have objected");
        assert_eq!(finding.action, Action::Reject);
        assert_eq!(finding.rule_id, "test.drop-database");
    }

    #[test]
    fn a_rule_ignores_other_commands() {
        assert_eq!(DropDatabase.evaluate("ls -la", &ctx_in("/home/ada")), None);
    }

    /// A simple engine for testing the Engine struct.
    struct ElevatedShell;

    impl Rule for ElevatedShell {
        fn id(&self) -> &'static str {
            "test.elevated"
        }

        fn evaluate(&self, _command: &str, ctx: &Context) -> Option<Finding> {
            if ctx.is_elevated {
                Some(Finding::new(self.id(), Action::Confirm, "running with elevated privileges"))
            } else {
                None
            }
        }
    }

    fn test_engine() -> Engine {
        Engine::new(vec![Box::new(DropDatabase), Box::new(ElevatedShell)])
    }

    #[test]
    fn engine_with_no_rules_allows_everything() {
        let engine = Engine::new(vec![]);
        let d = engine.evaluate("rm -rf /", &ctx_in("/"));
        assert_eq!(d.action(), Action::Allow);
    }

    #[test]
    fn engine_allows_when_no_rule_objects() {
        let d = test_engine().evaluate("ls -la", &ctx_in("/home/ada"));
        assert_eq!(d.action(), Action::Allow);
        assert!(d.findings().is_empty());
    }

    #[test]
    fn engine_combines_findings_from_different_rules() {
        let mut ctx = ctx_in("/home/ada");
        ctx.is_elevated = true;

        let d = test_engine().evaluate("psql -c 'DROP DATABASE prod'", &ctx);

        assert_eq!(d.action(), Action::Reject);
        let ids: Vec<&str> = d.findings().iter().map(|f| f.rule_id).collect();
        assert_eq!(ids, ["test.drop-database", "test.elevated"]);
    }
}