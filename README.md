# ExecGuard

A local-first command safety layer that sits between you and your shell (Bash,
Zsh, PowerShell). Before a typed command runs, a shell hook asks the
`execguard` binary for a verdict — allow, warn, confirm, or reject — and the
hook acts on it.

**Core philosophy:** a deterministic, local policy engine makes every safety
decision. No command ever leaves your machine, and no network call or AI
model is in the decision path. AI may help explain a verdict later, but it
never decides one.

## Why

It's easy to type a destructive command (`rm -rf ~`, `Remove-Item -Recurse
-Force C:\`) one character away from what you meant. ExecGuard catches the
dangerous ones before they run and asks you to confirm, instead of relying on
muscle memory and luck.

## How it works

1. A shell hook (one per shell) intercepts the command you just typed, before
   it executes.
2. It passes the command line to the `execguard` CLI.
3. A set of deterministic, built-in rules evaluate the command and return a
   verdict:

   | Verdict | Meaning                                   |
   |---------|--------------------------------------------|
   | ALLOW   | Run silently                                |
   | WARN    | Print a message, then run                   |
   | CONFIRM | Print a message, ask y/N, run only on `y`   |
   | REJECT  | Print a message, do not run                 |

4. The hook acts on the verdict. If ExecGuard itself fails for any reason, the
   hook fails open with a loud warning and runs the command anyway — a broken
   install should never lock you out of your own terminal.

## Project status

Early/active development. The policy engine, built-in `rm`/`Remove-Item`
recursive-delete rule, CLI, and Bash/PowerShell/Zsh hooks all work. Parsing is
still naive (whitespace-split), storage/history and more rules are not built
yet. See `dev-doc/HANDOFF.md` for the full, current state and what's left.

## Repository layout

```text
execguard/
├── crates/
│   ├── execguard-core/      # policy engine + built-in rules
│   ├── execguard-cli/       # `execguard` binary
│   ├── execguard-parser/    # command-line tokenizer/parser (scaffold)
│   └── execguard-storage/   # SQLite-backed history (scaffold)
└── shell/
    ├── execguard.ps1        # PowerShell hook
    ├── execguard.bash       # Bash hook
    └── execguard.zsh        # Zsh hook
```

## Setup (for collaborators)

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (stable toolchain)
- A supported shell: Bash, Zsh, or Windows PowerShell 5.1+

### Build

```bash
git clone <repo-url>
cd execguard
cargo build --release
```

This produces the `execguard` (or `execguard.exe` on Windows) binary at
`target/release/`.

### Run the tests

```bash
cargo test
```

### Load a shell hook

Each hook needs to be sourced in the shell session (add to your shell's
startup file to make it permanent).

```bash
source shell/execguard.bash      # Bash
source shell/execguard.zsh       # Zsh
```

```powershell
. .\shell\execguard.ps1          # PowerShell
```

The PowerShell hook prints `ExecGuard active (...)` once loaded.

### Try it

```bash
ls                          # silent, nothing happens
rm -rf ./nothing            # WARN, then runs
rm -r /                     # CONFIRM, press n to cancel
```

An escape hatch exists for every hook: set `EXECGUARD_DISABLE=1` to bypass
ExecGuard entirely.

## Contributing

Read `dev-doc/HANDOFF.md` before making changes — it documents the design
decisions (exit-code protocol, rule philosophy, fail-open behavior) and the
current priority list. Every rule or behavior change should ship with tests;
run `cargo test` before calling anything done.
