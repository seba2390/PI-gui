//! SSH UI: the device status dot, the add-host dialog, the connect / folder-browser dialog,
//! the host manager, the saved connections in Settings and ssh's password / passphrase prompts. The connection logic itself lives
//! in [`crate::hosts`] and the `remote` crate.

mod add_host;
mod connect;
mod edit_host;
mod host_form;
mod manage;
mod prompt;
mod saved_hosts;

pub use add_host::{AddHost, AddHostEvent};
pub use connect::{ConnectFlow, ConnectFlowEvent};
pub use edit_host::{EditHost, EditHostEvent};
pub use manage::{ManageHosts, ManageHostsEvent};
pub use prompt::open_prompt;
pub use saved_hosts::{SavedHostAction, confirm_remove_host, open_rename_host, saved_hosts};

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
        Some(
            HostStatus::Connecting
            | HostStatus::Installing { .. }
            | HostStatus::NeedsTrust { .. }
            | HostStatus::KeyChanged
            | HostStatus::MissingPi { .. },
        ) => colors.warning,
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
        HostStatus::Installing { step } => format!("Setting up pi: {step}…").into(),
        HostStatus::KeyChanged => "The host key has changed: check it to connect".into(),
        HostStatus::MissingPi { error: None } => {
            "Connected; pi will be installed when you use it".into()
        }
        HostStatus::MissingPi { error: Some(error) } => error.clone(),
        HostStatus::Failed(message) => message.clone(),
    }
}
