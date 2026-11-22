//! The live terminal: PTY + shell process + emulator state.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::JoinHandle;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, State};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::tty;

use crate::color::Palette;
use crate::keys::{KeyInput, encode_key, encode_paste};
use crate::mode::TermModeFlags;
use crate::snapshot::{GridPoint, Snapshot, build_snapshot, viewport_to_grid};

/// Default number of scrollback lines.
pub const DEFAULT_SCROLLBACK_LINES: usize = 10_000;

/// Grid size plus cell size in pixels (the latter is reported to programs via `TIOCSWINSZ`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width_px: u16,
    pub cell_height_px: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            cell_width_px: 8,
            cell_height_px: 16,
        }
    }
}

impl TerminalSize {
    /// Clamp to the smallest grid alacritty supports (2x1).
    fn clamped(self) -> Self {
        Self {
            cols: self.cols.max(2),
            rows: self.rows.max(1),
            ..self
        }
    }
}

impl From<TerminalSize> for WindowSize {
    fn from(s: TerminalSize) -> Self {
        Self {
            num_lines: s.rows,
            num_cols: s.cols,
            cell_width: s.cell_width_px,
            cell_height: s.cell_height_px,
        }
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        usize::from(self.rows)
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.rows)
    }

    fn columns(&self) -> usize {
        usize::from(self.cols)
    }
}

/// How to start a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalConfig {
    /// Working directory of the shell. Must be an existing directory.
    pub cwd: PathBuf,
    /// Shell program, run without arguments. `None` picks the user's shell: `$SHELL` (falling
    /// back to the passwd entry) on Unix, started as a login shell on macOS like Terminal.app /
    /// iTerm / Alacritty do; `%COMSPEC%` or `powershell` on Windows.
    pub shell: Option<PathBuf>,
    /// Arguments for `shell` (ignored when `shell` is `None`), e.g. `ssh -t host …` for a
    /// terminal on another machine.
    pub args: Vec<String>,
    /// Extra environment variables. `TERM=xterm-256color` and `COLORTERM=truecolor` are always
    /// set (and take precedence).
    pub env: Vec<(String, String)>,
    pub size: TerminalSize,
    /// Maximum number of scrollback lines (default 10 000).
    pub scrollback_lines: usize,
    pub palette: Palette,
}

impl TerminalConfig {
    /// Defaults for everything but the working directory.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            shell: None,
            args: Vec::new(),
            env: Vec::new(),
            size: TerminalSize::default(),
            scrollback_lines: DEFAULT_SCROLLBACK_LINES,
            palette: Palette::default(),
        }
    }
}

/// Events for the UI. Receive them from the channel returned by [`Terminal::spawn`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    /// Terminal content changed: repaint (call [`Terminal::snapshot`]). Coalesced: after one
    /// `Wakeup`, the next one is only sent once a snapshot has been taken.
    Wakeup,
    /// The program set the window title (OSC 0/2).
    Title(String),
    /// The program reset the title.
    ResetTitle,
    Bell,
    /// The program asked to copy text to the clipboard (OSC 52).
    ClipboardStore(String),
    /// The shell exited, with its exit code if it exited normally. Sent once; the terminal
    /// stays readable (snapshots, selection) afterwards but input is ignored.
    Exited(Option<i32>),
}

/// Selection granularity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SelectionKind {
    /// Cell by cell (click + drag).
    #[default]
    Simple,
    /// Whole words (double click).
    Word,
    /// Whole lines (triple click).
    Line,
}

impl From<SelectionKind> for SelectionType {
    fn from(kind: SelectionKind) -> Self {
        match kind {
            SelectionKind::Simple => Self::Simple,
            SelectionKind::Word => Self::Semantic,
            SelectionKind::Line => Self::Lines,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TermError {
    #[error("working directory {0:?} is not a directory")]
    InvalidCwd(PathBuf),
    #[error("failed to spawn shell in a pty: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("failed to start terminal io: {0}")]
    Io(#[source] std::io::Error),
}

/// State shared between the [`Terminal`] handle and the event listener that alacritty calls
/// from the IO thread (with the terminal lock held, so it must never lock the terminal).
struct Shared {
    events: async_channel::Sender<TerminalEvent>,
    wakeup_pending: AtomicBool,
    exited: AtomicBool,
    bold_is_bright: AtomicBool,
    palette: Mutex<Palette>,
    size: Mutex<TerminalSize>,
    /// Set right after the event loop is created, before any byte is parsed.
    pty_tx: OnceLock<EventLoopSender>,
}

impl Shared {
    fn emit(&self, event: TerminalEvent) {
        // The receiver may be gone (UI closed the panel); that's fine.
        let _ = self.events.try_send(event);
    }

    fn wakeup(&self) {
        if !self.wakeup_pending.swap(true, Ordering::AcqRel) {
            self.emit(TerminalEvent::Wakeup);
        }
    }

    fn mark_exited(&self, code: Option<i32>) {
        if !self.exited.swap(true, Ordering::AcqRel) {
            self.emit(TerminalEvent::Exited(code));
        }
    }

    fn write_pty(&self, bytes: Cow<'static, [u8]>) {
        if bytes.is_empty() || self.exited.load(Ordering::Acquire) {
            return;
        }
        if let Some(tx) = self.pty_tx.get()
            && let Err(err) = tx.send(Msg::Input(bytes))
        {
            log::debug!("terminal: dropping pty input: {err}");
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
struct Listener(Arc<Shared>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let shared = &self.0;
        match event {
            Event::Wakeup => shared.wakeup(),
            Event::Title(title) => shared.emit(TerminalEvent::Title(title)),
            Event::ResetTitle => shared.emit(TerminalEvent::ResetTitle),
            Event::Bell => shared.emit(TerminalEvent::Bell),
            Event::ClipboardStore(_, text) => shared.emit(TerminalEvent::ClipboardStore(text)),
            // Replies to queries (cursor position, device attributes, ...).
            Event::PtyWrite(text) => shared.write_pty(text.into_bytes().into()),
            Event::ColorRequest(index, format) => {
                let color = lock(&shared.palette).indexed(index);
                shared.write_pty(format(color.into()).into_bytes().into());
            }
            Event::TextAreaSizeRequest(format) => {
                let size = *lock(&shared.size);
                shared.write_pty(format(size.into()).into_bytes().into());
            }
            Event::ChildExit(status) => shared.mark_exited(status.code()),
            Event::Exit => shared.mark_exited(None),
            // OSC 52 paste is disabled (alacritty's default `Osc52::OnlyCopy`); the rest is
            // irrelevant for us.
            Event::ClipboardLoad(..) | Event::MouseCursorDirty | Event::CursorBlinkingChange => {}
        }
    }
}

type IoThread = JoinHandle<(EventLoop<tty::Pty, Listener>, State)>;

/// A running shell in a pseudo terminal plus its emulator state.
///
/// `Terminal` is `Send + Sync`; every method takes `&self` and only briefly locks the emulator
/// state, so it can be called from the UI thread every frame. See the crate docs for the
/// threading model.
pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    shared: Arc<Shared>,
    pty_tx: EventLoopSender,
    io_thread: Option<IoThread>,
    /// Absolute grid point and kind of the selection being made.
    selection_anchor: Mutex<Option<(Point, SelectionType)>>,
}

impl Terminal {
    /// Start the shell in a new PTY and the background IO thread.
    pub fn spawn(
        config: TerminalConfig,
    ) -> Result<(Self, async_channel::Receiver<TerminalEvent>), TermError> {
        if !config.cwd.is_dir() {
            return Err(TermError::InvalidCwd(config.cwd));
        }
        let size = config.size.clamped();
        let (events, rx) = async_channel::unbounded();
        let shared = Arc::new(Shared {
            events,
            wakeup_pending: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            bold_is_bright: AtomicBool::new(false),
            palette: Mutex::new(config.palette),
            size: Mutex::new(size),
            pty_tx: OnceLock::new(),
        });
        let listener = Listener(shared.clone());

        let term_config = TermConfig {
            scrolling_history: config.scrollback_lines,
            ..TermConfig::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(
            term_config,
            &size,
            listener.clone(),
        )));

        let pty = tty::new(&pty_options(&config), size.into(), 0).map_err(TermError::Spawn)?;
        let event_loop =
            EventLoop::new(term.clone(), listener, pty, true, false).map_err(TermError::Io)?;
        let pty_tx = event_loop.channel();
        // Cannot fail: the lock was created above and is set exactly once.
        let _ = shared.pty_tx.set(pty_tx.clone());
        let io_thread = event_loop.spawn();

        let terminal = Self {
            term,
            shared,
            pty_tx,
            io_thread: Some(io_thread),
            selection_anchor: Mutex::new(None),
        };
        Ok((terminal, rx))
    }

    /// Send raw bytes (user input) to the shell. Ignored after the shell exited.
    ///
    /// This does not scroll the viewport; call [`scroll_to_bottom`](Self::scroll_to_bottom) on
    /// key input like other terminals do.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.shared.write_pty(bytes.into());
    }

    /// Paste text, wrapping it in bracketed-paste markers when the program enabled that mode.
    pub fn paste(&self, text: &str) {
        let bracketed = self.mode().contains(TermModeFlags::BRACKETED_PASTE);
        self.write(encode_paste(text, bracketed));
    }

    /// Resize the grid and the PTY. Cheap no-op when the size did not change, so it can be
    /// called on every layout pass.
    pub fn resize(&self, size: TerminalSize) {
        let size = size.clamped();
        {
            let mut current = lock(&self.shared.size);
            if *current == size {
                return;
            }
            *current = size;
        }
        self.term.lock().resize(size);
        if !self.shared.exited.load(Ordering::Acquire) {
            let _ = self.pty_tx.send(Msg::Resize(size.into()));
        }
        self.shared.wakeup();
    }

    /// Scroll the viewport by `delta_lines`; positive scrolls up into the history.
    pub fn scroll(&self, delta_lines: i32) {
        self.scroll_display(Scroll::Delta(delta_lines));
    }

    /// Scroll by one screen; positive pages up into the history.
    pub fn scroll_page(&self, pages: i32) {
        let scroll = if pages >= 0 {
            Scroll::PageUp
        } else {
            Scroll::PageDown
        };
        for _ in 0..pages.unsigned_abs() {
            self.scroll_display(scroll);
        }
    }

    /// Jump back to the live output.
    pub fn scroll_to_bottom(&self) {
        self.scroll_display(Scroll::Bottom);
    }

    fn scroll_display(&self, scroll: Scroll) {
        self.term.lock().scroll_display(scroll);
        self.shared.wakeup();
    }

    /// Copy the visible state. Allocates a fresh snapshot; prefer
    /// [`snapshot_into`](Self::snapshot_into) in a render loop.
    pub fn snapshot(&self) -> Snapshot {
        let mut snapshot = Snapshot::default();
        self.snapshot_into(&mut snapshot);
        snapshot
    }

    /// Refill `out` with the visible state, reusing its buffers (no allocation in the steady
    /// state). Re-arms the [`TerminalEvent::Wakeup`] notification.
    pub fn snapshot_into(&self, out: &mut Snapshot) {
        // Clear before locking: anything parsed after we release the lock sends a new wakeup.
        self.shared.wakeup_pending.store(false, Ordering::Release);
        let palette = *lock(&self.shared.palette);
        let bold_is_bright = self.shared.bold_is_bright.load(Ordering::Relaxed);
        let term = self.term.lock();
        build_snapshot(&term, &palette, bold_is_bright, out);
    }

    /// Bytes to send for a key press in the current mode, or `None` if the key is not for the
    /// terminal (cmd combos). Write the result with [`write`](Self::write).
    pub fn key_bytes(&self, key: &KeyInput) -> Option<Vec<u8>> {
        encode_key(key, self.mode())
    }

    /// Start a selection at a viewport cell. `Word` and `Line` select immediately; `Simple`
    /// selects once [`update_selection`](Self::update_selection) is called. Both endpoint cells
    /// are included.
    pub fn start_selection(&self, point: GridPoint, kind: SelectionKind) {
        let ty = SelectionType::from(kind);
        let mut anchor = lock(&self.selection_anchor);
        let mut term = self.term.lock();
        let point = viewport_to_grid(&term, point);
        *anchor = Some((point, ty));
        term.selection = Some(Selection::new(ty, point, Side::Left));
        drop(term);
        self.shared.wakeup();
    }

    /// Extend the selection started with [`start_selection`](Self::start_selection) to a
    /// viewport cell (inclusive).
    pub fn update_selection(&self, point: GridPoint) {
        let anchor = lock(&self.selection_anchor);
        let Some((start, ty)) = *anchor else { return };
        let mut term = self.term.lock();
        let point = viewport_to_grid(&term, point);
        // Pick sides so that both the anchor cell and the current cell are included.
        let (start_side, end_side) = if point >= start {
            (Side::Left, Side::Right)
        } else {
            (Side::Right, Side::Left)
        };
        let mut selection = Selection::new(ty, start, start_side);
        selection.update(point, end_side);
        term.selection = Some(selection);
        drop(term);
        self.shared.wakeup();
    }

    pub fn clear_selection(&self) {
        let mut anchor = lock(&self.selection_anchor);
        *anchor = None;
        self.term.lock().selection = None;
        drop(anchor);
        self.shared.wakeup();
    }

    /// Text of the current selection, if any.
    pub fn selected_text(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|s| !s.is_empty())
    }

    /// Modes the running program has set.
    pub fn mode(&self) -> TermModeFlags {
        TermModeFlags::from(*self.term.lock().mode())
    }

    /// Tell the terminal whether it has keyboard focus; reports focus changes to programs that
    /// asked for it (`ESC [I` / `ESC [O`).
    pub fn set_focused(&self, focused: bool) {
        let report = {
            let mut term = self.term.lock();
            let changed = term.is_focused != focused;
            term.is_focused = focused;
            changed
                && term
                    .mode()
                    .contains(alacritty_terminal::term::TermMode::FOCUS_IN_OUT)
        };
        if report {
            self.write(if focused {
                &b"\x1b[I"[..]
            } else {
                &b"\x1b[O"[..]
            });
        }
    }

    /// Change the colors (e.g. on a theme switch).
    pub fn set_palette(&self, palette: Palette) {
        *lock(&self.shared.palette) = palette;
        self.shared.wakeup();
    }

    /// Draw bold text with the bright variant of ANSI colors 0..8 (off by default).
    pub fn set_bold_is_bright(&self, enabled: bool) {
        self.shared.bold_is_bright.store(enabled, Ordering::Relaxed);
        self.shared.wakeup();
    }

    /// Whether the shell has exited.
    pub fn has_exited(&self) -> bool {
        self.shared.exited.load(Ordering::Acquire)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Stop the IO thread. Dropping the event loop drops the PTY, which sends SIGHUP to the
        // shell and then *blocks* in `waitpid` until it exits, so join on a short-lived helper
        // thread instead of the (UI) thread dropping us.
        let _ = self.pty_tx.send(Msg::Shutdown);
        if let Some(handle) = self.io_thread.take() {
            let reaper = std::thread::Builder::new()
                .name("pty-reaper".into())
                .spawn(move || drop(handle.join()));
            if let Err(err) = reaper {
                // The handle was dropped with the closure, detaching the IO thread; it then
                // drops the PTY itself when it finishes.
                log::warn!("terminal: failed to spawn pty reaper thread: {err}");
            }
        }
    }
}

fn pty_options(config: &TerminalConfig) -> tty::Options {
    let mut env: HashMap<String, String> = config.env.iter().cloned().collect();
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    // Windows has an extra `escape_args` field.
    #[allow(clippy::needless_update)]
    tty::Options {
        shell: shell_for(config),
        working_directory: Some(config.cwd.clone()),
        drain_on_exit: true,
        env,
        ..tty::Options::default()
    }
}

fn shell_for(config: &TerminalConfig) -> Option<tty::Shell> {
    if let Some(shell) = &config.shell {
        return Some(tty::Shell::new(
            shell.to_string_lossy().into_owned(),
            config.args.clone(),
        ));
    }
    default_shell()
}

/// `None` lets alacritty pick: `$SHELL` / passwd, wrapped in `/usr/bin/login -flp` on macOS so it
/// runs as a login shell.
#[cfg(not(windows))]
fn default_shell() -> Option<tty::Shell> {
    None
}

/// PowerShell, like other Windows terminals (`COMSPEC`, i.e. cmd.exe, is always set, so it
/// would never be the fallback).
#[cfg(windows)]
#[allow(clippy::unnecessary_wraps)] // same signature as the Unix version
fn default_shell() -> Option<tty::Shell> {
    Some(tty::Shell::new("powershell.exe".to_owned(), vec!["-NoLogo".to_owned()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Terminal>();
    }

    #[test]
    fn env_always_sets_term_and_colorterm() {
        let mut config = TerminalConfig::new("/");
        config.env = vec![("TERM".into(), "dumb".into()), ("FOO".into(), "bar".into())];
        let options = pty_options(&config);
        assert_eq!(
            options.env.get("TERM").map(String::as_str),
            Some("xterm-256color")
        );
        assert_eq!(
            options.env.get("COLORTERM").map(String::as_str),
            Some("truecolor")
        );
        assert_eq!(options.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(options.working_directory, Some(PathBuf::from("/")));
    }

    #[test]
    fn size_is_clamped() {
        let size = TerminalSize {
            cols: 0,
            rows: 0,
            ..TerminalSize::default()
        }
        .clamped();
        assert_eq!((size.cols, size.rows), (2, 1));
    }

    #[test]
    fn spawn_rejects_missing_cwd() {
        let config = TerminalConfig::new("/definitely/not/a/real/dir");
        assert!(matches!(
            Terminal::spawn(config),
            Err(TermError::InvalidCwd(_))
        ));
    }
}
