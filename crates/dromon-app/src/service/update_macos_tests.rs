//! macOS's command lines and the in-place install, run on every platform: the install with a
//! stand-in for the commands (`hdiutil`, `codesign`, `ditto`) on real folders, so the swap,
//! its rollback and what each failure leaves behind are checked here; the real commands run
//! only on macOS.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use super::*;

const TEAM: &str = "ABCDE12345";

fn words(cmd: &Cmd) -> Vec<String> {
    cmd.words()
}

#[test]
fn the_command_lines() {
    let app = Path::new("/Volumes/x/Dromon.app");
    assert_eq!(
        words(&attach(Path::new("/c/Dromon-0.3.0-macos-universal.dmg"), Path::new("/c/mount"))),
        [
            "/usr/bin/hdiutil",
            "attach",
            "-nobrowse",
            "-readonly",
            "-noautoopen",
            "-mountpoint",
            "/c/mount",
            "/c/Dromon-0.3.0-macos-universal.dmg"
        ]
    );
    assert_eq!(words(&detach(Path::new("/c/mount"), false)), ["/usr/bin/hdiutil", "detach", "/c/mount"]);
    assert_eq!(words(&detach(Path::new("/c/mount"), true)), ["/usr/bin/hdiutil", "detach", "-force", "/c/mount"]);
    assert_eq!(words(&describe(app)), ["/usr/bin/codesign", "-dv", "--verbose=2", "/Volumes/x/Dromon.app"]);
    assert_eq!(
        words(&verify(app, TEAM)),
        [
            "/usr/bin/codesign",
            "--verify",
            "--deep",
            "--strict",
            "-R",
            "=identifier \"dev.dromon.Dromon\" and anchor apple generic \
             and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
             and certificate leaf[field.1.2.840.113635.100.6.1.13] exists \
             and certificate leaf[subject.OU] = \"ABCDE12345\"",
            "/Volumes/x/Dromon.app"
        ]
    );
    assert_eq!(
        words(&copy(app, Path::new("/Applications/.Dromon-update.app"))),
        ["/usr/bin/ditto", "/Volumes/x/Dromon.app", "/Applications/.Dromon-update.app"]
    );
    let bundle = Path::new("/Applications/Dromon.app");
    assert_eq!(staging(bundle), Path::new("/Applications/.Dromon-update.app"));
    assert_eq!(retired(bundle), Path::new("/Applications/.Dromon-old.app"));
}

#[test]
fn the_bundle_is_found_from_its_executable() {
    let table: &[(&str, Option<&str>)] = &[
        ("/Applications/Dromon.app/Contents/MacOS/dromon", Some("/Applications/Dromon.app")),
        ("/Users/me/Apps/Dromon.app/Contents/MacOS/dromon", Some("/Users/me/Apps/Dromon.app")),
        ("/Users/me/dromon/target/release/dromon", None),
        ("/Applications/Dromon/Contents/MacOS/dromon", None),
        ("/Applications/Dromon.app/Contents/Resources/dromon", None),
        ("/dromon", None),
    ];
    for (exe, bundle) in table {
        assert_eq!(bundle_of(Path::new(exe)).as_deref(), bundle.map(Path::new), "{exe}");
    }
}

#[test]
fn homebrew_installs_are_homebrews_to_update() {
    let apps = Path::new("/Applications/Dromon.app");
    let nothing = |_: &Path| false;
    assert!(!homebrew(apps, nothing));
    for room in CASKROOMS {
        assert!(homebrew(apps, |p: &Path| p == Path::new(room)), "{room}");
    }
    assert!(homebrew(Path::new("/opt/homebrew/Caskroom/dromon/0.2.0/Dromon.app"), nothing));
}

#[test]
fn only_a_developer_id_signature_names_a_team() {
    let signed = "Executable=/Applications/Dromon.app/Contents/MacOS/dromon\n\
                  Identifier=dev.dromon.Dromon\n\
                  Authority=Developer ID Application: Someone (ABCDE12345)\n\
                  Authority=Developer ID Certification Authority\n\
                  Authority=Apple Root CA\n\
                  TeamIdentifier=ABCDE12345\n";
    assert_eq!(team_of(signed).as_deref(), Some("ABCDE12345"));
    // Ad-hoc (the default without Apple credentials, and development builds).
    let adhoc = "Identifier=dev.dromon.Dromon\nSignature=adhoc\nTeamIdentifier=not set\n";
    assert_eq!(team_of(adhoc), None);
    // A team, but not Developer ID (an Apple Development certificate).
    let development = "Authority=Apple Development: Someone (XYZ)\nTeamIdentifier=ABCDE12345\n";
    assert_eq!(team_of(development), None);
    let odd = "Authority=Developer ID Application: x\nTeamIdentifier=abc\n";
    assert_eq!(team_of(odd), None);
}

/// The commands as a test plays them: `hdiutil attach` mounts a new `Dromon.app`, `ditto`
/// copies folders, `codesign --verify` passes unless its app is `refuse`'s, and every call
/// is kept.
struct Fake {
    calls: RefCell<Vec<Vec<String>>>,
    refuse: Option<PathBuf>,
    fail_attach: bool,
    fail_copy: bool,
}

impl Fake {
    fn new() -> Fake {
        Fake { calls: RefCell::new(Vec::new()), refuse: None, fail_attach: false, fail_copy: false }
    }

    fn run(&self, cmd: &Cmd) -> Result<String, String> {
        let w = cmd.words();
        self.calls.borrow_mut().push(w.clone());
        match (w[0].as_str(), w[1].as_str()) {
            ("/usr/bin/hdiutil", "attach") if self.fail_attach => Err("hdiutil: attach failed".into()),
            ("/usr/bin/hdiutil", "attach") => {
                let app = Path::new(&w[6]).join("Dromon.app");
                write(&app.join("Contents/MacOS/dromon"), "new");
                Ok(String::new())
            }
            ("/usr/bin/hdiutil", "detach") => Ok(String::new()),
            ("/usr/bin/codesign", "--verify") => match &self.refuse {
                Some(p) if Path::new(w.last().unwrap()) == p => Err("codesign: invalid signature".into()),
                _ => Ok(String::new()),
            },
            ("/usr/bin/ditto", _) if self.fail_copy => Err("ditto: no space left".into()),
            ("/usr/bin/ditto", _) => {
                copy_tree(Path::new(&w[1]), Path::new(&w[2]));
                Ok(String::new())
            }
            _ => panic!("unexpected command {w:?}"),
        }
    }

    /// Each call's program, with its subcommand or first option (`ditto` has neither).
    fn programs(&self) -> Vec<String> {
        let name = |w: &Vec<String>| w[0].rsplit('/').next().unwrap().to_owned();
        self.calls
            .borrow()
            .iter()
            .map(|w| if name(w) == "ditto" { name(w) } else { format!("{} {}", name(w), w[1]) })
            .collect()
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A running app in a temporary Applications folder, and a work folder.
struct Scene {
    _dir: tempfile::TempDir,
    bundle: PathBuf,
    work: PathBuf,
}

fn scene() -> Scene {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("Applications/Dromon.app");
    write(&bundle.join("Contents/MacOS/dromon"), "old");
    let work = dir.path().join("cache");
    Scene { bundle, work, _dir: dir }
}

impl Scene {
    fn exe(&self) -> String {
        std::fs::read_to_string(self.bundle.join("Contents/MacOS/dromon")).unwrap()
    }

    fn install(&self, fake: &Fake) -> Result<(), String> {
        let dmg = self.work.join("Dromon-0.3.0-macos-universal.dmg");
        super::install(&mut |c: &Cmd| fake.run(c), &dmg, &self.bundle, TEAM, &self.work)
    }

    /// Nothing but the app (and no staging or old copy beside it).
    fn tidy(&self) -> bool {
        let names: Vec<_> = std::fs::read_dir(self.bundle.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names == ["Dromon.app"]
    }
}

#[test]
fn the_new_app_is_checked_copied_checked_again_and_swapped_in() {
    let s = scene();
    let fake = Fake::new();
    assert_eq!(s.install(&fake), Ok(()));
    assert_eq!(s.exe(), "new");
    assert!(s.tidy(), "the old bundle and the staging copy are gone");
    assert_eq!(
        fake.programs(),
        ["hdiutil attach", "codesign --verify", "ditto", "codesign --verify", "hdiutil detach"]
    );
    let calls = fake.calls.borrow();
    let mounted = s.work.join("mount").join("Dromon.app");
    assert_eq!(calls[1].last().unwrap(), &mounted.to_string_lossy(), "the mounted app");
    assert_eq!(calls[3].last().unwrap(), &staging(&s.bundle).to_string_lossy(), "then its copy");
}

#[test]
fn an_app_the_team_didnt_sign_changes_nothing() {
    let s = scene();
    let mut fake = Fake::new();
    fake.refuse = Some(s.work.join("mount/Dromon.app"));
    let err = s.install(&fake).unwrap_err();
    assert!(err.starts_with("the new app isn't signed by Dromon's developer"), "{err}");
    assert_eq!(s.exe(), "old");
    assert!(s.tidy());
    assert_eq!(fake.programs().last().unwrap(), "hdiutil detach", "the image is detached");
    assert!(!fake.programs().contains(&"ditto".to_owned()), "nothing was copied");
}

#[test]
fn a_copy_that_fails_or_doesnt_verify_changes_nothing() {
    let s = scene();
    let mut fake = Fake::new();
    fake.fail_copy = true;
    assert!(s.install(&fake).unwrap_err().starts_with("couldn't copy the new app"));
    assert_eq!(s.exe(), "old");
    assert!(s.tidy());

    let mut fake = Fake::new();
    fake.refuse = Some(staging(&s.bundle));
    assert!(s.install(&fake).unwrap_err().starts_with("the copied app doesn't verify"));
    assert_eq!(s.exe(), "old");
    assert!(s.tidy(), "the staging copy is removed");
    assert_eq!(fake.programs().last().unwrap(), "hdiutil detach");
}

#[test]
fn an_image_that_doesnt_attach_changes_nothing() {
    let s = scene();
    let mut fake = Fake::new();
    fake.fail_attach = true;
    assert!(s.install(&fake).unwrap_err().starts_with("couldn't open the disk image"));
    assert_eq!(s.exe(), "old");
    assert_eq!(fake.programs(), ["hdiutil attach"], "nothing to detach");
}

#[test]
fn leftovers_of_an_earlier_attempt_are_cleared_first() {
    let s = scene();
    write(&staging(&s.bundle).join("stale"), "x");
    write(&retired(&s.bundle).join("stale"), "x");
    assert_eq!(s.install(&Fake::new()), Ok(()));
    assert_eq!(s.exe(), "new");
    assert!(s.tidy());
}

#[test]
fn a_swap_that_cant_finish_puts_the_old_app_back() {
    let s = scene();
    // The staged copy vanished before the second rename: that rename fails.
    let (staged, old) = (staging(&s.bundle), retired(&s.bundle));
    let err = swap(&s.bundle, &staged, &old).unwrap_err();
    assert!(err.starts_with("couldn't put the new app in place"), "{err}");
    assert_eq!(s.exe(), "old");
    assert!(s.tidy());
    // A bundle that isn't there can't be moved aside: the staged copy goes, nothing else moves.
    let missing = s.bundle.with_file_name("Missing.app");
    write(&staged.join("x"), "new");
    assert!(swap(&missing, &staged, &old).unwrap_err().starts_with("couldn't move this app aside"));
    assert!(!staged.exists());
    assert_eq!(s.exe(), "old");
}

#[test]
fn a_team_id_that_isnt_one_installs_nothing() {
    let s = scene();
    let fake = Fake::new();
    let dmg = s.work.join("x.dmg");
    let err = super::install(&mut |c: &Cmd| fake.run(c), &dmg, &s.bundle, "abc\" or true", &s.work).unwrap_err();
    assert!(err.contains("isn't one"), "{err}");
    assert!(fake.calls.borrow().is_empty());
}
