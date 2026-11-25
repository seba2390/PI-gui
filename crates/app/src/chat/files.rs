//! Files attached to a prompt, whose text went along with it: small pages above the prompt, like
//! its pictures. Clicking a PDF shows it here, and another document opens it.

use documents::{Kind, attached};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_sessions::prompt::AttachedFile;

use super::pdf_viewer;
use crate::config::AppConfig;
use crate::theme::palette;

const WIDTH: f32 = 60.;
const HEIGHT: f32 = 80.;

/// `files` as pages.
pub fn pages(prompt: u64, files: &[AttachedFile], cx: &App) -> Vec<AnyElement> {
    let dir = cx
        .try_global::<AppConfig>()
        .map(|config| config.documents_dir.clone())
        .unwrap_or_default();
    files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let id = SharedString::from(format!("attached-{prompt}-{index}"));
            page(id, file, &dir, cx)
        })
        .collect()
}

fn page(id: SharedString, file: &AttachedFile, dir: &std::path::Path, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (border, muted) = (theme.border, theme.muted_foreground);
    let picture = file.id.as_deref().and_then(|id| attached::picture(dir, id));
    let drawn = picture.is_some();
    let name = SharedString::from(file.name.clone());
    let (title, dir, file_id) = (name.clone(), dir.to_path_buf(), file.id.clone());
    div()
        .id(id)
        .debug_selector(|| "attached-file".into())
        .relative()
        .flex_none()
        .w(px(WIDTH))
        .h(px(HEIGHT))
        .rounded_md()
        .overflow_hidden()
        .border_1()
        .border_color(border)
        // Pages are white, and may be drawn without that.
        .bg(if drawn { white() } else { palette(cx).code_bg })
        .cursor_pointer()
        .hover(|style| style.border_color(muted))
        .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
        .on_click(move |_, window, cx| {
            let path = file_id
                .as_deref()
                .and_then(|id| attached::original(&dir, id));
            match path {
                Some(path) if Kind::of(&path) == Some(Kind::Pdf) => {
                    pdf_viewer::open(path, window, cx);
                }
                Some(path) if Kind::of(&path).is_some() => cx.open_with_system(&path),
                // Opening another kind of file could run it.
                Some(path) => cx.reveal_path(&path),
                None => {
                    window.push_notification(
                        format!("{name} is not where it was attached from."),
                        cx,
                    );
                }
            }
        })
        .when_some(picture, |this, picture| {
            this.child(img(picture).size_full().object_fit(ObjectFit::Cover))
        })
        .when(!drawn, |this| {
            this.child(
                v_flex()
                    .size_full()
                    .p_1()
                    .text_size(px(10.))
                    .line_height(px(13.))
                    .text_color(muted)
                    .child(div().w_full().line_clamp(4).child(file.name.clone())),
            )
        })
        .children(file.extension().map(|extension| marker(extension, cx)))
        .into_any_element()
}

/// What kind of file a page or picture is (".pdf"), in its corner.
pub fn marker(extension: impl Into<SharedString>, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .debug_selector(|| "file-marker".into())
        .absolute()
        .left(px(3.))
        .bottom(px(3.))
        .px_1()
        .rounded_sm()
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .text_size(px(10.))
        .line_height(px(13.))
        .text_color(theme.foreground)
        .child(extension.into())
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::{Root, WindowExt as _};
    use gpui_kit::{
        AppContext as _, Context, IntoElement, Modifiers, ParentElement as _, Render,
        TestAppContext, Window, div,
    };
    use pi_sessions::prompt::AttachedFile;

    use crate::config::AppConfig;

    struct Message(Vec<AttachedFile>);

    impl Render for Message {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().children(super::pages(1, &self.0, cx))
        }
    }

    #[gpui_kit::test]
    fn a_pdf_is_shown_here(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.pdf");
        std::fs::write(&path, "%PDF-1.4").unwrap();
        let documents_dir = dir.path().join("documents");
        let id = documents::attached::remember(&documents_dir, &path);
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_reduce_motion(true);
            let mut config = AppConfig::for_tests("pi".into());
            config.documents_dir = documents_dir;
            config.exe = None;
            cx.set_global(config);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let name = "report.pdf".into();
            let message = cx.new(|_| Message(vec![AttachedFile { name, id }]));
            Root::new(message, window, cx)
        });
        let page = cx.debug_bounds("attached-file").unwrap();
        cx.simulate_click(page.center(), Modifiers::none());
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    }
}
