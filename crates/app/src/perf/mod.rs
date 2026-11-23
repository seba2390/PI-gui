//! Response-time measurements, off unless `PISTACHIO_PERF=<file.jsonl>` is set.
//!
//! When on, the app writes JSON lines to that file: startup phases, how long each measured
//! interaction took to reach the screen, and render-time statistics per view. Each line
//! carries `t_ms` (since the process started) and `epoch_ms` (wall clock, so a driver
//! script can line it up with when it launched the app and when the process exited).
//! `PISTACHIO_BENCH=1` also runs a fixed navigation script ([`bench`]) and quits;
//! `script/perf.sh` does all of that and prints a table.
//!
//! When off, every helper here costs one relaxed atomic load.

pub mod bench;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::*;
use serde_json::{Value, json};

static START: OnceLock<Instant> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);
static FILE: Mutex<Option<File>> = Mutex::new(None);

thread_local! {
    /// Render durations (ms) per view, summarised by [`summary`].
    static RENDERS: RefCell<BTreeMap<&'static str, Vec<f64>>> = RefCell::default();
}

/// Starts the clock; opens the `PISTACHIO_PERF` file when set. Call first thing in `main`.
pub fn init() {
    START.get_or_init(Instant::now);
    let Some(path) = std::env::var_os("PISTACHIO_PERF").filter(|path| !path.is_empty()) else {
        return;
    };
    match File::create(&path) {
        Ok(file) => {
            *FILE.lock().unwrap_or_else(PoisonError::into_inner) = Some(file);
            ENABLED.store(true, Ordering::Relaxed);
            phase("main");
        }
        Err(error) => eprintln!(
            "PISTACHIO_PERF: cannot write {}: {error}",
            std::path::Path::new(&path).display()
        ),
    }
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

fn ms(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 100_000.).round() / 100.
}

fn write(mut value: Value) {
    let since_start = START.get().map_or(Duration::ZERO, Instant::elapsed);
    let epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    if let Value::Object(map) = &mut value {
        map.insert("t_ms".into(), ms(since_start).into());
        map.insert("epoch_ms".into(), ms(epoch).into());
    }
    if let Some(file) = FILE.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
        let _ = writeln!(file, "{value}").and_then(|()| file.flush());
    }
}

/// Records that startup (or shutdown) reached `name`.
pub fn phase(name: &str) {
    if enabled() {
        write(json!({ "kind": "phase", "name": name }));
    }
}

/// Measures from now until the frame showing the response is on screen, as `name`. Call it
/// as an input is handled: the result is the time the user waits to see the response.
pub fn frame(name: impl Into<String>, window: &Window) {
    if !enabled() {
        return;
    }
    frame_since(name, Instant::now(), window);
}

/// Like [`frame`], from an earlier `start` (an input whose effect showed up later).
pub fn frame_since(name: impl Into<String>, start: Instant, window: &Window) {
    if !enabled() {
        return;
    }
    let name = name.into();
    // Next-frame callbacks run just before a frame is drawn. The first one marks the frame
    // that draws the response; the frame after it starts at the next display refresh,
    // when that response is on screen. (A task spawned after the first callback could run
    // much later: GPUI runs back-to-back frames before ordinary tasks.)
    window.on_next_frame(move |window, _| {
        let before_draw = start.elapsed();
        window.on_next_frame(move |_, _| {
            write(json!({
                "kind": "interaction",
                "name": name,
                "ms": ms(start.elapsed()),
                "before_draw_ms": ms(before_draw),
            }));
        });
    });
}

/// Times a view's `render` (element construction only; layout and paint show up in
/// [`frame`] timings). Hold the guard for the body of `render`.
#[must_use]
pub struct RenderTimer(Option<(&'static str, Instant)>);

pub fn render(view: &'static str) -> RenderTimer {
    RenderTimer(enabled().then(|| (view, Instant::now())))
}

impl Drop for RenderTimer {
    fn drop(&mut self) {
        if let Some((view, start)) = self.0.take() {
            let elapsed = ms(start.elapsed());
            RENDERS.with(|renders| renders.borrow_mut().entry(view).or_default().push(elapsed));
        }
    }
}

/// Writes render statistics per view (count, median, p95, max, total) and resets them.
pub fn summary() {
    if !enabled() {
        return;
    }
    let renders = RENDERS.with(|renders| std::mem::take(&mut *renders.borrow_mut()));
    for (view, mut times) in renders {
        times.sort_by(f64::total_cmp);
        let at = |q: f64| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let index = ((times.len() - 1) as f64 * q).round() as usize;
            times[index]
        };
        write(json!({
            "kind": "renders",
            "view": view,
            "count": times.len(),
            "p50_ms": at(0.5),
            "p95_ms": at(0.95),
            "max_ms": at(1.0),
            "total_ms": (times.iter().sum::<f64>() * 100.).round() / 100.,
        }));
    }
}
