//! Properties of ssh's command lines. `check_arg` lets through exactly the values that can't be
//! read as an option and stay one config line. Every value `config_value` accepts reads back as
//! itself, in each `Percent` mode, through both of ssh's config-line parsers (small copies of
//! OpenSSH's here) and through the real `ssh -G`. Words `quote` writes read back as the same
//! words in a POSIX shell (a small reader here, and the real `sh`). The builders refuse a host or
//! put each value only in its own place, never as an option, and `shell_line` reads back as the
//! argument vector.

#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use std::sync::OnceLock;

use dromon_core::HostId;
use hegel::TestCase;
use hegel::generators::{self as gs, Generator};

use super::*;

/// Pieces ssh's config parsers and the shell read specially, and some they don't.
const PIECES: &[&str] = &[
    " ", "\t", "\u{b}", "\u{c}", "\r", "\n", "\r\n", "\0", "\"", "'", "\\", "\\\\", "\\'", "\\\"", "\\ ", "$", "${",
    "$(", "`", ";", "&", "|", "<", ">", "%", "%%", "%h", "#", "=", "-", "--", "!", "*", "?", "~", "(", ")", "{", "}",
    "[", "]", ",", "@", ":", "/", "\u{a0}", "\u{85}", "\u{2028}", "\u{3000}", "\u{feff}", "é", "ğ", "漢", "🚀", "a",
    "Z", "0", "box",
];

/// Whitespace of every kind, ASCII and not, and nothing.
const SPACES: &[&str] =
    &["", " ", "\t", "\u{b}", "\u{c}", "\r", "\n", "\r\n", "\u{a0}", "\u{85}", "\u{2028}", "\u{3000}", "\u{feff}"];

/// Text with what ssh and shells parse specially anywhere in it ([`PIECES`] and short random
/// text, in any order), that with whitespace of some kind before and after it, or random text,
/// now and then a long one.
#[hegel::composite]
fn awkward_text(tc: &TestCase) -> String {
    let piece = || hegel::one_of!(gs::sampled_from(PIECES).map(|p: &str| p.to_owned()), gs::text().max_size(4));
    let pieces = || gs::vecs(piece()).max_size(12).map(|pieces: Vec<String>| pieces.concat());
    let space = || gs::sampled_from(SPACES).map(|s: &str| s.to_owned());
    tc.draw(hegel::one_of!(
        pieces(),
        hegel::tuples!(space(), pieces(), space()).map(|(a, b, c): (String, String, String)| a + &b + &c),
        gs::text(),
        gs::text().min_size(200).max_size(2000),
    ))
}

/// A host field's value: awkward, or a plain word ssh takes as it is.
#[hegel::composite]
fn values(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(awkward_text(), gs::from_regex("[a-z0-9][a-z0-9.@:_-]{0,12}")))
}

/// A host as the app could hold one, from any source, each value awkward or plain.
#[hegel::composite]
fn hosts(tc: &TestCase) -> HostSpec {
    let source = tc.draw(gs::sampled_from(vec![HostSource::SshConfig, HostSource::Saved, HostSource::Recent]));
    let alias = tc.draw(values());
    HostSpec {
        id: HostId(alias.clone()),
        source,
        alias,
        hostname: tc.draw(gs::optional(values())),
        user: tc.draw(gs::optional(values())),
        port: tc.draw(gs::optional(gs::integers::<u16>())),
        identity_file: tc.draw(gs::optional(values())),
        proxy_jump: tc.draw(gs::optional(values())),
    }
}

fn percents() -> impl hegel::PrintableGenerator<Percent> {
    gs::sampled_from(vec![Percent::Escape, Percent::Reject, Percent::Literal])
}

/// The host option that treats `%` as `percent` says (`host_options`).
fn keyword(percent: Percent) -> &'static str {
    match percent {
        Percent::Escape => "IdentityFile",
        Percent::Reject => "User",
        Percent::Literal => "ProxyJump",
    }
}

/// `readconf.c`'s `WHITESPACE`.
const WHITESPACE: &[u8] = b" \t\r\n";

/// How many of `bytes`' first bytes are in `set` (`strspn`).
fn span(bytes: &[u8], set: &[u8]) -> usize {
    bytes.iter().take_while(|b| set.contains(b)).count()
}

/// `misc.c`'s `strdelim`: the next token of `line` from `*at`, split at whitespace, `"` and one
/// `=`, a `"…"` span taken whole; `None` when nothing is left or a quote isn't closed.
fn strdelim(line: &mut Vec<u8>, at: &mut Option<usize>) -> Option<Vec<u8>> {
    let old = (*at)?;
    let Some(found) = line[old..].iter().position(|b| WHITESPACE.contains(b) || b"\"=".contains(b)).map(|i| old + i)
    else {
        *at = None;
        return Some(line[old..].to_vec());
    };
    if line[found] == b'"' {
        line.remove(found);
        let Some(close) = line[found..].iter().position(|b| *b == b'"').map(|i| found + i) else {
            *at = None;
            return None;
        };
        *at = Some(close + 1 + span(&line[close + 1..], WHITESPACE));
        return Some(line[old..close].to_vec());
    }
    let mut next = found + 1 + span(&line[found + 1..], WHITESPACE);
    if line[found] != b'=' && line.get(next) == Some(&b'=') {
        next += 1 + span(&line[next + 1..], WHITESPACE);
    }
    *at = Some(next);
    Some(line[old..found].to_vec())
}

/// `misc.c`'s `argv_split` (OpenSSH 8.7+) with comments ending the line: words apart by spaces
/// and tabs, `"…"` and `'…'` spans, `\` escaping a quote, a backslash or (outside quotes) a space.
fn argv_split(s: &[u8]) -> Result<Vec<Vec<u8>>, &'static str> {
    let mut args = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if s[i] == b' ' || s[i] == b'\t' {
            i += 1;
            continue;
        }
        if s[i] == b'#' {
            break;
        }
        let (mut arg, mut quote) = (Vec::new(), 0u8);
        while i < s.len() {
            let c = s[i];
            match (c, s.get(i + 1)) {
                (b'\\', Some(b'\'' | b'"' | b'\\')) => {
                    i += 1;
                    arg.push(s[i]);
                }
                (b'\\', Some(b' ')) if quote == 0 => {
                    i += 1;
                    arg.push(b' ');
                }
                (b' ' | b'\t', _) if quote == 0 => break,
                (b'"' | b'\'', _) if quote == 0 => quote = c,
                _ if quote != 0 && c == quote => quote = 0,
                _ => arg.push(c),
            }
            i += 1;
        }
        if quote != 0 {
            return Err("invalid quotes");
        }
        args.push(arg);
    }
    Ok(args)
}

/// Which OpenSSH reads the line: 8.7+ splits it like a shell (`argv_split`); 8.4–8.6 (Windows
/// 11's inbox client) only knows double quotes (`strdelim`).
#[derive(Clone, Copy, Debug)]
enum Parser {
    Argv,
    Strdelim,
}

/// What OpenSSH reads as the one value of `line`, a line of `keyword`, which takes one string
/// (`readconf.c`'s `process_config_line` and `parse_string`), with `%%` read as `%` when
/// `percent` is `Escape` (`percent_expand`); `Err` with ssh's complaint where it refuses it.
fn ssh_reads(line: &str, keyword: &str, parser: Parser, percent: Percent) -> Result<String, &'static str> {
    let mut line = line.as_bytes().to_vec();
    // Trailing whitespace and form feeds go, all but the first character.
    while line.len() > 1 && b" \t\r\n\x0c".contains(&line[line.len() - 1]) {
        line.pop();
    }
    let mut at = Some(0);
    let mut read = strdelim(&mut line, &mut at);
    if read.as_deref() == Some(&b""[..]) {
        read = strdelim(&mut line, &mut at);
    }
    if !read.is_some_and(|k| k.eq_ignore_ascii_case(keyword.as_bytes())) {
        return Err("not a line of the keyword");
    }
    let value = match parser {
        Parser::Argv => {
            let rest = at.map_or(&[][..], |i| &line[i + span(&line[i..], WHITESPACE)..]);
            if rest.is_empty() {
                return Err("no argument after keyword");
            }
            let mut args = argv_split(rest)?.into_iter();
            let value = args.next().filter(|a| !a.is_empty()).ok_or("missing argument")?;
            if args.next().is_some() {
                return Err("extra arguments at end of line");
            }
            value
        }
        Parser::Strdelim => {
            let value = strdelim(&mut line, &mut at).filter(|a| !a.is_empty()).ok_or("missing argument")?;
            if strdelim(&mut line, &mut at).is_some_and(|a| !a.is_empty()) {
                return Err("garbage at end of line");
            }
            value
        }
    };
    let value = String::from_utf8(value).map_err(|_| "not UTF-8")?;
    if percent == Percent::Escape { percent_unescaped(&value) } else { Ok(value) }
}

/// `%%` as `%`, as ssh's `percent_expand` reads it; any other `%` would be a token.
fn percent_unescaped(s: &str) -> Result<String, &'static str> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' && chars.next() != Some('%') {
            return Err("a % token");
        }
        out.push(c);
    }
    Ok(out)
}

/// The words a POSIX shell reads from `line`, for the syntax `quote` writes: words apart by
/// blanks, `'…'` spans, and `\` quoting the next character outside them. Any other character
/// outside quotes would be shell syntax (`$`, `;`, a glob, …), so it's an error, as is an open
/// quote.
fn shell_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' => words.extend(word.take()),
            '\'' => {
                let w = word.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => w.push(c),
                        None => return Err("an open quote".into()),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') => {}
                Some(c) => word.get_or_insert_with(String::new).push(c),
                None => return Err("a backslash at the end".into()),
            },
            c => return Err(format!("{c:?} outside quotes")),
        }
    }
    words.extend(word);
    Ok(words)
}

/// What the real `sh` reads from `words`: `printf '%s\0' <words>`, split at NUL.
#[cfg(unix)]
fn real_sh_words(words: &str) -> Vec<String> {
    let out = Command::new("/bin/sh").arg("-c").arg(format!("printf '%s\\0' {words}")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().split_terminator('\0').map(str::to_owned).collect()
}

/// Whether `ssh` runs from `PATH`.
#[cfg(unix)]
fn have_ssh() -> bool {
    static HAVE: OnceLock<bool> = OnceLock::new();
    *HAVE.get_or_init(|| Command::new("ssh").arg("-V").output().is_ok_and(|o| o.status.success()))
}

/// What the real `ssh -G -F <config> [extra] box` reads as `BindInterface`, or its complaint.
#[cfg(unix)]
fn real_ssh_bind_interface(config: &Path, extra: &[String]) -> Result<String, String> {
    let out = Command::new("ssh").arg("-G").arg("-F").arg(config).args(extra).arg("box").output().unwrap();
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let line = out.stdout.split(|b| *b == b'\n').find_map(|l| l.strip_prefix(b"bindinterface "));
    line.map(|v| String::from_utf8_lossy(v).into_owned()).ok_or_else(|| "no bindinterface line".into())
}

/// `args` as strings (they all are).
fn strings(args: &[OsString]) -> Vec<String> {
    args.iter().map(|a| a.to_str().expect("arguments come from strings").to_owned()).collect()
}

/// Checks that `args` are `base`, then a saved or recent `host`'s `-o` options in their order
/// (`HostName`, `User`, `Port`, `IdentityFile`, `ProxyJump`, those it has), each `Keyword=` and
/// its value as `config_value` writes it; returns what follows.
fn after_host_options<'a>(args: &'a [String], base: &[&str], host: &HostSpec) -> &'a [String] {
    assert_eq!(args[..base.len()], *base);
    let mut rest = &args[base.len()..];
    if host.source == HostSource::SshConfig {
        return rest;
    }
    let options = [
        ("HostName", host.hostname.clone(), Percent::Escape),
        ("User", host.user.clone(), Percent::Reject),
        ("Port", host.port.map(|p| p.to_string()), Percent::Literal),
        ("IdentityFile", host.identity_file.clone(), Percent::Escape),
        ("ProxyJump", host.proxy_jump.clone(), Percent::Literal),
    ];
    for (keyword, value, percent) in options {
        let Some(value) = value else { continue };
        let written = config_value(keyword, &value, percent).expect("a built command's values were accepted");
        assert_eq!(rest[..2], ["-o".to_owned(), format!("{keyword}={written}")]);
        rest = &rest[2..];
    }
    rest
}

/// `text` before and after where the quoted folder goes, from `text_for` the folder `x`.
fn around(text_for: impl Fn(&str) -> String) -> (String, String) {
    let probe = text_for("x");
    let (before, after) = probe.split_once("'x'").expect("the folder is quoted in it");
    (before.to_owned(), after.to_owned())
}

/// Checks that `remote` is the remote command `text_for` writes with `folder` quoted in it, and
/// that the shell reads that quoted part back as `folder`, one word.
fn check_remote_command(remote: &str, folder: &str, text_for: impl Fn(&str) -> String) {
    let (before, after) = around(text_for);
    let quoted = remote.strip_prefix(&before).and_then(|r| r.strip_suffix(&after));
    assert_eq!(quoted, Some(quote(folder).as_str()), "{remote:?}");
    assert_eq!(shell_words(&quote(folder)), Ok(vec![folder.to_owned()]));
}

/// A Windows server's folder (`/C:`, `/C:/…`), which gets no remote command.
fn is_windows_folder(folder: &str) -> bool {
    let mut chars = folder.chars();
    chars.next() == Some('/')
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.next() == Some(':')
        && matches!(chars.next(), None | Some('/'))
}

#[hegel::test]
fn check_arg_refuses_exactly_empty_values_a_leading_dash_line_breaks_and_nul(tc: TestCase) {
    let value = tc.draw(awkward_text());
    let expected = if value.is_empty() {
        Err(CommandError::Empty("The value"))
    } else if value.starts_with('-') {
        Err(CommandError::LeadingDash("The value"))
    } else if value.contains(['\n', '\r', '\0']) {
        Err(CommandError::ControlChar("The value"))
    } else {
        Ok(())
    };
    assert_eq!(check_arg("The value", &value), expected);
}

/// Every value `config_value` accepts is the value ssh reads back, from `-o Keyword=…` and from
/// a config file's `Keyword …` line, in both parsers.
#[hegel::test]
#[hegel::explicit_test_case(value = "\u{c}".to_owned(), percent = Percent::Escape)]
fn every_value_config_value_accepts_reads_back_as_itself(tc: TestCase) {
    let value = tc.draw(awkward_text());
    let percent = tc.draw(percents());
    let keyword = keyword(percent);
    let Ok(written) = config_value(keyword, &value, percent) else { return };
    for line in [format!("{keyword}={written}"), format!("    {keyword} {written}\n")] {
        for parser in [Parser::Argv, Parser::Strdelim] {
            assert_eq!(ssh_reads(&line, keyword, parser, percent), Ok(value.clone()), "{parser:?} reading {line:?}");
        }
    }
}

/// `config_value` refuses only what its doc names: what `check_arg` refuses, `"`, `${`, `%` in
/// `Reject` mode and backslash sequences ssh 8.7+ would unescape (a quoted value's last `\`
/// with the closing quote among them).
#[hegel::test]
fn config_value_refuses_only_what_its_doc_names(tc: TestCase) {
    let value = tc.draw(awkward_text());
    let percent = tc.draw(percents());
    let Err(e) = config_value("User", &value, percent) else { return };
    if e != CommandError::Unquotable("User") {
        assert_eq!(Err(e), check_arg("User", &value));
        return;
    }
    let sequence = ["\\\\", "\\'", "\\\""].iter().any(|s| value.contains(s));
    let quoted_backslash = value.contains([' ', '\t', '#', '\'', '=']) && value.ends_with('\\');
    let percent_refused = percent == Percent::Reject && value.contains('%');
    assert!(
        value.contains('"') || value.contains("${") || percent_refused || sequence || quoted_backslash,
        "{value:?}"
    );
}

/// The real ssh (8.7+ here) reads each accepted value back, from a config file's line and from
/// `-o`. It goes in as `BindInterface`, which ssh keeps as read and `ssh -G` prints: ssh splits
/// every one-value keyword's line alike, but lower-cases `HostName`, checks `User`'s characters
/// and expands `%` later, so `%%` shows as written.
#[cfg(unix)]
#[hegel::test(test_cases = 40)] // Each case runs ssh twice.
#[hegel::explicit_test_case(value = "\u{c}".to_owned(), percent = Percent::Escape)]
fn the_real_ssh_reads_every_accepted_config_value_back(tc: TestCase) {
    let value = tc.draw(awkward_text());
    let percent = tc.draw(percents());
    if !have_ssh() {
        eprintln!("skipped: no ssh on PATH");
        return;
    }
    let Ok(written) = config_value(keyword(percent), &value, percent) else { return };
    let expected = if percent == Percent::Escape { value.replace('%', "%%") } else { value.clone() };
    let dir = tempfile::tempdir().unwrap();
    let (empty, block) = (dir.path().join("empty"), dir.path().join("block"));
    std::fs::write(&empty, "").unwrap();
    std::fs::write(&block, format!("Host box\n    BindInterface {written}\n")).unwrap();
    assert_eq!(real_ssh_bind_interface(&block, &[]), Ok(expected.clone()), "the line {written:?}");
    let option = ["-o".to_owned(), format!("BindInterface={written}")];
    assert_eq!(real_ssh_bind_interface(&empty, &option), Ok(expected), "-o {written:?}");
}

/// Quoted words, a space apart, read back in a shell as the same words, whatever they hold,
/// NUL included (no process argument can hold one; this reader can).
#[hegel::test]
fn quoted_words_read_back_as_the_same_words(tc: TestCase) {
    let words = tc.draw(gs::vecs(awkward_text()).min_size(1).max_size(4));
    let line = words.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ");
    assert_eq!(shell_words(&line), Ok(words));
}

/// A NUL stays inside `quote`'s quotes like anything else; `check_arg` and the watch's folder,
/// whose values go to a process, refuse it.
#[hegel::test]
fn a_nul_is_quoted_like_anything_else_and_refused_where_a_process_would_get_it(tc: TestCase) {
    let before = tc.draw(awkward_text());
    let after = tc.draw(awkward_text());
    let word = format!("{before}\0{after}");
    assert_eq!(shell_words(&quote(&word)), Ok(vec![word.clone()]));
    assert!(check_arg("The value", &word).is_err());
    assert_eq!(watch_script(&word), Err(CommandError::Unquotable("The folder")));
}

#[cfg(unix)]
#[hegel::test(test_cases = 50)] // Each case runs a shell.
fn a_real_shell_reads_quoted_words_back(tc: TestCase) {
    let words = tc.draw(gs::vecs(awkward_text()).min_size(1).max_size(4));
    tc.assume(!words.iter().any(|w| w.contains('\0')));
    let line = words.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ");
    assert_eq!(real_sh_words(&line), words);
}

#[hegel::test]
fn the_session_command_has_each_host_value_only_in_its_place(tc: TestCase) {
    let host = tc.draw(hosts());
    let verbose = tc.draw(gs::booleans());
    let Ok(cmd) = SshCommand::sftp_with(PathBuf::from("ssh"), &host, Vec::new(), verbose) else { return };
    let args = strings(&cmd.args);
    let rest = after_host_options(&args, BASE_ARGS, &host);
    let tail = ["-v", "-s", host.alias.as_str(), "sftp"];
    assert_eq!(rest, if verbose { &tail[..] } else { &tail[1..] });
    assert!(!host.alias.starts_with('-'));
}

/// The watch refuses what the session refuses and a folder that's empty or holds a NUL; else
/// the folder is only inside the quotes of its remote command, after `--`.
#[hegel::test]
fn the_watch_command_has_each_value_only_in_its_place(tc: TestCase) {
    let host = tc.draw(hosts());
    let folder = tc.draw(awkward_text());
    let built = SshCommand::watch_with(PathBuf::from("ssh"), &host, Vec::new(), &folder);
    let session = SshCommand::sftp_with(PathBuf::from("ssh"), &host, Vec::new(), false);
    assert_eq!(built.is_err(), session.is_err() || folder.is_empty() || folder.contains('\0'));
    let Ok(cmd) = built else { return };
    let args = strings(&cmd.args);
    let rest = after_host_options(&args, BASE_ARGS, &host);
    assert_eq!(rest[..2], [host.alias.clone(), "--".to_owned()]);
    assert_eq!(rest.len(), 3);
    check_remote_command(&rest[2], &folder, |f| watch_script(f).unwrap());
}

/// The terminal's command refuses what the session refuses; else the folder is only inside the
/// quotes of its `cd` (none for a Windows server's folder), and the whole command as one shell
/// line reads back as the program and its arguments.
#[hegel::test]
fn the_terminal_command_has_each_value_only_in_its_place_and_its_shell_line_reads_back(tc: TestCase) {
    let host = tc.draw(hosts());
    let folder = tc.draw(awkward_text());
    let built = SshCommand::terminal_with(PathBuf::from("ssh"), &host, &folder);
    let session = SshCommand::sftp_with(PathBuf::from("ssh"), &host, Vec::new(), false);
    assert_eq!(built.is_err(), session.is_err());
    let Ok(cmd) = built else { return };
    let args = strings(&cmd.args);
    let rest = after_host_options(&args, TERMINAL_ARGS, &host);
    assert_eq!(rest[0], host.alias);
    if is_windows_folder(&folder) {
        assert_eq!(rest.len(), 1);
    } else {
        assert_eq!(rest.len(), 2);
        let shell = r#"exec "$SHELL" -l"#;
        check_remote_command(&rest[1], &folder, |f| format!("cd -- {} && {shell} || {shell}", quote(f)));
    }
    let mut expected = vec!["ssh".to_owned()];
    expected.extend(args);
    assert_eq!(shell_words(&cmd.shell_line()), Ok(expected));
}

#[cfg(unix)]
#[hegel::test(test_cases = 30)] // Each case runs a shell.
fn a_real_shell_reads_the_terminal_commands_shell_line_back(tc: TestCase) {
    let host = tc.draw(hosts());
    let folder = tc.draw(awkward_text());
    tc.assume(!folder.contains('\0'));
    let Ok(cmd) = SshCommand::terminal_with(PathBuf::from("ssh"), &host, &folder) else { return };
    let mut expected = vec!["ssh".to_owned()];
    expected.extend(strings(&cmd.args));
    assert_eq!(real_sh_words(&cmd.shell_line()), expected);
}

/// A config file the checks accept goes in as `-F <path>` right after the base options, and
/// nothing else moves; any other is refused.
#[hegel::test]
fn a_config_file_goes_right_after_the_base_options_or_is_refused(tc: TestCase) {
    let path = tc.draw(awkward_text());
    let host =
        HostSpec { source: HostSource::Saved, hostname: Some("10.0.0.4".into()), ..HostSpec::from_config_alias("box") };
    let cmd = SshCommand::sftp_with(PathBuf::from("ssh"), &host, Vec::new(), false).unwrap();
    let before = strings(&cmd.args);
    match cmd.with_config_file(Path::new(&path)) {
        Err(e) => assert_eq!(Err(e), check_arg("The ssh config file", &path)),
        Ok(cmd) => {
            let n = BASE_ARGS.len();
            let mut expected = before[..n].to_vec();
            expected.extend(["-F".to_owned(), path.clone()]);
            expected.extend_from_slice(&before[n..]);
            assert_eq!(strings(&cmd.args), expected);
        }
    }
}
