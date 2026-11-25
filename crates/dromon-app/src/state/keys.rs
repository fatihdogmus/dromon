//! Keyboard shortcuts (§7) as a pure mapping from a key event to what it means. ⌘ on macOS is
//! Ctrl elsewhere: Rune reports either as `Mods::cmd` and `Key::Command(c)`.

use rune::{Key, KeyEvent, KeyPlatform};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Back,
    Forward,
    Up,
    /// Return or ⌘↓: open the selected folder.
    Open,
    SelectAll,
    ToggleHidden,
    Filter,
    /// ⌘D (Ctrl+D elsewhere): download the selection.
    Download,
    /// Escape: close the filter, else clear the selection.
    Escape,
    Move { delta: isize, extend: bool },
    Page { down: bool, extend: bool },
    End { last: bool, extend: bool },
    /// Printable text with no command modifier: type-to-select.
    Type(String),
    /// File operations (§7). Never while a text field has focus.
    File(FileKey),
}

/// The file operations' keys. ⌘V isn't one: pasting goes through the window's paste handler,
/// which reads the OS clipboard (`views/mod.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKey {
    /// ⌘⇧N.
    NewFolder,
    /// F2.
    Rename,
    /// ⌘⌫ on macOS, Delete elsewhere.
    Delete,
    /// ⌘C, ⌘X.
    Copy,
    Cut,
    /// ⌥⌘C.
    CopyPath,
    /// ⌥⌘V: "Move here".
    MoveHere,
}

impl FileKey {
    pub fn action(self) -> super::FileAction {
        use super::FileAction as F;
        match self {
            FileKey::NewFolder => F::NewFolder,
            FileKey::Rename => F::Rename,
            FileKey::Delete => F::Delete,
            FileKey::Copy => F::Copy,
            FileKey::Cut => F::Cut,
            FileKey::CopyPath => F::CopyPath,
            FileKey::MoveHere => F::RequestPaste { as_move: true },
        }
    }
}

/// How many rows Page Up and Page Down move.
pub const PAGE_ROWS: isize = 16;

/// ⌘ plus `c`, also as macOS may report it with ⌥ held (⌥C types “ç”, ⌥V “√”).
fn command(key: &Key, c: char) -> bool {
    let alt_form = match c {
        'c' => 'ç',
        'v' => '√',
        _ => c,
    };
    matches!(key, Key::Command(k) if *k == c || *k == alt_form)
}

pub fn shortcut(ev: &KeyEvent, platform: KeyPlatform) -> Option<Shortcut> {
    let m = ev.mods;
    let mac = matches!(platform, KeyPlatform::Mac);
    let key = &ev.key;
    if command(key, 'n') && m.shift && !m.alt {
        return Some(Shortcut::File(FileKey::NewFolder));
    }
    if command(key, 'c') && m.alt && !m.shift {
        return Some(Shortcut::File(FileKey::CopyPath));
    }
    if command(key, 'v') && m.alt && !m.shift {
        return Some(Shortcut::File(FileKey::MoveHere));
    }
    Some(match &ev.key {
        Key::Command('c') if !m.shift => Shortcut::File(FileKey::Copy),
        Key::Command('x') if !m.shift && !m.alt => Shortcut::File(FileKey::Cut),
        Key::Function(2) if !m.cmd && !m.alt && !m.shift => Shortcut::File(FileKey::Rename),
        Key::Backspace if mac && m.cmd && !m.alt && !m.shift => Shortcut::File(FileKey::Delete),
        Key::Delete if !mac && !m.cmd && !m.alt && !m.shift => Shortcut::File(FileKey::Delete),
        Key::Command('[') => Shortcut::Back,
        Key::Command(']') => Shortcut::Forward,
        Key::Command('a') => Shortcut::SelectAll,
        Key::Command('f') => Shortcut::Filter,
        Key::Command('d') if !m.shift && !m.alt => Shortcut::Download,
        // ⌘⇧. arrives as '.' or, with Shift applied by the layout, '>'.
        Key::Command('.') if m.shift => Shortcut::ToggleHidden,
        Key::Command('>') => Shortcut::ToggleHidden,
        // Ctrl+H, as GNOME Files does.
        Key::Command('h') if matches!(platform, KeyPlatform::Linux) && !m.shift => Shortcut::ToggleHidden,
        Key::Left if m.alt && !mac => Shortcut::Back,
        Key::Right if m.alt && !mac => Shortcut::Forward,
        Key::Up if m.cmd || (m.alt && !mac) => Shortcut::Up,
        Key::Down if m.cmd => Shortcut::Open,
        Key::Enter if !m.cmd && !m.alt => Shortcut::Open,
        Key::Escape => Shortcut::Escape,
        Key::Up => Shortcut::Move { delta: -1, extend: m.shift },
        Key::Down => Shortcut::Move { delta: 1, extend: m.shift },
        Key::PageUp => Shortcut::Page { down: false, extend: m.shift },
        Key::PageDown => Shortcut::Page { down: true, extend: m.shift },
        Key::Home => Shortcut::End { last: false, extend: m.shift },
        Key::End => Shortcut::End { last: true, extend: m.shift },
        Key::Text(t) if !m.cmd && !m.ctrl && !m.alt && !t.trim().is_empty() && t.chars().all(|c| !c.is_control()) => {
            Shortcut::Type(t.clone())
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::Mods;

    fn k(key: Key, mods: Mods) -> Option<Shortcut> {
        shortcut(&KeyEvent { key, mods }, KeyPlatform::Mac)
    }

    fn on(key: Key, mods: Mods, p: KeyPlatform) -> Option<Shortcut> {
        shortcut(&KeyEvent { key, mods }, p)
    }

    const CMD: Mods = Mods { shift: false, cmd: true, alt: false, ctrl: false, logo: true };
    const SHIFT: Mods = Mods { shift: true, cmd: false, alt: false, ctrl: false, logo: false };
    const ALT: Mods = Mods { shift: false, cmd: false, alt: true, ctrl: false, logo: false };
    const NONE: Mods = Mods { shift: false, cmd: false, alt: false, ctrl: false, logo: false };

    #[test]
    fn navigation_chords() {
        assert_eq!(k(Key::Command('['), CMD), Some(Shortcut::Back));
        assert_eq!(k(Key::Command(']'), CMD), Some(Shortcut::Forward));
        assert_eq!(k(Key::Up, CMD), Some(Shortcut::Up));
        assert_eq!(k(Key::Down, CMD), Some(Shortcut::Open));
        assert_eq!(k(Key::Enter, NONE), Some(Shortcut::Open));
        assert_eq!(on(Key::Left, ALT, KeyPlatform::Linux), Some(Shortcut::Back));
        assert_eq!(on(Key::Right, ALT, KeyPlatform::Windows), Some(Shortcut::Forward));
        assert_eq!(on(Key::Up, ALT, KeyPlatform::Windows), Some(Shortcut::Up));
        assert_eq!(k(Key::Left, ALT), None, "⌥← is left to text on macOS");
    }

    #[test]
    fn hidden_files_and_filter() {
        let cmd_shift = Mods { shift: true, ..CMD };
        assert_eq!(k(Key::Command('.'), cmd_shift), Some(Shortcut::ToggleHidden));
        assert_eq!(k(Key::Command('>'), cmd_shift), Some(Shortcut::ToggleHidden));
        assert_eq!(k(Key::Command('.'), CMD), None);
        let ctrl = Mods { ctrl: true, logo: false, ..CMD };
        assert_eq!(on(Key::Command('h'), ctrl, KeyPlatform::Linux), Some(Shortcut::ToggleHidden));
        assert_eq!(k(Key::Command('f'), CMD), Some(Shortcut::Filter));
    }

    #[test]
    fn download_is_cmd_d_everywhere() {
        assert_eq!(k(Key::Command('d'), CMD), Some(Shortcut::Download));
        let ctrl = Mods { ctrl: true, logo: false, ..CMD };
        assert_eq!(on(Key::Command('d'), ctrl, KeyPlatform::Windows), Some(Shortcut::Download));
        assert_eq!(on(Key::Command('d'), ctrl, KeyPlatform::Linux), Some(Shortcut::Download));
        assert_eq!(k(Key::Command('d'), Mods { shift: true, ..CMD }), None, "⌘⇧D is left alone");
        assert_eq!(k(Key::Text("d".into()), NONE), Some(Shortcut::Type("d".into())), "plain d still types");
    }

    #[test]
    fn file_operation_keys_on_each_platform() {
        let f = |k| Some(Shortcut::File(k));
        let ctrl = Mods { ctrl: true, logo: false, ..CMD };
        let shift = |m: Mods| Mods { shift: true, ..m };
        let alt = |m: Mods| Mods { alt: true, ..m };
        for p in [KeyPlatform::Mac, KeyPlatform::Windows, KeyPlatform::Linux] {
            let primary = if p == KeyPlatform::Mac { CMD } else { ctrl };
            assert_eq!(on(Key::Command('n'), shift(primary), p), f(FileKey::NewFolder), "{p:?}");
            assert_eq!(on(Key::Command('c'), primary, p), f(FileKey::Copy));
            assert_eq!(on(Key::Command('x'), primary, p), f(FileKey::Cut));
            assert_eq!(on(Key::Command('c'), alt(primary), p), f(FileKey::CopyPath));
            assert_eq!(on(Key::Command('v'), alt(primary), p), f(FileKey::MoveHere));
            assert_eq!(on(Key::Function(2), NONE, p), f(FileKey::Rename));
            assert_eq!(on(Key::Command('v'), primary, p), None, "⌘V is the paste handler's");
            assert_eq!(on(Key::Command('n'), primary, p), None, "⌘N is a new window's (M4)");
        }
        // Delete: ⌘⌫ on macOS, the Delete key elsewhere.
        assert_eq!(k(Key::Backspace, CMD), f(FileKey::Delete));
        assert_eq!(k(Key::Delete, NONE), None);
        assert_eq!(k(Key::Backspace, NONE), None);
        assert_eq!(on(Key::Delete, NONE, KeyPlatform::Windows), f(FileKey::Delete));
        assert_eq!(on(Key::Delete, NONE, KeyPlatform::Linux), f(FileKey::Delete));
        assert_eq!(on(Key::Backspace, ctrl, KeyPlatform::Linux), None);
        // macOS may name ⌥⌘C and ⌥⌘V by what ⌥ types.
        assert_eq!(k(Key::Command('ç'), alt(CMD)), f(FileKey::CopyPath));
        assert_eq!(k(Key::Command('√'), alt(CMD)), f(FileKey::MoveHere));
    }

    #[test]
    fn arrows_and_typing() {
        assert_eq!(k(Key::Down, NONE), Some(Shortcut::Move { delta: 1, extend: false }));
        assert_eq!(k(Key::Up, SHIFT), Some(Shortcut::Move { delta: -1, extend: true }));
        assert_eq!(k(Key::End, SHIFT), Some(Shortcut::End { last: true, extend: true }));
        assert_eq!(k(Key::Text("r".into()), NONE), Some(Shortcut::Type("r".into())));
        assert_eq!(k(Key::Text("R".into()), SHIFT), Some(Shortcut::Type("R".into())));
        assert_eq!(k(Key::Text(" ".into()), NONE), None);
        assert_eq!(k(Key::Text("r".into()), ALT), None);
    }
}
