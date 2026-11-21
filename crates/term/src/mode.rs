//! Terminal mode flags relevant to the UI (input encoding, mouse reporting, paste).

use alacritty_terminal::term::TermMode;

bitflags::bitflags! {
    /// Modes set by the program running in the terminal.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct TermModeFlags: u32 {
        /// DECCKM: cursor keys send `ESC O x` instead of `ESC [ x`.
        const APP_CURSOR         = 1 << 0;
        /// DECKPAM: application keypad.
        const APP_KEYPAD         = 1 << 1;
        /// Paste must be wrapped in `ESC [200~` / `ESC [201~`.
        const BRACKETED_PASTE    = 1 << 2;
        /// The alternate screen (full screen apps like vim/less) is active.
        const ALT_SCREEN         = 1 << 3;
        /// Report mouse button presses (X10/normal tracking).
        const MOUSE_REPORT_CLICK = 1 << 4;
        /// Report mouse motion while a button is held.
        const MOUSE_DRAG         = 1 << 5;
        /// Report all mouse motion.
        const MOUSE_MOTION       = 1 << 6;
        /// Use SGR (1006) mouse encoding.
        const SGR_MOUSE          = 1 << 7;
        /// Use UTF-8 (1005) mouse encoding.
        const UTF8_MOUSE         = 1 << 8;
        /// Report focus in/out (`ESC [I` / `ESC [O`).
        const FOCUS_IN_OUT       = 1 << 9;
        /// Mouse wheel in the alternate screen should send arrow keys.
        const ALTERNATE_SCROLL   = 1 << 10;
        /// The cursor is visible (DECTCEM).
        const SHOW_CURSOR        = 1 << 11;
        /// LNM: Enter sends `\r\n`.
        const LINE_FEED_NEW_LINE = 1 << 12;
        /// Any mouse reporting mode is active.
        const MOUSE_MODE = Self::MOUSE_REPORT_CLICK.bits()
            | Self::MOUSE_DRAG.bits()
            | Self::MOUSE_MOTION.bits();
    }
}

impl From<TermMode> for TermModeFlags {
    fn from(mode: TermMode) -> Self {
        const MAP: [(TermMode, TermModeFlags); 13] = [
            (TermMode::APP_CURSOR, TermModeFlags::APP_CURSOR),
            (TermMode::APP_KEYPAD, TermModeFlags::APP_KEYPAD),
            (TermMode::BRACKETED_PASTE, TermModeFlags::BRACKETED_PASTE),
            (TermMode::ALT_SCREEN, TermModeFlags::ALT_SCREEN),
            (
                TermMode::MOUSE_REPORT_CLICK,
                TermModeFlags::MOUSE_REPORT_CLICK,
            ),
            (TermMode::MOUSE_DRAG, TermModeFlags::MOUSE_DRAG),
            (TermMode::MOUSE_MOTION, TermModeFlags::MOUSE_MOTION),
            (TermMode::SGR_MOUSE, TermModeFlags::SGR_MOUSE),
            (TermMode::UTF8_MOUSE, TermModeFlags::UTF8_MOUSE),
            (TermMode::FOCUS_IN_OUT, TermModeFlags::FOCUS_IN_OUT),
            (TermMode::ALTERNATE_SCROLL, TermModeFlags::ALTERNATE_SCROLL),
            (TermMode::SHOW_CURSOR, TermModeFlags::SHOW_CURSOR),
            (
                TermMode::LINE_FEED_NEW_LINE,
                TermModeFlags::LINE_FEED_NEW_LINE,
            ),
        ];
        MAP.iter()
            .filter(|(m, _)| mode.contains(*m))
            .fold(Self::empty(), |acc, (_, f)| acc | *f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_alacritty_modes() {
        let flags = TermModeFlags::from(TermMode::APP_CURSOR | TermMode::BRACKETED_PASTE);
        assert_eq!(
            flags,
            TermModeFlags::APP_CURSOR | TermModeFlags::BRACKETED_PASTE
        );
        // Alacritty's default mode: cursor visible, line wrap, alternate scroll, urgency hints.
        let default = TermModeFlags::from(TermMode::default());
        assert!(default.contains(TermModeFlags::SHOW_CURSOR));
        assert!(!default.intersects(TermModeFlags::MOUSE_MODE));
    }
}
