//! Grid geometry and painting for the terminal panel.
//!
//! [`GridGeometry`] maps between window pixels and terminal cells (pure, unit tested).
//! [`paint`] draws a [`Snapshot`]: backgrounds, selection, text and cursor. Text is shaped per
//! run of identically styled cells with a forced advance of one cell, so every glyph lands on
//! its column; GPUI caches shaped lines across frames, so unchanged rows cost almost nothing.

use gpui_kit::*;
use term::{Cell, CellFlags, CursorShape, GridPoint, Rgb, Snapshot};

/// Font and cell size of the grid, resolved from the theme's monospace font.
#[derive(Clone, Debug, PartialEq)]
pub struct CellMetrics {
    pub font: Font,
    pub font_size: Pixels,
    pub cell: Size<Pixels>,
}

impl CellMetrics {
    /// Measures `family` at `font_size`. Line height is 1.35× the size, snapped to whole
    /// pixels so rows never blur.
    pub fn measure(family: SharedString, font_size: Pixels, window: &Window) -> Self {
        let font = font(family);
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        let width = text_system
            .advance(font_id, font_size, 'm')
            .map_or(font_size * 0.6, |advance| advance.width);
        let height = (font_size * 1.35).round();
        Self {
            font,
            font_size,
            cell: size(width, height),
        }
    }
}

/// Where the grid sits in the window and how many cells fit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GridGeometry {
    pub origin: Point<Pixels>,
    pub cell: Size<Pixels>,
    pub cols: u16,
    pub rows: u16,
}

impl GridGeometry {
    /// The largest grid of `cell`s that fits in `bounds` (at least 2×1, like alacritty).
    pub fn fit(bounds: Bounds<Pixels>, cell: Size<Pixels>) -> Self {
        let count = |extent: Pixels, step: Pixels, min: u16| -> u16 {
            if step <= px(0.) {
                return min;
            }
            let n = (extent / step).floor();
            // Saturating float → int cast; the clamp keeps it in u16 range.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let n = n.clamp(0., f32::from(u16::MAX)) as u16;
            n.max(min)
        };
        Self {
            origin: bounds.origin,
            cell,
            cols: count(bounds.size.width, cell.width, 2),
            rows: count(bounds.size.height, cell.height, 1),
        }
    }

    /// The cell under `position`, clamped into the grid (so drags outside still select).
    pub fn cell_at(&self, position: Point<Pixels>) -> GridPoint {
        let clamp = |offset: Pixels, step: Pixels, count: u16| -> usize {
            if step <= px(0.) || count == 0 {
                return 0;
            }
            let ix = (offset / step).floor().max(0.);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let ix = ix.min(f32::from(count - 1)) as usize;
            ix
        };
        GridPoint::new(
            clamp(position.y - self.origin.y, self.cell.height, self.rows),
            clamp(position.x - self.origin.x, self.cell.width, self.cols),
        )
    }

    /// Bounds of `width` cells starting at (`row`, `col`).
    pub fn cells_bounds(&self, row: usize, col: usize, width: usize) -> Bounds<Pixels> {
        bounds(
            point(
                self.origin.x + self.cell.width * to_f32(col),
                self.origin.y + self.cell.height * to_f32(row),
            ),
            size(self.cell.width * to_f32(width), self.cell.height),
        )
    }
}

#[allow(clippy::cast_precision_loss)]
fn to_f32(n: usize) -> f32 {
    n as f32
}

pub fn hsla(color: Rgb) -> Hsla {
    Rgba {
        r: f32::from(color.r) / 255.,
        g: f32::from(color.g) / 255.,
        b: f32::from(color.b) / 255.,
        a: 1.,
    }
    .into()
}

/// UI colors the snapshot does not carry.
#[derive(Clone, Copy, Debug)]
pub struct PaintStyle {
    pub selection: Hsla,
    pub focused: bool,
}

/// Paints `snapshot` into `geometry`. Must be called during the paint phase.
pub fn paint(
    snapshot: &Snapshot,
    geometry: &GridGeometry,
    metrics: &CellMetrics,
    style: PaintStyle,
    window: &mut Window,
    cx: &mut App,
) {
    let default_bg = snapshot.background;
    let mut text = String::new();

    for (row, line) in snapshot.lines.iter().enumerate() {
        // Backgrounds first, so text and the cursor draw on top.
        for run in line.runs() {
            let bg = run.style().bg;
            if bg != default_bg {
                window.paint_quad(fill(
                    geometry.cells_bounds(row, run.start_col, run.cells.len()),
                    hsla(bg),
                ));
            }
        }
        if let Some((start, end)) = snapshot.selection
            && (start.row..=end.row).contains(&row)
        {
            let from = if row == start.row { start.col } else { 0 };
            let to = if row == end.row {
                end.col
            } else {
                usize::from(snapshot.cols).saturating_sub(1)
            };
            if to >= from {
                window.paint_quad(fill(
                    geometry.cells_bounds(row, from, to - from + 1),
                    style.selection,
                ));
            }
        }

        for run in line.runs() {
            let cell = run.style();
            let decorated = cell
                .flags
                .intersects(CellFlags::ALL_UNDERLINES | CellFlags::STRIKEOUT);
            // Split at wide characters so each segment starts on its own column.
            let mut col = run.start_col;
            let mut seg_start = col;
            text.clear();
            for c in run.cells {
                if c.is_spacer() {
                    col += 1;
                    if text.is_empty() {
                        seg_start = col;
                    }
                    continue;
                }
                text.push(c.ch);
                if let Some(zw) = &c.zerowidth {
                    text.push_str(zw);
                }
                col += 1;
                if c.flags.contains(CellFlags::WIDE) {
                    paint_segment(
                        &text, row, seg_start, cell, decorated, geometry, metrics, window, cx,
                    );
                    text.clear();
                    seg_start = col + 1;
                }
            }
            paint_segment(
                &text, row, seg_start, cell, decorated, geometry, metrics, window, cx,
            );
        }
    }

    if let Some(cursor) = snapshot.cursor {
        let width = if cursor.wide { 2 } else { 1 };
        let cell_bounds = geometry.cells_bounds(cursor.row, cursor.col, width);
        let color = hsla(cursor.color);
        let shape = if style.focused {
            cursor.shape
        } else {
            CursorShape::HollowBlock
        };
        match shape {
            CursorShape::Block => {
                window.paint_quad(fill(cell_bounds, color));
                // Redraw the character under the cursor in the background color.
                if let Some(cell) = snapshot
                    .lines
                    .get(cursor.row)
                    .and_then(|line| line.cells.get(cursor.col))
                    && cell.ch != ' '
                {
                    let mut inverted = cell.clone();
                    inverted.fg = snapshot.background;
                    text.clear();
                    text.push(cell.ch);
                    paint_segment(
                        &text, cursor.row, cursor.col, &inverted, false, geometry, metrics, window,
                        cx,
                    );
                }
            }
            CursorShape::Beam => {
                window.paint_quad(fill(
                    bounds(cell_bounds.origin, size(px(2.), cell_bounds.size.height)),
                    color,
                ));
            }
            CursorShape::Underline => {
                let y = cell_bounds.bottom() - px(2.);
                window.paint_quad(fill(
                    bounds(
                        point(cell_bounds.origin.x, y),
                        size(cell_bounds.size.width, px(2.)),
                    ),
                    color,
                ));
            }
            CursorShape::HollowBlock => {
                window.paint_quad(outline(cell_bounds, color, BorderStyle::Solid));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_segment(
    text: &str,
    row: usize,
    col: usize,
    cell: &Cell,
    decorated: bool,
    geometry: &GridGeometry,
    metrics: &CellMetrics,
    window: &mut Window,
    cx: &mut App,
) {
    // Blank, undecorated runs need no glyphs (their background is already painted).
    let trimmed = if decorated {
        text
    } else {
        text.trim_end_matches(' ')
    };
    if trimmed.is_empty() {
        return;
    }
    let flags = cell.flags;
    let mut font = metrics.font.clone();
    if flags.contains(CellFlags::BOLD) {
        font.weight = FontWeight::BOLD;
    }
    if flags.contains(CellFlags::ITALIC) {
        font.style = FontStyle::Italic;
    }
    let fg = hsla(cell.fg);
    let underline = flags
        .intersects(CellFlags::ALL_UNDERLINES)
        .then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(cell.underline_color.map_or(fg, hsla)),
            wavy: flags.contains(CellFlags::UNDERCURL),
        });
    let strikethrough = flags
        .contains(CellFlags::STRIKEOUT)
        .then(|| StrikethroughStyle {
            thickness: px(1.),
            color: Some(fg),
        });
    let run = TextRun {
        len: trimmed.len(),
        font,
        color: fg,
        background_color: None,
        underline,
        strikethrough,
    };
    let shaped = window.text_system().shape_line(
        SharedString::from(trimmed.to_owned()),
        metrics.font_size,
        &[run],
        Some(geometry.cell.width),
    );
    let origin = geometry.cells_bounds(row, col, 1).origin;
    if let Err(error) = shaped.paint(
        origin,
        geometry.cell.height,
        TextAlign::Left,
        None,
        window,
        cx,
    ) {
        log::debug!("terminal text paint failed: {error:#}");
    }
}

/// Draws IME composition (`marked`, not yet sent to the shell) over the cursor cell,
/// underlined like in other macOS text fields, on the terminal's background.
pub fn paint_marked_text(
    marked: &str,
    snapshot: &Snapshot,
    geometry: &GridGeometry,
    metrics: &CellMetrics,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(cursor) = snapshot.cursor else {
        return;
    };
    // The cursor color reads well on the background in both themes.
    let fg = hsla(cursor.color);
    let run = TextRun {
        len: marked.len(),
        font: metrics.font.clone(),
        color: fg,
        background_color: None,
        underline: Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(fg),
            wavy: false,
        }),
        strikethrough: None,
    };
    let shaped = window.text_system().shape_line(
        SharedString::from(marked.to_owned()),
        metrics.font_size,
        &[run],
        None,
    );
    let origin = geometry.cells_bounds(cursor.row, cursor.col, 1).origin;
    window.paint_quad(fill(
        bounds(origin, size(shaped.width, geometry.cell.height)),
        hsla(snapshot.background),
    ));
    if let Err(error) = shaped.paint(
        origin,
        geometry.cell.height,
        TextAlign::Left,
        None,
        window,
        cx,
    ) {
        log::debug!("terminal marked text paint failed: {error:#}");
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{bounds, point, px, size};
    use term::GridPoint;

    use super::GridGeometry;

    #[test]
    fn fits_whole_cells_with_minimums() {
        let cell = size(px(8.), px(16.));
        let geometry = GridGeometry::fit(
            bounds(point(px(10.), px(20.)), size(px(803.), px(330.))),
            cell,
        );
        assert_eq!((geometry.cols, geometry.rows), (100, 20));
        let tiny = GridGeometry::fit(bounds(point(px(0.), px(0.)), size(px(3.), px(3.))), cell);
        assert_eq!((tiny.cols, tiny.rows), (2, 1));
        let degenerate = GridGeometry::fit(
            bounds(point(px(0.), px(0.)), size(px(100.), px(100.))),
            size(px(0.), px(0.)),
        );
        assert_eq!((degenerate.cols, degenerate.rows), (2, 1));
    }

    #[test]
    fn maps_pixels_to_cells_and_clamps() {
        let geometry = GridGeometry::fit(
            bounds(point(px(10.), px(20.)), size(px(80.), px(160.))),
            size(px(8.), px(16.)),
        );
        assert_eq!(
            geometry.cell_at(point(px(10.), px(20.))),
            GridPoint::new(0, 0)
        );
        assert_eq!(
            geometry.cell_at(point(px(27.9), px(36.))),
            GridPoint::new(1, 2)
        );
        assert_eq!(
            geometry.cell_at(point(px(-50.), px(-50.))),
            GridPoint::new(0, 0)
        );
        assert_eq!(
            geometry.cell_at(point(px(5000.), px(5000.))),
            GridPoint::new(9, 9)
        );
        let b = geometry.cells_bounds(2, 3, 4);
        assert_eq!(b.origin, point(px(34.), px(52.)));
        assert_eq!(b.size, size(px(32.), px(16.)));
    }
}
