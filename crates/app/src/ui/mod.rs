//! Small, app-specific UI building blocks shared by the views.

mod drag;
mod icons;
mod text;

pub use drag::WindowDrag;
pub use icons::AppIcon;
pub use text::{relative_time, short_path};

/// Height of the top bar of every column (sidebar, chat, right panel), so their
/// separators line up like one title bar.
pub const TOP_BAR_HEIGHT: gpui_kit::Pixels = gpui_kit::px(44.);

/// Horizontal space reserved for macOS traffic lights in the leftmost top bar.
pub const TRAFFIC_LIGHT_INSET: gpui_kit::Pixels = if cfg!(target_os = "macos") {
    gpui_kit::px(76.)
} else {
    gpui_kit::px(12.)
};
