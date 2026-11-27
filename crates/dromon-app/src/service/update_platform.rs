//! How this copy of Dromon was installed ([`detect`]), and putting a checked update in place
//! ([`install`]): the macOS app swapped in place (`update_macos.rs`), the Windows MSI handed to
//! `msiexec` (`update_windows.rs`). Linux packages update through their package manager, so
//! Linux only detects. Every external program runs by its absolute path, with an argument
//! vector, never through a shell; the command lines are pure functions, tested on every
//! platform.

use std::path::Path;

use crate::state::update::{Install, Package};

#[cfg(test)]
#[path = "update_platform_tests.rs"]
mod tests;

/// One program to run and its arguments.
#[cfg(any(target_os = "macos", windows, test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cmd {
    pub program: std::path::PathBuf,
    pub args: Vec<std::ffi::OsString>,
}

#[cfg(any(target_os = "macos", windows, test))]
impl Cmd {
    pub fn new(
        program: impl Into<std::path::PathBuf>,
        args: impl IntoIterator<Item = impl Into<std::ffi::OsString>>,
    ) -> Cmd {
        Cmd { program: program.into(), args: args.into_iter().map(Into::into).collect() }
    }

    /// The command line as words, for tests.
    #[cfg(test)]
    pub fn words(&self) -> Vec<String> {
        std::iter::once(self.program.as_os_str())
            .chain(self.args.iter().map(|a| a.as_os_str()))
            .map(|w| w.to_string_lossy().into_owned())
            .collect()
    }
}

/// Runs `cmd` to its end: its stderr and stdout when it succeeds, else why it didn't.
#[cfg(target_os = "macos")]
pub fn run(cmd: &Cmd) -> Result<String, String> {
    let program = cmd.program.display();
    let out = std::process::Command::new(&cmd.program)
        .args(&cmd.args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run {program}: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    if out.status.success() {
        Ok(text)
    } else {
        let why = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("no message").trim().to_owned();
        Err(format!("{program} failed ({}): {why}", out.status))
    }
}

/// How this copy was installed. Blocking (macOS asks `codesign`): the service's blocking threads.
pub fn detect() -> Install {
    match std::env::current_exe() {
        Ok(exe) => detect_at(&exe),
        Err(_) => Install::Other,
    }
}

#[cfg(target_os = "macos")]
fn detect_at(exe: &Path) -> Install {
    super::update_macos::detect(exe)
}

#[cfg(windows)]
fn detect_at(exe: &Path) -> Install {
    super::update_windows::detect(exe)
}

#[cfg(target_os = "linux")]
fn detect_at(exe: &Path) -> Install {
    let pacman: Vec<String> = std::fs::read_dir(PACMAN_LOCAL)
        .map(|dir| dir.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    linux(exe, Path::new(DPKG_LIST).exists(), &pacman)
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn detect_at(_: &Path) -> Install {
    Install::Other
}

/// Where the packages install the program.
#[cfg(any(target_os = "linux", test))]
const LINUX_EXE: &str = "/usr/bin/dromon";
/// The file list dpkg keeps for the installed `dromon` package.
#[cfg(any(target_os = "linux", test))]
const DPKG_LIST: &str = "/var/lib/dpkg/info/dromon.list";
/// pacman's database of installed packages: one `<name>-<version>-<rel>` folder each.
#[cfg(any(target_os = "linux", test))]
const PACMAN_LOCAL: &str = "/var/lib/pacman/local";

/// Linux: the packaged program (`/usr/bin/dromon`) from the `.deb` (`dpkg`: dpkg lists the
/// package) or the Arch package (`pacman`: the names in pacman's database); anything else, a
/// build run from its folder say, is [`Install::Other`].
#[cfg(any(target_os = "linux", test))]
fn linux(exe: &Path, dpkg: bool, pacman: &[String]) -> Install {
    if exe != Path::new(LINUX_EXE) {
        return Install::Other;
    }
    // `dromon-0.2.0-1`, never `dromon-bin-…` (another package).
    let arch = pacman
        .iter()
        .any(|name| name.strip_prefix("dromon-").is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit())));
    match (dpkg, arch) {
        (true, _) => Install::Deb,
        (false, true) => Install::Arch,
        (false, false) => Install::Other,
    }
}

/// Puts the checked file `file` in place for `install`. `Ok(true)`: the new version is in
/// place, start it (macOS); `Ok(false)`: its installer runs, quit (Windows). On an error nothing
/// changed. `work` is a folder for the install's own temporary files.
pub fn install(package: Package, file: &Path, install: &Install, work: &Path) -> Result<bool, String> {
    match (package, install) {
        (Package::Dmg, Install::MacApp { team: Some(team) }) => mac(file, team, work).map(|()| true),
        (Package::Msi, Install::WindowsMsi) => windows(file).map(|()| false),
        _ => Err("this copy of Dromon doesn't install that package".to_owned()),
    }
}

#[cfg(target_os = "macos")]
fn mac(dmg: &Path, team: &str, work: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("couldn't find the running app: {e}"))?;
    let bundle = super::update_macos::bundle_of(&exe).ok_or("the running app isn't in an app bundle")?;
    super::update_macos::install(&mut run, dmg, &bundle, team, work)
}

#[cfg(not(target_os = "macos"))]
fn mac(_: &Path, _: &str, _: &Path) -> Result<(), String> {
    Err("the macOS app updates only on macOS".to_owned())
}

#[cfg(windows)]
fn windows(msi: &Path) -> Result<(), String> {
    super::update_windows::install(msi)
}

#[cfg(not(windows))]
fn windows(_: &Path) -> Result<(), String> {
    Err("the Windows installer runs only on Windows".to_owned())
}
