//! Saved and recent hosts on a fixture, in memory (`docs/fixtures.md`, "Saved and recent
//! hosts"): Save and Remove change the Saved list, "Add to SSH config" moves a saved host into
//! the SSH Config section (no file is read or written), and the recent hosts the UI hands over
//! can be connected.
//!
//! A host the fixture doesn't list (a quick connect, a saved host made in the session) signs in
//! and lists as the fixture's server for the same name, or else for the fixture host with the
//! same hostname: quick-connecting `deploy@prod.example.com` reaches `prod`'s server.

use std::collections::BTreeSet;
use std::sync::Arc;

use dromon_core::{HostId, HostSource, HostSpec};

use super::Fixture;
use crate::state::hostbook::{ConfigPreview, upsert};
use crate::state::{HostBookCommand, HostBookEvent, ServiceEvent};

/// Where the fixture's "ssh config" is, as the sheet shows it.
pub const TARGET: &str = "~/.ssh/config";

pub struct Book {
    fixture: Arc<Fixture>,
    /// The SSH Config section: the fixture's, then hosts added to it.
    config: Vec<HostSpec>,
    saved: Vec<HostSpec>,
    recent: Vec<HostSpec>,
    /// Backups "made" so far, so a second add in the same minute gets `-2`.
    backups: BTreeSet<String>,
}

impl Book {
    pub fn new(fixture: Arc<Fixture>) -> Book {
        let of = |s: HostSource| fixture.hosts.iter().filter(|h| h.source == s).cloned().collect::<Vec<_>>();
        let (config, saved, recent) = (of(HostSource::SshConfig), of(HostSource::Saved), of(HostSource::Recent));
        Book { fixture, config, saved, recent, backups: BTreeSet::new() }
    }

    /// The host `id` names now: SSH Config, then saved, then recent, as the sidebar ranks them.
    pub fn spec(&self, id: &HostId) -> Option<&HostSpec> {
        self.config.iter().chain(&self.saved).chain(&self.recent).find(|h| &h.id == id)
    }

    /// The fixture server `id` reaches: its own, one with its name, or the one of the fixture
    /// host with its hostname.
    pub fn server_alias(&self, id: &HostId) -> Option<String> {
        if self.fixture.server(id.as_str()).is_some() {
            return Some(id.as_str().to_owned());
        }
        let spec = self.spec(id)?;
        if self.fixture.server(&spec.alias).is_some() {
            return Some(spec.alias.clone());
        }
        let hostname = spec.hostname.as_deref()?;
        let host = self
            .fixture
            .hosts
            .iter()
            .find(|h| h.hostname.as_deref() == Some(hostname) && self.fixture.server(&h.alias).is_some())?;
        Some(host.alias.clone())
    }

    /// The backup's name stamp: the fixture's clock (UTC), or the system's.
    fn stamp(&self) -> String {
        let now = self.fixture.now.unwrap_or_else(crate::now_secs);
        let t = rune::time::LocalTime::at_offset(now * 1000, 0);
        dromon_ssh::backup_stamp(t.year, t.month, t.day, t.hour, t.minute)
    }

    /// The backup the next add would make.
    fn next_backup(&self) -> String {
        let stamp = self.stamp();
        (1..)
            .map(|n| {
                if n == 1 {
                    format!("{TARGET}.dromon-backup-{stamp}")
                } else {
                    format!("{TARGET}.dromon-backup-{stamp}-{n}")
                }
            })
            .find(|b| !self.backups.contains(b))
            .unwrap_or_default()
    }

    fn preview(&self, id: &HostId) -> Result<ConfigPreview, String> {
        let spec =
            self.saved.iter().find(|h| &h.id == id).ok_or_else(|| format!("“{id}” isn't a saved host any more."))?;
        let block = dromon_ssh::host_block(spec).map_err(|e| format!("Its settings can't go in an ssh config: {e}"))?;
        Ok(ConfigPreview { target: TARGET.into(), backup: Some(self.next_backup()), block })
    }

    fn saved_event(&self) -> ServiceEvent {
        ServiceEvent::HostBook(HostBookEvent::Saved(self.saved.clone()))
    }

    /// Answers `cmd` as the service would, at once.
    pub fn command(&mut self, cmd: HostBookCommand, out: &mut Vec<ServiceEvent>) {
        match cmd {
            HostBookCommand::Recent(list) => self.recent = list,
            HostBookCommand::Save { spec, replaces } => {
                upsert(&mut self.saved, spec, replaces.as_ref());
                out.push(self.saved_event());
            }
            HostBookCommand::Remove(id) => {
                self.saved.retain(|h| h.id != id);
                out.push(self.saved_event());
            }
            HostBookCommand::PreviewAddToConfig(host) => {
                let preview = self.preview(&host);
                out.push(ServiceEvent::HostBook(HostBookEvent::Preview { host, preview }));
            }
            HostBookCommand::AddToConfig(host) => {
                if let Err(error) = self.preview(&host) {
                    out.push(ServiceEvent::HostBook(HostBookEvent::AddFailed { host, error }));
                    return;
                }
                self.backups.insert(self.next_backup());
                if let Some(i) = self.saved.iter().position(|h| h.id == host) {
                    let spec = self.saved.remove(i);
                    self.config.push(HostSpec { source: HostSource::SshConfig, ..spec });
                }
                out.push(ServiceEvent::Hosts(self.config.clone()));
                out.push(self.saved_event());
                out.push(ServiceEvent::HostBook(HostBookEvent::Added { host }));
            }
        }
    }
}
