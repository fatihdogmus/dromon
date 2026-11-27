//! The bug report sheet (`state/report.rs`): its title, a line saying the details go into a new
//! GitHub issue in the browser where they can be edited before submitting, and the report
//! itself, read-only, in the mono face, scrolling and selectable: the exact text Copy report
//! copies and Open GitHub fills in. **Copy report**, **Cancel**, **Open GitHub** (the default);
//! Escape cancels. When the link can't hold every event, a note under the text says so.
//!
//! Keys: `report`, `sheet-title`, `report-explain`, `report-scroll`, `report-text`,
//! `report-note`, `report-copy`, `report-cancel`, `report-open`.
//!
//! Also the launch's read of the OS for the report ([`detect`]): a file or `reg.exe`, on a thread
//! of its own.

use rune::{El, ScrollSpec, row, text};

use super::Paint;
use super::sheets::{button, buttons, frame};
use crate::state::report::{self, ReportAction, System};
use crate::state::{Action, App};
use crate::theme::{radius, space, type_};

const WIDTH: f32 = 640.0;
/// The most the report takes before it scrolls.
const TEXT_HEIGHT: f32 = 300.0;
const TITLE: &str = "Report a Bug";
const EXPLAIN: &str = "The details below go into a new issue on GitHub, in your browser, where you can edit them \
                       before you submit. Names of hosts, users, folders and files are replaced; nothing is sent \
                       until you submit.";

fn act(a: ReportAction) -> Action {
    Action::Report(a)
}

pub fn sheet(s: &App, p: Paint) -> Option<El> {
    let draft = s.report.draft.as_ref()?;
    let t = p.t;
    let body = text(draft.text.clone())
        .key("report-text")
        .font(type_::MONO)
        .color(t.text_2)
        .w_full()
        .wrap_anywhere()
        .selectable(true);
    let scroll = rune::col()
        .key("report-scroll")
        .role("region")
        .label("Report")
        .w_full()
        .max_h(TEXT_HEIGHT)
        .padding(space::S3)
        .radius(radius::ROW)
        .fill(t.raised)
        .border(p.hair, t.border_subtle)
        .scroll(ScrollSpec::default())
        .child(body);
    let note = (draft.left_out > 0).then(|| {
        let n = draft.left_out;
        let lines = if n == 1 { "line" } else { "lines" };
        text(format!(
            "The link to GitHub leaves out the oldest {n} event {lines} to stay short enough; Copy report has all of them."
        ))
        .key("report-note")
        .font(type_::META)
        .color(t.text_3)
        .w_full()
        .wrap()
    });
    let copy = if draft.copied { "Copied" } else { "Copy report" };
    let actions = vec![
        button(p, "report-copy", copy, false, act(ReportAction::Copy)),
        row().grow(1.0),
        button(p, "report-cancel", "Cancel", false, act(ReportAction::Cancel)),
        button(p, "report-open", "Open GitHub", true, act(ReportAction::OpenGitHub)),
    ];
    Some(
        frame(p, "report", TITLE, WIDTH)
            .child(text(TITLE).key("sheet-title").font(type_::TITLE).color(t.text))
            .child(text(EXPLAIN).key("report-explain").font(type_::BODY).color(t.text_2).w_full().wrap())
            .child(scroll)
            .children(note)
            .child(buttons(actions)),
    )
}

/// Reads the OS's name and version (and on Linux the desktop) for bug reports, once, on a
/// thread of its own, so the UI thread never waits on a file or a program.
pub fn detect() {
    let started = std::thread::Builder::new().name("dromon-system".into()).spawn(|| {
        report::read_system(System { os: os(), desktop: desktop() });
    });
    if let Err(e) = started {
        eprintln!("dromon: couldn't read the OS version: {e}");
    }
}

#[cfg(target_os = "linux")]
fn os() -> Option<String> {
    let read = |path: &str| std::fs::read_to_string(path).ok();
    let release = read("/etc/os-release").or_else(|| read("/usr/lib/os-release")).and_then(|t| report::os_release(&t));
    let kernel = read("/proc/sys/kernel/osrelease").map(|k| k.trim().to_owned()).filter(|k| !k.is_empty());
    match (release, kernel) {
        (Some(r), Some(k)) => Some(format!("{r} (Linux {k})")),
        (Some(r), None) => Some(r),
        (None, Some(k)) => Some(format!("Linux {k}")),
        (None, None) => None,
    }
}

#[cfg(target_os = "macos")]
fn os() -> Option<String> {
    let plist = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok()?;
    report::mac_version(&plist)
}

/// `reg.exe` from System32 by its full path, with an argument vector (no shell), and no console
/// window flashing up.
#[cfg(windows)]
fn os() -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let root = std::env::var_os("SystemRoot")?;
    let reg = std::path::Path::new(&root).join("System32").join("reg.exe");
    let out = std::process::Command::new(reg)
        .args(["query", r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    report::windows_version(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn os() -> Option<String> {
    None
}

fn desktop() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let var = |name: &str| std::env::var(name).ok();
    report::desktop(var("XDG_CURRENT_DESKTOP").as_deref(), var("XDG_SESSION_TYPE").as_deref())
}
