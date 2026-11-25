//! Deterministic fake hosts and file systems, so the whole UI runs (and renders goldens)
//! without a network. Every listing is a pure function of the host, the path and `now`.

use dromon_core::{ConnStatus, Entry, EntryKind, Error, HostId, HostSource, HostSpec, LinkTarget, RemotePath};

use crate::state::{Backend, Command, HostDirectory, ServiceEvent};

/// 2026-11-25 14:00:00 UTC: the clock every scene and test reads.
pub const NOW: i64 = 1_795_615_200;

/// The big folder: 10,000 photos.
pub const BIG_FOLDER: &str = "/srv/media/photos";
pub const BIG_COUNT: usize = 10_000;

const MIN: i64 = 60;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

fn host(alias: &str, source: HostSource, user: &str, hostname: &str, port: Option<u16>) -> HostSpec {
    HostSpec {
        id: HostId(alias.to_owned()),
        source,
        alias: alias.to_owned(),
        hostname: Some(hostname.to_owned()),
        user: Some(user.to_owned()),
        port,
        identity_file: None,
        proxy_jump: None,
    }
}

/// The sidebar's hosts: four from the SSH config, two saved, two recent quick connects.
pub fn hosts() -> HostDirectory {
    use HostSource::*;
    HostDirectory {
        hosts: vec![
            host("prod", SshConfig, "deploy", "prod.example.com", None),
            host("staging", SshConfig, "deploy", "staging.example.com", None),
            host("nas", SshConfig, "admin", "nas.local", Some(2222)),
            host("pi", SshConfig, "pi", "raspberrypi.local", None),
            host("build-box", SshConfig, "ci", "10.0.0.20", None),
            host("box", Saved, "fatih", "10.0.0.4", None),
            host("media", Saved, "fatih", "media.home.arpa", None),
            host("203.0.113.7", Recent, "ubuntu", "203.0.113.7", None),
            host("10.0.0.12", Recent, "root", "10.0.0.12", Some(2222)),
        ],
    }
}

/// Where a connection opens: prod at its site (the "last folder used"), the rest at home.
pub fn start(host: &HostId) -> RemotePath {
    match host.as_str() {
        "prod" => RemotePath::new("/var/www/site"),
        "nas" => RemotePath::new("/volume1"),
        "pi" => RemotePath::new("/home/pi"),
        "203.0.113.7" => RemotePath::new("/home/ubuntu"),
        "10.0.0.12" => RemotePath::root(),
        _ => RemotePath::new("/home/deploy"),
    }
}

/// The mock backend: connects at once and lists from [`list`].
pub struct MockService {
    pub now: i64,
}

impl Backend for MockService {
    fn send(&mut self, cmd: Command) -> Vec<ServiceEvent> {
        match cmd {
            Command::Connect { host } => vec![
                ServiceEvent::Status { host: host.clone(), status: ConnStatus::Ready },
                ServiceEvent::Opened { start: start(&host), host },
            ],
            Command::List { host, path } => match list(&host, &path, self.now) {
                Ok(entries) => vec![ServiceEvent::Listing { host, path, entries }],
                Err(error) => vec![ServiceEvent::ListFailed { host, path, error }],
            },
            Command::Disconnect { host } => vec![ServiceEvent::Status { host, status: ConnStatus::Disconnected }],
        }
    }
}

fn dir(name: &str, mtime: i64) -> Entry {
    Entry { name: name.into(), kind: EntryKind::Dir, size: 4096, mtime: Some(mtime), mode: Some(0o040755) }
}

fn file(name: &str, size: u64, mtime: i64) -> Entry {
    Entry { name: name.into(), kind: EntryKind::File, size, mtime: Some(mtime), mode: Some(0o100644) }
}

fn link_dir(name: &str, mtime: i64) -> Entry {
    Entry { name: name.into(), kind: EntryKind::Symlink { target: Some(LinkTarget::Dir) }, size: 30, mtime: Some(mtime), mode: Some(0o120777) }
}

/// Symlinks the mock resolves when listing through them.
fn resolve(path: &RemotePath) -> RemotePath {
    const LINKS: [(&str, &str); 2] = [("/var/www/site/current", "/var/www/site/releases/20261125-1348"), ("/var/www/html", "/var/www/site")];
    for (link, target) in LINKS {
        let link = RemotePath::new(link);
        if path.starts_with(&link) {
            let rest = &path.as_str()[link.as_str().len()..];
            return RemotePath::new(&format!("{target}{rest}"));
        }
    }
    path.clone()
}

/// FNV-1a, for stable pseudo-random contents.
fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// A small xorshift stream seeded from a hash.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0.max(1);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Lists `path` on `host` as of `now`.
pub fn list(host: &HostId, path: &RemotePath, now: i64) -> Result<Vec<Entry>, Error> {
    let path = resolve(path);
    let p = path.as_str();
    if matches!(p, "/root" | "/etc/ssl/private" | "/var/lib/mysql") {
        return Err(Error::PermissionDenied(path.clone()));
    }
    if let Some(entries) = fixed(host.as_str(), p, now) {
        return Ok(entries);
    }
    // Anything else must be a folder its parent lists.
    let parent = path.parent().ok_or_else(|| Error::NotFound(path.clone()))?;
    let name = path.file_name().unwrap_or_default();
    let siblings = list(host, &parent, now)?;
    if !siblings.iter().any(|e| e.name == name && e.kind.is_dir_like()) {
        return Err(Error::NotFound(path.clone()));
    }
    Ok(generated(host.as_str(), &path, now))
}

fn fixed(host: &str, p: &str, now: i64) -> Option<Vec<Entry>> {
    let ago = |s: i64| now - s;
    Some(match (host, p) {
        (_, "/") => vec![
            dir("bin", ago(40 * DAY)),
            dir("boot", ago(40 * DAY)),
            dir("etc", ago(2 * DAY)),
            dir("home", ago(90 * DAY)),
            dir("opt", ago(120 * DAY)),
            dir("root", ago(30 * DAY)),
            dir("srv", ago(9 * DAY)),
            dir("tmp", ago(4 * MIN)),
            dir("usr", ago(40 * DAY)),
            dir("var", ago(DAY + 3 * HOUR)),
        ],
        (_, "/var") => vec![
            dir("backups", ago(DAY)),
            dir("cache", ago(5 * HOUR)),
            dir("lib", ago(3 * DAY)),
            dir("log", ago(MIN)),
            dir("www", ago(DAY)),
        ],
        (_, "/var/www") => vec![dir("site", ago(2 * MIN)), dir("staging-site", ago(6 * DAY)), link_dir("html", ago(200 * DAY))],
        ("prod", "/var/www/site") => vec![
            dir("assets", ago(2 * MIN)),
            dir("node_modules", ago(22 * DAY)),
            dir("releases", ago(20 * HOUR)),
            dir("uploads", ago(26 * DAY)),
            dir(".well-known", ago(42 * DAY)),
            link_dir("current", ago(12 * MIN)),
            file(".env", 1_204, ago(12 * MIN)),
            file(".gitignore", 312, ago(64 * DAY)),
            file("index.html", 14_812, ago(20 * HOUR)),
            file("favicon.ico", 4_286, ago(42 * DAY)),
            file("robots.txt", 68, ago(120 * DAY)),
            file("package.json", 2_143, ago(3 * DAY)),
            file("package-lock.json", 412_388, ago(3 * DAY)),
            file("README.md", 3_391, ago(9 * DAY)),
            file("server.log", 4_812_331, ago(2 * MIN)),
            file("backup-2026-11-01.tar.gz", 18_204_116, ago(24 * DAY)),
            file("app.min.js", 286_410, ago(20 * HOUR)),
        ],
        ("prod", "/var/www/site/uploads") => vec![],
        ("prod", "/var/www/site/releases") => vec![
            dir("20261125-1348", ago(12 * MIN)),
            dir("20261124-0912", ago(DAY + 5 * HOUR)),
            dir("20261120-1730", ago(5 * DAY)),
        ],
        (_, "/home") => vec![dir("deploy", ago(DAY)), dir("pi", ago(8 * DAY)), dir("ubuntu", ago(30 * DAY)), dir("admin", ago(60 * DAY))],
        ("nas", "/volume1") => vec![
            dir("backups", ago(3 * HOUR)),
            dir("docker", ago(2 * DAY)),
            dir("homes", ago(40 * DAY)),
            dir("media", ago(5 * DAY)),
            dir("photos", ago(DAY)),
            file(".DS_Store", 6_148, ago(70 * DAY)),
            file("README.txt", 1_024, ago(300 * DAY)),
        ],
        (_, "/srv") => vec![dir("media", ago(9 * DAY)), dir("git", ago(30 * DAY))],
        (_, "/srv/media") => vec![dir("photos", ago(9 * DAY)), dir("video", ago(60 * DAY))],
        (_, BIG_FOLDER) => big_folder(now),
        _ => return None,
    })
}

/// 10,000 photos, a few megabytes each, taken over the past year.
fn big_folder(now: i64) -> Vec<Entry> {
    let mut rng = Rng(hash(BIG_FOLDER));
    (0..BIG_COUNT)
        .map(|i| {
            let size = 1_500_000 + rng.below(4_800_000);
            // Newer photos have higher numbers.
            let mtime = now - ((BIG_COUNT - i) as i64) * 52 * MIN - rng.below(40) as i64 * MIN;
            file(&format!("IMG_{:05}.jpg", i + 1), size, mtime)
        })
        .collect()
}

const DIR_NAMES: [&str; 14] =
    ["src", "lib", "config", "scripts", "data", "cache", "static", "build", "docs", "tests", "vendor", "logs", "tmp", "public"];
const FILE_NAMES: [&str; 18] = [
    "README.md", "main.rs", "index.js", "style.css", "notes.txt", "config.toml", "data.csv", "app.log", "photo.jpg",
    "archive.tar.gz", "Makefile", "run.sh", "settings.json", ".bashrc", ".profile", "schema.sql", "logo.svg", "LICENSE",
];

/// Plausible contents for any folder without a fixed listing; deeper folders hold fewer
/// subfolders, so browsing always ends.
fn generated(host: &str, path: &RemotePath, now: i64) -> Vec<Entry> {
    let mut rng = Rng(hash(host) ^ hash(path.as_str()));
    let depth = path.components().count() as u64;
    let dirs = if depth >= 6 { 0 } else { rng.below(5) };
    let files = 1 + rng.below(10);
    let mut out: Vec<Entry> = Vec::new();
    for _ in 0..dirs {
        let name = DIR_NAMES[rng.below(DIR_NAMES.len() as u64) as usize];
        if !out.iter().any(|e| e.name == name) {
            out.push(dir(name, now - rng.below(200 * DAY as u64) as i64));
        }
    }
    for _ in 0..files {
        let name = FILE_NAMES[rng.below(FILE_NAMES.len() as u64) as usize];
        if !out.iter().any(|e| e.name == name) {
            let size = match rng.below(4) {
                0 => rng.below(1_000),
                1 => rng.below(100_000),
                2 => rng.below(5_000_000),
                _ => rng.below(400_000_000),
            };
            out.push(file(name, size, now - rng.below(400 * DAY as u64) as i64));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> HostId {
        HostId(s.into())
    }

    #[test]
    fn listings_are_deterministic() {
        let a = list(&h("prod"), &RemotePath::new("/var/www/site/assets"), NOW).unwrap();
        let b = list(&h("prod"), &RemotePath::new("/var/www/site/assets"), NOW).unwrap();
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn the_site_has_dotfiles_and_a_symlinked_folder() {
        let site = list(&h("prod"), &start(&h("prod")), NOW).unwrap();
        assert!(site.iter().filter(|e| e.hidden()).count() >= 2);
        assert!(site.iter().any(|e| e.kind.is_symlink() && e.kind.is_dir_like()));
        assert!((14..=20).contains(&site.len()));
    }

    #[test]
    fn symlinks_list_their_target() {
        let through = list(&h("prod"), &RemotePath::new("/var/www/site/current"), NOW).unwrap();
        let direct = list(&h("prod"), &RemotePath::new("/var/www/site/releases/20261125-1348"), NOW).unwrap();
        assert!(!through.is_empty());
        assert_eq!(through, direct);
    }

    #[test]
    fn errors_for_denied_and_missing_folders() {
        assert!(matches!(list(&h("prod"), &RemotePath::new("/root"), NOW), Err(Error::PermissionDenied(_))));
        assert!(matches!(list(&h("prod"), &RemotePath::new("/nope"), NOW), Err(Error::NotFound(_))));
        assert_eq!(list(&h("prod"), &RemotePath::new("/var/www/site/uploads"), NOW).unwrap(), vec![]);
    }

    #[test]
    fn the_big_folder_has_ten_thousand_entries() {
        let big = list(&h("prod"), &RemotePath::new(BIG_FOLDER), NOW).unwrap();
        assert_eq!(big.len(), BIG_COUNT);
        assert_eq!(big[0].name, "IMG_00001.jpg");
    }
}
