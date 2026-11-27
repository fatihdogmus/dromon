//! The quick preview: its machine (open, follow, close, stale answers), what each kind of row
//! shows (head or tail, cut, binary, too large, unsupported), the panel's kept previews, and
//! Space against type-to-select. On the demo fixture (which answers at once) or a [`Recorder`]
//! to read what's sent and drive the answers by hand.

use dromon_core::{Entry, EntryKind, Error, HostId, LinkTarget, RemotePath};

use super::*;
use crate::fixture;
use crate::state::search::DEBOUNCE;
use crate::state::{Recorder, SearchAction, ServiceEvent};

fn prod() -> HostId {
    HostId("prod".into())
}

const SITE: &str = "/var/www/site";

fn demo() -> App {
    let mut s = fixture::app(fixture::demo());
    s.act(Action::SelectHost(prod()));
    s
}

fn recorded() -> (App, Recorder) {
    let mut s = demo();
    let rec = Recorder::default();
    s.backend = Box::new(rec.clone());
    (s, rec)
}

fn click(s: &mut App, name: &str) {
    s.act(Action::Click { name: name.into(), extend: false, toggle: false });
}

fn space(s: &mut App, now: f64) -> Vec<Effect> {
    s.shortcut(Shortcut::Preview, now)
}

/// The reads (and cancels) sent so far.
fn reads(rec: &Recorder) -> Vec<Command> {
    rec.sent().into_iter().filter(|c| matches!(c, Command::Preview { .. } | Command::CancelPreview { .. })).collect()
}

fn read_id(c: &Command) -> PreviewId {
    match c {
        Command::Preview { id, .. } | Command::CancelPreview { id, .. } => *id,
        _ => panic!("not a read: {c:?}"),
    }
}

fn on_panel(s: &App) -> Option<&str> {
    s.ui.preview.phase.target().map(|t| t.entry.name.as_str())
}

fn shown_text(s: &App) -> &Text {
    match &s.ui.preview.phase {
        Phase::Shown { body: Body::Text(t), .. } => t,
        other => panic!("no text on the panel: {other:?}"),
    }
}

fn entry(name: &str, kind: EntryKind, size: u64) -> Entry {
    Entry { name: name.into(), kind, size, mtime: None, mode: None, owner: None, group: None }
}

fn file(name: &str, size: u64) -> Entry {
    entry(name, EntryKind::File, size)
}

fn target(folder: &str, e: Entry) -> Target {
    Target { host: prod(), folder: RemotePath::new(folder), entry: e }
}

fn bytes(b: &[u8]) -> PreviewBytes {
    PreviewBytes(b.into())
}

/// Space opened the panel on `t` (nothing waits for the cursor).
fn show(t: &Target, id: u64, kept: Option<Body>) -> Event {
    Event::Show { target: Box::new(t.clone()), id: PreviewId(id), kept, moved: false }
}

/// The cursor moved to `t` while the panel was open.
fn moved_to(t: &Target, id: u64) -> Event {
    Event::Show { target: Box::new(t.clone()), id: PreviewId(id), kept: None, moved: true }
}

/// The cursor rests: frames go by, past [`REST`].
fn rest(s: &mut App) {
    s.preview_tick(100.0);
    s.preview_tick(100.0 + REST);
}

#[test]
fn the_machine_opens_reads_shows_and_closes() {
    let mut phase = Phase::Closed;
    let t = target(SITE, file("index.html", 10));
    let fx = step(&mut phase, show(&t, 1, None));
    let read = Fx::Read {
        id: PreviewId(1),
        host: prod(),
        path: RemotePath::new("/var/www/site/index.html"),
        range: PreviewRange::Head(TEXT_LIMIT),
    };
    assert_eq!(fx, [read]);
    assert!(matches!(phase, Phase::Loading { id: PreviewId(1), .. }));
    // Another read's answer changes nothing; this one's shows.
    assert!(step(&mut phase, Event::Read { id: PreviewId(9), bytes: bytes(b"no"), total: 2 }).is_empty());
    assert!(matches!(phase, Phase::Loading { .. }));
    assert!(step(&mut phase, Event::Read { id: PreviewId(1), bytes: bytes(b"<p>hi</p>\n"), total: 10 }).is_empty());
    let Phase::Shown { body: Body::Text(text), .. } = &phase else { panic!("{phase:?}") };
    assert_eq!((text.text.as_str(), text.lines(), text.numbered, text.cut), ("<p>hi</p>", 1, true, None));
    // A late answer for it, after it showed, is stale too.
    step(&mut phase, Event::Failed { id: PreviewId(1), error: Error::ConnectionLost });
    assert!(matches!(phase, Phase::Shown { .. }));
    // Closing keeps nothing.
    assert!(step(&mut phase, Event::Close).is_empty());
    assert_eq!(phase, Phase::Closed);
}

#[test]
fn moving_on_cancels_the_read_in_flight_and_keeps_what_was_shown() {
    let mut phase = Phase::Closed;
    let a = target(SITE, file("a.txt", 3));
    let b = target(SITE, file("b.txt", 3));
    step(&mut phase, show(&a, 1, None));
    let fx = step(&mut phase, show(&b, 2, None));
    assert_eq!(fx[0], Fx::Cancel { id: PreviewId(1), host: prod() }, "{fx:?}");
    assert!(matches!(fx[1], Fx::Read { id: PreviewId(2), .. }));
    // a's answer arrives after all: ignored.
    step(&mut phase, Event::Read { id: PreviewId(1), bytes: bytes(b"aaa"), total: 3 });
    assert!(matches!(&phase, Phase::Loading { target, .. } if target.entry.name == "b.txt"));
    step(&mut phase, Event::Read { id: PreviewId(2), bytes: bytes(b"bbb"), total: 3 });
    // Moving on from a shown preview keeps it; a kept one shows without a read.
    let fx = step(&mut phase, show(&a, 3, None));
    assert!(matches!(&fx[0], Fx::Keep(t, Body::Text(text)) if *t == b && text.text == "bbb"), "{fx:?}");
    let body = Body::Text(text(PreviewRange::Head(TEXT_LIMIT), b"kept", 4).unwrap());
    let fx = step(&mut phase, show(&a, 4, Some(body.clone())));
    assert_eq!(fx, [Fx::Cancel { id: PreviewId(3), host: prod() }]);
    assert!(matches!(&phase, Phase::Shown { body: b, .. } if *b == body));
    // A failed read says so.
    step(&mut phase, show(&target(SITE, file("c.txt", 3)), 5, None));
    step(&mut phase, Event::Failed { id: PreviewId(5), error: Error::PermissionDenied(RemotePath::new("/c.txt")) });
    assert!(matches!(phase, Phase::Failed { error: Error::PermissionDenied(_), .. }));
}

#[test]
fn logs_are_read_from_their_end_and_other_text_from_its_start() {
    let head = Plan::Text(PreviewRange::Head(TEXT_LIMIT));
    let tail = Plan::Text(PreviewRange::Tail(TEXT_LIMIT));
    assert_eq!(plan(&RemotePath::new(SITE), &file("index.html", 14_812)), head);
    assert_eq!(plan(&RemotePath::new(SITE), &file("server.log", 4_812_331)), tail);
    assert_eq!(plan(&RemotePath::new(SITE), &file("SERVER.LOG", 1)), tail);
    assert_eq!(plan(&RemotePath::new("/var/log"), &file("syslog", 1)), tail, "anything under /var/log");
    assert_eq!(plan(&RemotePath::new("/var/log/nginx"), &file("access.log.1", 1)), tail);
    assert_eq!(plan(&RemotePath::new("/var/logs"), &file("syslog", 1)), head, "a folder named alike isn't");
    assert_eq!(plan(&RemotePath::new(SITE), &file(".env", 1)), head, "dotfiles are text");
    let link = entry("notes", EntryKind::Symlink { target: Some(LinkTarget::File) }, 9);
    assert_eq!(plan(&RemotePath::new(SITE), &link), head, "a link to a file reads as the file");
}

#[test]
fn binaries_large_images_folders_and_other_kinds_show_their_details() {
    let site = RemotePath::new(SITE);
    assert_eq!(plan(&site, &file("photo.JPG", IMAGE_LIMIT)), Plan::Image);
    assert_eq!(plan(&site, &file("favicon.ico", 4_286)), Plan::Image);
    assert_eq!(plan(&site, &file("huge.png", IMAGE_LIMIT + 1)), Plan::TooLarge);
    // A link's size is its own: the read tells.
    let link = entry("huge.png", EntryKind::Symlink { target: Some(LinkTarget::File) }, 30);
    assert_eq!(plan(&site, &link), Plan::Image);
    assert_eq!(plan(&site, &file("backup.tar.gz", 18_204_116)), Plan::Unsupported);
    assert_eq!(plan(&site, &file("talk.mp4", 1)), Plan::Unsupported);
    assert_eq!(plan(&site, &entry("assets", EntryKind::Dir, 4096)), Plan::Unsupported);
    assert_eq!(
        plan(&site, &entry("current", EntryKind::Symlink { target: Some(LinkTarget::Dir) }, 30)),
        Plan::Unsupported
    );
    assert_eq!(plan(&site, &entry("deploy.sock", EntryKind::Other, 0)), Plan::Unsupported);
    // Text that's binary after all: a NUL in its first 8 KB.
    let mut b = vec![b'a'; SNIFF + 10];
    b[SNIFF - 1] = 0;
    assert!(text(PreviewRange::Head(TEXT_LIMIT), &b, b.len() as u64).is_none());
    b[SNIFF - 1] = b'a';
    b[SNIFF + 1] = 0;
    assert!(text(PreviewRange::Head(TEXT_LIMIT), &b, b.len() as u64).is_some(), "only the first 8 KB is looked at");
    // An image whose read says it's larger than the limit (a link's target).
    let mut phase = Phase::Closed;
    step(&mut phase, show(&target(SITE, link), 1, None));
    step(&mut phase, Event::Read { id: PreviewId(1), bytes: bytes(b"x"), total: IMAGE_LIMIT + 1 });
    assert!(matches!(phase, Phase::TooLarge { .. }), "{phase:?}");
    // Binary text, found by the read.
    step(&mut phase, show(&target(SITE, file(".DS_Store", 4)), 2, None));
    step(&mut phase, Event::Read { id: PreviewId(2), bytes: bytes(b"B\0d1"), total: 4 });
    assert!(matches!(phase, Phase::Unsupported { .. }), "{phase:?}");
}

#[test]
fn text_says_where_it_was_cut_and_numbers_lines_it_knows_the_place_of() {
    // A head that was cut: numbered from 1, no half character at the end.
    let t = text(PreviewRange::Head(9), "ab\ncd é".as_bytes()[..7].as_ref(), 100).unwrap();
    assert_eq!((t.text.as_str(), t.lines(), t.numbered, t.cut), ("ab\ncd ", 2, true, Some(Cut::Head)));
    // A tail that was cut starts at the next whole line, and has no numbers.
    let t = text(PreviewRange::Tail(8), b"ne 1\nline 2\n", 100).unwrap();
    assert_eq!((t.text.as_str(), t.numbered, t.cut, t.total), ("line 2", false, Some(Cut::Tail), 100));
    // A log shorter than the limit is all there: numbered, not cut; Windows line ends read as one.
    let t = text(PreviewRange::Tail(TEXT_LIMIT), b"a\r\nb\r\n", 6).unwrap();
    assert_eq!((t.text.as_str(), t.lines(), t.numbered, t.cut), ("a\nb", 2, true, None));
    // Not UTF-8: replacement characters, not an error.
    let t = text(PreviewRange::Head(TEXT_LIMIT), b"caf\xe9!", 5).unwrap();
    assert_eq!(t.text, "caf\u{FFFD}!");
    // What a text preview's Debug shows: sizes, never contents.
    assert!(!format!("{t:?}").contains("caf"), "{t:?}");
}

#[test]
fn space_opens_the_cursor_files_preview_and_space_or_escape_closes_it() {
    let mut s = demo();
    assert!(space(&mut s, 10.0).is_empty(), "no cursor row: nothing");
    assert!(!s.ui.preview.is_open());
    click(&mut s, "assets");
    assert!(space(&mut s, 10.0).is_empty(), "a folder: nothing");
    assert!(!s.ui.preview.is_open());
    click(&mut s, "index.html");
    assert_eq!(space(&mut s, 10.0), [Effect::FocusPreview]);
    assert_eq!(on_panel(&s), Some("index.html"));
    let t = shown_text(&s);
    assert!(t.text.starts_with("<!doctype html>"), "the fixture's made-up page");
    assert_eq!((t.cut, t.total), (None, 14_812));
    assert_eq!(space(&mut s, 11.0), [Effect::FocusList]);
    assert!(!s.ui.preview.is_open());
    space(&mut s, 12.0);
    assert_eq!(s.shortcut(Shortcut::Escape, 13.0), [Effect::FocusList]);
    assert!(!s.ui.preview.is_open());
    assert!(s.conn().unwrap().selection.contains("index.html"), "Escape closed the panel, the selection stays");
}

/// `text` searched for in subfolders (⌘⇧F, typed, the pause after it).
fn searched(s: &mut App, text: &str) {
    s.act(Action::Search(SearchAction::Open));
    s.ui.filter_edit = rune::TextEdit::new(text);
    s.act(Action::Search(SearchAction::Typed { text: text.into(), now: 10.0 }));
    s.act(Action::Search(SearchAction::Tick(10.0 + DEBOUNCE)));
}

#[test]
fn search_results_have_no_preview_and_a_search_closes_an_open_one() {
    let mut s = demo();
    click(&mut s, "index.html");
    space(&mut s, 1.0);
    assert_eq!(on_panel(&s), Some("index.html"));
    // The palette's "Search in subfolders…" while the panel is open: the results take the list.
    searched(&mut s, "*.json");
    assert!(s.results().is_some_and(|r| !r.found.is_empty()), "the search found something");
    assert!(!s.ui.preview.is_open(), "the folder's row isn't on screen any more");
    // Space among the results previews nothing: the folder's own cursor is hidden under them.
    assert!(space(&mut s, 20.0).is_empty());
    assert!(!s.ui.preview.is_open());
}

#[test]
fn the_preview_follows_the_cursor_and_closes_when_the_row_goes() {
    let mut s = demo();
    click(&mut s, "index.html");
    space(&mut s, 1.0);
    s.shortcut(Shortcut::Move { delta: 1, extend: false }, 2.0);
    assert_eq!(on_panel(&s), Some("package-lock.json"));
    rest(&mut s);
    assert_eq!(shown_text(&s).cut, Some(Cut::Head), "412 KB: the first 256 KB");
    click(&mut s, "server.log");
    assert_eq!(on_panel(&s), Some("server.log"));
    rest(&mut s);
    let t = shown_text(&s);
    assert_eq!((t.cut, t.numbered), (Some(Cut::Tail), false), "a log's last 256 KB");
    click(&mut s, "assets");
    assert!(matches!(s.ui.preview.phase, Phase::Unsupported { .. }), "a folder on the panel: its details");
    click(&mut s, "favicon.ico");
    assert!(matches!(&s.ui.preview.phase, Phase::Shown { body: Body::Image(p), .. } if p.image.is_some()));
    // The list's other shortcuts wait while it's open.
    assert!(s.shortcut(Shortcut::SelectAll, 3.0).is_empty());
    assert_eq!(s.conn().unwrap().selection.len(), 1);
    // Into a folder: no row is the cursor's any more.
    s.act(Action::Open("assets".into()));
    assert!(!s.ui.preview.is_open());
    assert_eq!(s.ui.preview.kept(), 0, "everything it held goes with it");
}

#[test]
fn moving_on_in_the_app_cancels_the_read_and_ignores_its_late_answer() {
    let (mut s, rec) = recorded();
    click(&mut s, "index.html");
    space(&mut s, 1.0);
    click(&mut s, "robots.txt");
    let sent = reads(&rec);
    assert!(matches!(&sent[..], [Command::Preview { .. }, Command::CancelPreview { .. }, Command::Preview { .. }]));
    let (first, second) = (read_id(&sent[0]), read_id(&sent[2]));
    assert_eq!(read_id(&sent[1]), first);
    assert_ne!(first, second);
    s.apply(ServiceEvent::Preview { id: first, host: prod(), bytes: bytes(b"<html>"), total: 6 });
    assert!(matches!(&s.ui.preview.phase, Phase::Loading { target, .. } if target.entry.name == "robots.txt"));
    s.apply(ServiceEvent::Preview { id: second, host: prod(), bytes: bytes(b"User-agent: *\n"), total: 68 });
    rest(&mut s);
    assert_eq!(shown_text(&s).text, "User-agent: *");
    // Closing while a read runs cancels it.
    click(&mut s, "package.json");
    space(&mut s, 2.0);
    let last = reads(&rec);
    assert!(matches!(last.last(), Some(Command::CancelPreview { .. })), "{last:?}");
    s.apply(ServiceEvent::PreviewFailed {
        id: read_id(last.last().unwrap()),
        host: prod(),
        error: Error::ConnectionLost,
    });
    assert!(!s.ui.preview.is_open(), "a late answer opens nothing");
}

#[test]
fn the_panel_keeps_its_last_eight_previews_while_it_is_open() {
    let (mut s, rec) = recorded();
    // Ten files that show something, one after the other.
    let names = [
        ".env",
        ".gitignore",
        "app.min.js",
        "favicon.ico",
        "index.html",
        "package-lock.json",
        "package.json",
        "README.md",
        "robots.txt",
        "server.log",
    ];
    let answer = |s: &mut App, rec: &Recorder| {
        let id = read_id(reads(rec).last().unwrap());
        s.apply(ServiceEvent::Preview { id, host: prod(), bytes: bytes(b"text"), total: 4 });
    };
    click(&mut s, names[0]);
    space(&mut s, 1.0);
    answer(&mut s, &rec);
    for name in &names[1..] {
        click(&mut s, name);
        answer(&mut s, &rec);
    }
    // The cursor rests on the last: its text shows (the ones passed were kept all the same).
    rest(&mut s);
    assert_eq!(s.ui.preview.kept(), CACHE, "the eight before the one on the panel");
    let sent = reads(&rec).len();
    // Back over the last few and forth again: nothing is read again.
    for name in names[6..].iter().rev().chain(&names[6..]) {
        click(&mut s, name);
        assert!(matches!(s.ui.preview.phase, Phase::Shown { .. }), "{name}");
    }
    assert_eq!(reads(&rec).len(), sent, "all kept");
    // The first one was shown longest ago: it fell out, and is read again.
    click(&mut s, names[0]);
    assert_eq!(reads(&rec).len(), sent + 1);
    assert!(s.ui.preview.kept() <= CACHE);
    // Closing forgets everything.
    space(&mut s, 5.0);
    assert_eq!(s.ui.preview.kept(), 0);
    assert!(!s.ui.preview.is_open());
}

#[test]
fn a_changed_file_is_read_again() {
    let (mut s, rec) = recorded();
    click(&mut s, "robots.txt");
    space(&mut s, 1.0);
    let id = read_id(reads(&rec).last().unwrap());
    s.apply(ServiceEvent::Preview { id, host: prod(), bytes: bytes(b"User-agent: *\n"), total: 68 });
    click(&mut s, "index.html");
    // robots.txt grew meanwhile (a refresh brought it): what the panel kept of it is stale.
    if let Some(c) = s.conn_mut()
        && let crate::state::Listing::Ready(entries) = &mut c.listing
        && let Some(e) = entries.iter_mut().find(|e| e.name == "robots.txt")
    {
        e.size = 99;
    }
    let before = reads(&rec).len();
    click(&mut s, "robots.txt");
    assert_eq!(reads(&rec).len(), before + 2, "index.html's read cancelled, robots.txt's read again");
    assert!(matches!(s.ui.preview.phase, Phase::Loading { .. }));
}

#[test]
fn space_types_while_type_to_select_is_collecting_letters() {
    let mut s = demo();
    s.shortcut(Shortcut::Type("rob".into()), 10.0);
    assert_eq!(s.conn().unwrap().selection.cursor(), Some("robots.txt"), "a file");
    // "rob", then Space within the pause: part of the prefix, no preview.
    assert!(space(&mut s, 10.3).is_empty());
    assert!(!s.ui.preview.is_open(), "Space typed");
    assert_eq!(s.conn().unwrap().selection.cursor(), Some("robots.txt"), "no row starts with \"rob \"");
    // After the pause, Space is the preview's again.
    assert_eq!(space(&mut s, 12.0), [Effect::FocusPreview]);
    assert_eq!(on_panel(&s), Some("robots.txt"));
    // While it's open, letters still select and the preview follows; Space typed among them
    // types too.
    s.shortcut(Shortcut::Type("in".into()), 13.0);
    assert_eq!(on_panel(&s), Some("index.html"));
    assert!(space(&mut s, 13.2).is_empty());
    assert!(s.ui.preview.is_open(), "still open: the Space was typed");
    assert!(space(&mut s, 15.0).contains(&Effect::FocusList));
    assert!(!s.ui.preview.is_open());
}

#[test]
fn download_from_the_panel_downloads_its_file() {
    let (mut s, rec) = recorded();
    click(&mut s, "index.html");
    space(&mut s, 1.0);
    s.shortcut(Shortcut::Download, 2.0);
    let jobs: Vec<_> = rec.sent().into_iter().filter(|c| matches!(c, Command::Transfer(_))).collect();
    let [
        Command::Transfer(crate::state::TransferCommand::Start(dromon_transfer::JobRequest::Download {
            items, ..
        })),
    ] = &jobs[..]
    else {
        panic!("one download: {jobs:?}")
    };
    assert_eq!(items.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["/var/www/site/index.html"]);
    assert!(s.ui.preview.is_open(), "the panel stays");
}

/// Every line of `t`, from its blocks ([`Text::block`]).
fn lines_of(t: &Text) -> Vec<&str> {
    (0..t.blocks()).flat_map(|b| t.block(b).split('\n')).collect()
}

#[test]
fn a_texts_lines_are_found_once_crlf_or_not_with_or_without_a_last_break() {
    let head = PreviewRange::Head(TEXT_LIMIT);
    // Windows line ends: one break each.
    let t = text(head, b"ab\r\nc\r\n\r\nlast", 13).unwrap();
    assert_eq!((t.starts.as_slice(), lines_of(&t)), ([0, 3, 5, 6].as_slice(), vec!["ab", "c", "", "last"]));
    assert_eq!(t.columns, 4);
    // No last line break: the last line is there all the same; a last break starts no line.
    let t = text(head, b"one\ntwo", 7).unwrap();
    assert_eq!(lines_of(&t), ["one", "two"]);
    let t = text(head, b"one\ntwo\n", 8).unwrap();
    assert_eq!(lines_of(&t), ["one", "two"]);
    // An empty file: one empty line, no width.
    let t = text(head, b"", 0).unwrap();
    assert_eq!((t.lines(), t.blocks(), t.block(0), t.columns), (1, 1, "", 0));
    // A cut tail: from its first whole line, the widest line's width counted on what's shown.
    let t = text(PreviewRange::Tail(20), b"a cut-off line\nline 2\nline 33\n", 100).unwrap();
    assert_eq!((lines_of(&t), t.columns, t.numbered), (vec!["line 2", "line 33"], 7, false));
}

#[test]
fn blocks_hold_sixteen_lines_and_their_numbers_go_on_from_the_block_before() {
    let head = PreviewRange::Head(TEXT_LIMIT);
    // 35 lines: two whole blocks and three lines.
    let all: Vec<String> = (1..=35).map(|i| format!("line {i}")).collect();
    let bytes = all.join("\r\n").into_bytes();
    let t = text(head, &bytes, bytes.len() as u64).unwrap();
    assert_eq!((t.lines(), t.blocks()), (35, 3));
    assert_eq!((t.block_lines(0), t.block_lines(1), t.block_lines(2)), (0..16, 16..32, 32..35));
    assert_eq!(t.block(0), all[..16].join("\n"), "no line break at either end");
    assert_eq!(t.block(1), all[16..32].join("\n"));
    assert_eq!(t.block(2), "line 33\nline 34\nline 35", "the last block is shorter");
    let numbers: Vec<String> = (17..=32).map(|n| n.to_string()).collect();
    assert_eq!(t.numbers(1), numbers.join("\n"), "the second block's numbers start at 17");
    assert_eq!(t.numbers(2), "33\n34\n35");
    assert_eq!(lines_of(&t), all, "every line once, in order");
    // Exactly one block: no empty one after it.
    let sixteen = (1..=16).map(|i| i.to_string()).collect::<Vec<_>>().join("\n") + "\n";
    let t = text(head, sixteen.as_bytes(), sixteen.len() as u64).unwrap();
    assert_eq!((t.blocks(), t.numbers(0).lines().last()), (1, Some("16")));
}

#[test]
fn a_line_over_two_thousand_characters_is_cut_there() {
    let head = PreviewRange::Head(TEXT_LIMIT);
    let read = |s: &str| text(head, s.as_bytes(), s.len() as u64).unwrap();
    // Exactly the limit: whole, and nothing said.
    let t = read(&"a".repeat(LONG_LINE));
    assert_eq!((t.text.len(), t.long, t.columns), (LONG_LINE, false, LONG_LINE));
    // One over: cut at the limit, " …" after it; the width is of what's shown.
    let t = read(&format!("short\n{}\nend", "b".repeat(LONG_LINE + 1)));
    assert!(t.long);
    assert_eq!(lines_of(&t), ["short", format!("{} …", "b".repeat(LONG_LINE)).as_str(), "end"]);
    assert_eq!(t.columns, LONG_LINE + 3, "a space and an ellipsis (two columns)");
    // Several bytes a character, the limit falling inside one: cut on its boundary.
    let t = read(&"é".repeat(LONG_LINE + 5));
    assert_eq!(t.text, format!("{} …", "é".repeat(LONG_LINE)));
    let t = read(&format!("{}日本", "x".repeat(LONG_LINE - 1)));
    assert_eq!(t.text, format!("{}日 …", "x".repeat(LONG_LINE - 1)));
    // Windows line ends aren't characters of the line: a line of exactly the limit stays whole.
    let t = read(&format!("{}\r\n{}\r\n", "c".repeat(LONG_LINE), "d".repeat(LONG_LINE + 1)));
    assert_eq!(lines_of(&t), ["c".repeat(LONG_LINE), format!("{} …", "d".repeat(LONG_LINE))]);
    // Short lines, many bytes: nothing cut.
    assert!(!read(&"line\n".repeat(1_000)).long);
}

#[test]
fn a_line_is_as_wide_as_its_columns_in_the_mono_face() {
    assert_eq!(columns(""), 0);
    assert_eq!(columns("plain ascii"), 11);
    assert_eq!(columns("\tx"), 9, "a tab to the next multiple of 8");
    assert_eq!(columns("ab\tc"), 9);
    assert_eq!(columns("日本"), 4, "wide characters take two");
    assert_eq!(columns("café"), 5, "others take two too: room to spare");
}

#[test]
fn a_text_arriving_while_the_cursor_moves_waits_for_it_to_rest() {
    let mut phase = Phase::Closed;
    let t = target(SITE, file("notes.txt", 5));
    step(&mut phase, moved_to(&t, 1));
    assert!(matches!(phase, Phase::Loading { rest: Rest::Moved, .. }), "the read starts at once: {phase:?}");
    step(&mut phase, Event::Read { id: PreviewId(1), bytes: bytes(b"hello"), total: 5 });
    assert!(matches!(phase, Phase::Waiting { .. }), "it came before the cursor rested: {phase:?}");
    // The first frame starts the rest; frames before its end change nothing.
    assert_eq!(tick(&mut phase, 10.0), Stepped { changed: false, wake: Some(10.0 + REST) });
    assert_eq!(tick(&mut phase, 10.1), Stepped { changed: false, wake: Some(10.0 + REST) });
    assert!(matches!(phase, Phase::Waiting { .. }));
    assert_eq!(tick(&mut phase, 10.0 + REST), Stepped { changed: true, wake: None });
    assert!(matches!(&phase, Phase::Shown { body: Body::Text(text), .. } if text.text == "hello"));
    // Moving on while a text waits keeps it, for coming back to it.
    step(&mut phase, moved_to(&target(SITE, file("other.txt", 5)), 2));
    step(&mut phase, Event::Read { id: PreviewId(2), bytes: bytes(b"other"), total: 5 });
    let fx = step(&mut phase, moved_to(&t, 3));
    assert!(matches!(&fx[0], Fx::Keep(k, Body::Text(text)) if k.entry.name == "other.txt" && text.text == "other"));
}

#[test]
fn a_text_arriving_after_the_cursor_rested_shows_at_once_and_only_texts_wait() {
    let mut phase = Phase::Closed;
    step(&mut phase, moved_to(&target(SITE, file("notes.txt", 5)), 1));
    tick(&mut phase, 10.0);
    assert_eq!(tick(&mut phase, 10.2), Stepped::default(), "rested, nothing to show yet");
    step(&mut phase, Event::Read { id: PreviewId(1), bytes: bytes(b"hello"), total: 5 });
    assert!(matches!(phase, Phase::Shown { .. }), "{phase:?}");
    // An image moved to doesn't wait, nor does a failed read or a binary.
    step(&mut phase, moved_to(&target(SITE, file("logo.png", 3)), 2));
    step(&mut phase, Event::Read { id: PreviewId(2), bytes: bytes(b"png"), total: 3 });
    assert!(matches!(phase, Phase::Shown { body: Body::Image(_), .. }), "{phase:?}");
    step(&mut phase, moved_to(&target(SITE, file("bin.txt", 3)), 3));
    step(&mut phase, Event::Read { id: PreviewId(3), bytes: bytes(b"a\0b"), total: 3 });
    assert!(matches!(phase, Phase::Unsupported { .. }), "{phase:?}");
    step(&mut phase, moved_to(&target(SITE, file("gone.txt", 3)), 4));
    step(&mut phase, Event::Failed { id: PreviewId(4), error: Error::NotFound(RemotePath::new("/gone.txt")) });
    assert!(matches!(phase, Phase::Failed { .. }), "{phase:?}");
}

#[test]
fn space_shows_at_once_and_moving_waits_for_the_cursor_to_rest() {
    let mut s = demo();
    click(&mut s, "index.html");
    space(&mut s, 1.0);
    assert!(matches!(s.ui.preview.phase, Phase::Shown { .. }), "Space isn't held back");
    // ↓ twice in quick succession: neither text shows while the cursor moves.
    s.shortcut(Shortcut::Move { delta: 1, extend: false }, 2.0);
    assert!(s.preview_tick(2.0).wake.is_some());
    assert!(matches!(&s.ui.preview.phase, Phase::Waiting { target, .. } if target.entry.name == "package-lock.json"));
    s.shortcut(Shortcut::Move { delta: 1, extend: false }, 2.05);
    assert!(!s.preview_tick(2.05).changed);
    assert!(matches!(&s.ui.preview.phase, Phase::Waiting { target, .. } if target.entry.name == "package.json"));
    assert!(!s.preview_tick(2.1).changed, "not rested yet");
    let shown = s.preview_tick(2.05 + REST);
    assert!(
        shown.changed
            && matches!(&s.ui.preview.phase, Phase::Shown { target, .. } if target.entry.name == "package.json")
    );
}
