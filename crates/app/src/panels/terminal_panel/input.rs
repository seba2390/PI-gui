//! Pure input translation: GPUI keystrokes → terminal keys, mouse events → xterm mouse reports.
//!
//! Nothing here touches a window, so it is unit tested directly.

use gpui_kit::{Keystroke, Modifiers};
use term::{GridPoint, Key, KeyInput, TermModeFlags};

/// Key context of the terminal grid (see [`suppressed_bindings`]).
pub const TERMINAL_CONTEXT: &str = "Terminal";

/// What a key press in the terminal should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// Copy the selection (cmd-c / ctrl-shift-c).
    Copy,
    /// Paste the clipboard (cmd-v / ctrl-shift-v).
    Paste,
    /// Plain text: leave the event alone so the platform delivers it to the input handler
    /// (which also handles IME composition and dead keys) and it is written from there.
    Text,
    /// Encode and send this key.
    Send(KeyInput),
    /// Not for the terminal (app shortcuts such as cmd-n): let it propagate.
    Ignore,
}

/// Translate a GPUI keystroke. `macos` selects the platform conventions (cmd vs ctrl-shift
/// clipboard shortcuts, which modifiers the platform still turns into text).
pub fn map_keystroke(keystroke: &Keystroke, macos: bool) -> KeyAction {
    let m = keystroke.modifiers;
    let key = keystroke.key.as_str();

    if macos && m.platform && !m.control && !m.alt {
        return match key {
            "c" if !m.shift => KeyAction::Copy,
            "v" if !m.shift => KeyAction::Paste,
            _ => KeyAction::Ignore,
        };
    }
    if !macos && m.control && m.shift && !m.alt && !m.platform {
        if key.eq_ignore_ascii_case("c") {
            return KeyAction::Copy;
        }
        if key.eq_ignore_ascii_case("v") {
            return KeyAction::Paste;
        }
    }
    if m.platform {
        return KeyAction::Ignore;
    }

    let modifiers = term_modifiers(m);
    if let Some(named) = named_key(key) {
        return KeyAction::Send(KeyInput::new(named, modifiers));
    }

    let has_text = keystroke.key_char.is_some();
    if has_text && platform_inserts_text(m, macos) {
        return KeyAction::Text;
    }

    // ctrl / alt combinations: send the character with its modifiers (ctrl-c → ETX, alt-b →
    // ESC b). With ctrl, `key_char` is absent or unreliable, so use the key itself.
    let ch = if m.control {
        single_char(key)
    } else {
        keystroke
            .key_char
            .as_deref()
            .and_then(single_char)
            .or_else(|| single_char(key))
    };
    let ch = match (key, ch) {
        ("space", _) => Some(' '),
        (_, ch) => ch,
    };
    ch.map_or(KeyAction::Ignore, |ch| {
        KeyAction::Send(KeyInput::new(Key::Char(ch), modifiers))
    })
}

/// Whether the platform hands an unhandled key press with these modifiers to the text input
/// handler. macOS composes text for option combinations too (option-8 is `[` on many European
/// layouts); Linux only inserts text for unmodified or shifted keys.
fn platform_inserts_text(m: Modifiers, macos: bool) -> bool {
    if macos {
        !m.control && !m.function && !m.platform
    } else {
        !m.control && !m.alt && !m.platform && !m.function
    }
}

fn term_modifiers(m: Modifiers) -> term::Modifiers {
    term::Modifiers {
        ctrl: m.control,
        alt: m.alt,
        shift: m.shift,
        cmd: m.platform,
    }
}

fn single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch)
}

/// GPUI key names (shared by the macOS and Linux backends) that map to special terminal keys.
fn named_key(key: &str) -> Option<Key> {
    Some(match key {
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "escape" => Key::Escape,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        _ => {
            let n: u8 = key.strip_prefix('f')?.parse().ok()?;
            if !(1..=20).contains(&n) {
                return None;
            }
            Key::F(n)
        }
    })
}

/// Keystrokes that app-level key bindings (gpui-kit's Root tab navigation and copy, the
/// window's ctrl shortcuts on Linux) would otherwise claim before the terminal sees them.
/// They are bound to `NoAction` in [`TERMINAL_CONTEXT`] so they reach the key-down handler.
/// cmd shortcuts (other than copy) and ``ctrl-` `` stay with the app.
pub fn suppressed_bindings() -> &'static [&'static str] {
    &[
        "tab",
        "shift-tab",
        "escape",
        "enter",
        "shift-enter",
        "alt-enter",
        "backspace",
        "up",
        "down",
        "left",
        "right",
        "alt-up",
        "alt-down",
        "space",
        "cmd-c",
        "ctrl-space",
        "ctrl-a",
        "ctrl-b",
        "ctrl-c",
        "ctrl-d",
        "ctrl-e",
        "ctrl-f",
        "ctrl-g",
        "ctrl-h",
        "ctrl-i",
        "ctrl-j",
        "ctrl-k",
        "ctrl-l",
        "ctrl-m",
        "ctrl-n",
        "ctrl-o",
        "ctrl-p",
        "ctrl-q",
        "ctrl-r",
        "ctrl-s",
        "ctrl-t",
        "ctrl-u",
        "ctrl-v",
        "ctrl-w",
        "ctrl-x",
        "ctrl-y",
        "ctrl-z",
    ]
}

/// A mouse button as reported to programs that enabled mouse tracking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

impl ReportButton {
    fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
            Self::WheelUp => 64,
            Self::WheelDown => 65,
        }
    }
}

/// The kind of mouse event being reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportKind {
    Press,
    Release,
    /// Motion with the button held.
    Drag,
}

/// Encode a mouse event for a program that enabled mouse tracking, or `None` when the mode does
/// not ask for this event (or the position cannot be encoded in the legacy format).
pub fn mouse_report(
    button: ReportButton,
    kind: ReportKind,
    point: GridPoint,
    modifiers: term::Modifiers,
    mode: TermModeFlags,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermModeFlags::MOUSE_MODE) {
        return None;
    }
    if kind == ReportKind::Drag
        && !mode.intersects(TermModeFlags::MOUSE_DRAG | TermModeFlags::MOUSE_MOTION)
    {
        return None;
    }
    let wheel = matches!(button, ReportButton::WheelUp | ReportButton::WheelDown);
    if wheel && kind == ReportKind::Release {
        return None;
    }

    let mut code = button.code();
    if kind == ReportKind::Drag {
        code += 32;
    }
    code +=
        4 * u8::from(modifiers.shift) + 8 * u8::from(modifiers.alt) + 16 * u8::from(modifiers.ctrl);

    let col = point.col.saturating_add(1);
    let row = point.row.saturating_add(1);
    if mode.contains(TermModeFlags::SGR_MOUSE) {
        let suffix = if kind == ReportKind::Release {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{col};{row}{suffix}").into_bytes());
    }

    // Legacy X10 / normal encoding: release is button 3, coordinates offset by 32 in one byte.
    if kind == ReportKind::Release {
        code = 3 + (code & !0b11);
    }
    let encode = |v: usize| u8::try_from(v + 32).ok();
    Some(vec![
        0x1b,
        b'[',
        b'M',
        32 + code,
        encode(col)?,
        encode(row)?,
    ])
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Keystroke, Modifiers};
    use term::{GridPoint, Key, KeyInput, TermModeFlags};

    use super::{
        KeyAction, ReportButton, ReportKind, map_keystroke, mouse_report, suppressed_bindings,
    };

    fn ks(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.to_owned(),
            key_char: key_char.map(str::to_owned),
        }
    }

    #[allow(clippy::fn_params_excessive_bools)]
    fn mods(control: bool, alt: bool, shift: bool, platform: bool) -> Modifiers {
        Modifiers {
            control,
            alt,
            shift,
            platform,
            function: false,
        }
    }

    const NONE: term::Modifiers = term::Modifiers::NONE;

    fn send(key: Key, modifiers: term::Modifiers) -> KeyAction {
        KeyAction::Send(KeyInput::new(key, modifiers))
    }

    #[test]
    fn printable_keys_go_through_the_input_handler() {
        for macos in [true, false] {
            let none = Modifiers::default();
            assert_eq!(
                map_keystroke(&ks("a", Some("a"), none), macos),
                KeyAction::Text
            );
            let shift = mods(false, false, true, false);
            assert_eq!(
                map_keystroke(&ks("a", Some("A"), shift), macos),
                KeyAction::Text
            );
            assert_eq!(
                map_keystroke(&ks("space", Some(" "), none), macos),
                KeyAction::Text
            );
            assert_eq!(
                map_keystroke(&ks("ø", Some("ø"), none), macos),
                KeyAction::Text
            );
        }
    }

    #[test]
    fn option_text_on_macos_but_meta_on_linux() {
        let alt = mods(false, true, false, false);
        // Option-8 types `[` on Danish / German Mac layouts.
        assert_eq!(
            map_keystroke(&ks("8", Some("["), alt), true),
            KeyAction::Text
        );
        assert_eq!(
            map_keystroke(&ks("b", Some("b"), alt), false),
            send(Key::Char('b'), term::Modifiers::ALT)
        );
    }

    #[test]
    fn named_keys() {
        let none = Modifiers::default();
        // macOS reports enter / tab with a key_char; they must still be encoded as keys.
        assert_eq!(
            map_keystroke(&ks("enter", Some("\n"), none), true),
            send(Key::Enter, NONE)
        );
        assert_eq!(
            map_keystroke(&ks("tab", Some("\t"), none), true),
            send(Key::Tab, NONE)
        );
        let shift = mods(false, false, true, false);
        assert_eq!(
            map_keystroke(&ks("tab", None, shift), true),
            send(Key::Tab, term::Modifiers::SHIFT)
        );
        assert_eq!(
            map_keystroke(&ks("escape", None, none), false),
            send(Key::Escape, NONE)
        );
        assert_eq!(
            map_keystroke(&ks("backspace", None, none), false),
            send(Key::Backspace, NONE)
        );
        assert_eq!(
            map_keystroke(&ks("up", None, none), true),
            send(Key::Up, NONE)
        );
        assert_eq!(
            map_keystroke(&ks("pagedown", None, none), true),
            send(Key::PageDown, NONE)
        );
        assert_eq!(
            map_keystroke(&ks("delete", None, none), true),
            send(Key::Delete, NONE)
        );
        assert_eq!(
            map_keystroke(&ks("f1", None, none), true),
            send(Key::F(1), NONE)
        );
        assert_eq!(
            map_keystroke(&ks("f12", None, none), false),
            send(Key::F(12), NONE)
        );
        assert_eq!(
            map_keystroke(&ks("f30", None, none), true),
            KeyAction::Ignore
        );
        let alt = mods(false, true, false, false);
        assert_eq!(
            map_keystroke(&ks("left", None, alt), true),
            send(Key::Left, term::Modifiers::ALT)
        );
    }

    #[test]
    fn ctrl_combinations() {
        let ctrl = mods(true, false, false, false);
        for macos in [true, false] {
            assert_eq!(
                map_keystroke(&ks("c", None, ctrl), macos),
                send(Key::Char('c'), term::Modifiers::CTRL)
            );
            // Linux may still report a key_char; ctrl wins.
            assert_eq!(
                map_keystroke(&ks("d", Some("d"), ctrl), macos),
                send(Key::Char('d'), term::Modifiers::CTRL)
            );
            assert_eq!(
                map_keystroke(&ks("space", None, ctrl), macos),
                send(Key::Char(' '), term::Modifiers::CTRL)
            );
            assert_eq!(
                map_keystroke(&ks("up", None, ctrl), macos),
                send(Key::Up, term::Modifiers::CTRL)
            );
        }
    }

    #[test]
    fn clipboard_shortcuts() {
        let cmd = mods(false, false, false, true);
        assert_eq!(map_keystroke(&ks("c", None, cmd), true), KeyAction::Copy);
        assert_eq!(map_keystroke(&ks("v", None, cmd), true), KeyAction::Paste);
        assert_eq!(map_keystroke(&ks("n", None, cmd), true), KeyAction::Ignore);
        let ctrl_shift = mods(true, false, true, false);
        assert_eq!(
            map_keystroke(&ks("c", None, ctrl_shift), false),
            KeyAction::Copy
        );
        assert_eq!(
            map_keystroke(&ks("v", None, ctrl_shift), false),
            KeyAction::Paste
        );
        // On macOS ctrl-shift-c is just a control character.
        assert_eq!(
            map_keystroke(&ks("c", None, ctrl_shift), true),
            send(
                Key::Char('c'),
                term::Modifiers {
                    ctrl: true,
                    shift: true,
                    ..NONE
                }
            )
        );
        // Super combos on Linux belong to the desktop / app.
        assert_eq!(map_keystroke(&ks("c", None, cmd), false), KeyAction::Ignore);
    }

    #[test]
    fn suppressed_bindings_parse() {
        for keys in suppressed_bindings() {
            assert!(Keystroke::parse(keys).is_ok(), "{keys}");
        }
        assert!(!suppressed_bindings().contains(&"ctrl-`"));
    }

    #[test]
    fn sgr_mouse_reports() {
        let mode = TermModeFlags::MOUSE_REPORT_CLICK | TermModeFlags::SGR_MOUSE;
        let p = GridPoint::new(4, 9);
        let report = |button, kind, modifiers| mouse_report(button, kind, p, modifiers, mode);
        assert_eq!(
            report(ReportButton::Left, ReportKind::Press, NONE),
            Some(b"\x1b[<0;10;5M".to_vec())
        );
        assert_eq!(
            report(ReportButton::Left, ReportKind::Release, NONE),
            Some(b"\x1b[<0;10;5m".to_vec())
        );
        assert_eq!(
            report(
                ReportButton::Right,
                ReportKind::Press,
                term::Modifiers::CTRL
            ),
            Some(b"\x1b[<18;10;5M".to_vec())
        );
        assert_eq!(
            report(ReportButton::WheelUp, ReportKind::Press, NONE),
            Some(b"\x1b[<64;10;5M".to_vec())
        );
        assert_eq!(
            report(ReportButton::WheelDown, ReportKind::Release, NONE),
            None
        );
        // Drag is only reported in drag / motion tracking modes.
        assert_eq!(report(ReportButton::Left, ReportKind::Drag, NONE), None);
        let drag_mode = mode | TermModeFlags::MOUSE_DRAG;
        assert_eq!(
            mouse_report(ReportButton::Left, ReportKind::Drag, p, NONE, drag_mode),
            Some(b"\x1b[<32;10;5M".to_vec())
        );
    }

    #[test]
    fn legacy_mouse_reports() {
        let mode = TermModeFlags::MOUSE_REPORT_CLICK;
        let p = GridPoint::new(0, 0);
        assert_eq!(
            mouse_report(ReportButton::Left, ReportKind::Press, p, NONE, mode),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(
            mouse_report(ReportButton::Middle, ReportKind::Release, p, NONE, mode),
            Some(vec![0x1b, b'[', b'M', 35, 33, 33])
        );
        // Columns past 223 cannot be encoded in one byte.
        let far = GridPoint::new(0, 300);
        assert_eq!(
            mouse_report(ReportButton::Left, ReportKind::Press, far, NONE, mode),
            None
        );
        // No tracking mode: nothing to report.
        assert_eq!(
            mouse_report(
                ReportButton::Left,
                ReportKind::Press,
                p,
                NONE,
                TermModeFlags::empty()
            ),
            None
        );
    }
}
