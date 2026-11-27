use dromon_core::HostId;

use super::*;

fn askpass() -> AskpassEnv {
    AskpassEnv {
        exe: PathBuf::from("/opt/dromon/dromon"),
        endpoint: "/run/user/1000/dromon/a.sock".into(),
        token: "ab".repeat(32),
    }
}

fn saved(alias: &str) -> HostSpec {
    HostSpec {
        id: HostId(alias.into()),
        source: HostSource::Saved,
        alias: alias.into(),
        hostname: Some("10.0.0.4".into()),
        user: Some("deploy".into()),
        port: Some(2222),
        identity_file: Some("~/.ssh/id_deploy".into()),
        proxy_jump: Some("bastion".into()),
    }
}

fn build(host: &HostSpec, verbose: bool) -> Result<SshCommand, CommandError> {
    SshCommand::sftp_with(PathBuf::from("ssh"), host, askpass().vars_with_display(None), verbose)
}

fn args(cmd: &SshCommand) -> Vec<&str> {
    cmd.args.iter().map(|a| a.to_str().unwrap()).collect()
}

const BASE: [&str; 14] = [
    "-T",
    "-x",
    "-e",
    "none",
    "-o",
    "BatchMode=no",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=3",
    "-o",
    "ConnectTimeout=15",
    "-o",
    "ControlMaster=no",
];

#[test]
fn config_host_passes_only_the_alias() {
    let mut host = saved("prod");
    host.source = HostSource::SshConfig;
    let cmd = build(&host, false).unwrap();
    let mut expected = BASE.to_vec();
    expected.extend(["-s", "prod", "sftp"]);
    assert_eq!(args(&cmd), expected);
    assert_eq!(cmd.program, PathBuf::from("ssh"));
}

#[test]
fn saved_host_gets_explicit_options() {
    let cmd = build(&saved("box"), false).unwrap();
    let mut expected = BASE.to_vec();
    expected.extend([
        "-o",
        "HostName=10.0.0.4",
        "-o",
        "User=deploy",
        "-o",
        "Port=2222",
        "-o",
        "IdentityFile=~/.ssh/id_deploy",
        "-o",
        "ProxyJump=bastion",
        "-s",
        "box",
        "sftp",
    ]);
    assert_eq!(args(&cmd), expected);
}

#[test]
fn recent_host_with_only_some_fields() {
    let host = HostSpec {
        source: HostSource::Recent,
        identity_file: None,
        proxy_jump: None,
        user: None,
        ..saved("deploy@10.0.0.4:22")
    };
    let cmd = build(&host, true).unwrap();
    let mut expected = BASE.to_vec();
    expected.extend(["-o", "HostName=10.0.0.4", "-o", "Port=2222", "-v", "-s", "deploy@10.0.0.4:22", "sftp"]);
    assert_eq!(args(&cmd), expected);
}

/// A recent host typed on Windows: its key path has spaces and backslashes, its address is IPv6.
#[test]
fn windows_identity_and_ipv6_options() {
    let host = HostSpec {
        source: HostSource::Recent,
        hostname: Some("fe80::1%Ethernet 2".into()),
        identity_file: Some(r"C:\Users\John Doe\.ssh\id_ed25519".into()),
        proxy_jump: None,
        ..saved("fe80::1-Ethernet-2")
    };
    let cmd = build(&host, false).unwrap();
    let a = args(&cmd);
    assert_eq!(
        a[BASE.len()..],
        [
            "-o",
            "HostName=\"fe80::1%%Ethernet 2\"",
            "-o",
            "User=deploy",
            "-o",
            "Port=2222",
            "-o",
            r#"IdentityFile="C:\Users\John Doe\.ssh\id_ed25519""#,
            "-s",
            "fe80::1-Ethernet-2",
            "sftp"
        ]
    );
}

#[test]
fn verbose_goes_before_the_subsystem() {
    let host = HostSpec::from_config_alias("prod");
    let cmd = build(&host, true).unwrap();
    assert_eq!(args(&cmd)[BASE.len()..], ["-v", "-s", "prod", "sftp"]);
}

#[test]
fn config_file_goes_right_after_the_base_options() {
    let cmd = build(&saved("box"), true).unwrap().with_config_file(Path::new("/tmp/x y/ssh_config")).unwrap();
    let a = args(&cmd);
    assert_eq!(a[..BASE.len()], BASE);
    assert_eq!(a[BASE.len()..BASE.len() + 3], ["-F", "/tmp/x y/ssh_config", "-o"]);
    assert_eq!(a[a.len() - 4..], ["-v", "-s", "box", "sftp"]);
    assert_eq!(a.len(), BASE.len() + 2 + 10 + 4);
}

#[test]
fn config_file_injection_is_rejected() {
    for path in ["", "-oProxyCommand=sh", "/a\nProxyCommand sh", "/a\0b", "/a\rb"] {
        let err = build(&saved("box"), false).unwrap().with_config_file(Path::new(path)).unwrap_err();
        assert!(err.to_string().starts_with("The ssh config file"), "{path:?}: {err}");
    }
}

#[test]
fn env_removal_is_opt_in() {
    let cmd = build(&saved("box"), false).unwrap();
    assert!(cmd.env_remove.is_empty());
    let cmd = cmd.without_env("SSH_AUTH_SOCK");
    assert_eq!(cmd.env_remove, [OsString::from("SSH_AUTH_SOCK")]);
    assert!(format!("{cmd:?}").contains("env_remove: [\"SSH_AUTH_SOCK\"]"));
}

/// ssh reads the `-F` file and nothing else: a value from it shows in `ssh -G`.
#[cfg(unix)]
#[test]
fn real_ssh_reads_the_config_file() {
    let ssh = std::path::Path::new("/usr/bin/ssh");
    if !ssh.exists() {
        eprintln!("skipped: no /usr/bin/ssh");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("ssh_config");
    // ConnectionAttempts, not Compression: macOS's ssh has no zlib and prints "compression UNKNOWN".
    std::fs::write(&config, "Host *\n  ServerAliveCountMax 7\n  ConnectionAttempts 4\n").unwrap();
    let cmd = build(&saved("box"), false).unwrap().with_config_file(&config).unwrap();
    let out = std::process::Command::new(ssh).arg("-G").args(&cmd.args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.lines().any(|l| l == "connectionattempts 4"), "{stdout}");
    // The command line still wins over the file.
    assert!(stdout.lines().any(|l| l == "serveralivecountmax 3"), "{stdout}");
}

#[test]
fn alias_injection_is_rejected() {
    for alias in ["-oProxyCommand=sh", "-v", "", "prod\nProxyCommand x", "a\0b", "a\rb"] {
        let host = HostSpec::from_config_alias(alias);
        assert!(build(&host, false).is_err(), "{alias:?} should be rejected");
    }
}

type Mutation = fn(&mut HostSpec);

#[test]
fn option_value_injection_is_rejected() {
    let cases: [(&str, Mutation); 6] = [
        ("hostname dash", |h| h.hostname = Some("-oProxyCommand=sh".into())),
        ("user dash", |h| h.user = Some("-x".into())),
        ("identity newline", |h| h.identity_file = Some("/k\nProxyCommand sh".into())),
        ("jump dash", |h| h.proxy_jump = Some("-J evil".into())),
        ("hostname nul", |h| h.hostname = Some("a\0b".into())),
        ("empty user", |h| h.user = Some(String::new())),
    ];
    for (name, mutate) in cases {
        let mut host = saved("box");
        mutate(&mut host);
        assert!(build(&host, false).is_err(), "{name} should be rejected");
    }
}

#[test]
fn errors_are_readable() {
    let host = HostSpec::from_config_alias("-x");
    let err = build(&host, false).unwrap_err();
    assert_eq!(err.to_string(), "The host alias can't start with \"-\"");
    let core: dromon_core::Error = err.into();
    assert!(matches!(core, dromon_core::Error::Connect(_)));
}

#[test]
fn option_values_are_quoted_and_escaped() {
    let esc = Percent::Escape;
    assert_eq!(config_option("IdentityFile", "/a b/k", esc).unwrap(), "IdentityFile=\"/a b/k\"");
    assert_eq!(
        config_option("IdentityFile", r"C:\Users\John Doe\.ssh\id", esc).unwrap(),
        r#"IdentityFile="C:\Users\John Doe\.ssh\id""#
    );
    assert_eq!(config_option("IdentityFile", r"C:\keys\id", esc).unwrap(), r"IdentityFile=C:\keys\id");
    assert_eq!(config_option("IdentityFile", "/k#1", esc).unwrap(), "IdentityFile=\"/k#1\"");
    assert_eq!(config_option("IdentityFile", "/o'k", esc).unwrap(), "IdentityFile=\"/o'k\"");
    assert_eq!(config_option("IdentityFile", "/a=b", esc).unwrap(), "IdentityFile=\"/a=b\"");
    assert_eq!(config_option("IdentityFile", "/k$HOME", esc).unwrap(), "IdentityFile=/k$HOME");
    assert_eq!(config_option("HostName", "fe80::1%eth0", esc).unwrap(), "HostName=fe80::1%%eth0");
    assert_eq!(config_option("ProxyJump", "a%b", Percent::Literal).unwrap(), "ProxyJump=a%b");
    assert_eq!(config_option("User", "a b", Percent::Reject).unwrap(), "User=\"a b\"");
    assert!(config_option("User", "a%b", Percent::Reject).is_err());
    assert!(config_option("IdentityFile", "/a\"b", esc).is_err());
    assert!(config_option("IdentityFile", "${HOME}/k", esc).is_err());
    assert!(config_option("IdentityFile", r"\\server\share\id", esc).is_err());
    assert!(config_option("IdentityFile", r"C:\my dir\", esc).is_err());
    assert_eq!(
        config_option("IdentityFile", "/a\"b", esc).unwrap_err().to_string(),
        "IdentityFile contains characters ssh can't read back reliably"
    );
}

fn terminal(host: &HostSpec, folder: &str) -> Result<SshCommand, CommandError> {
    SshCommand::terminal_with(PathBuf::from("ssh"), host, folder)
}

/// What a POSIX shell reads back from `line`: `sh -c 'printf "%s\0" <line>'`, split on NUL.
#[cfg(unix)]
fn sh_words(line: &str) -> Vec<String> {
    let out = std::process::Command::new("/bin/sh").arg("-c").arg(format!("printf '%s\\0' {line}")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    text.split_terminator('\0').map(str::to_owned).collect()
}

#[test]
fn terminal_options_are_the_base_ones_with_a_terminal() {
    let mut expected = BASE.to_vec();
    expected[0] = "-t";
    assert_eq!(TERMINAL_ARGS, expected);
}

#[test]
fn terminal_for_a_config_host_passes_the_alias_and_a_cd() {
    let cmd = terminal(&HostSpec::from_config_alias("prod"), "/var/www/site").unwrap();
    let mut expected = TERMINAL_ARGS.to_vec();
    expected.extend(["prod", r#"cd -- '/var/www/site' && exec "$SHELL" -l || exec "$SHELL" -l"#]);
    assert_eq!(args(&cmd), expected);
    assert_eq!(cmd.program, PathBuf::from("ssh"));
    // No askpass: the terminal's ssh asks for itself, and nothing Dromon remembers goes along.
    assert!(cmd.env.is_empty() && cmd.env_remove.is_empty());
    assert!(!args(&cmd).iter().any(|a| *a == "-s" || *a == "sftp" || *a == "-T"));
}

#[test]
fn terminal_for_saved_quick_connect_and_jump_hosts_gets_their_options() {
    let cmd = terminal(&saved("box"), "/srv").unwrap();
    let a = args(&cmd);
    assert_eq!(a[..TERMINAL_ARGS.len()], *TERMINAL_ARGS);
    assert_eq!(
        a[TERMINAL_ARGS.len()..],
        [
            "-o",
            "HostName=10.0.0.4",
            "-o",
            "User=deploy",
            "-o",
            "Port=2222",
            "-o",
            "IdentityFile=~/.ssh/id_deploy",
            "-o",
            "ProxyJump=bastion",
            "box",
            r#"cd -- '/srv' && exec "$SHELL" -l || exec "$SHELL" -l"#
        ]
    );
    // A recent quick connect with a jump host and nothing else.
    let host = HostSpec {
        source: HostSource::Recent,
        user: None,
        port: None,
        identity_file: None,
        proxy_jump: Some("j@bastion:2200".into()),
        ..saved("10.0.0.4")
    };
    let cmd = terminal(&host, "/").unwrap();
    let a = &args(&cmd)[TERMINAL_ARGS.len()..];
    assert_eq!(a[..5], ["-o", "HostName=10.0.0.4", "-o", "ProxyJump=j@bastion:2200", "10.0.0.4"]);
}

#[test]
fn terminal_refuses_what_sftp_refuses() {
    for alias in ["-oProxyCommand=sh", "", "a\nb"] {
        assert!(terminal(&HostSpec::from_config_alias(alias), "/").is_err(), "{alias:?}");
    }
    let host = HostSpec { proxy_jump: Some("-J evil".into()), ..saved("box") };
    assert!(terminal(&host, "/").is_err());
}

#[test]
fn a_windows_servers_folder_opens_ssh_without_a_cd() {
    for folder in ["/C:/Users/deploy", "/C:", "/d:/data"] {
        let cmd = terminal(&HostSpec::from_config_alias("win"), folder).unwrap();
        assert_eq!(args(&cmd).last(), Some(&"win"), "{folder}");
    }
    // Look-alikes are POSIX paths.
    for folder in ["/C:x", "/CC:/a", "C:/a", "/1:/a"] {
        let cmd = terminal(&HostSpec::from_config_alias("box"), folder).unwrap();
        assert!(args(&cmd).last().unwrap().starts_with("cd -- "), "{folder}");
    }
}

#[test]
fn quoting_keeps_every_awkward_path_one_word() {
    assert_eq!(quote("/a b"), "'/a b'");
    assert_eq!(quote("/it's"), r"'/it'\''s'");
    assert_eq!(quote("/$HOME/`id`"), "'/$HOME/`id`'");
    assert_eq!(quote("-rf"), "'-rf'");
    assert_eq!(quote(""), "''");
    assert_eq!(quote("/a\nb"), "'/a\nb'");
    assert_eq!(quote(r#"/"x"\y"#), r#"'/"x"\y'"#);
    assert_eq!(quote("/çağrı ğ 漢字"), "'/çağrı ğ 漢字'");
}

/// A real `sh` reads each awkward path back as it was, and the remote command `cd`s into it.
#[cfg(unix)]
#[test]
fn a_real_shell_reads_quoted_paths_back() {
    let awkward = [
        "/srv/with space",
        "/srv/it's 'quoted'",
        r#"/srv/"double" \back\slash"#,
        "/srv/$HOME ${x} $(id)",
        "/srv/`id`",
        "/srv/line\nbreak",
        "-rf",
        "/srv/;&|<>*?~#!",
        "/srv/çağrı 漢字 🚀",
    ];
    for path in awkward {
        assert_eq!(sh_words(&quote(path)), [path], "{path:?}");
    }
    // Nothing inside a quoted path runs.
    let dir = tempfile::tempdir().unwrap();
    let canary = dir.path().join("ran");
    let inject = format!("/x`touch {0}`$(touch {0})';touch {0};'", canary.display());
    assert_eq!(sh_words(&quote(&inject)), [inject.as_str()]);
    assert!(!canary.exists(), "nothing in a path runs");
    // The remote command, run by a shell: it lands in the folder (made here for the test).
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("it's a $dir `x`\nline");
    std::fs::create_dir(&folder).unwrap();
    let folder = folder.to_str().unwrap();
    let cmd = terminal(&HostSpec::from_config_alias("box"), folder).unwrap();
    let remote = args(&cmd).last().unwrap().replace(r#"exec "$SHELL" -l"#, "pwd");
    let out = std::process::Command::new("/bin/sh").arg("-c").arg(&remote).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n'), folder);
    // A folder that's gone: the shell says so, and the second command still runs (at home).
    let cmd = terminal(&HostSpec::from_config_alias("box"), "/no/such/folder").unwrap();
    let remote = args(&cmd).last().unwrap().replace(r#"exec "$SHELL" -l"#, "echo started");
    let out = std::process::Command::new("/bin/sh").arg("-c").arg(&remote).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "started\n");
    assert!(!out.stderr.is_empty(), "cd's own message shows");
}

/// The whole command as a shell line reads back as the same argument vector.
#[cfg(unix)]
#[test]
fn the_shell_line_reads_back_as_the_argument_vector() {
    let host = HostSpec { user: Some("first last".into()), ..saved("box") };
    let cmd = terminal(&host, "/srv/it's here").unwrap();
    let line = cmd.shell_line();
    let mut expected = vec!["ssh".to_owned()];
    expected.extend(args(&cmd).into_iter().map(str::to_owned));
    assert_eq!(sh_words(&line), expected);
    assert!(line.starts_with("'ssh' '-t' "), "{line}");
}

#[test]
fn askpass_env_sets_everything() {
    let env = askpass().vars_with_display(None);
    let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.to_str().unwrap());
    assert_eq!(get("SSH_ASKPASS"), Some("/opt/dromon/dromon"));
    assert_eq!(get("SSH_ASKPASS_REQUIRE"), Some("force"));
    assert_eq!(get("DISPLAY"), Some("dromon:0"));
    assert_eq!(get("DROMON_ASKPASS_ENDPOINT"), Some("/run/user/1000/dromon/a.sock"));
    assert_eq!(get("DROMON_ASKPASS_TOKEN"), Some("ab".repeat(32).as_str()));
    assert_eq!(env.len(), 5);
}

#[test]
fn display_is_kept_when_set_and_replaced_when_empty() {
    let set = askpass().vars_with_display(Some(OsStr::new(":1")));
    assert!(!set.iter().any(|(k, _)| k == "DISPLAY"));
    let empty = askpass().vars_with_display(Some(OsStr::new("")));
    assert!(empty.iter().any(|(k, v)| k == "DISPLAY" && v == FALLBACK_DISPLAY));
}

#[test]
fn debug_never_prints_the_token() {
    let token = askpass().token;
    assert!(!format!("{:?}", askpass()).contains(&token));
    let cmd = build(&saved("box"), false).unwrap();
    assert!(!format!("{cmd:?}").contains(&token));
}

#[cfg(not(windows))]
#[test]
fn ssh_program_is_plain_ssh() {
    assert_eq!(ssh_program(), PathBuf::from("ssh"));
}

fn build_watch(host: &HostSpec, folder: &str) -> Result<SshCommand, CommandError> {
    SshCommand::watch_with(PathBuf::from("ssh"), host, askpass().vars_with_display(None), folder)
}

#[test]
fn a_watch_runs_a_remote_command_with_the_host_options_and_askpass_of_the_session() {
    let cmd = build_watch(&saved("box"), "/srv/www").unwrap();
    let sftp = build(&saved("box"), false).unwrap();
    let a = args(&cmd);
    // The session's options (base, `-T`, `ControlMaster=no`, the host's -o), then the alias.
    assert_eq!(a[..a.len() - 3], args(&sftp)[..sftp.args.len() - 3]);
    assert!(a.contains(&"-T") && a.contains(&"ControlMaster=no"));
    assert_eq!(a[a.len() - 3..a.len() - 1], ["box", "--"]);
    assert!(!a.contains(&"-s") && !a.contains(&"sftp"), "{a:?}");
    assert_eq!(
        a[a.len() - 1],
        "command -v inotifywait >/dev/null 2>&1 || { echo dromon-watch-missing; exit 0; }; echo dromon-watch-ready; \
         inotifywait -m -q -e create,delete,move,modify,attrib,close_write --format %e -- '/srv/www' & w=$!; \
         exec 3<&0; { read _ <&3; kill $w; } >/dev/null 2>&1 & r=$!; wait $w; kill $r 2>/dev/null"
    );
    assert_eq!(cmd.env, sftp.env, "the same askpass");
    // A config host passes only its alias, as for the session; -F goes after the base options.
    let cmd = build_watch(&HostSpec::from_config_alias("prod"), "/").unwrap();
    let cmd = cmd.with_config_file(Path::new("/tmp/ssh_config")).unwrap();
    let a = args(&cmd);
    assert_eq!(a[..BASE.len()], BASE);
    assert_eq!(a[BASE.len()..a.len() - 1], ["-F", "/tmp/ssh_config", "prod", "--"]);
}

#[test]
fn a_watch_refuses_what_the_session_refuses_and_a_folder_it_cant_pass() {
    assert!(build_watch(&HostSpec::from_config_alias("-oProxyCommand=sh"), "/a").is_err());
    let mut host = saved("box");
    host.user = Some("-x".into());
    assert!(build_watch(&host, "/a").is_err());
    assert_eq!(build_watch(&saved("box"), "").unwrap_err(), CommandError::Empty("The folder"));
    assert_eq!(build_watch(&saved("box"), "/a\0b").unwrap_err(), CommandError::Unquotable("The folder"));
}

#[test]
fn a_watch_quotes_its_folder_whatever_it_holds() {
    for folder in ["/srv/my site", "/it's", "/''", "/\"q\"", "/$HOME/${x}", "/`rm -rf ~`", "/$(id)", "/a\nb", "-rf"] {
        let quoted = quote(folder);
        assert!(watch_script(folder).unwrap().contains(&format!(" -- {quoted} & w=$!;")), "{folder:?}");
    }
}

/// The watch's command in a real shell, with a fake `inotifywait` (`body`) on its `PATH`, its
/// input and output one socket, as sshd may give a session; its input kept open. Returns its
/// output lines up to the end of its output (`None`: the output didn't end within 2 s), and
/// whether the command ended within 5 s once its input closed then.
#[cfg(unix)]
fn run_watch(dir: &Path, body: &str) -> (Option<Vec<String>>, bool) {
    use std::io::{BufRead, BufReader};
    use std::os::fd::OwnedFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};
    let fake = dir.join("inotifywait");
    std::fs::write(&fake, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (ours, theirs) = UnixStream::pair().unwrap();
    let theirs_out = theirs.try_clone().unwrap();
    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(watch_script("/srv/my site").unwrap())
        .env("PATH", format!("{}:/usr/bin:/bin", dir.display()))
        .stdin(OwnedFd::from(theirs))
        .stdout(OwnedFd::from(theirs_out))
        .spawn()
        .unwrap();
    let reader = ours.try_clone().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(BufReader::new(reader).lines().map_while(Result::ok).collect::<Vec<_>>());
    });
    let out = rx.recv_timeout(Duration::from_secs(2)).ok();
    // macOS refuses to shut down a socket whose other end is closed already (the command ended):
    // its input is closed either way.
    match ours.shutdown(std::net::Shutdown::Write) {
        Err(e) if e.kind() == std::io::ErrorKind::NotConnected => {}
        r => r.unwrap(),
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let ended = child.try_wait().unwrap().is_some();
    let _ = child.kill();
    (out, ended)
}

/// When ssh goes (the command's input ends), the server's `inotifywait` goes too; when
/// `inotifywait` ends, the output does, so the app hears the watch is over.
#[cfg(unix)]
#[test]
fn a_watch_ends_with_its_input_and_its_output_with_inotifywait() {
    let dir = tempfile::tempdir().unwrap();
    let pid = dir.path().join("pid");
    // An inotifywait that runs until stopped: the output stays open while the input does.
    let (out, ended) = run_watch(dir.path(), &format!("echo $$ > '{}'; exec sleep 600", pid.display()));
    assert_eq!(out, None, "watching: the output stays open");
    assert!(ended, "the input closed: the command ended");
    let pid = std::fs::read_to_string(&pid).unwrap();
    let alive = std::process::Command::new("kill").args(["-0", pid.trim()]).status().unwrap().success();
    assert!(!alive, "and inotifywait with it");

    let (out, ended) = run_watch(dir.path(), "echo \"args: $*\"; echo CREATE");
    let args = "args: -m -q -e create,delete,move,modify,attrib,close_write --format %e -- /srv/my site";
    assert_eq!(out.as_deref(), Some(&[WATCH_READY, args, "CREATE"].map(String::from)[..]), "its input still open");
    assert!(ended);
    std::fs::remove_file(dir.path().join("inotifywait")).unwrap();
    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(watch_script("/x").unwrap())
        .env("PATH", dir.path())
        .output()
        .unwrap();
    child.stdout.retain(|b| *b != b'\n');
    assert_eq!(String::from_utf8(child.stdout).unwrap(), WATCH_MISSING, "no inotifywait on the server");
}

/// Feeds our options to the real `ssh -G` (no network, empty config) and checks it reads back
/// exactly the values we meant.
#[cfg(unix)]
#[test]
fn real_ssh_reads_our_options_back() {
    let ssh = std::path::Path::new("/usr/bin/ssh");
    if !ssh.exists() {
        eprintln!("skipped: no /usr/bin/ssh");
        return;
    }
    // ssh normalizes a literal address through getaddrinfo, which drops a zone naming an
    // interface the machine lacks; loopback is there everywhere (lo0 on macOS).
    let zoned = if cfg!(target_os = "macos") { "fe80::1%lo0" } else { "fe80::1%lo" };
    let host = HostSpec {
        hostname: Some(zoned.into()),
        user: Some("first last=#".into()),
        identity_file: Some("/tmp/o'k keys/id#1=x".into()),
        proxy_jump: Some("j@b:2200,c".into()),
        ..saved("box")
    };
    let cmd = build(&host, false).unwrap();
    let out = std::process::Command::new(ssh).args(["-F", "/dev/null", "-G"]).args(&cmd.args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let r = crate::parse_ssh_g(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(r.hostname.as_deref(), Some(zoned));
    assert_eq!(r.user.as_deref(), Some("first last=#"));
    assert_eq!(r.port, Some(2222));
    assert_eq!(r.proxy_jump.as_deref(), Some("j@b:2200,c"));
    // -G prints IdentityFile before % expansion, so the doubled % shows; there is none here.
    assert!(r.identity_files.iter().any(|f| f == "/tmp/o'k keys/id#1=x"), "{:?}", r.identity_files);
}
