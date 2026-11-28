//! Properties of host discovery. Reading any text never panics or loops, whatever its
//! `Include`s name (here every path holds the same text again). For a config made of `Host`,
//! `Match`, `Include`, other keywords and comments, the aliases are the literal names on its
//! `Host` lines and in what it includes, in first-seen order, each once; `Match` lines and other
//! keywords add none. `split_args` reads back tokens written plain or in double quotes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hegel::TestCase;
use hegel::generators::{self as gs, Generator};

use super::*;

/// Files in memory; globs match as `RealFs`'s do.
struct MemFs(BTreeMap<PathBuf, String>);

impl ConfigFs for MemFs {
    fn read_to_string(&self, path: &Path) -> Option<String> {
        self.0.get(path).cloned()
    }

    fn glob(&self, pattern: &str) -> Vec<PathBuf> {
        let Ok(pattern) = glob::Pattern::new(pattern) else { return Vec::new() };
        let options = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        };
        self.0.keys().filter(|p| pattern.matches_path_with(p, options)).cloned().collect()
    }
}

/// Every path holds `text`, and every pattern matches itself: whatever a file includes is read.
struct EchoFs(String);

impl ConfigFs for EchoFs {
    fn read_to_string(&self, _: &Path) -> Option<String> {
        Some(self.0.clone())
    }

    fn glob(&self, pattern: &str) -> Vec<PathBuf> {
        vec![PathBuf::from(pattern)]
    }
}

/// The files `Include` lines can name, and the aliases each defines.
const INCLUDED: &[(&str, &[&str])] = &[("inc-a", &["from-a", "shared"]), ("inc-b", &["from-b"])];

fn included_fs() -> MemFs {
    let files = INCLUDED
        .iter()
        .map(|(name, aliases)| (PathBuf::from(format!("/h/.ssh/{name}")), format!("Host {}\n", aliases.join(" "))));
    MemFs(files.collect())
}

/// Pieces config files hold, odd ones included.
const PIECES: &[&str] = &[
    "Host", "host", "Match", "Include", "inc-*", "=", " ", "\t", "\u{c}", "#", "\"", "'", "\\", "*", "?", "!", "\r",
    "\n", "\r\n", "\u{feff}", "é", "a", "~/", "/",
];

/// Text built from [`PIECES`] and random bits, or any text.
#[hegel::composite]
fn texts(tc: &TestCase) -> String {
    let piece = hegel::one_of!(gs::sampled_from(PIECES).map(|p: &str| p.to_owned()), gs::text().max_size(6));
    tc.draw(hegel::one_of!(gs::vecs(piece).max_size(16).map(|p: Vec<String>| p.concat()), gs::text()))
}

/// A token on a `Host` line: a name, a pattern, or any one-line text.
#[hegel::composite]
fn tokens(tc: &TestCase) -> String {
    let name = gs::from_regex(r"[a-z0-9.é_-]{1,8}");
    let pattern = gs::from_regex(r"!?[a-z0-9.*?-]{0,6}[*?][a-z0-9.]{0,4}");
    let any = gs::text().map(|t: String| t.replace(['\n', '\r'], ""));
    tc.draw(hegel::one_of!(name, pattern, any, gs::sampled_from(vec!["from-a".to_owned(), "shared".to_owned()])))
}

/// `token` as a config file writes it: plain when it has no blank, quote, backslash or control
/// character and doesn't start a comment, else in double quotes with `\` before `"` and `\`.
fn written(token: &str) -> String {
    let plain = !token.is_empty()
        && !token.starts_with('#')
        && !token.contains(|c: char| c.is_whitespace() || c.is_control() || "\"'\\".contains(c));
    if plain { token.to_owned() } else { format!("\"{}\"", token.replace('\\', "\\\\").replace('"', "\\\"")) }
}

/// One line of a generated config, and the aliases it adds (before de-duplication).
#[hegel::composite]
fn lines(tc: &TestCase) -> (String, Vec<String>) {
    let indent = tc.draw(gs::sampled_from(vec!["", "  ", "\t"]));
    let sep = tc.draw(gs::sampled_from(vec![" ", "=", " = ", "\t"]));
    match tc.draw(gs::integers::<u8>().max_value(4)) {
        0 => {
            let keyword = tc.draw(gs::sampled_from(vec!["Host", "host", "HOST"]));
            let names = tc.draw(gs::vecs(tokens()).max_size(4));
            let comment = if tc.draw(gs::booleans()) { " # Host hidden" } else { "" };
            let line = names.iter().map(|n| written(n)).collect::<Vec<_>>().join(" ");
            let literal = names.into_iter().filter(|n| !n.is_empty() && !n.contains(['*', '?', '!'])).collect();
            (format!("{indent}{keyword}{sep}{line}{comment}"), literal)
        }
        1 => (format!("{indent}Match{sep}host {} exec \"true\"", written(&tc.draw(tokens()))), Vec::new()),
        2 => {
            let (name, aliases) = tc.draw(gs::sampled_from(INCLUDED.to_vec()));
            let arg = tc.draw(gs::sampled_from(vec![name.to_owned(), "inc-*".to_owned(), "missing".to_owned()]));
            let adds: Vec<String> = match arg.as_str() {
                "inc-*" => INCLUDED.iter().flat_map(|(_, a)| a.iter()).map(|a| (*a).to_owned()).collect(),
                "missing" => Vec::new(),
                _ => aliases.iter().map(|a| (*a).to_owned()).collect(),
            };
            (format!("{indent}Include{sep}{arg}"), adds)
        }
        3 => {
            let keyword = tc.draw(gs::sampled_from(vec!["HostName", "Hostname", "User", "HostKeyAlias", "ProxyJump"]));
            (format!("{indent}{keyword}{sep}{}", written(&tc.draw(tokens()))), Vec::new())
        }
        _ => (format!("{indent}# {}", tc.draw(texts()).replace(['\n', '\r'], " ")), Vec::new()),
    }
}

#[hegel::test]
fn reading_any_text_never_panics_and_ends_whatever_it_includes(tc: TestCase) {
    let text = tc.draw(texts());
    let fs = EchoFs(text.clone());
    for home in [Some(Path::new("/h")), None] {
        let ctx = ParseContext { include_dir: Path::new("/h/.ssh"), home, fs: &fs };
        let found = parse_host_aliases(&text, &ctx);
        assert!(found.aliases.iter().all(|a| is_literal_alias(a)), "{:?}", found.aliases);
        assert!(found.files_read.len() <= text.len() + 1);
    }
}

#[hegel::test]
fn the_aliases_are_the_literal_host_names_in_first_seen_order(tc: TestCase) {
    let lines = tc.draw(gs::vecs(lines()).max_size(10));
    let eol = tc.draw(gs::sampled_from(vec!["\n", "\r\n"]));
    let text = lines.iter().map(|(line, _)| line.as_str()).collect::<Vec<_>>().join(eol);
    let mut expected: Vec<String> = Vec::new();
    for alias in lines.into_iter().flat_map(|(_, adds)| adds) {
        if !expected.contains(&alias) {
            expected.push(alias);
        }
    }
    let fs = included_fs();
    let ctx = ParseContext { include_dir: Path::new("/h/.ssh"), home: Some(Path::new("/h")), fs: &fs };
    assert_eq!(parse_host_aliases(&text, &ctx).aliases, expected, "{text:?}");
}

#[hegel::test]
fn split_args_reads_back_tokens_written_plain_or_quoted(tc: TestCase) {
    let tokens = tc.draw(gs::vecs(tokens()).max_size(6));
    let blanks = tc.draw(gs::sampled_from(vec![" ", "\t", "  \t "]));
    let line = tokens.iter().map(|t| written(t)).collect::<Vec<_>>().join(blanks);
    assert_eq!(split_args(&line), tokens, "{line:?}");
}
