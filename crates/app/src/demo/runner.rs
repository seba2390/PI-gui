//! Runs a storyboard in the main window, one step at a time, and writes the timeline.
//!
//! Clicks are synthetic GPUI mouse events dispatched to this window (the real controls react
//! exactly as to a user), never OS input, so the user's own pointer is left alone.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result, bail};
use gpui_kit::*;
use serde_json::{Value, json};

use super::script::{ModeName, Step, Storyboard, ThemeName, WebName};
use super::{DemoCursor, lookup};
use crate::chat::session::{Lifecycle, PiSession};
use crate::settings::{AppSettings, ThemeChoice};
use crate::window::MainWindow;

const FRAME: Duration = Duration::from_millis(16);
/// How long a control may take to appear (a menu opening, a panel loading).
const APPEAR: Duration = Duration::from_secs(5);
/// Height of one diff line, for `scroll_diff`.
const DIFF_LINE: f32 = 20.;

/// Starts the storyboard: titles the window, applies its start theme, and runs the steps.
pub fn start(
    storyboard: Arc<Storyboard>,
    cursor: Entity<DemoCursor>,
    window: &mut Window,
    cx: &mut Context<MainWindow>,
) -> Task<()> {
    window.set_window_title(&storyboard.window_title);
    if let Some(theme) = storyboard.theme {
        set_theme(theme, cx);
    }
    cx.spawn_in(window, async move |this, cx| {
        let timeline = Timeline::create(storyboard.timeline.as_deref());
        let mut runner = Runner {
            storyboard,
            cursor,
            this,
            timeline,
            rng: 0x2545_f491_4f6c_dd1d,
            targets: Vec::new(),
        };
        match runner.run(cx).await {
            Ok(()) => runner.timeline.write(json!({ "event": "done" })),
            Err(error) => {
                log::error!("demo: {error:#}");
                runner
                    .timeline
                    .write(json!({ "event": "error", "message": format!("{error:#}") }));
            }
        }
    })
}

fn set_theme(theme: ThemeName, cx: &mut App) {
    let choice = match theme {
        ThemeName::Light => ThemeChoice::Light,
        ThemeName::Dark => ThemeChoice::Dark,
    };
    AppSettings::update(cx, |settings| settings.theme = choice);
}

/// The timeline: JSON lines with the time since the demo started (`t_ms`) and wall-clock time
/// (`epoch_ms`, to line it up with a recording).
struct Timeline {
    file: Option<std::fs::File>,
    start: Instant,
}

impl Timeline {
    fn create(path: Option<&std::path::Path>) -> Self {
        let file = path.and_then(|path| {
            std::fs::File::create(path)
                .map_err(|error| log::error!("demo: cannot write {}: {error}", path.display()))
                .ok()
        });
        Self {
            file,
            start: Instant::now(),
        }
    }

    fn write(&mut self, mut value: Value) {
        let elapsed = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let epoch = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(0));
        if let Value::Object(map) = &mut value {
            map.insert("t_ms".into(), elapsed.into());
            map.insert("epoch_ms".into(), epoch.into());
        }
        if let Some(file) = &mut self.file {
            let _ = writeln!(file, "{value}").and_then(|()| file.flush());
        }
    }
}

struct Runner {
    storyboard: Arc<Storyboard>,
    cursor: Entity<DemoCursor>,
    this: WeakEntity<MainWindow>,
    timeline: Timeline,
    /// Typing jitter; seeded so takes type alike.
    rng: u64,
    /// The controls the current step pointed at, for the timeline.
    targets: Vec<Value>,
}

impl Runner {
    async fn run(&mut self, cx: &mut AsyncWindowContext) -> Result<()> {
        // Let the first frames paint (and record the marks).
        sleep(cx, Duration::from_millis(400)).await;
        let (size, scale) =
            cx.update(|window, _| (window.viewport_size(), window.scale_factor()))?;
        self.timeline.write(json!({
            "event": "ready",
            "window": { "width": f32::from(size.width), "height": f32::from(size.height) },
            "scale": scale,
        }));
        if let Some(file) = self.storyboard.start_file.clone() {
            let waiting = Instant::now();
            while !file.exists() {
                if waiting.elapsed() > Duration::from_secs(180) {
                    bail!("{} was not created", file.display());
                }
                sleep(cx, Duration::from_millis(20)).await;
            }
        }
        self.timeline.start = Instant::now();
        self.timeline.write(json!({ "event": "start" }));
        let start = point(size.width * 0.52, size.height * 0.62);
        self.cursor.update(cx, |cursor, cx| {
            *cursor = DemoCursor::new(start);
            cx.notify();
        });

        let steps = self.storyboard.steps.clone();
        for (index, step) in steps.iter().enumerate() {
            self.targets.clear();
            let name = match step {
                Step::Marker { name } => Some(name.clone()),
                _ => None,
            };
            self.timeline.write(json!({
                "event": "step", "phase": "start", "index": index, "step": step.kind(), "name": name,
            }));
            self.step(step, cx)
                .await
                .with_context(|| format!("step {} ({})", index + 1, step.kind()))?;
            self.timeline.write(json!({
                "event": "step", "phase": "end", "index": index, "step": step.kind(), "name": name,
                "targets": self.targets,
            }));
        }
        Ok(())
    }

    async fn step(&mut self, step: &Step, cx: &mut AsyncWindowContext) -> Result<()> {
        match step {
            Step::Wait {
                ms,
                until_idle: false,
            } => sleep(cx, Duration::from_millis(*ms)).await,
            Step::Wait {
                ms,
                until_idle: true,
            } => {
                let session = self.session(cx)?;
                let start = Instant::now();
                let mut idle_since: Option<Instant> = None;
                while start.elapsed() < Duration::from_millis(*ms) {
                    if session.read_with(cx, |session, _| session.is_busy()) {
                        idle_since = None;
                    } else if idle_since.get_or_insert_with(Instant::now).elapsed()
                        > Duration::from_millis(1000)
                    {
                        break;
                    }
                    sleep(cx, Duration::from_millis(100)).await;
                }
            }
            Step::Marker { .. } => {}
            Step::Mode { mode } => {
                let id = match mode {
                    ModeName::Chat => "mode:chat",
                    ModeName::Code => "mode:code",
                };
                self.click(id, true, cx).await?;
            }
            Step::NewChat { folder } => {
                let welcome = self.this.read_with(cx, |this, _| this.welcome_visible())?;
                let button = if welcome { "welcome-new" } else { "new-chat" };
                match folder {
                    // The button would ask for a folder; click it for show and open the folder.
                    Some(folder) => {
                        self.click(button, false, cx).await?;
                        let folder = folder.clone();
                        self.this.update_in(cx, |this, window, cx| {
                            this.new_chat_in(folder, window, cx);
                        })?;
                    }
                    None => self.click(button, true, cx).await?,
                }
                self.wait_for_session(cx, "pi to list its models", Duration::from_secs(60), |s| {
                    matches!(s.lifecycle(), Lifecycle::Ready) && !s.models().is_empty()
                })
                .await?;
            }
            Step::PickModel { id } => {
                self.wait_for_session(cx, "models", Duration::from_secs(60), |s| {
                    !s.models().is_empty()
                })
                .await?;
                self.pick("model-picker", &format!("model:{id}"), cx)
                    .await?;
                self.wait_for_session(cx, &format!("model {id}"), APPEAR, |s| {
                    s.model().is_some_and(|model| model.id == *id)
                })
                .await?;
            }
            Step::Thinking { level } => {
                self.wait_for_session(cx, "thinking levels", APPEAR, |s| {
                    s.thinking_levels().len() > 1
                })
                .await?;
                let level = self
                    .session(cx)?
                    .read_with(cx, |session, _| session.shown_thinking_level(*level));
                self.pick("thinking-picker", &format!("thinking:{level}"), cx)
                    .await?;
                self.wait_for_session(cx, &format!("thinking level {level}"), APPEAR, |s| {
                    s.thinking_level() == Some(level)
                })
                .await?;
            }
            Step::Web { mode } => {
                self.wait_for_session(cx, "web search", Duration::from_secs(30), |s| {
                    s.web_search_available()
                })
                .await?;
                let mode = match mode {
                    WebName::Off => "off",
                    WebName::Search => "search",
                    WebName::Deep => "deep",
                };
                self.pick("web-mode", &format!("web:{mode}"), cx).await?;
                self.wait_for_session(cx, &format!("web mode {mode}"), APPEAR, |s| {
                    s.web_mode().as_str() == mode
                })
                .await?;
            }
            Step::Type { text, cps } => self.type_text(text, *cps, cx).await?,
            Step::Send => {
                if let Some(required) = &self.storyboard.require_model {
                    let model = self.session(cx)?.read_with(cx, |session, _| {
                        session.model().map(|model| model.id.clone())
                    });
                    if model.as_deref() != Some(required.as_str()) {
                        bail!("refusing to send: the model is {model:?}, not {required}");
                    }
                }
                self.click("send", true, cx).await?;
            }
            Step::WaitIdle { timeout_s } => {
                self.wait_idle(Duration::from_secs(*timeout_s), cx).await?;
            }
            Step::OpenContext => {
                self.click("context-ring", true, cx).await?;
            }
            Step::CloseContext => {
                let composer = self.this.read_with(cx, |this, cx| {
                    this.visible_chat().read(cx).composer().clone()
                })?;
                composer.update(cx, |composer, cx| composer.close_context_breakdown(cx));
            }
            Step::ShowDiff => self.click("show-diff", true, cx).await?,
            Step::ExpandDiff { path } => {
                let id = format!("diff-file:{path}");
                self.target(&id, None, Duration::from_secs(20), cx).await?;
                self.click(&id, true, cx).await?;
            }
            Step::ScrollDiff { lines, ms } => self.scroll_diff(*lines, *ms, cx).await?,
            Step::OpenSettings => self.click("show-settings", true, cx).await?,
            Step::Click { target } => self.click(target, true, cx).await?,
            Step::Theme { theme } => {
                let item = match theme {
                    ThemeName::Light => "theme:light",
                    ThemeName::Dark => "theme:dark",
                };
                self.pick("theme-picker", item, cx).await?;
            }
            Step::Move { x, y } => {
                let size = cx.update(|window, _| window.viewport_size())?;
                self.glide(point(size.width * *x, size.height * *y), cx)
                    .await?;
            }
        }
        Ok(())
    }

    fn session(&self, cx: &mut AsyncWindowContext) -> Result<Entity<PiSession>> {
        self.this.read_with(cx, |this, cx| {
            this.visible_chat().read(cx).session().clone()
        })
    }

    /// Waits until the visible chat's session satisfies `ready`.
    async fn wait_for_session(
        &self,
        cx: &mut AsyncWindowContext,
        what: &str,
        timeout: Duration,
        ready: impl Fn(&PiSession) -> bool,
    ) -> Result<()> {
        let start = Instant::now();
        loop {
            let session = self.session(cx)?;
            if session.read_with(cx, |session, _| ready(session)) {
                return Ok(());
            }
            if start.elapsed() > timeout {
                bail!("timed out waiting for {what}");
            }
            sleep(cx, Duration::from_millis(50)).await;
        }
    }

    /// The bounds of mark `id`, once it is on screen (and recorded after `newer_than`).
    async fn target(
        &self,
        id: &str,
        newer_than: Option<u64>,
        timeout: Duration,
        cx: &mut AsyncWindowContext,
    ) -> Result<Bounds<Pixels>> {
        let start = Instant::now();
        loop {
            if let Some((bounds, seq)) = lookup(id)
                && newer_than.is_none_or(|newer_than| seq > newer_than)
                && bounds.size.width > px(0.)
            {
                return Ok(bounds);
            }
            if start.elapsed() > timeout {
                bail!("{id} did not appear on screen");
            }
            sleep(cx, FRAME).await;
        }
    }

    fn note_target(&mut self, id: &str, bounds: Bounds<Pixels>) {
        self.targets.push(json!({
            "id": id,
            "x": f32::from(bounds.origin.x),
            "y": f32::from(bounds.origin.y),
            "w": f32::from(bounds.size.width),
            "h": f32::from(bounds.size.height),
        }));
    }

    /// Moves the cursor (and the window's mouse) to `position` right away.
    fn move_to(&self, position: Point<Pixels>, cx: &mut AsyncWindowContext) -> Result<()> {
        self.cursor
            .update(cx, |cursor, cx| cursor.set_position(position, cx));
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
        Ok(())
    }

    /// Glides the cursor to `to`, easing in and out, hovering what it passes like a mouse.
    async fn glide(&self, to: Point<Pixels>, cx: &mut AsyncWindowContext) -> Result<()> {
        let from = self.cursor.read_with(cx, |cursor, _| cursor.position());
        let (dx, dy) = (f32::from(to.x - from.x), f32::from(to.y - from.y));
        let distance = dx.hypot(dy);
        if distance < 1. {
            return self.move_to(to, cx);
        }
        let duration = (380. + distance * 0.55).clamp(420., 1100.) / 1000.;
        let start = Instant::now();
        loop {
            let t = (start.elapsed().as_secs_f32() / duration).min(1.);
            let eased = ease_in_out(t);
            self.move_to(point(from.x + px(dx * eased), from.y + px(dy * eased)), cx)?;
            if t >= 1. {
                return Ok(());
            }
            sleep(cx, FRAME).await;
        }
    }

    /// Presses and releases the button where the cursor is; `real: false` only shows it.
    async fn press(&self, real: bool, cx: &mut AsyncWindowContext) -> Result<()> {
        let position = self.cursor.read_with(cx, |cursor, _| cursor.position());
        self.cursor
            .update(cx, |cursor, cx| cursor.set_pressed(true, cx));
        if real {
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
            })?;
        }
        sleep(cx, Duration::from_millis(110)).await;
        self.cursor
            .update(cx, |cursor, cx| cursor.set_pressed(false, cx));
        if real {
            cx.update(|window, cx| {
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
        }
        Ok(())
    }

    /// Glides to the middle of mark `id` and clicks it.
    async fn click(&mut self, id: &str, real: bool, cx: &mut AsyncWindowContext) -> Result<()> {
        let bounds = self.target(id, None, APPEAR, cx).await?;
        self.note_target(id, bounds);
        self.glide(bounds.center(), cx).await?;
        sleep(cx, Duration::from_millis(180)).await;
        self.press(real, cx).await?;
        sleep(cx, Duration::from_millis(120)).await;
        Ok(())
    }

    /// Opens the menu of `button`, points at `item` so it highlights, and chooses it.
    async fn pick(&mut self, button: &str, item: &str, cx: &mut AsyncWindowContext) -> Result<()> {
        let before = lookup(item).map_or(0, |(_, seq)| seq);
        self.click(button, true, cx).await?;
        self.target(item, Some(before), APPEAR, cx).await?;
        // Let the menu settle where it opened before aiming at it.
        sleep(cx, Duration::from_millis(350)).await;
        let bounds = self.target(item, None, APPEAR, cx).await?;
        self.note_target(item, bounds);
        let x = bounds.origin.x + (bounds.size.width * 0.35).min(px(70.));
        self.glide(point(x, bounds.center().y), cx).await?;
        sleep(cx, Duration::from_millis(450)).await;
        self.press(true, cx).await?;
        sleep(cx, Duration::from_millis(250)).await;
        Ok(())
    }

    fn jitter(&mut self) -> f32 {
        // xorshift64
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        #[allow(clippy::cast_precision_loss)]
        let unit = (self.rng >> 40) as f32 / (1u64 << 24) as f32;
        0.55 + unit * 0.9
    }

    async fn type_text(&mut self, text: &str, cps: f32, cx: &mut AsyncWindowContext) -> Result<()> {
        let bounds = self.target("composer-input", None, APPEAR, cx).await?;
        self.note_target("composer-input", bounds);
        // Click towards the right, clear of the text, then hide the pointer like macOS does
        // while typing.
        let x = bounds.origin.x + bounds.size.width - px(90.);
        self.glide(point(x, bounds.center().y), cx).await?;
        sleep(cx, Duration::from_millis(150)).await;
        self.press(true, cx).await?;
        sleep(cx, Duration::from_millis(300)).await;
        self.cursor
            .update(cx, |cursor, cx| cursor.set_visible(false, cx));
        let composer = self.this.read_with(cx, |this, cx| {
            this.visible_chat().read(cx).composer().clone()
        })?;
        let mut buffer = [0u8; 4];
        for character in text.chars() {
            let typed: &str = character.encode_utf8(&mut buffer);
            cx.update(|window, cx| {
                composer.update(cx, |composer, cx| composer.insert_text(typed, window, cx));
            })?;
            let mut delay = 1000. / cps * self.jitter();
            if character == ' ' {
                delay *= 1.2;
            } else if ".,?!".contains(character) {
                delay += 180.;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            sleep(cx, Duration::from_millis(delay as u64)).await;
        }
        Ok(())
    }

    /// Waits until the agent has been idle for a moment. Sending marks the session busy right
    /// away, so a session that is not busy here has already finished.
    async fn wait_idle(&mut self, timeout: Duration, cx: &mut AsyncWindowContext) -> Result<()> {
        let start = Instant::now();
        let session = self.session(cx)?;
        let mut idle_since: Option<Instant> = None;
        loop {
            let (busy, stopped) = session.read_with(cx, |session, _| {
                (
                    session.is_busy(),
                    matches!(session.lifecycle(), Lifecycle::Exited { .. }),
                )
            });
            if stopped {
                bail!("pi stopped");
            }
            if busy {
                idle_since = None;
            } else if idle_since.get_or_insert_with(Instant::now).elapsed()
                > Duration::from_millis(1500)
            {
                return Ok(());
            }
            if start.elapsed() > timeout {
                bail!("the agent was still working after {}s", timeout.as_secs());
            }
            sleep(cx, Duration::from_millis(100)).await;
        }
    }

    async fn scroll_diff(
        &mut self,
        lines: u32,
        ms: u64,
        cx: &mut AsyncWindowContext,
    ) -> Result<()> {
        let bounds = self.target("diff-list", None, APPEAR, cx).await?;
        self.note_target("diff-list", bounds);
        let position = point(
            bounds.center().x,
            bounds.origin.y + bounds.size.height * 0.55,
        );
        self.glide(position, cx).await?;
        sleep(cx, Duration::from_millis(250)).await;
        #[allow(clippy::cast_precision_loss)]
        let (total, duration) = (lines as f32 * DIFF_LINE, ms as f32 / 1000.);
        let start = Instant::now();
        let mut scrolled = 0.;
        loop {
            let t = (start.elapsed().as_secs_f32() / duration).min(1.);
            let goal = total * ease_in_out(t);
            let delta = goal - scrolled;
            scrolled = goal;
            cx.update(|window, cx| {
                window.dispatch_event(
                    PlatformInput::Scroll(ScrollEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(0.), px(-delta))),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                        source: ScrollSource::Pointer,
                    }),
                    cx,
                );
            })?;
            if t >= 1. {
                return Ok(());
            }
            sleep(cx, FRAME).await;
        }
    }
}

async fn sleep(cx: &AsyncWindowContext, duration: Duration) {
    cx.background_executor().timer(duration).await;
}

fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        4. * t * t * t
    } else {
        1. - (-2. * t + 2.).powi(3) / 2.
    }
}
