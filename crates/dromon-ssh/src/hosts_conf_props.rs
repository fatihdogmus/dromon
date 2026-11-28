//! Properties of saved hosts and "Add to SSH config". Hosts `host_block` accepts read back from
//! `hosts.conf` as written (as saved hosts, known by their names, the first of a name kept).
//! Reading any text never panics and gives only hosts with one literal, unique name. Appending
//! a block keeps the file's bytes and adds the block's lines after a blank line, in the file's
//! line endings. A path shows from `~` (or `%USERPROFILE%`) exactly when it's under home.

use hegel::TestCase;
use hegel::generators::{self as gs, Generator};

use super::*;

/// Pieces ssh's config parsers read specially, and some they don't.
const PIECES: &[&str] = &[
    " ", "\t", "\u{b}", "\u{c}", "\r", "\n", "\0", "\"", "'", "\\", "\\\\", "${", "%", "%%", "#", "=", "-", "*", "!",
    "@", ":", "/", "\u{a0}", "\u{2028}", "\u{feff}", "é", "漢", "a", "0", "Host", "Match",
];

/// Whitespace of every kind, ASCII and not, and nothing.
const SPACES: &[&str] = &["", " ", "\t", "\u{b}", "\u{c}", "\r", "\u{a0}", "\u{85}", "\u{2028}", "\u{3000}"];

/// Text with what ssh parses specially anywhere in it, with whitespace of some kind around it,
/// or any text.
#[hegel::composite]
fn awkward_text(tc: &TestCase) -> String {
    let piece = hegel::one_of!(gs::sampled_from(PIECES).map(|p: &str| p.to_owned()), gs::text().max_size(4));
    let pieces = gs::vecs(piece).max_size(10).map(|p: Vec<String>| p.concat());
    let space = || gs::sampled_from(SPACES).map(|s: &str| s.to_owned());
    tc.draw(hegel::one_of!(
        hegel::tuples!(space(), pieces, space()).map(|(a, b, c): (String, String, String)| a + &b + &c),
        gs::text(),
    ))
}

/// A host's value: awkward, or a plain word.
#[hegel::composite]
fn values(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(awkward_text(), gs::from_regex("[a-z0-9~/][a-z0-9.@:_/ -]{0,12}")))
}

/// A host from any source, its name and values awkward or plain, its port a real one.
#[hegel::composite]
fn hosts(tc: &TestCase) -> HostSpec {
    let source = tc.draw(gs::sampled_from(vec![HostSource::SshConfig, HostSource::Saved, HostSource::Recent]));
    let alias = tc.draw(hegel::one_of!(gs::from_regex("[A-Za-zé0-9._:%+][A-Za-zé0-9._:%+-]{0,10}"), values()));
    HostSpec {
        id: HostId(tc.draw(values())),
        source,
        alias,
        hostname: tc.draw(gs::optional(values())),
        user: tc.draw(gs::optional(values())),
        port: tc.draw(gs::optional(gs::integers::<u16>().min_value(1))),
        identity_file: tc.draw(gs::optional(values())),
        proxy_jump: tc.draw(gs::optional(values())),
    }
}

/// Lines a config file could hold: `Host` and `Match` lines, settings, comments, anything.
#[hegel::composite]
fn config_texts(tc: &TestCase) -> String {
    let keyword = gs::sampled_from(vec!["Host", "host", "HOST", "Match", "HostName", "User", "Port", "IdentityFile"]);
    let line = hegel::tuples!(
        gs::sampled_from(vec!["", "  ", "\t"]),
        keyword,
        gs::sampled_from(vec![" ", "=", " = ", "\t"]),
        awkward_text()
    )
    .map(|(indent, keyword, sep, rest): (&str, &str, &str, String)| format!("{indent}{keyword}{sep}{rest}"));
    let lines = gs::vecs(hegel::one_of!(line, awkward_text())).max_size(12);
    let eol = tc.draw(gs::sampled_from(vec!["\n", "\r\n"]));
    let bom = if tc.draw(gs::booleans()) { "\u{feff}" } else { "" };
    format!("{bom}{}", tc.draw(lines).join(eol))
}

#[hegel::test]
#[hegel::explicit_test_case(specs = vec![HostSpec {
    proxy_jump: Some("\u{c}".to_owned()),
    ..HostSpec::from_config_alias("0")
}])]
#[hegel::explicit_test_case(specs = vec![HostSpec {
    proxy_jump: Some("\u{c}\u{b}".to_owned()),
    ..HostSpec::from_config_alias("0")
}])]
fn hosts_conf_reads_back_every_host_host_block_accepts(tc: TestCase) {
    let specs = tc.draw(gs::vecs(hosts()).max_size(4));
    let accepted: Vec<HostSpec> = specs.into_iter().filter(|h| host_block(h).is_ok()).collect();
    let text = render_hosts_conf(&accepted).unwrap();
    let mut expected: Vec<HostSpec> = Vec::new();
    for host in accepted {
        if !expected.iter().any(|e| e.alias == host.alias) {
            expected.push(HostSpec { id: HostId(host.alias.clone()), source: HostSource::Saved, ..host });
        }
    }
    assert_eq!(parse_hosts_conf(&text), expected, "{text:?}");
}

#[hegel::test]
fn reading_any_text_gives_only_saved_hosts_with_one_literal_unique_name(tc: TestCase) {
    let text = tc.draw(hegel::one_of!(config_texts(), gs::text()));
    let hosts = parse_hosts_conf(&text);
    for (i, host) in hosts.iter().enumerate() {
        assert!(check_alias(&host.alias).is_ok(), "{host:?}");
        assert_eq!((&host.id.0, host.source), (&host.alias, HostSource::Saved));
        assert!(host.port != Some(0));
        assert!(!hosts[..i].iter().any(|h| h.alias == host.alias), "{:?} twice", host.alias);
    }
}

/// `appended` keeps `original`'s bytes, adds a line break if its last line has none and a blank
/// line, then the block's lines, all ending as `original`'s lines do (CRLF if any does).
#[hegel::test]
fn appending_keeps_the_file_and_adds_the_blocks_lines_after_a_blank_line(tc: TestCase) {
    let original = tc.draw(hegel::one_of!(gs::binary(), config_texts().map(String::into_bytes)));
    let block = tc.draw(hegel::one_of!(config_texts(), awkward_text()));
    let out = appended(&original, &block);
    assert!(out.starts_with(&original));
    let eol: &[u8] = if original.windows(2).any(|w| w == b"\r\n") { b"\r\n" } else { b"\n" };
    let mut tail = &out[original.len()..];
    if !original.is_empty() {
        if !original.ends_with(b"\n") {
            tail = tail.strip_prefix(eol).expect("the last line is ended");
        }
        tail = tail.strip_prefix(eol).expect("a blank line");
    }
    let lines: Vec<u8> = block.lines().flat_map(|l| [l.as_bytes(), eol].concat()).collect();
    assert_eq!(tail, &lines[..]);
}

#[hegel::test]
fn a_path_under_home_shows_from_the_home_sign(tc: TestCase) {
    let windows = tc.draw(gs::booleans());
    let home = tc.draw(awkward_text());
    let rest = tc.draw(awkward_text());
    let sep = tc.draw(gs::sampled_from(if windows { vec!["\\", "/"] } else { vec!["/"] }));
    let trailing = tc.draw(gs::sampled_from(vec!["", "/", "\\", "//"]));
    let bare = home.trim_end_matches(['/', '\\']);
    tc.assume(!bare.is_empty());
    let name = if windows { "%USERPROFILE%" } else { "~" };
    let home = format!("{bare}{trailing}");
    assert_eq!(display_path_str(bare, &home, windows), name);
    let path = format!("{bare}{sep}{rest}");
    assert_eq!(display_path_str(&path, &home, windows), format!("{name}{sep}{rest}"));
    if windows {
        let shouted = format!("{}{sep}{rest}", bare.to_ascii_uppercase());
        assert_eq!(display_path_str(&shouted, &home, windows), format!("{name}{sep}{rest}"));
    }
}

/// Any path shows in full, or as the home sign and what follows home when it's under home.
#[hegel::test]
fn any_other_path_shows_in_full(tc: TestCase) {
    let windows = tc.draw(gs::booleans());
    let path = tc.draw(awkward_text());
    let home = tc.draw(hegel::one_of!(awkward_text(), gs::sampled_from(vec![path.clone()])));
    let shown = display_path_str(&path, &home, windows);
    if shown == path {
        return;
    }
    let name = if windows { "%USERPROFILE%" } else { "~" };
    let rest = shown.strip_prefix(name).expect("shown from the home sign");
    let head = path.strip_suffix(rest).expect("the rest of the path");
    let bare = home.trim_end_matches(['/', '\\']);
    assert!(if windows { head.eq_ignore_ascii_case(bare) } else { head == bare }, "{head:?} isn't {home:?}");
    let seps: &[char] = if windows { &['\\', '/'] } else { &['/'] };
    assert!(rest.is_empty() || rest.starts_with(seps), "{rest:?} isn't at a folder");
}
