//! The files task on temp folders: `hosts.conf` read and written, Add to SSH config with its
//! backup, and the watcher seeing config edits. `ssh -G` points at a program that isn't there,
//! so discovery keeps the bare aliases.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dromon_core::{HostId, HostSource, HostSpec};
use dromon_ssh::ConfigSources;
use tokio::sync::mpsc;

use super::*;
use crate::service::{Config, Sink};
use crate::state::{HostBookCommand, HostBookEvent, ServiceEvent};

/// Longer than two watch periods.
const WAIT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct TestSink(mpsc::UnboundedSender<ServiceEvent>);

impl Sink for TestSink {
    fn emit(&self, ev: ServiceEvent) {
        let _ = self.0.send(ev);
    }
}

struct Files {
    _dir: tempfile::TempDir,
    ssh_dir: PathBuf,
    config: PathBuf,
    hosts_conf: PathBuf,
    book: Book,
    updates: mpsc::UnboundedReceiver<Update>,
    known: HashMap<HostId, HostSpec>,
    sink: TestSink,
    events: mpsc::UnboundedReceiver<ServiceEvent>,
}

fn id(s: &str) -> HostId {
    HostId(s.into())
}

fn saved(alias: &str, hostname: &str) -> HostSpec {
    HostSpec { source: HostSource::Saved, hostname: Some(hostname.into()), user: Some("deploy".into()), ..HostSpec::from_config_alias(alias) }
}

impl Files {
    /// A home with `.ssh/config` holding `config` and the app's folder holding `hosts`.
    fn start(config: &str, hosts: Option<&str>) -> Files {
        let dir = tempfile::tempdir().unwrap();
        let ssh_dir = dir.path().join(".ssh");
        std::fs::create_dir(&ssh_dir).unwrap();
        let config_path = ssh_dir.join("config");
        std::fs::write(&config_path, config).unwrap();
        let hosts_conf = dir.path().join("app").join("hosts.conf");
        if let Some(text) = hosts {
            std::fs::create_dir(dir.path().join("app")).unwrap();
            std::fs::write(&hosts_conf, text).unwrap();
        }
        let cfg = Config {
            ssh: dir.path().join("no-ssh-here"),
            helper_exe: PathBuf::from("dromon"),
            verbose: false,
            config_file: None,
            discover: Some(ConfigSources { user: Some(config_path.clone()), system: None, user_ssh_dir: ssh_dir.clone() }),
            hosts_file: Some(hosts_conf.clone()),
            hosts: Vec::new(),
            without_env: Vec::new(),
        };
        let (book, updates) = Book::start(&cfg);
        let (tx, events) = mpsc::unbounded_channel();
        Files { _dir: dir, ssh_dir, config: config_path, hosts_conf, book, updates, known: HashMap::new(), sink: TestSink(tx), events }
    }

    fn send(&mut self, cmd: HostBookCommand) {
        self.book.command(cmd, &mut self.known);
    }

    /// Passes updates through the manager's side until an event `pick` takes.
    async fn until<T>(&mut self, what: &str, mut pick: impl FnMut(&ServiceEvent) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            while let Ok(ev) = self.events.try_recv() {
                if let Some(found) = pick(&ev) {
                    return found;
                }
            }
            match tokio::time::timeout_at(deadline, self.updates.recv()).await {
                Ok(Some(u)) => self.book.update(u, &mut self.known, &self.sink),
                Ok(None) => panic!("the files task ended"),
                Err(_) => panic!("timed out waiting for {what}"),
            }
        }
    }

    async fn config_hosts(&mut self, what: &str, want: &[&str]) {
        let want: Vec<String> = want.iter().map(|s| (*s).to_owned()).collect();
        self.until(what, |ev| match ev {
            ServiceEvent::Hosts(h) if h.iter().map(|h| h.alias.clone()).collect::<Vec<_>>() == want => Some(()),
            _ => None,
        })
        .await;
    }

    async fn saved_hosts(&mut self, what: &str) -> Vec<String> {
        self.until(what, |ev| match ev {
            ServiceEvent::HostBook(HostBookEvent::Saved(list)) => Some(list.iter().map(|h| h.alias.clone()).collect()),
            _ => None,
        })
        .await
    }
}

#[tokio::test]
async fn startup_reads_both_files_and_the_manager_knows_the_hosts() {
    let hosts = dromon_ssh::render_hosts_conf(&[saved("box", "10.0.0.4")]).unwrap();
    let mut f = Files::start("Host prod staging\n  User deploy\nHost *\n  ServerAliveInterval 5\n", Some(&hosts));
    assert_eq!(f.saved_hosts("the saved hosts").await, ["box"]);
    f.config_hosts("the config hosts", &["prod", "staging"]).await;
    assert_eq!(f.known.get(&id("box")).map(|h| h.source), Some(HostSource::Saved));
    assert_eq!(f.known.get(&id("prod")).map(|h| h.source), Some(HostSource::SshConfig));
    // The recent hosts are the manager's to know, straight from the command.
    let recent = HostSpec { source: HostSource::Recent, id: id("u@h:22"), ..HostSpec::from_config_alias("h") };
    f.send(HostBookCommand::Recent(vec![recent]));
    assert_eq!(f.known.get(&id("u@h:22")).map(|h| h.source), Some(HostSource::Recent));
    f.send(HostBookCommand::Recent(Vec::new()));
    assert!(!f.known.contains_key(&id("u@h:22")));
}

#[tokio::test]
async fn save_and_remove_write_hosts_conf() {
    let mut f = Files::start("", None);
    assert_eq!(f.saved_hosts("nothing saved yet").await, Vec::<String>::new());
    f.send(HostBookCommand::Save { spec: saved("lab", "10.0.0.7"), replaces: None });
    assert_eq!(f.saved_hosts("lab saved").await, ["lab"]);
    let text = std::fs::read_to_string(&f.hosts_conf).unwrap();
    assert!(text.contains("Host lab\n    HostName 10.0.0.7\n    User deploy\n"), "{text}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&f.hosts_conf).unwrap().permissions().mode() & 0o777, 0o600);
    }
    f.send(HostBookCommand::Save { spec: saved("lab2", "10.0.0.8"), replaces: Some(id("lab")) });
    assert_eq!(f.saved_hosts("lab renamed").await, ["lab2"]);
    f.send(HostBookCommand::Remove(id("lab2")));
    assert_eq!(f.saved_hosts("lab2 removed").await, Vec::<String>::new());
    assert_eq!(dromon_ssh::read_hosts_conf(&f.hosts_conf).unwrap(), []);
    // A value ssh can't read back isn't written: the error, and the list as it was.
    let bad = HostSpec { user: Some("a%b".into()), ..saved("bad", "h") };
    f.send(HostBookCommand::Save { spec: bad, replaces: None });
    let why = f
        .until("the save failing", |ev| match ev {
            ServiceEvent::HostBook(HostBookEvent::SaveFailed(why)) => Some(why.clone()),
            _ => None,
        })
        .await;
    assert!(why.contains("User"), "{why}");
    assert_eq!(f.saved_hosts("the list as it was").await, Vec::<String>::new());
}

#[tokio::test]
async fn add_to_config_backs_up_appends_and_moves_the_host() {
    let hosts = dromon_ssh::render_hosts_conf(&[saved("box", "10.0.0.4"), saved("pi", "10.0.0.9")]).unwrap();
    let mut f = Files::start("Host prod\n  User deploy\n", Some(&hosts));
    f.config_hosts("prod", &["prod"]).await;
    f.send(HostBookCommand::PreviewAddToConfig(id("box")));
    let preview = f
        .until("the preview", |ev| match ev {
            ServiceEvent::HostBook(HostBookEvent::Preview { host, preview }) if *host == id("box") => Some(preview.clone()),
            _ => None,
        })
        .await
        .unwrap();
    assert_eq!(preview.target, display(&f.config, &f.ssh_dir));
    assert_eq!(preview.block, "Host box\n    HostName 10.0.0.4\n    User deploy\n");
    let backup = preview.backup.clone().unwrap();
    assert!(backup.contains("config.dromon-backup-"), "{backup}");

    f.send(HostBookCommand::AddToConfig(id("box")));
    f.until("added", |ev| matches!(ev, ServiceEvent::HostBook(HostBookEvent::Added { host }) if *host == id("box")).then_some(())).await;
    let config = std::fs::read_to_string(&f.config).unwrap();
    assert_eq!(config, "Host prod\n  User deploy\n\nHost box\n    HostName 10.0.0.4\n    User deploy\n");
    let backups: Vec<PathBuf> =
        std::fs::read_dir(&f.ssh_dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.to_string_lossy().contains("dromon-backup")).collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), "Host prod\n  User deploy\n");
    let names: Vec<String> = dromon_ssh::read_hosts_conf(&f.hosts_conf).unwrap().into_iter().map(|h| h.alias).collect();
    assert_eq!(names, ["pi"], "box left hosts.conf");
    assert_eq!(f.known.get(&id("box")).map(|h| h.source), Some(HostSource::SshConfig));
    assert_eq!(f.known.get(&id("pi")).map(|h| h.source), Some(HostSource::Saved));

    // A host that isn't saved any more can't be added.
    f.send(HostBookCommand::AddToConfig(id("box")));
    let error = f
        .until("the refusal", |ev| match ev {
            ServiceEvent::HostBook(HostBookEvent::AddFailed { error, .. }) => Some(error.clone()),
            _ => None,
        })
        .await;
    assert!(error.contains("isn't a saved host"), "{error}");
    assert_eq!(std::fs::read_to_string(&f.config).unwrap(), config, "unchanged");
}

/// `path` as the sheet shows it, with the test's home.
fn display(path: &Path, ssh_dir: &Path) -> String {
    dromon_ssh::display_path(path, ssh_dir.parent())
}

#[tokio::test]
async fn config_edits_are_seen_includes_too() {
    let mut f = Files::start("Include conf.d/*\nHost prod\n", None);
    f.config_hosts("prod", &["prod"]).await;
    // A new file where the Include looks: the folder changed.
    std::fs::create_dir(f.ssh_dir.join("conf.d")).unwrap();
    std::fs::write(f.ssh_dir.join("conf.d").join("lab"), "Host lab\n").unwrap();
    f.config_hosts("lab, from a new included file", &["lab", "prod"]).await;
    // An edit of the main file.
    std::fs::write(&f.config, "Include conf.d/*\nHost prod pi\n").unwrap();
    f.config_hosts("pi, added to the config", &["lab", "prod", "pi"]).await;
    // An edit of the included file.
    std::fs::write(f.ssh_dir.join("conf.d").join("lab"), "Host lab2\n").unwrap();
    f.config_hosts("lab renamed in its file", &["lab2", "prod", "pi"]).await;
}

#[tokio::test]
async fn hosts_conf_edits_are_seen() {
    let mut f = Files::start("", Some(""));
    assert_eq!(f.saved_hosts("empty").await, Vec::<String>::new());
    std::fs::write(&f.hosts_conf, dromon_ssh::render_hosts_conf(&[saved("ext", "1.2.3.4")]).unwrap()).unwrap();
    assert_eq!(f.saved_hosts("a host added by another hand").await, ["ext"]);
}

#[test]
fn sections_rank_ssh_config_over_saved_over_recent() {
    let of = |a: &str, source| HostSpec { source, ..HostSpec::from_config_alias(a) };
    let mut known = HashMap::new();
    set_section(&mut known, HostSource::Saved, &[of("box", HostSource::Saved), of("pi", HostSource::Saved)]);
    set_section(&mut known, HostSource::SshConfig, &[of("box", HostSource::SshConfig)]);
    assert_eq!(known[&id("box")].source, HostSource::SshConfig);
    set_section(&mut known, HostSource::Saved, &[of("box", HostSource::Saved)]);
    assert_eq!(known[&id("box")].source, HostSource::SshConfig, "the config's name stays the config's");
    assert!(!known.contains_key(&id("pi")));
    set_section(&mut known, HostSource::Recent, &[of("box", HostSource::Recent), of("r", HostSource::Recent)]);
    assert_eq!(known[&id("box")].source, HostSource::SshConfig);
    assert_eq!(known[&id("r")].source, HostSource::Recent);
}

#[test]
fn bare_aliases_keep_what_was_resolved() {
    let resolved = HostSpec { hostname: Some("10.0.0.4".into()), port: Some(2222), ..HostSpec::from_config_alias("prod") };
    let kept = keep_resolved(vec![HostSpec::from_config_alias("prod"), HostSpec::from_config_alias("new")], std::slice::from_ref(&resolved));
    assert_eq!(kept, [resolved, HostSpec::from_config_alias("new")]);
}

#[test]
fn the_watch_list_has_the_files_their_folders_and_the_top_files() {
    let src = ConfigSources { user: Some("/h/.ssh/config".into()), system: Some("/etc/ssh/ssh_config".into()), user_ssh_dir: "/h/.ssh".into() };
    let files = [PathBuf::from("/h/.ssh/config"), PathBuf::from("/h/.ssh/conf.d/a")];
    let list: Vec<String> = watch_list(&src, &files).into_iter().map(|p| p.to_string_lossy().into_owned()).collect();
    assert_eq!(list, ["/h/.ssh/config", "/h/.ssh", "/h/.ssh/conf.d/a", "/h/.ssh/conf.d", "/etc/ssh/ssh_config", "/etc/ssh"]);
}
