//! What the fixture's files hold when they're previewed (`Command::Preview`): content made up
//! from each file's name and size, the same on every run, so the panel has something to show
//! (a fixture lists files, it doesn't hold them).
//!
//! - Images (by name, as the app previews them: png, jpeg, gif, webp, avif, svg, ico) hold a
//!   small generated picture, an SVG whatever the name says: a sky, a sun and hills in colours
//!   picked by the name (an icon: a badge, 64 × 64). Their size stays the listing's.
//! - Files the app doesn't read as text (archives, media, programs, by name) and `.DS_Store`
//!   hold bytes with NULs in them.
//! - Everything else is text of exactly its listed size: lines of [`LINE`] bytes made up by
//!   kind (log lines, JSON, HTML, Markdown, code, prose), so any range of a file is the bytes
//!   that range of the whole file would be: a head and a log's tail both look real.

use dromon_core::{Entry, Error, RemotePath};

use crate::state::PreviewRange;
use crate::state::preview::{Plan, plan};

/// The length of a generated text line, its line break included.
pub const LINE: u64 = 64;

/// Reads `range` of the file `entry` at `path` lists (`None`: there's none): its bytes, and its
/// size.
pub fn read(entry: Option<Entry>, path: &RemotePath, range: PreviewRange) -> Result<(Vec<u8>, u64), Error> {
    let e = entry.ok_or_else(|| Error::NotFound(path.clone()))?;
    if e.kind.is_dir_like() {
        return Err(Error::Other(format!("{path} is a folder")));
    }
    let folder = path.parent().unwrap_or_else(RemotePath::root);
    let (offset, len) = range.span(e.size);
    let bytes = match plan(&folder, &e) {
        Plan::Image | Plan::TooLarge => picture(&e.name).into_bytes(),
        Plan::Unsupported => binary(offset, len),
        Plan::Text(_) if e.name == ".DS_Store" => binary(offset, len),
        Plan::Text(_) => text(&folder, &e.name, offset, len),
    };
    Ok((bytes, e.size))
}

/// `len` bytes from `offset` of a binary file: a NUL every seventh byte.
fn binary(offset: u64, len: u64) -> Vec<u8> {
    (offset..offset + len).map(|x| if x % 7 == 0 { 0 } else { (x * 131 % 251) as u8 }).collect()
}

/// `len` bytes from `offset` of the text file `name` in `folder`.
fn text(folder: &RemotePath, name: &str, offset: u64, len: u64) -> Vec<u8> {
    let kind = Kind::of(folder, name);
    let mut out = Vec::with_capacity(len as usize);
    let (mut i, mut skip) = (offset / LINE, (offset % LINE) as usize);
    while (out.len() as u64) < len {
        let line = kind.line(name, i);
        let rest = &line.as_bytes()[skip..];
        let need = (len - out.len() as u64) as usize;
        out.extend_from_slice(&rest[..rest.len().min(need)]);
        (i, skip) = (i + 1, 0);
    }
    out
}

/// What a text file's lines are like.
#[derive(Clone, Copy)]
enum Kind {
    Log,
    Json,
    Html,
    Markdown,
    Code,
    Prose,
}

impl Kind {
    fn of(folder: &RemotePath, name: &str) -> Kind {
        if crate::state::preview::is_log(folder, name) {
            return Kind::Log;
        }
        match name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).as_deref() {
            Some("json" | "lock") => Kind::Json,
            Some("html" | "htm" | "xml") => Kind::Html,
            Some("md") => Kind::Markdown,
            Some("js" | "ts" | "rs" | "py" | "sh" | "go" | "rb" | "php" | "css") => Kind::Code,
            _ => Kind::Prose,
        }
    }

    /// Line `i` (from 0) of the file `name`: exactly [`LINE`] bytes, a line break last.
    fn line(self, name: &str, i: u64) -> String {
        let n = i + 1;
        let mut s = match self {
            Kind::Log => {
                let t = i * 7 % 86_400;
                let level = ["INFO ", "INFO ", "INFO ", "WARN ", "INFO ", "ERROR"][(i % 6) as usize];
                let (status, ms) = ([200, 200, 304, 404, 200, 500][(i % 6) as usize], i * 37 % 900);
                let at = format!("2026-11-25 {:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60);
                format!("{at} {level} GET /assets/img-{:03}.png {status} {ms}ms", i % 600)
            }
            Kind::Json if i == 0 => "{".to_owned(),
            Kind::Json => format!("  \"package-{n:05}\": {{ \"version\": \"{}.{}.{}\" }},", n % 9, n % 31, n % 17),
            Kind::Html if i == 0 => "<!doctype html>".to_owned(),
            Kind::Html => format!("  <p class=\"row-{n}\">Section {n} of {name}</p>"),
            Kind::Markdown if i == 0 => format!("# {name}"),
            Kind::Markdown => format!("Paragraph {n}: notes on deploying the site, step {}.", n % 12 + 1),
            Kind::Code => format!("const value{n} = compute({n}, \"{}\");", n % 97),
            Kind::Prose => format!("{name}, line {n}: the quick brown fox jumps over"),
        };
        // ASCII only, so bytes are characters: cut or padded to the line's length.
        s.retain(|c| c.is_ascii() && c != '\n');
        s.truncate(LINE as usize - 1);
        while s.len() < LINE as usize - 1 {
            s.push(' ');
        }
        s.push('\n');
        s
    }
}

/// The generated picture for the image `name`: an SVG document.
fn picture(name: &str) -> String {
    const SKIES: [(&str, &str, &str, &str); 4] = [
        ("#1d3b5c", "#e9a46a", "#f6d68b", "#27403a"),
        ("#3a2a5e", "#d98aa6", "#ffd3a1", "#2c3450"),
        ("#0f4c5c", "#8ad0c4", "#fff1b8", "#1f3b2d"),
        ("#4a2c2a", "#f0b07a", "#ffe3a3", "#3b2f26"),
    ];
    let pick = name.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(usize::from(b))) % SKIES.len();
    let (top, bottom, sun, hill) = SKIES[pick];
    let is_icon = name.to_ascii_lowercase().ends_with(".ico");
    if is_icon {
        return format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\">\
             <rect x=\"4\" y=\"4\" width=\"56\" height=\"56\" rx=\"14\" fill=\"{top}\"/>\
             <circle cx=\"32\" cy=\"32\" r=\"14\" fill=\"{sun}\"/></svg>"
        );
    }
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"960\" height=\"640\">\
         <defs><linearGradient id=\"sky\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\">\
         <stop offset=\"0\" stop-color=\"{top}\"/><stop offset=\"1\" stop-color=\"{bottom}\"/>\
         </linearGradient></defs>\
         <rect width=\"960\" height=\"640\" fill=\"url(#sky)\"/>\
         <circle cx=\"640\" cy=\"300\" r=\"90\" fill=\"{sun}\"/>\
         <path d=\"M0 470 Q 240 360 480 450 T 960 420 V 640 H 0 Z\" fill=\"{hill}\" opacity=\"0.85\"/>\
         <path d=\"M0 540 Q 300 470 620 540 T 960 520 V 640 H 0 Z\" fill=\"{hill}\"/></svg>"
    )
}

#[cfg(test)]
mod tests {
    use dromon_core::EntryKind;

    use super::*;
    use crate::state::preview::TEXT_LIMIT;

    fn file(name: &str, size: u64) -> Option<Entry> {
        Some(Entry {
            name: name.into(),
            kind: EntryKind::File,
            size,
            mtime: None,
            mode: None,
            owner: None,
            group: None,
        })
    }

    #[test]
    fn a_text_file_is_its_listed_size_and_any_range_is_that_range_of_the_whole() {
        let path = RemotePath::new("/var/www/site/server.log");
        let size = 10_000;
        let (whole, total) = read(file("server.log", size), &path, PreviewRange::Head(size)).unwrap();
        assert_eq!((whole.len() as u64, total), (size, size));
        let (tail, _) = read(file("server.log", size), &path, PreviewRange::Tail(1_000)).unwrap();
        assert_eq!(tail, whole[9_000..]);
        let (head, _) = read(file("server.log", size), &path, PreviewRange::Head(1_000)).unwrap();
        assert_eq!(head, whole[..1_000]);
        assert!(whole.split(|b| *b == b'\n').all(|l| l.len() < LINE as usize), "lines of LINE bytes");
        assert!(std::str::from_utf8(&whole).unwrap().starts_with("2026-11-25 00:00:00 INFO  GET /assets/img-000.png"));
    }

    #[test]
    fn images_are_a_picture_and_binaries_have_nuls() {
        let (svg, total) =
            read(file("IMG_00042.jpg", 3_400_000), &RemotePath::new("/p/IMG_00042.jpg"), PreviewRange::Head(1))
                .unwrap();
        assert!(svg.starts_with(b"<svg") && total == 3_400_000, "the listed size stays");
        let path = RemotePath::new("/v/.DS_Store");
        let (bytes, _) = read(file(".DS_Store", 6_148), &path, PreviewRange::Head(TEXT_LIMIT)).unwrap();
        assert_eq!(bytes.len(), 6_148);
        assert!(bytes.contains(&0));
        let missing = read(None, &path, PreviewRange::Head(1));
        assert_eq!(missing, Err(Error::NotFound(path)));
    }
}
