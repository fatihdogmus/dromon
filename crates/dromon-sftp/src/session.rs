//! The SFTP session: one per connection, multiplexing every listing, operation and transfer
//! over the ssh child's stdio.
//!
//! Built on russh-sftp's [`RawSftpSession`] rather than its high-level `SftpSession`, because
//! the high-level type hides the server's extension list and the raw session, and Dromon needs
//! both: the extension list for [`Capabilities`] (`copy-data` in particular), raw `extended`
//! requests for `copy-data`, and raw `readdir` batches to stream large folders in pages.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dromon_core::{Entry, EntryKind, Error, LinkTarget, RemotePath, Result};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::rawsession::Limits as RawLimits;
use russh_sftp::client::{Config, RawSftpSession};
use russh_sftp::extensions::{self, LimitsExtension};
use russh_sftp::protocol::{FileAttributes, OpenFlags, SftpHandle};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::error::{Op, is_eof, is_failure, map_err};
use crate::file::{RemoteFile, WriteMode};

/// Chunk size for reads and writes when the server doesn't advertise `limits@openssh.com`.
pub const DEFAULT_CHUNK: u32 = 32 * 1024;

/// Largest packet this client accepts (russh-sftp's default, and OpenSSH's own limit).
const CLIENT_MAX_PACKET: u32 = 256 * 1024;

/// Largest read or write chunk: a packet minus room for headers, as OpenSSH computes its
/// `max_read_len` (`SFTP_MAX_MSG_LENGTH - 1024` = 261,120).
pub const MAX_CHUNK: u32 = CLIENT_MAX_PACKET - 1024;

/// Symlink targets resolved at once while listing.
const LINK_STATS_IN_FLIGHT: usize = 64;

/// The server's `limits@openssh.com` reply. Zero means "not limited" (per the extension).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    pub max_packet: u64,
    pub max_read: u64,
    pub max_write: u64,
    pub max_open_handles: u64,
}

impl From<&LimitsExtension> for Limits {
    fn from(l: &LimitsExtension) -> Self {
        Limits {
            max_packet: l.max_packet_len,
            max_read: l.max_read_len,
            max_write: l.max_write_len,
            max_open_handles: l.max_open_handles,
        }
    }
}

impl From<Limits> for RawLimits {
    fn from(l: Limits) -> Self {
        let nz = |v: u64| (v > 0).then_some(v);
        RawLimits {
            packet_len: nz(l.max_packet),
            read_len: nz(l.max_read),
            write_len: nz(l.max_write),
            open_handles: nz(l.max_open_handles),
        }
    }
}

/// What the server supports, from the extensions in its `SSH_FXP_VERSION` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// `posix-rename@openssh.com` version 1: atomic rename that replaces the target.
    pub posix_rename: bool,
    /// `copy-data` version 1: server-side copy between two open handles.
    pub copy_data: bool,
    /// From `limits@openssh.com`, when advertised and the query succeeded.
    pub limits: Option<Limits>,
    /// Every advertised extension and its version string, for the connection log.
    pub extensions: BTreeMap<String, String>,
}

impl Capabilities {
    /// Parses the extension list of a version reply. `limits` is left `None`: it needs a query.
    pub fn from_extensions(extensions: &HashMap<String, String>) -> Capabilities {
        let has = |name: &str, ver: &str| extensions.get(name).is_some_and(|v| v == ver);
        Capabilities {
            posix_rename: has(extensions::POSIX_RENAME, "1"),
            copy_data: has(COPY_DATA, "1"),
            limits: None,
            extensions: extensions.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        }
    }

    /// True when the server advertises `limits@openssh.com` version 1.
    pub fn advertises_limits(&self) -> bool {
        self.extensions.get(extensions::LIMITS).is_some_and(|v| v == "1")
    }

    /// Bytes per read or write request: the smaller of the server's read and write limits,
    /// capped at [`MAX_CHUNK`]; [`DEFAULT_CHUNK`] when the server states no limits.
    pub fn chunk_size(&self) -> u32 {
        let Some(l) = self.limits else { return DEFAULT_CHUNK };
        if l.max_read == 0 && l.max_write == 0 {
            return DEFAULT_CHUNK;
        }
        let mut chunk = u64::from(MAX_CHUNK);
        for v in [l.max_read, l.max_write] {
            if v > 0 {
                chunk = chunk.min(v);
            }
        }
        if l.max_packet > 1024 {
            chunk = chunk.min(l.max_packet - 1024);
        }
        u32::try_from(chunk).unwrap_or(MAX_CHUNK).max(1)
    }
}

/// The extension name of the server-side copy (draft-ietf-secsh-filexfer-extensions-00 §7).
pub(crate) const COPY_DATA: &str = "copy-data";

/// One SFTP session. Cheap to clone: clones share the session, so every task for a host
/// multiplexes over the same channel. Requests from concurrent tasks are pipelined.
///
/// Every method that talks to the server needs to run inside a tokio runtime.
#[derive(Clone)]
pub struct Sftp {
    pub(crate) raw: Arc<RawSftpSession>,
    caps: Arc<Capabilities>,
}

impl Sftp {
    /// Starts SFTP on `stream` (for Dromon: the ssh child's stdout and stdin joined with
    /// [`tokio::io::join`]), reads the server's extensions and queries `limits@openssh.com`
    /// when advertised.
    ///
    /// The version handshake has no timeout: ssh may sit on an askpass prompt (password, host
    /// key) for as long as the user takes. It ends when the server answers or the stream
    /// closes (ssh exited, e.g. after `ConnectTimeout` or a cancelled prompt). Every later
    /// request uses the [`Sftp::set_timeout`] reply timeout, 10 s by default.
    pub async fn new<S>(stream: S) -> Result<Sftp, Error>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let cfg = Config { max_packet_len: CLIENT_MAX_PACKET, ..Config::default() };
        let timeout = cfg.request_timeout_secs;
        let mut raw = RawSftpSession::new_with_config(stream, cfg);
        raw.set_timeout(u64::from(u32::MAX));
        let version = raw.init().await.map_err(|e| match map_err(e.clone(), Op::Open, &RemotePath::root()) {
            Error::ConnectionLost => Error::Connect("the connection closed before SFTP started".into()),
            _ => Error::Connect(format!("SFTP didn't start: {e}")),
        })?;
        raw.set_timeout(timeout);
        let mut caps = Capabilities::from_extensions(&version.extensions);
        if caps.advertises_limits() {
            // Not fatal: without limits we fall back to conservative chunks.
            if let Ok(reply) = raw.limits().await {
                let limits = Limits::from(&reply);
                raw.set_limits(limits.into());
                caps.limits = Some(limits);
            }
        }
        Ok(Sftp { raw: Arc::new(raw), caps: Arc::new(caps) })
    }

    pub fn capabilities(&self) -> Capabilities {
        (*self.caps).clone()
    }

    pub(crate) fn caps(&self) -> &Capabilities {
        &self.caps
    }

    /// Bytes per read or write request, see [`Capabilities::chunk_size`].
    pub fn chunk_size(&self) -> u32 {
        self.caps.chunk_size()
    }

    /// True when `self` and `other` are clones of one session (not just sessions to one host).
    pub fn same_session(&self, other: &Sftp) -> bool {
        Arc::ptr_eq(&self.raw, &other.raw)
    }

    /// Seconds to wait for each reply once its request is written (russh-sftp default: 10).
    pub fn set_timeout(&self, secs: u64) {
        self.raw.set_timeout(secs);
    }

    /// Closes the channel. Pending and later requests fail with [`Error::ConnectionLost`].
    pub fn close(&self) {
        let _ = self.raw.close_session();
    }

    /// The login folder: `realpath(".")`.
    pub async fn home(&self) -> Result<RemotePath> {
        self.realpath_str(".", &RemotePath::root()).await
    }

    /// The canonical absolute path, with symlinks resolved.
    pub async fn realpath(&self, path: &RemotePath) -> Result<RemotePath> {
        self.realpath_str(path.as_str(), path).await
    }

    async fn realpath_str(&self, path: &str, for_err: &RemotePath) -> Result<RemotePath> {
        let name = self.raw.realpath(path).await.map_err(|e| map_err(e, Op::Realpath, for_err))?;
        match name.files.first() {
            Some(f) => Ok(RemotePath::new(&f.filename)),
            None => Err(Error::Io(format!("Couldn't resolve {for_err}: empty reply"))),
        }
    }

    /// Follows symlinks: the kind is never `Symlink`.
    pub async fn stat(&self, path: &RemotePath) -> Result<Entry> {
        let attrs = self.raw.stat(path.as_str()).await.map_err(|e| map_err(e, Op::Stat, path))?;
        Ok(entry_from_attrs(display_name(path), &attrs.attrs, ""))
    }

    /// Doesn't follow symlinks; for a symlink the target's kind is resolved as in [`Sftp::list`].
    pub async fn lstat(&self, path: &RemotePath) -> Result<Entry> {
        let attrs = self.raw.lstat(path.as_str()).await.map_err(|e| map_err(e, Op::Stat, path))?;
        let mut entry = entry_from_attrs(display_name(path), &attrs.attrs, "");
        if entry.kind.is_symlink() {
            let target = self.raw.stat(path.as_str()).await.ok();
            entry.kind = EntryKind::Symlink { target: target.map(|a| link_target(&a.attrs)) };
        }
        Ok(entry)
    }

    /// True when something (anything, including a dangling symlink) exists at `path`.
    pub async fn exists(&self, path: &RemotePath) -> Result<bool> {
        match self.raw.lstat(path.as_str()).await {
            Ok(_) => Ok(true),
            Err(e) => match map_err(e, Op::Stat, path) {
                Error::NotFound(_) => Ok(false),
                other => Err(other),
            },
        }
    }

    /// The whole folder, in server order, without `.` and `..`. Symlinks carry the kind of
    /// their target (`None` when dangling or unreadable).
    pub async fn list(&self, path: &RemotePath) -> Result<Vec<Entry>> {
        let mut reader = DirReader::open(self.raw.clone(), path.clone()).await?;
        let mut all = Vec::new();
        while let Some(page) = reader.next_page(usize::MAX).await? {
            all.extend(page);
        }
        Ok(all)
    }

    /// Streams the folder in pages of `page` entries (the last page may be shorter), each
    /// with its symlinks resolved, so the first rows can paint before the folder is read.
    ///
    /// The channel yields at least one page on success (an empty one for an empty folder),
    /// or one `Err` and nothing after it; it closes when the listing is complete. Dropping
    /// the receiver stops the listing and closes the directory handle. Spawns a tokio task.
    ///
    /// Pages are filled from the server's `readdir` batches (OpenSSH sends 100 names per
    /// batch), read one batch at a time on the raw session.
    pub fn list_paged(&self, path: &RemotePath, page: usize) -> mpsc::Receiver<Result<Vec<Entry>>> {
        let page = page.max(1);
        let (tx, rx) = mpsc::channel(2);
        let raw = self.raw.clone();
        let path = path.clone();
        tokio::spawn(async move {
            let mut reader = match DirReader::open(raw, path).await {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            };
            let mut sent = false;
            loop {
                let next = tokio::select! {
                    next = reader.next_page(page) => next,
                    () = tx.closed() => return,
                };
                match next {
                    Ok(Some(entries)) => {
                        sent = true;
                        if tx.send(Ok(entries)).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) => {
                        if !sent {
                            let _ = tx.send(Ok(Vec::new())).await;
                        }
                        return;
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                }
            }
        });
        rx
    }

    /// Creates a folder with the server's default mode. [`Error::AlreadyExists`] if taken.
    pub async fn mkdir(&self, path: &RemotePath) -> Result<()> {
        match self.raw.mkdir(path.as_str(), FileAttributes::empty()).await {
            Ok(_) => Ok(()),
            Err(e) => Err(self.exists_or(e, Op::Mkdir, path, path).await),
        }
    }

    /// Plain SFTP rename. OpenSSH refuses to replace an existing target:
    /// [`Error::AlreadyExists`]. Use [`Sftp::posix_rename`] to replace atomically.
    pub async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<()> {
        match self.raw.rename(from.as_str(), to.as_str()).await {
            Ok(_) => Ok(()),
            Err(e) => Err(self.exists_or(e, Op::Rename, from, to).await),
        }
    }

    /// Atomic rename replacing `to` if it exists (`posix-rename@openssh.com`).
    /// [`Error::Unsupported`]`("posix-rename")` when the server doesn't advertise it.
    pub async fn posix_rename(&self, from: &RemotePath, to: &RemotePath) -> Result<()> {
        if !self.caps.posix_rename {
            return Err(Error::Unsupported("posix-rename"));
        }
        self.raw.posix_rename(from.as_str(), to.as_str()).await.map(drop).map_err(|e| map_err(e, Op::PosixRename, from))
    }

    pub async fn remove_file(&self, path: &RemotePath) -> Result<()> {
        self.raw.remove(path.as_str()).await.map(drop).map_err(|e| map_err(e, Op::RemoveFile, path))
    }

    /// Removes an empty folder.
    pub async fn remove_dir(&self, path: &RemotePath) -> Result<()> {
        self.raw.rmdir(path.as_str()).await.map(drop).map_err(|e| map_err(e, Op::RemoveDir, path))
    }

    /// Sets the modification time (seconds since the Unix epoch, clamped to SFTP v3's u32).
    /// SFTP v3 sets atime and mtime together; atime is set to now.
    pub async fn set_mtime(&self, path: &RemotePath, secs: i64) -> Result<()> {
        let attrs = FileAttributes {
            atime: Some(clamp_time(now_secs())),
            mtime: Some(clamp_time(secs)),
            ..FileAttributes::empty()
        };
        self.raw.setstat(path.as_str(), attrs).await.map(drop).map_err(|e| map_err(e, Op::SetTimes, path))
    }

    /// Opens a file for reading.
    pub async fn open_read(&self, path: &RemotePath) -> Result<RemoteFile> {
        self.open(path, OpenFlags::READ).await
    }

    /// Opens a file for writing, see [`WriteMode`].
    pub async fn open_write(&self, path: &RemotePath, mode: WriteMode) -> Result<RemoteFile> {
        let flags = match mode {
            WriteMode::Truncate => OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
            WriteMode::CreateNew => OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
            WriteMode::Resume => OpenFlags::WRITE | OpenFlags::CREATE,
        };
        self.open(path, flags).await
    }

    async fn open(&self, path: &RemotePath, flags: OpenFlags) -> Result<RemoteFile> {
        match self.raw.open(path.as_str(), flags, FileAttributes::empty()).await {
            Ok(h) => Ok(RemoteFile::new(self.raw.clone(), h.handle, path.clone())),
            Err(e) if flags.contains(OpenFlags::EXCLUDE) => Err(self.exists_or(e, Op::Open, path, path).await),
            Err(e) => Err(map_err(e, Op::Open, path)),
        }
    }

    /// Maps `err` from an operation on `path`; a generic `Failure` with something at `target`
    /// becomes [`Error::AlreadyExists`]`(target)`, since SFTP v3 has no status for `EEXIST`.
    pub(crate) async fn exists_or(&self, err: SftpError, op: Op, path: &RemotePath, target: &RemotePath) -> Error {
        if is_failure(&err) && self.raw.lstat(target.as_str()).await.is_ok() {
            return Error::AlreadyExists(target.clone());
        }
        map_err(err, op, path)
    }
}

/// An open directory handle read one `readdir` batch at a time.
struct DirReader {
    raw: Arc<RawSftpSession>,
    path: RemotePath,
    handle: Option<SftpHandle>,
    pending: Vec<Entry>,
}

impl DirReader {
    async fn open(raw: Arc<RawSftpSession>, path: RemotePath) -> Result<DirReader> {
        let handle = raw.opendir(path.as_str()).await.map_err(|e| map_err(e, Op::List, &path))?;
        Ok(DirReader { raw, path, handle: Some(handle.handle), pending: Vec::new() })
    }

    /// The next `page` entries with symlinks resolved, or `None` once everything was returned.
    async fn next_page(&mut self, page: usize) -> Result<Option<Vec<Entry>>> {
        while self.pending.len() < page {
            let Some(handle) = self.handle.clone() else { break };
            match self.raw.readdir(handle).await {
                Ok(name) => {
                    for f in name.files {
                        if f.filename != "." && f.filename != ".." {
                            self.pending.push(entry_from_attrs(f.filename, &f.attrs, &f.longname));
                        }
                    }
                }
                Err(e) => {
                    self.close().await;
                    if is_eof(&e) {
                        break;
                    }
                    return Err(map_err(e, Op::List, &self.path));
                }
            }
        }
        if self.pending.is_empty() {
            return Ok(None);
        }
        let take = page.min(self.pending.len());
        let mut out: Vec<Entry> = self.pending.drain(..take).collect();
        resolve_links(&self.raw, &self.path, &mut out).await;
        Ok(Some(out))
    }

    async fn close(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = self.raw.close(h).await;
        }
    }
}

impl Drop for DirReader {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            spawn_close(&self.raw, h);
        }
    }
}

/// Closes a handle in the background (from `Drop`, where we can't await).
pub(crate) fn spawn_close(raw: &Arc<RawSftpSession>, handle: SftpHandle) {
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let raw = raw.clone();
        rt.spawn(async move {
            let _ = raw.close(handle).await;
        });
    }
}

/// Fills in the target kind of every symlink in `entries` with concurrent `stat`s.
async fn resolve_links(raw: &Arc<RawSftpSession>, dir: &RemotePath, entries: &mut [Entry]) {
    fn apply(res: Option<(usize, Option<LinkTarget>)>, entries: &mut [Entry]) {
        if let Some((i, target)) = res {
            entries[i].kind = EntryKind::Symlink { target };
        }
    }
    let mut set = JoinSet::new();
    for i in 0..entries.len() {
        if !entries[i].kind.is_symlink() {
            continue;
        }
        if set.len() >= LINK_STATS_IN_FLIGHT {
            let res = set.join_next().await.and_then(|r| r.ok());
            apply(res, entries);
        }
        let raw = raw.clone();
        let path = String::from(dir.join(&entries[i].name));
        set.spawn(async move { (i, raw.stat(path).await.ok().map(|a| link_target(&a.attrs))) });
    }
    while let Some(res) = set.join_next().await {
        apply(res.ok(), entries);
    }
}

const S_IFMT: u32 = 0o170_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFREG: u32 = 0o100_000;
const S_IFLNK: u32 = 0o120_000;

/// Builds an [`Entry`] from SFTP attributes. The kind comes from the `st_mode` type bits, or,
/// when the server sends no permissions, from the first letter of `longname` (`ls -l` style).
/// A symlink's target is left `None` for the caller to resolve. The owner and group are the
/// names in `longname` when it reads as an `ls -l` line ([`longname_owner`]), else the numeric
/// uid and gid of the attributes.
pub(crate) fn entry_from_attrs(name: String, attrs: &FileAttributes, longname: &str) -> Entry {
    let kind = match attrs.permissions {
        Some(mode) => match mode & S_IFMT {
            S_IFDIR => EntryKind::Dir,
            S_IFREG => EntryKind::File,
            S_IFLNK => EntryKind::Symlink { target: None },
            _ => EntryKind::Other,
        },
        None => match longname.as_bytes().first() {
            Some(b'd') => EntryKind::Dir,
            Some(b'-') => EntryKind::File,
            Some(b'l') => EntryKind::Symlink { target: None },
            _ => EntryKind::Other,
        },
    };
    let (owner, group) = match longname_owner(longname) {
        Some((owner, group)) => (Some(owner.to_owned()), Some(group.to_owned())),
        None => (attrs.uid.map(|u| u.to_string()), attrs.gid.map(|g| g.to_string())),
    };
    Entry {
        name,
        kind,
        size: attrs.size.unwrap_or(0),
        mtime: attrs.mtime.map(i64::from),
        mode: attrs.permissions,
        owner,
        group,
    }
}

/// The owner and group of an `ls -l` style `longname`, as OpenSSH sends it in a `readdir`
/// reply: `-rw-r--r--    1 alice    staff        4321 Jan  1 12:00 name`. Only a line whose
/// first field reads as a mode string, followed by a link count, the two names and a size, is
/// taken; anything else (empty, another server's own format) is `None`. SFTP v3 leaves the
/// longname's format unspecified, so this is a best effort the caller falls back from.
pub(crate) fn longname_owner(longname: &str) -> Option<(&str, &str)> {
    let mut fields = longname.split_whitespace();
    let mode = fields.next()?;
    let links = fields.next()?;
    let (owner, group, size) = (fields.next()?, fields.next()?, fields.next()?);
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let mode = mode.as_bytes();
    let mode_like =
        mode.len() >= 10 && b"-dlcbpsD".contains(&mode[0]) && mode[1..10].iter().all(|b| b"rwxsStTl-".contains(b));
    (mode_like && digits(links) && digits(size)).then_some((owner, group))
}

fn link_target(attrs: &FileAttributes) -> LinkTarget {
    match attrs.permissions.map(|m| m & S_IFMT) {
        Some(S_IFDIR) => LinkTarget::Dir,
        Some(S_IFREG) => LinkTarget::File,
        _ => LinkTarget::Other,
    }
}

/// The name shown for a stat'ed path: its last component, or `/` for the root.
pub(crate) fn display_name(path: &RemotePath) -> String {
    path.file_name().unwrap_or("/").to_owned()
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn clamp_time(secs: i64) -> u32 {
    secs.clamp(0, i64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(mode: Option<u32>, size: u64, mtime: u32) -> FileAttributes {
        FileAttributes { size: Some(size), permissions: mode, mtime: Some(mtime), ..Default::default() }
    }

    #[test]
    fn kinds_from_mode_bits() {
        let e = entry_from_attrs("d".into(), &attrs(Some(0o040_755), 4096, 5), "");
        assert_eq!((e.kind, e.size, e.mtime, e.mode), (EntryKind::Dir, 4096, Some(5), Some(0o040_755)));
        let f = entry_from_attrs("f".into(), &attrs(Some(0o100_644), 10, 1), "");
        assert_eq!(f.kind, EntryKind::File);
        let l = entry_from_attrs("l".into(), &attrs(Some(0o120_777), 3, 1), "");
        assert_eq!(l.kind, EntryKind::Symlink { target: None });
        let p = entry_from_attrs("p".into(), &attrs(Some(0o010_644), 0, 1), "");
        assert_eq!(p.kind, EntryKind::Other);
    }

    #[test]
    fn kinds_from_longname_without_permissions() {
        let a = attrs(None, 0, 0);
        assert_eq!(entry_from_attrs("x".into(), &a, "drwxr-xr-x 2 u g 0 x").kind, EntryKind::Dir);
        assert_eq!(entry_from_attrs("x".into(), &a, "-rw-r--r-- 1 u g 0 x").kind, EntryKind::File);
        assert!(entry_from_attrs("x".into(), &a, "lrwxrwxrwx 1 u g 0 x").kind.is_symlink());
        assert_eq!(entry_from_attrs("x".into(), &a, "").kind, EntryKind::Other);
    }

    #[test]
    fn owner_and_group_from_an_openssh_longname() {
        let line = "-rw-r--r--    1 alice    staff        4321 Jan  1 12:00 notes.txt";
        assert_eq!(longname_owner(line), Some(("alice", "staff")));
        let dir = "drwxr-xr-x    2 root     wheel        4096 Nov 20  2025 with spaces in name";
        assert_eq!(longname_owner(dir), Some(("root", "wheel")));
        // Setuid, sticky and ACL marks; numeric names when the server has none for the ids.
        assert_eq!(longname_owner("-rwsr-xr-t+ 3 1001 1002 0 Jan 1 12:00 x"), Some(("1001", "1002")));
        assert_eq!(longname_owner("lrwxrwxrwx 1 bob users 7 Jan 1 12:00 l -> target"), Some(("bob", "users")));
        let e = entry_from_attrs(
            "notes.txt".into(),
            &FileAttributes { uid: Some(501), gid: Some(20), ..attrs(Some(0o100_644), 4321, 1) },
            line,
        );
        assert_eq!((e.owner.as_deref(), e.group.as_deref()), (Some("alice"), Some("staff")));
    }

    #[test]
    fn odd_or_missing_longnames_fall_back_to_the_numeric_ids() {
        for odd in [
            "",
            "notes.txt",
            "-rw-r--r-- 1 alice",
            "-rw-r--r-- one alice staff 4321 Jan 1 12:00 x",
            "-rw-r--r-- 1 alice 4321 Jan 1 12:00 x",
            "rw-r--r-- 1 alice staff 4321 Jan 1 12:00 x",
            "-rw-r--r-é 1 alice staff 4321 Jan 1 12:00 x",
            "-rwxrwxrwx",
            "Jan 1 12:00 notes.txt 4321 bytes",
        ] {
            assert_eq!(longname_owner(odd), None, "{odd:?}");
        }
        let with_ids = FileAttributes { uid: Some(501), gid: Some(20), ..attrs(Some(0o100_644), 1, 1) };
        let e = entry_from_attrs("x".into(), &with_ids, "-rw-r--r-- 1 alice");
        assert_eq!((e.owner.as_deref(), e.group.as_deref()), (Some("501"), Some("20")));
        let e = entry_from_attrs("x".into(), &attrs(Some(0o100_644), 1, 1), "");
        assert_eq!((e.owner, e.group), (None, None), "neither names nor ids");
    }

    #[test]
    fn link_targets() {
        assert_eq!(link_target(&attrs(Some(0o040_755), 0, 0)), LinkTarget::Dir);
        assert_eq!(link_target(&attrs(Some(0o100_644), 0, 0)), LinkTarget::File);
        assert_eq!(link_target(&attrs(Some(0o020_666), 0, 0)), LinkTarget::Other);
        assert_eq!(link_target(&attrs(None, 0, 0)), LinkTarget::Other);
    }

    fn exts(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn capabilities_from_openssh_extensions() {
        let caps = Capabilities::from_extensions(&exts(&[
            ("posix-rename@openssh.com", "1"),
            ("limits@openssh.com", "1"),
            ("copy-data", "1"),
            ("statvfs@openssh.com", "2"),
        ]));
        assert!(caps.posix_rename && caps.copy_data && caps.advertises_limits());
        assert_eq!(caps.limits, None);
        assert_eq!(caps.extensions.len(), 4);
    }

    #[test]
    fn capabilities_need_the_right_version() {
        let caps = Capabilities::from_extensions(&exts(&[
            ("posix-rename@openssh.com", "2"),
            ("copy-data", "0"),
            ("limits@openssh.com", "2"),
        ]));
        assert!(!caps.posix_rename && !caps.copy_data && !caps.advertises_limits());
        assert_eq!(Capabilities::from_extensions(&HashMap::new()), Capabilities::default());
    }

    #[test]
    fn chunk_size_from_limits() {
        let mut caps = Capabilities::default();
        assert_eq!(caps.chunk_size(), DEFAULT_CHUNK);
        // OpenSSH: 256 KiB packets, 261,120-byte reads and writes.
        caps.limits = Some(Limits { max_packet: 262_144, max_read: 261_120, max_write: 261_120, max_open_handles: 0 });
        assert_eq!(caps.chunk_size(), 261_120);
        caps.limits = Some(Limits { max_packet: 0, max_read: 65_536, max_write: 0, max_open_handles: 0 });
        assert_eq!(caps.chunk_size(), 65_536);
        caps.limits = Some(Limits { max_packet: 0, max_read: 1 << 30, max_write: 1 << 30, max_open_handles: 0 });
        assert_eq!(caps.chunk_size(), MAX_CHUNK);
        caps.limits = Some(Limits { max_packet: 0, max_read: 0, max_write: 0, max_open_handles: 0 });
        assert_eq!(caps.chunk_size(), DEFAULT_CHUNK);
        caps.limits = Some(Limits { max_packet: 34_816, max_read: 0, max_write: 1 << 20, max_open_handles: 0 });
        assert_eq!(caps.chunk_size(), 33_792);
    }

    #[test]
    fn raw_limits_treat_zero_as_unlimited() {
        let raw = RawLimits::from(Limits { max_packet: 0, max_read: 10, max_write: 0, max_open_handles: 7 });
        assert_eq!((raw.packet_len, raw.read_len, raw.write_len, raw.open_handles), (None, Some(10), None, Some(7)));
    }

    #[test]
    fn times_are_clamped() {
        assert_eq!(clamp_time(-5), 0);
        assert_eq!(clamp_time(1_577_934_245), 1_577_934_245);
        assert_eq!(clamp_time(i64::MAX), u32::MAX);
        assert_eq!(display_name(&RemotePath::root()), "/");
        assert_eq!(display_name(&RemotePath::new("/a/b")), "b");
    }
}
