//! "Show in Finder" for a downloaded file: the platform's file manager opens its folder with
//! the file selected. Runs on its own short-lived thread, so the UI never waits on it.
//!
//! - macOS: `open -R <file>`
//! - Windows: `explorer /select,"<file>"`
//! - Linux and the BSDs: the freedesktop `org.freedesktop.FileManager1.ShowItems` D-Bus call
//!   (Nautilus, Dolphin, Nemo, Thunar…); without it, `xdg-open` on the folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Shows `file` selected in its folder. Failures are ignored: there's nothing to tell the user
/// that the missing window doesn't already.
pub fn show_in_folder(file: PathBuf) {
    let _ = std::thread::Builder::new().name("dromon-reveal".into()).spawn(move || run(&file));
}

fn quiet(mut cmd: Command) -> bool {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.status().is_ok_and(|s| s.success())
}

#[cfg(target_os = "macos")]
fn run(file: &Path) {
    let mut cmd = Command::new("open");
    cmd.arg("-R").arg(file);
    quiet(cmd);
}

#[cfg(windows)]
fn run(file: &Path) {
    use std::os::windows::process::CommandExt;
    // explorer parses its own command line: `/select,` and the path must be one argument with
    // the path quoted, which Rust's own quoting would split.
    let mut cmd = Command::new("explorer");
    cmd.raw_arg(format!("/select,\"{}\"", file.display()));
    // explorer exits 1 even when it worked.
    let _ = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

#[cfg(all(unix, not(target_os = "macos")))]
fn run(file: &Path) {
    let mut dbus = Command::new("dbus-send");
    dbus.args([
        "--session",
        "--print-reply",
        "--dest=org.freedesktop.FileManager1",
        "--type=method_call",
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1.ShowItems",
    ])
    .arg(format!("array:string:{}", file_uri(file)))
    .arg("string:");
    if !quiet(dbus)
        && let Some(folder) = file.parent()
    {
        let mut open = Command::new("xdg-open");
        open.arg(folder);
        quiet(open);
    }
}

/// `file://` URI of an absolute path, every byte outside the unreserved set and `/`
/// percent-encoded.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn file_uris_escape_everything_but_plain_names() {
        assert_eq!(file_uri(Path::new("/home/u/Downloads/a b,c (1).txt")), "file:///home/u/Downloads/a%20b%2Cc%20%281%29.txt");
        assert_eq!(file_uri(Path::new("/tmp/ğ")), "file:///tmp/%C4%9F");
    }
}
