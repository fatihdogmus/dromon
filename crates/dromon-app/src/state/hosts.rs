//! The sidebar's host list: every known host, grouped into sections. A host with a live
//! connection moves up into Connected and leaves its own section.

use std::collections::BTreeMap;

use dromon_core::{ConnStatus, HostId, HostSource, HostSpec};

use super::conn::ConnView;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Connected,
    SshConfig,
    Saved,
    Recent,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Connected => "Connected",
            Section::SshConfig => "SSH Config",
            Section::Saved => "Saved",
            Section::Recent => "Recent",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Section::Connected => "sec-connected",
            Section::SshConfig => "sec-ssh-config",
            Section::Saved => "sec-saved",
            Section::Recent => "sec-recent",
        }
    }
}

/// Whether a connection belongs in the Connected section.
pub fn is_live(status: &ConnStatus) -> bool {
    matches!(status, ConnStatus::Connecting | ConnStatus::Ready | ConnStatus::Reconnecting { .. } | ConnStatus::NeedsInput)
}

/// Every host the app knows, in the order discovery gave them.
#[derive(Clone, Debug, Default)]
pub struct HostDirectory {
    pub hosts: Vec<HostSpec>,
}

impl HostDirectory {
    pub fn get(&self, id: &HostId) -> Option<&HostSpec> {
        self.hosts.iter().find(|h| &h.id == id)
    }

    /// The non-empty sections in sidebar order, with their hosts.
    pub fn sections<'a>(&'a self, conns: &BTreeMap<HostId, ConnView>) -> Vec<(Section, Vec<&'a HostSpec>)> {
        let live = |h: &HostSpec| conns.get(&h.id).is_some_and(|c| is_live(&c.status));
        let of = |source: HostSource| -> Vec<&HostSpec> {
            self.hosts.iter().filter(|h| h.source == source && !live(h)).collect()
        };
        let connected: Vec<&HostSpec> = self.hosts.iter().filter(|h| live(h)).collect();
        [
            (Section::Connected, connected),
            (Section::SshConfig, of(HostSource::SshConfig)),
            (Section::Saved, of(HostSource::Saved)),
            (Section::Recent, of(HostSource::Recent)),
        ]
        .into_iter()
        .filter(|(_, hosts)| !hosts.is_empty())
        .collect()
    }
}

/// The secondary line a host row can show: `user@hostname:port`, the parts it knows.
pub fn address(h: &HostSpec) -> Option<String> {
    let host = h.hostname.as_deref()?;
    let mut s = String::new();
    if let Some(u) = &h.user {
        s.push_str(u);
        s.push('@');
    }
    s.push_str(host);
    if let Some(p) = h.port.filter(|&p| p != 22) {
        s.push_str(&format!(":{p}"));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::conn::Sort;

    fn host(alias: &str, source: HostSource) -> HostSpec {
        HostSpec { source, ..HostSpec::from_config_alias(alias) }
    }

    #[test]
    fn live_hosts_move_to_connected() {
        let dir = HostDirectory {
            hosts: vec![host("prod", HostSource::SshConfig), host("pi", HostSource::SshConfig), host("box", HostSource::Saved)],
        };
        let mut conns = BTreeMap::new();
        let titles = |conns: &BTreeMap<HostId, ConnView>| -> Vec<(Section, Vec<String>)> {
            dir.sections(conns).into_iter().map(|(s, h)| (s, h.iter().map(|h| h.alias.clone()).collect())).collect()
        };
        assert_eq!(
            titles(&conns),
            [(Section::SshConfig, vec!["prod".to_owned(), "pi".to_owned()]), (Section::Saved, vec!["box".to_owned()])]
        );
        conns.insert(HostId("prod".into()), ConnView::connecting(HostId("prod".into()), true, Sort::default()));
        assert_eq!(titles(&conns)[0], (Section::Connected, vec!["prod".to_owned()]));
        assert_eq!(titles(&conns)[1], (Section::SshConfig, vec!["pi".to_owned()]));
        conns.get_mut(&HostId("prod".into())).unwrap().status = ConnStatus::Disconnected;
        assert_eq!(titles(&conns)[0].0, Section::SshConfig);
    }

    #[test]
    fn addresses_skip_the_default_port() {
        let mut h = host("box", HostSource::Saved);
        assert_eq!(address(&h), None);
        h.hostname = Some("10.0.0.4".into());
        h.user = Some("fatih".into());
        h.port = Some(22);
        assert_eq!(address(&h).as_deref(), Some("fatih@10.0.0.4"));
        h.port = Some(2222);
        assert_eq!(address(&h).as_deref(), Some("fatih@10.0.0.4:2222"));
    }
}
