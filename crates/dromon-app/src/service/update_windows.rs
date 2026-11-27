//! Windows: how the app was installed, and handing a newer MSI to Windows Installer.
//!
//! The MSI's fixed UpgradeCode (`packaging/windows/dromon.wxs`, docs/packaging.md) makes every
//! release upgrade any earlier one, so updating is running it: `msiexec /i <msi> /passive`
//! (a progress bar, no questions), started on its own, after which the app quits so the
//! installer can replace the running version. msiexec runs from the system folder by its full
//! path, never found on `PATH`.

use std::path::{Path, PathBuf};

use super::update_platform::Cmd;
#[cfg(windows)]
use crate::state::update::Install;

#[cfg(test)]
#[path = "update_windows_tests.rs"]
mod tests;

/// Whether `exe` is where the MSI installs the program: `<Program Files>\Dromon\dromon.exe`
/// (`program_files`: the 64-bit Program Files folder). Windows paths compare without case.
pub fn is_msi_install(exe: &Path, program_files: &Path) -> bool {
    let expected = program_files.join("Dromon").join("dromon.exe");
    exe.as_os_str().to_string_lossy().to_lowercase() == expected.as_os_str().to_string_lossy().to_lowercase()
}

/// `msiexec /i <msi> /passive`, from `system_root`'s System32.
pub fn msiexec(system_root: &Path, msi: &Path) -> Cmd {
    let program: PathBuf = system_root.join("System32").join("msiexec.exe");
    Cmd::new(program, ["/i".into(), msi.as_os_str().to_owned(), "/passive".into()])
}

/// How the app at `exe` was installed.
#[cfg(windows)]
pub fn detect(exe: &Path) -> Install {
    // ProgramW6432 is the 64-bit Program Files folder whichever way this process runs.
    let program_files = std::env::var_os("ProgramW6432").or_else(|| std::env::var_os("ProgramFiles"));
    match program_files {
        Some(dir) if is_msi_install(exe, Path::new(&dir)) => Install::WindowsMsi,
        _ => Install::Other,
    }
}

/// Starts the installer for `msi` on its own (the app quits next, it doesn't wait).
#[cfg(windows)]
pub fn install(msi: &Path) -> Result<(), String> {
    let root = std::env::var_os("SystemRoot").ok_or("SystemRoot isn't set, so msiexec can't be found")?;
    let cmd = msiexec(Path::new(&root), msi);
    std::process::Command::new(&cmd.program)
        .args(&cmd.args)
        .spawn()
        .map(drop)
        .map_err(|e| format!("couldn't start the installer: {e}"))
}
