//! Downloads that are asked for: the button that says how much it is, and the dialog that says
//! what exactly is downloaded.

use gpui_kit::component::button::Button;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{App, Div, ParentElement as _, SharedString, Styled as _, Window, div, px};

use super::dialog::open_form_dialog;
use super::{AppIcon, QuietButton as _};
use crate::model_store::size_label;
use crate::theme::palette;

/// One thing that is downloaded.
pub struct Part {
    pub icon: Icon,
    pub name: String,
    /// What it is for.
    pub note: String,
    pub size: u64,
}

/// What a download is.
pub struct Details {
    /// What it is good for.
    pub intro: String,
    pub parts: Vec<Part>,
    pub size: u64,
    pub disk_size: u64,
    /// Where it goes, and its license.
    pub note: String,
}

impl Details {
    /// Checks that the parts are all that is downloaded.
    #[cfg(test)]
    pub fn assert_adds_up(&self) {
        assert!(self.parts.iter().all(|part| part.size > 0));
        let size: u64 = self.parts.iter().map(|part| part.size).sum();
        assert_eq!(size, self.size, "{}", self.intro);
        assert!(self.disk_size >= self.size);
        assert!(self.note.contains("License: "), "{}", self.note);
    }

    /// For a dialog.
    pub fn render(&self, cx: &App) -> Div {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let parts = self.parts.iter().map(|part| {
            h_flex()
                .w_full()
                .gap_3()
                .px_3()
                .py_2()
                .child(
                    part.icon
                        .clone()
                        .small()
                        .text_color(palette(cx).accent_icon),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().child(part.name.clone()))
                        .child(div().text_xs().text_color(muted).child(part.note.clone())),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(size_label(part.size)),
                )
        });
        v_flex()
            .gap_3()
            .child(div().text_sm().text_color(muted).child(self.intro.clone()))
            .child(
                v_flex()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .px_3()
                            .pt_2()
                            .text_xs()
                            .text_color(muted)
                            .child("Missing on this computer"),
                    )
                    .children(parts)
                    .child(
                        h_flex()
                            .w_full()
                            .px_3()
                            .py_2()
                            .border_t_1()
                            .border_color(theme.border)
                            .text_sm()
                            .child(div().flex_1().child("Download"))
                            .child(format!(
                                "{} · {} on disk",
                                size_label(self.size),
                                size_label(self.disk_size)
                            )),
                    ),
            )
            .child(div().text_xs().text_color(muted).child(self.note.clone()))
    }
}

/// What a button that downloads `size` bytes says.
pub fn download_label(size: u64) -> String {
    format!("Download {}", size_label(size))
}

/// The button in a list that downloads `name`, after asking.
pub fn download_button(id: impl Into<SharedString>, name: &str, size: u64) -> Button {
    let label = download_label(size);
    Button::new(id.into())
        .icon(AppIcon::Download)
        .accessibility_label(format!("{label}, {name}"))
        .label(label)
        .outline()
        .xsmall()
}

/// Asks whether to download `name`, showing what that is; `start` is called on a yes.
pub fn ask_to_download(
    name: &str,
    details: Details,
    start: impl Fn(&mut App) + Clone + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let title: SharedString = format!("Download {name}?").into();
    open_form_dialog(None, window, cx, move |dialog, _, cx| {
        let start = start.clone();
        dialog
            .title(title.clone())
            .w(px(540.))
            .child(details.render(cx))
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("download-cancel")
                            .form_other(cx)
                            .icon(IconName::Close)
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("download-start")
                            .form_action(cx)
                            .icon(AppIcon::Download)
                            .label(download_label(details.size))
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                start(cx);
                            }),
                    ),
            )
    });
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::TestAppContext;

    use super::{Details, Part, ask_to_download, download_label};
    use crate::ui::AppIcon;
    use crate::ui::dialog::testing::{click, draw, is_open, press, window};

    #[test]
    fn labels_say_how_much() {
        assert_eq!(download_label(670_576_784), "Download 671 MB");
        assert_eq!(download_label(1_036_613_791), "Download 1.0 GB");
        assert_eq!(download_label(93_939), "Download 94 KB");
    }

    /// Nothing is downloaded without a yes.
    #[gpui_kit::test]
    fn a_download_starts_on_a_yes(cx: &mut TestAppContext) {
        let cx = window(cx);
        let started = Rc::new(Cell::new(0));
        for (answer, times) in [("cancel", 0), ("escape", 0), ("start", 1)] {
            let started_by = started.clone();
            cx.update(|window, cx| {
                let details = Details {
                    intro: "A model.".into(),
                    parts: vec![Part {
                        icon: AppIcon::Brain.into(),
                        name: "Model".into(),
                        note: "All of it".into(),
                        size: 1_000_000,
                    }],
                    size: 1_000_000,
                    disk_size: 1_000_000,
                    note: "License: MIT.".into(),
                };
                let start = move |_: &mut _| started_by.set(started_by.get() + 1);
                ask_to_download("Model", details, start, window, cx);
            });
            draw(cx);
            assert!(is_open(cx));
            match answer {
                "cancel" => click("download-cancel", cx),
                "escape" => press("escape", cx),
                _ => click("download-start", cx),
            }
            assert!(!is_open(cx), "{answer}");
            assert_eq!(started.get(), times, "{answer}");
        }
    }
}
