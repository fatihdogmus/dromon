//! "Report a Bug…" (the Help menu on macOS, the palette's "Report a bug…", an error's Report):
//! a sheet shows the user the report, and Open GitHub opens the repository's new-issue form
//! (`.github/ISSUE_TEMPLATE/bug.yml`) in the browser with it filled in; Copy report puts the same
//! text on the clipboard instead. There is no server of Dromon's: nothing leaves the machine
//! until the user submits the form on GitHub, where they can still edit it.
//!
//! The report is built here, from what the window knows, when the sheet opens ([`Draft`]); the
//! sheet shows that text and nothing else goes anywhere. It says which Dromon this is and how it
//! was installed (the updater's [`Install`]), the OS (read once at launch off the UI thread,
//! [`read_system`]), how the window draws (what the view last drew with, [`Report::saw`]), what
//! kind of state is on screen, the error it started from (if any), and the window's last
//! [`ACTIONS`] actions (kinds only) and [`EVENTS`] service events (their trace lines), kept in
//! [`Recent`] as they come (`App::act`, `App::apply_one`). Names of hosts, users, paths and files
//! are replaced by stand-ins (`redact.rs`); secrets and file contents are never in it.
//!
//! The link stays under [`URL_BUDGET`] characters: the oldest events are left out of it first,
//! whole lines, with a line saying how many; the sheet says so too. Copy report has them all.

use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::OnceLock;

use dromon_core::{ConnStatus, HostId, PromptKind};

use super::conn::{ListError, Listing};
use super::redact::{self, Redactor};
use super::remote_edit::Phase as EditPhase;
use super::update::{Install, Phase as UpdatePhase};
use super::{App, EditId, Effect, OpId, Sheet};
use crate::theme::{self, Access};

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;

/// The repository's new-issue form.
pub const NEW_ISSUE: &str = "https://github.com/fatihdogmus/dromon/issues/new";
/// The issue form the link opens (`.github/ISSUE_TEMPLATE/`).
pub const TEMPLATE: &str = "bug.yml";
/// The form's field the report goes into, by its `id`: GitHub fills a form's fields from the
/// link's query by id, so this is a contract with `bug.yml` (a test checks it).
pub const DIAGNOSTICS: &str = "diagnostics";
/// Every field id the link fills.
pub const FIELDS: &[&str] = &[DIAGNOSTICS];
/// The most characters the link may have: GitHub answers longer ones "414 URI Too Long".
pub const URL_BUDGET: usize = 7_000;
/// How many service events and actions the window keeps for a report.
pub const EVENTS: usize = 50;
pub const ACTIONS: usize = 20;

/// The window's last service events and actions, oldest first; a run of the same one is kept
/// once, with how many times it came.
#[derive(Clone, Debug, Default)]
pub struct Recent {
    events: VecDeque<(String, u32)>,
    actions: VecDeque<(&'static str, u32)>,
}

/// Adds `item` to `ring`, which keeps at most `cap`: one more of the last, or a new last.
fn push<T: PartialEq>(ring: &mut VecDeque<(T, u32)>, item: T, cap: usize) {
    if let Some((last, n)) = ring.back_mut()
        && *last == item
    {
        *n += 1;
        return;
    }
    if ring.len() == cap {
        ring.pop_front();
    }
    ring.push_back((item, 1));
}

/// "Open", or "Open ×3" for a run of three.
fn counted(what: &str, n: u32) -> String {
    if n == 1 { what.to_owned() } else { format!("{what} ×{n}") }
}

impl Recent {
    /// A service event arrived: its trace line (`service::trace_line`).
    pub fn event(&mut self, line: String) {
        push(&mut self.events, line, EVENTS);
    }

    /// The user did something: the action's kind (`Action::kind`).
    pub fn action(&mut self, kind: &'static str) {
        push(&mut self.actions, kind, ACTIONS);
    }
}

/// How the window draws, as the view last drew it: Rune hands the display's scale and the OS's
/// display preferences to the view only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    pub scale: f32,
    pub dark: bool,
    /// What the OS asks for, or'd with what's forced (`views::access`).
    pub access: Access,
}

/// The OS as read at launch (`views::report::detect`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct System {
    /// Its name and version: "macOS 15.2", "Ubuntu 24.04.1 LTS (Linux 6.8.0-49-generic)",
    /// "Windows 11 (build 26100.2605, 24H2)".
    pub os: Option<String>,
    /// Linux: the desktop and the session type, "GNOME on wayland".
    pub desktop: Option<String>,
}

static SYSTEM: OnceLock<System> = OnceLock::new();

/// What the launch read of the OS (once; later reads are ignored).
pub fn read_system(system: System) {
    let _ = SYSTEM.set(system);
}

/// This copy of Dromon and the computer it runs on, as a report says them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct About {
    pub version: String,
    /// The OS's name and version; its family (`linux`) until the launch's read is done, and in
    /// headless runs, which don't read it.
    pub os: String,
    pub arch: String,
    pub desktop: Option<String>,
}

impl About {
    /// This build, on this computer.
    pub fn here() -> About {
        let system = SYSTEM.get().cloned().unwrap_or_default();
        About {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            os: system.os.unwrap_or_else(|| std::env::consts::OS.to_owned()),
            arch: std::env::consts::ARCH.to_owned(),
            desktop: system.desktop,
        }
    }
}

/// Linux: `/etc/os-release`'s `PRETTY_NAME`, else its `NAME` and `VERSION_ID`.
pub fn os_release(text: &str) -> Option<String> {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_owned())
            .filter(|v| !v.is_empty())
    };
    field("PRETTY_NAME").or_else(|| {
        let name = field("NAME")?;
        Some(field("VERSION_ID").map_or(name.clone(), |v| format!("{name} {v}")))
    })
}

/// macOS: `/System/Library/CoreServices/SystemVersion.plist`'s `ProductVersion`, as "macOS 15.2".
pub fn mac_version(plist: &str) -> Option<String> {
    let after = plist.split_once("<key>ProductVersion</key>")?.1;
    let value = after.split_once("<string>")?.1.split_once("</string>")?.0.trim();
    (!value.is_empty()).then(|| format!("macOS {value}"))
}

/// Windows: what `reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion"` prints, as
/// "Windows 11 (build 22631.4460, 23H2)". Builds from 22000 are Windows 11 whatever
/// `ProductName` says (it says Windows 10 on both); a server keeps its product name.
pub fn windows_version(reg: &str) -> Option<String> {
    let value = |name: &str| {
        reg.lines().find_map(|l| {
            let mut words = l.split_whitespace();
            (words.next()? == name && words.next()?.starts_with("REG_")).then(|| words.collect::<Vec<_>>().join(" "))
        })
    };
    let build: u32 = value("CurrentBuild")?.parse().ok()?;
    let product = value("ProductName").unwrap_or_default();
    let name = match build {
        _ if product.contains("Server") => product,
        22_000.. => "Windows 11".to_owned(),
        _ => "Windows 10".to_owned(),
    };
    let update = value("UBR").and_then(|h| u32::from_str_radix(h.trim_start_matches("0x"), 16).ok());
    let build = update.map_or_else(|| build.to_string(), |u| format!("{build}.{u}"));
    Some(match value("DisplayVersion").filter(|v| !v.is_empty()) {
        Some(v) => format!("{name} (build {build}, {v})"),
        None => format!("{name} (build {build})"),
    })
}

/// Linux: the desktop (`XDG_CURRENT_DESKTOP`) and the session type (`XDG_SESSION_TYPE`).
pub fn desktop(current: Option<&str>, session: Option<&str>) -> Option<String> {
    fn clean(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    match (clean(current), clean(session)) {
        (Some(d), Some(s)) => Some(format!("{d} on {s}")),
        (Some(d), None) => Some(d.to_owned()),
        (None, Some(s)) => Some(format!("an unknown desktop on {s}")),
        (None, None) => None,
    }
}

/// What an error's Report is about: where the app showed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    /// A file operation's toast (a failed delete).
    Op(OpId),
    /// The toast of a file being edited whose save failed.
    Edit(EditId),
    /// The connection-failed screen of this host.
    Connection(HostId),
    /// The update toast, once installing failed.
    Update,
}

/// The sheet's buttons, and opening it (`Action::Report`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReportAction {
    /// "Report a Bug…" (`None`), or an error's Report: the sheet, with that error in it.
    Open(Option<Subject>),
    Copy,
    OpenGitHub,
    /// Cancel, Escape.
    Cancel,
}

/// One event line of a report, and how many events it stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    text: String,
    events: u32,
}

/// A report before it's rendered: everything above the events, and the events oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostics {
    head: Vec<String>,
    events: Vec<Line>,
}

impl Diagnostics {
    /// The text with the first `left_out` event lines left out (and a line saying how many
    /// events they were).
    fn render(&self, left_out: usize) -> String {
        let mut out = self.head.join("\n");
        out.push_str("\nRecent events, oldest first:");
        let omitted: u32 = self.events[..left_out].iter().map(|l| l.events).sum();
        if omitted > 0 {
            let _ = write!(out, "\n  ({omitted} older events omitted)");
        }
        for line in &self.events[left_out..] {
            let _ = write!(out, "\n  {}", line.text);
        }
        if self.events.is_empty() {
            out.push_str("\n  (none)");
        }
        out
    }

    /// The whole report.
    pub fn text(&self) -> String {
        self.render(0)
    }

    /// The new-issue link with as much of the report as fits `budget` characters: the oldest
    /// event lines go first, whole. Also how many lines it left out.
    pub fn link(&self, budget: usize) -> (String, usize) {
        let mut left_out = 0;
        loop {
            let url = issue_url(&self.render(left_out));
            if url.len() <= budget || left_out == self.events.len() {
                return (url, left_out);
            }
            left_out += 1;
        }
    }
}

/// `s` for a URL's query: RFC 3986's unreserved characters as they are, every other byte of its
/// UTF-8 as `%XX`.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// The new-issue form ([`TEMPLATE`]) with `diagnostics` in its [`DIAGNOSTICS`] field.
pub fn issue_url(diagnostics: &str) -> String {
    format!("{NEW_ISSUE}?template={}&{DIAGNOSTICS}={}", encode(TEMPLATE), encode(diagnostics))
}

/// The report on the sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// The whole report: what the sheet shows and Copy report copies.
    pub text: String,
    /// What Open GitHub opens: the form with the report in it, less its oldest events when the
    /// whole wouldn't fit [`URL_BUDGET`].
    pub url: String,
    /// How many of the oldest event lines the link leaves out.
    pub left_out: usize,
    /// Copy report was pressed: its button says so.
    pub copied: bool,
}

/// A window's bug report: what it keeps for one, and the sheet.
#[derive(Debug, Default)]
pub struct Report {
    pub recent: Recent,
    /// The sheet, while it's open.
    pub draft: Option<Draft>,
    /// This copy and the computer, when pinned (scenes, so their pictures don't change with the
    /// machine or the release); else [`About::here`].
    pub about: Option<About>,
    seen: Cell<Option<Seen>>,
}

impl Report {
    /// The view drew the window like this (`views::view`, each time it runs).
    pub fn saw(&self, seen: Seen) {
        self.seen.set(Some(seen));
    }

    pub fn is_open(&self) -> bool {
        self.draft.is_some()
    }
}

/// How this copy was installed, in words.
fn installed(install: Option<&Install>) -> &'static str {
    match install {
        None => "not known yet",
        Some(Install::MacApp { team: Some(_) }) => "macOS app, Developer ID signed",
        Some(Install::MacApp { team: None }) => "macOS app, not Developer ID signed",
        Some(Install::MacHomebrew) => "Homebrew cask",
        Some(Install::WindowsMsi) => "Windows installer (MSI)",
        Some(Install::Deb) => ".deb package",
        Some(Install::Arch) => "Arch package",
        Some(Install::Other) => "none Dromon knows (built from source, or run from a folder)",
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// What a prompt sheet asks, by kind.
fn prompt_kind(kind: &PromptKind) -> &'static str {
    match kind {
        PromptKind::HostKey { .. } => "a new host key",
        PromptKind::Password => "sign in, password",
        PromptKind::Passphrase { .. } => "sign in, key passphrase",
        PromptKind::Confirm => "a yes/no question",
        PromptKind::Notice => "a notice",
        PromptKind::Other => "sign in, a code",
    }
}

impl App {
    /// The report sheet's actions (`Action::Report`).
    pub(super) fn report_action(&mut self, action: ReportAction, fx: &mut Vec<Effect>) {
        match action {
            ReportAction::Open(subject) => {
                let diagnostics = self.diagnostics(subject.as_ref());
                let (url, left_out) = diagnostics.link(URL_BUDGET);
                self.report.draft = Some(Draft { text: diagnostics.text(), url, left_out, copied: false });
            }
            ReportAction::Copy => {
                if let Some(d) = &mut self.report.draft {
                    d.copied = true;
                    fx.push(Effect::Copy(d.text.clone()));
                }
            }
            ReportAction::OpenGitHub => {
                if let Some(d) = self.report.draft.take() {
                    fx.push(Effect::OpenUrl(d.url));
                }
            }
            ReportAction::Cancel => self.report.draft = None,
        }
    }

    /// The report of what this window knows now, about `subject` when it started from an error.
    pub fn diagnostics(&self, subject: Option<&Subject>) -> Diagnostics {
        let mut r = Redactor::default();
        let about = self.report.about.clone().unwrap_or_else(About::here);
        let mut head = vec![
            format!("Dromon {} (installed as: {})", about.version, installed(self.update.install.as_ref())),
            format!("OS: {}, {}", about.os, about.arch),
        ];
        head.extend(about.desktop.map(|d| format!("Desktop: {d}")));
        head.extend(self.display_lines());
        head.push(format!("Windows open: {}", self.windows_open()));
        head.push(format!("On screen: {}", self.on_screen().join("; ")));
        if let Some(error) = subject.and_then(|s| self.failure(s, &mut r)) {
            head.push(String::new());
            head.push("Error the report started from:".to_owned());
            head.extend(error.into_iter().map(|l| format!("  {l}")));
        }
        head.push(String::new());
        head.push("Recent actions, oldest first:".to_owned());
        let actions = &self.report.recent.actions;
        head.extend(actions.iter().map(|(kind, n)| format!("  {}", counted(kind, *n))));
        if actions.is_empty() {
            head.push("  (none)".to_owned());
        }
        let mut known: Vec<&str> = self.hosts.hosts.iter().map(|h| h.id.as_str()).collect();
        known.extend(self.conns.keys().map(HostId::as_str));
        let events = self
            .report
            .recent
            .events
            .iter()
            .map(|(line, n)| Line { text: counted(&r.trace(line, &known), *n), events: *n })
            .collect();
        Diagnostics { head, events }
    }

    /// How the window draws: the display, the settings, the display preferences.
    fn display_lines(&self) -> Vec<String> {
        let settings = format!(
            "Settings: theme {:?}, accent {}, density {:?}",
            self.settings.theme,
            theme::accent().name,
            self.settings.density
        );
        let Some(seen) = self.report.seen.get() else {
            return vec!["Display: not drawn yet".to_owned(), settings];
        };
        let a = seen.access;
        vec![
            format!("Display: scale {}, {}", seen.scale, if seen.dark { "dark" } else { "light" }),
            settings,
            format!(
                "Accessibility: reduce motion {}, increased contrast {}, reduce transparency {}",
                on_off(a.reduce_motion),
                on_off(a.increase_contrast),
                on_off(a.reduce_transparency)
            ),
        ]
    }

    /// What kind of state is on screen, and what's open over it: never a folder or a name.
    fn on_screen(&self) -> Vec<String> {
        let mut out = vec![self.content_kind()];
        let open = [
            (self.ui.preview.is_open(), "preview open"),
            (self.ui.palette.is_open(), "palette open"),
            (self.ui.quick.open, "quick connect open"),
            (self.transfers.open, "transfers open"),
            (self.files.menu.is_some(), "a menu open"),
            (self.conn().is_some_and(|c| c.filter.is_on()) && self.results().is_none(), "filter on"),
        ];
        out.extend(open.into_iter().filter(|(on, _)| *on).map(|(_, what)| what.to_owned()));
        out.extend(self.sheet_kind().map(|k| format!("sheet: {k}")));
        out
    }

    /// The window's content: the welcome state, Settings, or the host on screen's state.
    fn content_kind(&self) -> String {
        if let Some(section) = self.ui.page.settings() {
            return format!("Settings, {}", section.title());
        }
        let Some(c) = self.conn() else { return "welcome".to_owned() };
        let kind = match &c.status {
            ConnStatus::Connecting => "connecting",
            ConnStatus::NeedsInput => "signing in",
            ConnStatus::Reconnecting { .. } => "reconnecting",
            ConnStatus::Failed(_) => "connection failed",
            ConnStatus::Disconnected => "disconnected",
            ConnStatus::Ready if self.results().is_some() => "search results",
            ConnStatus::Ready => match &c.listing {
                Listing::Loading => "folder loading",
                Listing::Ready(entries) if entries.is_empty() => "empty folder",
                Listing::Ready(_) => "folder listing",
                Listing::Failed(ListError::PermissionDenied) => "folder: permission denied",
                Listing::Failed(ListError::NotFound) => "folder: not found",
                Listing::Failed(ListError::Other(_)) => "folder: couldn't be listed",
            },
        };
        kind.to_owned()
    }

    /// The sheet on screen, by kind.
    fn sheet_kind(&self) -> Option<&'static str> {
        if self.ui.confirm_quit {
            return Some("quit anyway?");
        }
        if self.update.confirming() {
            return Some("update anyway?");
        }
        match self.ui.sheets.open() {
            Some(Sheet::Ask { prompt, .. } | Sheet::Sent { prompt }) => Some(prompt_kind(&prompt.kind)),
            Some(Sheet::Log { .. }) => Some("connection log"),
            Some(Sheet::Delete { .. }) => Some("delete"),
            Some(Sheet::AddToConfig(_)) => Some("add to SSH config"),
            Some(Sheet::Properties(_)) => Some("properties"),
            None if self.conflict_on_screen().is_some() => Some("name clash"),
            None if self.edit_conflict_on_screen().is_some() => Some("edit conflict"),
            None => None,
        }
    }

    /// The error `subject` is about, as the report's lines; `None` once it's gone (dismissed).
    fn failure(&self, subject: &Subject, r: &mut Redactor) -> Option<Vec<String>> {
        let lines = match subject {
            Subject::Op(id) => {
                let op = self.ops.get(*id)?;
                let error = op.error.as_ref()?;
                vec![
                    format!("What: a file operation failed ({})", op.kind.name()),
                    format!("Kind: FileOp {}, {}", op.kind.name(), redact::error_kind(error)),
                    format!("Host: {}", r.host(&op.host)),
                    format!("Message: {}", r.error(error)),
                ]
            }
            Subject::Edit(id) => {
                let edit = self.edits.get(*id)?;
                let EditPhase::Failed { error } = &edit.phase else { return None };
                vec![
                    "What: saving a file being edited failed".to_owned(),
                    format!("Kind: RemoteEdit Upload, {}", redact::error_kind(error)),
                    format!("Host: {}", r.host(&edit.host)),
                    format!("Message: {}", r.error(error)),
                ]
            }
            Subject::Connection(host) => {
                let c = self.conns.get(host)?;
                let ConnStatus::Failed(_) = &c.status else { return None };
                vec![
                    "What: couldn't connect".to_owned(),
                    "Kind: Connection Failed".to_owned(),
                    format!("Host: {}", r.host(host)),
                    format!("Message: {}", redact::WITHHELD),
                ]
            }
            Subject::Update => {
                let UpdatePhase::InstallFailed { offer, .. } = &self.update.phase else { return None };
                vec![
                    format!("What: installing the update to {} failed", offer.version),
                    "Kind: Update InstallFailed".to_owned(),
                    format!("Message: {}", redact::WITHHELD),
                ]
            }
        };
        Some(lines)
    }
}
