//! The bottom terminal panel: the user's shell in the chat's working folder.
//!
//! The emulator and PTY live in the UI-agnostic `term` crate; this view only paints its
//! [`Snapshot`]s and forwards keyboard, IME, mouse and clipboard input. Repaints are driven by
//! the terminal's coalesced `Wakeup` events, so an idle or hidden terminal costs nothing.

mod colors;
mod grid;
mod input;

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use term::{
    Key, KeyInput, Snapshot, TermModeFlags, Terminal, TerminalConfig, TerminalEvent, TerminalSize,
};

use self::grid::{CellMetrics, GridGeometry, PaintStyle};
use self::input::{KeyAction, ReportButton, ReportKind, TERMINAL_CONTEXT};
use crate::config::AppConfig;
use crate::ui::QuietButton as _;

/// Height of the panel's header strip.
const HEADER_HEIGHT: Pixels = px(30.);

/// Binds the keys the terminal must receive itself (see [`input::suppressed_bindings`]).
pub fn init(cx: &mut App) {
    cx.bind_keys(
        input::suppressed_bindings()
            .iter()
            .map(|keys| KeyBinding::new(keys, NoAction, Some(TERMINAL_CONTEXT))),
    );
}

#[derive(Debug, Clone)]
pub enum TerminalPanelEvent {
    /// The shell exited or the user closed the panel.
    Closed,
}

enum State {
    Running(Rc<Terminal>),
    Failed(SharedString),
}

/// Mouse interaction in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    None,
    Selecting,
    /// A button press was reported to a program with mouse tracking.
    Reporting(ReportButton),
}

/// A terminal on an SSH host: the local `ssh -t …` that runs the remote login shell.
struct RemoteShell {
    host: remote::HostId,
    label: String,
    program: PathBuf,
    args: Vec<String>,
}

pub struct TerminalPanel {
    /// The shell's folder (a path on the host for a remote terminal).
    cwd: PathBuf,
    remote: Option<RemoteShell>,
    focus_handle: FocusHandle,
    state: State,
    /// Latest viewport copy; shared with the paint closure. Refilled in place (no allocation).
    snapshot: Rc<RefCell<Snapshot>>,
    /// A `Wakeup` arrived since the last snapshot.
    dirty: bool,
    /// Grid placement from the last layout, for mouse hit testing.
    geometry: Rc<Cell<GridGeometry>>,
    metrics: Option<CellMetrics>,
    dark: Option<bool>,
    title: Option<SharedString>,
    /// IME composition in progress (marked text), so editing keys go to the IME.
    marked: Option<String>,
    drag: Drag,
    /// Fractional scroll lines carried between wheel events (smooth trackpads).
    scroll_remainder: f32,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TerminalPanelEvent> for TerminalPanel {}

impl TerminalPanel {
    pub fn new(cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_shell(cwd, None, None, window, cx)
    }

    /// A terminal on SSH host `host` (shown as `label`) in `cwd` there, through the host's
    /// shared connection. With `command`, the remote login shell runs it first.
    pub fn remote(
        host: remote::HostId,
        label: String,
        ssh: &remote::Ssh,
        cwd: PathBuf,
        command: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (program, args) = ssh.terminal_command(&cwd.to_string_lossy(), command);
        let remote = RemoteShell {
            host,
            label,
            program,
            args: args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        };
        Self::with_shell(cwd, None, Some(remote), window, cx)
    }

    /// Like [`new`](Self::new) with an explicit shell instead of the user's login shell.
    fn with_shell(
        cwd: PathBuf,
        shell: Option<PathBuf>,
        remote: Option<RemoteShell>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, _, cx| {
                this.focus_changed(true, cx);
            }),
            cx.on_blur(&focus_handle, window, |this, _, cx| {
                this.focus_changed(false, cx);
            }),
        ];
        let mut this = Self {
            cwd,
            remote,
            focus_handle,
            state: State::Failed(SharedString::default()),
            snapshot: Rc::default(),
            dirty: true,
            geometry: Rc::default(),
            metrics: None,
            dark: None,
            title: None,
            marked: None,
            drag: Drag::None,
            scroll_remainder: 0.,
            _tasks: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.spawn(shell, cx);
        this
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The SSH host this terminal runs on (`None`: this computer).
    pub fn host(&self) -> Option<&remote::HostId> {
        self.remote.as_ref().map(|remote| &remote.host)
    }

    fn spawn(&mut self, shell: Option<PathBuf>, cx: &mut Context<Self>) {
        let mut config = match &self.remote {
            // The local ssh client only needs some existing folder.
            Some(remote) => {
                let mut config = TerminalConfig::new(
                    cx.global::<AppConfig>()
                        .home
                        .clone()
                        .unwrap_or_else(std::env::temp_dir),
                );
                config.shell = Some(remote.program.clone());
                config.args.clone_from(&remote.args);
                config
            }
            None => {
                let mut config = TerminalConfig::new(&self.cwd);
                config.shell = shell;
                config
            }
        };
        config.palette = colors::for_mode(cx.theme().mode.is_dark());
        config.env = cx
            .global::<AppConfig>()
            .child_env
            .terminal_vars()
            .filter_map(|(key, value)| Some((key.to_str()?.to_owned(), value.to_str()?.to_owned())))
            .collect();
        config
            .env
            .push(("TERM_PROGRAM".to_owned(), "pistachio".to_owned()));

        match Terminal::spawn(config) {
            Ok((terminal, events)) => {
                self.state = State::Running(Rc::new(terminal));
                self._tasks.push(cx.spawn(async move |this, cx| {
                    while let Ok(event) = events.recv().await {
                        if this
                            .update(cx, |this, cx| this.on_terminal_event(event, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
            }
            Err(error) => {
                log::warn!(
                    "could not start a terminal in {}: {error}",
                    self.cwd.display()
                );
                self.state = State::Failed(format!("Could not start a shell: {error}").into());
            }
        }
    }

    fn terminal(&self) -> Option<&Rc<Terminal>> {
        match &self.state {
            State::Running(terminal) => Some(terminal),
            State::Failed(_) => None,
        }
    }

    fn on_terminal_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        match event {
            TerminalEvent::Wakeup => {
                self.dirty = true;
                cx.notify();
            }
            TerminalEvent::Title(title) => {
                let title = title.trim();
                self.title = (!title.is_empty()).then(|| SharedString::from(title.to_owned()));
                cx.notify();
            }
            TerminalEvent::ResetTitle => {
                self.title = None;
                cx.notify();
            }
            TerminalEvent::ClipboardStore(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            TerminalEvent::Bell => {}
            TerminalEvent::Exited(_) => cx.emit(TerminalPanelEvent::Closed),
        }
    }

    fn focus_changed(&mut self, focused: bool, cx: &mut Context<Self>) {
        if let Some(terminal) = self.terminal() {
            terminal.set_focused(focused);
        }
        self.dirty = true;
        cx.notify();
    }

    // ---- keyboard --------------------------------------------------------------

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal().cloned() else {
            return;
        };
        match input::map_keystroke(&event.keystroke, cfg!(target_os = "macos")) {
            KeyAction::Copy => {
                if let Some(text) = terminal.selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                } else if cfg!(target_os = "macos") {
                    // cmd-c without a selection does nothing, like Terminal.app.
                }
                cx.stop_propagation();
            }
            KeyAction::Paste => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    terminal.scroll_to_bottom();
                    terminal.paste(&text);
                }
                cx.stop_propagation();
            }
            // Composing text: editing keys belong to the IME.
            KeyAction::Send(_) if self.marked.is_some() => {}
            KeyAction::Send(key) => {
                if let Some(bytes) = terminal.key_bytes(&key) {
                    terminal.clear_selection();
                    terminal.scroll_to_bottom();
                    terminal.write(bytes);
                }
                cx.stop_propagation();
            }
            // Printable text arrives through the input handler (IME, dead keys).
            KeyAction::Text | KeyAction::Ignore => {}
        }
    }

    fn write_text(&mut self, text: &str) {
        if let Some(terminal) = self.terminal()
            && !text.is_empty()
        {
            terminal.clear_selection();
            terminal.scroll_to_bottom();
            terminal.write(text.as_bytes().to_vec());
        }
    }

    // ---- mouse -----------------------------------------------------------------

    fn report_mouse(
        &self,
        button: ReportButton,
        kind: ReportKind,
        position: Point<Pixels>,
        modifiers: Modifiers,
    ) -> bool {
        let Some(terminal) = self.terminal() else {
            return false;
        };
        let mode = terminal.mode();
        // Shift bypasses mouse tracking so text can still be selected, as in xterm.
        if modifiers.shift || !mode.intersects(TermModeFlags::MOUSE_MODE) {
            return false;
        }
        let point = self.geometry.get().cell_at(position);
        let modifiers = term::Modifiers {
            ctrl: modifiers.control,
            alt: modifiers.alt,
            shift: false,
            cmd: false,
        };
        if let Some(bytes) = input::mouse_report(button, kind, point, modifiers, mode) {
            terminal.write(bytes);
        }
        true
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let button = match event.button {
            MouseButton::Left => ReportButton::Left,
            MouseButton::Middle => ReportButton::Middle,
            MouseButton::Right => ReportButton::Right,
            MouseButton::Navigate(_) => return,
        };
        if self.report_mouse(button, ReportKind::Press, event.position, event.modifiers) {
            self.drag = Drag::Reporting(button);
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        let Some(terminal) = self.terminal() else {
            return;
        };
        let point = self.geometry.get().cell_at(event.position);
        match event.click_count {
            1 => {
                terminal.clear_selection();
                terminal.start_selection(point, term::SelectionKind::Simple);
            }
            2 => terminal.start_selection(point, term::SelectionKind::Word),
            _ => terminal.start_selection(point, term::SelectionKind::Line),
        }
        self.drag = Drag::Selecting;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, _: &mut Context<Self>) {
        match self.drag {
            Drag::None => {}
            Drag::Selecting => {
                if event.pressed_button == Some(MouseButton::Left)
                    && let Some(terminal) = self.terminal()
                {
                    terminal.update_selection(self.geometry.get().cell_at(event.position));
                }
            }
            Drag::Reporting(button) => {
                self.report_mouse(button, ReportKind::Drag, event.position, event.modifiers);
            }
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if let Drag::Reporting(button) = self.drag {
            self.report_mouse(button, ReportKind::Release, event.position, event.modifiers);
        }
        self.drag = Drag::None;
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal().cloned() else {
            return;
        };
        let cell_height = self.geometry.get().cell.height.max(px(1.));
        let delta = event.delta.pixel_delta(cell_height).y / cell_height + self.scroll_remainder;
        let lines = delta.trunc();
        self.scroll_remainder = delta - lines;
        // |lines| is small (one wheel event); the cast cannot overflow in practice.
        #[allow(clippy::cast_possible_truncation)]
        let lines = lines as i32;
        if lines == 0 {
            return;
        }
        let mode = terminal.mode();
        if mode.intersects(TermModeFlags::MOUSE_MODE) && !event.modifiers.shift {
            let button = if lines > 0 {
                ReportButton::WheelUp
            } else {
                ReportButton::WheelDown
            };
            for _ in 0..lines.unsigned_abs().min(10) {
                self.report_mouse(button, ReportKind::Press, event.position, event.modifiers);
            }
        } else if mode.contains(TermModeFlags::ALT_SCREEN | TermModeFlags::ALTERNATE_SCROLL) {
            // Full-screen programs (less, vim) scroll with arrow keys.
            let key = KeyInput::plain(if lines > 0 { Key::Up } else { Key::Down });
            if let Some(bytes) = terminal.key_bytes(&key) {
                for _ in 0..lines.unsigned_abs().min(10) {
                    terminal.write(bytes.clone());
                }
            }
        } else {
            terminal.scroll(lines);
        }
        cx.stop_propagation();
    }

    // ---- rendering ---------------------------------------------------------------

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let home = cx.global::<AppConfig>().home.clone();
        let folder = match &self.remote {
            Some(remote) => format!("{}:{}", remote.label, self.cwd.display()),
            None => crate::ui::short_path(&self.cwd, home.as_deref()),
        };
        h_flex()
            .h(HEADER_HEIGHT)
            .flex_none()
            .pl_3()
            .pr_1()
            .gap_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(Icon::new(IconName::SquareTerminal).xsmall())
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child("Terminal"),
            )
            .child(div().truncate().child(folder))
            .when_some(self.title.clone(), |this, title| {
                this.child(div().text_color(theme.border).child("·"))
                    .child(div().min_w_0().truncate().child(title))
            })
            .child(div().flex_1())
            .child(
                Button::new("close-terminal")
                    .quiet(cx)
                    .xsmall()
                    .icon(IconName::Close)
                    .accessibility_label("Close terminal")
                    .tooltip("Close terminal")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(TerminalPanelEvent::Closed))),
            )
    }

    fn render_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (family, font_size, dark) = (
            theme.mono_font_family.clone(),
            theme.mono_font_size,
            theme.mode.is_dark(),
        );
        let selection = theme.selection;

        let Some(terminal) = self.terminal().cloned() else {
            let State::Failed(message) = &self.state else {
                return div().into_any_element();
            };
            return div()
                .p_3()
                .text_sm()
                .text_color(theme.danger)
                .child(message.clone())
                .into_any_element();
        };

        if self.dark != Some(dark) {
            if self.dark.is_some() {
                terminal.set_palette(colors::for_mode(dark));
            }
            self.dark = Some(dark);
        }
        if self
            .metrics
            .as_ref()
            .is_none_or(|m| m.font.family != family || m.font_size != font_size)
        {
            self.metrics = Some(CellMetrics::measure(family, font_size, window));
        }
        let Some(metrics) = self.metrics.clone() else {
            return div().into_any_element();
        };
        if self.dirty {
            self.dirty = false;
            terminal.snapshot_into(&mut self.snapshot.borrow_mut());
        }

        let focused = self.focus_handle.is_focused(window);
        let (snapshot, geometry_cell, focus) = (
            self.snapshot.clone(),
            self.geometry.clone(),
            self.focus_handle.clone(),
        );
        let entity = cx.entity();
        let resize_terminal = terminal.clone();
        let cell = metrics.cell;
        let marked = self.marked.clone();

        canvas(
            move |bounds, _, _| {
                let geometry = GridGeometry::fit(bounds, cell);
                geometry_cell.set(geometry);
                resize_terminal.resize(TerminalSize {
                    cols: geometry.cols,
                    rows: geometry.rows,
                    cell_width_px: pixels_u16(cell.width),
                    cell_height_px: pixels_u16(cell.height),
                });
                geometry
            },
            move |bounds, geometry, window, cx| {
                window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let snapshot = snapshot.borrow();
                    grid::paint(
                        &snapshot,
                        &geometry,
                        &metrics,
                        PaintStyle { selection, focused },
                        window,
                        cx,
                    );
                    if let Some(marked) = &marked {
                        grid::paint_marked_text(marked, &snapshot, &geometry, &metrics, window, cx);
                    }
                });
            },
        )
        .size_full()
        .into_any_element()
    }
}

fn pixels_u16(value: Pixels) -> u16 {
    // Cell sizes are a few dozen pixels; the clamp makes the cast lossless.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let value = f32::from(value).round().clamp(0., f32::from(u16::MAX)) as u16;
    value
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _timer = crate::perf::render("terminal");
        let header = self.render_header(cx);
        let grid = self.render_grid(window, cx);
        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.background)
            .border_t_1()
            .border_color(theme.border)
            .child(header)
            .child(
                div()
                    .id("terminal-grid")
                    .key_context(TERMINAL_CONTEXT)
                    .track_focus(&self.focus_handle)
                    .flex_1()
                    .min_h_0()
                    .pl_3()
                    .pr_1()
                    .pb_1()
                    .cursor(CursorStyle::IBeam)
                    .on_key_down(cx.listener(Self::on_key_down))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
                    .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_scroll(cx.listener(Self::on_scroll))
                    .child(grid),
            )
    }
}

/// The terminal has no editable buffer: the input handler only receives text (typed,
/// composed by the IME, or dictated) and positions the IME candidate window at the cursor.
impl EntityInputHandler for TerminalPanel {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len = self
            .marked
            .as_ref()
            .map_or(0, |text| text.encode_utf16().count());
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        self.write_text(text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| new_text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let geometry = self.geometry.get();
        let cursor = self.snapshot.borrow().cursor?;
        Some(geometry.cells_bounds(cursor.row, cursor.col, 1))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::{Duration, Instant};

    use gpui_kit::{Focusable as _, TestAppContext};

    use super::{State, TerminalPanel};

    /// Types into a real `/bin/sh` through the panel's key and input handlers and waits for
    /// the echoed output to show up in the painted snapshot.
    #[gpui_kit::test]
    #[allow(clippy::unused_async)] // gpui tests take an async body
    async fn typing_runs_commands_in_a_real_shell(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            super::init(cx);
            cx.set_global(crate::config::AppConfig::for_tests("pi".into()));
        });
        let dir = std::env::temp_dir();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            TerminalPanel::with_shell(dir, Some("/bin/sh".into()), None, window, cx)
        });
        cx.update(|window, cx| {
            let focus = panel.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        });
        cx.simulate_input("echo pistachio-$((6*7))");
        cx.simulate_keystrokes("enter");

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            cx.update(|window, cx| {
                panel.update(cx, |_, cx| cx.notify());
                window.draw(cx).clear(cx);
            });
            let text = cx.read(|cx| {
                let panel = panel.read(cx);
                assert!(matches!(panel.state, State::Running(_)));
                panel.snapshot.borrow().text()
            });
            if text.contains("pistachio-42") {
                break;
            }
            assert!(Instant::now() < deadline, "no output; screen:\n{text}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
