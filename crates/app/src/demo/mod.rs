//! Demo mode (`PISTACHIO_DEMO=<storyboard.toml>`): drives the real UI through a storyboard for
//! recording showcase videos.
//!
//! Controls the storyboard points at are *marked*: in demo mode a [`mark`] or [`probe`]
//! records the control's on-screen bounds during prepaint, so the [`runner`] can glide its
//! cursor there and click it with synthetic in-window mouse events (never OS input). Outside
//! demo mode the helpers leave the UI untouched.

mod cursor;
mod runner;
pub mod script;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::*;

pub use cursor::DemoCursor;
pub use runner::start;
pub use script::Storyboard;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Turns marks on for this process (demo mode).
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// A marked control's last painted bounds (window coordinates) and when they were recorded.
#[derive(Debug, Clone, Copy)]
struct Mark {
    bounds: Bounds<Pixels>,
    seq: u64,
}

thread_local! {
    static MARKS: RefCell<(u64, HashMap<SharedString, Mark>)> = RefCell::default();
}

fn record(id: &SharedString, bounds: Bounds<Pixels>) {
    MARKS.with(|marks| {
        let (seq, marks) = &mut *marks.borrow_mut();
        *seq += 1;
        marks.insert(id.clone(), Mark { bounds, seq: *seq });
    });
}

/// The bounds of mark `id` and their sequence number (newer recordings have larger ones).
pub(crate) fn lookup(id: &str) -> Option<(Bounds<Pixels>, u64)> {
    MARKS.with(|marks| marks.borrow().1.get(id).map(|mark| (mark.bounds, mark.seq)))
}

/// An invisible element filling its parent that records the parent's bounds as `id`. The
/// parent must be `relative()`. `None` outside demo mode.
pub fn probe(id: impl Into<SharedString>) -> Option<AnyElement> {
    if !enabled() {
        return None;
    }
    let id = id.into();
    Some(
        canvas(move |bounds, _, _| record(&id, bounds), |_, (), _, _| {})
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into_any_element(),
    )
}

/// `element`, with its bounds recorded as `id` in demo mode (unchanged otherwise).
pub fn mark(id: impl Into<SharedString>, element: impl IntoElement) -> AnyElement {
    match probe(id) {
        Some(probe) => div()
            .relative()
            .flex_none()
            .child(element)
            .child(probe)
            .into_any_element(),
        None => element.into_any_element(),
    }
}

/// A menu item labelled `label`; in demo mode its label is marked as `id` so the demo cursor
/// can point at it.
pub fn menu_item(label: impl Into<SharedString>, id: impl Into<SharedString>) -> PopupMenuItem {
    let label = label.into();
    if !enabled() {
        return PopupMenuItem::new(label);
    }
    let id = id.into();
    PopupMenuItem::element(move |_, _| {
        div()
            .relative()
            .flex_1()
            .child(label.clone())
            .children(probe(id.clone()))
    })
}
