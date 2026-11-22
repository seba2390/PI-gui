//! A small copy-to-clipboard button with the app's hover style.
//!
//! gpui-kit's `Clipboard` uses the ghost style, whose hover is nearly invisible on code
//! backgrounds; this one uses [`QuietButton`](super::QuietButton) and shows a green check for
//! two seconds after copying.

use std::time::Duration;

use gpui_kit::component::button::Button;
use gpui_kit::component::{Icon, IconName, Sizable as _};
use gpui_kit::*;

use super::QuietButton as _;
use crate::theme::palette;

/// How long the check mark stays after copying.
const CONFIRM_FOR: Duration = Duration::from_secs(2);

#[derive(IntoElement)]
pub struct CopyButton {
    id: ElementId,
    value: SharedString,
    tooltip: SharedString,
}

impl CopyButton {
    pub fn new(id: impl Into<ElementId>, value: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            tooltip: "Copy".into(),
        }
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = tooltip.into();
        self
    }
}

#[derive(Default)]
struct CopyState {
    copied: bool,
}

impl RenderOnce for CopyButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| CopyState::default());
        let copied = state.read(cx).copied;
        let icon = if copied {
            Icon::new(IconName::Check).text_color(palette(cx).success)
        } else {
            Icon::new(IconName::Copy)
        };
        let value = self.value;
        Button::new(self.id)
            .quiet(cx)
            .xsmall()
            .icon(icon)
            .accessibility_label(if copied {
                "Copied".into()
            } else {
                self.tooltip.clone()
            })
            .tooltip(if copied {
                "Copied".into()
            } else {
                self.tooltip
            })
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
                state.update(cx, |state, cx| {
                    state.copied = true;
                    cx.notify();
                });
                let state = state.downgrade();
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(CONFIRM_FOR).await;
                    let _ = state.update(cx, |state, cx| {
                        state.copied = false;
                        cx.notify();
                    });
                })
                .detach();
            })
    }
}
