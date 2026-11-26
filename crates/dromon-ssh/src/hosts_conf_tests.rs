use super::*;
use crate::quick::{QuickInput, quick_host};

fn saved(alias: &str) -> HostSpec {
    HostSpec {
        id: HostId(alias.into()),
        source: HostSource::Saved,
        alias: alias.into(),
        hostname: Some("10.0.0.4".into()),
        user: Some("deploy".into()),
        port: Some(2222),
        identity_file: Some("~/.ssh/id_box".into()),
        proxy_jump: Some("bastion".into()),
    }
}

#[test]
fn a_block_names_every_setting_it_has() {
    assert_eq!(
        host_block(&saved("box")).unwrap(),
        "Host box\n    HostName 10.0.0.4\n    User deploy\n    Port 2222\n    IdentityFile ~/.ssh/id_box\n    ProxyJump bastion\n"
    );
    let bare = HostSpec { user: None, port: None, identity_file: None, proxy_jump: None, ..saved("pi") };
    assert_eq!(host_block(&bare).unwrap(), "Host pi\n    HostName 10.0.0.4\n");
}

#[test]
fn block_values_are_quoted_and_escaped_like_options() {
    let spec = HostSpec {
        hostname: Some("fe80::1%eth0".into()),
        identity_file: Some(r"C:\Users\John Doe\.ssh\id".into()),
        ..saved("box")
    };
    let block = host_block(&spec).unwrap();
    assert!(block.contains("    HostName fe80::1%%eth0\n"), "{block}");
    assert!(block.contains("    IdentityFile \"C:\\Users\\John Doe\\.ssh\\id\"\n"), "{block}");
    for bad in [
        HostSpec { identity_file: Some("/a\"b".into()), ..saved("box") },
        HostSpec { user: Some("a%b".into()), ..saved("box") },
        HostSpec { hostname: Some("-oProxyCommand=sh".into()), ..saved("box") },
        HostSpec { alias: "my box".into(), ..saved("box") },
        HostSpec { alias: "*".into(), ..saved("box") },
        HostSpec { alias: "-x".into(), ..saved("box") },
        HostSpec { alias: "a\nHost b".into(), ..saved("box") },
    ] {
        assert!(host_block(&bad).is_err(), "{bad:?}");
    }
}

/// The real `ssh -G` reads a block the way we meant it.
#[cfg(unix)]
#[test]
fn real_ssh_reads_a_block() {
    let ssh = Path::new("/usr/bin/ssh");
    if !ssh.exists() {
        eprintln!("skipped: no /usr/bin/ssh");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let spec = HostSpec {
        user: Some("first.last".into()),
        identity_file: Some("/tmp/o'k keys/id#1=x".into()),
        proxy_jump: Some("j@b:2200,c".into()),
        ..saved("box")
    };
    let config = dir.path().join("config");
    fs::write(&config, appended(b"Host other\n    User nobody\n", &host_block(&spec).unwrap())).unwrap();
    let out = std::process::Command::new(ssh).arg("-F").arg(&config).args(["-G", "box"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let r = crate::parse_ssh_g(&String::from_utf8_lossy(&out.stdout));
    assert_eq!((r.hostname.as_deref(), r.user.as_deref(), r.port), (Some("10.0.0.4"), Some("first.last"), Some(2222)));
    assert_eq!(r.proxy_jump.as_deref(), Some("j@b:2200,c"));
    assert!(r.identity_files.iter().any(|f| f == "/tmp/o'k keys/id#1=x"), "{:?}", r.identity_files);
}

#[test]
fn hosts_conf_reads_back_what_it_wrote() {
    let odd = HostSpec {
        hostname: Some("fe80::1%eth0".into()),
        user: Some("first.last".into()),
        identity_file: Some("/keys/o'k dir/id#1=x%".into()),
        proxy_jump: Some("j@b:2200,c".into()),
        ..saved("odd")
    };
    let bare = HostSpec { user: None, port: None, identity_file: None, proxy_jump: None, ..saved("pi") };
    let specs = vec![saved("box"), odd, bare];
    let text = render_hosts_conf(&specs).unwrap();
    assert!(text.starts_with("# Dromon's saved hosts"), "{text}");
    assert_eq!(parse_hosts_conf(&text), specs);
}

#[test]
fn parsing_skips_what_isnt_a_saved_host() {
    let text = "\u{feff}# hand edited\nHost a b\n  HostName x\nHost *\n  User nobody\nHost ok\n  hostname = 1.2.3.4\n  PORT 0\n  Port nope\n  \
                ForwardAgent yes\n  User\nMatch all\n  User leaked\nHost ok\n  HostName second\nHost good\n  User \"a b\"\n";
    let got = parse_hosts_conf(text);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(
        (got[0].alias.as_str(), got[0].hostname.as_deref(), got[0].port, got[0].user.as_deref()),
        ("ok", Some("1.2.3.4"), None, None)
    );
    assert_eq!((got[1].id.as_str(), got[1].user.as_deref(), got[1].source), ("good", Some("a b"), HostSource::Saved));
}

#[test]
fn quick_hosts_always_make_a_block() {
    let i = QuickInput {
        alias: "lab".into(),
        identity_file: "/k/id ed".into(),
        ..QuickInput { host: "u@h:2".into(), ..Default::default() }
    };
    let spec = quick_host(&i, HostSource::Saved).unwrap();
    let block = host_block(&spec).unwrap();
    assert_eq!(parse_hosts_conf(&block), [spec]);
}

#[test]
fn writing_hosts_conf_is_atomic_and_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dromon").join(HOSTS_CONF);
    assert_eq!(read_hosts_conf(&path).unwrap(), []);
    write_hosts_conf(&path, &[saved("box")]).unwrap();
    assert_eq!(read_hosts_conf(&path).unwrap(), [saved("box")]);
    write_hosts_conf(&path, &[saved("a"), saved("b")]).unwrap();
    assert_eq!(read_hosts_conf(&path).unwrap(), [saved("a"), saved("b")]);
    let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leftovers, [std::ffi::OsString::from(HOSTS_CONF)], "no temporary files left");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }
    // A host ssh couldn't read back is refused, and the file stays as it was.
    let bad = HostSpec { user: Some("a%b".into()), ..saved("c") };
    assert!(write_hosts_conf(&path, &[saved("a"), bad]).is_err());
    assert_eq!(read_hosts_conf(&path).unwrap(), [saved("a"), saved("b")]);
}

#[test]
fn backups_never_overwrite_and_count_up() {
    let p = Path::new("/home/u/.ssh/config");
    assert_eq!(backup_name(p, "20261125-1342", 1), Path::new("/home/u/.ssh/config.dromon-backup-20261125-1342"));
    assert_eq!(backup_name(p, "20261125-1342", 2), Path::new("/home/u/.ssh/config.dromon-backup-20261125-1342-2"));
    assert_eq!(backup_stamp(2026, 11, 5, 9, 3), "20261105-0903");
}

#[test]
fn appending_keeps_the_file_and_adds_the_block_after_a_blank_line() {
    let block = "Host box\n    HostName h\n";
    assert_eq!(appended(b"", block), b"Host box\n    HostName h\n");
    assert_eq!(appended(b"Host a\n", block), b"Host a\n\nHost box\n    HostName h\n");
    assert_eq!(appended(b"Host a", block), b"Host a\n\nHost box\n    HostName h\n");
    assert_eq!(appended(b"Host a\r\n  User x\r\n", block), b"Host a\r\n  User x\r\n\r\nHost box\r\n    HostName h\r\n");
    assert_eq!(appended(b"Host a\r\n  User x", block), b"Host a\r\n  User x\r\n\r\nHost box\r\n    HostName h\r\n");
}

#[test]
fn append_block_backs_up_then_appends() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    let before = "Host a\r\n    User x\r\n";
    fs::write(&config, before).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config, fs::Permissions::from_mode(0o640)).unwrap();
    }
    let stamp = "20261125-1342";
    let planned = planned_backup(&config, stamp).unwrap();
    assert_eq!(planned, dir.path().join("config.dromon-backup-20261125-1342"));
    let out = append_block(&config, "Host box\n    HostName h\n", stamp).unwrap();
    assert_eq!(out.backup.as_deref(), Some(planned.as_path()));
    assert_eq!(fs::read_to_string(&planned).unwrap(), before);
    assert_eq!(fs::read_to_string(&config).unwrap(), "Host a\r\n    User x\r\n\r\nHost box\r\n    HostName h\r\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o640,
            "the file keeps its permissions"
        );
    }
    // The same minute again: the first backup stays, the next one is -2.
    assert_eq!(planned_backup(&config, stamp), Some(dir.path().join("config.dromon-backup-20261125-1342-2")));
    let again = append_block(&config, "Host pi\n", stamp).unwrap();
    assert_eq!(again.backup, Some(dir.path().join("config.dromon-backup-20261125-1342-2")));
    assert_eq!(fs::read_to_string(&planned).unwrap(), before, "the first backup is untouched");
    assert!(fs::read_to_string(&config).unwrap().ends_with("HostName h\r\n\r\nHost pi\r\n"));
    let names: Vec<String> =
        fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(!names.iter().any(|n| n.contains("dromon-tmp")), "{names:?}");
}

#[test]
fn a_missing_config_and_folder_are_created_private() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join(".ssh").join("config");
    assert_eq!(planned_backup(&config, "x"), None);
    let out = append_block(&config, "Host box\n    HostName h\n", "x").unwrap();
    assert_eq!(out.backup, None);
    assert_eq!(fs::read_to_string(&config).unwrap(), "Host box\n    HostName h\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&config).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(config.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }
}

#[cfg(unix)]
#[test]
fn a_linked_config_changes_the_file_it_points_to() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("dotfiles-ssh-config");
    fs::write(&real, "Host a\n").unwrap();
    let link = dir.path().join("config");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let out = append_block(&link, "Host box\n", "s").unwrap();
    assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays a link");
    assert_eq!(fs::read_to_string(&real).unwrap(), "Host a\n\nHost box\n");
    assert_eq!(fs::read_to_string(out.backup.unwrap()).unwrap(), "Host a\n");
}

#[cfg(unix)]
#[test]
fn a_failed_write_leaves_the_config_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join("ssh");
    fs::create_dir(&ssh).unwrap();
    let config = ssh.join("config");
    fs::write(&config, "Host a\n").unwrap();
    // A folder nothing can be written into: neither the backup nor the new file can be made.
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o500)).unwrap();
    let probe = ssh.join("probe");
    if fs::write(&probe, "").is_ok() {
        // Running as root: permissions don't stop us.
        let _ = fs::remove_file(&probe);
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        eprintln!("skipped: permissions don't apply (root)");
        return;
    }
    let err = append_block(&config, "Host box\n", "s").unwrap_err();
    assert!(err.to_string().starts_with("Couldn't back up to"), "{err}");
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(fs::read_to_string(&config).unwrap(), "Host a\n");
    assert_eq!(fs::read_dir(&ssh).unwrap().count(), 1, "nothing else was left behind");
}

#[test]
fn paths_show_under_home_as_the_platform_writes_it() {
    assert_eq!(display_path_str("/home/u/.ssh/config", "/home/u", false), "~/.ssh/config");
    assert_eq!(
        display_path_str("/home/u/.ssh/config.dromon-backup-1", "/home/u/", false),
        "~/.ssh/config.dromon-backup-1"
    );
    assert_eq!(display_path_str("/home/user2/.ssh/config", "/home/u", false), "/home/user2/.ssh/config");
    assert_eq!(display_path_str("/etc/ssh/ssh_config", "", false), "/etc/ssh/ssh_config");
    assert_eq!(display_path_str(r"C:\Users\Fatih\.ssh\config", r"C:\Users\Fatih", true), r"%USERPROFILE%\.ssh\config");
    assert_eq!(display_path_str(r"c:\users\fatih\.ssh\config", r"C:\Users\Fatih\", true), r"%USERPROFILE%\.ssh\config");
    assert_eq!(
        display_path_str(r"C:\Users\Fatih2\.ssh\config", r"C:\Users\Fatih", true),
        r"C:\Users\Fatih2\.ssh\config"
    );
    assert_eq!(display_path_str(r"D:\cfg\config", r"C:\Users\Fatih", true), r"D:\cfg\config");
    assert_eq!(display_path(Path::new("/x/y"), None), "/x/y");
}

/// Windows writes backups next to `%USERPROFILE%\.ssh\config` the same way (pure path maths).
#[cfg(windows)]
#[test]
fn windows_backup_names() {
    let p = Path::new(r"C:\Users\Fatih\.ssh\config");
    assert_eq!(
        backup_name(p, "20261125-1342", 3),
        Path::new(r"C:\Users\Fatih\.ssh\config.dromon-backup-20261125-1342-3")
    );
}
