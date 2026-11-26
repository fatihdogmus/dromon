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
    /// ⌘U (Ctrl+U elsewhere): pick files to upload into the folder on screen.
    Upload,
    /// Escape: close the filter, else clear the selection.
    Escape,
    Move { delta: isize, extend: bool },
    Page { down: bool, extend: bool },
    End { last: bool, extend: bool },
    /// Printable text with no command modifier: type-to-select.
    Type(String),
    /// File operations (§7). Never while a text field has focus.
    File(FileKey),
    /// ⌘⇧K (Ctrl+Shift+K elsewhere): the quick-connect popover.
    QuickConnect,
    /// ⌘K (Ctrl+K elsewhere): the command palette. Works while a text field has focus.
    Palette,
    /// ⌘L: the palette's "Go to folder…" on the folder on screen.
    GoToPath,
    /// ⌘1 … ⌘9: the sidebar's host 1 to 9, as shown (`App::sidebar_order`).
    Host(usize),
    /// ⌘⇧J: the transfers popover.
    ShowTransfers,
    /// ⌘, (Ctrl+, elsewhere): the Settings page.
    Settings,
    /// ⌘N (Ctrl+N elsewhere): another main window.
    NewWindow,
    /// ⌘W (Ctrl+W elsewhere): close this window.
    CloseWindow,
    /// Shift+F10 or the Menu key: the context menu of the cursor row. Never while a text field
    /// has focus.
    ContextMenu,
}

/// The chord spec of ⌘1 … ⌘9 for the sidebar's host `n` (1-based), for hints.
pub fn host_chord(n: usize) -> Option<&'static str> {
    const CHORDS: [&str; 9] = ["mod+1", "mod+2", "mod+3", "mod+4", "mod+5", "mod+6", "mod+7", "mod+8", "mod+9"];
    n.checked_sub(1).and_then(|i| CHORDS.get(i)).copied()
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
    /// ⌘R, and F5 on Windows and Linux: list the folder on screen again.
    Refresh,
    /// ⌘I on macOS, Alt+Return on Windows and Linux: the selected row's Properties.
    Properties,
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
            FileKey::Refresh => F::Refresh,
            FileKey::Properties => F::Properties,
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
    // Shift may arrive applied (`K`) or not (`k`).
    if (command(key, 'k') || command(key, 'K')) && m.shift && !m.alt {
        return Some(Shortcut::QuickConnect);
    }
    if (command(key, 'j') || command(key, 'J')) && m.shift && !m.alt {
        return Some(Shortcut::ShowTransfers);
    }
    if !m.shift && !m.alt {
        if command(key, 'k') {
            return Some(Shortcut::Palette);
        }
        if command(key, 'l') {
            return Some(Shortcut::GoToPath);
        }
        if let Key::Command(c @ '1'..='9') = key {
            return Some(Shortcut::Host(*c as usize - '0' as usize));
        }
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
        Key::Command('r') if !m.shift && !m.alt => Shortcut::File(FileKey::Refresh),
        Key::Function(5) if !mac && !m.cmd && !m.alt && !m.shift => Shortcut::File(FileKey::Refresh),
        Key::Backspace if mac && m.cmd && !m.alt && !m.shift => Shortcut::File(FileKey::Delete),
        Key::Delete if !mac && !m.cmd && !m.alt && !m.shift => Shortcut::File(FileKey::Delete),
        Key::Command('[') => Shortcut::Back,
        Key::Command(']') => Shortcut::Forward,
        Key::Command('a') => Shortcut::SelectAll,
        Key::Command('f') => Shortcut::Filter,
        Key::Command('d') if !m.shift && !m.alt => Shortcut::Download,
        Key::Command('u') if !m.shift && !m.alt => Shortcut::Upload,
        Key::Command(',') if !m.shift && !m.alt => Shortcut::Settings,
        Key::Command('n') if !m.shift && !m.alt => Shortcut::NewWindow,
        Key::Command('w') if !m.shift && !m.alt => Shortcut::CloseWindow,
        // ⌘⇧. arrives as '.' or, with Shift applied by the layout, '>'.
        Key::Command('.') if m.shift => Shortcut::ToggleHidden,
        Key::Command('>') => Shortcut::ToggleHidden,
        // Ctrl+H, as GNOME Files does.
        Key::Command('h') if matches!(platform, KeyPlatform::Linux) && !m.shift => Shortcut::ToggleHidden,
        Key::Left if m.alt && !mac => Shortcut::Back,
        Key::Right if m.alt && !mac => Shortcut::Forward,
        Key::Up if m.cmd || (m.alt && !mac) => Shortcut::Up,
        Key::Down if m.cmd => Shortcut::Open,
        Key::Command('i') if mac && !m.shift && !m.alt => Shortcut::File(FileKey::Properties),
        Key::Enter if !mac && m.alt && !m.cmd && !m.shift => Shortcut::File(FileKey::Properties),
        Key::Enter if !m.cmd && !m.alt => Shortcut::Open,
        Key::Escape => Shortcut::Escape,
        Key::ContextMenu if !m.cmd && !m.alt => Shortcut::ContextMenu,
        Key::Function(10) if m.shift && !m.cmd && !m.alt => Shortcut::ContextMenu,
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
    fn upload_is_cmd_u_everywhere() {
        assert_eq!(k(Key::Command('u'), CMD), Some(Shortcut::Upload));
        let ctrl = Mods { ctrl: true, logo: false, ..CMD };
        assert_eq!(on(Key::Command('u'), ctrl, KeyPlatform::Windows), Some(Shortcut::Upload));
        assert_eq!(on(Key::Command('u'), ctrl, KeyPlatform::Linux), Some(Shortcut::Upload));
        assert_eq!(k(Key::Command('u'), Mods { shift: true, ..CMD }), None, "⌘⇧U is left alone");
        assert_eq!(k(Key::Text("u".into()), NONE), Some(Shortcut::Type("u".into())), "plain u still types");
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
            assert_eq!(on(Key::Command('r'), primary, p), f(FileKey::Refresh));
            let f5 = if p == KeyPlatform::Mac { None } else { f(FileKey::Refresh) };
            assert_eq!(on(Key::Function(5), NONE, p), f5, "F5 refreshes on Windows and Linux");
            assert_eq!(on(Key::Command('v'), primary, p), None, "⌘V is the paste handler's");
            assert_eq!(on(Key::Command('n'), primary, p), Some(Shortcut::NewWindow), "{p:?}");
            assert_eq!(on(Key::Command('w'), primary, p), Some(Shortcut::CloseWindow), "{p:?}");
            assert_eq!(on(Key::Command('w'), shift(primary), p), None, "⌘⇧W is left alone");
            assert_eq!(on(Key::Command('n'), alt(primary), p), None);
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
    fn properties_is_cmd_i_on_macos_and_alt_return_elsewhere() {
        let props = Some(Shortcut::File(FileKey::Properties));
        let ctrl = Mods { ctrl: true, logo: false, ..CMD };
        assert_eq!(k(Key::Command('i'), CMD), props);
        assert_eq!(k(Key::Command('i'), Mods { shift: true, ..CMD }), None);
        assert_eq!(k(Key::Enter, ALT), None, "⌥↩ is nothing on macOS");
        for p in [KeyPlatform::Windows, KeyPlatform::Linux] {
            assert_eq!(on(Key::Enter, ALT, p), props, "{p:?}");
            assert_eq!(on(Key::Command('i'), ctrl, p), None, "{p:?}: Ctrl+I is nothing");
            assert_eq!(on(Key::Enter, Mods { shift: true, ..ALT }, p), None);
            assert_eq!(on(Key::Enter, NONE, p), Some(Shortcut::Open), "plain Return still opens");
        }
        assert_eq!(FileKey::Properties.action(), crate::state::FileAction::Properties);
    }

    #[test]
    fn quick_connect_is_cmd_shift_k() {
        let ctrl = Mods { ctrl: true, logo: false, ..CMD };
        let shift = |m: Mods| Mods { shift: true, ..m };
        assert_eq!(k(Key::Command('k'), shift(CMD)), Some(Shortcut::QuickConnect));
        assert_eq!(k(Key::Command('K'), shift(CMD)), Some(Shortcut::QuickConnect));
        assert_eq!(on(Key::Command('k'), shift(ctrl), KeyPlatform::Windows), Some(Shortcut::QuickConnect));
        assert_eq!(on(Key::Command('k'), shift(ctrl), KeyPlatform::Linux), Some(Shortcut::QuickConnect));
        assert_eq!(k(Key::Command('k'), CMD), Some(Shortcut::Palette), "⌘K is the palette's");
    }

    #[test]
    fn palette_go_to_path_hosts_and_transfers_on_each_platform() {
        let ctrl = Mods { ctrl: true, logo: false, ..CMD };
        let shift = |m: Mods| Mods { shift: true, ..m };
        let alt = |m: Mods| Mods { alt: true, ..m };
        for p in [KeyPlatform::Mac, KeyPlatform::Windows, KeyPlatform::Linux] {
            let primary = if p == KeyPlatform::Mac { CMD } else { ctrl };
            assert_eq!(on(Key::Command('k'), primary, p), Some(Shortcut::Palette), "{p:?}");
            assert_eq!(on(Key::Command('l'), primary, p), Some(Shortcut::GoToPath), "{p:?}");
            assert_eq!(on(Key::Command('L'), shift(primary), p), None, "⌘⇧L is left alone");
            assert_eq!(on(Key::Command('l'), alt(primary), p), None);
            for n in 1..=9 {
                let c = char::from_digit(n as u32, 10).unwrap();
                assert_eq!(on(Key::Command(c), primary, p), Some(Shortcut::Host(n)), "{p:?} {n}");
            }
            assert_eq!(on(Key::Command('0'), primary, p), None, "there's no host 0");
            assert_eq!(on(Key::Command('1'), alt(primary), p), None);
            assert_eq!(on(Key::Command('j'), shift(primary), p), Some(Shortcut::ShowTransfers), "{p:?}");
            assert_eq!(on(Key::Command('J'), shift(primary), p), Some(Shortcut::ShowTransfers));
            assert_eq!(on(Key::Command('j'), primary, p), None, "⌘J alone is nothing");
            // Plain digits and letters still type.
            assert_eq!(on(Key::Text("1".into()), NONE, p), Some(Shortcut::Type("1".into())));
            assert_eq!(on(Key::Text("l".into()), NONE, p), Some(Shortcut::Type("l".into())));
        }
    }

    #[test]
    fn host_chords_name_one_to_nine() {
        assert_eq!(host_chord(1), Some("mod+1"));
        assert_eq!(host_chord(9), Some("mod+9"));
        assert_eq!(host_chord(0), None);
        assert_eq!(host_chord(10), None);
    }

    #[test]
    fn settings_is_cmd_comma() {
        let ctrl = Mods { ctrl: true, logo: false, ..CMD };
        assert_eq!(k(Key::Command(','), CMD), Some(Shortcut::Settings));
        assert_eq!(on(Key::Command(','), ctrl, KeyPlatform::Windows), Some(Shortcut::Settings));
        assert_eq!(on(Key::Command(','), ctrl, KeyPlatform::Linux), Some(Shortcut::Settings));
        assert_eq!(k(Key::Command(','), Mods { shift: true, ..CMD }), None);
        assert_eq!(k(Key::Text(",".into()), NONE), Some(Shortcut::Type(",".into())), "a plain comma still types");
    }

    #[test]
    fn shift_f10_and_the_menu_key_open_the_context_menu_on_each_platform() {
        let ctrl = Mods { ctrl: true, logo: false, ..CMD };
        for p in [KeyPlatform::Mac, KeyPlatform::Windows, KeyPlatform::Linux] {
            assert_eq!(on(Key::Function(10), SHIFT, p), Some(Shortcut::ContextMenu), "{p:?}");
            assert_eq!(on(Key::ContextMenu, NONE, p), Some(Shortcut::ContextMenu), "{p:?}");
            assert_eq!(on(Key::ContextMenu, SHIFT, p), Some(Shortcut::ContextMenu), "{p:?}");
            assert_eq!(on(Key::Function(10), NONE, p), None, "plain F10 is the menu bar's");
            assert_eq!(on(Key::Function(10), ALT, p), None);
            assert_eq!(on(Key::Function(10), ctrl, p), None);
            assert_eq!(on(Key::ContextMenu, ALT, p), None);
            assert_eq!(on(Key::ContextMenu, ctrl, p), None);
        }
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
