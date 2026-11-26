//! The Properties sheet (⌘I on macOS, Alt+Return elsewhere, the row menu's Properties): what
//! the listing knows about one row (name, kind, location, size, modified, permissions, owner),
//! read-only, and for a folder its size on demand.
//!
//! A folder's size isn't in its listing: Calculate walks it on the service thread
//! (`Command::FolderSize`, `service/sizes.rs`). The total is counted as Finder counts a folder,
//! not as `du` does: the bytes of the files, symlinks (counted, never followed) and other
//! non-folders inside it; "N files, M folders" counts what's inside, the folder itself not
//! among the folders. Folders the walk couldn't read are left out and counted in a note.
//!
//! The size is a small machine ([`SizePhase`], stepped by the pure [`step`]): Idle (a
//! Calculate button) → Calculating (live count) → Done or Failed (Recalculate starts again).
//! Each walk has an id of this window's ([`SizeId`], numbered from the window's own range as
//! file operations are), so an answer for a walk that isn't the sheet's (an older one, or one
//! whose sheet closed) changes nothing. However the sheet goes away (Close, Escape, a prompt
//! taking its place, a disconnect), [`App::sync_properties`] notices after the action or event
//! and cancels the walk still running.

use dromon_core::{Entry, EntryKind, Error, HostId, LinkTarget, RemotePath};

use super::service::{Command, ServiceEvent, SizeId, SizeTotal};
use super::App;

#[cfg(test)]
#[path = "properties_tests.rs"]
mod tests;

/// Where the size of a folder on the sheet stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SizePhase {
    /// Not measured: the Calculate button.
    #[default]
    Idle,
    /// The walk `id` is under way and has found `found` so far.
    Calculating { id: SizeId, found: SizeTotal },
    /// The walk went through the folder (some subfolders may have been unreadable).
    Done { total: SizeTotal },
    /// The walk stopped: what it had counted, and why.
    Failed { total: SizeTotal, error: Error },
}

/// What happens to a folder's size.
#[derive(Clone, Debug, PartialEq)]
pub enum SizeEvent {
    /// Calculate (or Recalculate) sent the walk `id`.
    Start(SizeId),
    Progress(SizeId, SizeTotal),
    Done(SizeId, SizeTotal),
    Failed(SizeId, SizeTotal, Error),
}

/// Steps `phase` by `ev`; whether it changed. A walk starts unless one is running; the walk's
/// answers count only while it's the one running (anything else is stale and ignored).
pub fn step(phase: &mut SizePhase, ev: SizeEvent) -> bool {
    let running = match phase {
        SizePhase::Calculating { id, .. } => Some(*id),
        _ => None,
    };
    let next = match ev {
        SizeEvent::Start(id) if running.is_none() => SizePhase::Calculating { id, found: SizeTotal::default() },
        SizeEvent::Progress(id, found) if running == Some(id) => SizePhase::Calculating { id, found },
        SizeEvent::Done(id, total) if running == Some(id) => SizePhase::Done { total },
        SizeEvent::Failed(id, total, error) if running == Some(id) => SizePhase::Failed { total, error },
        _ => return false,
    };
    *phase = next;
    true
}

/// The sheet's state: the row as it was listed when the sheet opened, and its size.
#[derive(Clone, Debug, PartialEq)]
pub struct PropertiesView {
    pub host: HostId,
    /// The folder the row is in (the sheet's Location).
    pub folder: RemotePath,
    pub entry: Entry,
    /// A folder's size (unused for anything else, whose size is the listing's).
    pub size: SizePhase,
}

impl PropertiesView {
    pub fn new(host: HostId, folder: RemotePath, entry: Entry) -> PropertiesView {
        PropertiesView { host, folder, entry, size: SizePhase::Idle }
    }

    /// Whether the size is measured by walking it: a folder (a link to one is a link, and
    /// shows its own size).
    pub fn is_folder(&self) -> bool {
        self.entry.kind == EntryKind::Dir
    }

    /// The walk running for this sheet, if any.
    pub fn walking(&self) -> Option<SizeId> {
        match self.size {
            SizePhase::Calculating { id, .. } => Some(id),
            _ => None,
        }
    }
}

/// "Folder", "File", "Symbolic link to a folder", "Socket"…: the sheet's Kind.
pub fn kind_text(e: &Entry) -> &'static str {
    match e.kind {
        EntryKind::Dir => "Folder",
        EntryKind::File => "File",
        EntryKind::Symlink { target: Some(LinkTarget::Dir) } => "Symbolic link to a folder",
        EntryKind::Symlink { target: Some(LinkTarget::File) } => "Symbolic link to a file",
        EntryKind::Symlink { target: Some(LinkTarget::Other) } => "Symbolic link",
        EntryKind::Symlink { target: None } => "Symbolic link (target missing)",
        EntryKind::Other => match e.mode.map(|m| m & 0o170_000) {
            Some(0o140_000) => "Socket",
            Some(0o010_000) => "Named pipe",
            Some(0o020_000) => "Character device",
            Some(0o060_000) => "Block device",
            _ => "Special file",
        },
    }
}

/// `rwxr-xr-x (755)`, as `ls -l` writes the permission bits (setuid, setgid and sticky as
/// `s`/`S` and `t`/`T`), then the octal; "—" when the server sent no mode.
pub fn permissions_text(mode: Option<u32>) -> String {
    let Some(mode) = mode else { return "—".into() };
    let bits = mode & 0o7777;
    let mut s = String::with_capacity(9);
    for (shift, special, mark) in [(6, 0o4000, 's'), (3, 0o2000, 's'), (0, 0o1000, 't')] {
        let rwx = (bits >> shift) & 0o7;
        s.push(if rwx & 4 != 0 { 'r' } else { '-' });
        s.push(if rwx & 2 != 0 { 'w' } else { '-' });
        s.push(match (rwx & 1 != 0, bits & special != 0) {
            (true, true) => mark,
            (false, true) => mark.to_ascii_uppercase(),
            (true, false) => 'x',
            (false, false) => '-',
        });
    }
    let octal = if bits > 0o777 { format!("{bits:04o}") } else { format!("{bits:03o}") };
    format!("{s} ({octal})")
}

/// `alice:staff`: the names the server gave (or its numeric ids); "—" when it gave neither.
pub fn owner_text(e: &Entry) -> String {
    match (&e.owner, &e.group) {
        (Some(o), Some(g)) => format!("{o}:{g}"),
        (Some(o), None) => o.clone(),
        (None, Some(g)) => format!(":{g}"),
        (None, None) => "—".into(),
    }
}

/// "12 files, 3 folders" (with 1 file, 1 folder in the singular).
pub fn counts_text(t: &SizeTotal) -> String {
    let n = |count: u64, one: &str, many: &str| format!("{} {}", crate::fmt::thousands(count), if count == 1 { one } else { many });
    format!("{}, {}", n(t.files, "file", "files"), n(t.folders, "folder", "folders"))
}

/// "2 folders couldn't be read", when some couldn't.
pub fn unreadable_text(t: &SizeTotal) -> Option<String> {
    match t.unreadable {
        0 => None,
        1 => Some("1 folder couldn't be read".into()),
        n => Some(format!("{} folders couldn't be read", crate::fmt::thousands(n))),
    }
}

/// "Calculating… 1,204 items".
pub fn calculating_text(found: &SizeTotal) -> String {
    match found.items() {
        1 => "Calculating… 1 item".into(),
        n => format!("Calculating… {} items", crate::fmt::thousands(n)),
    }
}

/// This window's folder size walks: the next id, and the walk the open sheet waits for.
#[derive(Clone, Debug, Default)]
pub struct Sizes {
    last: u64,
    /// The walk sent and not answered to its end yet, and its host.
    running: Option<(HostId, SizeId)>,
}

impl App {
    /// The one selected row on screen, which Properties shows; `None` with none or several.
    pub fn properties_row(&self) -> Option<&Entry> {
        let c = self.conn()?;
        if c.selection.len() != 1 {
            return None;
        }
        c.selection.iter().next().and_then(|name| c.find(name))
    }

    /// Opens the sheet for the one selected row, unless another sheet is up.
    pub(super) fn open_properties(&mut self) {
        let Some(e) = self.properties_row().cloned() else { return };
        let Some(c) = self.conn() else { return };
        let view = PropertiesView::new(c.host.clone(), c.cwd.clone(), e);
        self.ui.sheets.open_properties(view);
    }

    /// Whether the sheet's Calculate can start a walk: the sheet is a folder's, nothing is
    /// walking, and its host is connected.
    pub fn can_calculate(&self) -> bool {
        let Some(v) = self.ui.sheets.properties() else { return false };
        v.is_folder() && v.walking().is_none() && self.conns.get(&v.host).is_some_and(|c| c.interactive())
    }

    /// Calculate (or Recalculate): asks the service to walk the sheet's folder.
    pub(super) fn calculate(&mut self) {
        if !self.can_calculate() {
            return;
        }
        self.files.sizes.last += 1;
        let id = SizeId((u64::from(self.window.0) << 32) + self.files.sizes.last);
        let Some(v) = self.ui.sheets.properties_mut() else { return };
        step(&mut v.size, SizeEvent::Start(id));
        let (host, path) = (v.host.clone(), v.folder.join(&v.entry.name));
        // Recorded before sending: a fixture answers inside the send.
        self.files.sizes.running = Some((host.clone(), id));
        self.send(Command::FolderSize { id, host, path });
    }

    /// A folder size event from the service: it steps the open sheet's size if it's that
    /// sheet's walk on that host; anything else is stale.
    pub(super) fn size_event(&mut self, ev: ServiceEvent) {
        let (host, ev) = match ev {
            ServiceEvent::FolderSizeProgress { id, host, total } => (host, SizeEvent::Progress(id, total)),
            ServiceEvent::FolderSizeDone { id, host, total } => (host, SizeEvent::Done(id, total)),
            ServiceEvent::FolderSizeFailed { id, host, total, error } => (host, SizeEvent::Failed(id, total, error)),
            _ => return,
        };
        let ended = match &ev {
            SizeEvent::Done(id, _) | SizeEvent::Failed(id, ..) => Some(*id),
            SizeEvent::Start(_) | SizeEvent::Progress(..) => None,
        };
        if let Some(id) = ended
            && self.files.sizes.running.as_ref().is_some_and(|(h, r)| *h == host && *r == id)
        {
            self.files.sizes.running = None;
        }
        if let Some(v) = self.ui.sheets.properties_mut().filter(|v| v.host == host) {
            step(&mut v.size, ev);
        }
    }

    /// After every action and event: a walk whose sheet went away (or moved on) is cancelled.
    pub(super) fn sync_properties(&mut self) {
        let Some((host, id)) = self.files.sizes.running.clone() else { return };
        if self.ui.sheets.properties().and_then(PropertiesView::walking) == Some(id) {
            return;
        }
        self.files.sizes.running = None;
        self.send(Command::CancelFolderSize { id, host });
    }
}
