//! Properties of a recording, for any run of steps on the demo between Record Steps… and Stop:
//! its fixture parses, its script is JSON, every row the script taps is a row of a folder in the
//! fixture (or one the script itself named), and no name the demo has (hosts, users, addresses,
//! files and folders) nor one the user typed reaches the report, its link's fields or what Open
//! GitHub copies. And for any folder, the recorded fixture writes its rows in the order sorting
//! by name gives, so the replay shows them as the user saw them.

use std::collections::BTreeSet;
use std::sync::Arc;

use dromon_core::{Entry, HostId, SortDir, SortKey, sort_entries};
use hegel::TestCase;
use hegel::generators as gs;
use serde_json::Value;

use super::*;
use crate::fixture::{self, Item, Node};
use crate::state::props::{Run, demo, distinctive, next, perform, start};
use crate::state::report::{DIAGNOSTICS, Draft, FIELDS, NEW_ISSUE};

/// The block fenced with ```` ```json <what> ````, from a report's text.
fn block(text: &str, what: &str) -> String {
    let open = format!("```json {what}\n");
    let from = text.find(&open).unwrap_or_else(|| panic!("no {what} block in {text}")) + open.len();
    let len = text[from..].find("\n```").expect("the block's end");
    text[from..from + len].to_owned()
}

/// `%XX` back to text, as GitHub fills a field.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).expect("an escape"));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).expect("UTF-8")
}

/// The link's fields, decoded.
fn fields(url: &str) -> Vec<String> {
    let query = url.split_once('?').map_or("", |(_, q)| q);
    query
        .split('&')
        .filter_map(|kv| FIELDS.iter().find_map(|id| kv.strip_prefix(&format!("{id}="))))
        .map(decode)
        .collect()
}

/// Every name the demo knows a host, a user, an address, a file or a folder by. Names under
/// four characters are left out (`pi`, `etc`, `log` are inside the report's own words), and so
/// are the fixture format's own word `home` and names that are an extension the stand-ins keep
/// (`html`, as in `file-6.html`), since the stand-ins write them by design, and names in the
/// repository's own address the link opens (`fatih`). A photo's name is looked for by its `IMG_`.
fn demo_names() -> BTreeSet<String> {
    let fx = fixture::demo();
    let mut names: BTreeSet<String> = BTreeSet::new();
    for h in &fx.hosts {
        names.extend([Some(&h.alias), h.user.as_ref(), h.hostname.as_ref()].into_iter().flatten().cloned());
    }
    let mut kept = BTreeSet::new();
    for s in fx.servers.values() {
        for path in s.tree.keys().chain([&s.home]) {
            names.extend(path.components().map(str::to_owned));
        }
        for node in s.tree.values() {
            let Node::Entries(items) = node else { continue };
            for item in items {
                if let Item::Entry(e) = item {
                    names.insert(e.name.clone());
                    kept.extend(e.name.rsplit_once('.').map(|(_, ext)| ext.to_owned()));
                }
            }
        }
    }
    names.retain(|n| n.chars().count() >= 4 && n != "home" && !kept.contains(n) && !NEW_ISSUE.contains(n.as_str()));
    names.insert("IMG_".to_owned());
    names
}

/// The names in `secrets` the draft carries anywhere: its text, its link, the link's fields or
/// what Open GitHub copies.
fn leaks(d: &Draft, secrets: &BTreeSet<String>) -> Vec<String> {
    let mut places = vec![d.text.clone(), d.url.clone()];
    places.extend(fields(&d.url));
    places.extend(d.copies.clone());
    secrets.iter().filter(|n| places.iter().any(|p| p.contains(n.as_str()))).cloned().collect()
}

/// Every name the fixture lists, anywhere.
fn listed_names(fx: &fixture::Fixture) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for s in fx.servers.values() {
        for path in s.tree.keys().chain([&s.home]) {
            out.extend(s.list(path).into_iter().flatten().map(|e| e.name));
        }
    }
    out
}

// Each case builds the app on the demo and records up to twenty-five steps: fewer cases than
// the default keep this to seconds.
#[hegel::test(test_cases = 25)]
fn a_recording_replays_on_its_own_fixture_and_names_nothing(tc: TestCase) {
    let mut s = demo();
    let mut run = Run::default();
    let first = start(&tc);
    perform(&mut s, first);
    let before = tc.draw(gs::integers::<usize>().max_value(3));
    for _ in 0..before {
        let what = next(&tc, &s, &mut run);
        tc.note(&format!("before: {what:?}"));
        perform(&mut s, what);
    }
    s.act(Action::Report(ReportAction::Record));
    assert_eq!(s.recording_steps(), Some(0));
    run.recording = true;
    let steps = tc.draw(gs::integers::<usize>().min_value(1).max_value(25));
    for i in 0..steps {
        let what = next(&tc, &s, &mut run);
        tc.note(&format!("{i}: {what:?}"));
        perform(&mut s, what);
        assert!(s.recording_steps().is_some(), "only Stop ends this recording");
    }
    s.act(Action::Report(ReportAction::StopRecording));
    let d = s.report.draft.clone().expect("the sheet, with the recording");
    assert!(d.recorded.is_some());
    let fixture_text = block(&d.text, "fixture");
    let fx = fixture::parse(&fixture_text).unwrap_or_else(|e| panic!("{e}\n{fixture_text}"));
    let script: Value = serde_json::from_str(&block(&d.text, "script")).expect("the script is JSON");
    let events = script["events"].as_array().expect("events");
    let mut rows = listed_names(&fx);
    rows.extend(events.iter().filter_map(|e| e["text"].as_str().map(str::to_owned)));
    for e in events {
        if let Some(name) = e["key"].as_str().and_then(|k| k.strip_prefix("rows/")) {
            assert!(rows.contains(name), "the script taps {name:?}, which the fixture doesn't list");
        }
    }
    let mut secrets = demo_names();
    secrets.extend(run.typed.iter().filter_map(|t| distinctive(t)).map(str::to_owned));
    assert_eq!(leaks(&d, &secrets), Vec::<String>::new(), "names in the report:\n{}", d.text);
    assert!(fields(&d.url).len() == 3 && d.url.contains(&format!("&{DIAGNOSTICS}=")));
}

#[hegel::test(test_cases = 50)]
#[hegel::explicit_test_case(names = vec![".0".to_owned(), "-".to_owned()], dirs = vec![false, false])]
fn a_recorded_folders_rows_are_written_in_the_order_sorting_by_name_gives(tc: TestCase) {
    // Hidden names, names starting with punctuation, plain ones: what sorts around a dot.
    let name = hegel::one_of!(
        gs::from_regex(r"\.[a-z0-9]{1,3}"),
        gs::from_regex("[-#( ][a-z0-9]{0,3}"),
        gs::from_regex("[a-z0-9][-#( .a-z0-9]{0,3}"),
    );
    let names = tc.draw(gs::vecs(name).unique(true).min_size(1).max_size(6));
    let dirs = tc.draw(gs::vecs(gs::booleans()).min_size(names.len()).max_size(names.len()));
    let names: Vec<String> = names.into_iter().filter(|n| n != "." && n != "..").collect();
    let kind = |dir: bool| if dir { "dir" } else { "file" };
    let entries: Vec<String> = names
        .iter()
        .zip(&dirs)
        .map(|(n, d)| format!("{{\"name\":{},\"kind\":\"{}\"}}", Value::from(n.as_str()), kind(*d)))
        .collect();
    let doc = format!(
        "{{\"version\":1,\"now\":0,\"hosts\":[{{\"alias\":\"h\",\"source\":\"saved\"}}],\
         \"servers\":{{\"h\":{{\"home\":\"/h\",\"tree\":{{\"/h\":[{}]}}}}}}}}",
        entries.join(",")
    );
    let mut s = fixture::app(Arc::new(fixture::parse(&doc).expect("the fixture")));
    s.wall_ms = || 0;
    s.act(Action::Report(ReportAction::Record));
    s.act(Action::SelectHost(HostId("h".into())));
    s.act(Action::Report(ReportAction::StopRecording));
    let text = block(&s.report.draft.as_ref().expect("the sheet").text, "fixture");
    let fx = fixture::parse(&text).expect("the recorded fixture");
    let server = fx.server("host-1").expect("the host");
    let Some(Node::Entries(items)) = server.tree.get(&server.home) else { panic!("home isn't listed: {text}") };
    let written: Vec<Entry> =
        items.iter().filter_map(|i| if let Item::Entry(e) = i { Some(e.clone()) } else { None }).collect();
    let mut sorted = written.clone();
    sort_entries(&mut sorted, SortKey::Name, SortDir::Ascending);
    let names = |es: &[Entry]| es.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&written), names(&sorted), "the replay would list the folder in another order");
}
