//! The dictation shortcut on the composer: hold it to talk (releasing ends dictation and
//! keeps the text), or tap it to start and tap again to stop.
//!
//! The decision is a small state machine, [`PushToTalk`], kept apart from GPUI so it can be
//! tested; the composer feeds it key-down and key-up events and acts on the [`Command`]s.

use std::time::{Duration, Instant};

/// The shortcut as GPUI keystroke text. ⌘Space is Spotlight and Ctrl+Space switches input
/// sources on macOS, so it takes Shift as well; Linux and Windows use Ctrl for ⌘.
pub const KEYS: &str = if cfg!(target_os = "macos") {
    "cmd-shift-space"
} else {
    "ctrl-shift-space"
};

/// [`KEYS`] as shown in tooltips and docs.
pub const LABEL: &str = if cfg!(target_os = "macos") {
    "⌘⇧Space"
} else {
    "Ctrl+Shift+Space"
};

/// A press at least this long is a hold (release stops); a shorter one is a tap (dictation
/// keeps going until the next press). Long enough that a quick tap never counts as a hold.
pub const HOLD_THRESHOLD: Duration = Duration::from_millis(400);

/// What the composer should do with dictation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Start,
    Stop,
    Nothing,
}

/// Tracks the shortcut between its key-down and key-up.
#[derive(Debug, Default)]
pub struct PushToTalk {
    press: Option<Press>,
}

#[derive(Debug, Clone, Copy)]
struct Press {
    at: Instant,
    /// This press started dictation, so releasing it after a hold stops it again. A press
    /// that stopped dictation has nothing left to do on release.
    started: bool,
}

impl PushToTalk {
    /// The shortcut went down. `repeat` is the OS key repeat while held; `active` says
    /// whether dictation is running.
    ///
    /// A fresh press always replaces an old one: if the window lost focus mid-hold the
    /// key-up never arrived, and that must not swallow the next press.
    pub fn key_down(&mut self, now: Instant, repeat: bool, active: bool) -> Command {
        if repeat {
            return Command::Nothing;
        }
        self.press = Some(Press {
            at: now,
            started: !active,
        });
        if active {
            Command::Stop
        } else {
            Command::Start
        }
    }

    /// The shortcut's key came up.
    pub fn key_up(&mut self, now: Instant, active: bool) -> Command {
        match self.press.take() {
            Some(press)
                if press.started
                    && active
                    && now.saturating_duration_since(press.at) >= HOLD_THRESHOLD =>
            {
                Command::Stop
            }
            _ => Command::Nothing,
        }
    }

    /// Forget the press, for when it could not start dictation (no model yet: the download
    /// dialog opened instead) so its release does nothing.
    pub fn reset(&mut self) {
        self.press = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn hold_starts_and_release_stops() {
        let t = Instant::now();
        let mut ptt = PushToTalk::default();
        assert_eq!(ptt.key_down(t, false, false), Command::Start);
        // Key repeat while held changes nothing.
        assert_eq!(ptt.key_down(t + ms(500), true, true), Command::Nothing);
        assert_eq!(ptt.key_up(t + ms(1200), true), Command::Stop);
    }

    #[test]
    fn tap_toggles() {
        let t = Instant::now();
        let mut ptt = PushToTalk::default();
        assert_eq!(ptt.key_down(t, false, false), Command::Start);
        assert_eq!(ptt.key_up(t + ms(120), true), Command::Nothing);
        // Dictation keeps running; the next tap stops it, and its release does nothing.
        let later = t + ms(5000);
        assert_eq!(ptt.key_down(later, false, true), Command::Stop);
        assert_eq!(ptt.key_up(later + ms(100), true), Command::Nothing);
    }

    #[test]
    fn holding_the_stop_press_does_not_restart() {
        let t = Instant::now();
        let mut ptt = PushToTalk::default();
        assert_eq!(ptt.key_down(t, false, true), Command::Stop);
        assert_eq!(ptt.key_up(t + ms(2000), true), Command::Nothing);
    }

    #[test]
    fn release_after_dictation_already_ended_does_nothing() {
        // A silence ended dictation while the key was still down.
        let t = Instant::now();
        let mut ptt = PushToTalk::default();
        ptt.key_down(t, false, false);
        assert_eq!(ptt.key_up(t + ms(3000), false), Command::Nothing);
    }

    #[test]
    fn a_lost_key_up_does_not_swallow_the_next_press() {
        let t = Instant::now();
        let mut ptt = PushToTalk::default();
        ptt.key_down(t, false, false);
        // No key-up (the window lost focus); dictation was ended some other way.
        assert_eq!(ptt.key_down(t + ms(9000), false, false), Command::Start);
    }

    #[test]
    fn reset_forgets_the_press() {
        let t = Instant::now();
        let mut ptt = PushToTalk::default();
        ptt.key_down(t, false, false);
        ptt.reset();
        assert_eq!(ptt.key_up(t + ms(1000), true), Command::Nothing);
    }

    #[test]
    fn keys_parse_as_a_keystroke() {
        let keystroke = gpui_kit::Keystroke::parse(KEYS).expect("valid keystroke");
        assert_eq!(keystroke.key, "space");
        assert!(keystroke.modifiers.shift);
    }
}
