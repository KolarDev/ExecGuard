use std::path::PathBuf;
use execguard_core::Action;
use execguard_core::Shell;
use execguard_core::Context;
use execguard_core::Finding;
use execguard_core::Decision;

fn main() {
    println!("{}", Action::Warn < Action::Reject); // true
    let worst = [Action::Allow, Action::Confirm, Action::Warn]
        .into_iter()
        .max()
        .unwrap();
    println!("{:?}", worst); // Confirm

    for action in [Action::Allow, Action::Warn, Action::Confirm, Action::Reject] {
        println!("{:?} -> {}", action, action.exit_code());
    }

    let action = Action::Confirm;
    println!("{}", action);        // CONFIRM   (Display)
    println!("{:?}", action);      // Confirm   (Debug)
    let text: String = action.to_string();
    println!("execguard [{}] length={}", text, text.len());

    for name in ["bash", "Pwsh", "fish"] {
        match Shell::from_name(name) {
            Some(shell) => println!("{name} -> {:?}", shell),
            None => println!("{name} -> not a supported shell"),
        }
    }


    let dir = std::env::current_dir().unwrap_or_default();
    let ctx = Context {
        cwd: dir,
        home: std::env::var_os("HOME").map(PathBuf::from),
        shell: Shell::Unknown,
        is_elevated: false,
    };
    println!("{:#?}", ctx);
    println!("at root? {}  at home? {}", ctx.cwd_is_root(), ctx.cwd_is_home());

    let path = "/home/ada";
    let d = Decision::from_findings(vec![
        Finding::new("fs.recursive-delete", Action::Reject, format!("would wipe {}", path)),
        Finding::new("priv.elevated", Action::Confirm, "running as root"),
    ]);
    println!("verdict: {} (exit code {})", d.action(), d.action().exit_code());
    for f in d.findings() {
        println!("  {}", f);
    }

}