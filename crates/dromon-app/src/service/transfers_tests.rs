//! The engine's own handling of commands that need no session.

use std::path::PathBuf;

use dromon_core::{EntryKind, HostId, RemotePath};
use dromon_transfer::{JobRequest, RemoteItem};

use super::*;

#[derive(Clone)]
struct Quiet;

impl Sink for Quiet {
    fn emit(&self, _: ServiceEvent) {}
}

#[tokio::test]
async fn settings_change_the_per_host_limit_of_the_running_engine() {
    let mut engine = Engine::new(Quiet);
    assert_eq!(engine.queue.per_host(), PER_HOST);
    engine.on_message(EngineMsg::Command(TransferCommand::PerHost(6)));
    assert_eq!(engine.queue.per_host(), 6);
    // Out of range from a caller that didn't clamp: never zero, which would stop every host.
    engine.on_message(EngineMsg::Command(TransferCommand::PerHost(0)));
    assert_eq!(engine.queue.per_host(), 1);
    // The jobs that are there stay; the limit is for the next starts.
    let host = HostId("prod".into());
    let item = RemoteItem { path: RemotePath::new("/a"), kind: EntryKind::File, size: 1 };
    let req = JobRequest::Download { host, items: vec![item], into: PathBuf::from("/tmp/d") };
    engine.on_message(EngineMsg::Command(TransferCommand::Start(req)));
    engine.on_message(EngineMsg::Command(TransferCommand::PerHost(8)));
    assert_eq!(engine.queue.views(engine.now()).len(), 1);
    assert_eq!(engine.queue.per_host(), 8);
}
