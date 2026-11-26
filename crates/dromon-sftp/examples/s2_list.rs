//! Spike S2: the system `ssh -s <host> sftp` child's stdio driving russh-sftp, listing a folder.
//!
//! ```text
//! cargo run -p dromon-sftp --example s2_list -- <destination> [path] [-- <extra ssh options…>]
//! cargo run -p dromon-sftp --example s2_list -- myhost /
//! cargo run -p dromon-sftp --example s2_list -- key@127.0.0.1 fixture/mixed -- \
//!     -p <port> -i <key folder>/id_ed25519 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
//! ```
//!
//! Against the test fixture, `DROMON_FIXTURE_KEEP=1 cargo test -p dromon-sftp --test fixture`
//! prints the port and key folder to use (see `docs/test-servers.md`).
//!
//! `path` defaults to the login folder; a relative path is taken relative to it. ssh's prompts
//! (passphrase, host key) go to the terminal: stderr and the tty are left alone.

use std::process::{ExitCode, Stdio};
use std::time::Instant;

use dromon_core::{EntryKind, LinkTarget, SortDir, SortKey, sort_entries};
use dromon_sftp::Sftp;
use tokio::process::Command;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (ours, ssh_extra) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (&args[..], &[][..]),
    };
    let (dest, path) = match ours {
        [dest] => (dest, None),
        [dest, path] => (dest, Some(path.as_str())),
        _ => {
            eprintln!("usage: s2_list <destination> [path] [-- <extra ssh options…>]");
            return ExitCode::from(2);
        }
    };

    let start = Instant::now();
    let mut child = match Command::new("ssh")
        .args(["-T", "-x", "-e", "none", "-o", "BatchMode=no"])
        .args(["-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3"])
        .args(["-o", "ConnectTimeout=15", "-o", "ControlMaster=no"])
        .args(ssh_extra)
        .args(["-s", dest, "sftp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("couldn't start ssh: {e}");
            return ExitCode::FAILURE;
        }
    };
    let stream = tokio::io::join(child.stdout.take().unwrap(), child.stdin.take().unwrap());
    let sftp = match Sftp::new(stream).await {
        Ok(sftp) => sftp,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let connected = start.elapsed();

    let caps = sftp.capabilities();
    let home = match sftp.home().await {
        Ok(home) => home,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let dir = match path {
        None => home.clone(),
        Some(p) => home.join(p),
    };
    let listed = Instant::now();
    let mut entries = match sftp.list(&dir).await {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let listed = listed.elapsed();
    sort_entries(&mut entries, SortKey::Name, SortDir::Ascending);

    println!("connected in {connected:.0?}; home {home}");
    println!(
        "server: posix-rename {}, copy-data {}, chunk {} bytes",
        caps.posix_rename,
        caps.copy_data,
        sftp.chunk_size()
    );
    println!("{dir}: {} entries in {listed:.0?}", entries.len());
    for e in &entries {
        let kind = match e.kind {
            EntryKind::Dir => "dir ",
            EntryKind::File => "file",
            EntryKind::Symlink { target: Some(LinkTarget::Dir) } => "l>d ",
            EntryKind::Symlink { target: Some(LinkTarget::File) } => "l>f ",
            EntryKind::Symlink { target: Some(LinkTarget::Other) } => "l>o ",
            EntryKind::Symlink { target: None } => "l>? ",
            EntryKind::Other => "othr",
        };
        let mode = e.mode.map_or_else(|| "    ".into(), |m| format!("{:04o}", m & 0o7777));
        let mtime = e.mtime.map_or_else(|| "-".into(), |t| t.to_string());
        println!("{kind} {mode} {:>12} {mtime:>11} {}", e.size, e.name);
    }
    sftp.close();
    let _ = child.wait().await;
    ExitCode::SUCCESS
}
