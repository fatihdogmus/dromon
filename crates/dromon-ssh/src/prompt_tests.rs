use dromon_core::PromptKind;

use super::*;

fn host_key(key_type: &str, fingerprint: &str) -> PromptKind {
    PromptKind::HostKey { key_type: Some(key_type.into()), fingerprint: Some(fingerprint.into()) }
}

fn passphrase(path: &str) -> PromptKind {
    PromptKind::Passphrase { key_path: Some(path.into()) }
}

const FP: &str = "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU";

#[test]
fn table() {
    let fp_dot = format!("ED25519 key fingerprint is {FP}.");
    let cases: Vec<(String, Option<&str>, PromptKind)> = vec![
        // OpenSSH 8.4 (single snprintf, trailing period).
        (
            format!(
                "The authenticity of host 'prod (10.0.0.4)' can't be established.\n{fp_dot}\n\
                 Are you sure you want to continue connecting (yes/no/[fingerprint])? "
            ),
            None,
            host_key("ED25519", FP),
        ),
        // OpenSSH 9.x: known under other names, DNS line.
        (
            format!(
                "The authenticity of host 'prod.example (10.0.0.4)' can't be established.\n\
                 ECDSA key fingerprint is {FP}.\n\
                 No matching host key fingerprint found in DNS.\n\
                 This host key is known by the following other names/addresses:\n    \
                 ~/.ssh/known_hosts:12: prod\n\
                 Are you sure you want to continue connecting (yes/no/[fingerprint])? "
            ),
            None,
            host_key("ECDSA", FP),
        ),
        // Keys of a different type already known (9.x wording).
        (
            format!(
                "The authenticity of host '[h]:2222 ([::1]:2222)' can't be established\n\
                 but keys of different type are already known for this host.\n\
                 RSA key fingerprint is {FP}.\n\
                 Are you sure you want to continue connecting (yes/no/[fingerprint])? "
            ),
            None,
            host_key("RSA", FP),
        ),
        // OpenSSH 10.x: `is: <fp>` without the period, security-key type, VisualHostKey art.
        (
            format!(
                "The authenticity of host 'pi (192.168.1.9)' can't be established.\n\
                 ED25519-SK key fingerprint is: {FP}\n\
                 +--[ED25519 256]--+\n|       .o.       |\n+----[SHA256]-----+\n\
                 Are you sure you want to continue connecting (yes/no/[fingerprint])? "
            ),
            None,
            host_key("ED25519-SK", FP),
        ),
        // FingerprintHash md5.
        (
            "The authenticity of host 'old (10.0.0.9)' can't be established.\n\
             RSA key fingerprint is MD5:9b:d3:2a:aa:6e:1e:63:b2:00:6d:a8:4f:03:22:f9:27.\n\
             Are you sure you want to continue connecting (yes/no/[fingerprint])? "
                .into(),
            None,
            host_key("RSA", "MD5:9b:d3:2a:aa:6e:1e:63:b2:00:6d:a8:4f:03:22:f9:27"),
        ),
        // Re-asks after an unclear answer, and the IP-mismatch confirmation.
        (
            "Please type 'yes', 'no' or the fingerprint: ".into(),
            None,
            PromptKind::HostKey { key_type: None, fingerprint: None },
        ),
        (
            "Warning: the ECDSA host key for 'prod' differs from the key for the IP address '10.0.0.5'\n\
             Offending key for IP in /home/u/.ssh/known_hosts:3\n\
             Matching host key in /home/u/.ssh/known_hosts:7\n\
             Are you sure you want to continue connecting (yes/no)? "
                .into(),
            None,
            PromptKind::HostKey { key_type: None, fingerprint: None },
        ),
        ("Please type 'yes' or 'no': ".into(), None, PromptKind::HostKey { key_type: None, fingerprint: None }),
        // Passwords.
        ("deploy@10.0.0.4's password: ".into(), None, PromptKind::Password),
        ("root@[fe80::1%eth0]'s password: ".into(), None, PromptKind::Password),
        ("Password: ".into(), None, PromptKind::Password),
        ("password:".into(), None, PromptKind::Password),
        ("(deploy@prod) Password: ".into(), None, PromptKind::Password),
        ("(deploy@prod) Password for deploy@EXAMPLE.COM: ".into(), None, PromptKind::Password),
        ("Enter password: ".into(), None, PromptKind::Password),
        ("(u@h) Enter your password: ".into(), None, PromptKind::Password),
        // Passphrases.
        ("Enter passphrase for key '/home/u/.ssh/id_ed25519': ".into(), None, passphrase("/home/u/.ssh/id_ed25519")),
        (
            "Enter passphrase for key 'C:\\Users\\John Doe/.ssh/id_rsa': ".into(),
            None,
            passphrase("C:\\Users\\John Doe/.ssh/id_rsa"),
        ),
        ("Enter passphrase for /home/u/.ssh/id_rsa: ".into(), None, passphrase("/home/u/.ssh/id_rsa")),
        ("Enter passphrase for \"/home/u/.ssh/id_rsa\": ".into(), None, passphrase("/home/u/.ssh/id_rsa")),
        (
            "Enter passphrase for /home/u/.ssh/id_rsa (will confirm each use): ".into(),
            None,
            passphrase("/home/u/.ssh/id_rsa"),
        ),
        ("Bad passphrase, try again for /home/u/.ssh/id_rsa: ".into(), None, passphrase("/home/u/.ssh/id_rsa")),
        ("Enter passphrase: ".into(), None, PromptKind::Passphrase { key_path: None }),
        // Everything else.
        ("(deploy@prod) Verification code: ".into(), None, PromptKind::Other),
        ("Verification code: ".into(), None, PromptKind::Other),
        ("(u@h) One-time password (OATH) for `u': ".into(), None, PromptKind::Other),
        ("(u@h) Enter one-time password: ".into(), None, PromptKind::Other),
        ("Enter PIN for ED25519-SK key /home/u/.ssh/id_ed25519_sk: ".into(), None, PromptKind::Other),
        ("Enter deploy@prod's old password: ".into(), None, PromptKind::Other),
        ("Enter deploy@prod's new password: ".into(), None, PromptKind::Other),
        ("Retype deploy@prod's new password: ".into(), None, PromptKind::Other),
        ("Enter passphrase (empty for no passphrase): ".into(), None, PromptKind::Other),
        (
            "(u@h) Duo two-factor login for u\n\nEnter a passcode or select one of the following options:\n\n \
             1. Duo Push to XXX-XXX-1234\n\nPasscode or option (1-1): "
                .into(),
            None,
            PromptKind::Other,
        ),
        ("(u@h) Please enter your OTP\nPassword: ".into(), None, PromptKind::Other),
        ("LDAP Password: ".into(), None, PromptKind::Other),
        ("Password for my account: ".into(), None, PromptKind::Other),
        ("".into(), None, PromptKind::Other),
        // Hints.
        ("Confirm user presence for key ED25519-SK SHA256:abc".into(), Some("none"), PromptKind::Notice),
        (
            "Allow use of key /home/u/.ssh/id_rsa?\nKey fingerprint SHA256:abc.".into(),
            Some("confirm"),
            PromptKind::Confirm,
        ),
        ("Add key /home/u/.ssh/id_ed25519 (u@laptop) to agent?".into(), Some("confirm"), PromptKind::Confirm),
        ("Password: ".into(), Some("CONFIRM"), PromptKind::Confirm),
        ("Password: ".into(), Some(" none "), PromptKind::Notice),
        ("Enter passphrase for key '/k': ".into(), Some("none"), PromptKind::Notice),
        ("Password: ".into(), Some("something-new"), PromptKind::Password),
        ("Password: ".into(), Some(""), PromptKind::Password),
        // The host-key question wins over `confirm` (OpenSSH sends it without a hint today).
        (
            format!(
                "The authenticity of host 'prod (10.0.0.4)' can't be established.\n{fp_dot}\n\
                 Are you sure you want to continue connecting (yes/no/[fingerprint])? "
            ),
            Some("confirm"),
            host_key("ED25519", FP),
        ),
        (
            "Please type 'yes' or 'no': ".into(),
            Some("confirm"),
            PromptKind::HostKey { key_type: None, fingerprint: None },
        ),
    ];
    for (text, hint, expected) in cases {
        assert_eq!(classify_prompt(&text, hint), expected, "prompt {text:?} hint {hint:?}");
    }
}

#[test]
fn user_host_prefix_needs_an_at_sign_and_no_spaces() {
    assert_eq!(strip_user_host_prefix("(u@h) Password:"), "Password:");
    assert_eq!(strip_user_host_prefix("(note) Password:"), "(note) Password:");
    assert_eq!(strip_user_host_prefix("(a b@c) Password:"), "(a b@c) Password:");
    assert_eq!(strip_user_host_prefix("(u@h)"), "(u@h)");
}

#[test]
fn password_identity_is_user_at_host_when_ssh_names_it() {
    let cases = [
        ("deploy@10.0.0.4's password: ", "deploy@10.0.0.4"),
        ("  root@[fe80::1%eth0]'s Password:\n", "root@[fe80::1%eth0]"),
        ("(deploy@prod) Password: ", "deploy@prod"),
        ("(deploy@prod) Password for deploy@EXAMPLE.COM: ", "deploy@prod"),
        ("Password: ", "Password:"),
        ("Enter password: ", "Enter password:"),
        ("Password for deploy@EXAMPLE.COM: ", "Password for deploy@EXAMPLE.COM:"),
        ("(note) Password: ", "(note) Password:"),
        ("'s password: ", "'s password:"),
        ("", ""),
    ];
    for (text, expected) in cases {
        assert_eq!(password_identity(text), expected, "{text:?}");
    }
    // The jump host and the target of a ProxyJump are told apart.
    assert_ne!(password_identity("jump@bastion's password: "), password_identity("deploy@10.0.0.4's password: "));
}

#[test]
fn passphrase_with_non_ascii_path() {
    assert_eq!(
        classify_prompt("Enter passphrase for key '/home/ğüş/.ssh/id_ed25519': ", None),
        passphrase("/home/ğüş/.ssh/id_ed25519")
    );
}
