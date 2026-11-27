//! The launcher's tables, for every platform on any of them: what each terminal is started
//! with, where each is found, and what System default is.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use dromon_core::HostSpec;
use dromon_ssh::SshCommand;

use super::*;

/// A machine with these files and folders, `PATH` `/usr/bin` and `/bin`, home `/home/me`.
fn machine(platform: Platform, files: &[&str], terminal_env: Option<&str>) -> Machine {
    let files: HashSet<PathBuf> = files.iter().map(PathBuf::from).collect();
    Machine {
        platform,
        path: vec![PathBuf::from("/usr/bin"), PathBuf::from("/bin")],
        terminal_env: terminal_env.map(str::to_owned),
        home: Some(PathBuf::from("/home/me")),
        system_root: None,
        exists: Box::new(move |p: &Path| files.contains(p)),
    }
}

/// The ssh of a folder with a quote and a `;` in its name.
fn ssh() -> SshCommand {
    SshCommand {
        program: "ssh".into(),
        args: vec!["-t".into(), "prod".into(), "cd -- '/a;b'".into()],
        ..terminal_ssh()
    }
}

fn terminal_ssh() -> SshCommand {
    SshCommand::terminal(&HostSpec::from_config_alias("prod"), "/srv/it's").unwrap()
}

fn strs(args: &[OsString]) -> Vec<&str> {
    args.iter().map(|a| a.to_str().unwrap()).collect()
}

fn planned(terminal: Terminal, program: &str) -> (String, Vec<String>, bool, bool) {
    let launch = plan(&Found { terminal: Some(terminal), program: program.into() }, &ssh());
    let args = strs(&launch.args).into_iter().map(str::to_owned).collect();
    (launch.program.to_string_lossy().into_owned(), args, launch.hands_off, launch.script.is_some())
}

const SSH: [&str; 4] = ["ssh", "-t", "prod", "cd -- '/a;b'"];

fn with(before: &[&str]) -> Vec<String> {
    before.iter().chain(SSH.iter()).map(|s| (*s).to_owned()).collect()
}

#[test]
fn each_terminal_gets_its_own_flags_and_the_ssh_argv_whole() {
    let rows: [(Terminal, &str, &[&str], bool); 9] = [
        (Terminal::Ghostty, "/usr/bin/open", &["-na", "Ghostty", "--args", "-e"], true),
        (Terminal::WezTerm, "/Applications/WezTerm.app/Contents/MacOS/wezterm", &["start", "--"], false),
        (Terminal::Kitty, "/usr/bin/kitty", &[], false),
        (Terminal::Alacritty, "/usr/bin/alacritty", &["-e"], false),
        (Terminal::GnomeTerminal, "/usr/bin/gnome-terminal", &["--"], true),
        (Terminal::Konsole, "/usr/bin/konsole", &["-e"], false),
        (Terminal::Foot, "/usr/bin/foot", &[], false),
        (Terminal::Xterm, "/usr/bin/xterm", &["-e"], false),
        (Terminal::Alacritty, "/Applications/Alacritty.app/Contents/MacOS/alacritty", &["-e"], false),
    ];
    for (terminal, program, before, hands_off) in rows {
        assert_eq!(planned(terminal, program), (program.to_owned(), with(before), hands_off, false), "{terminal:?}");
    }
    // `$TERMINAL` or `x-terminal-emulator` naming something unknown: `-e`.
    let launch = plan(&Found { terminal: None, program: "/usr/bin/x-terminal-emulator".into() }, &ssh());
    assert_eq!((strs(&launch.args), launch.hands_off), (with(&["-e"]).iter().map(String::as_str).collect(), false));
}

#[test]
fn windows_terminal_takes_a_new_tab_and_the_console_is_conhost_running_ssh() {
    let wt = r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\wt.exe";
    let (program, args, hands_off, script) = planned(Terminal::WindowsTerminal, wt);
    assert_eq!(program, wt);
    // wt splits its own command line at `;`: written `\;` inside the ssh argv.
    assert_eq!(args, ["new-tab", "--", "ssh", "-t", "prod", r"cd -- '/a\;b'"]);
    assert!(hands_off && !script);
    // conhost.exe with ssh's whole argv: a console of its own, no cmd /c, no creation flags.
    let conhost = r"C:\Windows\System32\conhost.exe";
    assert_eq!(planned(Terminal::Console, conhost), (conhost.to_owned(), with(&[]), false, false));
}

#[test]
fn terminal_and_iterm2_open_a_command_script_and_nothing_else() {
    let ssh = terminal_ssh();
    for (terminal, app) in [(Terminal::TerminalApp, "Terminal"), (Terminal::Iterm2, "iTerm")] {
        let launch = plan(&Found { terminal: Some(terminal), program: OPEN.into() }, &ssh);
        assert_eq!(launch.program, OPEN);
        // The script's path goes last when it's written (`start`).
        assert_eq!(strs(&launch.args), ["-a", app], "{terminal:?}");
        assert!(launch.hands_off);
        assert_eq!(launch.script.as_deref(), Some(command_script(&ssh).as_str()));
    }
}

#[test]
fn the_command_script_deletes_itself_then_becomes_the_quoted_ssh() {
    let ssh = terminal_ssh();
    let script = command_script(&ssh);
    assert_eq!(script, format!("#!/bin/sh\nrm -f -- \"$0\"\nexec {}\n", ssh.shell_line()));
    // The platform's ssh (`ssh`, or Windows' `C:\…\ssh.exe` where these tests also run).
    let start = format!("#!/bin/sh\nrm -f -- \"$0\"\nexec '{}' '-t' ", ssh.program.display());
    assert!(script.starts_with(&start), "{script}");
    assert!(script.contains(r"'\''"), "the folder's quote is quoted: {script}");
    assert_eq!(script.lines().count(), 3, "one command besides deleting itself");
}

/// The real script: written 0700 under a fresh name, it runs by itself (its `#!`), deletes
/// itself, and its program gets exactly the arguments ssh would.
#[cfg(unix)]
#[test]
fn a_written_script_runs_its_one_command_and_deletes_itself() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("cache").join("dromon");
    // `printf` in ssh's place prints each argument on its own line.
    let words = ["%s\\n", "a b", "it's", "$HOME `id`", "-rf"];
    let echo =
        SshCommand { program: "printf".into(), args: words[..].iter().map(OsString::from).collect(), ..terminal_ssh() };
    let path = write_script(&folder, &command_script(&echo)).unwrap();
    let again = write_script(&folder, "#!/bin/sh\n").unwrap();
    assert_ne!(path, again, "every launch has its own script");
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with(&format!("dromon-terminal-{}-", std::process::id())) && name.ends_with(".command"));
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o700);
    let out = std::process::Command::new(&path).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a b\nit's\n$HOME `id`\n-rf\n");
    assert!(!path.exists(), "it deleted itself");
}

/// `start` writes the script, passes its path last, and removes it when nothing ran it.
#[cfg(unix)]
#[test]
fn start_hands_the_script_on_and_cleans_up_after_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let echo = SshCommand { program: "true".into(), args: vec![], ..terminal_ssh() };
    // `sh <script>`, as Terminal would run it: it runs and deletes itself.
    let ran = Launch { program: "/bin/sh".into(), args: vec![], hands_off: true, script: Some(command_script(&echo)) };
    assert_eq!(start(&ran, dir.path()), Ok(()));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "the script deleted itself");
    // A launcher that fails leaves no script behind.
    let failed =
        Launch { program: "/bin/false".into(), args: vec![], hands_off: true, script: Some("#!/bin/sh\n".into()) };
    assert!(start(&failed, dir.path()).is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "removed");
    let missing = Launch { program: "/no/such/open".into(), args: vec![], hands_off: true, script: Some("x".into()) };
    assert!(start(&missing, dir.path()).unwrap_err().starts_with("open: "));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn the_mac_finds_apps_in_applications_and_on_path() {
    let m = machine(
        Platform::Mac,
        &[
            "/Applications/iTerm.app",
            "/home/me/Applications/Ghostty.app",
            "/Applications/kitty.app",
            "/usr/bin/wezterm",
        ],
        None,
    );
    assert_eq!(
        m.installed(),
        [Terminal::TerminalApp, Terminal::Iterm2, Terminal::Ghostty, Terminal::WezTerm, Terminal::Kitty]
    );
    let program = |t| m.find(t).map(|f| f.program);
    assert_eq!(program(Terminal::TerminalApp), Some(PathBuf::from(OPEN)));
    assert_eq!(program(Terminal::Iterm2), Some(PathBuf::from(OPEN)));
    assert_eq!(program(Terminal::Ghostty), Some(PathBuf::from(OPEN)));
    assert_eq!(program(Terminal::Kitty), Some(PathBuf::from("/Applications/kitty.app/Contents/MacOS/kitty")));
    assert_eq!(program(Terminal::WezTerm), Some(PathBuf::from("/usr/bin/wezterm")));
    assert_eq!(program(Terminal::Alacritty), None);
    assert_eq!(m.find(Terminal::Xterm), None, "not offered on macOS");
    assert_eq!(m.resolve(Terminal::System).unwrap().terminal, Some(Terminal::TerminalApp));
    assert_eq!(m.resolve(Terminal::Alacritty).unwrap_err(), "Alacritty isn't installed");
}

#[test]
fn windows_prefers_windows_terminal_then_the_console() {
    let mut m = machine(Platform::Windows, &[], None);
    m.path = vec![PathBuf::from(r"C:\Users\me\AppData\Local\Microsoft\WindowsApps")];
    assert_eq!(m.installed(), [Terminal::Console]);
    // The console window is conhost, under %SystemRoot% (C:\Windows when it isn't set).
    let conhost = |root: &str| PathBuf::from(root).join("System32").join("conhost.exe");
    assert_eq!(
        m.resolve(Terminal::System).unwrap(),
        Found { terminal: Some(Terminal::Console), program: conhost(r"C:\Windows") }
    );
    m.system_root = Some(PathBuf::from(r"D:\WINNT"));
    assert_eq!(m.resolve(Terminal::Console).unwrap().program, conhost(r"D:\WINNT"));
    let wt = PathBuf::from(r"C:\Users\me\AppData\Local\Microsoft\WindowsApps").join("wt.exe");
    let seen = wt.clone();
    m.exists = Box::new(move |p: &Path| p == seen);
    assert_eq!(m.installed(), [Terminal::WindowsTerminal, Terminal::Console]);
    assert_eq!(m.resolve(Terminal::System).unwrap(), Found { terminal: Some(Terminal::WindowsTerminal), program: wt });
    assert_eq!(m.resolve(Terminal::Console).unwrap().terminal, Some(Terminal::Console), "chosen: kept");
}

#[test]
fn linux_tries_terminal_then_x_terminal_emulator_then_the_known_ones() {
    let files = ["/usr/bin/xterm", "/bin/konsole", "/usr/bin/x-terminal-emulator", "/opt/foot/foot", "/usr/bin/kitty"];
    let m = machine(Platform::Linux, &files, Some("/opt/foot/foot"));
    assert_eq!(m.installed(), [Terminal::Konsole, Terminal::Kitty, Terminal::Xterm]);
    // $TERMINAL first, with its own flags when it names a known one.
    assert_eq!(
        m.resolve(Terminal::System).unwrap(),
        Found { terminal: Some(Terminal::Foot), program: "/opt/foot/foot".into() }
    );
    let m = machine(Platform::Linux, &files, Some("kitty"));
    assert_eq!(
        m.resolve(Terminal::System).unwrap(),
        Found { terminal: Some(Terminal::Kitty), program: "/usr/bin/kitty".into() }
    );
    // Not found: x-terminal-emulator, which takes `-e`.
    let m = machine(Platform::Linux, &files, Some("nonesuch"));
    assert_eq!(
        m.resolve(Terminal::System).unwrap(),
        Found { terminal: None, program: "/usr/bin/x-terminal-emulator".into() }
    );
    // Then the known ones, in order.
    let m = machine(Platform::Linux, &["/usr/bin/xterm", "/bin/konsole"], None);
    assert_eq!(
        m.resolve(Terminal::System).unwrap(),
        Found { terminal: Some(Terminal::Konsole), program: "/bin/konsole".into() }
    );
    // A chosen one is that one, wherever $TERMINAL points.
    let m = machine(Platform::Linux, &files, Some("kitty"));
    assert_eq!(m.resolve(Terminal::Xterm).unwrap().program, PathBuf::from("/usr/bin/xterm"));
    let none = machine(Platform::Linux, &[], None);
    assert_eq!(none.installed(), []);
    assert_eq!(none.resolve(Terminal::System).unwrap_err(), "no terminal app was found; pick one in Settings");
}

#[test]
fn every_platforms_terminals_have_their_own_names_and_keys() {
    let all: Vec<Terminal> =
        [Platform::Mac, Platform::Windows, Platform::Linux].iter().flat_map(|p| p.terminals().to_vec()).collect();
    let labels: HashSet<&str> = all.iter().map(|t| t.label()).collect();
    let keys: HashSet<&str> = all.iter().map(|t| t.key()).collect();
    // kitty and Alacritty are on two platforms: 14 rows, 12 terminals.
    assert_eq!((all.len(), labels.len(), keys.len()), (14, 12, 12));
    assert!(!labels.contains(Terminal::System.label()));
}

/// The real launcher on this machine: a program that isn't there is a readable failure.
#[cfg(unix)]
#[test]
fn starting_reports_a_missing_program_and_a_failing_hand_off() {
    let dir = tempfile::tempdir().unwrap();
    let missing = Launch { program: "/no/such/term".into(), args: vec![], hands_off: false, script: None };
    let why = start(&missing, dir.path()).unwrap_err();
    assert!(why.starts_with("term: "), "{why}");
    let failing = Launch {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "echo first >&2; echo 'Unable to find application named iTerm' >&2; exit 1".into()],
        hands_off: true,
        script: None,
    };
    assert_eq!(start(&failing, dir.path()).unwrap_err(), "sh: Unable to find application named iTerm");
    let fine =
        Launch { program: "/bin/sh".into(), args: vec!["-c".into(), "exit 0".into()], hands_off: true, script: None };
    assert_eq!(start(&fine, dir.path()), Ok(()));
}
