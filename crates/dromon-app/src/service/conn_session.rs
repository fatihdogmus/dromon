//! One connection attempt of a host's task (`conn.rs`): a fresh askpass token and
//! `AuthAttempt`, the ssh child and its stderr, the SFTP handshake on its stdio and then the
//! session, and the folder listing in flight. Dropping a [`Session`] unregisters the token,
//! cancels its open prompts and kills ssh. The task decides what each [`SessionEvent`] means.

use std::collections::VecDeque;
use std::future::pending;
use std::io;
use std::process::ExitStatus;
use std::sync::Arc;

use dromon_core::{Entry, Error, HostSpec, PromptId, RemotePath};
use dromon_sftp::Sftp;
use dromon_ssh::{AskpassEnv, AskpassServer, SshCommand, spawn};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinError, JoinHandle};
use tokio::time::Instant;
use zeroize::Zeroizing;

use super::super::auth::AttemptPrompts;
use super::super::poll::Collector;
use super::{PAGE, Shared};

/// stderr lines kept per attempt for reading why ssh exited.
const TAIL: usize = 64;

/// A folder being listed: for the UI (pages go on as they come), or a poll (pages collect
/// quietly and the whole is compared with what the UI has). One per host at a time.
pub(super) struct ListRun {
    pub(super) path: RemotePath,
    pages: mpsc::Receiver<Result<Vec<Entry>, Error>>,
    pub(super) first: bool,
    pub(super) poll: bool,
    pub(super) started: Instant,
    pub(super) got: Collector,
}

impl ListRun {
    pub(super) fn new(sftp: &Sftp, path: RemotePath, poll: bool) -> ListRun {
        ListRun { pages: sftp.list_paged(&path, PAGE), path, first: true, poll, started: Instant::now(), got: Collector::default() }
    }
}

/// One connection attempt.
pub(super) struct Session {
    askpass: Option<Arc<AskpassServer>>,
    token: Zeroizing<String>,
    pub(super) prompts: AttemptPrompts,
    pub(super) stderr: Option<mpsc::Receiver<String>>,
    pub(super) tail: VecDeque<String>,
    /// Resolves when ssh exits; dropping `kill` kills it.
    pub(super) exit: Option<JoinHandle<io::Result<ExitStatus>>>,
    _kill: oneshot::Sender<()>,
    pub(super) handshake: Option<JoinHandle<Result<(Sftp, RemotePath), Error>>>,
    pub(super) sftp: Option<Sftp>,
    pub(super) home: RemotePath,
    pub(super) listing: Option<ListRun>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(server) = &self.askpass {
            server.unregister(&self.token);
        }
        if let Some(h) = &self.handshake {
            h.abort();
        }
        if let Some(sftp) = &self.sftp {
            sftp.close();
        }
    }
}

pub(super) enum SessionEvent {
    Stderr(String),
    StderrClosed,
    Exited(Result<io::Result<ExitStatus>, JoinError>),
    Up(Result<Result<(Sftp, RemotePath), Error>, JoinError>),
    HungUp(PromptId),
    Page(Option<Result<Vec<Entry>, Error>>),
}

async fn recv_opt<T>(rx: &mut Option<mpsc::Receiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => pending().await,
    }
}

async fn join_opt<T>(h: &mut Option<JoinHandle<T>>) -> Result<T, JoinError> {
    match h {
        Some(h) => h.await,
        None => pending().await,
    }
}

async fn page_opt(run: &mut Option<ListRun>) -> Option<Result<Vec<Entry>, Error>> {
    match run {
        Some(run) => run.pages.recv().await,
        None => pending().await,
    }
}

impl Session {
    /// Starts ssh for `spec` (with `-v` if `verbose`) with a fresh token, and the SFTP handshake
    /// on its stdio.
    pub(super) async fn start(spec: &HostSpec, shared: &Shared, verbose: bool) -> Result<(Session, String), String> {
        let cfg = &shared.cfg;
        let (env, askpass) = match &shared.askpass {
            Some(server) => {
                let env = server.register(spec.id.clone(), &cfg.helper_exe).map_err(|e| format!("Couldn't set up sign-in prompts: {e}"))?;
                (env, Some(server.clone()))
            }
            // No askpass endpoint: ssh's prompts are cancelled, keys and agents still work.
            None => (AskpassEnv { exe: cfg.helper_exe.clone(), endpoint: String::new(), token: String::new() }, None),
        };
        let token = Zeroizing::new(env.token.clone());
        let unregister = |askpass: &Option<Arc<AskpassServer>>| {
            if let Some(server) = askpass {
                server.unregister(&token);
            }
        };
        let built = SshCommand::sftp(spec, &env, verbose).and_then(|cmd| match &cfg.config_file {
            Some(file) => cmd.with_config_file(file),
            None => Ok(cmd),
        });
        let mut cmd = match built {
            Ok(cmd) => cmd,
            Err(e) => {
                unregister(&askpass);
                return Err(e.to_string());
            }
        };
        cmd.program = cfg.ssh.clone();
        for name in &cfg.without_env {
            cmd = cmd.without_env(name.clone());
        }
        let line = std::iter::once(cmd.program.to_string_lossy().into_owned())
            .chain(cmd.args.iter().map(|a| a.to_string_lossy().into_owned()))
            .collect::<Vec<_>>()
            .join(" ");
        let mut child = match spawn(&cmd).await {
            Ok(child) => child,
            Err(e) => {
                unregister(&askpass);
                return Err(format!("Couldn't run {}: {e}", cmd.program.display()));
            }
        };
        let stream = child.take_stream();
        let stderr = child.take_stderr();
        let (kill, killed) = oneshot::channel::<()>();
        let exit = tokio::spawn(async move {
            tokio::select! {
                status = child.wait() => status,
                _ = killed => {
                    child.kill().await?;
                    Err(io::Error::other("stopped"))
                }
            }
        });
        let handshake = stream.map(|stream| {
            tokio::spawn(async move {
                let sftp = Sftp::new(stream).await?;
                let home = sftp.home().await?;
                Ok((sftp, home))
            })
        });
        let session = Session {
            askpass,
            token,
            prompts: AttemptPrompts::new(spec.id.clone(), shared.ids.clone()),
            stderr,
            tail: VecDeque::new(),
            exit: Some(exit),
            _kill: kill,
            handshake,
            sftp: None,
            home: RemotePath::root(),
            listing: None,
        };
        Ok((session, format!("Running {line}")))
    }

    async fn next(&mut self) -> SessionEvent {
        tokio::select! {
            line = recv_opt(&mut self.stderr) => match line {
                Some(line) => SessionEvent::Stderr(line),
                None => SessionEvent::StderrClosed,
            },
            status = join_opt(&mut self.exit) => SessionEvent::Exited(status),
            up = join_opt(&mut self.handshake) => SessionEvent::Up(up),
            id = self.prompts.hung_up() => SessionEvent::HungUp(id),
            page = page_opt(&mut self.listing) => SessionEvent::Page(page),
        }
    }

    pub(super) fn keep(&mut self, line: &str) {
        if self.tail.len() == TAIL {
            self.tail.pop_front();
        }
        self.tail.push_back(line.to_owned());
    }
}

pub(super) async fn session_next(s: &mut Option<Session>) -> SessionEvent {
    match s {
        Some(s) => s.next().await,
        None => pending().await,
    }
}
