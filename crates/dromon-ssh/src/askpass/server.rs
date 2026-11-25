//! The app side of askpass: accepts helper connections, checks their token, hands the prompt to
//! the service thread, and writes the answer back.
//!
//! Each accepted connection carries one request. Unknown tokens and malformed or oversized
//! requests close the connection without forwarding anything. Answers are never logged.

// The token registry and `serve_connection` are platform-neutral, but only the Unix transport
// uses them until the Windows named-pipe transport lands.
#![cfg_attr(not(unix), allow(dead_code))]

use std::fmt;
use std::io;
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dromon_core::{HostId, PromptAnswer};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

use super::protocol::{Reply, decode_request, encode_reply, read_message_async};
#[cfg(unix)]
use crate::command::AskpassEnv;

/// How long a helper may take to send its request after connecting.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How many prompts may wait for the service thread.
pub const REQUEST_QUEUE: usize = 32;

/// A prompt from ssh for one of our connections. Answer through `reply`; dropping it cancels.
/// If the helper goes away first (ssh exited), `reply.is_closed()` turns true and an answer is
/// discarded.
pub struct AskpassRequest {
    pub host: HostId,
    pub prompt: String,
    pub hint: Option<String>,
    pub reply: oneshot::Sender<PromptAnswer>,
}

impl fmt::Debug for AskpassRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AskpassRequest")
            .field("host", &self.host)
            .field("prompt", &self.prompt)
            .field("hint", &self.hint)
            .finish_non_exhaustive()
    }
}

/// One issued token and the host it belongs to.
type TokenEntry = (Zeroizing<String>, HostId);

/// Issued tokens and the host each belongs to.
#[derive(Clone, Default)]
pub(crate) struct Registry {
    tokens: Arc<Mutex<Vec<TokenEntry>>>,
}

impl Registry {
    pub(crate) fn insert(&self, token: String, host: HostId) {
        self.lock().push((Zeroizing::new(token), host));
    }

    pub(crate) fn remove(&self, token: &str) {
        self.lock().retain(|(t, _)| !constant_time_eq(t.as_bytes(), token.as_bytes()));
    }

    /// The host for `token`, comparing against every entry in constant time.
    pub(crate) fn lookup(&self, token: &str) -> Option<HostId> {
        let mut found = None;
        for (t, host) in self.lock().iter() {
            if constant_time_eq(t.as_bytes(), token.as_bytes()) {
                found = Some(host.clone());
            }
        }
        found
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<TokenEntry>> {
        self.tokens.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Byte equality whose timing depends only on the lengths (tokens all have the same length).
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

/// A fresh token: 32 random bytes as lowercase hex.
pub(crate) fn new_token() -> io::Result<String> {
    random_hex::<32>()
}

pub(crate) fn random_hex<const N: usize>() -> io::Result<String> {
    let mut bytes = Zeroizing::new([0u8; N]);
    getrandom::fill(&mut *bytes).map_err(|e| io::Error::other(format!("no randomness: {e}")))?;
    Ok(hex::encode(*bytes))
}

/// Serves one helper connection: read the request, check the token, forward it, write the
/// answer. Platform-neutral, so a Windows named pipe can reuse it.
pub(crate) async fn serve_connection<S>(stream: S, registry: Registry, requests: mpsc::Sender<AskpassRequest>) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let line = tokio::time::timeout(REQUEST_TIMEOUT, read_message_async(&mut reader))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "askpass request timed out"))??;
    let request = decode_request(&line)?;
    let Some(host) = registry.lookup(&request.token) else {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "askpass request with an unknown token"));
    };

    let (reply_tx, reply_rx) = oneshot::channel();
    let forwarded = AskpassRequest { host, prompt: request.prompt, hint: request.hint, reply: reply_tx };
    requests.send(forwarded).await.map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "askpass requests are no longer read"))?;

    let answer = tokio::select! {
        answer = reply_rx => answer.unwrap_or(PromptAnswer::Cancel),
        () = hang_up(&mut reader) => return Ok(()),
    };
    let reply = match answer {
        PromptAnswer::Text(text) => Reply::Answer(Zeroizing::new(text)),
        PromptAnswer::Cancel => Reply::Cancel,
    };
    writer.write_all(&encode_reply(&reply)?).await?;
    writer.shutdown().await
}

/// Resolves when the helper closes its end or sends anything more (a protocol violation).
async fn hang_up<R: AsyncRead + Unpin>(reader: &mut R) {
    let mut byte = [0u8; 1];
    let _ = reader.read(&mut byte).await;
}

/// The askpass endpoint for this app instance. Removes its socket when dropped.
pub struct AskpassServer {
    #[cfg(unix)]
    path: PathBuf,
    registry: Registry,
    requests: Option<mpsc::Receiver<AskpassRequest>>,
    #[cfg(unix)]
    accept: tokio::task::JoinHandle<()>,
}

impl fmt::Debug for AskpassServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AskpassServer").field("endpoint", &self.endpoint()).finish_non_exhaustive()
    }
}

impl AskpassServer {
    /// Requests from helpers, in arrival order. `None` after the first call.
    pub fn take_requests(&mut self) -> Option<mpsc::Receiver<AskpassRequest>> {
        self.requests.take()
    }

    /// Forgets a token, so its helper requests are rejected from now on.
    pub fn unregister(&self, token: &str) {
        self.registry.remove(token);
    }
}

#[cfg(unix)]
impl AskpassServer {
    /// Binds a socket `askpass-<pid>-<random>.sock` in `$XDG_RUNTIME_DIR/dromon`, or else in
    /// `<temp>/dromon-<uid>`, creating the folder 0700 and refusing it unless it is a real folder
    /// owned by us. Must be called inside a tokio runtime.
    pub fn bind() -> io::Result<Self> {
        let uid = unix::current_uid()?;
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()).map(PathBuf::from)
            && runtime.is_absolute()
            && let Ok(server) = Self::bind_in_as(&runtime.join("dromon"), uid)
        {
            return Ok(server);
        }
        Self::bind_in_as(&std::env::temp_dir().join(format!("dromon-{uid}")), uid)
    }

    /// [`AskpassServer::bind`] in a given folder, which is created or checked the same way.
    pub fn bind_in(dir: &Path) -> io::Result<Self> {
        Self::bind_in_as(dir, unix::current_uid()?)
    }

    fn bind_in_as(dir: &Path, uid: u32) -> io::Result<Self> {
        use std::os::unix::fs::PermissionsExt;

        unix::ensure_private_dir(dir, uid)?;
        let name = format!("askpass-{}-{}.sock", std::process::id(), random_hex::<8>()?);
        let path = dir.join(name);
        if path.to_str().is_none() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "askpass socket path is not UTF-8"));
        }
        let listener = tokio::net::UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;

        let registry = Registry::default();
        let (tx, rx) = mpsc::channel(REQUEST_QUEUE);
        let accept = tokio::spawn(unix::accept_loop(listener, uid, registry.clone(), tx));
        Ok(AskpassServer { path, registry, requests: Some(rx), accept })
    }

    /// What goes in `DROMON_ASKPASS_ENDPOINT`.
    pub fn endpoint(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    /// Issues a fresh token for one connection attempt to `host` and returns the environment
    /// for its ssh process. `helper_exe` is our own executable (`std::env::current_exe()`).
    /// Call [`AskpassServer::unregister`] with `env.token` when that ssh process is gone.
    pub fn register(&self, host: HostId, helper_exe: &Path) -> io::Result<AskpassEnv> {
        let token = new_token()?;
        self.registry.insert(token.clone(), host);
        Ok(AskpassEnv { exe: helper_exe.to_path_buf(), endpoint: self.endpoint(), token })
    }
}

#[cfg(unix)]
impl Drop for AskpassServer {
    fn drop(&mut self) {
        self.accept.abort();
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(not(unix))]
impl AskpassServer {
    /// Not implemented on this platform yet (Windows named pipes are spike S3).
    pub fn bind() -> io::Result<Self> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "askpass is not supported on this platform yet"))
    }

    pub fn endpoint(&self) -> String {
        String::new()
    }

    pub fn register(&self, _host: HostId, _helper_exe: &std::path::Path) -> io::Result<crate::command::AskpassEnv> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "askpass is not supported on this platform yet"))
    }
}

#[cfg(unix)]
mod unix {
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::path::Path;
    use std::sync::OnceLock;
    use std::time::Duration;

    use tokio::net::UnixListener;
    use tokio::sync::mpsc;

    use super::{AskpassRequest, Registry, random_hex, serve_connection};

    pub(super) async fn accept_loop(listener: UnixListener, uid: u32, registry: Registry, tx: mpsc::Sender<AskpassRequest>) {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    // The 0700 folder already keeps other users out; this is a second check.
                    if stream.peer_cred().is_ok_and(|cred| cred.uid() != uid) {
                        continue;
                    }
                    tokio::spawn(serve_connection(stream, registry.clone(), tx.clone()));
                }
                // Out of file descriptors and the like: wait a little rather than spin.
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    }

    /// Our effective uid, read from a file we create (std has no `getuid`, and unsafe is denied).
    pub(super) fn current_uid() -> io::Result<u32> {
        static UID: OnceLock<u32> = OnceLock::new();
        if let Some(uid) = UID.get() {
            return Ok(*uid);
        }
        let probe = std::env::temp_dir().join(format!(".dromon-uid-{}-{}", std::process::id(), random_hex::<8>()?));
        let file = std::fs::File::create_new(&probe)?;
        let uid = file.metadata().map(|m| m.uid());
        drop(file);
        let _ = std::fs::remove_file(&probe);
        let uid = uid?;
        Ok(*UID.get_or_init(|| uid))
    }

    /// Creates `dir` with mode 0700, or accepts an existing one only if it is a real folder (not
    /// a symlink) owned by `uid`, tightening its mode to 0700 if needed.
    pub(super) fn ensure_private_dir(dir: &Path, uid: u32) -> io::Result<()> {
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let meta = std::fs::symlink_metadata(dir)?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "askpass folder is not a real folder"));
        }
        if meta.uid() != uid {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "askpass folder belongs to another user"));
        }
        if meta.mode() & 0o777 != 0o700 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_basics() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn tokens_are_64_hex_chars_and_unique() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b);
    }

    #[test]
    fn registry_lookup_and_remove() {
        let r = Registry::default();
        r.insert("t1".into(), HostId("a".into()));
        r.insert("t2".into(), HostId("b".into()));
        assert_eq!(r.lookup("t1"), Some(HostId("a".into())));
        assert_eq!(r.lookup("t2"), Some(HostId("b".into())));
        assert_eq!(r.lookup("t3"), None);
        assert_eq!(r.lookup(""), None);
        r.remove("t1");
        assert_eq!(r.lookup("t1"), None);
        assert_eq!(r.lookup("t2"), Some(HostId("b".into())));
    }

    #[tokio::test]
    async fn serve_connection_over_a_duplex_pipe() {
        let registry = Registry::default();
        registry.insert("good".into(), HostId("prod".into()));
        let (tx, mut rx) = mpsc::channel(4);
        let (mut client, server) = tokio::io::duplex(1024);
        let task = tokio::spawn(serve_connection(server, registry, tx));

        client.write_all(b"{\"token\":\"good\",\"prompt\":\"Password: \",\"hint\":null}\n").await.unwrap();
        let req = rx.recv().await.unwrap();
        assert_eq!(req.host, HostId("prod".into()));
        assert_eq!(req.prompt, "Password: ");
        assert_eq!(req.hint, None);
        req.reply.send(PromptAnswer::Text("pw".into())).unwrap();

        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"{\"answer\":\"pw\"}\n");
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn dropped_reply_sender_cancels() {
        let registry = Registry::default();
        registry.insert("good".into(), HostId("prod".into()));
        let (tx, mut rx) = mpsc::channel(4);
        let (mut client, server) = tokio::io::duplex(1024);
        let task = tokio::spawn(serve_connection(server, registry, tx));
        client.write_all(b"{\"token\":\"good\",\"prompt\":\"p\"}\n").await.unwrap();
        drop(rx.recv().await.unwrap());
        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"{\"cancel\":true}\n");
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn helper_hanging_up_closes_the_reply_channel() {
        let registry = Registry::default();
        registry.insert("good".into(), HostId("prod".into()));
        let (tx, mut rx) = mpsc::channel(4);
        let (mut client, server) = tokio::io::duplex(1024);
        let task = tokio::spawn(serve_connection(server, registry, tx));
        client.write_all(b"{\"token\":\"good\",\"prompt\":\"p\"}\n").await.unwrap();
        let req = rx.recv().await.unwrap();
        drop(client);
        task.await.unwrap().unwrap();
        assert!(req.reply.is_closed());
    }

    #[tokio::test]
    async fn unknown_token_and_garbage_are_not_forwarded() {
        let registry = Registry::default();
        registry.insert("good".into(), HostId("prod".into()));
        for input in [&b"{\"token\":\"bad\",\"prompt\":\"p\"}\n"[..], b"garbage\n", b""] {
            let (tx, mut rx) = mpsc::channel(4);
            let (mut client, server) = tokio::io::duplex(1024);
            let task = tokio::spawn(serve_connection(server, registry.clone(), tx));
            client.write_all(input).await.unwrap();
            client.shutdown().await.unwrap();
            assert!(task.await.unwrap().is_err());
            let mut out = Vec::new();
            client.read_to_end(&mut out).await.unwrap();
            assert!(out.is_empty());
            assert!(rx.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn oversized_request_is_rejected() {
        let registry = Registry::default();
        let (tx, mut rx) = mpsc::channel(4);
        let (mut client, server) = tokio::io::duplex(256 * 1024);
        let task = tokio::spawn(serve_connection(server, registry, tx));
        let mut big = vec![b'x'; super::super::protocol::MAX_MESSAGE_BYTES + 10];
        big.push(b'\n');
        client.write_all(&big).await.unwrap();
        assert!(task.await.unwrap().is_err());
        assert!(rx.try_recv().is_err());
    }
}
