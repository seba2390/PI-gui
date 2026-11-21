//! UI-agnostic backend for the integrated terminal panel.
//!
//! [`Terminal`] owns a pseudo terminal running the user's shell and the terminal emulator state
//! (via [`alacritty_terminal`], the same approach as Zed's terminal). It does not depend on any
//! GUI crate: the UI renders from [`Snapshot`]s, sends input through [`Terminal::write`] /
//! [`Terminal::key_bytes`] / [`Terminal::paste`], and repaints when it receives
//! [`TerminalEvent::Wakeup`].
//!
//! ```no_run
//! use term::{Key, KeyInput, Terminal, TerminalConfig, TerminalEvent};
//!
//! let (terminal, events) = Terminal::spawn(TerminalConfig::new("/tmp")).unwrap();
//! if let Some(bytes) = terminal.key_bytes(&KeyInput::plain(Key::Char('l'))) {
//!     terminal.write(bytes);
//! }
//! while let Ok(event) = events.recv_blocking() {
//!     match event {
//!         TerminalEvent::Wakeup => {
//!             let snapshot = terminal.snapshot();
//!             // paint snapshot.lines / snapshot.cursor / snapshot.selection ...
//!         }
//!         TerminalEvent::Exited(_) => break,
//!         _ => {}
//!     }
//! }
//! ```
//!
//! # Threading model
//!
//! * **IO thread** (one per terminal, named "PTY reader"): alacritty's `EventLoop`. It polls the
//!   PTY, parses shell output into the emulator state, writes queued input to the PTY, applies
//!   PTY resizes and notices the child exiting. It holds the emulator lock only while parsing a
//!   chunk (at most 64 KiB at a time) and uses a fair lock, so UI calls are never starved.
//! * **Caller / UI thread**: every [`Terminal`] method takes `&self`; `Terminal` is
//!   `Send + Sync`. Methods lock the emulator state briefly (a snapshot copies just the visible
//!   viewport); input and resize requests are queued to the IO thread over a channel and never
//!   block on the PTY.
//! * **Events** flow to the UI over an unbounded [`async_channel`], so they can be awaited from
//!   any executor (GPUI's foreground executor) or with `recv_blocking`. `Wakeup` is coalesced:
//!   after one is sent, the next is only sent once the UI has taken a snapshot, so a flood of
//!   output can never queue up more than one pending repaint.
//! * **Replies** to terminal queries (cursor position reports, color queries, text area size)
//!   are written back to the PTY directly from the IO thread; the UI is not involved.
//! * **Shutdown**: dropping a `Terminal` asks the IO thread to stop and returns immediately. A
//!   short-lived "pty-reaper" thread joins it; dropping the PTY sends `SIGHUP` to the shell and
//!   reaps it (on Windows, closes the pseudo console).
//!
//! # Memory
//!
//! Scrollback is bounded by [`TerminalConfig::scrollback_lines`] (default 10 000 lines; alacritty
//! allocates history lazily as it fills). Snapshots contain only the visible viewport, and
//! [`Terminal::snapshot_into`] reuses an existing snapshot's buffers.

mod color;
mod keys;
mod mode;
mod snapshot;
mod terminal;

pub use color::{Palette, Rgb};
pub use keys::{Key, KeyInput, Modifiers, encode_key, encode_paste};
pub use mode::TermModeFlags;
pub use snapshot::{
    Cell, CellFlags, CursorShape, CursorState, GridPoint, Line, Snapshot, StyleRun, StyleRuns,
};
pub use terminal::{
    DEFAULT_SCROLLBACK_LINES, SelectionKind, TermError, Terminal, TerminalConfig, TerminalEvent,
    TerminalSize,
};
