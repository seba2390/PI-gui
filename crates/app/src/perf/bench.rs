//! `PISTACHIO_BENCH=1`: a fixed navigation script whose response times go to the
//! `PISTACHIO_PERF` file, then quits. Run it through `script/perf.sh`, which sets up a
//! private pi folder with generated sessions, so the user's own sessions are never touched.
//!
//! Inputs are synthetic GPUI events dispatched to this window (like demo mode), never OS
//! input. Each step is timed from the input until the frame showing its result has been
//! presented; steps whose result arrives later (a session's history) are also timed until
//! that content is on screen.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use gpui_kit::component::WindowExt as _;
use gpui_kit::*;
use pi_sessions::{Mode, SessionSummary};

use crate::actions::{
    CheckForUpdates, NewChat, OpenSettings, ShowChatMode, ShowCodeMode, ShowDiff, ShowFiles,
    ToggleRightPanel, ToggleSidebar, ToggleTerminal,
};
use crate::config::AppConfig;
use crate::window::MainWindow;

/// Pause between steps, so background work of one step (pi starting, a folder scan) does not
/// land inside the next step's measurement.
const SETTLE: Duration = Duration::from_millis(350);
const TIMEOUT: Duration = Duration::from_secs(10);

pub fn requested() -> bool {
    std::env::var_os("PISTACHIO_BENCH").is_some_and(|value| !value.is_empty())
}

/// Runs the script in the main window, then quits the app.
pub fn start(window: &mut Window, cx: &mut Context<MainWindow>) -> Task<()> {
    cx.spawn_in(window, async move |this, cx| {
        let mut bench = Bench { this };
        if let Err(error) = bench.run(cx).await {
            log::error!("bench: {error:#}");
            super::phase(&format!("bench_error: {error:#}"));
        }
        super::summary();
        super::phase("quit");
        let _ = cx.update(|_, cx| cx.quit());
    })
}

struct Bench {
    this: WeakEntity<MainWindow>,
}

impl Bench {
    async fn run(&mut self, cx: &mut AsyncWindowContext) -> Result<()> {
        self.wait_until("the session list", cx, |this, cx| {
            this.sidebar_entity().read(cx).is_loaded()
        })
        .await?;
        sleep(cx, Duration::from_millis(800)).await;
        super::summary(); // startup renders, separately from the script's
        // What the menu bar checks to enable its items, before any input.
        let available = cx.update(|window, cx| {
            [
                ("Settings…", window.is_action_available(&OpenSettings, cx)),
                (
                    "Check for Updates…",
                    window.is_action_available(&CheckForUpdates, cx),
                ),
                ("New Chat", window.is_action_available(&NewChat, cx)),
            ]
        })?;
        for (item, enabled) in available {
            super::phase(&format!("menu at launch: {item} enabled={enabled}"));
        }

        // Chat mode: hover the list, open a few chats, hover and scroll a long one.
        self.action("switch mode", ShowChatMode, cx).await?;
        let chats = self.sessions(Mode::Chat, cx)?;
        self.hover_rows(&chats, cx).await?;
        for summary in chats.iter().take(4) {
            self.open(summary, cx).await?;
        }
        if let Some(longest) = chats.iter().max_by_key(|summary| file_len(&summary.path)) {
            self.open(longest, cx).await?;
        }
        self.hover_chat(cx).await?;
        self.scroll_chat(cx).await?;

        // Window-level navigation.
        for _ in 0..3 {
            self.action("switch mode", ShowCodeMode, cx).await?;
            self.action("switch mode", ShowChatMode, cx).await?;
        }
        for _ in 0..2 {
            self.action("toggle sidebar", ToggleSidebar, cx).await?;
        }
        for _ in 0..2 {
            self.click_mark("show-settings", "open settings", cx)
                .await?;
            sleep(cx, SETTLE).await;
            self.click_mark("show-settings", "close settings", cx)
                .await?;
            sleep(cx, SETTLE).await;
        }

        // Code mode: open sessions, then the side panel and terminal.
        self.action("switch mode", ShowCodeMode, cx).await?;
        let code = self.sessions(Mode::Code, cx)?;
        self.hover_rows(&code, cx).await?;
        for summary in code.iter().take(3) {
            self.open(summary, cx).await?;
        }
        for _ in 0..2 {
            self.action("toggle side panel", ToggleRightPanel, cx)
                .await?;
        }
        self.action("show changes", ShowDiff, cx).await?;
        self.action("show files", ShowFiles, cx).await?;
        self.action("toggle side panel", ToggleRightPanel, cx)
            .await?;
        for _ in 0..2 {
            self.action("toggle terminal", ToggleTerminal, cx).await?;
        }
        self.action("new chat", NewChat, cx).await?;

        // Deleting a session (the generated one meant for it).
        self.action("switch mode", ShowChatMode, cx).await?;
        let doomed = self
            .sessions(Mode::Chat, cx)?
            .into_iter()
            .find(|summary| summary.title().contains("delete me"))
            .context("no generated session titled \"delete me\"")?;
        self.delete(&doomed, cx).await?;
        Ok(())
    }

    /// The listed local sessions of `mode`, newest first.
    fn sessions(&self, mode: Mode, cx: &mut AsyncWindowContext) -> Result<Vec<SessionSummary>> {
        self.this.read_with(cx, |this, cx| {
            let config = cx.global::<AppConfig>();
            this.sidebar_entity()
                .read(cx)
                .listed()
                .iter()
                .filter(|summary| config.mode_of(&summary.cwd) == mode)
                .cloned()
                .collect()
        })
    }

    async fn wait_until(
        &self,
        what: &str,
        cx: &mut AsyncWindowContext,
        done: impl Fn(&MainWindow, &App) -> bool,
    ) -> Result<()> {
        let start = Instant::now();
        while !self.this.read_with(cx, |this, cx| done(this, cx))? {
            if start.elapsed() > TIMEOUT {
                let shown = self.this.read_with(cx, |this, cx| {
                    let session = this.visible_chat().read(cx).session().read(cx);
                    format!(
                        "welcome {}, showing {:?} with {} items",
                        this.welcome_visible(),
                        session.session_file(),
                        session.transcript().items().len()
                    )
                })?;
                bail!("timed out waiting for {what} ({shown})");
            }
            sleep(cx, Duration::from_millis(2)).await;
        }
        Ok(())
    }

    async fn action(
        &self,
        name: &str,
        action: impl Action,
        cx: &mut AsyncWindowContext,
    ) -> Result<()> {
        cx.update(|window, cx| {
            super::frame(name, window);
            window.dispatch_action(action.boxed_clone(), cx);
        })?;
        sleep(cx, SETTLE).await;
        Ok(())
    }

    /// Opens `summary` by clicking its sidebar row: timed until the click's frame and until
    /// the conversation's history is on screen.
    async fn open(&self, summary: &SessionSummary, cx: &mut AsyncWindowContext) -> Result<()> {
        let start = self
            .click_mark(&row_mark(summary), "open session: click", cx)
            .await?;
        // A click that lands while the sidebar lists its sessions again (pi resuming the
        // previous one writes to its file) can miss: click once more, timed from the first.
        let shown = |this: &MainWindow, cx: &App, path: &std::path::Path| {
            this.visible_chat()
                .read(cx)
                .session()
                .read(cx)
                .session_file()
                .as_deref()
                == Some(path)
        };
        let waited = Instant::now();
        while !self
            .this
            .read_with(cx, |this, cx| shown(this, cx, &summary.path))?
        {
            if waited.elapsed() > Duration::from_secs(1) {
                log::warn!("bench: clicking {} again", summary.path.display());
                self.click_mark(&row_mark(summary), "open session: click again", cx)
                    .await?;
                break;
            }
            sleep(cx, Duration::from_millis(2)).await;
        }
        let path = summary.path.clone();
        let what = format!("the history of {}", summary.path.display());
        self.wait_until(&what, cx, move |this, cx| {
            let session = this.visible_chat().read(cx).session().read(cx);
            session.session_file().as_deref() == Some(path.as_path())
                && !session.transcript().items().is_empty()
        })
        .await?;
        cx.update(|window, _| super::frame_since("open session: history shown", start, window))?;
        // The composer's model, and in a chat the web search button.
        self.wait_until("the composer", cx, |this, cx| {
            let session = this.visible_chat().read(cx).session().read(cx);
            session.model().is_some()
                && (session.mode() != Mode::Chat || session.web_search_available())
        })
        .await?;
        cx.update(|window, _| {
            super::frame_since("open session: composer shown", start, window);
        })?;
        sleep(cx, SETTLE).await;
        Ok(())
    }

    /// Asks to delete `summary`, then confirms as the dialog's button would.
    async fn delete(&self, summary: &SessionSummary, cx: &mut AsyncWindowContext) -> Result<()> {
        let doomed = summary.clone();
        self.this.update_in(cx, |this, window, cx| {
            super::frame("delete: open dialog", window);
            this.confirm_delete(None, doomed, window, cx);
        })?;
        sleep(cx, SETTLE).await;
        let start = Instant::now();
        let doomed = summary.clone();
        self.this.update_in(cx, |this, window, cx| {
            super::frame("delete: confirm", window);
            window.close_dialog(cx);
            this.delete_session(None, &doomed, window, cx);
        })?;
        let path = summary.path.clone();
        self.wait_until("the deleted row to go", cx, move |this, cx| {
            this.sidebar_entity().read(cx).summary(&path).is_none()
        })
        .await?;
        cx.update(|window, _| super::frame_since("delete: row gone", start, window))?;
        sleep(cx, SETTLE).await;
        Ok(())
    }

    /// Moves the pointer onto each row in turn (each move changes which row is hovered).
    async fn hover_rows(
        &self,
        sessions: &[SessionSummary],
        cx: &mut AsyncWindowContext,
    ) -> Result<()> {
        for summary in sessions.iter().take(12) {
            let Some((bounds, _)) = crate::demo::lookup(&row_mark(summary)) else {
                continue;
            };
            self.hover(bounds.center(), "hover: sidebar row", cx)?;
            sleep(cx, Duration::from_millis(60)).await;
        }
        sleep(cx, SETTLE).await;
        Ok(())
    }

    /// Sweeps the pointer down the conversation (turn hover actions).
    async fn hover_chat(&self, cx: &mut AsyncWindowContext) -> Result<()> {
        let size = cx.update(|window, _| window.viewport_size())?;
        let x = size.width * 0.6;
        let mut y = px(120.);
        while y < size.height - px(200.) {
            self.hover(point(x, y), "hover: conversation", cx)?;
            sleep(cx, Duration::from_millis(60)).await;
            y += px(45.);
        }
        sleep(cx, SETTLE).await;
        Ok(())
    }

    async fn scroll_chat(&self, cx: &mut AsyncWindowContext) -> Result<()> {
        let size = cx.update(|window, _| window.viewport_size())?;
        let position = point(size.width * 0.6, size.height * 0.45);
        for step in 0..40 {
            let delta = if step < 20 { px(80.) } else { px(-80.) };
            cx.update(|window, cx| {
                super::frame("scroll: conversation", window);
                window.dispatch_event(
                    PlatformInput::Scroll(ScrollEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(0.), delta)),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                        source: ScrollSource::Pointer,
                    }),
                    cx,
                );
            })?;
            sleep(cx, Duration::from_millis(20)).await;
        }
        sleep(cx, SETTLE).await;
        Ok(())
    }

    fn hover(
        &self,
        position: Point<Pixels>,
        name: &str,
        cx: &mut AsyncWindowContext,
    ) -> Result<()> {
        cx.update(|window, cx| {
            super::frame(name, window);
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
            // A move that changes nothing still gets a frame, so it is timed as well.
            window.refresh();
        })
    }

    /// Clicks the middle of mark `id` (see [`crate::demo::mark`]), timed as `name`; returns
    /// when the click was dispatched.
    async fn click_mark(
        &self,
        id: &str,
        name: &str,
        cx: &mut AsyncWindowContext,
    ) -> Result<Instant> {
        let start = Instant::now();
        let bounds = loop {
            if let Some((bounds, _)) = crate::demo::lookup(id)
                && bounds.size.width > px(0.)
            {
                break bounds;
            }
            if start.elapsed() > TIMEOUT {
                bail!("{id} is not on screen");
            }
            sleep(cx, Duration::from_millis(16)).await;
        };
        let position = bounds.center();
        cx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
        })?;
        sleep(cx, Duration::from_millis(40)).await;
        let clicked = Instant::now();
        cx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            super::frame(name, window);
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );
        })?;
        Ok(clicked)
    }
}

/// The mark of a local session's sidebar row.
pub fn row_mark(summary: &SessionSummary) -> String {
    format!("session:{}", summary.path.display())
}

fn file_len(path: &PathBuf) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

async fn sleep(cx: &AsyncWindowContext, duration: Duration) {
    cx.background_executor().timer(duration).await;
}
