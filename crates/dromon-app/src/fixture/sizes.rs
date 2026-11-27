//! The Properties sheet's folder sizes over the fixture's trees (`Command::FolderSize`), at
//! once: the service's own count ([`Sizes`]) on the trees' walk.

use dromon_core::{Error, HostId, RemotePath};

use super::backend::Core;
use crate::service::sizes::Sizes;
use crate::state::{ServiceEvent, SizeCommand, SizeEvent, SizeId, SizeTotal};

impl Core {
    /// A folder size command from window `Core::from`. A walk is over by the time its sheet
    /// could cancel it, so cancelling does nothing.
    pub(super) fn folder_size(&mut self, cmd: SizeCommand, out: &mut Vec<ServiceEvent>) {
        match cmd {
            SizeCommand::Start { id, host, path } => self.walk_size(id, host, path, out),
            SizeCommand::Cancel { .. } => {}
        }
    }

    /// The Properties sheet's Calculate, walked at once over the trees: one progress event,
    /// then Done, or Failed when the walk can't go on (a folder whose listing loses the
    /// connection drops it, as a file operation's does). Not connected: `ConnectionLost` at
    /// once.
    fn walk_size(&mut self, id: SizeId, host: HostId, path: RemotePath, out: &mut Vec<ServiceEvent>) {
        if !self.up(&host) {
            let ev = SizeEvent::Failed { id, host, total: SizeTotal::default(), error: Error::ConnectionLost };
            out.push(ServiceEvent::FolderSize(ev));
            return;
        }
        let mut sizes = Sizes::new(|_| {});
        let walked = self.transfers.tree.walk(&host, &path, &mut sizes);
        let total = sizes.total;
        match walked {
            Ok(()) => {
                out.push(ServiceEvent::FolderSize(SizeEvent::Progress { id, host: host.clone(), total }));
                out.push(ServiceEvent::FolderSize(SizeEvent::Done { id, host, total }));
            }
            Err(error) => {
                let lost = error == Error::ConnectionLost;
                out.push(ServiceEvent::FolderSize(SizeEvent::Failed { id, host: host.clone(), total, error }));
                if lost {
                    self.walk_lost(&host, out);
                }
            }
        }
    }
}
