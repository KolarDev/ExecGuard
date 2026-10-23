//! Built-in safety rules.
//!
//! Each rule gets its own file, named after what it watches for. `targets`
//! and `program` hold logic more than one rule needs (recognising a
//! dangerous path, recognising a program under any spelling of its name).

mod database;
mod permissions;
mod program;
mod recursive_delete;
mod storage_device;
mod targets;
mod vcs;

pub use database::DestructiveStatement;
pub use permissions::PermissionBomb;
pub use recursive_delete::RecursiveDelete;
pub use storage_device::DestroysStorageDevice;
pub use vcs::HistoryRewrite;

use crate::Rule;

/// Every built-in rule, ready to hand to the engine.
pub fn builtin() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(RecursiveDelete),
        Box::new(HistoryRewrite),
        Box::new(DestructiveStatement),
        Box::new(PermissionBomb),
        Box::new(DestroysStorageDevice),
    ]
}

#[cfg(test)]
mod tests {
    use crate::{Action, Context, Engine, Shell};
    use std::path::PathBuf;

    fn ctx() -> Context {
        Context {
            cwd: PathBuf::from("/home/ada/project"),
            home: Some(PathBuf::from("/home/ada")),
            shell: Shell::Bash,
            is_elevated: false,
        }
    }

    /// The same promise `recursive_delete` tests on its own: no built-in
    /// rule, from any file, is ever allowed to REJECT (see
    /// dev-doc/HANDOFF.md 4.2). REJECT is reserved for the user's own
    /// blacklist.
    #[test]
    fn no_builtin_rule_ever_rejects() {
        let engine = Engine::with_builtin_rules();
        let mut c = ctx();
        c.is_elevated = true;

        let risky_commands = [
            "rm -rf /",
            "git push --force origin main",
            "git reset --hard",
            "git clean -fdx",
            r#"psql -c "DROP DATABASE prod""#,
            "chmod -R 777 /",
            "chown -R root:root /",
            "dd if=/dev/zero of=/dev/sda",
            "mkfs.ext4 /dev/sdb1",
            "diskpart",
        ];
        for cmd in risky_commands {
            let action = engine.evaluate(cmd, &c).action();
            assert_ne!(action, Action::Reject, "a built-in rule rejected: {cmd}");
        }
    }
}
