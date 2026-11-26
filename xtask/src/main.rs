//! Dromon's developer commands, run as `cargo <command>` through the aliases in
//! `.cargo/config.toml` (`cargo xtask <command>` works too). Each one runs a script under
//! `scripts/` or a cargo command, passing the remaining arguments on; CI runs the same commands.
//! Relative paths in the arguments are relative to where you run it, as with the scripts.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// What a command runs.
enum Run {
    /// `bash scripts/<file>` with the arguments.
    Script(&'static str),
    /// `cargo <before…> <arguments…> <after…>`; `mock` also sets `DROMON_MOCK=1`.
    Cargo { before: &'static [&'static str], after: &'static [&'static str], mock: bool },
}

/// (name, what it runs, usage, what it does).
const COMMANDS: &[(&str, Run, &str, &str)] = &[
    (
        "dev",
        Run::Script("dev.sh"),
        "[--mock] [-- APP ARGS]",
        "live editing: view and theme edits patch into the running app",
    ),
    (
        "app",
        Run::Cargo { before: &["run", "-p", "dromon-app", "--"], after: &[], mock: false },
        "[APP ARGS]",
        "the app, debug build, on your ssh config",
    ),
    (
        "demo",
        Run::Cargo { before: &["run", "-p", "dromon-app", "--"], after: &[], mock: true },
        "[APP ARGS]",
        "the app on the built-in demo: fake hosts and folders, no servers",
    ),
    (
        "lint",
        Run::Cargo {
            before: &["clippy", "--workspace", "--all-targets"],
            after: &["--", "-D", "warnings"],
            mock: false,
        },
        "[--locked]",
        "clippy, as CI runs it",
    ),
    (
        "goldens",
        Run::Script("goldens.sh"),
        "[--check]",
        "re-render the UI goldens in tests/goldens (--check: compare, as CI)",
    ),
    ("flows", Run::Script("flows.sh"), "[OUT DIR]", "render tests/flows/* to strips and trees in target/flows"),
    ("smoke", Run::Script("smoke-app.sh"), "[--window] [--connect]", "smoke-test the dromon binary"),
    ("repro", Run::Script("repro.sh"), "<ISSUE|issue.md>", "replay a bug report headless into target/repro"),
    ("perf", Run::Script("perf.sh"), "[--gate …|all]", "the performance gates, as one table"),
    ("accents", Run::Script("accents.sh"), "", "render the accent candidates side by side in target/accents"),
    (
        "package-linux",
        Run::Script("package-linux.sh"),
        "[--arch …] [--verify]",
        "build the release binary and the Linux packages",
    ),
];

fn help() {
    println!("Dromon's developer commands: cargo <command> [args]\n");
    for (name, _, usage, what) in COMMANDS {
        println!("  cargo {:<40} {what}", format!("{name} {usage}"));
    }
    println!("\nEach script under scripts/ explains its own options (cargo <command> --help for most).");
}

/// The bash to run scripts with. On Windows that is Git's (Git Bash, as on GitHub's runners):
/// a bare `bash` from a program finds System32's first, which is WSL's launcher.
fn bash() -> PathBuf {
    if !cfg!(windows) {
        return "bash".into();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    let not_wsl = |p: &Path| {
        let s = p.to_string_lossy().to_ascii_lowercase();
        !s.contains("\\system32\\") && !s.contains("\\windowsapps\\")
    };
    // A bash.exe on PATH that isn't WSL's; else the one beside git.exe (<Git>/cmd → <Git>/bin).
    dirs.iter()
        .map(|d| d.join("bash.exe"))
        .find(|p| p.is_file() && not_wsl(p))
        .or_else(|| {
            dirs.iter()
                .filter(|d| d.join("git.exe").is_file())
                .filter_map(|d| d.parent().map(|git| git.join("bin").join("bash.exe")))
                .find(|p| p.is_file())
        })
        .unwrap_or_else(|| "bash".into())
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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask sits in the repository root");
    let mut command = match run {
        Run::Script(file) => {
            // Forward slashes: the scripts find the repository with `dirname "${BASH_SOURCE[0]}"`,
            // which doesn't split Windows' backslashes (Git Bash reads `D:/a/b` fine).
            let script = root.join("scripts").join(file).to_string_lossy().replace('\\', "/");
            let mut c = Command::new(bash());
            c.arg(script).args(args);
            c
        }
        Run::Cargo { before, after, mock } => {
            let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
            c.args(*before).args(args).args(*after);
            if *mock {
                c.env("DROMON_MOCK", "1");
            }
            c
        }
    };
    match command.status() {
        Ok(status) => ExitCode::from(status.code().map_or(1, |c| u8::try_from(c).unwrap_or(1))),
        Err(e) => {
            eprintln!("xtask: couldn't run {name}: {e}");
            ExitCode::FAILURE
        }
    }
}
