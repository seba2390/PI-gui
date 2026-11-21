//! SSH UI: the device status dot, the add-host dialog and the connect / folder-browser
//! dialog. The connection logic itself lives in [`crate::hosts`] and the `remote` crate.

mod add_host;
mod connect;

pub use add_host::{AddHost, AddHostEvent};
pub use connect::{ConnectFlow, ConnectFlowEvent};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

use crate::hosts::HostStatus;
use crate::theme::palette;

/// The small status light: green connected, amber connecting / needs attention, red failed,
/// grey not connected.
pub fn status_dot(status: Option<&HostStatus>, cx: &App) -> Div {
    let colors = palette(cx);
    let color = match status {
        // `None` is "This computer": always available.
        None | Some(HostStatus::Connected) => colors.success,
        Some(HostStatus::Connecting | HostStatus::NeedsTrust { .. } | HostStatus::MissingPi) => {
            colors.warning
        }
        Some(HostStatus::Failed(_)) => colors.error,
        Some(HostStatus::Idle) => cx.theme().muted_foreground.opacity(0.5),
    };
    div().size(px(7.)).flex_none().rounded_full().bg(color)
}

/// A one-line explanation of a status, for tooltips.
pub fn status_text(status: &HostStatus) -> SharedString {
    match status {
        HostStatus::Idle => "Not connected".into(),
        HostStatus::Connecting => "Connecting…".into(),
        HostStatus::Connected => "Connected".into(),
        HostStatus::NeedsTrust { .. } => "Unknown host key: confirm it to connect".into(),
        HostStatus::MissingPi => "Connected, but pi is not installed there".into(),
        HostStatus::Failed(message) => message.clone(),
    }
}
