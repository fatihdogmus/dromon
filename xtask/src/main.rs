//! Dromon's developer commands, run as `cargo <command>` through the aliases in
//! `.cargo/config.toml` (`cargo xtask <command>` works too). Each one runs a script under
//! `scripts/` or a cargo command from the repository root, passing the remaining arguments on.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

/// What a command runs.
enum Run {
    /// `bash scripts/<file>` with the arguments.
    Script(&'static str),
    /// `cargo <args…>` with the arguments after them; `mock` also sets `DROMON_MOCK=1`.
    Cargo { args: &'static [&'static str], mock: bool },
}

/// (name, what it runs, usage, what it does).
const COMMANDS: &[(&str, Run, &str, &str)] = &[
    ("dev", Run::Script("dev.sh"), "[--mock] [-- APP ARGS]", "live editing: view and theme edits patch into the running app"),
    ("app", Run::Cargo { args: &["run", "-p", "dromon-app", "--"], mock: false }, "[APP ARGS]", "the app, debug build, on your ssh config"),
    ("demo", Run::Cargo { args: &["run", "-p", "dromon-app", "--"], mock: true }, "[APP ARGS]", "the app on the built-in demo: fake hosts and folders, no servers"),
    ("lint", Run::Cargo { args: &["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"], mock: false }, "", "clippy as CI runs it"),
    ("goldens", Run::Script("goldens.sh"), "[--check]", "re-render the UI goldens in tests/goldens (--check: compare, as CI)"),
    ("flows", Run::Script("flows.sh"), "[NAME…]", "render tests/flows/* to strips and trees in target/flows"),
    ("smoke", Run::Script("smoke-app.sh"), "[--window] [--connect]", "smoke-test the dromon binary"),
    ("repro", Run::Script("repro.sh"), "<ISSUE|issue.md>", "replay a bug report headless into target/repro"),
    ("perf", Run::Script("perf.sh"), "[--gate …|all]", "the performance gates, as one table"),
    ("accents", Run::Script("accents.sh"), "", "render the accent candidates side by side in target/accents"),
    ("package-linux", Run::Script("package-linux.sh"), "[--arch …] [--verify]", "build the release binary and the Linux packages"),
];

fn help() {
    println!("Dromon's developer commands: cargo <command> [args]\n");
    for (name, _, usage, what) in COMMANDS {
        println!("  cargo {:<40} {what}", format!("{name} {usage}"));
    }
    println!("\nEach script under scripts/ explains its own options (cargo <command> --help for most).");
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(name) = args.next().filter(|n| n != "help" && n != "--help" && n != "-h") else {
        help();
        return ExitCode::SUCCESS;
    };
    let Some((_, run, _, _)) = COMMANDS.iter().find(|(n, ..)| *n == name) else {
        eprintln!("xtask: no command `{name}`\n");
        help();
        return ExitCode::from(2);
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut command = match run {
        Run::Script(file) => {
            let mut c = Command::new("bash");
            c.arg(root.join("scripts").join(file));
            c
        }
        Run::Cargo { args, mock } => {
            let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
            c.args(*args);
            if *mock {
                c.env("DROMON_MOCK", "1");
            }
            c
        }
    };
    match command.args(args).current_dir(&root).status() {
        Ok(status) => ExitCode::from(status.code().map_or(1, |c| u8::try_from(c).unwrap_or(1))),
        Err(e) => {
            eprintln!("xtask: couldn't run {name}: {e}");
            ExitCode::FAILURE
        }
    }
}
