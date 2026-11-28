//! Properties of the stand-ins, for any names: a stand-in carries nothing of a name but its
//! leading dot, whether it sorts before a dot, and a short, plain extension; the same name gets
//! the same stand-in and different
//! names different ones; a free-form error is withheld whole; a trace line keeps its words and
//! names each host by a stand-in; and a folder's stand-ins sort as its names do, as a recording
//! needs.

use std::collections::HashMap;

use dromon_core::{Entry, EntryKind, Error, HostId, RemotePath, SortDir, SortKey, sort_entries};
use hegel::TestCase;
use hegel::generators::{self as gs, Generator};

use super::*;

/// Names as servers send them: empty, very long, unicode, control characters, `/`, leading dots
/// and dashes, digits, punctuation that sorts between letters, and names inside each other.
#[hegel::composite]
fn name(tc: &TestCase) -> String {
    let odd: Vec<String> = [
        "",
        ".",
        "..",
        "-",
        ".0",
        "0",
        "a",
        "A",
        "ab",
        "abc",
        ".env",
        ".a.txt",
        "a.txt",
        "a.TXT",
        "a.tar.gz",
        "Makefile",
        "x.verylongext",
        "x.a-b",
        "file-1.txt",
        "dir-1",
        "host-1",
        "<path>",
        "é",
        "e\u{301}",
        "İ",
        "ß",
        "日本",
        "a b",
        "-rf",
        "#recycle",
        "(old)",
        "a/b",
        "\u{0}",
        "\n",
        "~",
        "_",
        "😀.png",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    tc.draw(hegel::one_of!(
        gs::sampled_from(odd),
        gs::from_regex(r"\.?[-#( !0-9A-Za-z_.~]{0,12}"),
        gs::text().max_size(16),
        gs::text().min_size(200).max_size(300),
    ))
}

/// `name` made a path component: no `/`, and not empty, `.` or `..`.
fn component(name: String) -> String {
    let name = name.replace('/', "_");
    if name.is_empty() || name == "." || name == ".." { format!("{name}x") } else { name }
}

/// The extension a stand-in keeps, as `redact.rs` says it: after the last dot of a name that
/// isn't only a dot file, ASCII letters and digits, at most eight of them.
fn kept_extension(name: &str) -> Option<&str> {
    let (stem, ext) = name.rsplit_once('.')?;
    let plain = (1..=8).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric());
    (!stem.is_empty() && plain).then_some(ext)
}

/// What a stand-in starts with: a hidden name's dot, `-` for a name that sorts before a dot.
fn lead(name: &str) -> &'static str {
    if name.starts_with('.') {
        "."
    } else if dromon_core::natural_cmp(name, ".").is_lt() {
        "-"
    } else {
        ""
    }
}

/// Another name with all a stand-in may keep of `name`: its lead and its extension.
fn twin(name: &str, i: usize) -> String {
    let lead = lead(name);
    match kept_extension(name) {
        Some(ext) => format!("{lead}q{i}.{ext}"),
        None => format!("{lead}q{i}"),
    }
}

/// One thing a report asks of its redactor, naming names by their index in a pool.
#[derive(Clone, Debug)]
enum Ask {
    Name(usize, bool),
    Path(Vec<usize>),
    Folder(Vec<usize>),
    Host(usize),
    Error(u8, Vec<usize>),
}

#[hegel::composite]
fn ask(tc: &TestCase, pool: usize) -> Ask {
    let i = || gs::integers::<usize>().max_value(pool - 1);
    let path = || gs::vecs(i()).max_size(4);
    match tc.draw(gs::integers::<u8>().max_value(4)) {
        0 => Ask::Name(tc.draw(i()), tc.draw(gs::booleans())),
        1 => Ask::Path(tc.draw(path())),
        2 => Ask::Folder(tc.draw(path())),
        3 => Ask::Host(tc.draw(i())),
        _ => Ask::Error(tc.draw(gs::integers::<u8>().max_value(2)), tc.draw(path())),
    }
}

/// What `r` answers `ask`, the pool's names being `names`.
fn answer(r: &mut Redactor, names: &[String], ask: &Ask) -> String {
    let path = |ixs: &[usize]| RemotePath::new(&ixs.iter().map(|&i| names[i].as_str()).collect::<Vec<_>>().join("/"));
    match ask {
        Ask::Name(i, dir) => r.name(&names[*i], *dir),
        Ask::Path(ixs) => r.path(&path(ixs)),
        Ask::Folder(ixs) => r.remote_path(&path(ixs)),
        Ask::Host(i) => r.host(&HostId(names[*i].clone())),
        Ask::Error(0, ixs) => r.error(&Error::PermissionDenied(path(ixs))),
        Ask::Error(1, ixs) => r.error(&Error::NotFound(path(ixs))),
        Ask::Error(_, ixs) => r.error(&Error::AlreadyExists(path(ixs))),
    }
}

#[hegel::test]
fn a_stand_in_depends_on_a_name_only_through_its_dot_and_extension(tc: TestCase) {
    let pool = tc.draw(gs::vecs(name().map(component)).unique(true).min_size(1).max_size(8));
    let asks = tc.draw(gs::vecs(ask(pool.len())).max_size(24));
    let twins: Vec<String> = pool.iter().enumerate().map(|(i, n)| twin(n, i)).collect();
    let (mut real, mut other) = (Redactor::default(), Redactor::default());
    for a in &asks {
        let said = answer(&mut real, &pool, a);
        assert_eq!(said, answer(&mut other, &twins, a), "{a:?}: the stand-in told the names apart");
    }
}

#[hegel::test]
fn a_stand_in_holds_no_character_but_its_own_and_a_kept_extensions(tc: TestCase) {
    let pool = tc.draw(gs::vecs(name().map(component)).unique(true).min_size(1).max_size(8));
    let asks = tc.draw(gs::vecs(ask(pool.len())).max_size(24));
    let mut r = Redactor::default();
    for a in asks.iter().filter(|a| !matches!(a, Ask::Error(..))) {
        let said = answer(&mut r, &pool, a);
        assert!(said.chars().all(|c| c.is_ascii_alphanumeric() || "-./<>".contains(c)), "{a:?} gave {said:?}");
    }
}

#[hegel::test]
fn a_hidden_name_keeps_its_dot_and_a_file_only_a_short_plain_extension(tc: TestCase) {
    let name = tc.draw(name());
    let dir = tc.draw(gs::booleans());
    let mut r = Redactor::default();
    let stand_in = r.name(&name, dir);
    assert_eq!(stand_in.starts_with('.'), name.starts_with('.'), "{stand_in:?}");
    let body = stand_in.strip_prefix(lead(&name)).unwrap_or_else(|| panic!("{stand_in:?} for {name:?}"));
    let want = match (dir, kept_extension(&name)) {
        (true, _) => "dir-1".to_owned(),
        (false, Some(ext)) => format!("file-1.{ext}"),
        (false, None) => "file-1".to_owned(),
    };
    assert_eq!(body, want);
    let path = RemotePath::new(&format!("/srv/{}", component(name.clone())));
    let sentence = Redactor::default().path(&path);
    match path.file_name().and_then(kept_extension) {
        Some(ext) => assert_eq!(sentence, format!("<dir>/file-1.{ext}")),
        None => assert_eq!(sentence, "<path>"),
    }
}

#[hegel::test]
fn the_same_name_gets_the_same_stand_in_and_different_names_different_ones(tc: TestCase) {
    let pool = tc.draw(gs::vecs(name()).unique(true).min_size(1).max_size(8));
    let i = gs::integers::<usize>().max_value(pool.len() - 1);
    let asks = tc.draw(gs::vecs(hegel::tuples!(i, gs::integers::<u8>().max_value(2))).max_size(30));
    let mut r = Redactor::default();
    let mut given: HashMap<(usize, u8), String> = HashMap::new();
    for (i, how) in asks {
        let stand_in = match how {
            0 => r.name(&pool[i], false),
            1 => r.name(&pool[i], true),
            _ => r.host(&HostId(pool[i].clone())),
        };
        if let Some(before) = given.get(&(i, how)) {
            assert_eq!(&stand_in, before, "{:?} changed its stand-in", pool[i]);
        }
        let clash = given.iter().find(|(k, v)| **k != (i, how) && **v == stand_in);
        assert!(clash.is_none(), "{:?} and {clash:?} share {stand_in:?}", (&pool[i], how));
        given.insert((i, how), stand_in);
    }
}

#[hegel::test]
fn a_free_form_error_says_only_that_it_was_withheld(tc: TestCase) {
    let message = tc.draw(gs::text());
    let how = tc.draw(gs::integers::<u8>().max_value(2));
    let error = match how {
        0 => Error::Connect(message),
        1 => Error::Io(message),
        _ => Error::Other(message),
    };
    assert_eq!(Redactor::default().error(&error), WITHHELD);
}

/// One field of a trace line after its kind: a host by its index, or a `key=value` word.
#[derive(Clone, Debug)]
enum Field {
    Host(usize),
    Word(String, String),
}

#[hegel::composite]
fn field(tc: &TestCase, hosts: usize) -> Field {
    if tc.draw(gs::booleans()) {
        Field::Host(tc.draw(gs::integers::<usize>().max_value(hosts - 1)))
    } else {
        // No `h` in a key: `host=` only starts a host's field.
        Field::Word(tc.draw(gs::from_regex("[a-gi-z]{1,5}")), tc.draw(gs::from_regex("[A-Za-z0-9]{0,5}")))
    }
}

#[hegel::test]
fn a_trace_line_keeps_its_words_and_names_each_host_by_a_stand_in(tc: TestCase) {
    let ids = tc.draw(gs::vecs(name()).unique(true).min_size(1).max_size(5));
    let known = tc.draw(gs::vecs(gs::booleans()).min_size(ids.len()).max_size(ids.len()));
    let kind = tc.draw(gs::sampled_from(vec!["Status", "Opened", "Listing", "Weird"]));
    let fields = tc.draw(gs::vecs(field(ids.len())).max_size(6));
    // An id the app doesn't know has no space: the line's next word starts at one.
    let ids: Vec<String> =
        ids.iter().zip(&known).map(|(id, k)| if *k { id.clone() } else { id.replace(' ', "_") }).collect();
    tc.assume(ids.iter().enumerate().all(|(i, a)| ids[..i].iter().all(|b| a != b)));
    let known_ids: Vec<&str> = ids.iter().zip(&known).filter(|(_, k)| **k).map(|(id, _)| id.as_str()).collect();
    let text = |f: &Field| match f {
        Field::Host(i) => format!("host={}", ids[*i]),
        Field::Word(k, v) => format!("{k}={v}"),
    };
    let line: String = std::iter::once(kind.to_owned()).chain(fields.iter().map(text)).collect::<Vec<_>>().join(" ");
    // Where a known id holding spaces would swallow the words after a host, the line is
    // ambiguous by design (the longest known id wins): those lines aren't this property's.
    let mut at = kind.len();
    for f in &fields {
        let rest = &line[at + 1..];
        if let Field::Host(i) = f {
            let after = &rest["host=".len()..];
            let swallows = known_ids.iter().any(|k| {
                !k.is_empty()
                    && k.len() > ids[*i].len()
                    && after.starts_with(k)
                    && (after.len() == k.len() || after[k.len()..].starts_with(' '))
            });
            tc.assume(!swallows);
        }
        at += 1 + text(f).len();
    }
    let mut numbers: Vec<usize> = Vec::new();
    let want: Vec<String> = std::iter::once(kind.to_owned())
        .chain(fields.iter().map(|f| match f {
            Field::Host(i) => {
                if !numbers.contains(i) {
                    numbers.push(*i);
                }
                format!("host=host-{}", numbers.iter().position(|n| n == i).map_or(0, |p| p + 1))
            }
            word => text(word),
        }))
        .collect();
    tc.note(&format!("line: {line:?}, known: {known_ids:?}"));
    assert_eq!(Redactor::default().trace(&line, &known_ids), want.join(" "));
}

/// `names` as one folder's entries, folders (`dirs`) or files.
fn entries(names: &[String], dirs: &[bool]) -> Vec<Entry> {
    let kind = |dir: bool| if dir { EntryKind::Dir } else { EntryKind::File };
    let entry = |(name, dir): (&String, &bool)| Entry {
        name: name.clone(),
        kind: kind(*dir),
        size: 0,
        mtime: None,
        mode: None,
        owner: None,
        group: None,
    };
    names.iter().zip(dirs).map(entry).collect()
}

/// A recording's folder in stand-ins, as `record_fixture.rs` names it: its entries sorted by
/// name, then each named in that order; the names as written, and as the list sorts them.
fn named_in_order(r: &mut Redactor, mut folder: Vec<Entry>) -> (Vec<String>, Vec<String>) {
    sort_entries(&mut folder, SortKey::Name, SortDir::Ascending);
    let mut stand_ins: Vec<Entry> =
        folder.into_iter().map(|e| Entry { name: r.name(&e.name, e.kind.is_dir_like()), ..e }).collect();
    let written: Vec<String> = stand_ins.iter().map(|e| e.name.clone()).collect();
    sort_entries(&mut stand_ins, SortKey::Name, SortDir::Ascending);
    (written, stand_ins.into_iter().map(|e| e.name).collect())
}

#[hegel::test]
#[hegel::explicit_test_case(names = vec![".x".to_owned(), "-".to_owned()], dirs = vec![false, false])]
fn a_folders_stand_ins_sort_as_its_names_do(tc: TestCase) {
    let names = tc.draw(gs::vecs(name().map(component)).unique(true).min_size(1).max_size(8));
    let dirs = tc.draw(gs::vecs(gs::booleans()).min_size(names.len()).max_size(names.len()));
    let (written, sorted) = named_in_order(&mut Redactor::default(), entries(&names, &dirs));
    assert_eq!(written, sorted, "sorting the stand-ins by name changed the folder's order");
}

#[hegel::test]
#[hegel::explicit_test_case(folders = vec![
    vec![("e".to_owned(), false)],
    vec![("a".to_owned(), false), ("e".to_owned(), false)],
])]
#[hegel::explicit_test_case(folders = vec![vec![("-".to_owned(), true), (".0".to_owned(), true)]])]
fn every_folders_stand_ins_sort_as_its_names_do_through_one_redactor(tc: TestCase) {
    // Names that are often in more than one folder (a few letters), or any; folders or files.
    let names = hegel::one_of!(gs::from_regex("[a-e]{1,2}"), name().map(component));
    let folder = gs::vecs(hegel::tuples!(names, gs::booleans())).unique(true).max_size(5);
    let folders: Vec<Vec<(String, bool)>> = tc.draw(gs::vecs(folder).max_size(4));
    let mut r = Redactor::default();
    // As a recording's fixture does: every name numbered first, then each folder named.
    r.number_in_order(folders.iter().flatten().map(|(n, dir)| (n.as_str(), *dir)));
    for folder in &folders {
        // A name is one entry in a folder: the first drawn of it.
        let mut seen = std::collections::HashSet::new();
        let folder: Vec<&(String, bool)> = folder.iter().filter(|(n, _)| seen.insert(n.clone())).collect();
        let names: Vec<String> = folder.iter().map(|(n, _)| n.clone()).collect();
        let dirs: Vec<bool> = folder.iter().map(|(_, d)| *d).collect();
        let (written, sorted) = named_in_order(&mut r, entries(&names, &dirs));
        assert_eq!(written, sorted, "{names:?}: sorting the stand-ins by name changed the folder's order");
    }
}
