//! Detecting Linux installs, and which packages each install takes.

use std::path::Path;

use super::*;

#[test]
fn linux_packages_are_told_apart_by_their_package_manager() {
    let exe = Path::new("/usr/bin/dromon");
    let none: Vec<String> = Vec::new();
    let arch = vec!["bash-5.2-1".to_owned(), "dromon-0.2.0-1".to_owned()];
    let other = vec!["dromon-bin-0.2.0-1".to_owned(), "dromonx-1-1".to_owned()];
    assert_eq!(linux(exe, true, &none), Install::Deb);
    assert_eq!(linux(exe, false, &arch), Install::Arch);
    assert_eq!(linux(exe, false, &other), Install::Other, "another package's name isn't ours");
    assert_eq!(linux(exe, false, &none), Install::Other);
    // A build run from its folder, whatever is installed besides.
    assert_eq!(linux(Path::new("/home/me/dromon/target/release/dromon"), true, &arch), Install::Other);
    assert_eq!((DPKG_LIST, PACMAN_LOCAL), ("/var/lib/dpkg/info/dromon.list", "/var/lib/pacman/local"));
}

#[test]
fn each_install_takes_only_its_own_package() {
    let work = Path::new("/nowhere");
    let file = Path::new("/nowhere/file");
    let mac = Install::MacApp { team: Some("ABCDE12345".into()) };
    let refused = "this copy of Dromon doesn't install that package";
    for (package, install) in [
        (Package::Dmg, Install::WindowsMsi),
        (Package::Dmg, Install::MacApp { team: None }),
        (Package::Dmg, Install::MacHomebrew),
        (Package::Msi, mac.clone()),
        (Package::Msi, Install::Deb),
        (Package::Dmg, Install::Other),
    ] {
        assert_eq!(super::install(package, file, &install, work), Err(refused.to_owned()), "{package:?} {install:?}");
    }
    // The right pairs reach their platform's installer, which this platform may not be.
    if !cfg!(target_os = "macos") {
        assert_eq!(super::install(Package::Dmg, file, &mac, work), Err("the macOS app updates only on macOS".into()));
    }
    if !cfg!(windows) {
        let msi = super::install(Package::Msi, file, &Install::WindowsMsi, work);
        assert_eq!(msi, Err("the Windows installer runs only on Windows".into()));
    }
}

#[test]
fn a_command_line_is_its_program_and_arguments() {
    let cmd = Cmd::new("/usr/bin/true", ["a b", "c"]);
    assert_eq!(cmd.words(), ["/usr/bin/true", "a b", "c"]);
}
