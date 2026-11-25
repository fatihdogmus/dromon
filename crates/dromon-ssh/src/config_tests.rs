use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::*;

/// An in-memory file system: path → contents. Globs are matched with the same options as
/// `RealFs`.
#[derive(Default)]
struct MemFs {
    files: BTreeMap<PathBuf, String>,
}

impl MemFs {
    fn with(mut self, path: &str, text: &str) -> Self {
        self.files.insert(PathBuf::from(path), text.to_owned());
        self
    }
}

impl ConfigFs for MemFs {
    fn read_to_string(&self, path: &Path) -> Option<String> {
        self.files.get(path).cloned()
    }

    fn glob(&self, pattern: &str) -> Vec<PathBuf> {
        let Ok(pattern) = glob::Pattern::new(pattern) else { return Vec::new() };
        let options = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        };
        // Reverse order on purpose: the parser must sort.
        self.files.keys().rev().filter(|p| pattern.matches_path_with(p, options)).cloned().collect()
    }
}

fn user_sources() -> ConfigSources {
    ConfigSources {
        user: Some(PathBuf::from("/home/u/.ssh/config")),
        system: Some(PathBuf::from("/etc/ssh/ssh_config")),
        user_ssh_dir: PathBuf::from("/home/u/.ssh"),
    }
}

fn aliases_of(text: &str) -> Vec<String> {
    let fs = MemFs::default();
    let ctx = ParseContext { include_dir: Path::new("/home/u/.ssh"), home: Some(Path::new("/home/u")), fs: &fs };
    parse_host_aliases(text, &ctx).aliases
}

fn discover(fs: &MemFs) -> Discovery {
    discover_aliases_with(&user_sources(), fs)
}

#[test]
fn reads_plain_host_lines() {
    let text = "Host prod\n  HostName 10.0.0.4\n  User deploy\n\nHost staging\n";
    assert_eq!(aliases_of(text), ["prod", "staging"]);
}

#[test]
fn several_aliases_per_line() {
    assert_eq!(aliases_of("Host a b\tc\n"), ["a", "b", "c"]);
}

#[test]
fn keywords_are_case_insensitive() {
    assert_eq!(aliases_of("HOST a\nhost b\nhOsT c\n"), ["a", "b", "c"]);
}

#[test]
fn equals_form() {
    assert_eq!(aliases_of("Host=a\nHost = b\nHost =c d\n  Host\t=\te\n"), ["a", "b", "c", "d", "e"]);
}

#[test]
fn comments_are_skipped() {
    let text = "# Host commented\n  # Host also\nHost real # Host trailing\nHost x #y\n";
    assert_eq!(aliases_of(text), ["real", "x"]);
}

#[test]
fn hash_inside_a_token_is_not_a_comment() {
    assert_eq!(aliases_of("Host a#b\n"), ["a#b"]);
}

#[test]
fn double_and_single_quotes() {
    assert_eq!(aliases_of("Host \"quoted\" 'single' \"with space\"\n"), ["quoted", "single", "with space"]);
}

#[test]
fn backslash_escapes() {
    assert_eq!(aliases_of(r#"Host a\ b "c\"d" e\f"#), ["a b", "c\"d", r"e\f"]);
}

#[test]
fn unterminated_quote_drops_the_line() {
    assert_eq!(aliases_of("Host \"open a\nHost ok\n"), ["ok"]);
}

#[test]
fn patterns_and_negations_are_skipped() {
    let text = "Host *\nHost *.example.com web?\nHost db !db-old literal\nHost !bad\n";
    assert_eq!(aliases_of(text), ["db", "literal"]);
}

#[test]
fn match_blocks_contribute_no_aliases() {
    let text = "Match host secret exec \"true\"\n  User x\nMatch all\nHost after\n";
    assert_eq!(aliases_of(text), ["after"]);
}

#[test]
fn other_keywords_are_ignored() {
    let text = "HostName notalias\nHostKeyAlias nope\nUser x\nHost yes\n";
    assert_eq!(aliases_of(text), ["yes"]);
}

#[test]
fn duplicates_keep_first_seen_order() {
    assert_eq!(aliases_of("Host b a\nHost a c b\n"), ["b", "a", "c"]);
}

#[test]
fn crlf_and_bom() {
    assert_eq!(aliases_of("\u{feff}Host a\r\nHost b\r\n"), ["a", "b"]);
}

#[test]
fn empty_and_garbage_input_never_panic() {
    assert!(aliases_of("").is_empty());
    assert!(aliases_of("Host\nHost   \nHost=\n=\n\"\n\\\n").is_empty());
}

#[test]
fn relative_include_resolves_against_user_ssh_dir() {
    let fs = MemFs::default()
        .with("/home/u/.ssh/config", "Host top\nInclude extra\n")
        .with("/home/u/.ssh/extra", "Host included\n");
    let d = discover(&fs);
    assert_eq!(d.aliases, ["top", "included"]);
    assert_eq!(d.files_read, [PathBuf::from("/home/u/.ssh/config"), PathBuf::from("/home/u/.ssh/extra")]);
}

#[test]
fn nested_relative_include_still_resolves_against_user_ssh_dir() {
    let fs = MemFs::default()
        .with("/home/u/.ssh/config", "Include conf.d/o*\n")
        .with("/home/u/.ssh/conf.d/one", "Host one\nInclude two\n")
        .with("/home/u/.ssh/two", "Host two\n")
        .with("/home/u/.ssh/conf.d/two", "Host wrong\n");
    assert_eq!(discover(&fs).aliases, ["one", "two"]);
}

#[test]
fn glob_matches_are_read_in_lexical_order() {
    let fs = MemFs::default()
        .with("/home/u/.ssh/config", "Include config.d/*.conf\nHost last\n")
        .with("/home/u/.ssh/config.d/b.conf", "Host b\n")
        .with("/home/u/.ssh/config.d/a.conf", "Host a\n")
        .with("/home/u/.ssh/config.d/c.conf", "Host c\n")
        .with("/home/u/.ssh/config.d/skip.txt", "Host skipped\n")
        .with("/home/u/.ssh/config.d/.hidden.conf", "Host hidden\n");
    assert_eq!(discover(&fs).aliases, ["a", "b", "c", "last"]);
}

#[test]
fn include_sits_where_it_is_written() {
    let fs = MemFs::default()
        .with("/home/u/.ssh/config", "Host first\nInclude x\nHost third\n")
        .with("/home/u/.ssh/x", "Host second\n");
    assert_eq!(discover(&fs).aliases, ["first", "second", "third"]);
}

#[test]
fn several_paths_on_one_include_line() {
    let fs = MemFs::default()
        .with("/home/u/.ssh/config", "Include b a \"c\"\n")
        .with("/home/u/.ssh/a", "Host a\n")
        .with("/home/u/.ssh/b", "Host b\n")
        .with("/home/u/.ssh/c", "Host c\n");
    assert_eq!(discover(&fs).aliases, ["b", "a", "c"]);
}

#[test]
fn tilde_and_absolute_includes() {
    let fs = MemFs::default()
        .with("/home/u/.ssh/config", "Include ~/work/ssh.conf\nInclude /opt/ssh/*.conf\nInclude=~/.ssh/eq\n")
        .with("/home/u/work/ssh.conf", "Host work\n")
        .with("/opt/ssh/z.conf", "Host opt\n")
        .with("/home/u/.ssh/eq", "Host eq\n");
    assert_eq!(discover(&fs).aliases, ["work", "opt", "eq"]);
}

#[test]
fn tilde_user_form_is_not_followed() {
    let fs = MemFs::default()
        .with("/home/u/.ssh/config", "Include ~other/x\n")
        .with("/home/u/.ssh/~other/x", "Host nope\n");
    assert!(discover(&fs).aliases.is_empty());
}

#[test]
fn include_inside_host_and_match_blocks_is_followed() {
    let fs = MemFs::default()
        .with("/home/u/.ssh/config", "Host *\n  Include a\nMatch exec \"x\"\n  Include b\n")
        .with("/home/u/.ssh/a", "Host a\n")
        .with("/home/u/.ssh/b", "Host b\n");
    assert_eq!(discover(&fs).aliases, ["a", "b"]);
}

#[test]
fn system_config_resolves_relative_includes_against_its_folder() {
    let fs = MemFs::default()
        .with("/etc/ssh/ssh_config", "Include ssh_config.d/*.conf\nInclude ~/.ssh/sneaky\n")
        .with("/etc/ssh/ssh_config.d/50-site.conf", "Host site\n")
        .with("/home/u/.ssh/sneaky", "Host sneaky\n");
    let src = ConfigSources { user: None, ..user_sources() };
    assert_eq!(discover_aliases_with(&src, &fs).aliases, ["site"]);
}

#[test]
fn user_aliases_come_before_system_ones_and_are_deduplicated() {
    let fs = MemFs::default()
        .with("/home/u/.ssh/config", "Host shared mine\n")
        .with("/etc/ssh/ssh_config", "Host sys shared\n");
    assert_eq!(discover(&fs).aliases, ["shared", "mine", "sys"]);
}

#[test]
fn missing_files_are_skipped() {
    let fs = MemFs::default().with("/home/u/.ssh/config", "Include missing nothing/*\nHost ok\n");
    let d = discover(&fs);
    assert_eq!(d.aliases, ["ok"]);
    assert_eq!(d.files_read, [PathBuf::from("/home/u/.ssh/config")]);
    assert_eq!(discover(&MemFs::default()), Discovery::default());
}

#[test]
fn cycles_are_safe() {
    let fs = MemFs::default()
        .with("/home/u/.ssh/config", "Host top\nInclude a\n")
        .with("/home/u/.ssh/a", "Host a\nInclude b\n")
        .with("/home/u/.ssh/b", "Host b\nInclude a\nInclude config\n");
    let d = discover(&fs);
    assert_eq!(d.aliases, ["top", "a", "b"]);
    assert_eq!(d.files_read.len(), 3);
}

#[test]
fn self_include_is_safe() {
    let fs = MemFs::default().with("/home/u/.ssh/config", "Host me\nInclude config\nInclude *\n");
    assert_eq!(discover(&fs).aliases, ["me"]);
}

#[test]
fn depth_limit_matches_ssh() {
    // f0 includes f1 includes f2 ... f20. ssh reads depths 0..=16.
    let mut fs = MemFs::default().with("/home/u/.ssh/config", "Host h0\nInclude f1\n");
    for i in 1..=20 {
        fs = fs.with(&format!("/home/u/.ssh/f{i}"), &format!("Host h{i}\nInclude f{}\n", i + 1));
    }
    let d = discover(&fs);
    let expected: Vec<String> = (0..=MAX_INCLUDE_DEPTH).map(|i| format!("h{i}")).collect();
    assert_eq!(d.aliases, expected);
    assert_eq!(d.files_read.len(), MAX_INCLUDE_DEPTH + 1);
}

#[test]
fn glob_metacharacters_in_the_base_dir_are_literal() {
    let fs = MemFs::default()
        .with("/home/[u]/.ssh/config", "Include x\n")
        .with("/home/[u]/.ssh/x", "Host bracket\n")
        .with("/home/u/.ssh/x", "Host wrong\n");
    let src = ConfigSources {
        user: Some(PathBuf::from("/home/[u]/.ssh/config")),
        system: None,
        user_ssh_dir: PathBuf::from("/home/[u]/.ssh"),
    };
    assert_eq!(discover_aliases_with(&src, &fs).aliases, ["bracket"]);
}

#[test]
fn parse_host_aliases_reports_only_included_files() {
    let fs = MemFs::default().with("/home/u/.ssh/inc", "Host inc\n");
    let ctx = ParseContext { include_dir: Path::new("/home/u/.ssh"), home: Some(Path::new("/home/u")), fs: &fs };
    let d = parse_host_aliases("Host a\nInclude inc\n", &ctx);
    assert_eq!(d.aliases, ["a", "inc"]);
    assert_eq!(d.files_read, [PathBuf::from("/home/u/.ssh/inc")]);
}

#[test]
fn split_keyword_forms() {
    assert_eq!(split_keyword("Host a"), Some(("Host", "a")));
    assert_eq!(split_keyword("  Host=a  "), Some(("Host", "a")));
    assert_eq!(split_keyword("Host = a b"), Some(("Host", "a b")));
    assert_eq!(split_keyword("Host"), Some(("Host", "")));
    assert_eq!(split_keyword("   "), None);
    assert_eq!(split_keyword("#Host a"), None);
}

#[cfg(unix)]
#[test]
fn real_fs_discovery_with_includes() {
    let tmp = tempfile::tempdir().unwrap();
    let ssh_dir = tmp.path().join(".ssh");
    std::fs::create_dir_all(ssh_dir.join("config.d")).unwrap();
    std::fs::write(ssh_dir.join("config"), "Host top\nInclude config.d/*\nInclude ~/.ssh/tilde\n").unwrap();
    std::fs::write(ssh_dir.join("config.d/20-b"), "Host b\n").unwrap();
    std::fs::write(ssh_dir.join("config.d/10-a"), "Host a\n").unwrap();
    std::fs::create_dir(ssh_dir.join("config.d/30-dir")).unwrap();
    std::fs::write(ssh_dir.join("tilde"), "Host tilde\n").unwrap();
    let src = ConfigSources { user: Some(ssh_dir.join("config")), system: None, user_ssh_dir: ssh_dir.clone() };
    let d = discover_aliases(&src);
    assert_eq!(d.aliases, ["top", "a", "b", "tilde"]);
    assert_eq!(
        d.files_read,
        [ssh_dir.join("config"), ssh_dir.join("config.d/10-a"), ssh_dir.join("config.d/20-b"), ssh_dir.join("tilde")]
    );
}

#[cfg(unix)]
#[test]
fn unreadable_file_is_skipped() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let ssh_dir = tmp.path().join(".ssh");
    std::fs::create_dir_all(&ssh_dir).unwrap();
    std::fs::write(ssh_dir.join("config"), "Include locked\nInclude bad-utf8\nHost ok\n").unwrap();
    std::fs::write(ssh_dir.join("locked"), "Host locked\n").unwrap();
    std::fs::set_permissions(ssh_dir.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
    std::fs::write(ssh_dir.join("bad-utf8"), b"Host \xff\xfe\n").unwrap();
    let src = ConfigSources { user: Some(ssh_dir.join("config")), system: None, user_ssh_dir: ssh_dir.clone() };
    let d = discover_aliases(&src);
    // Root can read 0o000 files, so only assert what holds either way.
    assert!(d.aliases.contains(&"ok".to_owned()));
    assert!(!d.files_read.contains(&ssh_dir.join("bad-utf8")));
}
