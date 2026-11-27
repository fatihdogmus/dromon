//! The stand-ins: numbering, extensions, errors from their variants, trace lines.

use super::*;

fn host(s: &str) -> HostId {
    HostId(s.into())
}

#[test]
fn hosts_are_numbered_in_the_order_first_named_and_keep_their_number() {
    let mut r = Redactor::default();
    assert_eq!(r.host(&host("prod")), "host-1");
    assert_eq!(r.host(&host("deploy@203.0.113.7:2222")), "host-2");
    assert_eq!(r.host(&host("prod")), "host-1", "the same host, the same stand-in");
    assert_eq!(r.host(&host("nas")), "host-3");
    let mut other = Redactor::default();
    assert_eq!(other.host(&host("nas")), "host-1", "numbering is one report's");
}

#[test]
fn a_file_keeps_only_a_short_plain_extension() {
    let mut r = Redactor::default();
    let p = |r: &mut Redactor, s: &str| r.path(&RemotePath::new(s));
    assert_eq!(p(&mut r, "/var/www/site/index.html"), "<dir>/file-1.html");
    assert_eq!(p(&mut r, "/home/alice/backup-2026.tar.gz"), "<dir>/file-2.gz", "the last extension only");
    assert_eq!(p(&mut r, "/var/www/site/index.html"), "<dir>/file-1.html", "the same file, the same number");
    assert_eq!(p(&mut r, "/srv/other/index.html"), "<dir>/file-3.html", "another folder's is another file");
    assert_eq!(p(&mut r, "/home/alice/.env"), "<path>", "a dot file has no extension");
    assert_eq!(p(&mut r, "/home/alice/Makefile"), "<path>");
    assert_eq!(p(&mut r, "/home/alice/notes.acme-secret"), "<path>", "not a plain extension");
    assert_eq!(p(&mut r, "/home/alice/notes.verylongext"), "<path>", "longer than an extension");
    assert_eq!(p(&mut r, "/home/alice/projects"), "<path>", "a folder");
    assert_eq!(p(&mut r, "/"), "<path>");
    assert_eq!(p(&mut r, "/srv/photos/IMG_0001.JPG"), "<dir>/file-4.JPG");
}

#[test]
fn errors_are_rendered_from_their_variant_with_the_path_replaced() {
    let mut r = Redactor::default();
    let secret = RemotePath::new("/home/alice/payroll/2026.xlsx");
    assert_eq!(r.error(&Error::PermissionDenied(secret.clone())), "You don't have access to <dir>/file-1.xlsx");
    assert_eq!(r.error(&Error::NotFound(RemotePath::new("/home/alice/payroll"))), "<path> doesn't exist");
    assert_eq!(r.error(&Error::AlreadyExists(secret)), "<dir>/file-1.xlsx already exists");
    assert_eq!(r.error(&Error::ConnectionLost), "The connection was lost");
    assert_eq!(r.error(&Error::Unsupported("copy-data")), "The server doesn't support copy-data");
    for free in [
        Error::Connect("ssh: Could not resolve hostname alice.example.com".into()),
        Error::Io("/Users/alice/Downloads/x.txt: No space left on device".into()),
        Error::Other("alice".into()),
    ] {
        assert_eq!(r.error(&free), WITHHELD, "{free:?}");
    }
    assert_eq!(error_kind(&Error::PermissionDenied(RemotePath::root())), "PermissionDenied");
    assert_eq!(error_kind(&Error::Io(String::new())), "Io");
}

#[test]
fn trace_lines_name_stand_ins_for_their_hosts() {
    let mut r = Redactor::default();
    let known = ["prod", "my box", "alice@203.0.113.7:2222"];
    assert_eq!(r.trace("Status host=prod status=Ready", &known), "Status host=host-1 status=Ready");
    assert_eq!(
        r.trace("Prompt host=alice@203.0.113.7:2222 id=7", &known),
        "Prompt host=host-2 id=7",
        "a quick connect's id goes whole: no user, address or port"
    );
    assert_eq!(r.trace("Opened host=my box", &known), "Opened host=host-3", "a known id with a space");
    assert_eq!(r.trace("Opened host=gone.example.com", &known), "Opened host=host-4", "an id the app no longer has");
    assert_eq!(r.trace("FileOpDone host=prod id=3", &known), "FileOpDone host=host-1 id=3");
    assert_eq!(r.trace("Transfers jobs=2 active=1 done=1", &known), "Transfers jobs=2 active=1 done=1");
    assert_eq!(r.trace("Weird xhost=prod", &known), "Weird xhost=prod", "only the key itself");
}
