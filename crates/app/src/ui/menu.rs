//! Menu items the component library has no kind for.

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::*;

use super::FadingText;

/// How wide the text of an item may be. With the check mark, the padding and the scroll bar, this
/// is what a menu has room for (it is never wider than 31.25rem).
const MAX_WIDTH: Rems = rems(27.5);

/// A menu item with a name and, in muted text, what it is. A long text fades out where the menu
/// ends: a plain [`PopupMenuItem::new`] would draw it past the menu's edge. In demo mode it is
/// marked as `id` (see [`crate::demo`]).
pub fn described_item(
    id: impl Into<SharedString>,
    name: impl Into<SharedString>,
    description: impl Into<SharedString>,
) -> PopupMenuItem {
    let (id, name, description) = (id.into(), name.into(), description.into());
    PopupMenuItem::element(move |_, cx| {
        h_flex()
            .id(id.clone())
            .relative()
            .role(Role::Label)
            .aria_label(format!("{name}, {description}"))
            .max_w(MAX_WIDTH)
            .min_w_0()
            .gap_2()
            // The description gives way first.
            .child(
                div()
                    .flex_none()
                    .max_w_full()
                    .child(FadingText::new(name.clone())),
            )
            .child(
                div()
                    .min_w_0()
                    .text_color(cx.theme().muted_foreground)
                    .child(FadingText::new(description.clone())),
            )
            .children(crate::demo::probe(id.clone()))
    })
}
