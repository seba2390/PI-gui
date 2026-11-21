//! Render snapshots: a self-contained, fully color-resolved copy of the visible viewport.
//!
//! # Layout
//!
//! A [`Snapshot`] holds exactly `rows` [`Line`]s, each with exactly `cols` [`Cell`]s, so cell
//! `(row, col)` is `snapshot.lines[row].cells[col]` and hit testing / selection painting is plain
//! indexing. All colors are already resolved to [`Rgb`] (palette, 256-color, truecolor, runtime
//! OSC overrides, dim, optional bold-as-bright, inverse and hidden), so a renderer only paints.
//!
//! Wide characters (CJK, most emoji) occupy two cells: the first has [`CellFlags::WIDE`] and
//! holds the character, the second has [`CellFlags::WIDE_SPACER`] and must not be drawn as text
//! (its background may still be painted).
//!
//! To build text runs cheaply, [`Line::runs`] groups adjacent cells with identical style without
//! allocating.
//!
//! # Allocation
//!
//! [`Terminal::snapshot_into`](crate::Terminal::snapshot_into) refills an existing snapshot and
//! reuses its line/cell buffers, so a renderer that keeps one `Snapshot` around performs no
//! per-frame allocation in the steady state (zero-width combining characters are the only
//! exception, and they are rare). A `Cell` is 32 bytes on 64-bit targets.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line as GridLine, Point};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell as TermCell, Flags};
use alacritty_terminal::vte::ansi::{CursorShape as AnsiCursorShape, NamedColor};

use crate::color::{ColorResolver, Palette, Rgb};
use crate::mode::TermModeFlags;

/// A cell position in the visible viewport (`row` 0 is the top visible line).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GridPoint {
    pub row: usize,
    pub col: usize,
}

impl GridPoint {
    pub const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

bitflags::bitflags! {
    /// Text attributes of a cell. Colors in [`Cell`] already account for `INVERSE`, `DIM` and
    /// `HIDDEN`; the flags are kept for information.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct CellFlags: u16 {
        const BOLD             = 1 << 0;
        const ITALIC           = 1 << 1;
        const UNDERLINE        = 1 << 2;
        const DOUBLE_UNDERLINE = 1 << 3;
        const UNDERCURL        = 1 << 4;
        const DOTTED_UNDERLINE = 1 << 5;
        const DASHED_UNDERLINE = 1 << 6;
        const STRIKEOUT        = 1 << 7;
        const DIM              = 1 << 8;
        const INVERSE          = 1 << 9;
        const HIDDEN           = 1 << 10;
        /// First half of a double-width character.
        const WIDE             = 1 << 11;
        /// Second half of a double-width character: do not draw text here.
        const WIDE_SPACER      = 1 << 12;
        /// Any underline style.
        const ALL_UNDERLINES = Self::UNDERLINE.bits()
            | Self::DOUBLE_UNDERLINE.bits()
            | Self::UNDERCURL.bits()
            | Self::DOTTED_UNDERLINE.bits()
            | Self::DASHED_UNDERLINE.bits();
    }
}

impl CellFlags {
    /// Flags that describe layout rather than style; ignored when grouping [`StyleRun`]s.
    const LAYOUT: Self = Self::WIDE.union(Self::WIDE_SPACER);

    fn from_term(flags: Flags) -> Self {
        const MAP: [(Flags, CellFlags); 13] = [
            (Flags::BOLD, CellFlags::BOLD),
            (Flags::ITALIC, CellFlags::ITALIC),
            (Flags::UNDERLINE, CellFlags::UNDERLINE),
            (Flags::DOUBLE_UNDERLINE, CellFlags::DOUBLE_UNDERLINE),
            (Flags::UNDERCURL, CellFlags::UNDERCURL),
            (Flags::DOTTED_UNDERLINE, CellFlags::DOTTED_UNDERLINE),
            (Flags::DASHED_UNDERLINE, CellFlags::DASHED_UNDERLINE),
            (Flags::STRIKEOUT, CellFlags::STRIKEOUT),
            (Flags::DIM, CellFlags::DIM),
            (Flags::INVERSE, CellFlags::INVERSE),
            (Flags::HIDDEN, CellFlags::HIDDEN),
            (Flags::WIDE_CHAR, CellFlags::WIDE),
            (Flags::WIDE_CHAR_SPACER, CellFlags::WIDE_SPACER),
        ];
        MAP.iter()
            .filter(|(f, _)| flags.contains(*f))
            .fold(Self::empty(), |acc, (_, c)| acc | *c)
    }
}

/// One fully resolved terminal cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The base character (`' '` for empty cells and wide-char spacers).
    pub ch: char,
    /// Foreground with dim / bold-as-bright / inverse / hidden applied.
    pub fg: Rgb,
    /// Background with inverse applied.
    pub bg: Rgb,
    /// Explicit underline color (SGR 58), if set; otherwise underline with `fg`.
    pub underline_color: Option<Rgb>,
    pub flags: CellFlags,
    /// Zero-width characters (combining marks, variation selectors) drawn on top of `ch`.
    pub zerowidth: Option<Box<str>>,
}

impl Cell {
    fn blank(palette_fg: Rgb, palette_bg: Rgb) -> Self {
        Self {
            ch: ' ',
            fg: palette_fg,
            bg: palette_bg,
            underline_color: None,
            flags: CellFlags::empty(),
            zerowidth: None,
        }
    }

    /// Whether the cell should be skipped when drawing text (wide-char spacer).
    pub fn is_spacer(&self) -> bool {
        self.flags.contains(CellFlags::WIDE_SPACER)
    }

    fn same_style(&self, other: &Self) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.underline_color == other.underline_color
            && self.flags.difference(CellFlags::LAYOUT) == other.flags.difference(CellFlags::LAYOUT)
    }
}

/// One viewport line: exactly `cols` cells.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub cells: Vec<Cell>,
}

impl Line {
    /// The visible text of the line (spacers skipped, zero-width chars included, trailing
    /// whitespace kept). Allocates; meant for tests, link detection and the like.
    pub fn text(&self) -> String {
        let mut text = String::with_capacity(self.cells.len());
        for cell in self.cells.iter().filter(|c| !c.is_spacer()) {
            text.push(cell.ch);
            if let Some(zw) = &cell.zerowidth {
                text.push_str(zw);
            }
        }
        text
    }

    /// Iterate over maximal runs of adjacent cells sharing fg, bg, underline color and style
    /// flags. Does not allocate.
    pub fn runs(&self) -> StyleRuns<'_> {
        StyleRuns {
            cells: &self.cells,
            col: 0,
        }
    }
}

/// Iterator returned by [`Line::runs`].
#[derive(Clone, Debug)]
pub struct StyleRuns<'a> {
    cells: &'a [Cell],
    col: usize,
}

/// A run of identically styled cells starting at column `start_col`.
#[derive(Clone, Copy, Debug)]
pub struct StyleRun<'a> {
    pub start_col: usize,
    pub cells: &'a [Cell],
}

impl StyleRun<'_> {
    /// Style of the run (taken from its first cell).
    pub fn style(&self) -> &Cell {
        &self.cells[0]
    }
}

impl<'a> Iterator for StyleRuns<'a> {
    type Item = StyleRun<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let first = self.cells.first()?;
        let len = self
            .cells
            .iter()
            .take_while(|c| c.same_style(first))
            .count();
        let (run, rest) = self.cells.split_at(len);
        let item = StyleRun {
            start_col: self.col,
            cells: run,
        };
        self.cells = rest;
        self.col += len;
        Some(item)
    }
}

/// Cursor shapes (a hidden cursor is reported as `Snapshot::cursor == None`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CursorShape {
    #[default]
    Block,
    Beam,
    Underline,
    HollowBlock,
}

/// Visible cursor state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorState {
    pub row: usize,
    pub col: usize,
    pub shape: CursorShape,
    /// The cursor sits on a double-width character and should cover two cells.
    pub wide: bool,
    /// Cursor color (palette cursor, or a color the program set via OSC 12).
    pub color: Rgb,
}

/// Everything needed to paint the terminal viewport. See the module docs for the layout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: u16,
    pub rows: u16,
    /// Exactly `rows` lines of the current viewport (respecting `display_offset`).
    pub lines: Vec<Line>,
    /// `None` when the cursor is hidden or scrolled out of view.
    pub cursor: Option<CursorState>,
    /// Number of lines scrolled up into history (0 = following the output).
    pub display_offset: usize,
    /// Number of lines in the scrollback history.
    pub history_size: usize,
    /// Normalized (start <= end, both inclusive) selection clamped to the viewport, if visible.
    /// Intermediate rows are selected across their full width.
    pub selection: Option<(GridPoint, GridPoint)>,
    /// Terminal default background (handy for painting the area around the grid).
    pub background: Rgb,
    /// Modes at the time of the snapshot.
    pub mode: TermModeFlags,
}

impl Snapshot {
    /// The visible text, one line per row (trailing whitespace trimmed). Allocates.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(line.text().trim_end());
        }
        out
    }

    /// Whether the viewport cell is inside the selection.
    pub fn is_selected(&self, point: GridPoint) -> bool {
        self.selection
            .is_some_and(|(start, end)| start <= point && point <= end)
    }
}

/// Fill `out` from the terminal state, reusing its buffers.
pub(crate) fn build_snapshot<T: EventListener>(
    term: &Term<T>,
    palette: &Palette,
    bold_is_bright: bool,
    out: &mut Snapshot,
) {
    let content = term.renderable_content();
    let cols = term.columns();
    let rows = term.screen_lines();
    let offset = content.display_offset;
    let resolver = ColorResolver {
        palette,
        overrides: content.colors,
        bold_is_bright,
    };

    let default_fg = resolver.named(NamedColor::Foreground);
    let default_bg = resolver.named(NamedColor::Background);

    out.cols = u16::try_from(cols).unwrap_or(u16::MAX);
    out.rows = u16::try_from(rows).unwrap_or(u16::MAX);
    out.display_offset = offset;
    out.history_size = term.history_size();
    out.background = default_bg;
    out.mode = TermModeFlags::from(content.mode);

    out.lines.truncate(rows);
    out.lines.resize_with(rows, Line::default);
    for line in &mut out.lines {
        line.cells.clear();
        line.cells.reserve(cols);
    }

    let offset_i32 = i32::try_from(offset).unwrap_or(i32::MAX);
    for indexed in content.display_iter {
        let Ok(row) = usize::try_from(indexed.point.line.0.saturating_add(offset_i32)) else {
            continue;
        };
        if let Some(line) = out.lines.get_mut(row) {
            line.cells.push(convert_cell(indexed.cell, &resolver));
        }
    }
    // Defensive: guarantee the documented shape even if the grid iterator came up short.
    for line in &mut out.lines {
        line.cells
            .resize_with(cols, || Cell::blank(default_fg, default_bg));
    }

    out.cursor = cursor_state(term, &content.cursor, &resolver, offset_i32, rows);
    out.selection = content
        .selection
        .and_then(|range| viewport_selection(range, offset, rows, cols));
}

fn convert_cell(cell: &TermCell, resolver: &ColorResolver<'_>) -> Cell {
    let mut fg = resolver.fg(cell.fg, cell.flags);
    let mut bg = resolver.bg(cell.bg);
    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.flags.contains(Flags::HIDDEN) {
        fg = bg;
    }
    let underline_color = cell.underline_color().map(|c| resolver.fg(c, cell.flags));
    let zerowidth = cell
        .zerowidth()
        .filter(|zw| !zw.is_empty())
        .map(|zw| zw.iter().collect::<String>().into_boxed_str());
    let flags = CellFlags::from_term(cell.flags);
    let ch = if flags.contains(CellFlags::WIDE_SPACER) || cell.c == '\t' {
        ' '
    } else {
        cell.c
    };
    Cell {
        ch,
        fg,
        bg,
        underline_color,
        flags,
        zerowidth,
    }
}

fn cursor_state<T>(
    term: &Term<T>,
    cursor: &alacritty_terminal::term::RenderableCursor,
    resolver: &ColorResolver<'_>,
    offset: i32,
    rows: usize,
) -> Option<CursorState> {
    let shape = match cursor.shape {
        AnsiCursorShape::Hidden => return None,
        AnsiCursorShape::Block => CursorShape::Block,
        AnsiCursorShape::Beam => CursorShape::Beam,
        AnsiCursorShape::Underline => CursorShape::Underline,
        AnsiCursorShape::HollowBlock => CursorShape::HollowBlock,
    };
    let row = usize::try_from(cursor.point.line.0.saturating_add(offset)).ok()?;
    if row >= rows {
        return None;
    }
    let wide = term.grid()[cursor.point].flags.contains(Flags::WIDE_CHAR);
    Some(CursorState {
        row,
        col: cursor.point.column.0,
        shape,
        wide,
        color: resolver.named(NamedColor::Cursor),
    })
}

fn viewport_selection(
    range: SelectionRange,
    offset: usize,
    rows: usize,
    cols: usize,
) -> Option<(GridPoint, GridPoint)> {
    if rows == 0 || cols == 0 {
        return None;
    }
    let top = -i64::try_from(offset).ok()?;
    let bottom = top + i64::try_from(rows).ok()? - 1;
    let (start_line, end_line) = (i64::from(range.start.line.0), i64::from(range.end.line.0));
    if end_line < top || start_line > bottom {
        return None;
    }
    let to_row = |line: i64| usize::try_from(line - top).ok();
    let start = if start_line < top {
        GridPoint::new(0, 0)
    } else {
        GridPoint::new(to_row(start_line)?, range.start.column.0.min(cols - 1))
    };
    let end = if end_line > bottom {
        GridPoint::new(rows - 1, cols - 1)
    } else {
        GridPoint::new(to_row(end_line)?, range.end.column.0.min(cols - 1))
    };
    Some((start, end))
}

/// Convert a viewport point to an absolute grid point, clamped to the viewport.
pub(crate) fn viewport_to_grid<T>(term: &Term<T>, point: GridPoint) -> Point {
    let rows = term.screen_lines().max(1);
    let cols = term.columns().max(1);
    let row = i32::try_from(point.row.min(rows - 1)).unwrap_or(0);
    let offset = i32::try_from(term.grid().display_offset()).unwrap_or(0);
    Point::new(GridLine(row - offset), Column(point.col.min(cols - 1)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Scroll;
    use alacritty_terminal::index::Side;
    use alacritty_terminal::selection::{Selection, SelectionType};
    use alacritty_terminal::term::Config;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::vte::ansi::Processor;
    use pretty_assertions::assert_eq;

    fn term(cols: usize, rows: usize) -> Term<VoidListener> {
        Term::new(Config::default(), &TermSize::new(cols, rows), VoidListener)
    }

    fn feed(term: &mut Term<VoidListener>, bytes: &[u8]) {
        let mut parser: Processor = Processor::new();
        parser.advance(term, bytes);
    }

    fn snap(term: &Term<VoidListener>) -> Snapshot {
        let mut out = Snapshot::default();
        build_snapshot(term, &Palette::default(), false, &mut out);
        out
    }

    #[test]
    fn plain_text_and_shape() {
        let mut t = term(10, 3);
        feed(&mut t, b"hello\r\nworld");
        let s = snap(&t);
        assert_eq!((s.cols, s.rows), (10, 3));
        assert_eq!(s.lines.len(), 3);
        assert!(s.lines.iter().all(|l| l.cells.len() == 10));
        assert_eq!(s.text(), "hello\nworld\n");
        let p = Palette::default();
        assert_eq!(s.lines[0].cells[0].fg, p.foreground);
        assert_eq!(s.lines[0].cells[0].bg, p.background);
        assert_eq!(s.background, p.background);
    }

    #[test]
    fn named_colors_and_attributes() {
        let mut t = term(20, 2);
        feed(
            &mut t,
            b"\x1b[31mR\x1b[0m\x1b[1;42mB\x1b[0m\x1b[3;4;9mI\x1b[0m\x1b[2;34mD",
        );
        let s = snap(&t);
        let p = Palette::default();
        let cells = &s.lines[0].cells;
        assert_eq!(cells[0].fg, p.ansi[1]);
        assert_eq!(cells[1].bg, p.ansi[2]);
        assert!(cells[1].flags.contains(CellFlags::BOLD));
        // Bold is not drawn bright by default.
        assert_eq!(cells[1].fg, p.foreground);
        assert!(
            cells[2]
                .flags
                .contains(CellFlags::ITALIC | CellFlags::UNDERLINE | CellFlags::STRIKEOUT)
        );
        assert!(cells[3].flags.contains(CellFlags::DIM));
        assert_ne!(cells[3].fg, p.ansi[4]);
    }

    #[test]
    fn bold_as_bright() {
        let mut t = term(5, 1);
        feed(&mut t, b"\x1b[1;31mX");
        let mut s = Snapshot::default();
        let p = Palette::default();
        build_snapshot(&t, &p, true, &mut s);
        assert_eq!(s.lines[0].cells[0].fg, p.ansi[9]);
    }

    #[test]
    fn indexed_and_truecolor() {
        let mut t = term(10, 1);
        feed(
            &mut t,
            b"\x1b[38;5;196mA\x1b[48;5;21mB\x1b[0m\x1b[38;2;1;2;3;48;2;250;251;252mC",
        );
        let s = snap(&t);
        let cells = &s.lines[0].cells;
        assert_eq!(cells[0].fg, Rgb::new(255, 0, 0));
        assert_eq!(cells[1].bg, Rgb::new(0, 0, 255));
        assert_eq!(cells[2].fg, Rgb::new(1, 2, 3));
        assert_eq!(cells[2].bg, Rgb::new(250, 251, 252));
    }

    #[test]
    fn inverse_and_hidden_are_applied() {
        let mut t = term(10, 1);
        feed(&mut t, b"\x1b[7;31mI\x1b[0m\x1b[8mH");
        let s = snap(&t);
        let p = Palette::default();
        let cells = &s.lines[0].cells;
        assert_eq!((cells[0].fg, cells[0].bg), (p.background, p.ansi[1]));
        assert!(cells[0].flags.contains(CellFlags::INVERSE));
        assert_eq!(cells[1].fg, cells[1].bg);
        assert!(cells[1].flags.contains(CellFlags::HIDDEN));
    }

    #[test]
    fn wide_chars_have_spacers() {
        let mut t = term(10, 2);
        feed(&mut t, "a日b".as_bytes());
        let s = snap(&t);
        let cells = &s.lines[0].cells;
        assert_eq!(cells[1].ch, '日');
        assert!(cells[1].flags.contains(CellFlags::WIDE));
        assert!(cells[2].is_spacer());
        assert_eq!(cells[3].ch, 'b');
        assert_eq!(s.lines[0].text().trim_end(), "a日b");
        assert_eq!(s.cursor.map(|c| (c.row, c.col)), Some((0, 4)));
    }

    #[test]
    fn zero_width_chars_are_kept() {
        let mut t = term(10, 1);
        feed(&mut t, "e\u{301}x".as_bytes());
        let s = snap(&t);
        assert_eq!(s.lines[0].cells[0].zerowidth.as_deref(), Some("\u{301}"));
        assert_eq!(s.lines[0].cells[1].ch, 'x');
    }

    #[test]
    fn cursor_position_and_shape() {
        let mut t = term(10, 5);
        feed(&mut t, b"\x1b[3;7H");
        let s = snap(&t);
        let cursor = s.cursor.expect("visible cursor");
        assert_eq!(
            (cursor.row, cursor.col, cursor.shape),
            (2, 6, CursorShape::Block)
        );
        assert_eq!(cursor.color, Palette::default().cursor);

        feed(&mut t, b"\x1b[6 q");
        assert_eq!(snap(&t).cursor.map(|c| c.shape), Some(CursorShape::Beam));
        feed(&mut t, b"\x1b[4 q");
        assert_eq!(
            snap(&t).cursor.map(|c| c.shape),
            Some(CursorShape::Underline)
        );

        feed(&mut t, b"\x1b[?25l");
        assert_eq!(snap(&t).cursor, None);
    }

    #[test]
    fn line_wrap() {
        let mut t = term(5, 3);
        feed(&mut t, b"abcdefgh");
        let s = snap(&t);
        assert_eq!(s.lines[0].text(), "abcde");
        assert_eq!(s.lines[1].text(), "fgh  ");
        assert_eq!(s.cursor.map(|c| (c.row, c.col)), Some((1, 3)));
    }

    #[test]
    fn scrollback_and_display_offset() {
        let mut t = term(8, 3);
        for i in 0..10 {
            feed(&mut t, format!("line{i}\r\n").as_bytes());
        }
        let s = snap(&t);
        assert_eq!(s.text(), "line8\nline9\n");
        assert_eq!(s.history_size, 8);
        assert_eq!(s.display_offset, 0);
        assert!(s.cursor.is_some());

        t.scroll_display(Scroll::Delta(2));
        let s = snap(&t);
        assert_eq!(s.display_offset, 2);
        assert_eq!(s.text(), "line6\nline7\nline8");
        // Cursor is on the (now hidden) last line.
        assert_eq!(s.cursor, None);

        t.scroll_display(Scroll::Top);
        assert_eq!(snap(&t).text(), "line0\nline1\nline2");
        t.scroll_display(Scroll::Bottom);
        assert_eq!(snap(&t).display_offset, 0);
    }

    #[test]
    fn selection_is_reported_in_viewport_coordinates() {
        let mut t = term(10, 3);
        feed(&mut t, b"one\r\ntwo\r\nthree");
        let mut sel = Selection::new(
            SelectionType::Simple,
            Point::new(GridLine(0), Column(1)),
            Side::Left,
        );
        sel.update(Point::new(GridLine(1), Column(1)), Side::Right);
        t.selection = Some(sel);
        let s = snap(&t);
        assert_eq!(
            s.selection,
            Some((GridPoint::new(0, 1), GridPoint::new(1, 1)))
        );
        assert!(s.is_selected(GridPoint::new(0, 9)));
        assert!(!s.is_selected(GridPoint::new(1, 2)));
        assert_eq!(t.selection_to_string().as_deref(), Some("ne\ntw"));
    }

    #[test]
    fn selection_clamped_when_partially_scrolled_away() {
        let mut t = term(5, 2);
        feed(&mut t, b"a\r\nb\r\nc\r\nd");
        // Viewport shows "c", "d"; history has "a", "b".
        let sel_start = Point::new(GridLine(-2), Column(0));
        let mut sel = Selection::new(SelectionType::Simple, sel_start, Side::Left);
        sel.update(Point::new(GridLine(0), Column(0)), Side::Right);
        t.selection = Some(sel);
        assert_eq!(
            snap(&t).selection,
            Some((GridPoint::new(0, 0), GridPoint::new(0, 0)))
        );
    }

    #[test]
    fn style_runs_group_cells() {
        let mut t = term(6, 1);
        feed(&mut t, "ab\x1b[31mc日\x1b[0m".as_bytes());
        let s = snap(&t);
        let runs: Vec<_> = s.lines[0]
            .runs()
            .map(|r| (r.start_col, r.cells.len()))
            .collect();
        // "ab" default, "c日<spacer>" red, trailing blank cell default.
        assert_eq!(runs, vec![(0, 2), (2, 3), (5, 1)]);
        let red = s.lines[0].runs().nth(1).expect("red run");
        assert_eq!(red.style().fg, Palette::default().ansi[1]);
    }

    #[test]
    fn snapshot_buffers_are_reused_across_resizes() {
        let mut t = term(10, 4);
        feed(&mut t, b"x");
        let mut s = Snapshot::default();
        build_snapshot(&t, &Palette::default(), false, &mut s);
        t.resize(TermSize::new(6, 2));
        build_snapshot(&t, &Palette::default(), false, &mut s);
        assert_eq!((s.cols, s.rows, s.lines.len()), (6, 2, 2));
        assert!(s.lines.iter().all(|l| l.cells.len() == 6));
    }

    #[test]
    fn viewport_to_grid_respects_offset() {
        let mut t = term(5, 2);
        feed(&mut t, b"a\r\nb\r\nc\r\nd");
        t.scroll_display(Scroll::Delta(1));
        assert_eq!(
            viewport_to_grid(&t, GridPoint::new(0, 2)),
            Point::new(GridLine(-1), Column(2))
        );
        assert_eq!(
            viewport_to_grid(&t, GridPoint::new(9, 9)),
            Point::new(GridLine(0), Column(4))
        );
    }

    #[test]
    fn cell_stays_small() {
        assert!(size_of::<Cell>() <= 32);
    }
}
