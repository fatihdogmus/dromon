//! Fixture parsing (errors name their JSON path), listing rules, the generator, the demo.

use dromon_core::{EntryKind, Error, HostSource, LinkTarget, RemotePath};

use super::generate::{Pattern, Rng, hash};
use super::parse::rfc3339;
use super::*;

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

/// The error `src` fails with.
fn error(src: &str) -> String {
    match parse(src) {
        Ok(_) => panic!("parsed: {src}"),
        Err(e) => e.to_string(),
    }
}

/// A fixture of one host `h` whose server is `server` (JSON).
fn with_server(server: &str) -> String {
    format!(r#"{{"version": 1, "hosts": [{{"alias": "h", "source": "saved"}}], "servers": {{"h": {server}}}}}"#)
}

fn tree(folders: &str) -> Fixture {
    parse(&with_server(&format!(r#"{{"home": "/home/u", "tree": {folders}}}"#))).unwrap()
}

fn names(entries: &[dromon_core::Entry]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}

#[test]
fn a_minimal_fixture() {
    let fx = parse(r#"{"version": 1}"#).unwrap();
    assert_eq!(fx, Fixture { now: None, hosts: vec![], servers: Default::default(), transfers: vec![] });
}

#[test]
fn hosts_and_servers() {
    let fx = parse(
        r#"{"version": 1, "now": 1795615200, "description": "x",
            "hosts": [{"alias": "a", "source": "ssh-config", "user": "u", "hostname": "a.example", "port": 2222},
                      {"alias": "b", "source": "recent"}],
            "servers": {"a": {"home": "/srv//a/", "auth": [{"password": {"accept": "pw"}}], "fail": "boom"}}}"#,
    )
    .unwrap();
    assert_eq!(fx.now, Some(1_795_615_200));
    assert_eq!(fx.hosts[0].source, HostSource::SshConfig);
    assert_eq!((fx.hosts[0].user.as_deref(), fx.hosts[0].port), (Some("u"), Some(2222)));
    assert_eq!(fx.hosts[1].hostname, None);
    let a = fx.server("a").unwrap();
    assert_eq!(a.home, p("/srv/a"));
    assert_eq!(a.auth, [Step::Password { prompt: None, accept: "pw".into() }]);
    assert_eq!(a.fail.as_deref(), Some("boom"));
    assert!(fx.server("b").is_none());
}

#[test]
fn entries_may_name_their_owner_and_group() {
    let fx = tree(r#"{"/w": [{"name": "a", "kind": "file", "owner": "alice", "group": "staff"}, {"name": "b", "kind": "file", "owner": "1001"}, {"name": "c", "kind": "dir"}]}"#);
    let listed = fx.server("h").unwrap().list(&p("/w")).unwrap();
    let owners: Vec<_> = listed.iter().map(|e| (e.owner.as_deref(), e.group.as_deref())).collect();
    assert_eq!(owners, [(Some("alice"), Some("staff")), (Some("1001"), None), (None, None)], "left out: unknown");
    // An entry serialized before owners existed still reads.
    let old: dromon_core::Entry = serde_json::from_str(r#"{"name": "a", "kind": "File", "size": 1, "mtime": null, "mode": null}"#).unwrap();
    assert_eq!((old.owner, old.group), (None, None));
}

#[test]
fn entry_errors_name_their_json_path() {
    let entry = |e: &str| error(&with_server(&format!(r#"{{"home": "/", "tree": {{"/var": [{{"name": "a", "kind": "dir"}}, {{"name": "b", "kind": "file"}}, {e}]}}}}"#)));
    assert_eq!(entry(r#"{"name": "c", "kind": "folder"}"#), r#"servers.h.tree./var: entry 3: unknown kind "folder" (expected file, dir, symlink or other)"#);
    assert_eq!(entry(r#"{"name": "c", "kind": "file", "mtime": "yesterday"}"#), r#"servers.h.tree./var: entry 3: mtime: "yesterday" is not an RFC 3339 date (2026-11-25T14:00:00Z) or epoch seconds"#);
    assert_eq!(entry(r#"{"kind": "file"}"#), "servers.h.tree./var: entry 3: name: missing");
    assert_eq!(entry(r#"{"name": "a", "kind": "file"}"#), r#"servers.h.tree./var: entry 3: "a" is listed twice"#);
    assert_eq!(entry(r#"{"name": "c", "kind": "file", "target": "dir"}"#), "servers.h.tree./var: entry 3: target: only a symlink has a target");
    assert_eq!(entry(r#"{"name": "c/d", "kind": "file"}"#), r#"servers.h.tree./var: entry 3: name: "c/d" is not a file name"#);
    assert_eq!(entry(r#"{"name": "c", "kind": "file", "sise": 1}"#), r#"servers.h.tree./var: entry 3: unknown field "sise" (expected name, kind, target, size, mtime, mode, owner, group)"#);
    assert_eq!(entry(r#"{"name": "c", "kind": "file", "owner": 501}"#), "servers.h.tree./var: entry 3: owner: expected a string, found a number");
    assert_eq!(entry(r#"{"name": "c", "kind": "file", "group": ""}"#), "servers.h.tree./var: entry 3: group: must not be empty");
    assert_eq!(entry(r#"{"name": "c", "kind": "file", "size": -1}"#), "servers.h.tree./var: entry 3: size: expected a whole number of at least 0, found -1");
    assert_eq!(entry(r#"{"generate": {"pattern": "IMG.jpg", "count": 3}}"#), r#"servers.h.tree./var: entry 3: generate.pattern: "IMG.jpg" needs exactly one {} or {0N} (N digits, zero-padded) for the number"#);
    assert_eq!(entry(r#"{"generate": {"pattern": "a{}", "count": 3, "size": [5, 1]}}"#), "servers.h.tree./var: entry 3: generate.size: [5, 1]: min is above max");
    assert_eq!(entry(r#"{"generate": {"pattern": "a{}"}}"#), "servers.h.tree./var: entry 3: generate.count: missing");
}

#[test]
fn errors_name_their_json_path() {
    assert_eq!(error("[]"), "a fixture is a JSON object, found a list");
    assert!(error("{").starts_with("not valid JSON: "));
    assert_eq!(error("{}"), "version: missing (this Dromon reads version 1)");
    assert_eq!(error(r#"{"version": 2}"#), "version: 2 is not supported (this Dromon reads version 1)");
    assert_eq!(error(r#"{"version": 1, "hostz": []}"#), r#"unknown field "hostz" (expected version, description, now, hosts, servers, transfers)"#);
    assert_eq!(error(r#"{"version": 1, "now": "soon"}"#), r#"now: "soon" is not an RFC 3339 date (2026-11-25T14:00:00Z) or epoch seconds"#);
    assert_eq!(error(r#"{"version": 1, "hosts": [{"alias": "a", "source": "saved"}, {"alias": "b", "source": "config"}]}"#), r#"hosts: host 2: source: unknown source "config" (expected ssh-config, saved or recent)"#);
    assert_eq!(error(r#"{"version": 1, "hosts": [{"alias": "a", "source": "saved", "port": 70000}]}"#), "hosts: host 1: port: 70000 is not a port (1–65535)");
    assert_eq!(error(r#"{"version": 1, "hosts": [{"alias": "a", "source": "saved"}, {"alias": "a", "source": "saved"}]}"#), r#"hosts: host 2: alias: "a" is already a host"#);
    assert_eq!(error(r#"{"version": 1, "servers": {"ghost": {"home": "/"}}}"#), "servers.ghost: no host has this alias (add it to hosts)");
    assert_eq!(error(&with_server("{}")), "servers.h.home: missing");
    assert_eq!(error(&with_server(r#"{"home": "~"}"#)), r#"servers.h.home: "~" is not an absolute path"#);
    assert_eq!(error(&with_server(r#"{"home": "/", "hmoe": "/"}"#)), r#"servers.h: unknown field "hmoe" (expected home, auth, tree, fail, changes)"#);
    assert_eq!(error(&with_server(r#"{"home": "/", "tree": {"var": []}}"#)), r#"servers.h.tree.var: "var" is not an absolute path"#);
    assert_eq!(error(&with_server(r#"{"home": "/", "tree": {"/a": [], "/a/": []}}"#)), "servers.h.tree./a/: lists /a again (paths are normalized)");
    assert_eq!(error(&with_server(r#"{"home": "/", "tree": {"/a": {"error": "gone"}}}"#)), r#"servers.h.tree./a.error: unknown error "gone" (expected permission-denied, not-found or connection-lost)"#);
    assert_eq!(error(&with_server(r#"{"home": "/", "tree": {"/a": "x"}}"#)), r#"servers.h.tree./a: expected a list of entries or {"error": …}, found a string"#);
}

#[test]
fn auth_step_errors() {
    let step = |s: &str| error(&with_server(&format!(r#"{{"home": "/", "auth": [{{"notice": {{"text": "n"}}}}, {s}]}}"#)));
    assert_eq!(step(r#"{"password": {}}"#), "servers.h.auth: step 2: password.accept: missing");
    assert_eq!(step(r#"{"otp": {"accept": "1"}}"#), r#"servers.h.auth: step 2: otp: unknown step "otp" (expected host-key, password, passphrase, code, confirm or notice)"#);
    assert_eq!(step(r#"{"password": {"accept": "a"}, "code": {"accept": "b"}}"#), "servers.h.auth: step 2: a step is an object with one field: host-key, password, passphrase, code, confirm or notice");
    assert_eq!(step(r#"{"host-key": {"key_type": "ED25519"}}"#), "servers.h.auth: step 2: host-key.fingerprint: missing");
    assert_eq!(step(r#"{"passphrase": {"key_path": "/k", "accept": "a", "prompt": "p"}}"#), r#"servers.h.auth: step 2: passphrase: unknown field "prompt" (expected key_path, accept)"#);
    assert_eq!(step(r#"{"confirm": {"text": ""}}"#), "servers.h.auth: step 2: confirm.text: must not be empty");
    // Every kind parses.
    let fx = parse(&with_server(
        r#"{"home": "/", "auth": [{"host-key": {"key_type": "ED25519", "fingerprint": "SHA256:x"}}, {"password": {"prompt": "pw: ", "accept": "a"}},
            {"passphrase": {"key_path": "/k", "accept": ""}}, {"code": {"accept": "123"}}, {"confirm": {"text": "ok?"}}, {"notice": {"text": "touch"}}]}"#,
    ))
    .unwrap();
    assert_eq!(fx.server("h").unwrap().auth.len(), 6);
}

#[test]
fn rfc3339_dates() {
    assert_eq!(rfc3339("2026-11-25T14:00:00Z"), Some(DEMO_NOW));
    assert_eq!(rfc3339("2026-11-25 16:00:00+02:00"), Some(DEMO_NOW));
    assert_eq!(rfc3339("2026-11-25t13:30:00.123-00:30"), Some(DEMO_NOW));
    assert_eq!(rfc3339("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(rfc3339("1969-12-31T23:59:59Z"), Some(-1));
    assert_eq!(rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
    for bad in ["2026-02-29T00:00:00Z", "2026-11-25T14:00:00", "2026-11-25", "2026-13-01T00:00:00Z", "2026-11-25T24:00:00Z", "2026-11-25T14:00:00.Z", "x"] {
        assert_eq!(rfc3339(bad), None, "{bad}");
    }
}

#[test]
fn entries_take_defaults_by_kind() {
    let fx = tree(
        r#"{"/d": [{"name": "f", "kind": "file"}, {"name": "d", "kind": "dir"}, {"name": "l", "kind": "symlink", "target": "dir"},
                    {"name": "dangling", "kind": "symlink", "target": null}, {"name": "s", "kind": "other"},
                    {"name": "x", "kind": "file", "mode": "0755", "size": 7, "mtime": "2026-11-25T14:00:00Z"}, {"name": "y", "kind": "file", "mode": 33261}]}"#,
    );
    let e = fx.server("h").unwrap().list(&p("/d")).unwrap();
    assert_eq!((e[0].kind, e[0].size, e[0].mode, e[0].mtime), (EntryKind::File, 0, Some(0o100_644), None));
    assert_eq!((e[1].kind, e[1].size, e[1].mode), (EntryKind::Dir, 4096, Some(0o040_755)));
    assert_eq!(e[2].kind, EntryKind::Symlink { target: Some(LinkTarget::Dir) });
    assert_eq!(e[3].kind, EntryKind::Symlink { target: None });
    assert_eq!((e[4].kind, e[4].mode), (EntryKind::Other, None));
    assert_eq!((e[5].mode, e[5].size, e[5].mtime), (Some(0o100_755), 7, Some(DEMO_NOW)));
    assert_eq!(e[6].mode, Some(0o100_755));
}

#[test]
fn folders_listed_implied_empty_and_missing() {
    let fx = tree(
        r#"{"/srv/data": [{"name": "sub", "kind": "dir"}, {"name": "link", "kind": "symlink", "target": "dir"}, {"name": "f", "kind": "file"}],
            "/srv/data/locked": {"error": "permission-denied"}, "/srv/gone": {"error": "not-found"}, "/srv/flaky": {"error": "connection-lost"}}"#,
    );
    let s = fx.server("h").unwrap();
    // Folders on the way to listed ones, and to home, list just those subfolders.
    assert_eq!(names(&s.list(&p("/")).unwrap()), ["home", "srv"]);
    assert_eq!(names(&s.list(&p("/srv")).unwrap()), ["data", "flaky", "gone"]);
    assert!(s.list(&p("/")).unwrap().iter().all(|e| e.kind == EntryKind::Dir && e.mtime.is_none()));
    assert_eq!(names(&s.list(&p("/home")).unwrap()), ["u"]);
    assert!(s.list(&p("/home/u")).unwrap().is_empty(), "home exists");
    // A folder (or a link to one) a listing names, but that isn't listed, is empty.
    assert!(s.list(&p("/srv/data/sub")).unwrap().is_empty());
    assert!(s.list(&p("/srv/data/link")).unwrap().is_empty());
    // Errors, and what doesn't exist.
    assert!(matches!(s.list(&p("/srv/data/locked")), Err(Error::PermissionDenied(_))));
    assert!(matches!(s.list(&p("/srv/gone")), Err(Error::NotFound(_))));
    assert!(matches!(s.list(&p("/srv/flaky")), Err(Error::ConnectionLost)));
    for missing in ["/srv/data/f", "/srv/data/nope", "/nope", "/srv/data/sub/deeper"] {
        assert!(matches!(s.list(&p(missing)), Err(Error::NotFound(_))), "{missing}");
    }
}

#[test]
fn patterns() {
    let pat = Pattern::parse("IMG_{05}.jpg").unwrap();
    assert_eq!((pat.name(1), pat.name(123_456)), ("IMG_00001.jpg".into(), "IMG_123456.jpg".into()));
    assert_eq!(Pattern::parse("f{}").unwrap().name(7), "f7");
    for bad in ["f", "f{5}", "f{}{}", "f{0}", "f{0x}", "{"] {
        assert!(Pattern::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn generators_are_deterministic_and_seeded_by_folder() {
    let fx = tree(
        r#"{"/g": [{"name": "first", "kind": "dir"},
                   {"generate": {"pattern": "f{03}.txt", "count": 3, "start": 0, "size": [10, 20], "mtime": {"end": 1000000, "every": 600, "jitter": 120}}},
                   {"generate": {"pattern": "d{}", "count": 2, "kind": "dir", "mtime": 5}}],
            "/h": [{"generate": {"pattern": "f{03}.txt", "count": 3, "start": 0, "size": [10, 20], "mtime": {"end": 1000000, "every": 600, "jitter": 120}}}]}"#,
    );
    let s = fx.server("h").unwrap();
    let g = s.list(&p("/g")).unwrap();
    assert_eq!(names(&g), ["first", "f000.txt", "f001.txt", "f002.txt", "d1", "d2"]);
    assert_eq!(g, s.list(&p("/g")).unwrap(), "the same on every listing");
    assert!(g[1..4].iter().all(|e| (10..=20).contains(&e.size) && e.kind == EntryKind::File));
    for (i, e) in g[1..4].iter().enumerate() {
        let t = e.mtime.unwrap();
        let base = 1_000_000 - (3 - i as i64) * 600;
        assert!(t <= base && t >= base - 120 && (base - t) % 60 == 0, "{t}");
    }
    assert_eq!((g[4].kind, g[4].size, g[4].mtime), (EntryKind::Dir, 4096, Some(5)));
    // Another folder draws other numbers.
    let h = s.list(&p("/h")).unwrap();
    assert_ne!(h.iter().map(|e| e.size).collect::<Vec<_>>(), g[1..4].iter().map(|e| e.size).collect::<Vec<_>>());
}

/// The demo's 10,000 photos are exactly the old mock's.
#[test]
fn the_demo_big_folder_matches_the_mock_formula() {
    let big = demo().server("prod").unwrap().list(&p("/srv/media/photos")).unwrap();
    assert_eq!(big.len(), 10_000);
    let mut rng = Rng(hash("/srv/media/photos"));
    for (i, e) in big.iter().enumerate() {
        let size = 1_500_000 + rng.below(4_800_000);
        let mtime = DEMO_NOW - ((10_000 - i) as i64) * 52 * 60 - rng.below(40) as i64 * 60;
        assert_eq!((e.name.as_str(), e.size, e.mtime, e.mode), (format!("IMG_{:05}.jpg", i + 1).as_str(), size, Some(mtime), Some(0o100_644)));
    }
}

#[test]
fn the_demo() {
    let fx = demo();
    assert_eq!(fx.now, Some(DEMO_NOW));
    assert_eq!(fx.hosts.len(), 9);
    assert!(fx.hosts.iter().all(|h| fx.server(&h.alias).is_some_and(|s| s.auth.is_empty())), "every demo host connects at once");
    let prod = fx.server("prod").unwrap();
    let site = prod.list(&prod.home).unwrap();
    assert_eq!(site.len(), 17);
    assert!(site.iter().filter(|e| e.hidden()).count() >= 2);
    assert!(site.iter().any(|e| e.kind.is_symlink() && e.kind.is_dir_like()));
    assert!(matches!(prod.list(&p("/root")), Err(Error::PermissionDenied(_))));
    assert!(prod.list(&p("/var/www/site/uploads")).unwrap().is_empty());
    assert!(!prod.list(&p("/var/www/site/current")).unwrap().is_empty());
    assert!(matches!(prod.list(&p("/nope")), Err(Error::NotFound(_))));
}

#[test]
fn the_fixture_flag() {
    let a = |v: &[&str]| crate::fixture_arg(&v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
    assert_eq!(a(&[]), Ok(None));
    assert_eq!(a(&["--other", "x"]), Ok(None));
    assert_eq!(a(&["--fixture", "f.json"]), Ok(Some("f.json".into())));
    assert_eq!(a(&["x", "--fixture=f.json"]), Ok(Some("f.json".into())));
    assert!(a(&["--fixture"]).is_err());
    assert!(a(&["--fixture", "a", "--fixture", "b"]).is_err());
}

#[test]
fn loading_reports_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    std::fs::write(&path, r#"{"version": 1, "hosts": [{"alias": "a"}]}"#).unwrap();
    let path = path.to_str().unwrap();
    assert_eq!(load(path).unwrap_err(), format!("invalid fixture {path}: hosts: host 1: source: missing"));
    assert!(load("/nonexistent/x.json").unwrap_err().starts_with("can't read the fixture /nonexistent/x.json: "));
}
