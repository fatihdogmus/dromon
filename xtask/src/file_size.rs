//! The file-size limit `cargo lint` checks last: no Rust file of the workspace (under
//! [`ROOTS`]) is longer than [`MAX_LINES`] lines. A file that grows past it is split along a
//! real seam (docs/clean-code.md); there is no list of exceptions. Build output (`target/`)
//! and hidden folders are skipped.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The most lines a `.rs` file may have.
pub const MAX_LINES: usize = 800;

/// The folders of the repository that hold the workspace's Rust code.
pub const ROOTS: &[&str] = &["crates", "xtask"];

/// Checks every `.rs` file under the repository's [`ROOTS`]; prints each that is too long,
/// with its line count, and returns whether there were none.
pub fn check(root: &Path) -> bool {
    let mut long = Vec::new();
    for dir in ROOTS {
        match too_long(&root.join(dir)) {
            Ok(found) => long.extend(found),
            Err(e) => {
                eprintln!("xtask: couldn't check the file sizes: {e}");
                return false;
            }
        }
    }
    if long.is_empty() {
        return true;
    }
    eprintln!("Rust files over {MAX_LINES} lines; split each along a real seam (docs/clean-code.md):");
    for (path, lines) in long {
        eprintln!("  {}: {lines} lines", path.strip_prefix(root).unwrap_or(&path).display());
    }
    false
}

/// Every `.rs` file under `dir` with more than [`MAX_LINES`] lines, with its line count, in
/// path order.
pub fn too_long(dir: &Path) -> io::Result<Vec<(PathBuf, usize)>> {
    let mut out = Vec::new();
    walk(dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, usize)>) -> io::Result<()> {
    let named = |path: &Path, e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", path.display()));
    for entry in fs::read_dir(dir).map_err(|e| named(dir, e))? {
        let entry = entry.map_err(|e| named(dir, e))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type().map_err(|e| named(&path, e))?.is_dir() {
            if name != "target" && !name.starts_with('.') {
                walk(&path, out)?;
            }
        } else if name.ends_with(".rs") {
            let lines = fs::read_to_string(&path).map_err(|e| named(&path, e))?.lines().count();
            if lines > MAX_LINES {
                out.push((path, lines));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder for one test, removed again by [`Scratch`]'s drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("dromon-xtask-{test}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        /// Writes `rel` with `lines` lines.
        fn file(&self, rel: &str, lines: usize) -> PathBuf {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "x\n".repeat(lines)).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_file_of_801_lines_fails_with_its_count() {
        let dir = Scratch::new("801");
        let long = dir.file("crate/src/long.rs", 801);
        assert_eq!(too_long(&dir.0).unwrap(), [(long, 801)]);
    }

    #[test]
    fn a_file_of_800_lines_passes() {
        let dir = Scratch::new("800");
        dir.file("crate/src/full.rs", 800);
        let found = too_long(&dir.0).unwrap();
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn build_output_hidden_folders_and_other_files_are_not_checked() {
        let dir = Scratch::new("skipped");
        dir.file("crate/target/debug/build/out.rs", 900);
        dir.file("crate/.cache/gen.rs", 900);
        dir.file("crate/notes.txt", 900);
        let found = too_long(&dir.0).unwrap();
        assert!(found.is_empty(), "{found:?}");
    }
}
