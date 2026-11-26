use super::*;
use crate::command::SshCommand;

fn addr(user: Option<&str>, host: &str, port: Option<&str>) -> Option<Address> {
    Some(Address { user: user.map(str::to_owned), host: host.to_owned(), port: port.map(str::to_owned) })
}

#[test]
fn addresses_split_into_user_host_and_port() {
    assert_eq!(split_address("deploy@10.0.0.4:2222"), addr(Some("deploy"), "10.0.0.4", Some("2222")));
    assert_eq!(split_address("  deploy@prod.example.com \n"), addr(Some("deploy"), "prod.example.com", None));
    assert_eq!(split_address("prod.example.com:22"), addr(None, "prod.example.com", Some("22")));
    assert_eq!(split_address("[fe80::1%eth0]:2200"), addr(None, "fe80::1%eth0", Some("2200")));
    assert_eq!(split_address("root@[2001:db8::7]"), addr(Some("root"), "2001:db8::7", None));
    assert_eq!(split_address("ssh://pi@raspberrypi.local:2022/"), addr(Some("pi"), "raspberrypi.local", Some("2022")));
    // ssh splits at the last @.
    assert_eq!(split_address("me@corp.example@bastion"), addr(Some("me@corp.example"), "bastion", None));
    // A port that isn't one still splits off: Port says what's wrong with it.
    assert_eq!(split_address("box:ssh"), addr(None, "box", Some("ssh")));
}

#[test]
fn nothing_to_split_is_none() {
    for text in ["prod", "10.0.0.4", "fe80::1", "2001:db8::7", "", "   ", "user@", "[::1", "[::1]x", ":22"] {
        assert_eq!(split_address(text), None, "{text:?}");
    }
    // `@host:` has a host and an (empty) port.
    assert_eq!(split_address("@host:"), addr(None, "host", Some("")));
}

#[test]
fn ports_are_1_to_65535_or_nothing() {
    assert_eq!(parse_port(""), Ok(None));
    assert_eq!(parse_port(" 22 "), Ok(Some(22)));
    assert_eq!(parse_port("65535"), Ok(Some(65535)));
    for bad in ["0", "65536", "-1", "+22", "2 2", "ssh", "99999999999999999999"] {
        assert!(parse_port(bad).is_err(), "{bad:?}");
    }
}

fn input(host: &str) -> QuickInput {
    QuickInput { host: host.into(), ..QuickInput::default() }
}

fn errors(r: Result<HostSpec, Vec<(QuickField, String)>>) -> Vec<QuickField> {
    r.unwrap_err().into_iter().map(|(f, _)| f).collect()
}

#[test]
fn a_recent_host_from_one_typed_address() {
    let h = quick_host(&input("deploy@10.0.0.4:2222"), HostSource::Recent).unwrap();
    assert_eq!(h.id, HostId("deploy@10.0.0.4:2222".into()));
    assert_eq!(h.alias, "10.0.0.4");
    assert_eq!((h.hostname.as_deref(), h.user.as_deref(), h.port), (Some("10.0.0.4"), Some("deploy"), Some(2222)));
    assert_eq!((h.identity_file, h.proxy_jump), (None, None));
    // Without a port the id says 22: the same server however it's typed.
    let h = quick_host(&QuickInput { user: "deploy".into(), ..input("10.0.0.4") }, HostSource::Recent).unwrap();
    assert_eq!(h.id, HostId("deploy@10.0.0.4:22".into()));
    assert_eq!(h.port, None);
    let v6 = quick_host(&input("[2001:db8::7]:2200"), HostSource::Recent).unwrap();
    assert_eq!(v6.id, HostId("[2001:db8::7]:2200".into()));
    assert_eq!(v6.alias, "2001:db8::7");
}

#[test]
fn a_saved_host_is_known_by_its_name() {
    let i = QuickInput {
        host: "10.0.0.4".into(),
        user: "deploy".into(),
        port: "2222".into(),
        alias: "box".into(),
        identity_file: "~/.ssh/id deploy".into(),
        proxy_jump: "jump@bastion:2200".into(),
    };
    let h = quick_host(&i, HostSource::Saved).unwrap();
    assert_eq!((h.id.as_str(), h.alias.as_str(), h.source), ("box", "box", HostSource::Saved));
    assert_eq!(h.identity_file.as_deref(), Some("~/.ssh/id deploy"));
    assert_eq!(h.proxy_jump.as_deref(), Some("jump@bastion:2200"));
    // Editing it gives the same fields back.
    assert_eq!(QuickInput::from_spec(&h), i);
    // A made-up name stays empty when editing.
    let plain = quick_host(&input("10.0.0.9"), HostSource::Saved).unwrap();
    assert_eq!(plain.alias, "10.0.0.9");
    assert_eq!(QuickInput::from_spec(&plain).alias, "");
}

#[test]
fn every_field_is_checked_and_errors_say_which() {
    let bad = QuickInput {
        host: "-oProxyCommand=sh".into(),
        user: "a b".into(),
        port: "70000".into(),
        alias: "my box".into(),
        identity_file: "-i".into(),
        proxy_jump: "-J evil".into(),
    };
    let fields = errors(quick_host(&bad, HostSource::Saved));
    assert_eq!(
        fields,
        [
            QuickField::Host,
            QuickField::User,
            QuickField::Port,
            QuickField::Alias,
            QuickField::IdentityFile,
            QuickField::ProxyJump
        ]
    );
    assert_eq!(errors(quick_host(&input(""), HostSource::Recent)), [QuickField::Host]);
    assert_eq!(errors(quick_host(&input("pro d"), HostSource::Recent)), [QuickField::Host]);
    assert_eq!(errors(quick_host(&input("a\u{7}b"), HostSource::Recent)), [QuickField::Host]);
    assert_eq!(errors(quick_host(&input("a\"b"), HostSource::Recent)), [QuickField::Host]);
    assert_eq!(errors(quick_host(&input("${HOME}"), HostSource::Recent)), [QuickField::Host]);
    assert_eq!(
        errors(quick_host(&QuickInput { user: "a%b".into(), ..input("h") }, HostSource::Recent)),
        [QuickField::User]
    );
    assert_eq!(
        errors(quick_host(&QuickInput { identity_file: "/k\nx".into(), ..input("h") }, HostSource::Recent)),
        [QuickField::IdentityFile]
    );
    assert_eq!(
        errors(quick_host(&QuickInput { alias: "a*".into(), ..input("h") }, HostSource::Saved)),
        [QuickField::Alias]
    );
    assert_eq!(
        errors(quick_host(&QuickInput { alias: "a@b".into(), ..input("h") }, HostSource::Saved)),
        [QuickField::Alias]
    );
    assert_eq!(errors(quick_host(&input("h:0"), HostSource::Recent)), [QuickField::Port]);
    // A typed address that disagrees with the fields.
    let clash = QuickInput { user: "root".into(), port: "22".into(), ..input("deploy@h:2222") };
    assert_eq!(errors(quick_host(&clash, HostSource::Recent)), [QuickField::User, QuickField::Port]);
    let msg = quick_host(&input("-x"), HostSource::Recent).unwrap_err().remove(0).1;
    assert_eq!(msg, "Can't start with “-”");
}

#[test]
fn spaces_are_fine_in_identity_paths_only() {
    let i = QuickInput { identity_file: r"C:\Users\John Doe\.ssh\id_ed25519".into(), ..input("h") };
    assert!(quick_host(&i, HostSource::Recent).is_ok());
    let i = QuickInput { identity_file: r"\\server\share\id".into(), ..input("h") };
    assert_eq!(errors(quick_host(&i, HostSource::Recent)), [QuickField::IdentityFile]);
    let i = QuickInput { identity_file: r"C:\my dir\".into(), ..input("h") };
    assert_eq!(errors(quick_host(&i, HostSource::Recent)), [QuickField::IdentityFile]);
    let i = QuickInput { proxy_jump: "a b".into(), ..input("h") };
    assert_eq!(errors(quick_host(&i, HostSource::Recent)), [QuickField::ProxyJump]);
}

#[test]
fn generated_names_are_safe_tokens() {
    assert_eq!(generated_alias("10.0.0.4"), "10.0.0.4");
    assert_eq!(generated_alias("fe80::1%eth0"), "fe80::1%eth0");
    assert_eq!(generated_alias("münchen.example"), "münchen.example");
    assert_eq!(generated_alias("-x"), "x");
    assert_eq!(generated_alias("a/b c"), "a-b-c");
    assert_eq!(generated_alias("---"), "host");
    for host in ["10.0.0.4", "-x", "a/b c", "---", "ä"] {
        assert!(check_alias(&generated_alias(host)).is_ok(), "{host}");
    }
}

#[test]
fn quick_hosts_build_the_explicit_command() {
    let i = QuickInput {
        identity_file: "/keys/id ed".into(),
        proxy_jump: "bastion".into(),
        ..input("deploy@10.0.0.4:2222")
    };
    let h = quick_host(&i, HostSource::Recent).unwrap();
    let env = crate::AskpassEnv { exe: "/opt/dromon".into(), endpoint: "e".into(), token: "t".into() };
    let cmd = SshCommand::sftp(&h, &env, false).unwrap();
    let args: Vec<String> = cmd.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let n = crate::BASE_ARGS.len();
    assert_eq!(
        args[n..],
        [
            "-o",
            "HostName=10.0.0.4",
            "-o",
            "User=deploy",
            "-o",
            "Port=2222",
            "-o",
            "IdentityFile=\"/keys/id ed\"",
            "-o",
            "ProxyJump=bastion",
            "-s",
            "10.0.0.4",
            "sftp"
        ]
    );
}

#[test]
fn same_target_ignores_the_name_and_the_default_port() {
    let a = quick_host(&input("deploy@h"), HostSource::Recent).unwrap();
    let b = quick_host(&QuickInput { alias: "named".into(), ..input("deploy@h:22") }, HostSource::Saved).unwrap();
    assert!(same_target(&a, &b));
    let c = quick_host(&input("root@h"), HostSource::Recent).unwrap();
    assert!(!same_target(&a, &c));
}
