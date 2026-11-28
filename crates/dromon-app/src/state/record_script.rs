//! A recording's script (Rune's format, `docs/ui.md` "Script Events and Element Keys") and its
//! steps in words. Each step becomes the input that makes it, at element keys, whatever input the
//! user used: a row's Open is a double-click on `rows/<name>`, Rename is F2, Delete and
//! Properties go through the row's menu, so a replay works on every platform. Names are the
//! stand-ins the fixture uses (the same [`Redactor`]).
//!
//! The replay window is the repro's (1080 × 680, the default density), so a row is only tapped
//! once the list shows it: the script follows where the list is scrolled and, for a row below
//! the fold, scrolls it into view with the wheel first. Where the user's window was somewhere the
//! last step didn't leave it (an event moved it, the recording started there), the script goes
//! there first: the host's row, then ⌘L and the folder's path.

use dromon_core::{HostId, RemotePath, SortKey};

use super::fixture::{CODE, Model, SECRET, WRONG, json};
use super::{Answer, Folder, Place, Recording, Row, Step, Taken};
use crate::state::redact::Redactor;
use crate::state::{Density, Sort};
use crate::theme::space;

/// The list's height in the replay's window (1080 × 680), and the part of it a row is sure to
/// show in.
const VIEW: f32 = 584.0;
const VIEW_SAFE: f32 = 560.0;

/// The steps, with a click on a row followed by its double-click's Open folded into the Open
/// (a double-click's first press selects the row).
pub fn compact(steps: &[Taken]) -> Vec<Taken> {
    let mut out: Vec<Taken> = Vec::new();
    for t in steps {
        if let Step::Open(row) = &t.step {
            while out.last().is_some_and(|l| {
                matches!(&l.step, Step::Click { row: r, extend: false, toggle: false } if r == row) && l.at == t.at
            }) {
                out.pop();
            }
        }
        out.push(t.clone());
    }
    out
}

/// The script being written.
struct Script<'a> {
    rec: &'a Recording,
    model: &'a mut Model,
    r: &'a mut Redactor,
    events: Vec<String>,
    t: f64,
    /// Where the replay's window is.
    here: Option<Place>,
    /// How far the list is scrolled, when known.
    offset: Option<f32>,
    sort: Sort,
    hidden: bool,
    /// The name field is up: for a rename of this row, or a new folder (`None`).
    editing: Option<Option<Row>>,
}

impl Script<'_> {
    /// An event `gap` seconds after the last; `body` is its fields after `t`.
    fn ev(&mut self, gap: f64, body: String) {
        self.t += gap;
        self.events.push(format!("{{\"t\":{:.2},{body}}}", self.t));
    }

    fn tap(&mut self, key: &str) {
        self.ev(0.8, format!("\"type\":\"tap\",\"key\":{}", json(key)));
    }

    fn key(&mut self, name: &str, cmd: bool, shift: bool) {
        let mut body = format!("\"type\":\"key\",\"name\":{}", json(name));
        if cmd {
            body.push_str(",\"cmd\":true");
        }
        if shift {
            body.push_str(",\"shift\":true");
        }
        self.ev(0.6, body);
    }

    fn type_text(&mut self, text: &str) {
        self.ev(0.6, format!("\"type\":\"type\",\"text\":{}", json(text)));
        self.t += 0.035 * text.chars().count() as f64;
    }

    fn menu(&mut self, key: &str, item: &str) {
        self.ev(0.8, format!("\"type\":\"secondary-click\",\"key\":{}", json(key)));
        self.tap(item);
    }

    fn host_key(&mut self, host: &HostId) -> String {
        format!("host-{}", self.r.host(host))
    }

    fn row_key(&mut self, row: &Row) -> String {
        format!("rows/{}", self.r.name(&row.name, row.dir))
    }

    /// The replay moved to `folder` of `host`: the list is at its top.
    fn moved_to(&mut self, host: &HostId, folder: RemotePath) {
        self.here = Some(Place { host: host.clone(), folder: Some(folder) });
        self.offset = Some(0.0);
    }

    /// Goes to `want` unless the replay is there.
    fn ensure(&mut self, want: &Place) {
        if self.here.as_ref().map(|p| &p.host) != Some(&want.host) {
            let key = self.host_key(&want.host);
            self.tap(&key);
            self.t += 0.6;
            let home = self.model.home(&want.host);
            self.moved_to(&want.host, home);
        }
        if let Some(folder) = &want.folder
            && self.here.as_ref().and_then(|p| p.folder.as_ref()) != Some(folder)
        {
            self.go_to(&want.host, folder);
        }
    }

    /// ⌘L, the folder's path, Return.
    fn go_to(&mut self, host: &HostId, folder: &RemotePath) {
        self.key("l", true, false);
        let path = self.r.remote_path(folder);
        self.type_text(&path);
        self.key("Enter", false, false);
        self.moved_to(host, folder.clone());
    }

    /// Scrolls the list so `row` shows, if it may not.
    fn reveal(&mut self, row: &Row) {
        let Some(Place { host, folder: Some(folder) }) = self.here.clone() else { return };
        let name = self.r.name(&row.name, row.dir);
        let Some(rows) = self.model.rows(&host, &folder, self.sort, self.hidden) else { return };
        let Some(i) = rows.iter().position(|n| *n == name) else { return };
        let h = Density::default().row();
        let top = space::S1 + i as f32 * h;
        let bottom = top + h;
        if self.offset.is_some_and(|off| top >= off && bottom <= off + VIEW_SAFE) {
            return;
        }
        if self.offset != Some(0.0) {
            self.ev(0.5, "\"type\":\"wheel\",\"key\":\"list\",\"dx\":0,\"dy\":100000".to_owned());
        }
        self.offset = Some(0.0);
        if bottom > VIEW_SAFE {
            let max = (2.0 * space::S1 + rows.len() as f32 * h - VIEW).max(0.0);
            let to = (top - VIEW_SAFE / 3.0).clamp(0.0, max).round();
            self.ev(0.3, format!("\"type\":\"wheel\",\"key\":\"list\",\"dx\":0,\"dy\":{}", -to));
            self.offset = Some(to);
        }
    }

    /// The first step that works in a folder sets the sort and hidden files the user had.
    fn tune(&mut self) {
        let want = self.rec.sort;
        if want.key == SortKey::Name {
            if want != self.sort {
                self.tap("col-Name");
                self.offset = None;
            }
        } else {
            let column = column(want.key);
            self.tap(column);
            if want.dir != self.sort.toggled(want.key).dir {
                self.tap(column);
            }
            self.offset = None;
        }
        self.sort = want;
        if !self.rec.hidden {
            self.key(".", true, true);
            self.hidden = false;
            self.offset = None;
        }
    }
}

/// A sort column's header key.
fn column(key: SortKey) -> &'static str {
    match key {
        SortKey::Name => "col-Name",
        SortKey::Size => "col-Size",
        SortKey::Modified => "col-Modified",
    }
}

/// Whether a step works on the folder on screen, so the replay must be there first.
fn in_folder(step: &Step) -> bool {
    !matches!(
        step,
        Step::Connect(_)
            | Step::Answer { .. }
            | Step::Decline(_)
            | Step::Dismiss(_)
            | Step::Escape
            | Step::Named { .. }
            | Step::EditCancel
            | Step::ConfirmDelete(_)
            | Step::Disconnect(_)
            | Step::Transfers
            | Step::NotReplayed(_)
    )
}

impl Script<'_> {
    fn sign_in(&mut self, kind: Answer, wrong: bool) {
        match kind {
            Answer::Yes => self.tap("sheet-submit"),
            Answer::Password | Answer::Passphrase | Answer::Code => {
                let text = match (wrong, kind) {
                    (true, _) => WRONG,
                    (false, Answer::Code) => CODE,
                    _ => SECRET,
                };
                self.type_text(text);
                self.key("Enter", false, false);
            }
        }
    }

    fn click(&mut self, row: &Row, extend: bool, toggle: bool) {
        self.reveal(row);
        let hold = match (extend, toggle) {
            (true, _) => Some("shift"),
            (false, true) => Some("cmd"),
            _ => None,
        };
        if let Some(h) = hold {
            self.ev(0.4, format!("\"hold\":\"{h}\""));
        }
        let key = self.row_key(row);
        self.ev(0.4, format!("\"type\":\"tap\",\"key\":{}", json(&key)));
        if hold.is_some() {
            self.ev(0.3, "\"hold\":\"\"".to_owned());
        }
    }

    fn arrows(&mut self, delta: isize, extend: bool) {
        let name = match delta {
            16 => "PageDown",
            -16 => "PageUp",
            d if d > 0 => "Down",
            _ => "Up",
        };
        let times = if delta.unsigned_abs() == 16 { 1 } else { delta.unsigned_abs().max(1) };
        for _ in 0..times {
            self.key(name, false, extend);
        }
        self.offset = None;
    }

    /// The name field's Return: everything in it replaced by the stand-in.
    fn named(&mut self, name: &str, dir: bool) {
        let stand_in = self.r.name(name, dir);
        self.key("a", true, false);
        self.type_text(&stand_in);
        self.key("Enter", false, false);
        if let Some(Place { host, folder: Some(folder) }) = self.here.clone() {
            match self.editing.take() {
                Some(Some(row)) => {
                    let from = self.r.name(&row.name, row.dir);
                    self.model.rename(&host, &folder, &from, &stand_in);
                }
                Some(None) => self.model.add_dir(&host, &folder, &stand_in),
                None => {}
            }
        }
        self.offset = None;
    }

    fn deleted(&mut self, names: &[String]) {
        self.tap("delete-confirm");
        let Some(Place { host, folder: Some(folder) }) = self.here.clone() else { return };
        let Some(rows) = self.model.rows(&host, &folder, self.sort, true) else { return };
        // Each name as the folder's row has it: a folder's stand-in or a file's.
        let gone: Vec<String> = names
            .iter()
            .flat_map(|n| [self.r.name(n, true), self.r.name(n, false)])
            .filter(|n| rows.contains(n))
            .collect();
        self.model.remove(&host, &folder, &gone);
        self.offset = None;
    }

    fn open(&mut self, row: &Row) {
        self.reveal(row);
        let key = self.row_key(row);
        self.ev(0.8, format!("\"type\":\"double-click\",\"key\":{}", json(&key)));
    }

    /// The row's menu, and its item `item`.
    fn row_menu(&mut self, row: &Row, item: &str) {
        self.reveal(row);
        let key = self.row_key(row);
        self.menu(&key, item);
    }

    /// The input that makes `t`'s step.
    fn input(&mut self, t: &Taken) {
        match &t.step {
            Step::Connect(host) => {
                let key = self.host_key(host);
                self.tap(&key);
                self.t += 0.4;
            }
            Step::Answer { kind, wrong, .. } => self.sign_in(*kind, *wrong),
            Step::Decline(_) => self.tap("sheet-no"),
            Step::Dismiss(_) | Step::Escape | Step::EditCancel | Step::Preview(None) => {
                self.key("Escape", false, false);
            }
            Step::Open(row) => self.open(row),
            Step::Up => self.key("Up", true, false),
            Step::Back => self.tap("tb-back"),
            Step::Forward => self.tap("tb-forward"),
            Step::GoTo(path) => {
                if let Some(host) = t.at.as_ref().map(|p| p.host.clone()) {
                    self.go_to(&host, path);
                }
            }
            Step::Click { row, extend, toggle } => self.click(row, *extend, *toggle),
            Step::TypeSelect(Some(row)) => self.click(row, false, false),
            Step::Move { delta, extend } => self.arrows(*delta, *extend),
            Step::End { last, extend } => {
                self.key(if *last { "End" } else { "Home" }, false, *extend);
                self.offset = None;
            }
            _ => self.input_more(t),
        }
    }

    fn input_more(&mut self, t: &Taken) {
        match &t.step {
            Step::SelectAll => self.key("a", true, false),
            Step::Sort(key) => {
                self.tap(column(*key));
                self.sort = self.sort.toggled(*key);
                self.offset = None;
            }
            Step::Hidden => {
                self.key(".", true, true);
                self.hidden = !self.hidden;
                self.offset = None;
            }
            Step::Refresh => self.key("r", true, false),
            Step::NewFolder => {
                self.key("n", true, true);
                self.editing = Some(None);
            }
            Step::Rename(row) => {
                self.key("F2", false, false);
                self.editing = Some(Some(row.clone()));
            }
            Step::Named { name, dir } => self.named(name, *dir),
            Step::Delete(row) => self.row_menu(row, "ctx-delete"),
            Step::ConfirmDelete(names) => self.deleted(names),
            Step::Properties(row) => self.row_menu(row, "ctx-properties"),
            Step::Preview(Some(_)) => self.key(" ", false, false),
            Step::Download => self.key("d", true, false),
            Step::OpenLog => self.tap("status-conn"),
            Step::Disconnect(host) => {
                let key = self.host_key(host);
                self.menu(&key, "ctx-host-disconnect");
            }
            Step::Transfers => self.tap("tb-transfers"),
            _ => {}
        }
    }

    /// After `t`: where the replay is now.
    fn after(&mut self, t: &Taken) {
        match &t.after {
            Some(Place { host, folder: Some(folder) }) => {
                if self.here.as_ref().and_then(|p| p.folder.as_ref()) != Some(folder) {
                    self.moved_to(host, folder.clone());
                }
            }
            Some(Place { host, folder: None }) => {
                let home = self.model.home(host);
                self.moved_to(host, home);
            }
            None => self.here = None,
        }
    }
}

/// The steps in words.
struct Words<'a> {
    rec: &'a Recording,
    r: &'a mut Redactor,
}

impl Words<'_> {
    fn row(&mut self, row: &Row) -> String {
        self.r.name(&row.name, row.dir)
    }

    /// What a folder opened at `t` answered, when it failed.
    fn outcome(&self, t: &Taken, row: &Row) -> &'static str {
        let Some(Place { host, folder: Some(folder) }) = &t.at else { return "" };
        match self.rec.servers.get(host).and_then(|s| s.folders.get(&folder.join(&row.name))) {
            Some(Folder::Denied) => " (permission denied)",
            Some(Folder::Missing) => " (not found)",
            Some(Folder::Lost) => " (the connection was lost)",
            _ => "",
        }
    }

    fn say(&mut self, t: &Taken) -> String {
        match &t.step {
            Step::Connect(h) => {
                let failed = self.rec.servers.get(h).is_some_and(|s| s.failed && !s.opened);
                format!("Opened {}{}", self.r.host(h), if failed { " (it couldn't connect)" } else { "" })
            }
            Step::Answer { kind, wrong, .. } => {
                let what = match kind {
                    Answer::Password => "Answered the password",
                    Answer::Passphrase => "Answered the key's passphrase",
                    Answer::Code => "Answered the code",
                    Answer::Yes => return "Answered Trust or Yes".to_owned(),
                };
                let how = if *wrong { "; the server refused it" } else { "" };
                format!("{what} (what you typed isn't recorded{how})")
            }
            Step::Decline(_) => "Answered No".to_owned(),
            Step::Dismiss(Some(_)) => "Cancelled signing in".to_owned(),
            Step::Dismiss(None) => "Closed the sheet".to_owned(),
            Step::Open(row) if row.dir => format!("Opened {}{}", self.row(row), self.outcome(t, row)),
            Step::Open(row) => format!("Opened {} (it downloads)", self.row(row)),
            Step::Up => "Went up a folder".to_owned(),
            Step::Back => "Went back".to_owned(),
            Step::Forward => "Went forward".to_owned(),
            Step::GoTo(path) => format!("Went to {}", self.r.remote_path(path)),
            Step::Click { row, extend: true, .. } => format!("Selected up to {} (Shift)", self.row(row)),
            Step::Click { row, toggle: true, .. } => format!("Added or removed {} (⌘ or Ctrl)", self.row(row)),
            Step::Click { row, .. } => format!("Selected {}", self.row(row)),
            Step::TypeSelect(Some(row)) => format!("Typed to select {}", self.row(row)),
            Step::TypeSelect(None) => "Typed to select (nothing matched)".to_owned(),
            _ => self.say_more(&t.step),
        }
    }

    fn say_more(&mut self, step: &Step) -> String {
        match step {
            Step::Move { delta, extend } => {
                let way = if *delta > 0 { "down" } else { "up" };
                let shift = if *extend { " (Shift)" } else { "" };
                format!("Moved {way} {}{shift}", delta.unsigned_abs())
            }
            Step::End { last: true, .. } => "Went to the last row".to_owned(),
            Step::End { last: false, .. } => "Went to the first row".to_owned(),
            Step::SelectAll => "Selected all".to_owned(),
            Step::Sort(key) => format!("Sorted by {key:?}"),
            Step::Hidden => "Showed or hid hidden files".to_owned(),
            Step::Escape => "Pressed Escape".to_owned(),
            Step::Refresh => "Refreshed the folder".to_owned(),
            Step::NewFolder => "Started a new folder".to_owned(),
            Step::Rename(row) => format!("Started renaming {}", self.row(row)),
            Step::Named { name, dir } => format!("Named it {}", self.r.name(name, *dir)),
            Step::EditCancel => "Cancelled the name".to_owned(),
            Step::Delete(_) => "Asked to delete the selection".to_owned(),
            Step::ConfirmDelete(names) => format!("Deleted {}", crate::fmt::items(names.len())),
            Step::Properties(row) => format!("Opened the properties of {}", self.row(row)),
            Step::Preview(Some(row)) => format!("Previewed {}", self.row(row)),
            Step::Preview(None) => "Closed the preview".to_owned(),
            Step::Download => "Downloaded the selection".to_owned(),
            Step::OpenLog => "Opened the connection log".to_owned(),
            Step::Disconnect(h) => format!("Disconnected {}", self.r.host(h)),
            Step::Transfers => "Showed or hid the transfers".to_owned(),
            Step::NotReplayed(what) => format!("{what} (not replayed)"),
            _ => String::new(),
        }
    }
}

/// The recording's script in its fenced block, and its steps in words, the script's description
/// naming the Dromon `version` it was recorded with.
pub fn build(
    rec: &Recording,
    steps: &[Taken],
    model: &mut Model,
    r: &mut Redactor,
    version: &str,
) -> (String, Vec<String>) {
    let mut s = Script {
        rec,
        model,
        r,
        events: Vec::new(),
        t: 0.0,
        here: None,
        offset: None,
        sort: Sort::default(),
        hidden: true,
        editing: None,
    };
    let mut tuned = false;
    if let Some(start) = &rec.start {
        s.ensure(start);
    }
    for t in steps {
        if in_folder(&t.step)
            && let Some(at) = &t.at
        {
            s.ensure(at);
            if !tuned {
                s.tune();
                tuned = true;
            }
        }
        s.input(t);
        s.after(t);
    }
    if let Some(last) = steps.last().and_then(|t| t.after.clone()) {
        s.ensure(&last);
    }
    s.ev(1.5, "\"type\":\"mark\",\"name\":\"end\"".to_owned());
    let n = steps.len();
    let description = format!(
        "Recorded steps (Dromon {version}): {n} {}, names replaced. Replay them on the fixture recorded with them.",
        if n == 1 { "step" } else { "steps" }
    );
    let script = format!("{{\"description\": {},\n\"events\": [\n{}\n]}}", json(&description), s.events.join(",\n"));
    let mut words = Words { rec, r: s.r };
    let mut lines = Vec::new();
    if let Some(Place { host, folder }) = &rec.start {
        let folder = folder.as_ref().map(|f| words.r.remote_path(f)).unwrap_or_default();
        lines.push(format!("Started with {} open, in {folder}", words.r.host(host)));
    }
    for (i, t) in steps.iter().enumerate() {
        lines.push(format!("{}. {}", i + 1, words.say(t)));
    }
    (format!("```json script\n{script}\n```"), lines)
}
