//! In-process tests against russh-sftp's server over a duplex pipe (no Docker): what goes on
//! the wire for `copy-data`, the unsupported-extension paths and paged listing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use dromon_core::{EntryKind, Error, LinkTarget, RemotePath};
use dromon_sftp::Sftp;
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, Packet, SftpHandle, SftpString, Status,
    StatusCode, Version,
};

type Log = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// Files: every path has `size` bytes, its handle is the path itself. Folders: `/dir` holds
/// `names` regular files, plus `.`, `..` and a symlink `link` (to a folder), 100 per batch.
struct Mock {
    extensions: HashMap<String, String>,
    size: u64,
    names: usize,
    next: usize,
    log: Log,
}

fn ok(id: u32) -> Status {
    Status { id, status_code: StatusCode::Ok, error_message: String::new(), language_tag: "en".into() }
}

fn attrs(mode: u32, size: u64) -> FileAttributes {
    FileAttributes { size: Some(size), permissions: Some(mode), mtime: Some(1), ..Default::default() }
}

impl russh_sftp::server::Handler for Mock {
    type Error = StatusCode;

    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _: u32, _: HashMap<String, String>) -> Result<Version, StatusCode> {
        Ok(Version { version: 3, extensions: self.extensions.clone() })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: SftpString,
        _: russh_sftp::protocol::OpenFlags,
        _: FileAttributes,
    ) -> Result<Handle, StatusCode> {
        Ok(Handle { id, handle: SftpHandle::from(filename) })
    }

    async fn fstat(&mut self, id: u32, _: SftpHandle) -> Result<Attrs, StatusCode> {
        Ok(Attrs { id, attrs: attrs(0o100_644, self.size) })
    }

    async fn stat(&mut self, id: u32, path: SftpString) -> Result<Attrs, StatusCode> {
        match path.as_str() {
            "/dir/link" => Ok(Attrs { id, attrs: attrs(0o040_755, 0) }),
            _ => Err(StatusCode::NoSuchFile),
        }
    }

    async fn read(&mut self, id: u32, _: SftpHandle, off: u64, len: u32) -> Result<Data, StatusCode> {
        self.log.lock().unwrap().push(("read".into(), off.to_be_bytes().to_vec()));
        if off >= self.size {
            return Err(StatusCode::Eof);
        }
        Ok(Data { id, data: vec![7; (self.size - off).min(u64::from(len)) as usize] })
    }

    async fn close(&mut self, id: u32, _: SftpHandle) -> Result<Status, StatusCode> {
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: SftpString) -> Result<Handle, StatusCode> {
        if path != "/dir" {
            return Err(StatusCode::PermissionDenied);
        }
        self.next = 0;
        Ok(Handle { id, handle: SftpHandle::from(path) })
    }

    async fn readdir(&mut self, id: u32, _: SftpHandle) -> Result<Name, StatusCode> {
        let mut all: Vec<File> = vec![
            File::new(".", attrs(0o040_755, 0)),
            File::new("..", attrs(0o040_755, 0)),
            File::new("link", attrs(0o120_777, 3)),
        ];
        all.extend((0..self.names).map(|i| File::new(format!("n{i:05}"), attrs(0o100_644, 1))));
        if self.next >= all.len() {
            return Err(StatusCode::Eof);
        }
        let end = (self.next + 100).min(all.len());
        let files = all.drain(self.next..end).collect();
        self.next = end;
        Ok(Name { id, files })
    }

    async fn extended(&mut self, id: u32, request: String, data: Vec<u8>) -> Result<Packet, StatusCode> {
        self.log.lock().unwrap().push((request, data));
        Ok(ok(id).into())
    }
}

async fn start(extensions: &[(&str, &str)], size: u64, names: usize) -> (Sftp, Log) {
    let log = Log::default();
    let mock = Mock {
        extensions: extensions.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        size,
        names,
        next: 0,
        log: log.clone(),
    };
    let (client, server) = tokio::io::duplex(1 << 20);
    russh_sftp::server::run(server, mock).await;
    (Sftp::new(client).await.unwrap(), log)
}

fn p(s: &str) -> RemotePath {
    RemotePath::new(s)
}

#[tokio::test]
async fn stream_closing_before_the_handshake_is_a_connect_error() {
    let (client, server) = tokio::io::duplex(1024);
    drop(server);
    let err = Sftp::new(client).await.err();
    assert_eq!(err, Some(Error::Connect("the connection closed before SFTP started".into())));
}

#[tokio::test]
async fn session_closing_later_is_connection_lost() {
    let (sftp, _) = start(&[], 10, 0).await;
    sftp.close();
    assert_eq!(sftp.stat(&p("/dir/link")).await, Err(Error::ConnectionLost));
}

#[tokio::test]
async fn no_extensions_means_unsupported() {
    let (sftp, log) = start(&[], 10, 0).await;
    let caps = sftp.capabilities();
    assert!(!caps.posix_rename && !caps.copy_data && caps.limits.is_none());
    assert_eq!(sftp.chunk_size(), dromon_sftp::DEFAULT_CHUNK);
    assert_eq!(sftp.copy_data(&p("/a"), &p("/b")).await, Err(Error::Unsupported("copy-data")));
    assert_eq!(sftp.posix_rename(&p("/a"), &p("/b")).await, Err(Error::Unsupported("posix-rename")));
    assert!(log.lock().unwrap().is_empty(), "no extended request was sent");
    // The server answers OpUnsupported to anything it doesn't implement.
    assert_eq!(sftp.mkdir(&p("/x")).await, Err(Error::Unsupported("creating folders")));
}

fn copy_data_payload(read: &[u8], off: u64, len: u64, write: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend((read.len() as u32).to_be_bytes());
    v.extend(read);
    v.extend(off.to_be_bytes());
    v.extend(len.to_be_bytes());
    v.extend((write.len() as u32).to_be_bytes());
    v.extend(write);
    v.extend(off.to_be_bytes());
    v
}

#[tokio::test]
async fn copy_data_requests_on_the_wire() {
    const MIB: u64 = 1 << 20;
    let (sftp, log) = start(&[("copy-data", "1")], 70 * MIB, 0).await;
    let mut progress = Vec::new();
    sftp.copy_data_with_progress(&p("/src"), &p("/dst"), |n| progress.push(n)).await.unwrap();
    assert_eq!(progress, [32 * MIB, 64 * MIB, 70 * MIB]);
    let log = log.lock().unwrap();
    let expected: Vec<(String, Vec<u8>)> = [(0, 32 * MIB), (32 * MIB, 32 * MIB), (64 * MIB, 6 * MIB)]
        .into_iter()
        .map(|(off, len)| ("copy-data".to_owned(), copy_data_payload(b"/src", off, len, b"/dst")))
        .collect();
    assert_eq!(*log, expected);
}

#[tokio::test]
async fn reads_are_sent_before_they_are_awaited() {
    let (sftp, log) = start(&[], 100, 0).await;
    let file = sftp.open_read(&p("/f")).await.unwrap();
    let reads: Vec<_> = [0, 40, 80, 120].into_iter().map(|off| file.read_at(off, 40)).collect();
    // Nothing awaited yet; give the server time to see the requests.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let offsets: Vec<Vec<u8>> = log.lock().unwrap().iter().map(|(_, d)| d.clone()).collect();
    let want: Vec<Vec<u8>> = [0u64, 40, 80, 120].iter().map(|o| o.to_be_bytes().to_vec()).collect();
    assert_eq!(offsets, want, "all four reads on the wire, in order");
    let mut lens = Vec::new();
    for r in reads {
        lens.push(r.await.unwrap().len());
    }
    assert_eq!(lens, [40, 40, 20, 0], "short read at the end, then empty at EOF");
    file.close().await.unwrap();
}

#[tokio::test]
async fn paged_listing_over_batches() {
    let (sftp, _) = start(&[], 0, 1050).await;
    let mut rx = sftp.list_paged(&p("/dir"), 512);
    let mut pages = Vec::new();
    while let Some(page) = rx.recv().await {
        pages.push(page.unwrap());
    }
    let sizes: Vec<usize> = pages.iter().map(Vec::len).collect();
    assert_eq!(sizes, [512, 512, 27], "1050 files + link, without . and ..");
    assert_eq!(pages[0][0].name, "link");
    assert_eq!(pages[0][0].kind, EntryKind::Symlink { target: Some(LinkTarget::Dir) });

    let all = sftp.list(&p("/dir")).await.unwrap();
    assert_eq!(all.len(), 1051);
    assert!(all.iter().all(|e| e.name != "." && e.name != ".."));

    let mut rx = sftp.list_paged(&p("/other"), 512);
    assert_eq!(rx.recv().await, Some(Err(Error::PermissionDenied(p("/other")))));
    assert_eq!(rx.recv().await, None);
}
