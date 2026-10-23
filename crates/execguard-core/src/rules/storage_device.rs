//! Catches commands that reformat or directly overwrite a storage device:
//! `dd` writing to a raw device node, any `mkfs*` filesystem creator,
//! PowerShell's `Format-Volume`, and Windows' `diskpart`.
//!
//! `diskpart` only warns, not confirms: running it doesn't commit to
//! anything by itself (it's an interactive tool, often used just to list
//! disks), unlike the other three, which mean exactly what they say the
//! moment they're invoked.

use super::program::{base_name, is_program};
use crate::{Action, Context, Finding, Rule, SimpleCommand};

pub struct DestroysStorageDevice;

impl Rule for DestroysStorageDevice {
    fn id(&self) -> &'static str {
        "fs.destroys-storage-device"
    }

    fn evaluate(&self, command: &SimpleCommand, _ctx: &Context) -> Option<Finding> {
        if is_program(&command.program, "dd") {
            if !dd_targets_a_device(&command.args) {
                return None;
            }
            return Some(Finding::new(
                self.id(),
                Action::Confirm,
                "writing straight to a device node with dd overwrites it; everything already on it is gone",
            ));
        }

        if base_name(&command.program).to_ascii_lowercase().starts_with("mkfs") {
            return Some(Finding::new(
                self.id(),
                Action::Confirm,
                "mkfs creates a new, empty filesystem; everything already on the target is gone",
            ));
        }

        if is_program(&command.program, "Format-Volume") {
            return Some(Finding::new(
                self.id(),
                Action::Confirm,
                "Format-Volume erases the volume's filesystem; everything on it is gone",
            ));
        }

        if is_program(&command.program, "diskpart") {
            return Some(Finding::new(
                self.id(),
                Action::Warn,
                "diskpart can repartition or format disks; double-check its commands before confirming anything inside it",
            ));
        }

        None
    }
}

/// `dd ... of=/dev/sda ...`: the output is a raw device node, not a file.
fn dd_targets_a_device(args: &[String]) -> bool {
    args.iter().any(|a| a.strip_prefix("of=").is_some_and(|value| value.starts_with("/dev/")))
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
        DestroysStorageDevice.evaluate(&simple, &ctx()).map(|f| f.action)
    }

    #[test]
    fn dd_writing_to_a_device_needs_confirmation() {
        assert_eq!(check("dd if=/dev/zero of=/dev/sda bs=1M"), Some(Action::Confirm));
        assert_eq!(check("dd if=image.img of=/dev/sdb1"), Some(Action::Confirm));
    }

    #[test]
    fn dd_between_ordinary_files_is_quiet() {
        assert_eq!(check("dd if=file.img of=backup.img"), None);
        assert_eq!(check("dd if=/dev/sda of=backup.img"), None); // reading FROM a device is fine
        assert_eq!(check("dd --help"), None);
    }

    #[test]
    fn any_mkfs_variant_needs_confirmation() {
        for cmd in ["mkfs -t ext4 /dev/sdb1", "mkfs.ext4 /dev/sdb1", "mkfs.xfs /dev/sdb1"] {
            assert_eq!(check(cmd), Some(Action::Confirm), "missed: {cmd}");
        }
    }

    #[test]
    fn format_volume_needs_confirmation() {
        assert_eq!(check("Format-Volume -DriveLetter D"), Some(Action::Confirm));
        assert_eq!(check("format-volume -driveletter d"), Some(Action::Confirm)); // case-insensitive
    }

    #[test]
    fn diskpart_only_warns() {
        assert_eq!(check("diskpart"), Some(Action::Warn));
        assert_eq!(check("diskpart /s script.txt"), Some(Action::Warn));
    }

    #[test]
    fn unrelated_programs_are_ignored() {
        assert_eq!(check("ls -la"), None);
        assert_eq!(check("cat notes.txt"), None);
    }
}
