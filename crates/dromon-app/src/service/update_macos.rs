//! macOS: how the app was installed, and putting a newer one in its place.
//!
//! - **Homebrew**: an app whose cask is installed (`<prefix>/Caskroom/dromon`), or that lives
//!   in a Caskroom, is Homebrew's to update ([`Install::MacHomebrew`]).
//! - **Developer ID**: the running app's team comes from `codesign -dv`; an app without one (a
//!   development build, ad-hoc signed) doesn't update itself.
//! - **In place** ([`install`]): the DMG (already checked against the signed `SHA256SUMS`) is
//!   attached read-only and out of Finder's sight; its `Dromon.app` must pass `codesign --verify
//!   --deep --strict` against a requirement naming Dromon's bundle id, a Developer ID certificate
//!   and the running app's team. It is copied with `ditto` next to the running bundle (the same
//!   volume, so the swap is two renames), checked again there, then swapped in: the running
//!   bundle moves aside, the new one takes its name, and the old one is removed. If the second
//!   rename fails the old bundle is moved back. Until the swap, nothing but a hidden staging copy
//!   beside the app exists; any failure removes it, and the running app stays as it was. The
//!   image is detached whatever happened. The caller then quits and starts the app again
//!   (`Tx::relaunch`, Launch Services' `open -n` on the same path: the new app).
//!
//! The swap isn't one atomic system call (`renamex_np`'s `RENAME_SWAP` needs FFI): between the
//! two renames, a few microseconds, no app is at the path, and a crash there leaves the old
//! bundle as `.Dromon-old.app` beside it.

use std::path::{Path, PathBuf};

use super::update_platform::Cmd;
#[cfg(target_os = "macos")]
use crate::state::update::Install;

#[cfg(test)]
#[path = "update_macos_tests.rs"]
mod tests;

const HDIUTIL: &str = "/usr/bin/hdiutil";
const CODESIGN: &str = "/usr/bin/codesign";
const DITTO: &str = "/usr/bin/ditto";
/// The app's bundle id (`packaging/macos/Info.plist`, fixed: docs/packaging.md).
const BUNDLE_ID: &str = "dev.dromon.Dromon";
/// The app in the release's DMG.
const APP: &str = "Dromon.app";
/// Where Homebrew keeps an installed `dromon` cask: Apple silicon's prefix, then Intel's.
pub const CASKROOMS: [&str; 2] = ["/opt/homebrew/Caskroom/dromon", "/usr/local/Caskroom/dromon"];

/// The app bundle the executable `exe` runs from (`…/Dromon.app/Contents/MacOS/dromon`).
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent().filter(|p| p.file_name().is_some_and(|n| n == "MacOS"))?;
    let contents = macos.parent().filter(|p| p.file_name().is_some_and(|n| n == "Contents"))?;
    let bundle = contents.parent().filter(|p| p.extension().is_some_and(|e| e == "app"))?;
    Some(bundle.to_path_buf())
}

/// Whether Homebrew manages `bundle`: it lies in a Caskroom, or the `dromon` cask is installed
/// (`exists` is the file system's, a test's in tests).
pub fn homebrew(bundle: &Path, exists: impl Fn(&Path) -> bool) -> bool {
    bundle.components().any(|c| c.as_os_str() == "Caskroom") || CASKROOMS.iter().any(|p| exists(Path::new(p)))
}

/// `codesign -dv`: what signs `app` (on stderr).
pub fn describe(app: &Path) -> Cmd {
    Cmd::new(CODESIGN, ["-dv".into(), "--verbose=2".into(), app.as_os_str().to_owned()])
}

/// The Developer ID team in `codesign -dv`'s output: `None` unless a Developer ID Application
/// certificate signed it and its team is a ten-character team id.
pub fn team_of(output: &str) -> Option<String> {
    let developer_id = output.lines().any(|l| l.starts_with("Authority=Developer ID Application:"));
    let team = output.lines().find_map(|l| l.strip_prefix("TeamIdentifier="))?.trim();
    (developer_id && is_team(team)).then(|| team.to_owned())
}

/// Apple's team ids: ten upper-case letters and digits.
fn is_team(team: &str) -> bool {
    team.len() == 10 && team.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// The code requirement a new app must meet: Dromon's bundle id, signed by a Developer ID
/// Application certificate (Apple's Developer ID CA, the Developer ID leaf) of `team`.
pub fn requirement(team: &str) -> String {
    format!(
        "=identifier \"{BUNDLE_ID}\" and anchor apple generic \
         and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
         and certificate leaf[field.1.2.840.113635.100.6.1.13] exists \
         and certificate leaf[subject.OU] = \"{team}\""
    )
}

/// `codesign --verify --deep --strict` of `app` against [`requirement`].
pub fn verify(app: &Path, team: &str) -> Cmd {
    let args = ["--verify".into(), "--deep".into(), "--strict".into(), "-R".into(), requirement(team).into()];
    Cmd::new(CODESIGN, args.into_iter().chain([app.as_os_str().to_owned()]))
}

/// Attach `dmg` read-only at `mount`, without Finder opening or showing it.
pub fn attach(dmg: &Path, mount: &Path) -> Cmd {
    let args = ["attach".into(), "-nobrowse".into(), "-readonly".into(), "-noautoopen".into(), "-mountpoint".into()];
    Cmd::new(HDIUTIL, args.into_iter().chain([mount.as_os_str().to_owned(), dmg.as_os_str().to_owned()]))
}

/// Detach the image at `mount` (`force`: even if something holds it open).
pub fn detach(mount: &Path, force: bool) -> Cmd {
    let force = force.then(|| "-force".into());
    Cmd::new(HDIUTIL, std::iter::once("detach".into()).chain(force).chain([mount.as_os_str().to_owned()]))
}

/// Copy the bundle `from` to `to` as macOS does (signatures, extended attributes, links).
pub fn copy(from: &Path, to: &Path) -> Cmd {
    Cmd::new(DITTO, [from.as_os_str().to_owned(), to.as_os_str().to_owned()])
}

/// The new app's copy beside `bundle` until the swap, hidden from Finder.
pub fn staging(bundle: &Path) -> PathBuf {
    bundle.with_file_name(".Dromon-update.app")
}

/// Where the running bundle moves for the swap, removed after it.
pub fn retired(bundle: &Path) -> PathBuf {
    bundle.with_file_name(".Dromon-old.app")
}

/// Puts the app in the DMG `dmg` in place of `bundle`, signed by `team`; `run` runs each
/// command (the system's, a test's). `work` holds the mount point. On an error the running
/// bundle is as it was.
pub fn install(
    run: &mut impl FnMut(&Cmd) -> Result<String, String>,
    dmg: &Path,
    bundle: &Path,
    team: &str,
    work: &Path,
) -> Result<(), String> {
    if !is_team(team) {
        return Err(format!("the running app's team id {team:?} isn't one"));
    }
    let mount = work.join("mount");
    std::fs::create_dir_all(&mount).map_err(|e| format!("couldn't make {}: {e}", mount.display()))?;
    run(&attach(dmg, &mount)).map_err(|e| format!("couldn't open the disk image: {e}"))?;
    let placed = put_in_place(run, &mount.join(APP), bundle, team);
    // Whatever happened; an image left attached is all a failure here leaves behind.
    if run(&detach(&mount, false)).is_err() {
        let _ = run(&detach(&mount, true));
    }
    placed
}

/// Checks the mounted app, copies it beside `bundle`, checks the copy, swaps it in.
fn put_in_place(
    run: &mut impl FnMut(&Cmd) -> Result<String, String>,
    new_app: &Path,
    bundle: &Path,
    team: &str,
) -> Result<(), String> {
    run(&verify(new_app, team)).map_err(|e| format!("the new app isn't signed by Dromon's developer: {e}"))?;
    let (staged, retired) = (staging(bundle), retired(bundle));
    // Leftovers of an earlier attempt that stopped half-way.
    remove(&staged)?;
    remove(&retired)?;
    let copied = run(&copy(new_app, &staged))
        .map_err(|e| format!("couldn't copy the new app beside this one: {e}"))
        .and_then(|_| run(&verify(&staged, team)).map_err(|e| format!("the copied app doesn't verify: {e}")));
    if let Err(e) = copied {
        let _ = remove(&staged);
        return Err(e);
    }
    swap(bundle, &staged, &retired)
}

/// The swap: `bundle` to `retired`, `staged` to `bundle`; the old one back if the second rename
/// fails. Then the old bundle goes (the running process keeps the files it has open).
pub fn swap(bundle: &Path, staged: &Path, retired: &Path) -> Result<(), String> {
    std::fs::rename(bundle, retired).map_err(|e| {
        let _ = remove(staged);
        format!("couldn't move this app aside for the new one: {e}")
    })?;
    if let Err(e) = std::fs::rename(staged, bundle) {
        let back = std::fs::rename(retired, bundle);
        let _ = remove(staged);
        return Err(match back {
            Ok(()) => format!("couldn't put the new app in place: {e}"),
            Err(b) => {
                format!("couldn't put the new app in place ({e}) or this one back ({b}): it is {}", retired.display())
            }
        });
    }
    let _ = remove(retired);
    Ok(())
}

/// Removes `path` (a folder and what's in it, a file or a link) if it's there.
fn remove(path: &Path) -> Result<(), String> {
    let gone = match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => Err(e),
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
    };
    gone.map_err(|e| format!("couldn't remove {}: {e}", path.display()))
}

/// How the app at `exe` was installed.
#[cfg(target_os = "macos")]
pub fn detect(exe: &Path) -> Install {
    let Some(bundle) = bundle_of(exe) else { return Install::Other };
    if homebrew(&bundle, Path::exists) {
        return Install::MacHomebrew;
    }
    let team = super::update_platform::run(&describe(&bundle)).ok().and_then(|out| team_of(&out));
    Install::MacApp { team }
}
