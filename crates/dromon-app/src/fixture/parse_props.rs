//! Properties of the fixture parser, for any input: it never panics on any text, any JSON
//! value, any fixture-shaped document or any change to the demo; and it fails on what isn't a
//! fixture: text that isn't JSON, JSON without `"version": 1`, an unknown field anywhere, a
//! required field left out, and a field holding a value its kind can't take.

use hegel::TestCase;
use hegel::generators::{self as gs, Generator};
use serde_json::{Map, Number, Value, json};

use super::*;

/// Every field name a fixture has, so drawn objects reach deep.
const FIELDS: &[&str] = &[
    "version",
    "description",
    "now",
    "hosts",
    "servers",
    "transfers",
    "alias",
    "source",
    "user",
    "hostname",
    "port",
    "home",
    "auth",
    "tree",
    "fail",
    "changes",
    "name",
    "kind",
    "target",
    "size",
    "mtime",
    "mode",
    "owner",
    "group",
    "generate",
    "pattern",
    "count",
    "start",
    "seed",
    "end",
    "every",
    "jitter",
    "error",
    "add",
    "remove",
    "modify",
    "host-key",
    "key_type",
    "fingerprint",
    "password",
    "passphrase",
    "key_path",
    "code",
    "confirm",
    "notice",
    "text",
    "prompt",
    "accept",
    "state",
    "found",
    "done",
    "total",
    "files",
    "speed",
    "eta",
    "waiting",
    "conflict",
    "folder",
    "dir",
    "more",
    "existing",
    "incoming",
    "reveal",
    "errors",
    "skipped",
    "title",
    "host",
    "/",
    "/a",
];

/// Strings a fixture's values are made of, and any text.
#[hegel::composite]
fn word(tc: &TestCase) -> String {
    let words: Vec<String> = [
        "",
        "a",
        "/",
        "/a",
        "/a/b",
        "..",
        "prod",
        "dir",
        "file",
        "symlink",
        "other",
        "saved",
        "recent",
        "ssh-config",
        "download",
        "upload",
        "queued",
        "preparing",
        "running",
        "paused",
        "done",
        "failed",
        "cancelled",
        "permission-denied",
        "not-found",
        "connection-lost",
        "f-{}.txt",
        "IMG_{05}.jpg",
        "{099999999999999999999}",
        "0644",
        "0o777",
        "77777777777",
        "2026-11-25T14:00:00Z",
        "9999-12-31T23:59:60+23:59",
        "0000-01-01T00:00:00Z",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    tc.draw(hegel::one_of!(gs::sampled_from(words), gs::text().max_size(12)))
}

/// Numbers at every edge: small ones, whole ones of either sign up to the type's limits, and
/// finite floats, tiny and huge.
#[hegel::composite]
fn number(tc: &TestCase) -> Value {
    tc.draw(hegel::one_of!(
        gs::integers::<u64>().max_value(10).map(Value::from),
        gs::integers::<i64>().map(Value::from),
        gs::integers::<u64>().map(Value::from),
        gs::sampled_from(vec![0.5, -1.0, 1e19, 1.8e19, 1e300, f64::MAX, f64::MIN_POSITIVE]).map(Value::from),
        gs::floats::<f64>().allow_nan(false).allow_infinity(false).map(|f| Value::Number(Number::from_f64(f).unwrap())),
    ))
}

/// Any JSON value, its objects' keys mostly a fixture's field names.
fn json() -> impl hegel::PrintableGenerator<Value> {
    let leaf =
        hegel::one_of!(gs::just(Value::Null), gs::booleans().map(Value::Bool), number(), word().map(Value::String),);
    gs::recursive(leaf, |sub| {
        let pair = hegel::tuples!(key(), sub.clone());
        hegel::one_of!(
            gs::vecs(sub).max_size(4).map(Value::Array),
            gs::vecs(pair).max_size(5).map(|kv| Value::Object(kv.into_iter().collect::<Map<_, _>>())),
        )
    })
    .max_leaves(40)
}

#[hegel::composite]
fn key(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(gs::sampled_from(FIELDS.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()), word()))
}

/// One of `valid`, or now and then anything at all (seldom, so most documents get deep).
fn one(tc: &TestCase, valid: Vec<Value>) -> Value {
    if tc.draw(gs::weighted_booleans(0.97)) {
        tc.draw(gs::sampled_from(valid))
    } else {
        let date = r"\d{4}-\d{2}-\d{2}[Tt ]\d{2}:\d{2}:\d{2}(\.\d{1,3})?(Z|[+-]\d{2}:\d{2})";
        tc.draw(hegel::one_of!(json(), number(), word().map(Value::String), gs::from_regex(date).map(Value::String)))
    }
}

/// An object of the fields drawn present, each there with the chance given.
fn object_of(tc: &TestCase, fields: Vec<(&str, f64, Value)>) -> Value {
    let mut m = Map::new();
    for (k, chance, v) in fields {
        if tc.draw(gs::weighted_booleans(chance)) {
            m.insert(k.to_owned(), v);
        }
    }
    Value::Object(m)
}

/// A time: epoch seconds of either sign, or an RFC 3339 date.
fn time(tc: &TestCase) -> Value {
    let any = Value::from(tc.draw(gs::integers::<i64>()));
    one(tc, vec![json!(1_795_615_200), json!("2026-11-25T14:00:00Z"), json!("2026-11-25 14:00:00.5+01:00"), any])
}

/// A whole number of at least 0, up to the limit.
fn whole(tc: &TestCase) -> Value {
    let any = Value::from(tc.draw(gs::integers::<u64>()));
    one(tc, vec![json!(0), json!(3), json!(u64::MAX), any])
}

fn kind(tc: &TestCase) -> Value {
    one(tc, ["file", "dir", "symlink", "other"].map(Value::from).to_vec())
}

fn mode(tc: &TestCase) -> Value {
    one(tc, vec![json!("0644"), json!("100755"), json!(420)])
}

/// Seconds, a speed: a number of at least 0, tiny or huge.
fn seconds(tc: &TestCase) -> Value {
    let any = tc.draw(gs::floats::<f64>().min_value(0.0).allow_nan(false).allow_infinity(false));
    one(tc, vec![json!(0), json!(1.5), json!(1e19), json!(1e300), json!(f64::MAX), json!(any)])
}

#[hegel::composite]
fn entry(tc: &TestCase) -> Value {
    if tc.draw(gs::weighted_booleans(0.2)) {
        let mtime = object_of(tc, vec![("end", 0.9, time(tc)), ("every", 0.9, whole(tc)), ("jitter", 0.5, whole(tc))]);
        let patterns = vec![json!("f-{}.txt"), json!("IMG_{05}.jpg"), json!("{020}")];
        let counts = vec![json!(0), json!(3), json!(1_000_000)];
        let sizes = vec![json!([1, 5]), json!([0, u64::MAX]), json!(7)];
        let generate = object_of(
            tc,
            vec![
                ("pattern", 0.95, one(tc, patterns)),
                ("count", 0.95, one(tc, counts)),
                ("start", 0.3, whole(tc)),
                ("kind", 0.3, kind(tc)),
                ("size", 0.4, one(tc, sizes)),
                ("mtime", 0.4, one(tc, vec![mtime, time(tc)])),
                ("mode", 0.2, mode(tc)),
                ("seed", 0.2, json!("s")),
            ],
        );
        return json!({ "generate": generate });
    }
    let name = Value::String(tc.draw(gs::from_regex("[a-z][a-z.]{0,7}")));
    let targets = vec![json!("file"), json!("dir"), json!("other"), Value::Null];
    object_of(
        tc,
        vec![
            ("name", 0.95, one(tc, vec![name])),
            ("kind", 0.95, kind(tc)),
            ("target", 0.2, one(tc, targets)),
            ("size", 0.5, whole(tc)),
            ("mtime", 0.5, time(tc)),
            ("mode", 0.3, mode(tc)),
            ("owner", 0.2, json!("u")),
        ],
    )
}

#[hegel::composite]
fn job(tc: &TestCase, hosts: Vec<String>) -> Value {
    let host = match hosts.is_empty() {
        true => Value::String(tc.draw(word())),
        false => one(tc, hosts.into_iter().map(Value::String).collect()),
    };
    let side = |tc: &TestCase| object_of(tc, vec![("size", 0.5, whole(tc)), ("mtime", 0.5, time(tc))]);
    let states = ["queued", "preparing", "running", "paused", "done", "failed", "cancelled"].map(Value::from).to_vec();
    let conflict = object_of(
        tc,
        vec![
            ("name", 0.9, json!("n")),
            ("folder", 0.9, one(tc, vec![json!("/a"), json!("a")])),
            ("dir", 0.3, Value::Bool(tc.draw(gs::booleans()))),
            ("more", 0.3, whole(tc)),
            ("existing", 0.5, side(tc)),
            ("incoming", 0.5, side(tc)),
        ],
    );
    let files = vec![json!([0, 1]), json!([3, 3])];
    object_of(
        tc,
        vec![
            ("kind", 0.95, one(tc, vec![json!("download"), json!("upload")])),
            ("host", 0.95, host),
            ("title", 0.95, one(tc, vec![json!("t")])),
            ("state", 0.95, one(tc, states)),
            ("found", 0.15, whole(tc)),
            ("error", 0.15, one(tc, vec![json!("e")])),
            ("done", 0.4, one(tc, vec![json!(0), json!(5)])),
            ("total", 0.4, one(tc, vec![json!(0), json!(10)])),
            ("files", 0.3, one(tc, files)),
            ("speed", 0.4, seconds(tc)),
            ("eta", 0.4, seconds(tc)),
            ("waiting", 0.3, one(tc, vec![json!(true), json!(false)])),
            ("conflict", 0.2, conflict),
            ("reveal", 0.2, one(tc, vec![json!("/a"), json!("a")])),
            ("errors", 0.2, one(tc, vec![json!(["x"])])),
            ("skipped", 0.2, one(tc, vec![json!(["y"])])),
        ],
    )
}

#[hegel::composite]
fn server(tc: &TestCase) -> Value {
    let mut tree = Map::new();
    for _ in 0..tc.draw(gs::integers::<usize>().max_value(3)) {
        let path = one(tc, vec![json!("/"), json!("/a"), json!("/a/b")]);
        let node = match tc.draw(gs::weighted_booleans(0.2)) {
            true => {
                json!({ "error": one(tc, ["permission-denied", "not-found", "connection-lost"].map(Value::from).to_vec()) })
            }
            false => Value::Array(tc.draw(gs::vecs(entry()).max_size(4))),
        };
        tree.insert(path.as_str().map_or_else(|| path.to_string(), str::to_owned), node);
    }
    let auth = vec![
        json!([{ "password": { "accept": "pw" } }]),
        json!([{ "host-key": { "key_type": "k", "fingerprint": "f" } }]),
    ];
    let changes =
        vec![json!([{ "remove": "/a" }]), json!([[{ "add": "/a/n", "kind": "file" }, { "modify": "/a", "size": 3 }]])];
    object_of(
        tc,
        vec![
            ("home", 0.95, one(tc, vec![json!("/"), json!("/a"), json!("/a/../b/./")])),
            ("auth", 0.2, one(tc, auth)),
            ("tree", 0.8, Value::Object(tree)),
            ("fail", 0.1, json!("gone")),
            ("changes", 0.2, one(tc, changes)),
        ],
    )
}

/// A document shaped like a fixture, mostly as the format takes it, any field possibly anything.
#[hegel::composite]
fn fixture_doc(tc: &TestCase) -> Value {
    let aliases = tc.draw(gs::vecs(gs::from_regex("[a-z]{1,4}")).unique(true).max_size(3));
    let sources = ["saved", "recent", "ssh-config"].map(Value::from).to_vec();
    let mut hosts = Vec::new();
    let mut servers = Map::new();
    for alias in &aliases {
        let port = vec![json!(22), json!(65535)];
        hosts.push(object_of(
            tc,
            vec![
                ("alias", 0.95, one(tc, vec![Value::String(alias.clone())])),
                ("source", 0.95, one(tc, sources.clone())),
                ("user", 0.3, one(tc, vec![json!("u")])),
                ("port", 0.2, one(tc, port)),
            ],
        ));
        servers.insert(alias.clone(), tc.draw(server()));
    }
    let transfers = tc.draw(gs::vecs(job(aliases.clone())).max_size(3));
    object_of(
        tc,
        vec![
            ("version", 0.95, one(tc, vec![json!(VERSION)])),
            ("description", 0.3, json!("d")),
            ("now", 0.5, time(tc)),
            ("hosts", 0.95, Value::Array(hosts)),
            ("servers", 0.9, Value::Object(servers)),
            ("transfers", 0.9, Value::Array(transfers)),
        ],
    )
}

/// One step from a value into its parts.
#[derive(Clone, Debug, PartialEq)]
enum Seg {
    Key(String),
    At(usize),
}

/// The demo, as JSON.
fn demo_value() -> Value {
    serde_json::from_str(super::super::DEMO_JSON).expect("the demo is JSON")
}

/// The path to every value inside `v` (not `v` itself), parents first.
fn paths(v: &Value, here: &mut Vec<Seg>, out: &mut Vec<Vec<Seg>>) {
    let children: Vec<(Seg, &Value)> = match v {
        Value::Object(m) => m.iter().map(|(k, v)| (Seg::Key(k.clone()), v)).collect(),
        Value::Array(a) => a.iter().enumerate().map(|(i, v)| (Seg::At(i), v)).collect(),
        _ => Vec::new(),
    };
    for (seg, child) in children {
        here.push(seg);
        out.push(here.clone());
        paths(child, here, out);
        here.pop();
    }
}

fn all_paths(v: &Value) -> Vec<Vec<Seg>> {
    let mut out = Vec::new();
    paths(v, &mut Vec::new(), &mut out);
    out
}

fn get_mut<'a>(v: &'a mut Value, path: &[Seg]) -> Option<&'a mut Value> {
    path.iter().try_fold(v, |v, seg| match seg {
        Seg::Key(k) => v.get_mut(k.as_str()),
        Seg::At(i) => v.get_mut(*i),
    })
}

/// Takes the value at `path` out of its object or list.
fn remove(v: &mut Value, path: &[Seg]) {
    let Some((last, parent)) = path.split_last() else { return };
    match (get_mut(v, parent), last) {
        (Some(Value::Object(m)), Seg::Key(k)) => {
            m.remove(k);
        }
        (Some(Value::Array(a)), Seg::At(i)) if *i < a.len() => {
            a.remove(*i);
        }
        _ => {}
    }
}

fn key_at(path: &[Seg], i: usize) -> Option<&str> {
    match path.get(i) {
        Some(Seg::Key(k)) => Some(k),
        _ => None,
    }
}

/// Whether the demo's field at `path` must be there (`docs/fixtures.md`).
fn required(path: &[Seg]) -> bool {
    let n = path.len();
    let last = key_at(path, n - 1).unwrap_or_default();
    let parent = n.checked_sub(2).and_then(|i| key_at(path, i));
    let in_generate = path.iter().any(|s| *s == Seg::Key("generate".into()));
    match (key_at(path, 0), n) {
        (Some("version"), 1) => true,
        (Some("hosts"), 3) => ["alias", "source"].contains(&last),
        (Some("servers"), 3) => last == "home",
        (Some("servers"), 5) => last == "error",
        (Some("servers"), _) if parent == Some("generate") => ["pattern", "count"].contains(&last),
        (Some("servers"), _) if parent == Some("mtime") && in_generate => ["end", "every"].contains(&last),
        (Some("servers"), 6) => ["name", "kind"].contains(&last),
        _ => false,
    }
}

/// Values the field at `path` of the demo can't take.
fn wrong_values(path: &[Seg]) -> &'static [&'static str] {
    let n = path.len();
    match (key_at(path, 0), n, key_at(path, n - 1)) {
        (Some("servers"), 2, _) => &["[]", "\"x\"", "1", "null"],
        (Some("servers"), 4, _) => &["\"x\"", "1", "true", "{\"error\":\"nope\"}", "{}"],
        (Some("servers"), 5, None) => &["1", "\"x\"", "{}"],
        (Some("hosts"), 2, None) => &["1", "\"x\"", "[]"],
        (_, _, Some("version")) => &["\"1\"", "2", "true", "[]"],
        (_, _, Some("description")) => &["1", "true", "[]"],
        (_, _, Some("now")) => &["true", "[]", "\"yesterday\"", "1.5"],
        (_, _, Some("hosts")) => &["{}", "\"x\"", "1"],
        (_, _, Some("alias")) => &["1", "\"\"", "\"has space\"", "true"],
        (_, _, Some("source")) => &["\"elsewhere\"", "1"],
        (_, _, Some("user" | "hostname")) => &["1", "\"\"", "true"],
        (_, _, Some("port")) => &["0", "70000", "\"22\"", "-1"],
        (_, _, Some("servers")) => &["[]", "1"],
        (_, _, Some("home")) => &["\"relative\"", "1", "\"\""],
        (_, _, Some("tree")) => &["[]", "1"],
        (_, _, Some("name")) => &["\"\"", "\"a/b\"", "\".\"", "\"..\"", "1"],
        (_, _, Some("kind")) => &["\"folder\"", "1"],
        (_, _, Some("target")) => &["\"nowhere\"", "1"],
        (_, _, Some("size")) => &["-1", "\"1\"", "1.5"],
        (_, _, Some("mtime")) => &["true", "\"yesterday\"", "[]"],
        (_, _, Some("mode")) => &["true", "\"999\"", "\"x\"", "-1"],
        (_, _, Some("generate")) => &["1", "\"x\""],
        (_, _, Some("pattern")) => &["\"no braces\"", "1", "\"{}{}\"", "\"{021}\""],
        (_, _, Some("count")) => &["-1", "\"1\"", "1000001"],
        (_, _, Some("end")) => &["true"],
        (_, _, Some("every" | "jitter")) => &["-1", "\"1\""],
        (_, _, Some("error")) => &["\"nope\"", "1"],
        _ => &[],
    }
}

#[hegel::test]
fn any_text_parses_without_panicking_and_fails_unless_it_is_json(tc: TestCase) {
    let text = tc.draw(hegel::one_of!(gs::text(), gs::from_regex(r#"[{}\[\]":,0-9a-z ]{0,40}"#)));
    let parsed = parse(&text);
    if serde_json::from_str::<Value>(&text).is_err() {
        assert!(parsed.is_err(), "{text:?} isn't JSON");
    }
}

#[hegel::test]
fn any_json_parses_without_panicking_and_fails_without_version_one(tc: TestCase) {
    let doc = tc.draw(json());
    let parsed = parse(&doc.to_string());
    if doc.get("version").and_then(Value::as_u64) != Some(VERSION) {
        assert!(parsed.is_err(), "{doc} has no version 1");
    }
}

#[hegel::test]
#[hegel::explicit_test_case(doc = json!({
    "hosts": [{"alias": "a", "source": "saved"}],
    "transfers": [{"error": "e", "eta": 1e300, "host": "a", "kind": "download", "state": "failed", "title": "t"}],
    "version": 1
}))]
fn a_fixture_shaped_document_parses_without_panicking(tc: TestCase) {
    let doc = tc.draw(fixture_doc());
    let _ = parse(&doc.to_string());
}

/// A fixture with one generator in `/`, of `generate`'s fields.
fn one_generator(generate: Value) -> Value {
    json!({
        "version": 1,
        "hosts": [{"alias": "a", "source": "saved"}],
        "servers": {"a": {"home": "/", "tree": {"/": [{"generate": generate}]}}}
    })
}

/// Every folder of a fixture the parser takes lists without panicking: a generator's numbers,
/// sizes and dates at their limits included.
#[hegel::test]
#[hegel::explicit_test_case(doc = one_generator(json!({"pattern": "f-{}", "count": 1, "size": [0, u64::MAX]})))]
#[hegel::explicit_test_case(doc = one_generator(json!({"pattern": "f-{}", "count": 2, "start": u64::MAX})))]
#[hegel::explicit_test_case(doc = one_generator(json!({
    "pattern": "f-{}", "count": 2, "mtime": {"end": 0, "every": i64::MAX}
})))]
fn a_parsed_fixture_lists_every_folder_without_panicking(tc: TestCase) {
    let doc = tc.draw(fixture_doc());
    let Ok(fx) = parse(&doc.to_string()) else { return };
    for server in fx.servers.values() {
        for path in server.tree.keys() {
            let _ = server.list(path);
        }
    }
}

#[hegel::test]
fn a_changed_demo_parses_without_panicking(tc: TestCase) {
    let mut doc = demo_value();
    let changes = tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
    for _ in 0..changes {
        let paths = all_paths(&doc);
        tc.assume(!paths.is_empty());
        let path = &paths[tc.draw(gs::integers::<usize>().max_value(paths.len() - 1))];
        match tc.draw(gs::integers::<u8>().max_value(2)) {
            0 => remove(&mut doc, path),
            how => {
                let new = if how == 1 { tc.draw(json()) } else { tc.draw(number()) };
                if let Some(v) = get_mut(&mut doc, path) {
                    *v = new;
                }
            }
        }
    }
    let _ = parse(&doc.to_string());
}

#[hegel::test]
fn an_unknown_field_anywhere_in_the_demo_is_an_error(tc: TestCase) {
    let mut doc = demo_value();
    let mut objects: Vec<Vec<Seg>> = vec![Vec::new()];
    objects.extend(all_paths(&doc).into_iter().filter(|p| get_mut(&mut doc.clone(), p).is_some_and(|v| v.is_object())));
    let path = &objects[tc.draw(gs::integers::<usize>().max_value(objects.len() - 1))];
    let Some(Value::Object(m)) = get_mut(&mut doc, path) else { unreachable!("an object's path") };
    m.insert("zz-unknown-field".into(), Value::Bool(true));
    assert!(parse(&doc.to_string()).is_err(), "an unknown field at {path:?} was taken");
}

#[hegel::test]
fn a_required_field_left_out_of_the_demo_is_an_error(tc: TestCase) {
    let mut doc = demo_value();
    let required: Vec<Vec<Seg>> = all_paths(&doc).into_iter().filter(|p| required(p)).collect();
    let path = &required[tc.draw(gs::integers::<usize>().max_value(required.len() - 1))];
    remove(&mut doc, path);
    assert!(parse(&doc.to_string()).is_err(), "{path:?} left out was taken");
}

#[hegel::test]
fn a_value_its_field_cant_take_is_an_error(tc: TestCase) {
    let mut doc = demo_value();
    let fields: Vec<Vec<Seg>> = all_paths(&doc).into_iter().filter(|p| !wrong_values(p).is_empty()).collect();
    let path = &fields[tc.draw(gs::integers::<usize>().max_value(fields.len() - 1))];
    let wrong = tc.draw(gs::sampled_from(wrong_values(path)));
    *get_mut(&mut doc, path).expect("the field") = serde_json::from_str(wrong).expect("JSON");
    assert!(parse(&doc.to_string()).is_err(), "{path:?} took {wrong}");
}
