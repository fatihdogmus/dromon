//! The ssh child process: stdin/stdout as one stream for the SFTP client, stderr as log lines.

use std::io;
use std::process::{ExitStatus, Stdio};

use tokio::io::{AsyncBufReadExt, BufReader, Join};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::mpsc;

use crate::command::SshCommand;

/// The SFTP byte stream: reads ssh's stdout, writes its stdin.
pub type SshStream = Join<ChildStdout, ChildStdin>;

/// How many stderr lines may wait for the reader. Further lines are dropped rather than letting
/// ssh block on a full pipe.
pub const STDERR_BUFFER_LINES: usize = 1024;

/// Windows `CREATE_NO_WINDOW`: no console window flashes up for ssh.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A running ssh. Dropping it kills the process.
#[derive(Debug)]
pub struct SshChild {
    child: Child,
    stream: Option<SshStream>,
    stderr: Option<mpsc::Receiver<String>>,
}

/// Starts `cmd` with piped stdio: the inherited environment minus `cmd.env_remove`, plus
/// `cmd.env`. Must be called inside a tokio runtime: a task forwards stderr lines (lossily
/// decoded, line endings stripped) until ssh closes it.
pub async fn spawn(cmd: &SshCommand) -> io::Result<SshChild> {
    let mut command = tokio::process::Command::new(&cmd.program);
    for name in &cmd.env_remove {
        command.env_remove(name);
    }
    command
        .args(&cmd.args)
        .envs(cmd.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = spawn_retrying_busy(&mut command).await?;
    let stdin = child.stdin.take().ok_or_else(|| io::Error::other("ssh stdin not piped"))?;
    let stdout = child.stdout.take().ok_or_else(|| io::Error::other("ssh stdout not piped"))?;
    let stderr = child.stderr.take().ok_or_else(|| io::Error::other("ssh stderr not piped"))?;

    let (tx, rx) = mpsc::channel(STDERR_BUFFER_LINES);
    tokio::spawn(forward_stderr(stderr, tx));
    Ok(SshChild { child, stream: Some(tokio::io::join(stdout, stdin)), stderr: Some(rx) })
}

/// How often a spawn that finds the program "busy" is tried again, and the first wait (doubling).
const BUSY_TRIES: u32 = 5;
const BUSY_WAIT: std::time::Duration = std::time::Duration::from_millis(10);

/// `command.spawn()`, tried again a few times when the program is briefly "busy" (ETXTBSY): its
/// file was just written (a package update replacing ssh; the test suite's fake ssh) and a child
/// another thread forked meanwhile still holds the write descriptor until it execs. Any other
/// error, or the last busy one, is returned as is.
async fn spawn_retrying_busy(command: &mut tokio::process::Command) -> io::Result<Child> {
    let mut wait = BUSY_WAIT;
    for _ in 1..BUSY_TRIES {
        match command.spawn() {
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy => {
                tokio::time::sleep(wait).await;
                wait *= 2;
            }
            other => return other,
        }
    }
    command.spawn()
}

async fn forward_stderr(stderr: ChildStderr, tx: mpsc::Sender<String>) {
    let mut reader = BufReader::new(stderr);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        while matches!(buf.last(), Some(b'\n' | b'\r')) {
            buf.pop();
        }
        // Keep draining even if nobody listens, so ssh never blocks on stderr.
        let _ = tx.try_send(String::from_utf8_lossy(&buf).into_owned());
    }
}

impl SshChild {
    /// The OS process id, while it runs.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// The SFTP stream, for `russh_sftp::client::SftpSession::new`. `None` after the first call.
    pub fn take_stream(&mut self) -> Option<SshStream> {
        self.stream.take()
    }

    /// ssh's stderr, one line per message. `None` after the first call.
    pub fn take_stderr(&mut self) -> Option<mpsc::Receiver<String>> {
        self.stderr.take()
    }

    /// Waits for ssh to exit. Closes our end of stdin first if the stream was never taken.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.stream = None;
        self.child.wait().await
    }

    /// The exit status if ssh has already exited.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Kills ssh and waits for it to exit.
    pub async fn kill(&mut self) -> io::Result<()> {
        self.child.kill().await
    }
}

#[cfg(all(test, unix))]
#[path = "process_tests.rs"]
mod tests;
