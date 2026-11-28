//! The bug report: what it says and in which order, that no name survives, the link and its
//! budget, the sheet's actions, the OS readings, and the form's field ids.

use dromon_core::{Error, HostSource, HostSpec, Prompt, PromptId, RemotePath};

use super::*;
use crate::fixture;
use crate::state::service::FileOpEvent;
use crate::state::{Action, FileOp, PaletteAction, Recorder, ServiceEvent};

fn demo() -> App {
    let mut s = fixture::app(fixture::demo());
    s.report.about = Some(About {
        version: "9.9.0".into(),
        os: "Ubuntu 24.04.1 LTS (Linux 6.8.0-49-generic)".into(),
        arch: "x86_64".into(),
        desktop: Some("GNOME on wayland".into()),
    });
    s
}

fn open(s: &mut App, subject: Option<Subject>) -> Vec<Effect> {
    s.act(Action::Report(ReportAction::Open(subject)))
}

fn draft(s: &App) -> &Draft {
    s.report.draft.as_ref().expect("the sheet is open")
}

/// `%XX` back to bytes (what GitHub puts in the field).
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

/// The diagnostics field of a new-issue link, decoded.
fn field(url: &str) -> String {
    let (_, value) = url.split_once(&format!("&{DIAGNOSTICS}=")).expect("the field");
    decode(value)
}

#[test]
fn the_ring_keeps_the_last_ones_and_counts_a_run_once() {
    let mut r = Recent::default();
    for i in 0..EVENTS + 5 {
        r.event(format!("Listing host=prod entries={i}"));
    }
    assert_eq!(r.events.len(), EVENTS);
    assert_eq!(r.events.front().map(|(l, _)| l.as_str()), Some("Listing host=prod entries=5"), "the oldest go");
    r.event("Transfers jobs=1 active=1 done=0".into());
    r.event("Transfers jobs=1 active=1 done=0".into());
    assert_eq!(r.events.back(), Some(&("Transfers jobs=1 active=1 done=0".to_owned(), 2)));
    for _ in 0..3 {
        r.action("Move");
    }
    r.action("Open");
    assert_eq!(r.actions.iter().cloned().collect::<Vec<_>>(), [("Move", 3), ("Open", 1)]);
}

#[test]
fn actions_are_listed_by_kind_and_the_reports_own_are_not() {
    let mut s = demo();
    s.act(Action::SelectHost(HostId("prod".into())));
    s.act(Action::Open("releases".into()));
    s.act(Action::GoUp);
    s.act(Action::GoUp);
    open(&mut s, None);
    s.act(Action::Report(ReportAction::Copy));
    let kinds: Vec<_> = s.report.recent.actions.iter().cloned().collect();
    assert!(kinds.ends_with(&[("SelectHost", 1), ("Open", 1), ("GoUp", 2)]), "{kinds:?}");
    assert_eq!(Action::File(crate::state::FileAction::Rename).kind(), "File");
    assert_eq!(Action::Click { name: "secret.txt".into(), extend: false, toggle: false }.kind(), "Click");
}

#[test]
fn the_report_says_what_it_knows_in_order() {
    let mut s = demo();
    s.report.saw(Seen { scale: 2.0, dark: true, access: Access { reduce_motion: true, ..Access::default() } });
    s.apply(ServiceEvent::Update(crate::state::update::UpdateEvent::Install(Install::Deb)));
    s.act(Action::SelectHost(HostId("prod".into())));
    open(&mut s, None);
    let text = &draft(&s).text;
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[..8],
        [
            "Dromon 9.9.0 (installed as: .deb package)",
            "OS: Ubuntu 24.04.1 LTS (Linux 6.8.0-49-generic), x86_64",
            "Desktop: GNOME on wayland",
            "Display: scale 2, dark",
            "Settings: theme System, accent Spearmint, density Comfortable",
            "Accessibility: reduce motion on, increased contrast off, reduce transparency off",
            "Windows open: 1",
            "On screen: folder listing",
        ],
        "{text}"
    );
    let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle:?} in {text}"));
    assert!(at("Recent actions, oldest first:\n  SelectHost") < at("Recent events, oldest first:"));
    // The events oldest first: the update's news, then prod connecting and listing.
    let events = &text[at("Recent events, oldest first:")..];
    let order = ["Update Install", "Status host=host-1 status=Connecting", "Opened host=host-1", "Listing"];
    let found: Vec<usize> = order.iter().map(|o| events.find(o).unwrap_or_else(|| panic!("{o} in {events}"))).collect();
    assert!(found.windows(2).all(|w| w[0] < w[1]), "{events}");
    assert!(!text.contains("Error the report started from"), "started from the menu");
}

#[test]
fn what_is_on_screen_is_its_kind_only() {
    let mut s = demo();
    open(&mut s, None);
    assert!(draft(&s).text.contains("On screen: welcome\n"));
    assert!(draft(&s).text.contains("Display: not drawn yet\n"), "a window the view never drew");
    s.act(Action::SelectHost(HostId("prod".into())));
    s.act(Action::Click { name: "index.html".into(), extend: false, toggle: false });
    s.act(Action::Preview(crate::state::PreviewAction::Toggle));
    s.act(Action::OpenLog);
    open(&mut s, None);
    let text = &draft(&s).text;
    assert!(text.contains("On screen: folder listing; preview open; sheet: connection log\n"), "{text}");
    s.act(Action::OpenSettings);
    open(&mut s, None);
    assert!(draft(&s).text.contains("On screen: Settings, Appearance"));
}

/// Names a report must never carry: hosts, a user, an address, a port, folders, files.
const SECRETS: &[&str] =
    &["acme-prod-db", "acme-corp", "zelda", "198.51.100.77", "48213", "secret-plans", "q3-payroll", "kitchen"];

/// A window that knows hosts, users, addresses, ports, folders and files by those names, and
/// saw them in every kind of event.
fn full_of_names() -> App {
    let mut s = demo();
    let rec = Recorder::default();
    s.backend = Box::new(rec);
    let db = HostId("acme-prod-db".into());
    let quick = HostId("zelda@198.51.100.77:48213".into());
    let spec = |id: &HostId, source| HostSpec {
        id: id.clone(),
        source,
        alias: id.0.clone(),
        hostname: Some("db.acme-corp.example".into()),
        user: Some("zelda".into()),
        port: Some(48213),
        identity_file: Some("/home/zelda/.ssh/id_kitchen".into()),
        proxy_jump: Some("zelda@jump.acme-corp.example".into()),
    };
    s.apply(ServiceEvent::Hosts(vec![spec(&db, HostSource::SshConfig)]));
    s.hosts.hosts.push(spec(&quick, HostSource::Recent));
    s.act(Action::SelectHost(quick.clone()));
    s.act(Action::SelectHost(db.clone()));
    let secret = RemotePath::new("/home/zelda/secret-plans/q3-payroll.xlsx");
    let prompt = Prompt {
        id: PromptId(4),
        host: db.clone(),
        kind: dromon_core::PromptKind::Password,
        text: "zelda@db.acme-corp.example's password: ".into(),
        error: None,
    };
    for ev in [
        ServiceEvent::Prompt(prompt),
        ServiceEvent::Log { host: db.clone(), line: "debug1: Connecting to 198.51.100.77 port 48213".into() },
        ServiceEvent::Opened { host: db.clone(), start: RemotePath::new("/home/zelda") },
        ServiceEvent::ListFailed {
            host: db.clone(),
            path: secret.clone(),
            error: Error::PermissionDenied(secret.clone()),
        },
        ServiceEvent::Status {
            host: quick.clone(),
            status: ConnStatus::Failed("ssh: zelda@198.51.100.77: refused".into()),
        },
    ] {
        s.apply(ev);
    }
    let id = s.ops.begin(db.clone(), &FileOp::Delete { paths: vec![secret.clone()] });
    s.apply(ServiceEvent::FileOp(FileOpEvent::Failed { id, host: db, error: Error::PermissionDenied(secret) }));
    s
}

#[test]
fn no_name_survives_in_the_report_or_the_link() {
    let mut s = full_of_names();
    let op = s.ops.ops.last().map(|o| o.id).unwrap();
    let quick = HostId("zelda@198.51.100.77:48213".into());
    for subject in [None, Some(Subject::Op(op)), Some(Subject::Connection(quick))] {
        open(&mut s, subject.clone());
        let d = draft(&s).clone();
        for secret in SECRETS {
            assert!(!d.text.contains(secret), "{secret} in the report ({subject:?}):\n{}", d.text);
            assert!(!d.url.contains(secret), "{secret} in the link ({subject:?})");
            assert!(!field(&d.url).contains(secret), "{secret} in the field ({subject:?})");
        }
    }
    // The stand-ins are there instead, numbered alike throughout.
    open(&mut s, Some(Subject::Op(op)));
    let text = &draft(&s).text;
    assert!(text.contains("  Host: host-1\n  Message: You don't have access to <dir>/file-1.xlsx"), "{text}");
    assert!(text.contains("Opened host=host-1"), "{text}");
    assert!(text.contains("Status host=host-2 status=Failed"), "{text}");
}

/// The value of the link's field `id`, decoded.
fn field_of(url: &str, id: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|kv| kv.strip_prefix(&format!("{id}="))).map(decode)
}

/// A server full of the same names, a password among them.
const NAMED: &str = r#"{"version": 1, "now": 1795615200,
  "hosts": [{"alias": "acme-prod-db", "source": "saved", "user": "zelda", "hostname": "db.acme-corp.example", "port": 48213}],
  "servers": {"acme-prod-db": {"home": "/home/zelda",
    "auth": [{"password": {"prompt": "zelda@db.acme-corp.example's password: ", "accept": "hunter2-kitchen"}}],
    "tree": {
      "/home/zelda": [
        {"name": "secret-plans", "kind": "dir", "owner": "zelda", "group": "acme-corp"},
        {"name": "q3-payroll.xlsx", "kind": "file", "size": 48100, "owner": "zelda"},
        {"name": ".kitchen-notes", "kind": "file", "size": 12}],
      "/home/zelda/secret-plans": [{"name": "kitchen.txt", "kind": "file", "size": 9}]}}}}"#;

#[test]
fn no_name_survives_in_a_recording_or_its_link_fields_or_what_is_copied() {
    fn still() -> i64 {
        0
    }
    let mut s = fixture::app(std::sync::Arc::new(fixture::parse(NAMED).unwrap()));
    s.wall_ms = still;
    s.act(Action::Report(ReportAction::Record));
    s.act(Action::SelectHost(HostId("acme-prod-db".into())));
    s.ui.sheets.edit_mut().text = "hunter2-kitchen".into();
    s.act(Action::SubmitPrompt);
    s.act(Action::Open("secret-plans".into()));
    s.act(Action::Back);
    s.act(Action::Click { name: "q3-payroll.xlsx".into(), extend: false, toggle: false });
    s.act(Action::File(crate::state::FileAction::Rename));
    s.files.edit.field_mut().expect("the name field").edit = rune::TextEdit::new("zelda-acme-corp-merger.xlsx");
    s.act(Action::File(crate::state::FileAction::EditSubmit));
    s.act(Action::Palette(PaletteAction::OpenPath));
    s.ui.palette.edit = rune::TextEdit::new("/home/zelda/secret-plans");
    s.act(Action::Palette(PaletteAction::Edited));
    s.act(Action::Palette(PaletteAction::Run));
    let paths = vec![std::path::PathBuf::from("/home/zelda/Desktop/kitchen-q3-payroll.pdf")];
    s.act(Action::Upload { paths, folder: None });
    s.act(Action::Report(ReportAction::StopRecording));
    let d = draft(&s).clone();
    assert!(d.text.contains("```json fixture") && d.text.contains("```json script"), "{}", d.text);
    let fields: Vec<String> = FIELDS.iter().filter_map(|id| field_of(&d.url, id)).collect();
    assert_eq!(fields.len(), 3, "the link fills every field");
    let mut secrets = SECRETS.to_vec();
    secrets.extend(["hunter2", "merger", "Desktop", "notes"]);
    for secret in secrets {
        assert!(!d.text.contains(secret), "{secret} in the report:\n{}", d.text);
        assert!(!d.url.contains(secret), "{secret} in the link");
        for f in &fields {
            assert!(!f.contains(secret), "{secret} in a field:\n{f}");
        }
        assert!(!d.copies.as_deref().unwrap_or("").contains(secret), "{secret} in what's copied");
    }
    // The steps are there, by their stand-ins, and the upload by what it was.
    for said in [
        "Answered the password (what you typed isn't recorded)",
        "Named it file-",
        "Went to /dir-",
        "Uploaded files (not replayed)",
    ] {
        assert!(d.text.contains(said), "{said:?} in {}", d.text);
    }
}

#[test]
fn an_errors_report_says_its_kind_and_a_message_without_names() {
    let mut s = full_of_names();
    let op = s.ops.ops.last().map(|o| o.id).unwrap();
    open(&mut s, Some(Subject::Op(op)));
    let text = draft(&s).text.clone();
    let error = "Error the report started from:\n  What: a file operation failed (Delete)\n  \
                 Kind: FileOp Delete, PermissionDenied\n  Host: host-1\n  \
                 Message: You don't have access to <dir>/file-1.xlsx\n";
    assert!(text.contains(error), "{text}");
    assert!(text.find(error) < text.find("Recent actions"), "before the actions");
    open(&mut s, Some(Subject::Connection(HostId("zelda@198.51.100.77:48213".into()))));
    let text = draft(&s).text.clone();
    assert!(text.contains("  What: couldn't connect\n  Kind: Connection Failed\n  Host: host-1\n"), "{text}");
    assert!(text.contains(&format!("  Message: {}", redact::WITHHELD)), "ssh's words name hosts: {text}");
    // A dismissed toast's error is gone: the report says nothing of it.
    s.ops.forget(op);
    open(&mut s, Some(Subject::Op(op)));
    assert!(!draft(&s).text.contains("Error the report started from"));
}

#[test]
fn the_link_fits_its_budget_by_leaving_out_the_oldest_events_whole() {
    let mut s = demo();
    for i in 0..EVENTS {
        s.report.recent.event(format!("Listing host=prod entries={i} {}", "x".repeat(120)));
    }
    open(&mut s, None);
    let d = draft(&s).clone();
    assert!(d.url.len() <= URL_BUDGET, "{} characters", d.url.len());
    assert!(d.left_out > 0 && d.left_out < EVENTS);
    let sent = field(&d.url);
    assert!(sent.contains(&format!("\n  ({} older events omitted)\n", d.left_out)), "{sent}");
    assert!(sent.contains(&format!("entries={} ", EVENTS - 1)), "the newest stays");
    assert!(!sent.contains("entries=0 "), "the oldest goes");
    // Every line of what's sent is a whole line of the report, but the omitted note.
    let all: Vec<&str> = d.text.lines().collect();
    assert!(sent.lines().all(|l| all.contains(&l) || l.ends_with("older events omitted)")), "{sent}");
    // Copy report has it all, untrimmed.
    let fx = s.act(Action::Report(ReportAction::Copy));
    assert_eq!(fx, [Effect::Copy(d.text.clone())]);
    assert!(d.text.contains("entries=0 ") && !d.text.contains("omitted"));
}

#[test]
fn a_recording_the_link_cant_hold_is_copied_the_fixture_first_then_the_script_too() {
    let d =
        Diagnostics { head: vec!["Dromon 9.9.0".into()], events: Vec::new(), steps: vec!["1. Opened host-1".into()] };
    let block = |what: &str, n: usize| format!("```json {what}\n{}\n```", "x".repeat(n));
    let small = d.link(URL_BUDGET, Some((&block("fixture", 100), &block("script", 100))));
    assert_eq!(small.copies, None);
    assert_eq!(field_of(&small.url, FIXTURE).as_deref(), Some(block("fixture", 100).as_str()));
    let fixture = block("fixture", URL_BUDGET);
    let big = d.link(URL_BUDGET, Some((&fixture, &block("script", 100))));
    assert!(big.url.len() <= URL_BUDGET);
    assert_eq!(big.copies.as_deref(), Some(fixture.as_str()));
    assert_eq!(field_of(&big.url, FIXTURE).as_deref(), Some(PASTE_FIXTURE));
    assert_eq!(field_of(&big.url, SCRIPT).as_deref(), Some(block("script", 100).as_str()));
    let script = block("script", URL_BUDGET);
    let both = d.link(URL_BUDGET, Some((&fixture, &script)));
    assert!(both.url.len() <= URL_BUDGET);
    assert_eq!(both.copies, Some(format!("{fixture}\n\n{script}")));
    assert_eq!(field_of(&both.url, SCRIPT).as_deref(), Some(PASTED_WITH));
    assert!(
        field_of(&both.url, DIAGNOSTICS)
            .unwrap()
            .starts_with("Steps you recorded (names replaced):\n  1. Opened host-1\n\n")
    );
}

#[test]
fn a_short_report_goes_whole() {
    let mut s = demo();
    open(&mut s, None);
    let d = draft(&s);
    assert_eq!(d.left_out, 0);
    assert_eq!(field(&d.url), d.text, "the preview is what's sent");
}

#[test]
fn the_link_names_the_form_and_its_field_and_encodes_the_text() {
    assert_eq!(
        issue_url("a b\n/é&=+%"),
        "https://github.com/fatihdogmus/dromon/issues/new?template=bug.yml&diagnostics=a%20b%0A%2F%C3%A9%26%3D%2B%25"
    );
    assert_eq!(encode("AZaz09-_.~"), "AZaz09-_.~", "unreserved characters stay");
    assert_eq!(encode("×"), "%C3%97");
}

#[test]
fn open_github_opens_the_link_and_closes_the_sheet() {
    let mut s = demo();
    assert!(open(&mut s, None).is_empty());
    assert!(s.report.is_open());
    let url = draft(&s).url.clone();
    assert!(url.starts_with("https://github.com/fatihdogmus/dromon/issues/new?template=bug.yml&diagnostics="));
    assert_eq!(s.act(Action::Report(ReportAction::OpenGitHub)), [Effect::OpenUrl(url)]);
    assert!(!s.report.is_open());
    assert!(s.act(Action::Report(ReportAction::OpenGitHub)).is_empty(), "once");
}

#[test]
fn copy_report_copies_the_text_and_says_so_cancel_and_escape_close() {
    let mut s = demo();
    open(&mut s, None);
    let text = draft(&s).text.clone();
    assert_eq!(s.act(Action::Report(ReportAction::Copy)), [Effect::Copy(text)]);
    assert!(draft(&s).copied, "the button says Copied");
    s.act(Action::Report(ReportAction::Cancel));
    assert!(!s.report.is_open());
    open(&mut s, None);
    s.act(Action::DismissSheet);
    assert!(!s.report.is_open(), "Escape");
}

#[test]
fn escape_closes_a_sheet_over_the_report_first() {
    let mut s = demo();
    s.act(Action::SelectHost(HostId("prod".into())));
    open(&mut s, None);
    s.act(Action::OpenLog);
    s.act(Action::DismissSheet);
    assert!(s.ui.sheets.open().is_none() && s.report.is_open(), "the log went, the report waits");
    s.act(Action::DismissSheet);
    assert!(!s.report.is_open());
}

#[test]
fn the_palette_offers_report_a_bug_and_keeps_the_settings_page() {
    let mut s = demo();
    s.act(Action::OpenSettings);
    s.act(Action::Palette(PaletteAction::Toggle));
    s.ui.palette.edit = rune::TextEdit::new("bug");
    s.act(Action::Palette(PaletteAction::Edited));
    let item = s.palette_items().into_iter().find(|i| i.key == "pal-report-bug").expect("the row");
    assert_eq!(item.title, "Report a bug…");
    s.act(Action::Palette(PaletteAction::Pick("pal-report-bug".into())));
    assert!(s.report.is_open() && !s.ui.palette.is_open());
    assert!(s.on_settings(), "a report of the Settings page");
    assert!(draft(&s).text.contains("On screen: Settings, Appearance"));
}

#[test]
fn a_failed_deletes_toast_reports_its_error() {
    let mut s = full_of_names();
    let toast = s.toasts().into_iter().find(|t| matches!(t.id, crate::state::toast::ToastId::Op(_))).unwrap();
    let subject = toast.report().expect("a Report button");
    open(&mut s, Some(subject));
    assert!(draft(&s).text.contains("Kind: FileOp Delete, PermissionDenied"));
}

#[test]
fn while_the_report_is_open_the_lists_shortcuts_wait() {
    let mut s = demo();
    s.act(Action::SelectHost(HostId("prod".into())));
    open(&mut s, None);
    s.shortcut(crate::state::Shortcut::Palette, 0.0);
    assert!(!s.ui.palette.is_open());
}

#[test]
fn the_os_is_read_from_what_each_system_says() {
    let release = "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n";
    assert_eq!(os_release(release).as_deref(), Some("Ubuntu 24.04.1 LTS"));
    assert_eq!(os_release("NAME=Arch\nVERSION_ID=rolling\n").as_deref(), Some("Arch rolling"));
    assert_eq!(os_release("NAME='NixOS'\n").as_deref(), Some("NixOS"));
    assert_eq!(os_release("ID=x\n"), None);
    let plist = "<dict>\n\t<key>ProductName</key>\n\t<string>macOS</string>\n\t<key>ProductVersion</key>\n\
                 \t<string>15.2</string>\n</dict>";
    assert_eq!(mac_version(plist).as_deref(), Some("macOS 15.2"));
    assert_eq!(mac_version("<dict></dict>"), None);
    let reg = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\r\n    \
               ProductName    REG_SZ    Windows 10 Pro\r\n    CurrentBuild    REG_SZ    22631\r\n    \
               DisplayVersion    REG_SZ    23H2\r\n    UBR    REG_DWORD    0x116c\r\n";
    assert_eq!(windows_version(reg).as_deref(), Some("Windows 11 (build 22631.4460, 23H2)"));
    let ten = "    ProductName    REG_SZ    Windows 10 Home\r\n    CurrentBuild    REG_SZ    19045\r\n";
    assert_eq!(windows_version(ten).as_deref(), Some("Windows 10 (build 19045)"));
    let server =
        "    ProductName    REG_SZ    Windows Server 2022 Datacenter\r\n    CurrentBuild    REG_SZ    20348\r\n";
    assert_eq!(windows_version(server).as_deref(), Some("Windows Server 2022 Datacenter (build 20348)"));
    assert_eq!(windows_version("nothing"), None);
    assert_eq!(desktop(Some("ubuntu:GNOME"), Some("wayland")).as_deref(), Some("ubuntu:GNOME on wayland"));
    assert_eq!(desktop(Some("KDE"), Some(" ")).as_deref(), Some("KDE"));
    assert_eq!(desktop(None, None), None);
}

#[test]
fn every_field_the_link_fills_is_in_the_bug_form() {
    let form = include_str!("../../../../.github/ISSUE_TEMPLATE/bug.yml");
    let ids: Vec<&str> = form.lines().filter_map(|l| l.trim().strip_prefix("id:")).map(str::trim).collect();
    for id in FIELDS {
        assert!(ids.contains(id), "bug.yml has no field {id:?} (it has {ids:?})");
    }
    assert!(ids.contains(&"what-happened") && ids.contains(&"expected"), "{ids:?}");
    assert_eq!(TEMPLATE, "bug.yml");
}
