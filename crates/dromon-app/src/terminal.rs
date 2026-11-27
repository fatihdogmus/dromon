//! "Open in Terminal": the user's own terminal app, running an ssh of its own to the host in a
//! folder (`state/menu.rs` says which). It runs on a short-lived thread, so the UI never waits on
//! it, and it never goes through a local shell: every program gets an argument vector. The one
//! exception is macOS's Terminal and iTerm2, which take no command line, only a file to open:
//! there the launcher writes a `.command` script (0700, in the app's cache folder) whose only
//! command is the ssh line, every word of it quoted by `SshCommand::shell_line`, and which deletes
//! itself as it starts ([`command_script`]); `open -a Terminal <script>` runs it. No Apple Events,
//! so the hardened, notarized app needs no automation entitlement and asks no consent.
//!
//! The ssh is `dromon_ssh::SshCommand::terminal`'s: the host's own options, with `-t`, and no
//! askpass, so the terminal's ssh asks for passwords, passphrases and codes itself. Dromon never
//! hands over a secret it remembers.
//!
//! | Terminal | Started as |
//! | --- | --- |
//! | Terminal (macOS) | `open -a Terminal <script.command>` |
//! | iTerm2 | `open -a iTerm <script.command>` |
//! | Ghostty (macOS) | `open -na Ghostty --args -e <ssh…>` |
//! | WezTerm | `wezterm start -- <ssh…>` |
//! | kitty, foot | `kitty <ssh…>`, `foot <ssh…>` |
//! | Alacritty, Konsole, XTerm | `<program> -e <ssh…>` |
//! | GNOME Terminal | `gnome-terminal -- <ssh…>` |
//! | Windows Terminal | `wt.exe new-tab -- <ssh…>` (each `;` written `\;`, wt's own separator) |
//! | Console window (Windows) | `%SystemRoot%\System32\conhost.exe <ssh.exe…>`: a console of its own, whose command line conhost hands on to ssh as it is |
//! | `$TERMINAL`, `x-terminal-emulator` | `<program> -e <ssh…>`, or the table's row for a known name |
//!
//! "System default" is Terminal on macOS; Windows Terminal, else the console window, on
//! Windows; `$TERMINAL`, then `x-terminal-emulator`, then the first of GNOME Terminal, Konsole,
//! kitty, Alacritty, foot and XTerm installed on Linux. Where they are is looked up in
//! `/Applications` (and `~/Applications`) and on `PATH` ([`Machine`]); what to run is the pure
//! [`plan`], tested for every platform on any of them.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use dromon_core::HostSpec;
use dromon_ssh::SshCommand;

use crate::state::settings::Terminal;

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;

/// Whose conventions apply: where apps are and which terminals there are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
    Linux,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    /// The terminals Settings may offer here, in the order it lists them.
    pub fn terminals(self) -> &'static [Terminal] {
        use Terminal as T;
        match self {
            Platform::Mac => &[T::TerminalApp, T::Iterm2, T::Ghostty, T::WezTerm, T::Kitty, T::Alacritty],
            Platform::Windows => &[T::WindowsTerminal, T::Console],
            Platform::Linux => &[T::GnomeTerminal, T::Konsole, T::Kitty, T::Alacritty, T::Foot, T::Xterm],
        }
    }
}

/// A terminal found on this machine, ready to plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Which known terminal; `None` for `$TERMINAL` or `x-terminal-emulator` naming one this
    /// table doesn't know, which take `-e`.
    pub terminal: Option<Terminal>,
    /// What to run: the terminal's binary, `open` for the macOS apps started through the OS, or
    /// `conhost.exe` for the console window.
    pub program: PathBuf,
}

/// What to start for one "Open in Terminal".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// The program hands the command to the terminal and exits (`open`, `wt.exe`,
    /// `gnome-terminal`): its exit status and error output say whether that worked. Otherwise
    /// the program is the terminal window itself, and only starting it can fail.
    pub hands_off: bool,
    /// A `.command` script to write first ([`command_script`]); its path goes last in `args`.
    pub script: Option<String>,
}

const OPEN: &str = "/usr/bin/open";

/// What starts `found` running `ssh`. Pure: the same table on every platform.
pub fn plan(found: &Found, ssh: &SshCommand) -> Launch {
    let argv: Vec<OsString> = std::iter::once(ssh.program.clone().into_os_string()).chain(ssh.args.clone()).collect();
    let program = found.program.clone().into_os_string();
    let run = |program: OsString, before: &[&str], hands_off: bool| Launch {
        program,
        args: before.iter().map(OsString::from).chain(argv.iter().cloned()).collect(),
        hands_off,
        script: None,
    };
    let script = |program: OsString, app: &str| Launch {
        program,
        args: vec!["-a".into(), app.into()],
        hands_off: true,
        script: Some(command_script(ssh)),
    };
    match found.terminal {
        Some(Terminal::TerminalApp) => script(program, "Terminal"),
        Some(Terminal::Iterm2) => script(program, "iTerm"),
        Some(Terminal::Ghostty) => run(program, &["-na", "Ghostty", "--args", "-e"], true),
        Some(Terminal::WezTerm) => run(program, &["start", "--"], false),
        // conhost runs the rest of its command line, which Rust wrote with the MSVC rules ssh
        // reads back.
        Some(Terminal::Kitty | Terminal::Foot | Terminal::Console) => run(program, &[], false),
        Some(Terminal::GnomeTerminal) => run(program, &["--"], true),
        Some(Terminal::WindowsTerminal) => Launch {
            program,
            args: ["new-tab", "--"].iter().map(OsString::from).chain(argv.iter().map(|a| wt_escape(a))).collect(),
            hands_off: true,
            script: None,
        },
        // `-e` and the command: Alacritty, Konsole, XTerm, and whatever `$TERMINAL` or
        // `x-terminal-emulator` is (Debian's policy for the latter). System is resolved first.
        Some(Terminal::Alacritty | Terminal::Konsole | Terminal::Xterm | Terminal::System) | None => {
            run(program, &["-e"], false)
        }
    }
}

/// The `.command` script Terminal and iTerm2 open: it deletes itself, then becomes the ssh, the
/// one command it has, every word quoted for `sh`.
pub fn command_script(ssh: &SshCommand) -> String {
    format!("#!/bin/sh\nrm -f -- \"$0\"\nexec {}\n", ssh.shell_line())
}

/// An argument for `wt.exe`, which splits its own command line at `;`: written `\;`.
fn wt_escape(arg: &std::ffi::OsStr) -> OsString {
    arg.to_string_lossy().replace(';', r"\;").into()
}

/// What this machine has, as [`Machine::find`] and [`Machine::resolve`] read it.
pub struct Machine {
    pub platform: Platform,
    /// `PATH`'s folders.
    pub path: Vec<PathBuf>,
    /// `$TERMINAL`, when set and not empty.
    pub terminal_env: Option<String>,
    pub home: Option<PathBuf>,
    /// Windows' `%SystemRoot%`, when set and not empty (`C:\Windows` otherwise).
    pub system_root: Option<PathBuf>,
    /// Whether something is at this path (an app bundle, a program).
    pub exists: Box<dyn Fn(&Path) -> bool>,
}

impl Machine {
    /// This machine.
    pub fn here() -> Machine {
        Machine {
            platform: Platform::current(),
            path: std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default(),
            terminal_env: std::env::var("TERMINAL").ok().filter(|t| !t.trim().is_empty()),
            home: std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from),
            system_root: std::env::var_os("SystemRoot").filter(|r| !r.is_empty()).map(PathBuf::from),
            // `symlink_metadata`: Windows' app execution aliases (`wt.exe`) are reparse points.
            exists: Box::new(|p: &Path| p.symlink_metadata().is_ok()),
        }
    }

    /// `name` on `PATH`, or `name` itself when it's a path.
    fn which(&self, name: &str) -> Option<PathBuf> {
        let as_path = Path::new(name);
        if as_path.components().count() > 1 || as_path.is_absolute() {
            return (self.exists)(as_path).then(|| as_path.to_path_buf());
        }
        self.path.iter().map(|dir| dir.join(name)).find(|p| (self.exists)(p))
    }

    /// `<name>.app` in `/Applications` or `~/Applications`.
    fn app(&self, name: &str) -> Option<PathBuf> {
        let bundle = format!("{name}.app");
        let mut places = vec![Path::new("/Applications").join(&bundle)];
        places.extend(self.home.as_ref().map(|h| h.join("Applications").join(&bundle)));
        places.into_iter().find(|p| (self.exists)(p))
    }

    /// Where `terminal` is here, if it is.
    pub fn find(&self, terminal: Terminal) -> Option<Found> {
        let found = |program: PathBuf| Some(Found { terminal: Some(terminal), program });
        // A macOS app's binary in its bundle, else on PATH.
        let bundled = |app: &str, binary: &str| {
            self.app(app).map(|b| b.join("Contents/MacOS").join(binary)).or_else(|| self.which(binary))
        };
        match (self.platform, terminal) {
            (_, Terminal::System) => None,
            (Platform::Mac, Terminal::TerminalApp) => found(OPEN.into()),
            (Platform::Mac, Terminal::Iterm2) => self.app("iTerm").and_then(|_| found(OPEN.into())),
            (Platform::Mac, Terminal::Ghostty) => self.app("Ghostty").and_then(|_| found(OPEN.into())),
            (Platform::Mac, Terminal::WezTerm) => bundled("WezTerm", "wezterm").and_then(found),
            (Platform::Mac, Terminal::Kitty) => bundled("kitty", "kitty").and_then(found),
            (Platform::Mac, Terminal::Alacritty) => bundled("Alacritty", "alacritty").and_then(found),
            (Platform::Windows, Terminal::WindowsTerminal) => self.which("wt.exe").and_then(found),
            (Platform::Windows, Terminal::Console) => {
                let root = self.system_root.clone().unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
                found(root.join("System32").join("conhost.exe"))
            }
            (Platform::Linux, t) if Platform::Linux.terminals().contains(&t) => {
                self.which(linux_name(t)).and_then(found)
            }
            _ => None,
        }
    }

    /// The terminals Settings offers here besides System default: those installed.
    pub fn installed(&self) -> Vec<Terminal> {
        self.platform.terminals().iter().copied().filter(|t| self.find(*t).is_some()).collect()
    }

    /// What `choice` starts here; why not, when nothing can.
    pub fn resolve(&self, choice: Terminal) -> Result<Found, String> {
        if choice != Terminal::System {
            return self.find(choice).ok_or_else(|| format!("{} isn't installed", choice.label()));
        }
        let first = |list: &[Terminal]| list.iter().find_map(|t| self.find(*t));
        let found = match self.platform {
            Platform::Mac => self.find(Terminal::TerminalApp),
            Platform::Windows => first(&[Terminal::WindowsTerminal, Terminal::Console]),
            Platform::Linux => self
                .terminal_env
                .as_deref()
                .and_then(|name| self.which(name).map(|program| Found { terminal: known_name(name), program }))
                .or_else(|| self.which("x-terminal-emulator").map(|program| Found { terminal: None, program }))
                .or_else(|| first(Platform::Linux.terminals())),
        };
        found.ok_or_else(|| "no terminal app was found; pick one in Settings".to_owned())
    }
}

/// A Linux terminal's program name.
fn linux_name(t: Terminal) -> &'static str {
    match t {
        Terminal::GnomeTerminal => "gnome-terminal",
        Terminal::Konsole => "konsole",
        Terminal::Kitty => "kitty",
        Terminal::Alacritty => "alacritty",
        Terminal::Foot => "foot",
        Terminal::WezTerm => "wezterm",
        _ => "xterm",
    }
}

/// The known terminal `$TERMINAL` names by its file name, so it gets its own flags.
fn known_name(program: &str) -> Option<Terminal> {
    let name = Path::new(program).file_name()?.to_str()?;
    [Terminal::GnomeTerminal, Terminal::Konsole, Terminal::Kitty, Terminal::Alacritty, Terminal::Foot]
        .into_iter()
        .chain([Terminal::Xterm, Terminal::WezTerm])
        .find(|t| linux_name(*t) == name)
}

/// Opens `terminal` running ssh to `host` in `folder`, on a thread of its own; `failed` hears
/// why when it couldn't (on that thread).
pub fn open(terminal: Terminal, host: HostSpec, folder: String, failed: impl FnOnce(String) + Send + 'static) {
    let spawned = std::thread::Builder::new().name("dromon-terminal".into()).spawn(move || {
        let launched = SshCommand::terminal(&host, &folder)
            .map_err(|e| e.to_string())
            .and_then(|ssh| Ok(plan(&Machine::here().resolve(terminal)?, &ssh)))
            .and_then(|launch| start(&launch, &script_folder()));
        if let Err(why) = launched {
            failed(why);
        }
    });
    if let Err(e) = spawned {
        eprintln!("dromon: couldn't start the terminal thread: {e}");
    }
}

/// Looks for the terminals installed here on a thread of its own; `found` hears them there.
pub fn detect(found: impl FnOnce(Vec<Terminal>) + Send + 'static) {
    let spawned =
        std::thread::Builder::new().name("dromon-terminal".into()).spawn(move || found(Machine::here().installed()));
    if let Err(e) = spawned {
        eprintln!("dromon: couldn't look for terminals: {e}");
    }
}

/// Where `.command` scripts are written: the app's cache folder, else the temporary folder.
fn script_folder() -> PathBuf {
    rune::dirs::AppDirs::new(crate::persist::APP_ID).map_or_else(std::env::temp_dir, |d| d.cache().to_path_buf())
}

/// Writes `content` as a new script in `folder` (made if missing), readable, writable and
/// runnable by the user alone (0700), under a name no other launch uses.
fn write_script(folder: &Path, content: &str) -> std::io::Result<PathBuf> {
    use std::io::Write;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(folder)?;
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = folder.join(format!("dromon-terminal-{}-{n}.command", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o700);
    let mut file = options.open(&path)?;
    file.write_all(content.as_bytes())?;
    Ok(path)
}

/// Starts `launch`, its script (if any) written into `folder` first. A program that hands off
/// is waited for and its error output read; a terminal window is only started (and reaped on
/// this thread when it closes).
fn start(launch: &Launch, folder: &Path) -> Result<(), String> {
    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args);
    let script = match &launch.script {
        Some(content) => {
            let path = write_script(folder, content).map_err(|e| format!("couldn't write its script: {e}"))?;
            cmd.arg(&path);
            Some(path)
        }
        None => None,
    };
    let started = run(&mut cmd, launch);
    // The script deletes itself when it runs; one that never ran goes here.
    if let (Err(_), Some(path)) = (&started, script) {
        let _ = std::fs::remove_file(path);
    }
    started
}

fn run(cmd: &mut Command, launch: &Launch) -> Result<(), String> {
    let name = Path::new(&launch.program).file_name().map_or_else(|| "the terminal".into(), |n| n.to_string_lossy());
    cmd.stdin(Stdio::null()).stdout(Stdio::null());
    cmd.stderr(if launch.hands_off { Stdio::piped() } else { Stdio::null() });
    let child = cmd.spawn().map_err(|e| format!("{name}: {e}"))?;
    if !launch.hands_off {
        let mut child = child;
        let _ = child.wait();
        return Ok(());
    }
    let out = child.wait_with_output().map_err(|e| format!("{name}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map_or_else(|| format!("{name} stopped ({})", out.status), |line| format!("{name}: {line}")))
}
