//! The document view: a from-scratch text widget.
//!
//! No `TextEdit`, no layout engine, no editor crate. This module owns
//! measurement, hit-testing, selection painting, the gutter, scrollbars, and
//! input handling.
//!
//! # Rendering model
//!
//! Only the visible window is ever touched:
//!
//! 1. `first_line` comes from `scroll_y / line_h`; the paint loop runs for
//!    `ceil(view_h / line_h) + 1` lines and stops there.
//! 2. Each line's byte span comes from the treap's line index in `O(log n)`.
//! 3. A line's bytes are read in one contiguous window (at most
//!    [`WINDOW_BYTES`]) positioned by the horizontal scroll offset, so a
//!    200 000-byte line costs the same per frame as a 20-byte one. Long lines get
//!    a cached checkpoint index (every [`INDEX_STEP`] bytes, always at character
//!    boundaries) so locating the byte under a pixel never rescans the line.
//!
//! A frame therefore costs `O(visible_lines)` tree descents plus a bounded
//! amount of byte walking — never a document scan, and never a whole-line scan.
//!
//! # Hit-testing and positioning
//!
//! Every glyph advances by exactly `char_w` (see [`Metrics`]), so the x position
//! of any byte is a sum of per-byte advances. Byte -> x and x -> byte are inverse
//! scans over the same data, which is why the caret, the selection and the mouse
//! always agree.

use crate::buffer::Buffer;
use crate::document::Document;
use crate::motion;
use crate::undo::{EditKind, Sel};
use crate::ui::{Metrics, Palette};
use egui::{Align2, Color32, Id, Pos2, Rect, Sense, Stroke};

/// Bytes read per line window. Must comfortably exceed [`INDEX_STEP`] so that one
/// window always covers a full viewport width of characters.
const WINDOW_BYTES: usize = 8192;
/// Checkpoint spacing for the long-line index.
const INDEX_STEP: usize = 4096;
/// Lines shorter than this are measured inline instead of being cached.
const CACHE_THRESHOLD: usize = 256;
/// Minimum scrollbar thumb length, so the thumb stays grabbable.
const MIN_SCROLLBAR_THUMB: f32 = 24.0;

// ---------------------------------------------------------------------------
// Per-line checkpoint index
// ---------------------------------------------------------------------------

/// Pixel-x checkpoints along one line, so random access costs `O(INDEX_STEP)`
/// instead of `O(line length)`.
struct LineIndex {
    /// `(byte offset within the line, pixel x)`, always at character boundaries.
    points: Vec<(u32, f32)>,
    /// Total pixel width of the line.
    width: f32,
}

impl LineIndex {
    fn build(buf: &Buffer, start: usize, end: usize, m: &Metrics) -> LineIndex {
        let total = end - start;
        let mut points = Vec::with_capacity(total / INDEX_STEP + 2);
        points.push((0u32, 0f32));
        let mut x = 0f32;
        let mut off = 0usize;
        let mut next_check = INDEX_STEP;
        let mut chunk = vec![0u8; 16 * 1024];
        while off < total {
            let want = (total - off).min(chunk.len());
            let got = buf.read_prefix(start + off, &mut chunk[..want]);
            if got == 0 {
                break;
            }
            for &b in &chunk[..got] {
                // Checkpoints must land on character boundaries, otherwise the
                // byte offset they store is not a valid caret position.
                if (b & 0xC0) != 0x80 && off >= next_check {
                    points.push((off as u32, x));
                    next_check = off + INDEX_STEP;
                }
                x += advance(b, x, m);
                off += 1;
            }
        }
        LineIndex { points, width: x }
    }

    /// Nearest checkpoint at or before `x_px`.
    fn checkpoint(&self, x_px: f32) -> (usize, f32) {
        let i = self.points.partition_point(|p| p.1 <= x_px);
        let i = i.saturating_sub(1);
        (self.points[i].0 as usize, self.points[i].1)
    }

    /// Nearest checkpoint at or before byte offset `off`.
    fn checkpoint_byte(&self, off: usize) -> (usize, f32) {
        let i = self.points.partition_point(|p| (p.0 as usize) <= off);
        let i = i.saturating_sub(1);
        (self.points[i].0 as usize, self.points[i].1)
    }
}

/// Pixel advance of byte `b` at current x.
///
/// A character advances exactly one cell, regardless of how wide its glyph
/// renders. Continuation bytes advance nothing.
#[inline]
fn advance(b: u8, x: f32, m: &Metrics) -> f32 {
    if b == b'\t' {
        ((x / m.tab_w).floor() + 1.0) * m.tab_w - x
    } else if (b & 0xC0) != 0x80 {
        m.char_w
    } else {
        0.0
    }
}

/// Drop a trailing incomplete UTF-8 sequence so the window can be read as `&str`.
fn trim_utf8(b: &mut Vec<u8>) {
    let n = b.len();
    if n == 0 {
        return;
    }
    let mut i = n;
    while i > 0 && (b[i - 1] & 0xC0) == 0x80 {
        i -= 1;
    }
    if i == 0 {
        return;
    }
    let lead = b[i - 1];
    let need = if lead < 0x80 {
        1
    } else if lead >= 0xF0 {
        4
    } else if lead >= 0xE0 {
        3
    } else {
        2
    };
    if n - (i - 1) < need {
        b.truncate(i - 1);
    }
}

/// Read `[from_off, end_off)` of a line into `scratch`.
fn read_window(
    buf: &Buffer,
    line_start: usize,
    from_off: usize,
    end_off: usize,
    scratch: &mut Vec<u8>,
) {
    scratch.clear();
    if end_off <= from_off {
        return;
    }
    let want = (end_off - from_off).min(WINDOW_BYTES);
    scratch.resize(want, 0);
    let got = buf.read_prefix(line_start + from_off, scratch);
    scratch.truncate(got);
    trim_utf8(scratch);
}

// ---------------------------------------------------------------------------
// View state
// ---------------------------------------------------------------------------

/// Caches that persist across frames for one document's view.
#[derive(Default)]
pub struct ViewState {
    /// Long-line index cache, keyed by line number and dropped on every edit.
    cache: std::collections::HashMap<usize, LineIndex>,
    cache_generation: u64,
    /// Blink phase anchor, in egui's time base.
    blink_origin: f64,
    /// Set while a double-click drag is extending by whole words.
    drag_word: bool,
    /// Widest line seen so far, in pixels, which sizes the horizontal extent.
    ///
    /// Measuring every line in the document would be a full scan, so the extent
    /// grows as wide lines are painted. That is enough in practice: to scroll a
    /// long line horizontally you must first have it on screen.
    pub longest_line_px: f32,
}

impl ViewState {
    /// Index for a line, or `None` when the line is short enough to measure inline.
    fn index_for(&mut self, buf: &Buffer, line: usize, start: usize, end: usize, m: &Metrics) -> Option<&LineIndex> {
        let generation = buf.generation();
        if self.cache_generation != generation {
            self.cache.clear();
            self.cache_generation = generation;
        }
        if end - start <= CACHE_THRESHOLD {
            return None;
        }
        Some(
            self.cache
                .entry(line)
                .or_insert_with(|| LineIndex::build(buf, start, end, m)),
        )
    }

}

/// Measure a short line directly.
fn measure_inline(buf: &Buffer, start: usize, end: usize, m: &Metrics) -> f32 {
    let mut chunk = [0u8; 512];
    let want = (end - start).min(chunk.len());
    let got = buf.read_prefix(start, &mut chunk[..want]);
    let mut x = 0f32;
    for &b in &chunk[..got] {
        x += advance(b, x, m);
    }
    x
}

/// Pixel x of byte offset `off` within a line, relative to the line start.
fn x_of_byte(
    buf: &Buffer,
    idx: Option<&LineIndex>,
    line_start: usize,
    line_len: usize,
    off: usize,
    m: &Metrics,
    scratch: &mut Vec<u8>,
) -> f32 {
    let off = off.min(line_len);
    let (from, mut x) = match idx {
        Some(i) => i.checkpoint_byte(off),
        None => (0usize, 0f32),
    };
    if from >= off {
        return x;
    }
    read_window(buf, line_start, from, off, scratch);
    let mut k = 0usize;
    while k < scratch.len() {
        let b = scratch[k];
        x += advance(b, x, m);
        k += 1;
        if b >= 0xC0 {
            while k < scratch.len() && (scratch[k] & 0xC0) == 0x80 {
                k += 1;
            }
        }
    }
    x
}

/// Byte offset whose cell contains pixel `x_px` (relative to the line start).
fn byte_at_x(
    buf: &Buffer,
    idx: Option<&LineIndex>,
    line_start: usize,
    line_len: usize,
    x_px: f32,
    m: &Metrics,
    scratch: &mut Vec<u8>,
) -> usize {
    let (from, start_x) = match idx {
        Some(i) => i.checkpoint(x_px),
        None => (0usize, 0f32),
    };
    read_window(buf, line_start, from, line_len, scratch);
    let mut x = start_x;
    let mut off = from;
    let mut k = 0usize;
    while k < scratch.len() {
        let b = scratch[k];
        let a = advance(b, x, m);
        if x + a > x_px {
            return off;
        }
        x += a;
        off += 1;
        k += 1;
        if b >= 0xC0 {
            while k < scratch.len() && (scratch[k] & 0xC0) == 0x80 {
                k += 1;
                off += 1;
            }
        }
    }
    line_len
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// Everything derived from the widget rect and the scroll offsets.
pub struct Layout {
    pub full: Rect,
    pub gutter: Rect,
    pub text: Rect,
    pub vbar: Rect,
    pub hbar: Rect,
    pub first_line: usize,
    pub visible_lines: usize,
    pub total_lines: usize,
    pub content_width: f32,
    pub content_height: f32,
}

/// Layout plus the metrics and palette needed to paint it.
struct Ctx<'a> {
    m: &'a Metrics,
    palette: &'a Palette,
    lay: &'a Layout,
}

fn layout(full: Rect, doc: &Document, view: &ViewState, m: &Metrics) -> Layout {
    let total_lines = doc.line_count();
    let digits = total_lines.max(1).to_string().len().max(2);
    let gutter_width = (digits as f32 * m.char_w + 16.0).round();
    let bar = 12.0;

    let text = Rect::from_min_max(
        Pos2::new(full.left() + gutter_width, full.top()),
        Pos2::new(
            (full.right() - bar).max(full.left() + gutter_width),
            (full.bottom() - bar).max(full.top()),
        ),
    );
    let gutter = Rect::from_min_max(full.min, Pos2::new(text.left(), text.bottom()));
    let vbar = Rect::from_min_max(
        Pos2::new(text.right(), full.top()),
        Pos2::new(full.right(), text.bottom()),
    );
    let hbar = Rect::from_min_max(
        Pos2::new(full.left(), text.bottom()),
        Pos2::new(full.right(), full.bottom()),
    );

    let line_h = m.line_h;
    let visible_lines = ((text.height() / line_h).floor() as usize).max(1) + 1;
    let first_line = (doc.text.scroll_y / line_h).floor().max(0.0) as usize;
    let first_line = first_line.min(total_lines.saturating_sub(1));

    Layout {
        full,
        gutter,
        text,
        vbar,
        hbar,
        first_line,
        visible_lines,
        total_lines,
        content_height: total_lines as f32 * line_h,
        content_width: view.longest_line_px.max(text.width()),
    }
}

/// Clamp a scroll offset to the valid range.
fn clamp_scroll(v: f32, view_extent: f32, content_extent: f32) -> f32 {
    v.max(0.0).min((content_extent - view_extent).max(0.0))
}

// ---------------------------------------------------------------------------
// Widget entry point
// ---------------------------------------------------------------------------

/// What the view wants the app to do after this frame.
#[derive(Default)]
pub struct ViewResponse {
    /// Text to place on the clipboard.
    pub copied: Option<String>,
    /// An edit was refused because a background operation owns the document.
    pub edit_blocked: bool,
    /// The user asked to paste, but the view has no clipboard access, so the app
    /// must perform it.
    pub paste_requested: bool,
}

/// Focus id of a document's editor surface.
pub fn focus_id(doc: crate::document::DocId) -> Id {
    Id::new(("editor-view", doc))
}

/// Draw the document and handle all of its input.
pub fn show(
    ui: &mut egui::Ui,
    doc: &mut Document,
    view: &mut ViewState,
    m: &Metrics,
    palette: &Palette,
    search_focused: bool,
) -> ViewResponse {
    let mut out = ViewResponse::default();
    let mut scratch: Vec<u8> = Vec::with_capacity(WINDOW_BYTES);

    let full = ui.available_rect_before_wrap();
    let (rect, _) = ui.allocate_exact_size(full.size(), Sense::hover());
    let id = focus_id(doc.id);
    let response = ui.interact(rect, id, Sense::click_and_drag());

    // Claim the keys that egui's focus system would otherwise consume.
    //
    // By default egui treats Tab, the arrow keys and Escape as focus-navigation
    // input: it moves focus to another widget and the editor never sees the
    // keystroke, so Tab would never insert and the arrow keys would not move the
    // caret. Declaring the filter takes those keys back for as long as this widget
    // holds focus. egui only applies it from the second focused frame onward, which
    // is why focus is requested by click rather than by Tab.
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            );
        });
    }

    if response.hovered() {
        let (dx, dy) = ui.input(|i| (i.smooth_scroll_delta.x, i.smooth_scroll_delta.y));
        if dy != 0.0 || dx != 0.0 {
            doc.text.scroll_y -= dy;
            doc.text.scroll_x -= dx;
        }
    }

    let lay = layout(rect, doc, view, m);
    let cx = Ctx {
        m,
        palette,
        lay: &lay,
    };

    handle_mouse(ui, doc, view, &cx, &response, &mut scratch);
    if !search_focused {
        handle_keyboard(ui, doc, view, &cx, &response, &mut out, &mut scratch);
    }

    if doc.text.scroll_to_caret {
        reveal_caret(doc, view, &cx, &mut scratch);
        doc.text.scroll_to_caret = false;
    }

    // Scroll may have changed; clamp, then re-derive so painting is consistent.
    doc.text.scroll_x = clamp_scroll(doc.text.scroll_x, lay.text.width(), lay.content_width);
    doc.text.scroll_y = clamp_scroll(doc.text.scroll_y, lay.text.height(), lay.content_height);
    let lay = layout(rect, doc, view, m);
    let cx = Ctx {
        m,
        palette,
        lay: &lay,
    };

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, palette.bg);

    let caret_line = doc.text.buffer.line_of_byte(doc.text.sel.caret);
    paint_lines(ui, doc, view, &cx, caret_line, &mut scratch);
    paint_gutter(ui, doc, &cx, caret_line);
    paint_caret(ui, doc, view, &cx, caret_line, &response, &mut scratch);
    paint_scrollbars(ui, doc, &cx, &response);

    if response.has_focus() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(530));
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }

    out
}

/// Scroll so the caret is inside the viewport.
fn reveal_caret(doc: &mut Document, view: &mut ViewState, cx: &Ctx, scratch: &mut Vec<u8>) {
    let caret = doc.text.sel.caret.min(doc.text.buffer.len());
    let line = doc.text.buffer.line_of_byte(caret);
    let line_h = cx.m.line_h;

    let top = line as f32 * line_h;
    let bottom = top + line_h;
    if top < doc.text.scroll_y {
        doc.text.scroll_y = top;
    } else if bottom > doc.text.scroll_y + cx.lay.text.height() {
        doc.text.scroll_y = bottom - cx.lay.text.height();
    }

    let (ls, le) = doc.text.buffer.line_span(line);
    let llen = le - ls;
    let x = {
        let idx = view.index_for(&doc.text.buffer, line, ls, le, cx.m);
        x_of_byte(&doc.text.buffer, idx, ls, llen, caret - ls, cx.m, scratch)
    };
    let margin = cx.m.char_w * 2.0;
    if x - margin < doc.text.scroll_x {
        doc.text.scroll_x = (x - margin).max(0.0);
    } else if x + margin > doc.text.scroll_x + cx.lay.text.width() {
        doc.text.scroll_x = x + margin - cx.lay.text.width();
    }
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

fn paint_lines(
    ui: &egui::Ui,
    doc: &Document,
    view: &mut ViewState,
    cx: &Ctx,
    caret_line: usize,
    scratch: &mut Vec<u8>,
) {
    let painter = ui.painter_at(cx.lay.full);
    let line_h = cx.m.line_h;
    let buf = &doc.text.buffer;
    let (sel_a, sel_b) = doc.text.sel.range();
    let matches = &doc.search.matches;
    let current_match = doc.search.current;
    let scroll_x = doc.text.scroll_x;
    let scroll_y = doc.text.scroll_y;
    let mut widest = view.longest_line_px;

    let last_line = (cx.lay.first_line + cx.lay.visible_lines).min(cx.lay.total_lines);
    for line in cx.lay.first_line..last_line {
        let (ls, le) = buf.line_span(line);
        let llen = le - ls;

        let y = cx.lay.text.top() - scroll_y + line as f32 * line_h;
        let row = Rect::from_min_max(
            Pos2::new(cx.lay.full.left(), y),
            Pos2::new(cx.lay.full.right(), y + line_h),
        );
        if line == caret_line {
            painter.rect_filled(row, 0.0, cx.palette.current_line);
        }

        let idx = view.index_for(buf, line, ls, le, cx.m);
        // Sizes the horizontal extent. Long lines are already indexed (and cached)
        // so this is a lookup; short lines cost one bounded scan.
        widest = widest.max(match idx {
            Some(i) => i.width,
            None => measure_inline(buf, ls, le, cx.m),
        });

        // --- match highlights ---
        if !matches.is_empty() {
            let mut i = matches.partition_point(|mm| mm.end <= ls);
            while i < matches.len() && matches[i].start < le {
                let mm = matches[i];
                let x1 = x_of_byte(buf, idx, ls, llen, mm.start.max(ls) - ls, cx.m, scratch);
                let x2 = x_of_byte(buf, idx, ls, llen, mm.end.min(le) - ls, cx.m, scratch);
                let bg = if Some(i) == current_match {
                    cx.palette.match_current_bg
                } else {
                    cx.palette.match_bg
                };
                let left = cx.lay.text.left() + x1 - scroll_x;
                let r = Rect::from_min_max(
                    Pos2::new(left, y),
                    Pos2::new((cx.lay.text.left() + x2 - scroll_x).max(left + 1.0), y + line_h),
                )
                .intersect(cx.lay.text);
                if r.width() > 0.0 {
                    painter.rect_filled(r, 0.0, bg);
                }
                i += 1;
            }
        }

        // --- selection ---
        if sel_a != sel_b {
            let a = sel_a.max(ls);
            let b = sel_b.min(le);
            let spans_newline = sel_a <= ls && sel_b > le;
            if a < b || spans_newline {
                let x1 = x_of_byte(buf, idx, ls, llen, a - ls, cx.m, scratch);
                let mut x2 = x_of_byte(buf, idx, ls, llen, b - ls, cx.m, scratch);
                if sel_b > le {
                    // The line terminator is selected; show it as one cell.
                    x2 += cx.m.char_w;
                }
                let left = cx.lay.text.left() + x1 - scroll_x;
                let right = cx.lay.text.left() + x2 - scroll_x;
                let r = Rect::from_min_max(
                    Pos2::new(left, y),
                    Pos2::new(right.max(left + 1.0), y + line_h),
                )
                .intersect(cx.lay.text);
                if r.width() > 0.0 {
                    painter.rect_filled(r, 0.0, cx.palette.selection);
                }
            }
        }

        if llen == 0 {
            continue;
        }

        // --- text ---
        let (start_off, start_x) = match idx {
            Some(i) => i.checkpoint(scroll_x),
            None => (0usize, 0f32),
        };
        read_window(buf, ls, start_off, llen, scratch);

        let mut x = cx.lay.text.left() + start_x - scroll_x;
        let clip_right = cx.lay.text.right() + cx.m.char_w;
        let mut k = 0usize;
        while k < scratch.len() && x < clip_right {
            let run_start = k;
            let run_x = x;
            while k < scratch.len() && scratch[k] != b'\t' {
                let b = scratch[k];
                x += advance(b, x, cx.m);
                k += 1;
                if b >= 0xC0 {
                    while k < scratch.len() && (scratch[k] & 0xC0) == 0x80 {
                        k += 1;
                    }
                }
            }
            if k > run_start && run_x + cx.m.char_w > cx.lay.text.left() {
                if let Ok(s) = std::str::from_utf8(&scratch[run_start..k]) {
                    painter.text(
                        Pos2::new(run_x, y),
                        Align2::LEFT_TOP,
                        s,
                        cx.m.font.clone(),
                        cx.palette.text,
                    );
                }
            }
            if k < scratch.len() && scratch[k] == b'\t' {
                // A tab paints as blank space; the caret still occupies its cell.
                x += advance(b'\t', x, cx.m);
                k += 1;
            }
        }
    }
    view.longest_line_px = widest;
}

/// Paint line numbers, right-aligned and aligned with the text rows.
fn paint_gutter(ui: &egui::Ui, doc: &Document, cx: &Ctx, caret_line: usize) {
    let painter = ui.painter_at(cx.lay.full);
    painter.rect_filled(cx.lay.gutter, 0.0, cx.palette.gutter_bg);

    let line_h = cx.m.line_h;
    let scroll_y = doc.text.scroll_y;
    let last_line = (cx.lay.first_line + cx.lay.visible_lines).min(cx.lay.total_lines);
    for line in cx.lay.first_line..last_line {
        let y = cx.lay.text.top() - scroll_y + line as f32 * line_h;
        let color = if line == caret_line {
            cx.palette.gutter_active
        } else {
            cx.palette.gutter_text
        };
        painter.text(
            Pos2::new(cx.lay.gutter.right() - 8.0, y),
            Align2::RIGHT_TOP,
            (line + 1).to_string(),
            cx.m.font.clone(),
            color,
        );
    }
    painter.vline(
        cx.lay.gutter.right() - 0.5,
        cx.lay.gutter.y_range(),
        Stroke::new(1.0, Color32::from_black_alpha(80)),
    );
}

fn paint_caret(
    ui: &egui::Ui,
    doc: &Document,
    view: &mut ViewState,
    cx: &Ctx,
    caret_line: usize,
    response: &egui::Response,
    scratch: &mut Vec<u8>,
) {
    let caret = doc.text.sel.caret.min(doc.text.buffer.len());
    let line = caret_line;
    let y = cx.lay.text.top() - doc.text.scroll_y + line as f32 * cx.m.line_h;
    if y + cx.m.line_h < cx.lay.text.top() || y > cx.lay.text.bottom() {
        return;
    }
    let (ls, le) = doc.text.buffer.line_span(line);
    let llen = le - ls;
    let cx_off = caret - ls;
    let x = {
        let idx = view.index_for(&doc.text.buffer, line, ls, le, cx.m);
        x_of_byte(&doc.text.buffer, idx, ls, llen, cx_off, cx.m, scratch)
    };
    let x = cx.lay.text.left() + x - doc.text.scroll_x;
    if x < cx.lay.text.left() - 1.0 || x > cx.lay.text.right() + 1.0 {
        return;
    }

    let now = ui.input(|i| i.time);
    let since = now - view.blink_origin;
    let visible = !response.has_focus() || since < 0.53 || (since % 1.06) < 0.53;
    if visible {
        let painter = ui.painter_at(cx.lay.text);
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x, y + 1.0),
                Pos2::new(x + 1.5, y + cx.m.line_h - 1.0),
            ),
            0.0,
            cx.palette.caret,
        );
    }
}

/// Restart the blink phase so the caret is solid right after interaction.
fn kick_blink(view: &mut ViewState, ui: &egui::Ui) {
    view.blink_origin = ui.input(|i| i.time);
}

#[allow(clippy::too_many_arguments)]
fn paint_scrollbars(ui: &egui::Ui, doc: &mut Document, cx: &Ctx, response: &egui::Response) {
    let painter = ui.painter_at(cx.lay.full);

    let view_h = cx.lay.text.height();
    if cx.lay.content_height > view_h + 0.5 {
        let track = Rect::from_min_max(
            Pos2::new(cx.lay.vbar.left() + 2.0, cx.lay.text.top()),
            Pos2::new(cx.lay.vbar.right() - 2.0, cx.lay.text.bottom()),
        );
        let thumb = vertical_thumb(track, view_h, cx.lay.content_height, doc.text.scroll_y);
        let id = Id::new(("vscroll", doc.id));
        let r = ui.interact(track, id, Sense::click_and_drag());
        let hovered = r.hovered() || r.dragged();
        if r.dragged() {
            let dy = ui.input(|i| i.pointer.delta().y);
            let scale = (cx.lay.content_height - view_h) / (track.height() - thumb.height()).max(1.0);
            doc.text.scroll_y += dy * scale;
        } else if r.clicked() {
            if let Some(p) = r.interact_pointer_pos() {
                let frac = ((p.y - track.top()) / track.height().max(1.0)).clamp(0.0, 1.0);
                doc.text.scroll_y = frac * (cx.lay.content_height - view_h);
            }
        }
        painter.rect_filled(track, 3.0, Color32::from_black_alpha(40));
        painter.rect_filled(
            thumb,
            3.0,
            if hovered {
                cx.palette.scrollbar_hover
            } else {
                cx.palette.scrollbar
            },
        );
    }

    let view_w = cx.lay.text.width();
    if cx.lay.content_width > view_w + 0.5 {
        let track = Rect::from_min_max(
            Pos2::new(cx.lay.text.left(), cx.lay.hbar.top() + 2.0),
            Pos2::new(cx.lay.text.right(), cx.lay.hbar.bottom() - 2.0),
        );
        let thumb = horizontal_thumb(track, view_w, cx.lay.content_width, doc.text.scroll_x);
        let id = Id::new(("hscroll", doc.id));
        let r = ui.interact(track, id, Sense::click_and_drag());
        let hovered = r.hovered() || r.dragged();
        if r.dragged() {
            let dx = ui.input(|i| i.pointer.delta().x);
            let scale = (cx.lay.content_width - view_w) / (track.width() - thumb.width()).max(1.0);
            doc.text.scroll_x += dx * scale;
        } else if r.clicked() {
            if let Some(p) = r.interact_pointer_pos() {
                let frac = ((p.x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0);
                doc.text.scroll_x = frac * (cx.lay.content_width - view_w);
            }
        }
        painter.rect_filled(track, 3.0, Color32::from_black_alpha(40));
        painter.rect_filled(
            thumb,
            3.0,
            if hovered {
                cx.palette.scrollbar_hover
            } else {
                cx.palette.scrollbar
            },
        );
    }

    let _ = response;
}

fn vertical_thumb(track: Rect, view_h: f32, content_h: f32, scroll: f32) -> Rect {
    let frac = (view_h / content_h).clamp(0.0, 1.0);
    let thumb_h = (track.height() * frac).max(MIN_SCROLLBAR_THUMB).min(track.height());
    let max_scroll = (content_h - view_h).max(0.0);
    let t = if max_scroll > 0.0 {
        (scroll / max_scroll).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let top = track.top() + t * (track.height() - thumb_h);
    Rect::from_min_max(
        Pos2::new(track.left(), top),
        Pos2::new(track.right(), top + thumb_h),
    )
}

fn horizontal_thumb(track: Rect, view_w: f32, content_w: f32, scroll: f32) -> Rect {
    let frac = (view_w / content_w).clamp(0.0, 1.0);
    let thumb_w = (track.width() * frac).max(MIN_SCROLLBAR_THUMB).min(track.width());
    let max_scroll = (content_w - view_w).max(0.0);
    let t = if max_scroll > 0.0 {
        (scroll / max_scroll).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let left = track.left() + t * (track.width() - thumb_w);
    Rect::from_min_max(
        Pos2::new(left, track.top()),
        Pos2::new(left + thumb_w, track.bottom()),
    )
}

// ---------------------------------------------------------------------------
// Mouse
// ---------------------------------------------------------------------------

/// Convert a screen position into a byte offset.
fn pos_to_byte(
    doc: &mut Document,
    view: &mut ViewState,
    cx: &Ctx,
    pos: Pos2,
    scratch: &mut Vec<u8>,
) -> usize {
    let rel = (pos.y - cx.lay.text.top() + doc.text.scroll_y) / cx.m.line_h;
    let line = (rel.floor().max(0.0) as usize).min(cx.lay.total_lines.saturating_sub(1));
    let (ls, le) = doc.text.buffer.line_span(line);
    let llen = le - ls;
    let x_px = (pos.x - cx.lay.text.left() + doc.text.scroll_x).max(0.0);
    let idx = view.index_for(&doc.text.buffer, line, ls, le, cx.m);
    let off = byte_at_x(&doc.text.buffer, idx, ls, llen, x_px, cx.m, scratch);
    ls + off
}

fn handle_mouse(
    ui: &egui::Ui,
    doc: &mut Document,
    view: &mut ViewState,
    cx: &Ctx,
    response: &egui::Response,
    scratch: &mut Vec<u8>,
) {
    // Generous bounds so a drag that leaves the widget still selects.
    let in_text = |p: Pos2| {
        p.y >= cx.lay.text.top() - 40.0
            && p.y <= cx.lay.text.bottom() + 40.0
            && p.x >= cx.lay.full.left()
    };

    if response.double_clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            if in_text(p) {
                let at = pos_to_byte(doc, view, cx, p, scratch);
                let (a, b) = motion::word_at(&doc.text.buffer, at);
                doc.text.sel = Sel { anchor: a, caret: b };
                doc.text.target_col = None;
                view.drag_word = true;
                response.request_focus();
                kick_blink(view, ui);
            }
        }
    } else if response.clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            if in_text(p) {
                let at = pos_to_byte(doc, view, cx, p, scratch);
                let shift = ui.input(|i| i.modifiers.shift);
                if shift {
                    doc.extend_to(at);
                } else {
                    doc.set_caret(at);
                }
                view.drag_word = false;
                response.request_focus();
                kick_blink(view, ui);
            }
        }
    }

    if response.dragged() {
        if let Some(p) = response.interact_pointer_pos() {
            let at = pos_to_byte(doc, view, cx, p, scratch);
            if view.drag_word {
                // Extend by whole words, anchored at the double-clicked word.
                let anchor = doc.text.sel.anchor;
                let (a, b) = motion::word_at(&doc.text.buffer, at);
                doc.text.sel = if at >= anchor {
                    Sel {
                        anchor,
                        caret: b.max(anchor),
                    }
                } else {
                    Sel {
                        anchor,
                        caret: a.min(anchor),
                    }
                };
            } else {
                doc.extend_to(at);
            }
            doc.text.scroll_to_caret = false;
            kick_blink(view, ui);
        }

        // Dragging past the viewport edge scrolls and keeps extending.
        if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
            let mut scrolled = false;
            if p.y < cx.lay.text.top() {
                doc.text.scroll_y -= cx.m.line_h;
                scrolled = true;
            } else if p.y > cx.lay.text.bottom() {
                doc.text.scroll_y += cx.m.line_h;
                scrolled = true;
            }
            if p.x < cx.lay.text.left() {
                doc.text.scroll_x -= cx.m.char_w * 4.0;
                scrolled = true;
            } else if p.x > cx.lay.text.right() {
                doc.text.scroll_x += cx.m.char_w * 4.0;
                scrolled = true;
            }
            if scrolled {
                doc.text.scroll_x = clamp_scroll(doc.text.scroll_x, cx.lay.text.width(), cx.lay.content_width);
                doc.text.scroll_y = clamp_scroll(doc.text.scroll_y, cx.lay.text.height(), cx.lay.content_height);
                let clamped = Pos2::new(
                    p.x.clamp(cx.lay.text.left(), cx.lay.text.right() - 1.0),
                    p.y.clamp(cx.lay.text.top(), cx.lay.text.bottom() - 1.0),
                );
                let at = pos_to_byte(doc, view, cx, clamped, scratch);
                if view.drag_word {
                    let anchor = doc.text.sel.anchor;
                    let (a, b) = motion::word_at(&doc.text.buffer, at);
                    doc.text.sel = if at >= anchor {
                        Sel { anchor, caret: b.max(anchor) }
                    } else {
                        Sel { anchor, caret: a.min(anchor) }
                    };
                } else {
                    doc.text.sel.caret = at;
                }
                doc.text.scroll_to_caret = false;
                ui.ctx().request_repaint();
            }
        }
    } else {
        view.drag_word = false;
    }
}

// ---------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------

/// A decoded editing or navigation intent.
enum Intent {
    Caret(usize),
    /// Vertical motion: like `Caret`, but preserves the remembered column.
    Vertical(usize),
    Extend(usize),
    Insert(String, EditKind),
    Newline,
    Tab,
    Backspace,
    Delete,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    SelectAll,
    BreakGroup,
}

fn handle_keyboard(
    ui: &egui::Ui,
    doc: &mut Document,
    view: &mut ViewState,
    cx: &Ctx,
    response: &egui::Response,
    out: &mut ViewResponse,
    scratch: &mut Vec<u8>,
) {
    if !response.has_focus() {
        return;
    }
    let events = ui.input(|i| i.events.clone());
    if events.is_empty() {
        return;
    }

    let buf_len = doc.text.buffer.len();
    let mut caret_now = doc.text.sel.caret.min(buf_len);
    let page = cx.lay.visible_lines.saturating_sub(1).max(1);
    let mut intents: Vec<Intent> = Vec::new();
    // Page movement also shifts the viewport, which is applied immediately.
    let mut page_scroll = 0f32;

    for ev in &events {
        match ev {
            egui::Event::Text(t) => {
                let ctrl = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
                // Some platforms deliver Enter as a text event carrying a bare CR.
                // Enter is handled as a line break, so a lone CR must not become
                // document text.
                if !ctrl && t != "\r" {
                    intents.push(Intent::Insert(t.clone(), EditKind::Typing));
                }
            }
            // `Event::Paste` is handled by the app, which owns the "one undo
            // step per paste" policy and can also serve the Edit menu.
            egui::Event::Copy => intents.push(Intent::Copy),
            egui::Event::Cut => intents.push(Intent::Cut),
            // `egui-winit` turns Ctrl+C/X/V into `Copy`/`Cut`/`Paste` events and
            // returns early, so it never emits the key event as well. Accepting the
            // chords here too costs nothing and keeps the editor working if a
            // backend does not do that translation.
            egui::Event::Key {
                key: egui::Key::C,
                pressed: true,
                modifiers,
                ..
            } if modifiers.ctrl || modifiers.command => intents.push(Intent::Copy),
            egui::Event::Key {
                key: egui::Key::X,
                pressed: true,
                modifiers,
                ..
            } if modifiers.ctrl || modifiers.command => intents.push(Intent::Cut),
            egui::Event::Key {
                key: egui::Key::V,
                pressed: true,
                modifiers,
                ..
            } if modifiers.ctrl || modifiers.command => intents.push(Intent::Paste),
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                use egui::Key;
                let shift = modifiers.shift;
                let ctrl = modifiers.ctrl || modifiers.command;
                let push_move = |intents: &mut Vec<Intent>, to: usize| {
                    if shift {
                        intents.push(Intent::Extend(to));
                    } else {
                        intents.push(Intent::Caret(to));
                    }
                };
                // Vertical moves keep the desired column, so they need their own
                // variant rather than `Caret`.
                let push_vertical = |intents: &mut Vec<Intent>, to: usize| {
                    if shift {
                        intents.push(Intent::Extend(to));
                    } else {
                        intents.push(Intent::Vertical(to));
                    }
                };

                match key {
                    Key::ArrowLeft if ctrl => {
                        let to = motion::prev_word_boundary(&doc.text.buffer, caret_now);
                        caret_now = to;
                        push_move(&mut intents, to);
                    }
                    Key::ArrowRight if ctrl => {
                        let to = motion::next_word_boundary(&doc.text.buffer, caret_now);
                        caret_now = to;
                        push_move(&mut intents, to);
                    }
                    Key::ArrowLeft => {
                        // A plain arrow collapses the selection to its edge.
                        let to = if !shift && doc.has_selection() {
                            doc.sel_range().0
                        } else {
                            doc.text.buffer.prev_char(caret_now)
                        };
                        caret_now = to;
                        push_move(&mut intents, to);
                    }
                    Key::ArrowRight => {
                        let to = if !shift && doc.has_selection() {
                            doc.sel_range().1
                        } else {
                            doc.text.buffer.next_char(caret_now)
                        };
                        caret_now = to;
                        push_move(&mut intents, to);
                    }
                    Key::ArrowUp => {
                        let to = vertical_move(doc, caret_now, -1);
                        caret_now = to;
                        push_vertical(&mut intents, to);
                    }
                    Key::ArrowDown => {
                        let to = vertical_move(doc, caret_now, 1);
                        caret_now = to;
                        push_vertical(&mut intents, to);
                    }
                    Key::Home if ctrl => push_move(&mut intents, 0),
                    Key::End if ctrl => push_move(&mut intents, buf_len),
                    Key::Home => {
                        let to = motion::line_home(&doc.text.buffer, caret_now);
                        caret_now = to;
                        push_move(&mut intents, to);
                    }
                    Key::End => {
                        let to = motion::line_end(&doc.text.buffer, caret_now);
                        caret_now = to;
                        push_move(&mut intents, to);
                    }
                    Key::PageUp => {
                        let to = page_move(doc, caret_now, page as isize, -1);
                        caret_now = to;
                        push_vertical(&mut intents, to);
                        page_scroll -= page as f32 * cx.m.line_h;
                    }
                    Key::PageDown => {
                        let to = page_move(doc, caret_now, page as isize, 1);
                        caret_now = to;
                        push_vertical(&mut intents, to);
                        page_scroll += page as f32 * cx.m.line_h;
                    }
                    Key::Enter => intents.push(Intent::Newline),
                    Key::Tab => intents.push(Intent::Tab),
                    Key::Backspace => intents.push(Intent::Backspace),
                    Key::Delete => intents.push(Intent::Delete),
                    Key::A if ctrl => intents.push(Intent::SelectAll),
                    Key::Z if ctrl && shift => intents.push(Intent::Redo),
                    Key::Z if ctrl => intents.push(Intent::Undo),
                    Key::Y if ctrl => intents.push(Intent::Redo),
                    Key::Escape => intents.push(Intent::BreakGroup),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    doc.text.scroll_y += page_scroll;

    let editable = doc.accepts_edits();
    for intent in intents {
        match intent {
            Intent::Caret(to) => {
                doc.set_caret(to);
                kick_blink(view, ui);
            }
            Intent::Vertical(to) => {
                doc.set_caret_vertical(to);
                kick_blink(view, ui);
            }
            Intent::Extend(to) => {
                doc.extend_to(to);
                doc.text.scroll_to_caret = true;
                kick_blink(view, ui);
            }
            Intent::SelectAll => {
                doc.select_all();
                kick_blink(view, ui);
            }
            Intent::BreakGroup => doc.text.history.break_group(),
            Intent::Copy => {
                if let Some(t) = doc.selected_text() {
                    out.copied = Some(t);
                }
            }
            Intent::Cut => {
                if !editable {
                    out.edit_blocked = true;
                } else if let Some(t) = doc.cut() {
                    out.copied = Some(t);
                    kick_blink(view, ui);
                }
            }
            Intent::Paste => {
                if !editable {
                    out.edit_blocked = true;
                } else {
                    // The app owns the clipboard, so it performs the paste.
                    out.paste_requested = true;
                }
            }
            Intent::Undo => {
                if !editable {
                    out.edit_blocked = true;
                } else {
                    doc.undo();
                    kick_blink(view, ui);
                }
            }
            Intent::Redo => {
                if !editable {
                    out.edit_blocked = true;
                } else {
                    doc.redo();
                    kick_blink(view, ui);
                }
            }
            Intent::Insert(t, kind) => {
                if !editable {
                    out.edit_blocked = true;
                } else if doc.insert_str(&t, kind).is_ok() {
                    kick_blink(view, ui);
                }
            }
            Intent::Newline => {
                if !editable {
                    out.edit_blocked = true;
                } else if doc.insert_newline().is_ok() {
                    kick_blink(view, ui);
                }
            }
            Intent::Tab => {
                if !editable {
                    out.edit_blocked = true;
                } else if doc.insert_tab().is_ok() {
                    kick_blink(view, ui);
                }
            }
            Intent::Backspace => {
                if !editable {
                    out.edit_blocked = true;
                } else if doc.backspace() {
                    kick_blink(view, ui);
                }
            }
            Intent::Delete => {
                if !editable {
                    out.edit_blocked = true;
                } else if doc.delete_forward() {
                    kick_blink(view, ui);
                }
            }
        }
    }
    let _ = scratch;
}

/// Move the caret one line up or down, preserving the desired column.
fn vertical_move(doc: &mut Document, caret: usize, delta: isize) -> usize {
    let line = doc.text.buffer.line_of_byte(caret);
    let col = doc
        .text
        .target_col
        .unwrap_or_else(|| motion::column_of(&doc.text.buffer, caret));
    doc.text.target_col = Some(col);

    let total = doc.line_count();
    let target = if delta < 0 {
        line.saturating_sub(delta.unsigned_abs())
    } else {
        (line + delta as usize).min(total.saturating_sub(1))
    };
    motion::offset_at_column(&doc.text.buffer, target, col)
}

/// Move the caret by whole pages, preserving the desired column.
fn page_move(doc: &mut Document, caret: usize, page: isize, dir: isize) -> usize {
    let line = doc.text.buffer.line_of_byte(caret) as isize;
    let col = doc
        .text
        .target_col
        .unwrap_or_else(|| motion::column_of(&doc.text.buffer, caret));
    doc.text.target_col = Some(col);
    let total = doc.line_count() as isize;
    let target = (line + page * dir).clamp(0, total - 1) as usize;
    motion::offset_at_column(&doc.text.buffer, target, col)
}
