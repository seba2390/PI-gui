//! What is attached to the next prompt, as cards above the message box: a small picture of the
//! file with its extension on it, its name, and what is going on with it.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::files;
use crate::theme::palette;
use crate::ui::{QuietButton as _, spinner};

const WIDTH: f32 = 190.;
const PICTURE_WIDTH: f32 = 44.;
const PICTURE_HEIGHT: f32 = 58.;
/// The cards' height at most (about three rows), then they scroll.
pub const MAX_HEIGHT: f32 = 246.;

/// The small picture of an attached file.
#[derive(Clone, Default)]
pub enum Preview {
    #[default]
    None,
    Image(Arc<Image>),
    /// A picture of the first page.
    Page(PathBuf),
    /// The first lines of the text.
    Text(SharedString),
}

pub struct Card {
    pub id: ElementId,
    pub name: SharedString,
    pub extension: Option<SharedString>,
    pub preview: Preview,
    /// What is going on with it, or else what kind of file it is.
    pub status: SharedString,
    /// Why, if there is more to say.
    pub why: Option<SharedString>,
    /// It does not go along with the prompt.
    pub left_out: bool,
    pub reading: bool,
}

impl Card {
    pub fn render(
        self,
        remove: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        cx: &App,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let tooltip = match &self.why {
            Some(why) => SharedString::from(format!("{}: {why}", self.name)),
            None => self.name.clone(),
        };
        let status_color = if self.left_out {
            palette(cx).warning
        } else {
            theme.muted_foreground
        };
        h_flex()
            .id(self.id.clone())
            .when(self.reading, |this| {
                this.debug_selector(|| "reading-attachment".into())
            })
            .flex_none()
            .w(px(WIDTH))
            .max_w_full()
            .gap_2()
            .p_1p5()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .child(self.picture(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().truncate().child(self.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(status_color)
                            .truncate()
                            .child(self.status.clone()),
                    ),
            )
            .child(
                Button::new("remove")
                    .accessibility_label(format!("Remove {}", self.name))
                    .quiet(cx)
                    .xsmall()
                    .icon(IconName::Close)
                    .on_click(remove),
            )
    }

    fn picture(&self, cx: &App) -> Div {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let tile = div()
            .relative()
            .flex_none()
            .w(px(PICTURE_WIDTH))
            .h(px(PICTURE_HEIGHT))
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(match self.preview {
                // Pages are white, and may be drawn without that.
                Preview::Page(_) => white(),
                _ => palette(cx).code_bg,
            });
        let tile =
            match &self.preview {
                Preview::Image(image) => {
                    tile.child(img(image.clone()).size_full().object_fit(ObjectFit::Cover))
                }
                Preview::Page(page) => {
                    tile.child(img(page.clone()).size_full().object_fit(ObjectFit::Cover))
                }
                Preview::Text(lines) => tile.child(
                    v_flex()
                        .p(px(4.))
                        .text_size(px(5.))
                        .line_height(px(6.5))
                        .text_color(muted)
                        .children(lines.lines().map(|line| {
                            div().h(px(6.5)).whitespace_nowrap().child(line.to_owned())
                        })),
                ),
                Preview::None if self.reading => tile,
                Preview::None => tile
                    .flex()
                    .justify_center()
                    .pt_2()
                    .child(Icon::new(IconName::File).small().text_color(muted)),
            };
        tile.when(self.reading, |this| {
            this.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme.background.opacity(0.72))
                    .child(spinner::ring(
                        px(18.),
                        palette(cx).accent_icon,
                        theme.foreground.opacity(0.15),
                        cx,
                    )),
            )
        })
        .children(
            self.extension
                .clone()
                .map(|extension| files::marker(extension, cx)),
        )
    }
}
