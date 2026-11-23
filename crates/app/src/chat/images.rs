//! Pictures in the chat: small thumbnails next to tool calls that returned images (such as
//! `read` on a screenshot) and above prompts with attachments, and a full-window viewer
//! that browses all the pictures of that call, group or prompt.
//!
//! The transcript keeps images as the base64 pi sent; they are decoded here only when a row
//! showing them is rendered, and kept in a small cache so scrolling doesn't decode again.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use gpui_kit::component::carousel::{
    Carousel, CarouselContent, CarouselItem, CarouselNext, CarouselPrevious, CarouselState,
};
use gpui_kit::component::{
    ActiveTheme as _, FocusableExt as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use transcript::Image;

use crate::theme::palette;

/// Thumbnails shown at most; the rest are counted ("+3") and open in the viewer.
pub const TOOL_THUMBNAILS: usize = 2;
pub const PROMPT_THUMBNAILS: usize = 4;

/// Decoded pictures kept at most; past this the cache starts over. A screenshot is a few MB
/// once decoded, and only the rows on screen need theirs.
const MAX_DECODED: usize = 48;

/// The picture formats GPUI can show, by MIME type.
fn format(mime_type: &str) -> Option<ImageFormat> {
    Some(match mime_type.to_ascii_lowercase().as_str() {
        "image/png" => ImageFormat::Png,
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/webp" => ImageFormat::Webp,
        "image/gif" => ImageFormat::Gif,
        "image/svg+xml" => ImageFormat::Svg,
        "image/bmp" => ImageFormat::Bmp,
        "image/tiff" => ImageFormat::Tiff,
        _ => return None,
    })
}

/// Identifies an image's data without hashing megabytes on every render: the shared buffer's
/// address and length, plus a sample of its start and end in case an address is reused.
fn cache_key(image: &Image) -> (usize, usize, u64) {
    use std::hash::{DefaultHasher, Hash as _, Hasher as _};
    let data = image.data.as_bytes();
    let mut hasher = DefaultHasher::new();
    data[..data.len().min(64)].hash(&mut hasher);
    data[data.len().saturating_sub(64)..].hash(&mut hasher);
    image.mime_type.hash(&mut hasher);
    (image.data.as_ptr() as usize, data.len(), hasher.finish())
}

/// Decoded pictures by [`cache_key`]; `None` for data that can't be shown.
type DecodedCache = HashMap<(usize, usize, u64), Option<Arc<gpui_kit::Image>>>;

thread_local! {
    static DECODED: RefCell<DecodedCache> = RefCell::new(HashMap::new());
}

/// The picture ready to draw, or `None` if it is not one GPUI can show (or not valid base64).
pub fn decoded(image: &Image) -> Option<Arc<gpui_kit::Image>> {
    let key = cache_key(image);
    DECODED.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(found) = cache.get(&key) {
            return found.clone();
        }
        if cache.len() >= MAX_DECODED {
            cache.clear();
        }
        let picture = format(&image.mime_type).and_then(|format| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(image.data.as_bytes())
                .ok()?;
            Some(Arc::new(gpui_kit::Image::from_bytes(format, bytes)))
        });
        cache.insert(key, picture.clone());
        picture
    })
}

/// Up to `max` thumbnails of `images` in a row, then "+N" for the others. Clicking one opens
/// the viewer on it, with all of `images` to browse.
pub fn thumbnails(
    id: impl Into<ElementId>,
    images: &[Image],
    max: usize,
    size: Size<Pixels>,
    cx: &App,
) -> Option<AnyElement> {
    if images.is_empty() {
        return None;
    }
    let id: ElementId = id.into();
    let theme = cx.theme();
    let (border, muted, background) = (theme.border, theme.muted_foreground, palette(cx).code_bg);
    let shown = images.len().min(max);
    let all: Arc<[Image]> = images.into();
    let tiles = (0..shown).map(|index| {
        let all = all.clone();
        let tile = div()
            .id(ElementId::NamedInteger(
                SharedString::from(format!("{id:?}-thumb")),
                index as u64,
            ))
            .flex_none()
            .w(size.width)
            .h(size.height)
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .bg(background)
            .cursor_pointer()
            .hover(|style| style.border_color(muted))
            .tooltip(|window, cx| {
                component::tooltip::Tooltip::new("View full size").build(window, cx)
            })
            .on_click(move |_, window, cx| open_viewer(all.clone(), index, window, cx));
        match decoded(&images[index]) {
            Some(picture) => tile.child(img(picture).size_full().object_fit(ObjectFit::Cover)),
            // Not a format we can draw: a placeholder that still opens the viewer.
            None => tile
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::File).small().text_color(muted)),
        }
    });
    let more = images.len() - shown;
    Some(
        h_flex()
            .flex_none()
            .gap_1()
            .items_center()
            .children(tiles)
            .when(more > 0, |this| {
                let all = all.clone();
                this.child(
                    div()
                        .id(ElementId::NamedInteger(
                            SharedString::from(format!("{id:?}-more")),
                            0,
                        ))
                        .px_1()
                        .text_xs()
                        .text_color(muted)
                        .cursor_pointer()
                        .hover(|style| style.text_color(cx.theme().foreground))
                        .on_click(move |_, window, cx| open_viewer(all.clone(), shown, window, cx))
                        .child(format!("+{more}")),
                )
            })
            .into_any_element(),
    )
}

/// Every picture in the conversation, in order (prompts' attachments and tool results).
pub fn all_images(items: &[transcript::TranscriptItem]) -> Vec<Image> {
    use transcript::{BlockKind, ItemKind};
    items
        .iter()
        .flat_map(|item| match &item.kind {
            ItemKind::User(user) => user.images.clone(),
            ItemKind::Assistant(assistant) => assistant
                .blocks
                .iter()
                .filter_map(|block| match &block.kind {
                    BlockKind::Tool(tool) => Some(tool.images.clone()),
                    _ => None,
                })
                .flatten()
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// Opens the full-window viewer on `images[start]`; ← / → (or the arrows) browse the others,
/// Esc or a click outside closes it.
pub fn open_viewer(images: Arc<[Image]>, start: usize, window: &mut Window, cx: &mut App) {
    let viewer = cx.new(|cx| ImageViewer::new(&images, start, cx));
    let width = (window.viewport_size().width * 0.92).min(px(1600.));
    let focus = viewer.read(cx).carousel.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.w(width).child(viewer.clone())
    });
    // After the dialog has taken focus, give it to the carousel so the arrow keys work.
    window.defer(cx, move |window, cx| focus.focus(window, cx));
}

/// The viewer's content: one slide per picture.
struct ImageViewer {
    carousel: Entity<CarouselState>,
    pictures: Vec<Option<Arc<gpui_kit::Image>>>,
    _observe: Subscription,
}

impl ImageViewer {
    fn new(images: &[Image], start: usize, cx: &mut Context<Self>) -> Self {
        let count = images.len();
        let carousel = cx.new(|_| {
            CarouselState::new(count).with_selected_index(start.min(count.saturating_sub(1)))
        });
        let observe = cx.observe(&carousel, |_, _, cx| cx.notify());
        Self {
            carousel,
            pictures: images.iter().map(decoded).collect(),
            _observe: observe,
        }
    }
}

/// Height of the picture area: most of the window, leaving room for the dialog's chrome.
const VIEWER_HEIGHT_RATIO: f32 = 0.74;

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = window.viewport_size().height * VIEWER_HEIGHT_RATIO;
        let count = self.pictures.len();
        let current = self.carousel.read(cx).selected_index().unwrap_or(0);
        let muted = cx.theme().muted_foreground;
        let slides = self.pictures.iter().enumerate().map(|(index, picture)| {
            CarouselItem::new(("viewer-picture", index), index, &self.carousel)
                .h(height)
                .child(match picture {
                    Some(picture) => img(picture.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain)
                        .into_any_element(),
                    None => div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(muted)
                        .child("This picture can't be shown")
                        .into_any_element(),
                })
        });
        component::v_flex()
            .w_full()
            .gap_2()
            .child(
                Carousel::new("image-viewer", &self.carousel)
                    .accessibility_label("Pictures")
                    // It holds focus only so ← / → work; a ring around the picture would just
                    // be noise.
                    .focus_ring(false)
                    // Room for the previous / next buttons, which sit outside the slides.
                    .mx(px(56.))
                    .child(
                        CarouselContent::new(&self.carousel)
                            .h(height)
                            .children(slides),
                    )
                    .when(count > 1, |this| {
                        this.child(CarouselPrevious::new(&self.carousel))
                            .child(CarouselNext::new(&self.carousel))
                    }),
            )
            .when(count > 1, |this| {
                this.child(
                    div()
                        .w_full()
                        .text_center()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("{} of {count}", current + 1)),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's own `test` attribute into scope.
    use super::{decoded, format};
    use gpui_kit::ImageFormat;
    use std::sync::Arc;
    use transcript::Image;

    /// A 1×1 PNG.
    const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    #[test]
    fn decodes_known_formats_once() {
        let image = Image::new(PIXEL, "image/png");
        let first = decoded(&image).expect("a PNG decodes");
        let again = decoded(&image.clone()).expect("cached");
        assert!(
            Arc::ptr_eq(&first, &again),
            "the shared data is decoded once"
        );
        assert_eq!(first.format(), ImageFormat::Png);
        assert!(decoded(&Image::new(PIXEL, "application/pdf")).is_none());
        assert!(decoded(&Image::new("not base64!", "image/png")).is_none());
    }

    #[test]
    fn formats_by_mime_type() {
        assert_eq!(format("image/JPEG"), Some(ImageFormat::Jpeg));
        assert_eq!(format("image/webp"), Some(ImageFormat::Webp));
        assert_eq!(format("text/plain"), None);
    }
}
