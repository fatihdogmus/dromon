//! Properties of quick connect. Splitting any typed address never panics, and an address built
//! from parts (`user@host:port`, `[v6]:port`, `ssh://…`) splits back into them. A port is
//! exactly a number from 1 to 65535, or nothing. A made-up name is always a valid one. A recent
//! host's id is the same however its port 22 is written. `same_target` is reflexive and
//! symmetric. `quick_host` never panics on any fields, and a host it makes passes every check
//! the command builder and a `Host` block make.

use hegel::TestCase;
use hegel::generators::{self as gs, Generator};

use super::*;
use crate::command::SshCommand;
use crate::hosts_conf::{host_block, parse_hosts_conf};

/// Pieces addresses and ssh read specially, and some they don't.
const PIECES: &[&str] = &[
    "@", ":", "[", "]", "/", "ssh://", "SSH://", "%", "-", ".", " ", "\t", "\n", "\u{c}", "\0", "\"", "'", "\\", "${",
    "#", "=", "*", "é", "ğ", "漢", "🚀", "\u{a0}", "\u{3000}", "a", "0", "22", "65535", "65536", "::1",
];

/// Text with what addresses and ssh parse specially anywhere in it, or any text.
#[hegel::composite]
fn awkward_text(tc: &TestCase) -> String {
    let piece = hegel::one_of!(gs::sampled_from(PIECES).map(|p: &str| p.to_owned()), gs::text().max_size(4));
    tc.draw(hegel::one_of!(gs::vecs(piece).max_size(10).map(|p: Vec<String>| p.concat()), gs::text()))
}

/// A field as typed: awkward, or plain.
#[hegel::composite]
fn typed(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(awkward_text(), gs::from_regex("[a-z0-9][a-z0-9._-]{0,10}"), gs::just(String::new())))
}

/// One part of an address: no whitespace at its ends and none of `forbidden`.
#[hegel::composite]
fn parts(tc: &TestCase, forbidden: &'static str) -> String {
    let part = tc.draw(hegel::one_of!(awkward_text(), gs::from_regex("[a-z0-9][a-z0-9.-]{0,10}")));
    let part: String = part.trim().chars().filter(|c| !forbidden.contains(*c)).collect();
    part.trim().to_owned()
}

/// A host from any source, as typed or made up.
#[hegel::composite]
fn specs(tc: &TestCase) -> HostSpec {
    let source = tc.draw(gs::sampled_from(vec![HostSource::SshConfig, HostSource::Saved, HostSource::Recent]));
    let alias = tc.draw(typed());
    HostSpec {
        id: HostId(alias.clone()),
        source,
        alias,
        hostname: tc.draw(gs::optional(typed())),
        user: tc.draw(gs::optional(typed())),
        port: tc.draw(gs::optional(gs::integers::<u16>())),
        identity_file: tc.draw(gs::optional(typed())),
        proxy_jump: tc.draw(gs::optional(typed())),
    }
}

#[hegel::test]
#[hegel::explicit_test_case(text = "0\u{800}\u{800}".to_owned())]
fn splitting_any_typed_address_never_panics(tc: TestCase) {
    let text = tc.draw(awkward_text());
    if let Some(a) = split_address(&text) {
        assert!(!a.host.is_empty());
    }
}

/// `[user@]host[:port]` with a user or a port splits back into its parts, with or without
/// `ssh://` and a last `/`, and so does `[user@][v6][:port]`.
#[hegel::test]
#[hegel::explicit_test_case(
    user = None::<String>,
    name = "0\u{800}\u{800}".to_owned(),
    address = std::net::Ipv6Addr::UNSPECIFIED,
    v6 = false,
    port = Some("0".to_owned()),
    uri = false
)]
fn an_address_built_from_parts_splits_back_into_them(tc: TestCase) {
    let user = tc.draw(gs::optional(parts("")));
    let name = tc.draw(parts("@:[/"));
    let address = tc.draw(gs::ip_addresses().v6());
    let v6 = tc.draw(gs::booleans());
    let port = tc.draw(gs::optional(parts("@:/")));
    let uri = tc.draw(gs::booleans());
    let host = if v6 { address.to_string() } else { name };
    tc.assume(!host.is_empty() && user.as_ref().is_none_or(|u| !u.is_empty()));
    tc.assume(v6 || uri || user.is_some() || port.is_some());
    tc.assume(!user.as_ref().is_some_and(|u| u.to_ascii_lowercase().starts_with("ssh://")));
    let user_part = user.as_ref().map(|u| format!("{u}@")).unwrap_or_default();
    let host_part = if v6 { format!("[{host}]") } else { host.clone() };
    let port_part = port.as_ref().map(|p| format!(":{p}")).unwrap_or_default();
    let mut text = format!("{user_part}{host_part}{port_part}");
    if uri {
        text = format!("ssh://{text}/");
    }
    assert_eq!(split_address(&text), Some(Address { user, host, port }), "{text:?}");
}

/// Empty (after trimming) is no port; otherwise only the digits of a number from 1 to 65535
/// are one, leading zeros and all.
#[hegel::test]
fn a_port_is_a_number_from_1_to_65535_or_nothing(tc: TestCase) {
    let text = tc.draw(hegel::one_of!(awkward_text(), gs::from_regex(r"\s*0*[0-9]{1,6}\s*")));
    let t = text.trim();
    let digits = t.trim_start_matches('0');
    let number = (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) && digits.len() <= 5)
        .then(|| digits.parse::<u32>().unwrap_or(0))
        .filter(|n| (1..=65535).contains(n));
    match parse_port(&text) {
        Ok(None) => assert!(t.is_empty(), "{text:?}"),
        Ok(Some(p)) => assert_eq!(Some(u32::from(p)), number, "{text:?}"),
        Err(_) => assert!(!t.is_empty() && number.is_none(), "{text:?}"),
    }
}

#[hegel::test]
fn every_port_reads_as_itself_however_it_is_padded(tc: TestCase) {
    let port = tc.draw(gs::integers::<u16>().min_value(1));
    let zeros = tc.draw(gs::integers::<usize>().max_value(4));
    let space = tc.draw(gs::sampled_from(vec!["", " ", "\t", "\n", "\u{a0}", "\u{3000}"]));
    assert_eq!(parse_port(&format!("{space}{}{port}{space}", "0".repeat(zeros))), Ok(Some(port)));
}

#[hegel::test]
fn a_made_up_name_is_always_a_valid_one_and_always_the_same(tc: TestCase) {
    let hostname = tc.draw(awkward_text());
    let alias = generated_alias(&hostname);
    assert_eq!(check_alias(&alias), Ok(()), "{alias:?}");
    assert_eq!(generated_alias(&hostname), alias);
}

#[hegel::test]
fn a_recent_hosts_id_is_the_same_however_port_22_is_written(tc: TestCase) {
    let hostname = tc.draw(awkward_text());
    let user = tc.draw(gs::optional(awkward_text()));
    let port = tc.draw(gs::optional(gs::integers::<u16>()));
    let id = recent_id(&hostname, user.as_deref(), port);
    assert_eq!(recent_id(&hostname, user.as_deref(), port), id);
    if port.is_none_or(|p| p == 22) {
        assert_eq!(recent_id(&hostname, user.as_deref(), Some(22)), id);
        assert_eq!(recent_id(&hostname, user.as_deref(), None), id);
    }
}

#[hegel::test]
fn same_target_is_reflexive_and_symmetric(tc: TestCase) {
    let a = tc.draw(specs());
    let b = tc.draw(hegel::one_of!(specs(), gs::just(HostSpec { alias: "other".into(), ..a.clone() })));
    assert!(same_target(&a, &a));
    assert_eq!(same_target(&a, &b), same_target(&b, &a));
}

/// Whatever is typed, `quick_host` never panics, and a host it makes is one the session, watch
/// and terminal commands and a `Host` block all accept; a saved one reads back from its block.
#[hegel::test]
#[hegel::explicit_test_case(
    host = "\u{80}\u{80}\u{800}".to_owned(),
    user = String::new(),
    port = String::new(),
    alias = String::new(),
    identity_file = String::new(),
    proxy_jump = String::new(),
    source = HostSource::Saved
)]
fn a_quick_host_passes_every_check_the_command_builder_makes(tc: TestCase) {
    let host = tc.draw(typed());
    let user = tc.draw(typed());
    let port = tc.draw(hegel::one_of!(typed(), gs::integers::<u32>().max_value(70000).map(|p| p.to_string())));
    let alias = tc.draw(typed());
    let identity_file = tc.draw(typed());
    let proxy_jump = tc.draw(typed());
    let source = tc.draw(gs::sampled_from(vec![HostSource::Saved, HostSource::Recent]));
    let input = QuickInput { host, user, port, alias, identity_file, proxy_jump };
    let Ok(spec) = quick_host(&input, source) else { return };
    let env = crate::AskpassEnv { exe: "/opt/dromon".into(), endpoint: "e".into(), token: "t".into() };
    assert!(SshCommand::sftp(&spec, &env, false).is_ok(), "{spec:?}");
    assert!(SshCommand::watch(&spec, &env, "/").is_ok(), "{spec:?}");
    assert!(SshCommand::terminal(&spec, "/").is_ok(), "{spec:?}");
    let block = host_block(&spec).unwrap();
    if source == HostSource::Saved {
        assert_eq!(parse_hosts_conf(&block), [spec]);
    }
}
