//! Formulas in answers: `$…$`, `\(…\)` and `$$…$$` inline, `$$` and `\[` blocks on lines of
//! their own. Which `$` delimit a formula is decided while the Markdown is parsed (pandoc's
//! rules, so "$5 and $10" stay prices: see `vendor/README.md`).
//!
//! `KaTeX` (`katex-native`, a Rust port with `KaTeX`'s fonts) lays a formula out; `usvg` turns
//! its glyphs into paths, and GPUI draws that SVG as a mask in the text color, sharp at any
//! scale and in either theme. A formula `KaTeX` can't parse is left to the text view, which
//! shows its source as written.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use ::markdown::mdast::Node as MdNode;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::text::{
    InlineElement, InlineRenderContext, MarkdownExtensions, MarkdownNode, MarkdownParseContext,
    MarkdownPlugin,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// The inline plugin's node name.
const INLINE: &str = "math";
/// The block plugin's node name.
const BLOCK: &str = "math-block";
/// `KaTeX` sets math 1.21 times the size of the text around it (its `.katex` CSS rule).
const SCALE: f32 = 1.21;
/// How many formulas [`formula`] keeps; a streamed answer lays the same ones out again.
const CACHE_SIZE: usize = 512;

/// A laid-out formula: path-only SVG and its size in ems of the math font.
#[derive(Debug)]
pub struct Formula {
    svg: Vec<u8>,
    width: f32,
    /// Above the baseline.
    height: f32,
    /// Below the baseline.
    depth: f32,
}

/// Formulas laid out so far, by source and display style (`None`: `KaTeX` can't parse it).
type Cache = HashMap<(String, bool), Option<Arc<Formula>>>;

/// `tex` laid out (in display style when `display`), or `None` when `KaTeX` can't parse it.
pub fn formula(tex: &str, display: bool) -> Option<Arc<Formula>> {
    static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);
    let key = (tex.trim().to_owned(), display);
    if let Some(found) = CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return found.clone();
    }
    let made = lay_out(&key.0, display).map(Arc::new);
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if cache.len() >= CACHE_SIZE {
        cache.clear();
    }
    cache.insert(key, made.clone());
    made
}

fn lay_out(tex: &str, display: bool) -> Option<Formula> {
    // The context holds KaTeX's macro tables; making one per formula would cost more than the
    // layout itself.
    thread_local! {
        static KATEX: katex_native::katex::KatexContext = katex_native::katex::KatexContext::default();
    }
    if tex.is_empty() {
        return None;
    }
    let layout = KATEX
        .with(|katex| katex_native::layout(katex, tex, display))
        .ok()?;
    if layout.width <= 0. || layout.height + layout.depth <= 0. {
        return None;
    }
    // The tag of a numbered equation goes to the right, after a gap (see `svg::to_svg`).
    let width = layout.width + layout.tag.as_ref().map_or(0., |tag| 1. + tag.width);
    let height = layout
        .height
        .max(layout.tag.as_ref().map_or(0., |tag| tag.height));
    let depth = layout
        .depth
        .max(layout.tag.as_ref().map_or(0., |tag| tag.depth));
    // KaTeX's SVG names its fonts; GPUI's SVG renderer doesn't have them, so the glyphs go
    // out as paths.
    let svg = katex_native::svg::to_svg(&layout, 16., "black");
    let tree = usvg::Tree::from_str(&svg, katex_native::raster::svg_options()).ok()?;
    let svg = tree.to_string(&usvg::WriteOptions::default());
    #[allow(clippy::cast_possible_truncation)] // ems: small numbers
    Some(Formula {
        svg: svg.into_bytes(),
        width: width as f32,
        height: height as f32,
        depth: depth as f32,
    })
}

/// The formula drawn at `font_size` (the surrounding text's) in `color`.
fn draw(formula: &Formula, font_size: Pixels, color: Hsla) -> Svg {
    let em = font_size * SCALE;
    svg()
        .data(&formula.svg)
        .flex_none()
        .w(em * formula.width)
        .h(em * (formula.height + formula.depth))
        .text_color(color)
}

/// `$…$`, `\(…\)` and inline `$$…$$`.
struct InlineMath;

impl MarkdownPlugin for InlineMath {
    fn name(&self) -> &str {
        INLINE
    }

    fn parse(&self, node: &MdNode, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let MdNode::InlineMath(math) = node else {
            return None;
        };
        let source = cx.node_source(node).unwrap_or(&math.value);
        let display = source.starts_with("$$");
        let formula = formula(&math.value, display)?;
        // Copying the formula gives what the answer wrote.
        Some(
            MarkdownNode::new(INLINE, formula)
                .text(source.to_owned())
                .markdown(source.to_owned()),
        )
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _: &mut Window,
        _: &mut App,
    ) -> Option<InlineElement> {
        let formula = node.data::<Arc<Formula>>()?;
        let baseline = context.font_size() * SCALE * formula.height;
        let element = draw(formula, context.font_size(), context.text_style().color);
        Some(InlineElement::new(element).with_baseline(baseline))
    }
}

/// `$$` and `\[` on lines of their own: centered, scrolling sideways when too wide.
struct BlockMath;

impl MarkdownPlugin for BlockMath {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        BLOCK
    }

    fn parse(&self, node: &MdNode, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let MdNode::Math(math) = node else {
            return None;
        };
        let formula = formula(&math.value, true)?;
        let source = cx.node_source(node).unwrap_or(&math.value).to_owned();
        Some(
            MarkdownNode::new(BLOCK, formula)
                .text(math.value.clone())
                .markdown(source),
        )
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let color = cx.theme().foreground;
        let id = ElementId::Name(format!("math:{}", node.as_markdown()).into());
        div().id(id).w_full().overflow_x_scroll().py_1().child(
            div()
                .flex()
                .min_w_full()
                .justify_center()
                .when_some(node.data::<Arc<Formula>>(), |this, formula| {
                    this.child(draw(formula, font_size, color))
                }),
        )
    }
}

/// Registers both plugins on `extensions`.
pub fn plugins(extensions: MarkdownExtensions) -> MarkdownExtensions {
    extensions.plugin(InlineMath).plugin(BlockMath)
}

#[cfg(test)]
mod tests {
    use super::formula;

    #[test]
    fn formulas_lay_out_as_paths() {
        let inline = formula(r"\frac{a}{b} + x^2", false).unwrap();
        let svg = std::str::from_utf8(&inline.svg).unwrap();
        assert!(svg.contains("<path") && !svg.contains("<text"), "{svg}");
        assert!(inline.width > 1. && inline.height > 0. && inline.depth > 0.);
        // Display style draws the fraction larger.
        let display = formula(r"\frac{a}{b} + x^2", true).unwrap();
        assert!(display.height > inline.height);
        for tex in [
            r"\sum_{n=1}^{\infty} \frac{1}{n^2} = \frac{\pi^2}{6}",
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            r"f(x) = \begin{cases} 1 & x > 0 \\ 0 & \text{otherwise} \end{cases}",
            r"\mathbb{R}^n \to \mathcal{L}(\mathbf{v})",
        ] {
            assert!(formula(tex, true).is_some(), "{tex}");
        }
    }

    #[test]
    fn broken_formulas_are_none() {
        assert!(formula(r"\frac{a}{", false).is_none());
        assert!(formula("   ", false).is_none());
    }
}
