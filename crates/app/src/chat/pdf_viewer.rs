//! The full-window viewer of PDF files. A helper process draws the pages ([`documents::viewer`]),
//! only those that are looked at and their neighbors.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use documents::viewer::layout::{self, Frame, Layout, Place};
use documents::viewer::{Link, PageRequest, Size as PageSize, Target, Viewer};
use gpui_kit::component::button::Button;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IconName, Sizable as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::viewer_dialog;
use crate::config::AppConfig;
use crate::theme::palette;
use crate::ui::{QuietButton as _, spinner};
use crate::ui_state::UiState;

/// Around the visible pages, so many more are drawn.
const AHEAD: usize = 1;
/// And so many are kept.
const KEPT: usize = 3;
/// How far the arrow keys scroll.
const LINE: f32 = 60.;

/// Opens the viewer on the PDF file at `path`. Esc or a click outside closes it.
pub fn open(path: PathBuf, window: &mut Window, cx: &mut App) {
    let viewer = cx.new(|cx| PdfViewer::new(path, cx));
    let focus = viewer.read(cx).focus.clone();
    viewer_dialog::open(viewer, focus, window, cx);
}

enum State {
    Opening,
    Failed(SharedString),
    Open {
        viewer: Arc<Mutex<Viewer>>,
        sizes: Vec<PageSize>,
    },
}

/// How a page is drawn.
#[derive(Clone, Copy, PartialEq)]
struct Look {
    scale: f32,
    dark: bool,
}

struct Page {
    look: Look,
    image: Arc<RenderImage>,
    links: Rc<[Link]>,
}

pub struct PdfViewer {
    path: PathBuf,
    focus: FocusHandle,
    scroll: ScrollHandle,
    state: State,
    /// `None` fills the width.
    zoom: Option<f32>,
    /// As it was last drawn.
    layout: Option<Layout>,
    /// What stays where it is when the zoom changes, from the top of the view.
    anchor: f32,
    pages: HashMap<usize, Page>,
    broken: HashSet<usize>,
    drawing: Option<Task<()>>,
    /// Where links were followed from.
    back: Vec<Place>,
    _opening: Task<()>,
    _release: Subscription,
}

impl PdfViewer {
    fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let exe = cx.global::<AppConfig>().exe.clone();
        let opened = cx.background_executor().spawn({
            let path = path.clone();
            async move {
                let exe = exe.ok_or("the app's own executable could not be found")?;
                Viewer::open(&exe, &path)
            }
        });
        let opening = cx.spawn(async move |this, cx| {
            let opened = opened.await;
            let _ = this.update(cx, |this, cx| {
                this.state = match opened {
                    Ok((viewer, sizes)) => State::Open {
                        viewer: Arc::new(Mutex::new(viewer)),
                        sizes,
                    },
                    Err(error) => State::Failed(format!("It cannot be shown: {error}.").into()),
                };
                cx.notify();
            });
        });
        let release = cx.on_release(|this, cx| {
            for (_, page) in this.pages.drain() {
                cx.drop_image(page.image, None);
            }
        });
        Self {
            path,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            state: State::Opening,
            zoom: None,
            layout: None,
            anchor: 0.,
            pages: HashMap::new(),
            broken: HashSet::new(),
            drawing: None,
            back: Vec::new(),
            _opening: opening,
            _release: release,
        }
    }

    /// Whether pages are dark: as was last chosen, or like the app.
    fn is_dark(cx: &App) -> bool {
        cx.try_global::<UiState>()
            .and_then(|state| state.pdf_dark)
            .unwrap_or_else(|| cx.theme().mode.is_dark())
    }

    fn top(&self) -> f32 {
        -self.scroll.offset().y.as_f32()
    }

    fn scroll_to(&self, top: f32) {
        let x = self.scroll.offset().x;
        let top = match &self.layout {
            Some(layout) => top.min(layout.height - self.scroll.bounds().size.height.as_f32()),
            None => top,
        };
        self.scroll.set_offset(point(x, px(-top.max(0.))));
    }

    fn go(&mut self, place: Place, cx: &mut Context<Self>) {
        if let Some(layout) = &self.layout {
            self.scroll_to(layout.y(place) - layout::GAP);
            cx.notify();
        }
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(place) = self.back.pop() {
            if let Some(layout) = &self.layout {
                self.scroll_to(layout.y(place));
            }
            cx.notify();
        }
    }

    fn follow(&mut self, target: &Target, cx: &mut Context<Self>) {
        match target {
            Target::Uri(uri) => cx.open_url(uri),
            Target::Page { page, y } => {
                if let Some(layout) = &self.layout {
                    self.back.push(layout.place(self.top()));
                }
                self.go(Place { page: *page, y: *y }, cx);
            }
        }
    }

    /// Zooms to `zoom`, with what is at `anchor` in the view staying there.
    fn zoom_to(&mut self, zoom: Option<f32>, anchor: f32, cx: &mut Context<Self>) {
        self.zoom = zoom.map(|zoom| zoom.clamp(layout::MIN_SCALE, layout::MAX_SCALE));
        self.anchor = anchor;
        cx.notify();
    }

    fn zoom_step(&mut self, zoom_in: bool, cx: &mut Context<Self>) {
        if let Some(layout) = &self.layout {
            let zoom = layout::step(layout.scale(), zoom_in);
            let middle = self.scroll.bounds().size.height.as_f32() / 2.;
            self.zoom_to(Some(zoom), middle, cx);
        }
    }

    fn zoom_by(&mut self, factor: f32, at: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(layout) = &self.layout {
            let zoom = layout.scale() * factor;
            let anchor = (at.y - self.scroll.bounds().top()).as_f32();
            self.zoom_to(Some(zoom), anchor, cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(layout) = &self.layout else { return };
        let view = self.scroll.bounds().size.height.as_f32();
        let (top, height) = (self.top(), layout.height);
        let key = &event.keystroke;
        if key.modifiers.secondary(cx) {
            match key.key.as_str() {
                "=" | "+" => self.zoom_step(true, cx),
                "-" => self.zoom_step(false, cx),
                "0" => self.zoom_to(None, 0., cx),
                "[" | "left" => self.go_back(cx),
                "up" => self.scroll_to(0.),
                "down" => self.scroll_to(height),
                _ => return,
            }
        } else if key.modifiers.alt || key.modifiers.control {
            return;
        } else {
            let page = layout.page_at(top + layout::GAP + 1.);
            match key.key.as_str() {
                "up" => self.scroll_to(top - LINE),
                "down" => self.scroll_to(top + LINE),
                "space" if key.modifiers.shift => self.scroll_to(top - view * 0.9),
                "pageup" => self.scroll_to(top - view * 0.9),
                "space" | "pagedown" => self.scroll_to(top + view * 0.9),
                "home" => self.scroll_to(0.),
                "end" => self.scroll_to(height),
                "left" => self.go(
                    Place {
                        page: page.saturating_sub(1),
                        y: 0.,
                    },
                    cx,
                ),
                "right" => {
                    let page = page + 1;
                    self.go(Place { page, y: 0. }, cx);
                }
                _ => return,
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The layout in a view `width` wide, scrolled to where it was before it changed.
    fn lay_out(&mut self, width: f32) -> Option<Layout> {
        let State::Open { sizes, .. } = &self.state else {
            return None;
        };
        let scale = self.zoom.unwrap_or_else(|| layout::fit(sizes, width));
        let new = Layout::new(sizes, scale, width);
        if let Some(old) = self.layout.take()
            && old != new
        {
            let anchor = std::mem::take(&mut self.anchor);
            let place = old.place(self.top() + anchor);
            let middle = (-self.scroll.offset().x.as_f32() + width / 2.) / old.width;
            let left = (middle * new.width - width / 2.).max(0.);
            let top = (new.y(place) - anchor).max(0.);
            self.scroll.set_offset(point(px(-left), px(-top)));
        }
        self.layout = Some(new.clone());
        Some(new)
    }

    /// Starts drawing the first of `wanted` that does not look like `look`.
    fn draw(
        &mut self,
        wanted: impl IntoIterator<Item = usize>,
        look: Look,
        cx: &mut Context<Self>,
    ) {
        let State::Open { viewer, .. } = &self.state else {
            return;
        };
        let next = wanted.into_iter().find(|page| {
            !self.broken.contains(page) && self.pages.get(page).is_none_or(|p| p.look != look)
        });
        let Some(page) = next.filter(|_| self.drawing.is_none()) else {
            return;
        };
        let request = PageRequest {
            page,
            scale: look.scale,
            dark: look.dark,
        };
        let viewer = viewer.clone();
        let drawn = cx.background_executor().spawn(async move {
            let picture = viewer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .draw(&request)?;
            let buffer = image::RgbaImage::from_raw(picture.width, picture.height, picture.pixels)
                .ok_or("its picture has another size")?;
            let image = RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)]);
            Ok::<_, String>((Arc::new(image), picture.links))
        });
        self.drawing = Some(cx.spawn(async move |this, cx| {
            let drawn = drawn.await;
            let _ = this.update(cx, |this, cx| {
                this.drawing = None;
                match drawn {
                    Ok((image, links)) => {
                        let links = links.into();
                        let new = Page { look, image, links };
                        if let Some(old) = this.pages.insert(page, new) {
                            cx.drop_image(old.image, None);
                        }
                    }
                    Err(error) => {
                        log::warn!("page {} of {}: {error}", page + 1, this.path.display());
                        this.broken.insert(page);
                    }
                }
                cx.notify();
            });
        }));
    }

    fn render_pages(
        &mut self,
        view: Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(layout) = self.lay_out(view.width.as_f32()) else {
            return self.render_message(cx);
        };
        let visible = layout.visible(self.top(), view.height.as_f32());
        let count = layout.count();
        let look = Look {
            scale: layout.scale() * window.scale_factor(),
            dark: Self::is_dark(cx),
        };
        let around = |range: &std::ops::Range<usize>, by: usize| {
            range.start.saturating_sub(by)..(range.end + by).min(count)
        };
        let kept = around(&visible, KEPT);
        let gone: Vec<usize> = self
            .pages
            .keys()
            .copied()
            .filter(|page| !kept.contains(page))
            .collect();
        for page in gone {
            if let Some(page) = self.pages.remove(&page) {
                cx.drop_image(page.image, Some(window));
            }
        }
        let wanted = visible.clone().chain(around(&visible, AHEAD));
        self.draw(wanted, look, cx);
        let pages: Vec<_> = visible
            .map(|page| self.render_page(page, layout.frame(page), layout.scale(), look.dark, cx))
            .collect();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .bg(palette(cx).code_bg)
            .child(
                div()
                    .id("pdf-pages")
                    .debug_selector(|| "pdf-pages".into())
                    .size_full()
                    .overflow_scroll()
                    .track_scroll(&self.scroll)
                    .on_pinch(cx.listener(|this, event: &PinchEvent, _, cx| {
                        this.zoom_by(1. + event.delta, event.position, cx);
                    }))
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .w(px(layout.width))
                            .h(px(layout.height))
                            .children(pages),
                    ),
            )
            .child(Scrollbar::vertical(&self.scroll))
            .into_any_element()
    }

    fn render_page(
        &self,
        page: usize,
        frame: Frame,
        scale: f32,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let drawn = self.pages.get(&page);
        let links = drawn.map_or_default(|page| page.links.clone());
        let links = (0..links.len()).map(|index| {
            let link = &links[index];
            let target = link.target.clone();
            let tooltip: SharedString = match &target {
                Target::Uri(uri) => uri.clone().into(),
                Target::Page { page, .. } => format!("Page {}", page + 1).into(),
            };
            div()
                .id(("pdf-link", index))
                .debug_selector(|| "pdf-link".into())
                .absolute()
                .left(px(link.x * scale))
                .top(px(link.y * scale))
                .w(px(link.width * scale))
                .h(px(link.height * scale))
                .rounded_sm()
                .cursor_pointer()
                .hover(|style| style.bg(palette(cx).accent_icon.opacity(0.15)))
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .on_click(cx.listener(move |this, _, _, cx| this.follow(&target, cx)))
        });
        let links: Vec<_> = links.collect();
        div()
            .id(("pdf-page", page))
            .absolute()
            .left(px(frame.x))
            .top(px(frame.y))
            .w(px(frame.width))
            .h(px(frame.height))
            .shadow_sm()
            .bg(if dark {
                rgb(0x0026_2624).into()
            } else {
                white()
            })
            .when_some(drawn, |this, page| {
                this.child(
                    img(page.image.clone())
                        .size_full()
                        .object_fit(ObjectFit::Fill),
                )
            })
            .when(self.broken.contains(&page), |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(palette(cx).subtle)
                        .child("This page can't be shown"),
                )
            })
            .children(links)
            .into_any_element()
    }

    /// Instead of pages.
    fn render_message(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.state {
            State::Failed(error) => viewer_dialog::message(
                error.clone(),
                Button::new("open-elsewhere")
                    .form_other(cx)
                    .label("Open in another app")
                    .on_click(cx.listener(|this, _, _, cx| cx.open_with_system(&this.path))),
                cx,
            )
            .into_any_element(),
            _ => {
                // Until the helper answers, which is hardly seen.
                let (ring, track) = (palette(cx).accent_icon, cx.theme().muted_foreground);
                let ring = spinner::ring(px(22.), ring, track.opacity(0.2), cx);
                viewer_dialog::message(SharedString::default(), ring, cx).into_any_element()
            }
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        use viewer_dialog::button;

        let muted = cx.theme().muted_foreground;
        let dark = Self::is_dark(cx);
        let open = self.layout.as_ref().map(|layout| {
            let page = layout.page_at(self.top() + self.scroll.bounds().size.height.as_f32() / 3.);
            (page + 1, layout.count(), layout.scale())
        });
        viewer_dialog::toolbar(cx)
            .when(!self.back.is_empty(), |this| {
                this.child(
                    button("pdf-back", IconName::ArrowLeft, "Back to the link", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.go_back(cx))),
                )
            })
            .child(viewer_dialog::name(&self.path))
            .when_some(open, |this, (page, count, scale)| {
                this.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("{page} / {count}")),
                )
                .child(
                    button("pdf-zoom-out", IconName::Minus, "Zoom out", cx)
                        .disabled(scale <= layout::MIN_SCALE)
                        .on_click(cx.listener(|this, _, _, cx| this.zoom_step(false, cx))),
                )
                .child(
                    Button::new("pdf-fit")
                        .quiet(cx)
                        .small()
                        .w(px(52.))
                        .label(format!("{:.0}%", scale * 100.))
                        .tooltip("Fill the width")
                        .on_click(cx.listener(|this, _, _, cx| this.zoom_to(None, 0., cx))),
                )
                .child(
                    button("pdf-zoom-in", IconName::Plus, "Zoom in", cx)
                        .disabled(scale >= layout::MAX_SCALE)
                        .on_click(cx.listener(|this, _, _, cx| this.zoom_step(true, cx))),
                )
                .child(
                    if dark {
                        button("pdf-dark", IconName::Sun, "Light pages", cx)
                    } else {
                        button("pdf-dark", IconName::Moon, "Dark pages", cx)
                    }
                    .on_click(cx.listener(move |_, _, _, cx| {
                        UiState::update(cx, |state| state.pdf_dark = Some(!dark));
                        cx.notify();
                    })),
                )
            })
            .child(
                button(
                    "pdf-elsewhere",
                    IconName::ExternalLink,
                    "Open in another app",
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| cx.open_with_system(&this.path))),
            )
            .child(viewer_dialog::close(cx))
    }
}

impl Render for PdfViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let area = viewer_dialog::area(window, cx);
        let view = size(
            area.width,
            (area.height - px(viewer_dialog::TOOLBAR)).max(px(1.)),
        );
        let pages = self.render_pages(view, window, cx);
        v_flex()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .w(area.width)
            .h(area.height)
            .child(self.render_toolbar(cx))
            .child(pages)
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::fmt::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    use documents::viewer::{Link, Target};
    use gpui_kit::component::{Root, WindowExt as _};
    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, Modifiers, Render, TestAppContext,
        VisualTestContext, Window, div,
    };

    use super::{PdfViewer, State, viewer_dialog};
    use crate::config::AppConfig;
    use crate::ui_state::UiState;

    struct Empty;

    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// A viewer of five pages (100 × 200 points) with `links` on each, drawn one pixel large by
    /// a helper that logs what it is asked.
    fn viewer<'a>(
        dir: &std::path::Path,
        links: &[Link],
        cx: &'a mut TestAppContext,
    ) -> (Entity<PdfViewer>, &'a mut VisualTestContext) {
        cx.executor().allow_parking();
        let mut sizes = String::new();
        for page in 0..5 {
            let comma = if page == 0 { "" } else { "," };
            let _ = write!(sizes, "{comma}{{\"width\":100,\"height\":200}}");
        }
        let links = serde_json::to_string(links).unwrap();
        let (log, exe) = (dir.join("log"), dir.join("app"));
        let script = format!(
            "#!/bin/sh\nread line; echo '{{\"opened\":{{\"pages\":[{sizes}]}}}}'\n\
             while read -r line; do\necho \"$line\" >> '{}'\n\
             echo '{{\"drawn\":{{\"width\":1,\"height\":1,\"links\":{links}}}}}'; printf 1234\ndone\n",
            log.display()
        );
        std::fs::write(&exe, script).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_reduce_motion(true);
            let mut config = AppConfig::for_tests("pi".into());
            config.exe = Some(exe);
            cx.set_global(config);
            cx.set_global(UiState::default());
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let empty = cx.new(|_| Empty);
            Root::new(empty, window, cx)
        });
        let viewer = cx.update(|window, cx| {
            let viewer = cx.new(|cx| PdfViewer::new(dir.join("paper.pdf"), cx));
            let focus = viewer.read(cx).focus.clone();
            viewer_dialog::open(viewer.clone(), focus, window, cx);
            viewer
        });
        settle(&viewer, cx);
        (viewer, cx)
    }

    /// Lets the helper answer.
    fn settle(viewer: &Entity<PdfViewer>, cx: &mut VisualTestContext) {
        for _ in 0..500 {
            cx.run_until_parked();
            let busy = cx.read(|cx| {
                let viewer = viewer.read(cx);
                matches!(viewer.state, State::Opening)
                    || viewer.drawing.is_some()
                    || viewer.pages.is_empty()
            });
            if !busy {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the pages were not drawn");
    }

    fn asked(dir: &std::path::Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(dir.join("log"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// Only the pages that are looked at and their neighbors are drawn.
    #[gpui_kit::test]
    fn pages_are_drawn_as_they_are_looked_at(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (viewer, cx) = viewer(dir.path(), &[], cx);
        let pages: Vec<_> = asked(dir.path())
            .iter()
            .map(|request| request["page"].as_u64().unwrap())
            .collect();
        assert_eq!(pages, [0, 1]);
        viewer.update(cx, |viewer, cx| {
            viewer.scroll_to(f32::MAX);
            cx.notify();
        });
        settle(&viewer, cx);
        cx.read(|cx| {
            let viewer = viewer.read(cx);
            assert!(viewer.pages.contains_key(&4) && !viewer.pages.contains_key(&0));
        });
    }

    #[gpui_kit::test]
    fn pages_are_light_or_dark(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (viewer, cx) = viewer(dir.path(), &[], cx);
        let app = cx.update(|_, cx| PdfViewer::is_dark(cx));
        assert_eq!(asked(dir.path())[0]["dark"], app);
        viewer.update(cx, |_, cx| {
            UiState::update(cx, |state| state.pdf_dark = Some(!app));
            cx.notify();
        });
        settle(&viewer, cx);
        assert_eq!(asked(dir.path()).last().unwrap()["dark"], !app);
    }

    #[gpui_kit::test]
    fn links_are_followed(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let link = |y, target| Link {
            x: 0.,
            y,
            width: 100.,
            height: 20.,
            target,
        };
        let links = [
            link(0., Target::Uri("https://pi.dev".into())),
            link(30., Target::Page { page: 3, y: 50. }),
        ];
        let (viewer, cx) = viewer(dir.path(), &links, cx);
        let area = cx.debug_bounds("pdf-pages").unwrap();
        let scale = viewer.update(cx, |viewer, _| viewer.layout.as_ref().unwrap().scale());
        let at = |y: f32| {
            let below = gpui_kit::px(12. + (y + 10.) * scale);
            gpui_kit::point(area.center().x, area.top() + below)
        };
        cx.simulate_click(at(0.), Modifiers::none());
        assert_eq!(cx.opened_url().as_deref(), Some("https://pi.dev"));

        cx.simulate_click(at(30.), Modifiers::none());
        settle(&viewer, cx);
        viewer.update(cx, |viewer, cx| {
            let layout = viewer.layout.clone().unwrap();
            let place = layout.place(viewer.top() + 12.);
            assert_eq!(place.page, 3);
            assert!((place.y - 50.).abs() < 1., "{place:?}");
            viewer.go_back(cx);
            assert!(viewer.top() < 1. && viewer.back.is_empty());
        });
    }

    /// What is looked at stays there.
    #[gpui_kit::test]
    fn zooming_keeps_the_place(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (viewer, cx) = viewer(dir.path(), &[], cx);
        let place = viewer.update(cx, |viewer, cx| {
            let layout = viewer.layout.clone().unwrap();
            viewer.scroll_to(layout.y(super::Place { page: 2, y: 60. }));
            viewer.zoom_step(false, cx);
            layout.place(viewer.top() + viewer.anchor)
        });
        settle(&viewer, cx);
        viewer.update(cx, |viewer, _| {
            let middle = viewer.scroll.bounds().size.height.as_f32() / 2.;
            let now = viewer.layout.as_ref().unwrap().place(viewer.top() + middle);
            assert_eq!(now.page, place.page);
            assert!((now.y - place.y).abs() < 1., "{now:?} {place:?}");
        });
    }

    /// Its keys work at once, and Esc closes it and ends the helper.
    #[gpui_kit::test]
    fn it_opens_and_closes(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (first, cx) = viewer(dir.path(), &[], cx);
        cx.update(|window, cx| window.close_all_dialogs(cx));
        drop(first);
        let path = dir.path().join("paper.pdf");
        cx.update(|window, cx| super::open(path, window, cx));
        let before = asked(dir.path()).len();
        for _ in 0..500 {
            cx.run_until_parked();
            if asked(dir.path()).len() >= before + 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        cx.simulate_keystrokes("end");
        for _ in 0..500 {
            cx.run_until_parked();
            if asked(dir.path()).iter().any(|request| request["page"] == 4) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(asked(dir.path()).iter().any(|request| request["page"] == 4));
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn a_file_that_does_not_open_says_why(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            let mut config = AppConfig::for_tests("pi".into());
            config.exe = None;
            cx.set_global(config);
        });
        let viewer = cx.new(|cx| PdfViewer::new(dir.path().join("a.pdf"), cx));
        cx.run_until_parked();
        viewer.read_with(cx, |viewer, _| {
            assert!(matches!(&viewer.state, State::Failed(error) if error.contains("executable")));
        });
    }
}
