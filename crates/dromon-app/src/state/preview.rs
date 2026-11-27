//! Quick preview (Space): a look at the cursor row's file without downloading it, as Finder's
//! Quick Look gives one: an image, or the start of a text file (the end of a log).
//!
//! Space on a file row opens the panel; Space or Escape closes it. While it's open it follows
//! the list's cursor (↑/↓, a click, type-to-select): each row it lands on is read anew, unless
//! the panel still holds it among the last [`CACHE`] it showed. While type-to-select is
//! collecting letters, Space types instead (`TypeSelect::collecting`).
//!
//! What a row shows ([`plan`]):
//!
//! - an image (png, jpeg, gif, webp, avif, svg, ico) up to [`IMAGE_LIMIT`]: all of it, decoded
//!   on Rune's decoder workers ([`Preview::decode`] hands the bytes over; the view never decodes);
//! - text, code, config and logs: the first [`TEXT_LIMIT`] bytes as UTF-8 (lossy), the last ones
//!   for a log (`.log`, anything under `/var/log`); a NUL byte in the first [`SNIFF`] bytes says
//!   it's binary after all;
//! - anything else (folders, special files, archives, media, programs, an image over the
//!   limit): its details, no content.
//!
//! A text's lines are found once, when it arrives, so the view lays out only the blocks of
//! [`BLOCK`] lines on screen ([`Text::block`]); a line longer than [`LONG_LINE`] characters (a
//! minified file's) is cut there.
//!
//! The panel is a machine ([`Phase`], stepped by the pure [`step`]): Closed → Loading → Shown,
//! TooLarge, Unsupported or Failed; what needs no read (a folder, a big image, a preview the
//! panel kept) skips Loading. Moving the cursor reads the next file at once, but a text read
//! for it waits (Loading → Waiting → Shown) until the cursor has rested [`REST`] seconds, on the
//! window's clock ([`tick`], from the view's poll), so ↓ held through a folder lays out no text
//! it passes. Each read has an id of this window's ([`PreviewId`], numbered from
//! the window's own range as folder size walks are): moving on cancels the read in flight, and
//! an answer for any other read is ignored.
//!
//! Previews live in memory only: never written anywhere, never logged (`DROMON_TRACE` counts
//! their bytes), all dropped when the panel closes. Their `Debug` shows sizes, never contents.

use std::collections::VecDeque;
use std::sync::Arc;

use dromon_core::{ConnStatus, Entry, EntryKind, Error, HostId, LinkTarget, RemotePath};
use rune::{Image, ImageCache};

use super::service::{Command, PreviewBytes, PreviewId, PreviewRange, ServiceEvent};
use super::transfers::Stepped;
use super::{Action, App, Effect, Shortcut};

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;

/// How much of a text file a preview reads: 256 KB.
pub const TEXT_LIMIT: u64 = 256_000;
/// The largest image a preview reads: 20 MB.
pub const IMAGE_LIMIT: u64 = 20_000_000;
/// How much of what a text preview read is looked through for a NUL byte (a binary file).
pub const SNIFF: usize = 8_192;
/// How many previews the open panel keeps, so ↑↓ back and forth doesn't read again.
pub const CACHE: usize = 8;

/// The panel's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewAction {
    /// Space: open the cursor row's preview, or close the panel.
    Toggle,
    /// Close, Escape.
    Close,
    /// Download (⌘D): the file on the panel, into the downloads folder.
    Download,
}

/// The row a preview is of, as the folder listed it when the preview came to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub host: HostId,
    pub folder: RemotePath,
    pub entry: Entry,
}

impl Target {
    pub fn path(&self) -> RemotePath {
        self.folder.join(&self.entry.name)
    }

    /// The same row: the panel stays on it.
    fn same_row(&self, other: &Target) -> bool {
        self.host == other.host && self.folder == other.folder && self.entry.name == other.entry.name
    }

    /// The same file, unchanged since: a kept preview of it still holds.
    fn same_file(&self, other: &Target) -> bool {
        self.same_row(other) && self.entry.size == other.entry.size && self.entry.mtime == other.entry.mtime
    }
}

/// How a row is previewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Read it whole and decode it.
    Image,
    /// Read this much of it as text.
    Text(PreviewRange),
    /// An image larger than [`IMAGE_LIMIT`]: its details.
    TooLarge,
    /// Not something a preview shows: its details.
    Unsupported,
}

impl Plan {
    fn range(self) -> Option<PreviewRange> {
        match self {
            Plan::Image => Some(PreviewRange::Head(IMAGE_LIMIT)),
            Plan::Text(range) => Some(range),
            Plan::TooLarge | Plan::Unsupported => None,
        }
    }
}

/// Image formats a preview shows (AVIF only where Rune is built with its `avif` codec).
const IMAGES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "ico"];

/// Names that say a file isn't text (archives, media, documents, programs, fonts, databases),
/// so it isn't read only to be found binary.
const BINARIES: &[&str] = &[
    "zip", "gz", "tgz", "xz", "bz2", "zst", "tar", "7z", "rar", "jar", "deb", "rpm", "dmg", "iso", "pdf", "mp3", "m4a",
    "aac", "flac", "wav", "ogg", "opus", "mp4", "m4v", "mov", "mkv", "webm", "avi", "heic", "tif", "tiff", "bmp",
    "psd", "bin", "exe", "so", "dylib", "dll", "o", "a", "wasm", "class", "pyc", "sqlite", "db", "woff", "woff2",
    "ttf", "otf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
];

/// A name's extension, lowercased: "" for none (and for a dotfile's whole name).
fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Whether `name` in `folder` is a log, whose end is what's new: a `.log` file, or anything
/// under `/var/log`.
pub fn is_log(folder: &RemotePath, name: &str) -> bool {
    extension(name) == "log" || folder.starts_with(&RemotePath::new("/var/log"))
}

/// How the row `e` of `folder` is previewed, from what its listing says.
pub fn plan(folder: &RemotePath, e: &Entry) -> Plan {
    let file = matches!(e.kind, EntryKind::File | EntryKind::Symlink { target: Some(LinkTarget::File) });
    if !file {
        return Plan::Unsupported;
    }
    let ext = extension(&e.name);
    if IMAGES.contains(&ext.as_str()) {
        // A link's size is the link's own, not its target's: the read finds out.
        let too_large = e.kind == EntryKind::File && e.size > IMAGE_LIMIT;
        return if too_large { Plan::TooLarge } else { Plan::Image };
    }
    if BINARIES.contains(&ext.as_str()) {
        return Plan::Unsupported;
    }
    Plan::Text(if is_log(folder, &e.name) { PreviewRange::Tail(TEXT_LIMIT) } else { PreviewRange::Head(TEXT_LIMIT) })
}

/// Where a text preview was cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    /// "Showing the first 256 KB of …".
    Head,
    /// "Showing the last 256 KB of …".
    Tail,
}

/// How many characters of a line a text preview shows: a longer one (a minified file's) is cut
/// there, " …" after it.
pub const LONG_LINE: usize = 2_000;
/// How many lines the view lays out as one block ([`Text::block`]).
pub const BLOCK: usize = 16;

/// A text preview. The view lays out only the blocks of lines on screen, by [`Text::block`].
#[derive(Clone, PartialEq)]
pub struct Text {
    /// The lines shown, separated by `\n`, without a last line break, each at most
    /// [`LONG_LINE`] characters (and the " …" of the cut).
    pub text: String,
    /// Where each line starts in `text`, in bytes; the first at 0.
    pub starts: Vec<u32>,
    /// The widest line's [`columns`].
    pub columns: usize,
    /// Whether the lines are numbered: they start at the file's first line. A cut tail isn't:
    /// where it starts in the file isn't known without reading all of it.
    pub numbered: bool,
    pub cut: Option<Cut>,
    /// Some line was longer than [`LONG_LINE`], and is cut.
    pub long: bool,
    /// The file's size.
    pub total: u64,
}

impl Text {
    /// How many lines there are (an empty text has one, empty).
    pub fn lines(&self) -> usize {
        self.starts.len()
    }

    /// How many blocks of [`BLOCK`] lines there are, the last maybe fewer.
    pub fn blocks(&self) -> usize {
        self.lines().div_ceil(BLOCK)
    }

    /// The lines of block `b` (from 0), by index.
    pub fn block_lines(&self, b: usize) -> std::ops::Range<usize> {
        b * BLOCK..((b + 1) * BLOCK).min(self.lines())
    }

    /// Block `b`'s lines, separated by `\n`.
    pub fn block(&self, b: usize) -> &str {
        let lines = self.block_lines(b);
        let start = self.starts[lines.start] as usize;
        let end = self.starts.get(lines.end).map_or(self.text.len(), |next| *next as usize - 1);
        &self.text[start..end]
    }

    /// Block `b`'s line numbers (from 1), one a line.
    pub fn numbers(&self, b: usize) -> String {
        self.block_lines(b).map(|i| (i + 1).to_string()).collect::<Vec<_>>().join("\n")
    }
}

impl std::fmt::Debug for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Text(<{} bytes, {} lines>, cut: {:?}, total: {})",
            self.text.len(),
            self.lines(),
            self.cut,
            self.total
        )
    }
}

/// How wide `line` is in the mono face, in columns: one an ASCII character, two any other (a
/// wide one takes two, a narrow one leaves room to spare), a tab to the next multiple of 8. The
/// content of a text preview is as wide as its widest line, laid out or not.
pub fn columns(line: &str) -> usize {
    line.chars().fold(0, |at, c| match c {
        '\t' => (at / 8 + 1) * 8,
        c if c.is_ascii() => at + 1,
        _ => at + 2,
    })
}

/// `text` (lines separated by `\n`) with each line over [`LONG_LINE`] characters cut there,
/// " …" after it; whether any was.
fn cap_lines(text: String) -> (String, bool) {
    // Where `line` is cut: past its LONG_LINE-th character, if it has more (no line of fewer
    // bytes does).
    let cut = |line: &str| (line.len() > LONG_LINE).then(|| line.char_indices().nth(LONG_LINE)).flatten();
    if !text.split('\n').any(|line| cut(line).is_some()) {
        return (text, false);
    }
    let mut capped = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            capped.push('\n');
        }
        match cut(line) {
            Some((at, _)) => {
                capped.push_str(&line[..at]);
                capped.push_str(" …");
            }
            None => capped.push_str(line),
        }
    }
    (capped, true)
}

/// Where each line of `text` (lines separated by `\n`) starts, and the widest one's columns.
fn line_starts(text: &str) -> (Vec<u32>, usize) {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(at, _)| at as u32 + 1));
    let widest = text.split('\n').map(columns).max().unwrap_or(0);
    (starts, widest)
}

/// An image preview: the bytes read, and Rune's decoding of them ([`Preview::decode`]).
#[derive(Clone)]
pub struct Picture {
    pub bytes: PreviewBytes,
    pub image: Option<Image>,
    /// The view was told the decode ended ([`App::preview_settled`]).
    settled: bool,
}

impl PartialEq for Picture {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({:?}, decoding: {})", self.bytes, self.image.is_some())
    }
}

impl Picture {
    fn new(bytes: PreviewBytes) -> Picture {
        Picture { bytes, image: None, settled: false }
    }

    /// Its size in pixels once decoded: an SVG's own size, a raster's pixels.
    pub fn size(&self) -> Option<(u32, u32)> {
        let [w, h] = self.image.as_ref()?.intrinsic_size()?;
        Some((w.round() as u32, h.round() as u32))
    }

    /// Whether Rune couldn't decode it (a damaged file, a format this build lacks).
    pub fn failed(&self) -> bool {
        self.image.as_ref().is_some_and(|i| i.error().is_some())
    }
}

/// What a preview shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Text(Text),
    Image(Picture),
}

/// The text `bytes` read as `range` of a file of `total` bytes show; `None` when a NUL byte in
/// the first [`SNIFF`] bytes says the file is binary.
pub fn text(range: PreviewRange, bytes: &[u8], total: u64) -> Option<Text> {
    if bytes[..bytes.len().min(SNIFF)].contains(&0) {
        return None;
    }
    let (offset, _) = range.span(total);
    let tail = offset > 0;
    let head = !tail && (bytes.len() as u64) < total;
    let mut shown = bytes;
    if tail && let Some(at) = shown.iter().position(|&b| b == b'\n').filter(|at| at + 1 < shown.len()) {
        // A cut tail starts mid-line (maybe mid-character): it shows from the next line on.
        shown = &shown[at + 1..];
    }
    let mut text = String::from_utf8_lossy(shown).into_owned();
    if head && shown.last().is_some_and(|b| *b >= 0x80) && text.ends_with('\u{FFFD}') {
        // A cut head may end mid-character: that's the cut, not a bad byte.
        text.pop();
    }
    if text.contains('\r') {
        text = text.replace("\r\n", "\n");
    }
    // A last line break ends the last line; it doesn't start another.
    if text.ends_with('\n') {
        text.pop();
    }
    let cut = if tail {
        Some(Cut::Tail)
    } else if head {
        Some(Cut::Head)
    } else {
        None
    };
    let (text, long) = cap_lines(text);
    let (starts, columns) = line_starts(&text);
    Some(Text { text, starts, columns, numbered: !tail, cut, long, total })
}

/// How long the cursor rests on a row before a text read for it shows, in seconds: moving
/// through files with ↓ held reads them but lays none of them out.
pub const REST: f64 = 0.15;

/// Whether the cursor has rested on the panel's row, so that a text read for it may show.
/// Only texts wait: images and details show as they come, and so does what Space opened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rest {
    /// It has, or the panel opened on this row.
    Rested,
    /// It just moved here; the next tick starts the wait (actions have no clock).
    Moved,
    /// It rests until this time, on the window's clock.
    Until(f64),
}

impl Rest {
    /// The rest at `clock`.
    fn at(self, clock: f64) -> Rest {
        match self {
            Rest::Moved => Rest::Until(clock + REST),
            Rest::Until(until) if clock >= until => Rest::Rested,
            rest => rest,
        }
    }

    /// When it's over, if it's waiting.
    fn until(self) -> Option<f64> {
        match self {
            Rest::Until(until) => Some(until),
            Rest::Rested | Rest::Moved => None,
        }
    }
}

/// Where the panel stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Closed,
    /// The read `id` of `target` is under way; `rest` is the cursor's.
    Loading {
        target: Target,
        id: PreviewId,
        plan: Plan,
        rest: Rest,
    },
    /// `target`'s text came while the cursor was still moving: it shows (and is laid out) once
    /// the cursor has rested. It looks like Loading meanwhile.
    Waiting {
        target: Target,
        body: Body,
        rest: Rest,
    },
    Shown {
        target: Target,
        body: Body,
    },
    /// An image over [`IMAGE_LIMIT`].
    TooLarge {
        target: Target,
    },
    /// Nothing a preview shows (a folder, a binary file): its details.
    Unsupported {
        target: Target,
    },
    /// The read failed.
    Failed {
        target: Target,
        error: Error,
    },
}

impl Phase {
    /// The row on the panel; `None` while it's closed.
    pub fn target(&self) -> Option<&Target> {
        match self {
            Phase::Closed => None,
            Phase::Loading { target, .. }
            | Phase::Waiting { target, .. }
            | Phase::Shown { target, .. }
            | Phase::TooLarge { target }
            | Phase::Unsupported { target }
            | Phase::Failed { target, .. } => Some(target),
        }
    }
}

/// What happens to the panel.
#[derive(Debug)]
pub enum Event {
    /// Show `target`: `id` names its read, if it needs one; `kept` is the panel's earlier
    /// preview of it, if it holds one; `moved`: the cursor moved to it (a text read for it
    /// waits until the cursor rests), rather than Space opening the panel on it.
    Show {
        target: Box<Target>,
        id: PreviewId,
        kept: Option<Body>,
        moved: bool,
    },
    /// The read `id` brought `bytes` of a file of `total` bytes.
    Read {
        id: PreviewId,
        bytes: PreviewBytes,
        total: u64,
    },
    Failed {
        id: PreviewId,
        error: Error,
    },
    Close,
}

/// What the panel's owner does after a step.
#[derive(Debug, PartialEq)]
pub enum Fx {
    Read {
        id: PreviewId,
        host: HostId,
        path: RemotePath,
        range: PreviewRange,
    },
    /// Stop the read in flight: the panel moved on, or closed.
    Cancel {
        id: PreviewId,
        host: HostId,
    },
    /// Keep what was shown, for coming back to it.
    Keep(Target, Body),
}

/// What `bytes` of `target`, read as `plan`, show; a text waits for the cursor's `rest`.
fn arrived(target: Target, plan: Plan, bytes: PreviewBytes, total: u64, rest: Rest) -> Phase {
    match plan {
        Plan::Image if total > IMAGE_LIMIT => Phase::TooLarge { target },
        Plan::Image => Phase::Shown { target, body: Body::Image(Picture::new(bytes)) },
        Plan::Text(range) => match text(range, &bytes.0, total) {
            Some(text) if rest == Rest::Rested => Phase::Shown { target, body: Body::Text(text) },
            Some(text) => Phase::Waiting { target, body: Body::Text(text), rest },
            None => Phase::Unsupported { target },
        },
        Plan::TooLarge | Plan::Unsupported => Phase::Unsupported { target },
    }
}

/// The phase leaves for another: a read in flight stops, a preview shown (or waiting to) is
/// kept (unless the panel closes: then everything goes).
fn leave(phase: &mut Phase, keep: bool, fx: &mut Vec<Fx>) {
    match std::mem::take(phase) {
        Phase::Loading { target, id, .. } => fx.push(Fx::Cancel { id, host: target.host }),
        Phase::Shown { target, body } | Phase::Waiting { target, body, .. } if keep => {
            fx.push(Fx::Keep(target, body));
        }
        _ => {}
    }
}

/// Steps the cursor's rest at `clock` (the view's poll, every frame): a text waiting for it
/// shows once it has rested. Says whether what shows changed, and when the next step is due.
pub fn tick(phase: &mut Phase, clock: f64) -> Stepped {
    match phase {
        Phase::Loading { rest, .. } => {
            *rest = rest.at(clock);
            Stepped { changed: false, wake: rest.until() }
        }
        Phase::Waiting { rest, .. } => {
            *rest = rest.at(clock);
            if *rest != Rest::Rested {
                return Stepped { changed: false, wake: rest.until() };
            }
            if let Phase::Waiting { target, body, .. } = std::mem::take(phase) {
                *phase = Phase::Shown { target, body };
            }
            Stepped { changed: true, wake: None }
        }
        _ => Stepped::default(),
    }
}

/// Steps `phase` by `ev`: what its owner does next. A read's answer counts only while it's the
/// read the panel waits for; anything else is stale and ignored.
pub fn step(phase: &mut Phase, ev: Event) -> Vec<Fx> {
    let mut fx = Vec::new();
    match ev {
        Event::Show { target, id, kept, moved } => {
            let target = *target;
            leave(phase, true, &mut fx);
            *phase = match (kept, plan(&target.folder, &target.entry)) {
                (Some(body), _) => Phase::Shown { target, body },
                (None, Plan::TooLarge) => Phase::TooLarge { target },
                (None, Plan::Unsupported) => Phase::Unsupported { target },
                (None, plan) => {
                    if let Some(range) = plan.range() {
                        fx.push(Fx::Read { id, host: target.host.clone(), path: target.path(), range });
                    }
                    let rest = if moved { Rest::Moved } else { Rest::Rested };
                    Phase::Loading { target, id, plan, rest }
                }
            };
        }
        Event::Read { id, bytes, total } => {
            if matches!(phase, Phase::Loading { id: waiting, .. } if *waiting == id)
                && let Phase::Loading { target, plan, rest, .. } = std::mem::take(phase)
            {
                *phase = arrived(target, plan, bytes, total, rest);
            }
        }
        Event::Failed { id, error } => {
            if matches!(phase, Phase::Loading { id: waiting, .. } if *waiting == id)
                && let Phase::Loading { target, .. } = std::mem::take(phase)
            {
                *phase = Phase::Failed { target, error };
            }
        }
        Event::Close => leave(phase, false, &mut fx),
    }
    fx
}

/// One window's preview panel: its machine, the previews it keeps, its read ids, and Rune's
/// decoder for its images.
pub struct Preview {
    pub phase: Phase,
    /// The last [`CACHE`] previews shown, oldest first.
    kept: VecDeque<(Target, Body)>,
    last: u64,
    images: ImageCache,
}

impl Default for Preview {
    fn default() -> Self {
        // No waker until `AppSpec::setup` gives one (scenes and tests wait for their images).
        Preview { phase: Phase::Closed, kept: VecDeque::new(), last: 0, images: ImageCache::new(Arc::new(|| {})) }
    }
}

impl Preview {
    pub fn is_open(&self) -> bool {
        self.phase != Phase::Closed
    }

    /// Rune's waker (`AppSpec::setup`): an image that finished decoding wakes the window.
    pub fn set_waker(&mut self, waker: rune::Waker) {
        self.images = ImageCache::new(waker);
    }

    /// How many previews the panel keeps now.
    pub fn kept(&self) -> usize {
        self.kept.len()
    }

    fn take_kept(&mut self, t: &Target) -> Option<Body> {
        let at = self.kept.iter().position(|(k, _)| k.same_file(t))?;
        self.kept.remove(at).map(|(_, body)| body)
    }

    fn keep(&mut self, t: Target, body: Body) {
        self.kept.retain(|(k, _)| !k.same_row(&t));
        self.kept.push_back((t, body));
        while self.kept.len() > CACHE {
            self.kept.pop_front();
        }
    }

    /// Hands an image on screen that isn't decoding yet to Rune's decoder workers.
    fn decode(&mut self) {
        if let Phase::Shown { body: Body::Image(p), .. } = &mut self.phase
            && p.image.is_none()
        {
            p.image = Some(self.images.load(p.bytes.0.clone()));
        }
    }

    /// Waits until the image on screen is decoded: a headless picture (a scene) is the same on
    /// every run. The app never waits: the decoder wakes it.
    pub fn wait_for_image(&self) {
        if let Phase::Shown { body: Body::Image(Picture { image: Some(image), .. }), .. } = &self.phase {
            let _ = image.wait();
        }
    }
}

impl App {
    /// The row the panel follows: the cursor's, in the folder on screen while the list shows.
    fn preview_row(&self) -> Option<Target> {
        let c = self.conn()?;
        if self.on_settings() || matches!(c.status, ConnStatus::Failed(_) | ConnStatus::Disconnected) {
            return None;
        }
        let entry = c.find(c.selection.cursor()?)?.clone();
        Some(Target { host: c.host.clone(), folder: c.cwd.clone(), entry })
    }

    /// The panel's actions of [`App::act`].
    pub(super) fn preview_action(&mut self, action: PreviewAction, fx: &mut Vec<Effect>) {
        match action {
            PreviewAction::Toggle if self.ui.preview.is_open() => self.close_preview(fx),
            PreviewAction::Toggle => {
                // A folder, or no row: nothing to look at.
                if let Some(target) = self.preview_row().filter(|t| !t.entry.kind.is_dir_like()) {
                    self.show_preview(target, false);
                    fx.push(Effect::FocusPreview);
                }
            }
            PreviewAction::Close => self.close_preview(fx),
            PreviewAction::Download => {
                let Some(t) = self.ui.preview.phase.target() else { return };
                if self.conn().is_some_and(|c| c.host == t.host && c.cwd == t.folder) {
                    let names = vec![t.entry.name.clone()];
                    self.download(&names);
                }
            }
        }
    }

    /// While the panel is open: Space and Escape close it (Space types while type-to-select is
    /// collecting), ⌘D downloads its file, and the arrows and typing move the cursor it follows;
    /// the list's other shortcuts wait. `None`: the key goes on as usual.
    pub(super) fn preview_shortcut(&mut self, sc: &Shortcut, now: f64) -> Option<Vec<Effect>> {
        let action = match sc {
            Shortcut::Move { .. } | Shortcut::Page { .. } | Shortcut::End { .. } | Shortcut::Type(_) => return None,
            Shortcut::Preview if self.ui.type_select.collecting(now) => return None,
            Shortcut::Preview | Shortcut::Escape => Action::Preview(PreviewAction::Close),
            Shortcut::Download => Action::Preview(PreviewAction::Download),
            _ => return Some(Vec::new()),
        };
        Some(self.act(action))
    }

    /// The panel on `target`: `moved`, the cursor moved there (rather than Space opening it).
    fn show_preview(&mut self, target: Target, moved: bool) {
        self.ui.preview.last += 1;
        let id = PreviewId((u64::from(self.window.0) << 32) + self.ui.preview.last);
        let kept = self.ui.preview.take_kept(&target);
        self.preview_step(Event::Show { target: Box::new(target), id, kept, moved });
    }

    fn close_preview(&mut self, fx: &mut Vec<Effect>) {
        if self.ui.preview.is_open() {
            self.preview_step(Event::Close);
            self.ui.preview.kept.clear();
            fx.push(Effect::FocusList);
        }
    }

    /// Steps the panel and carries out what it asks.
    fn preview_step(&mut self, ev: Event) {
        for fx in step(&mut self.ui.preview.phase, ev) {
            match fx {
                Fx::Read { id, host, path, range } => self.send(Command::Preview { id, host, path, range }),
                Fx::Cancel { id, host } => self.send(Command::CancelPreview { id, host }),
                Fx::Keep(target, body) => self.ui.preview.keep(target, body),
            }
        }
        self.ui.preview.decode();
    }

    /// A preview read's answer from the service.
    pub(super) fn preview_event(&mut self, ev: ServiceEvent) {
        let (host, ev) = match ev {
            ServiceEvent::Preview { id, host, bytes, total } => (host, Event::Read { id, bytes, total }),
            ServiceEvent::PreviewFailed { id, host, error } => (host, Event::Failed { id, error }),
            _ => return,
        };
        if self.ui.preview.phase.target().is_some_and(|t| t.host == host) {
            self.preview_step(ev);
        }
    }

    /// After every action and event: the open panel follows the cursor's row, and closes when
    /// there's none (the folder or host changed, the list went away).
    pub(super) fn sync_preview(&mut self) {
        let Some(open) = self.ui.preview.phase.target() else { return };
        match self.preview_row() {
            Some(row) if row.same_row(open) => {}
            Some(row) => self.show_preview(row, true),
            None => {
                self.preview_step(Event::Close);
                self.ui.preview.kept.clear();
            }
        }
    }

    /// The view's poll, every frame at `clock`: the cursor's rest steps (a text waiting for it
    /// may show), and an image that finished decoding (or failed to) since the view last ran
    /// has it run again to say its size. When the next step is due, if one is.
    pub fn preview_tick(&mut self, clock: f64) -> Stepped {
        let mut stepped = tick(&mut self.ui.preview.phase, clock);
        stepped.changed |= self.preview_settled();
        stepped
    }

    fn preview_settled(&mut self) -> bool {
        match &mut self.ui.preview.phase {
            Phase::Shown { body: Body::Image(p), .. }
                if !p.settled && p.image.as_ref().is_some_and(|i| i.is_ready()) =>
            {
                p.settled = true;
                true
            }
            _ => false,
        }
    }
}
