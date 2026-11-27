//! Windows' command line and the MSI's install folder (run on every platform; the installer
//! itself runs only on Windows).

use std::path::Path;

use super::*;

#[test]
fn msiexec_runs_from_system32_passive() {
    let cmd = msiexec(
        Path::new(r"C:\Windows"),
        Path::new(r"C:\Users\me\AppData\Local\dromon\cache\updates\Dromon-0.3.0-windows-x64.msi"),
    );
    let program = Path::new(r"C:\Windows").join("System32").join("msiexec.exe");
    assert_eq!(cmd.program, program);
    let args: Vec<String> = cmd.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(
        args,
        ["/i", r"C:\Users\me\AppData\Local\dromon\cache\updates\Dromon-0.3.0-windows-x64.msi", "/passive"]
    );
}

#[test]
fn only_the_msis_folder_is_an_msi_install() {
    let pf = Path::new(r"C:\Program Files");
    let installed = pf.join("Dromon").join("dromon.exe");
    assert!(is_msi_install(&installed, pf));
    // Windows paths compare without case.
    let shouted = Path::new(&installed.to_string_lossy().to_uppercase()).to_path_buf();
    assert!(is_msi_install(&shouted, pf));
    assert!(!is_msi_install(Path::new(r"C:\dev\dromon\target\release\dromon.exe"), pf));
    assert!(!is_msi_install(&pf.join("Other").join("dromon.exe"), pf));
}
