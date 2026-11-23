//! The typing animation for a conversation's new automatic name in the sidebar.
//!
//! The row's old text is erased from the end, a short beat passes, and the new name is
//! typed out character by character behind a caret, at a slightly uneven pace like a
//! person typing; the caret blinks twice and goes. [`frame`] is the whole animation as a
//! function of time, so the row just asks it what to show.

use std::time::Duration;

/// Erasing takes this long per character, but at most [`MAX_ERASE`] in all.
const ERASE_PER_CHAR: Duration = Duration::from_millis(14);
const MAX_ERASE: Duration = Duration::from_millis(320);
/// The beat between erasing and typing.
const PAUSE: Duration = Duration::from_millis(140);
/// The average time per typed character, varied per character by up to ±[`JITTER`] percent.
const TYPE_PER_CHAR: Duration = Duration::from_millis(40);
const JITTER: u64 = 45;
/// How long the finished name keeps a blinking caret.
const HOLD: Duration = Duration::from_millis(1000);
/// The caret's blink half-period while held.
const BLINK: Duration = Duration::from_millis(250);

/// What a row shows at one moment of the animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub text: &'a str,
    /// The caret is drawn after the text.
    pub caret: bool,
    /// The animation is over: show the name plainly.
    pub done: bool,
}

/// The pause before character `index` (`c`) of the name appears: [`TYPE_PER_CHAR`] varied
/// by a fixed pseudo-random amount (the same on every redraw, so nothing flickers), a little
/// longer after a space or punctuation, as when a person types.
fn char_delay(index: usize, c: char) -> Duration {
    let mut hash = (index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ u64::from(c);
    hash ^= hash >> 29;
    let percent = 100 - JITTER + hash % (2 * JITTER + 1);
    let base = TYPE_PER_CHAR * u32::try_from(percent).unwrap_or(100) / 100;
    if index > 0 && (c == ' ' || c.is_ascii_punctuation()) {
        base + base / 2
    } else {
        base
    }
}

fn erase_time(from: &str) -> Duration {
    (ERASE_PER_CHAR * u32::try_from(from.chars().count()).unwrap_or(u32::MAX)).min(MAX_ERASE)
}

fn pause_time(from: &str) -> Duration {
    if from.is_empty() {
        Duration::ZERO
    } else {
        PAUSE
    }
}

fn type_time(to: &str) -> Duration {
    to.chars()
        .enumerate()
        .map(|(index, c)| char_delay(index, c))
        .sum()
}

/// How long the whole animation from `from` to `to` takes.
pub fn duration(from: &str, to: &str) -> Duration {
    erase_time(from) + pause_time(from) + type_time(to) + HOLD
}

/// The first `count` characters of `text`.
fn prefix(text: &str, count: usize) -> &str {
    text.char_indices()
        .nth(count)
        .map_or(text, |(end, _)| &text[..end])
}

/// What to show `elapsed` into replacing `from` (the row's old text; empty when the row is
/// new) with the name `to`.
pub fn frame<'a>(from: &'a str, to: &'a str, elapsed: Duration) -> Frame<'a> {
    let erase = erase_time(from);
    if elapsed < erase {
        let total = from.chars().count();
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )] // a short title's length, and the result is in 0..=total
        let left = {
            let gone = (elapsed.as_secs_f64() / erase.as_secs_f64() * total as f64).floor();
            total.saturating_sub(gone as usize)
        };
        return Frame {
            text: prefix(from, left),
            caret: true,
            done: false,
        };
    }
    let mut t = elapsed.saturating_sub(erase);
    let pause = pause_time(from);
    if t < pause {
        return Frame {
            text: "",
            caret: true,
            done: false,
        };
    }
    t -= pause;
    let mut typed = Duration::ZERO;
    for (index, c) in to.chars().enumerate() {
        typed += char_delay(index, c);
        if t < typed {
            return Frame {
                text: prefix(to, index),
                caret: true,
                done: false,
            };
        }
    }
    let held = t.saturating_sub(typed);
    if held < HOLD {
        // Solid for the first half-period, then blinking.
        let phase = held.as_millis() / BLINK.as_millis();
        return Frame {
            text: to,
            caret: phase.is_multiple_of(2),
            done: false,
        };
    }
    Frame {
        text: to,
        caret: false,
        done: true,
    }
}

#[cfg(test)]
mod tests {
    use super::{Duration, Frame, HOLD, duration, frame};

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn erases_then_types_then_settles() {
        let (from, to) = ("fix the bug pls", "Login bug fix");
        assert_eq!(frame(from, to, ms(0)).text, from);
        assert!(frame(from, to, ms(0)).caret);
        // Erasing shortens the old text from its end.
        let halfway = frame(from, to, ms(105)).text;
        assert!(
            from.starts_with(halfway) && halfway.len() < from.len(),
            "{halfway}"
        );
        // Then the new name grows from its start.
        let mut last = 0;
        let mut seen_partial = false;
        let end = u64::try_from(duration(from, to).as_millis()).unwrap();
        for step in (0..end).step_by(10) {
            let f = frame(from, to, ms(step));
            if to.starts_with(f.text) && !f.text.is_empty() {
                assert!(f.text.len() >= last, "typing never goes back");
                last = f.text.len();
                seen_partial |= f.text.len() < to.len();
            }
        }
        assert!(seen_partial);
        assert_eq!(
            frame(from, to, duration(from, to)),
            Frame {
                text: to,
                caret: false,
                done: true
            }
        );
    }

    #[test]
    fn a_new_row_starts_typing_at_once() {
        let f = frame("", "Trip to Japan", ms(0));
        assert_eq!((f.text, f.caret, f.done), ("", true, false));
        assert_ne!(frame("", "Trip to Japan", ms(300)).text, "");
    }

    #[test]
    fn caret_blinks_while_held() {
        let to = "Done";
        let typed = duration("", to).checked_sub(HOLD).unwrap();
        let carets: Vec<bool> = (0..4)
            .map(|half| frame("", to, typed + ms(half * 250 + 10)).caret)
            .collect();
        assert_eq!(carets, [true, false, true, false]);
        assert_eq!(frame("", to, typed + ms(10)).text, to);
    }

    #[test]
    fn handles_multibyte_text_and_is_short() {
        for step in (0..3000).step_by(7) {
            let f = frame("Æblegrød 🍎", "Smørrebrød på dansk", ms(step));
            assert!(!f.text.is_empty() || f.caret || f.done);
        }
        // Long old titles are erased quickly; a typical name settles in under three seconds.
        assert!(duration(&"x".repeat(80), "Planning a trip to Japan") < Duration::from_secs(3));
    }
}
