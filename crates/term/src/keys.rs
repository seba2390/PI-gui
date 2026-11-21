//! Pure keyboard / paste → PTY byte encoding (xterm conventions).

use crate::mode::TermModeFlags;

/// A logical key press as delivered by the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyInput {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl KeyInput {
    pub const fn new(key: Key, modifiers: Modifiers) -> Self {
        Self { key, modifiers }
    }

    /// A key without modifiers.
    pub const fn plain(key: Key) -> Self {
        Self {
            key,
            modifiers: Modifiers::NONE,
        }
    }
}

/// Keys the terminal knows how to encode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// The character the key produced, *after* shift / keyboard layout were applied
    /// (shift+a is `Char('A')`).
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// Function key F1..=F20 (other values encode to nothing).
    F(u8),
}

/// Modifier state of a key press.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Cmd on macOS, Super/Windows elsewhere. Combos with it are reserved for the UI.
    pub cmd: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        cmd: false,
    };
    pub const CTRL: Self = Self {
        ctrl: true,
        ..Self::NONE
    };
    pub const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };
    pub const SHIFT: Self = Self {
        shift: true,
        ..Self::NONE
    };
    pub const CMD: Self = Self {
        cmd: true,
        ..Self::NONE
    };

    /// xterm modifier parameter (`1 + shift + 2*alt + 4*ctrl`), `None` without modifiers.
    fn xterm_param(self) -> Option<u8> {
        let p = 1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl);
        (p > 1).then_some(p)
    }
}

/// Encode a key press into the bytes a terminal sends to the PTY.
///
/// Returns `None` when the key should not be sent to the PTY at all (any `cmd` combo, which the
/// UI uses for copy/paste etc., or an unsupported function key).
pub fn encode_key(key: &KeyInput, mode: TermModeFlags) -> Option<Vec<u8>> {
    let m = key.modifiers;
    if m.cmd {
        return None;
    }
    let app_cursor = mode.contains(TermModeFlags::APP_CURSOR);
    let param = m.xterm_param();

    let bytes = match key.key {
        Key::Char(c) => encode_char(c, m),
        Key::Enter if mode.contains(TermModeFlags::LINE_FEED_NEW_LINE) => {
            alt_prefixed(m.alt, b"\r\n")
        }
        Key::Enter => alt_prefixed(m.alt, b"\r"),
        Key::Tab if m.shift => b"\x1b[Z".to_vec(),
        Key::Tab => alt_prefixed(m.alt, b"\t"),
        Key::Backspace if m.ctrl => alt_prefixed(m.alt, b"\x08"),
        Key::Backspace => alt_prefixed(m.alt, b"\x7f"),
        Key::Escape => alt_prefixed(m.alt, b"\x1b"),
        Key::Up => cursor_key(b'A', app_cursor, param),
        Key::Down => cursor_key(b'B', app_cursor, param),
        Key::Right => cursor_key(b'C', app_cursor, param),
        Key::Left => cursor_key(b'D', app_cursor, param),
        Key::Home => cursor_key(b'H', app_cursor, param),
        Key::End => cursor_key(b'F', app_cursor, param),
        Key::Insert => tilde_key(2, param),
        Key::Delete => tilde_key(3, param),
        Key::PageUp => tilde_key(5, param),
        Key::PageDown => tilde_key(6, param),
        Key::F(n) => return function_key(n, param),
    };
    Some(bytes)
}

/// Encode pasted text, honoring bracketed paste mode.
///
/// In bracketed mode, ESC and ETX (ctrl-c) bytes are stripped so the paste cannot terminate the
/// bracket early or inject control sequences. Otherwise newlines are normalized to `\r`, which is
/// what the Enter key sends.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if text.is_empty() {
        return Vec::new();
    }
    if bracketed {
        let mut out = Vec::with_capacity(text.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        // ASCII bytes never occur inside multi-byte UTF-8 sequences, so filtering bytes is safe.
        out.extend(text.bytes().filter(|b| *b != 0x1b && *b != 0x03));
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        let mut out = Vec::with_capacity(text.len());
        let mut bytes = text.bytes().peekable();
        while let Some(b) = bytes.next() {
            match b {
                b'\r' => {
                    if bytes.peek() == Some(&b'\n') {
                        bytes.next();
                    }
                    out.push(b'\r');
                }
                b'\n' => out.push(b'\r'),
                b => out.push(b),
            }
        }
        out
    }
}

fn alt_prefixed(alt: bool, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if alt {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

fn encode_char(c: char, m: Modifiers) -> Vec<u8> {
    if m.ctrl
        && let Some(byte) = ctrl_byte(c)
    {
        return alt_prefixed(m.alt, &[byte]);
    }
    let mut buf = [0; 4];
    alt_prefixed(m.alt, c.encode_utf8(&mut buf).as_bytes())
}

/// The C0 control byte for ctrl+`c`, following xterm's US-layout conventions.
fn ctrl_byte(c: char) -> Option<u8> {
    let byte = u8::try_from(c).ok()?;
    match byte {
        b'a'..=b'z' => Some(byte - b'a' + 1),
        b'A'..=b'Z' => Some(byte - b'A' + 1),
        b'@' | b' ' | b'2' | b'`' => Some(0x00),
        b'[' | b'3' => Some(0x1b),
        b'\\' | b'4' => Some(0x1c),
        b']' | b'5' => Some(0x1d),
        b'^' | b'6' => Some(0x1e),
        b'_' | b'7' | b'-' | b'/' => Some(0x1f),
        b'?' | b'8' => Some(0x7f),
        _ => None,
    }
}

/// Arrow keys and Home/End: `ESC [ x` / `ESC O x` (application cursor), `ESC [1;<mod> x`.
fn cursor_key(final_byte: u8, app_cursor: bool, param: Option<u8>) -> Vec<u8> {
    match param {
        Some(p) => format!("\x1b[1;{p}{}", char::from(final_byte)).into_bytes(),
        None if app_cursor => vec![0x1b, b'O', final_byte],
        None => vec![0x1b, b'[', final_byte],
    }
}

/// `ESC [ <code> ~` / `ESC [ <code> ; <mod> ~`.
fn tilde_key(code: u8, param: Option<u8>) -> Vec<u8> {
    match param {
        Some(p) => format!("\x1b[{code};{p}~").into_bytes(),
        None => format!("\x1b[{code}~").into_bytes(),
    }
}

fn function_key(n: u8, param: Option<u8>) -> Option<Vec<u8>> {
    const TILDE_CODES: [u8; 16] = [
        15, 17, 18, 19, 20, 21, 23, 24, 25, 26, 28, 29, 31, 32, 33, 34,
    ];
    match n {
        1..=4 => {
            let final_byte = char::from(b"PQRS"[usize::from(n - 1)]);
            Some(match param {
                Some(p) => format!("\x1b[1;{p}{final_byte}").into_bytes(),
                None => format!("\x1bO{final_byte}").into_bytes(),
            })
        }
        5..=20 => Some(tilde_key(TILDE_CODES[usize::from(n - 5)], param)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn enc(key: Key, modifiers: Modifiers) -> Option<Vec<u8>> {
        encode_key(&KeyInput::new(key, modifiers), TermModeFlags::empty())
    }

    fn enc_mode(key: Key, modifiers: Modifiers, mode: TermModeFlags) -> Option<Vec<u8>> {
        encode_key(&KeyInput::new(key, modifiers), mode)
    }

    #[allow(clippy::unnecessary_wraps)]
    fn b(s: &str) -> Option<Vec<u8>> {
        Some(s.as_bytes().to_vec())
    }

    const CTRL_SHIFT: Modifiers = Modifiers {
        ctrl: true,
        shift: true,
        ..Modifiers::NONE
    };
    const CTRL_ALT: Modifiers = Modifiers {
        ctrl: true,
        alt: true,
        ..Modifiers::NONE
    };

    #[test]
    fn plain_chars() {
        assert_eq!(enc(Key::Char('a'), Modifiers::NONE), b("a"));
        assert_eq!(enc(Key::Char('A'), Modifiers::SHIFT), b("A"));
        assert_eq!(enc(Key::Char('é'), Modifiers::NONE), b("é"));
        assert_eq!(enc(Key::Char('日'), Modifiers::NONE), b("日"));
    }

    #[test]
    fn ctrl_letters_map_to_c0() {
        assert_eq!(enc(Key::Char('c'), Modifiers::CTRL), Some(vec![0x03]));
        assert_eq!(enc(Key::Char('a'), Modifiers::CTRL), Some(vec![0x01]));
        assert_eq!(enc(Key::Char('Z'), CTRL_SHIFT), Some(vec![0x1a]));
        assert_eq!(enc(Key::Char(' '), Modifiers::CTRL), Some(vec![0x00]));
        assert_eq!(enc(Key::Char('['), Modifiers::CTRL), Some(vec![0x1b]));
        assert_eq!(enc(Key::Char('\\'), Modifiers::CTRL), Some(vec![0x1c]));
        assert_eq!(enc(Key::Char('/'), Modifiers::CTRL), Some(vec![0x1f]));
        // No control equivalent: the character is sent as-is.
        assert_eq!(enc(Key::Char('ø'), Modifiers::CTRL), b("ø"));
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(enc(Key::Char('b'), Modifiers::ALT), b("\x1bb"));
        assert_eq!(enc(Key::Char('x'), CTRL_ALT), Some(vec![0x1b, 0x18]));
        assert_eq!(enc(Key::Backspace, Modifiers::ALT), b("\x1b\x7f"));
        assert_eq!(enc(Key::Enter, Modifiers::ALT), b("\x1b\r"));
    }

    #[test]
    fn special_keys() {
        assert_eq!(enc(Key::Enter, Modifiers::NONE), b("\r"));
        assert_eq!(enc(Key::Tab, Modifiers::NONE), b("\t"));
        assert_eq!(enc(Key::Tab, Modifiers::SHIFT), b("\x1b[Z"));
        assert_eq!(enc(Key::Backspace, Modifiers::NONE), Some(vec![0x7f]));
        assert_eq!(enc(Key::Backspace, Modifiers::CTRL), Some(vec![0x08]));
        assert_eq!(enc(Key::Escape, Modifiers::NONE), b("\x1b"));
        assert_eq!(enc(Key::Insert, Modifiers::NONE), b("\x1b[2~"));
        assert_eq!(enc(Key::Delete, Modifiers::NONE), b("\x1b[3~"));
        assert_eq!(enc(Key::PageUp, Modifiers::NONE), b("\x1b[5~"));
        assert_eq!(enc(Key::PageDown, Modifiers::SHIFT), b("\x1b[6;2~"));
    }

    #[test]
    fn enter_in_line_feed_mode() {
        let mode = TermModeFlags::LINE_FEED_NEW_LINE;
        assert_eq!(enc_mode(Key::Enter, Modifiers::NONE, mode), b("\r\n"));
    }

    #[test]
    fn arrows_honor_app_cursor_mode() {
        assert_eq!(enc(Key::Up, Modifiers::NONE), b("\x1b[A"));
        assert_eq!(enc(Key::Left, Modifiers::NONE), b("\x1b[D"));
        assert_eq!(enc(Key::Home, Modifiers::NONE), b("\x1b[H"));
        assert_eq!(enc(Key::End, Modifiers::NONE), b("\x1b[F"));
        let app = TermModeFlags::APP_CURSOR;
        assert_eq!(enc_mode(Key::Up, Modifiers::NONE, app), b("\x1bOA"));
        assert_eq!(enc_mode(Key::Right, Modifiers::NONE, app), b("\x1bOC"));
        assert_eq!(enc_mode(Key::Home, Modifiers::NONE, app), b("\x1bOH"));
        // Modified keys use the CSI form regardless of the cursor mode.
        assert_eq!(enc_mode(Key::Up, Modifiers::CTRL, app), b("\x1b[1;5A"));
    }

    #[test]
    fn modified_arrows() {
        assert_eq!(enc(Key::Up, Modifiers::CTRL), b("\x1b[1;5A"));
        assert_eq!(enc(Key::Down, Modifiers::SHIFT), b("\x1b[1;2B"));
        assert_eq!(enc(Key::Right, Modifiers::ALT), b("\x1b[1;3C"));
        assert_eq!(enc(Key::Left, CTRL_SHIFT), b("\x1b[1;6D"));
        assert_eq!(enc(Key::End, Modifiers::CTRL), b("\x1b[1;5F"));
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc(Key::F(1), Modifiers::NONE), b("\x1bOP"));
        assert_eq!(enc(Key::F(4), Modifiers::NONE), b("\x1bOS"));
        assert_eq!(enc(Key::F(1), Modifiers::SHIFT), b("\x1b[1;2P"));
        assert_eq!(enc(Key::F(5), Modifiers::NONE), b("\x1b[15~"));
        assert_eq!(enc(Key::F(10), Modifiers::NONE), b("\x1b[21~"));
        assert_eq!(enc(Key::F(11), Modifiers::NONE), b("\x1b[23~"));
        assert_eq!(enc(Key::F(12), Modifiers::NONE), b("\x1b[24~"));
        assert_eq!(enc(Key::F(12), Modifiers::CTRL), b("\x1b[24;5~"));
        assert_eq!(enc(Key::F(0), Modifiers::NONE), None);
        assert_eq!(enc(Key::F(21), Modifiers::NONE), None);
    }

    #[test]
    fn cmd_combos_are_left_to_the_ui() {
        assert_eq!(enc(Key::Char('c'), Modifiers::CMD), None);
        assert_eq!(enc(Key::Up, Modifiers::CMD), None);
        assert_eq!(
            enc(
                Key::Enter,
                Modifiers {
                    cmd: true,
                    shift: true,
                    ..Modifiers::NONE
                }
            ),
            None
        );
    }

    #[test]
    fn paste_plain_normalizes_newlines() {
        assert_eq!(encode_paste("a\nb\r\nc\rd", false), b"a\rb\rc\rd".to_vec());
        assert_eq!(encode_paste("", false), Vec::<u8>::new());
    }

    #[test]
    fn paste_bracketed_wraps_and_sanitizes() {
        assert_eq!(
            encode_paste("ls\n", true),
            b"\x1b[200~ls\n\x1b[201~".to_vec()
        );
        assert_eq!(
            encode_paste("a\x1b[201~rm\x03 é", true),
            "\x1b[200~a[201~rm é\x1b[201~".as_bytes().to_vec()
        );
        assert_eq!(encode_paste("", true), Vec::<u8>::new());
    }
}
