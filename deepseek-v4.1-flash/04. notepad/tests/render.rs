//! Headless tests for the custom text view.
//!
//! The document area has no ready-made widget underneath it, so its layout,
//! hit-testing and input handling are the most failure-prone code in the program —
//! an off-by-one in the byte/pixel mapping would put the caret in the wrong place
//! and a bad slice would panic mid-frame. These tests drive the real
//! [`notepad::ui::textview::show`] through egui's headless runner, so the paint
//! path, the event path and the focus handling all execute for real, without a
//! window.

use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};
use notepad::buffer::Buffer;
use notepad::document::{DocId, Document, Eol};
use notepad::ui::textview::{self, ViewResponse, ViewState};
use notepad::ui::{Metrics, Palette};
use notepad::undo::{EditKind, Sel};
use std::cell::RefCell;

const SCREEN: Vec2 = Vec2::new(900.0, 600.0);
const FONT: f32 = 14.0;
const TAB_COLUMNS: f32 = 4.0;

/// A document plus the view state, driven headlessly.
struct Harness {
    ctx: egui::Context,
    doc: Document,
    view: ViewState,
    metrics: Metrics,
    palette: Palette,
    time: f64,
    /// Response captured from the most recent frame.
    last: RefCell<ViewResponse>,
    /// Shapes emitted by the most recent frame.
    shapes: usize,
    /// Held modifiers for the next frame.
    modifiers: Modifiers,
}

impl Harness {
    fn new(text: &str) -> Self {
        let mut buffer = Buffer::empty();
        buffer.insert(0, text.as_bytes()).unwrap();
        let doc = Document::new(1 as DocId, buffer, None, Eol::Lf, false);

        let ctx = egui::Context::default();
        // One pass to build the font atlas, so metrics can be measured.
        let _ = run(&ctx, 0.0, vec![], &mut |_| {});
        // `Metrics::new` takes the font lock itself, so it must NOT be called from
        // inside `fonts_mut` — that re-enters the same lock and deadlocks.
        let metrics = Metrics::new(&ctx, FONT, TAB_COLUMNS);
        check_metrics(&metrics);

        Harness {
            ctx,
            doc,
            view: ViewState::default(),
            metrics,
            palette: Palette::default(),
            time: 0.0,
            last: RefCell::new(ViewResponse::default()),
            shapes: 0,
            modifiers: Modifiers::NONE,
        }
    }

    /// Run one frame with the given events.
    fn frame(&mut self, events: Vec<Event>) {
        self.time += 1.0 / 60.0;
        let doc = &mut self.doc;
        let view = &mut self.view;
        let metrics = &self.metrics;
        let palette = &self.palette;
        let last = &self.last;
        let modifiers = self.modifiers;
        let mut shapes = 0;
        let out = run_mod(&self.ctx, self.time, events, modifiers, &mut |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    *last.borrow_mut() =
                        textview::show(ui, doc, view, metrics, palette, false);
                });
        });
        shapes += out.shapes.len();
        self.shapes = shapes;
    }

    fn focus(&mut self) {
        let id = textview::focus_id(self.doc.id);
        self.ctx.memory_mut(|m| m.request_focus(id));
        // The focus lock only applies from the second focused frame, so run a pass
        // to let it take effect before sending keys.
        self.frame(vec![]);
    }

    fn text(&self) -> String {
        String::from_utf8(self.doc.text.buffer.read_vec(0, self.doc.len())).unwrap()
    }

    /// Screen position of the cell at `column` on `line`.
    fn cell_pos(&self, line: usize, column: usize) -> Pos2 {
        let gutter = self.gutter_width();
        Pos2::new(
            gutter + column as f32 * self.metrics.char_w + self.metrics.char_w * 0.5,
            line as f32 * self.metrics.line_h + self.metrics.line_h * 0.5,
        )
    }

    fn gutter_width(&self) -> f32 {
        let digits = self.doc.line_count().max(1).to_string().len().max(2);
        (digits as f32 * self.metrics.char_w + 16.0).round()
    }
}

fn run(
    ctx: &egui::Context,
    time: f64,
    events: Vec<Event>,
    ui_fn: &mut dyn FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    run_mod(ctx, time, events, Modifiers::NONE, ui_fn)
}

/// Run a frame with held modifiers.
///
/// `InputState::modifiers` is fed only from `Event::ModifiersChanged`, not from the
/// per-event modifier field, so a Shift+click test has to announce the held state
/// as an event before the pointer events — exactly as a real backend does.
fn run_mod(
    ctx: &egui::Context,
    time: f64,
    mut events: Vec<Event>,
    modifiers: Modifiers,
    ui_fn: &mut dyn FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    if modifiers != Modifiers::NONE {
        events.insert(0, Event::ModifiersChanged(modifiers));
    }
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
        time: Some(time),
        events,
        ..Default::default()
    };
    ctx.run_ui(input, ui_fn)
}

/// Check the measured metrics so a mis-measured font is caught immediately.
///
/// A zero or absurd advance would silently make every caret position wrong, which
/// would then show up as confusing failures in the hit-testing tests.
fn check_metrics(m: &Metrics) {
    assert!(
        m.char_w > 1.0,
        "monospace advance must be measurable, got {}",
        m.char_w
    );
    assert!(
        m.line_h > 4.0,
        "line height must be measurable, got {}",
        m.line_h
    );
    assert!(
        m.char_w < m.line_h * 2.0,
        "advance {:.2} is implausible next to line height {:.2}",
        m.char_w,
        m.line_h
    );
}

fn key(k: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn press(pos: Pos2) -> Vec<Event> {
    vec![
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        },
    ]
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

#[test]
fn renders_tricky_content_without_panicking() {
    // Multi-byte characters, tabs, CRLF, blank lines, a very long line, and a line
    // ending in a lone CR — every case that has bitten the byte/pixel mapping.
    let mut text = String::new();
    text.push_str("plain ascii line\n");
    text.push_str("\tleading tab\n");
    text.push_str("trailing tab\t\n");
    text.push('\n');
    text.push_str("   \n");
    text.push_str("accents café naïve Größe Ångström\n");
    text.push_str("greek Ωμέγα and cyrillic Привет\n");
    text.push_str("cjk 漢字 日本語 한국어\n");
    text.push_str("emoji 😀🚀🌟🎉 and ZWJ 👨‍👩‍👧‍👦\n");
    text.push_str("combining e\u{0301} and x\u{0301}\u{0302}\u{0303}\n");
    text.push_str("crlf line\r\n");
    text.push_str("lone cr at end\r");
    text.push_str(&format!("{}\n", "L".repeat(5000)));
    text.push_str("after the long line\n");

    let mut h = Harness::new(&text);
    for _ in 0..5 {
        h.frame(vec![]);
    }
    assert!(h.shapes > 0, "the view must paint something");

    // Scroll all the way down and horizontally, then paint again.
    h.doc.text.scroll_y = f32::MAX;
    h.frame(vec![]);
    h.doc.text.scroll_x = 100_000.0;
    h.doc.text.scroll_y = 0.0;
    h.frame(vec![]);
    assert!(h.shapes > 0);
}

#[test]
fn paints_only_the_visible_lines() {
    // The same viewport, two documents that differ only in how many lines they
    // have. If rendering touched the whole document, the shape count would grow
    // with the line count; it must not.
    let small = "line\n".repeat(50);
    let huge = "line\n".repeat(200_000);

    let mut a = Harness::new(&small);
    a.frame(vec![]);
    let small_shapes = a.shapes;

    let mut b = Harness::new(&huge);
    b.frame(vec![]);
    let huge_shapes = b.shapes;

    assert!(small_shapes > 0 && huge_shapes > 0);
    // Allow generous slack for the gutter digits and scrollbars, but the counts
    // must be the same order of magnitude: ~40 visible lines either way.
    assert!(
        huge_shapes < small_shapes * 3 + 50,
        "rendering a 200k-line document produced {huge_shapes} shapes versus \
         {small_shapes} for a 50-line one, so it is not limiting itself to the viewport"
    );
}

#[test]
fn long_line_costs_the_same_as_a_short_one() {
    // A single 500,000-byte line versus a single 20-byte line, same viewport.
    let short = Harness::new("short line\n");
    let long = Harness::new(&format!("{}\n", "L".repeat(500_000)));

    let mut s = short;
    s.frame(vec![]);
    let mut l = long;
    l.frame(vec![]);

    assert!(
        l.shapes < s.shapes * 4 + 50,
        "a 500,000-byte line produced {} shapes versus {} for a short line, so the \
         renderer is working on the whole line instead of the visible window",
        l.shapes,
        s.shapes
    );
}

// ---------------------------------------------------------------------------
// Hit testing
// ---------------------------------------------------------------------------

#[test]
fn clicking_places_the_caret_at_the_clicked_character() {
    let mut h = Harness::new("abcdefghij\nsecond line\nthird\n");
    h.frame(vec![]);

    // Click in the middle of the character at each column of the first line.
    for column in 0..10usize {
        let pos = h.cell_pos(0, column);
        h.frame(press(pos));
        assert_eq!(
            h.doc.text.sel.caret, column,
            "clicking column {column} put the caret at {}",
            h.doc.text.sel.caret
        );
        assert!(h.doc.text.sel.is_empty(), "a plain click must not select");
    }

    // And on later lines, which exercises the vertical mapping too.
    for (line, text) in [(1usize, "second line"), (2, "third")] {
        for column in 0..text.len() {
            let pos = h.cell_pos(line, column);
            h.frame(press(pos));
            let expected = h.doc.text.buffer.line_start(line) + column;
            assert_eq!(
                h.doc.text.sel.caret, expected,
                "clicking line {line} column {column}"
            );
        }
    }
}

#[test]
fn clicking_past_the_end_of_a_line_clamps_to_it() {
    let mut h = Harness::new("ab\nlonger second line\n");
    h.frame(vec![]);
    // Column 20 is well past the end of "ab".
    let pos = h.cell_pos(0, 20);
    h.frame(press(pos));
    assert_eq!(h.doc.text.sel.caret, 2, "clamps to the end of the line content");
}

#[test]
fn clicking_below_the_last_line_clamps_to_the_document_end() {
    let mut h = Harness::new("only one line\n");
    h.frame(vec![]);
    // Below the last line but still inside the viewport (the document is two lines:
    // the text and the empty line after its newline).
    let pos = Pos2::new(h.gutter_width() + 4.0, h.metrics.line_h * 1.5);
    h.frame(press(pos));
    assert_eq!(h.doc.text.sel.caret, h.doc.len());
}

#[test]
fn shift_click_extends_the_selection() {
    let mut h = Harness::new("abcdefghij\n");
    h.frame(vec![]);
    h.frame(press(h.cell_pos(0, 2)));
    assert_eq!(h.doc.text.sel, Sel::caret(2));

    let pos = h.cell_pos(0, 7);
    h.modifiers = Modifiers::SHIFT;
    h.frame(vec![
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::SHIFT,
        },
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::SHIFT,
        },
    ]);
    h.modifiers = Modifiers::NONE;
    assert_eq!(h.doc.text.sel, Sel { anchor: 2, caret: 7 });
}

// ---------------------------------------------------------------------------
// Keyboard through the real widget
// ---------------------------------------------------------------------------

#[test]
fn typing_inserts_text_at_the_caret() {
    let mut h = Harness::new("hello\n");
    h.frame(vec![]);
    h.frame(press(h.cell_pos(0, 5)));
    h.focus();

    h.frame(vec![Event::Text(" world".to_string())]);
    assert_eq!(h.text(), "hello world\n");
    assert_eq!(h.doc.text.sel.caret, 11);
}

#[test]
fn tab_inserts_a_tab_rather_than_moving_focus() {
    // egui treats Tab as focus navigation unless the widget claims it, so this
    // verifies the focus lock is actually in effect.
    let mut h = Harness::new("ab\n");
    h.frame(vec![]);
    h.frame(press(h.cell_pos(0, 2)));
    assert_eq!(h.doc.text.sel.caret, 2);
    h.focus();

    h.frame(vec![key(Key::Tab, Modifiers::NONE)]);
    assert_eq!(h.text(), "ab\t\n", "Tab must reach the document");
    assert_eq!(
        h.ctx.memory(|m| m.focused()),
        Some(textview::focus_id(h.doc.id)),
        "focus must stay on the editor"
    );
}

#[test]
fn arrow_keys_move_the_caret_rather_than_moving_focus() {
    let mut h = Harness::new("abc\ndef\n");
    h.frame(vec![]);
    h.frame(press(h.cell_pos(0, 0)));
    h.focus();
    assert_eq!(h.doc.text.sel.caret, 0);

    h.frame(vec![key(Key::ArrowRight, Modifiers::NONE)]);
    assert_eq!(h.doc.text.sel.caret, 1);

    h.frame(vec![key(Key::ArrowDown, Modifiers::NONE)]);
    assert_eq!(h.doc.text.sel.caret, 5, "one line down, same column");

    h.frame(vec![key(Key::ArrowUp, Modifiers::NONE)]);
    assert_eq!(h.doc.text.sel.caret, 1, "back up to the same column");

    h.frame(vec![key(Key::ArrowLeft, Modifiers::NONE)]);
    assert_eq!(h.doc.text.sel.caret, 0);

    assert_eq!(
        h.ctx.memory(|m| m.focused()),
        Some(textview::focus_id(h.doc.id)),
        "the arrows must not have moved focus away"
    );
}

#[test]
fn backspace_and_delete_edit_the_document() {
    let mut h = Harness::new("abc\n");
    h.frame(vec![]);
    h.frame(press(h.cell_pos(0, 3)));
    h.focus();
    assert_eq!(h.doc.text.sel.caret, 3);

    h.frame(vec![key(Key::Backspace, Modifiers::NONE)]);
    assert_eq!(h.text(), "ab\n");

    h.doc.set_caret(0);
    h.frame(vec![]);
    h.frame(vec![key(Key::Delete, Modifiers::NONE)]);
    assert_eq!(h.text(), "b\n");
}

#[test]
fn shift_arrows_extend_the_selection() {
    let mut h = Harness::new("abcdef\n");
    h.frame(vec![]);
    h.frame(press(h.cell_pos(0, 1)));
    h.focus();

    let shift = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    h.frame(vec![key(Key::ArrowRight, shift)]);
    h.frame(vec![key(Key::ArrowRight, shift)]);
    assert_eq!(h.doc.text.sel, Sel { anchor: 1, caret: 3 });

    // A plain arrow collapses the selection to its edge instead of moving.
    h.frame(vec![key(Key::ArrowLeft, Modifiers::NONE)]);
    assert_eq!(h.doc.text.sel, Sel::caret(1));
}

#[test]
fn ctrl_arrows_move_by_word() {
    let mut h = Harness::new("alpha beta gamma\n");
    h.frame(vec![]);
    h.frame(press(h.cell_pos(0, 0)));
    h.focus();

    let ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    h.frame(vec![key(Key::ArrowRight, ctrl)]);
    assert_eq!(h.doc.text.sel.caret, 6, "start of the second word");
    h.frame(vec![key(Key::ArrowRight, ctrl)]);
    assert_eq!(h.doc.text.sel.caret, 11, "start of the third word");
    h.frame(vec![key(Key::ArrowLeft, ctrl)]);
    assert_eq!(h.doc.text.sel.caret, 6);
}

#[test]
fn ctrl_shift_arrows_select_by_word() {
    let mut h = Harness::new("alpha beta gamma\n");
    h.frame(vec![]);
    h.frame(press(h.cell_pos(0, 0)));
    h.focus();

    let both = Modifiers {
        ctrl: true,
        command: true,
        shift: true,
        ..Modifiers::NONE
    };
    h.frame(vec![key(Key::ArrowRight, both)]);
    assert_eq!(h.doc.text.sel, Sel { anchor: 0, caret: 6 });
    h.frame(vec![key(Key::ArrowRight, both)]);
    assert_eq!(h.doc.text.sel, Sel { anchor: 0, caret: 11 });
}

#[test]
fn home_end_and_document_navigation() {
    let mut h = Harness::new("    indented\nsecond\nthird\n");
    h.frame(vec![]);
    h.frame(press(h.cell_pos(0, 10)));
    h.focus();

    h.frame(vec![key(Key::Home, Modifiers::NONE)]);
    assert_eq!(h.doc.text.sel.caret, 4, "first press: first non-blank");
    h.frame(vec![key(Key::Home, Modifiers::NONE)]);
    assert_eq!(h.doc.text.sel.caret, 0, "second press: column 0");

    h.frame(vec![key(Key::End, Modifiers::NONE)]);
    assert_eq!(h.doc.text.sel.caret, 12, "end of the line content");

    let ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    h.frame(vec![key(Key::End, ctrl)]);
    assert_eq!(h.doc.text.sel.caret, h.doc.len());
    h.frame(vec![key(Key::Home, ctrl)]);
    assert_eq!(h.doc.text.sel.caret, 0);
}

#[test]
fn page_keys_move_the_caret_and_the_viewport() {
    let text = (0..500).map(|i| format!("line {i}\n")).collect::<String>();
    let mut h = Harness::new(&text);
    h.frame(vec![]);
    h.frame(press(h.cell_pos(0, 0)));
    h.focus();
    assert_eq!(h.doc.text.sel.caret, 0);

    h.frame(vec![key(Key::PageDown, Modifiers::NONE)]);
    let after_page = h.doc.text.sel.caret;
    assert!(after_page > 0, "PageDown must move the caret");
    assert!(
        h.doc.text.buffer.line_of_byte(after_page) > 10,
        "PageDown should advance roughly a screenful, went to line {}",
        h.doc.text.buffer.line_of_byte(after_page)
    );
    assert!(h.doc.text.scroll_y > 0.0, "the viewport must follow");

    h.frame(vec![key(Key::PageUp, Modifiers::NONE)]);
    assert_eq!(h.doc.text.sel.caret, 0, "PageUp returns to the start");
}

#[test]
fn ctrl_a_selects_everything_and_typing_replaces_it() {
    let mut h = Harness::new("one\ntwo\nthree\n");
    h.frame(vec![]);
    h.focus();

    let ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    h.frame(vec![key(Key::A, ctrl)]);
    assert_eq!(h.doc.text.sel, Sel { anchor: 0, caret: h.doc.len() });

    h.frame(vec![Event::Text("replaced".to_string())]);
    assert_eq!(h.text(), "replaced");
    assert_eq!(h.doc.text.history.undo_depth(), 1, "one undoable action");
}

#[test]
fn ctrl_z_and_ctrl_y_undo_and_redo_through_the_view() {
    let mut h = Harness::new("");
    h.frame(vec![]);
    h.focus();

    let ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    h.frame(vec![Event::Text("hello".to_string())]);
    assert_eq!(h.text(), "hello");

    h.frame(vec![key(Key::Z, ctrl)]);
    assert_eq!(h.text(), "", "Ctrl+Z must undo");

    h.frame(vec![key(Key::Y, ctrl)]);
    assert_eq!(h.text(), "hello", "Ctrl+Y must redo");
}

#[test]
fn cut_reports_text_for_the_clipboard() {
    let mut h = Harness::new("hello world\n");
    h.frame(vec![]);
    h.doc.set_sel(Sel { anchor: 0, caret: 6 });
    h.focus();

    let ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    h.frame(vec![key(Key::X, ctrl)]);
    assert_eq!(h.text(), "world\n", "Ctrl+X must cut");
    assert_eq!(h.last.borrow().copied.as_deref(), Some("hello "));

    // The same must work through the event a real backend sends.
    h.doc.undo();
    h.doc.set_sel(Sel { anchor: 0, caret: 6 });
    h.frame(vec![Event::Cut]);
    assert_eq!(h.text(), "world\n", "Event::Cut must cut too");
    assert_eq!(
        h.last.borrow().copied.as_deref(),
        Some("hello "),
        "the cut text must be handed to the app for the clipboard"
    );
}

#[test]
fn editing_is_refused_and_reported_while_a_background_operation_runs() {
    use notepad::progress::{Op, OpKind};

    let mut h = Harness::new("text\n");
    h.frame(vec![]);
    h.focus();

    // Simulate a replace-all owning the document.
    h.doc.op = Some(Op::new(OpKind::ReplaceAll));
    h.frame(vec![Event::Text("X".to_string())]);
    assert_eq!(h.text(), "text\n", "the edit must not be applied");
    assert!(
        h.last.borrow().edit_blocked,
        "the refusal must be reported so the app can tell the user"
    );

    // Once the operation ends, editing resumes.
    h.doc.op = None;
    h.frame(vec![Event::Text("X".to_string())]);
    assert_eq!(h.text(), "Xtext\n");
}

#[test]
fn search_highlights_do_not_break_painting() {
    use notepad::search::Match;

    let mut h = Harness::new("needle here and needle there\nshort\n");
    // Highlight with matches that extend past the end of a line, which is where a
    // naive clamp would panic.
    h.doc.search.matches = vec![
        Match { start: 0, end: 6 },
        Match { start: 17, end: 23 },
        Match { start: 28, end: h.doc.len() },
    ];
    h.doc.search.current = Some(1);
    h.frame(vec![]);
    assert!(h.shapes > 0);

    // A match that starts inside a character must not panic the painter either.
    h.doc.search.matches = vec![Match { start: 1, end: 2 }];
    h.doc.search.current = Some(0);
    h.frame(vec![]);
    assert!(h.shapes > 0);
}

#[test]
fn caret_stays_visible_while_navigating() {
    let text = (0..400).map(|i| format!("line {i}\n")).collect::<String>();
    let mut h = Harness::new(&text);
    h.frame(vec![]);
    h.focus();

    let ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    // Jump to the end of the document; the viewport must follow.
    h.frame(vec![key(Key::End, ctrl)]);
    assert!(
        h.doc.text.scroll_y > 0.0,
        "the viewport must scroll to keep the caret visible"
    );

    let line = h.doc.text.buffer.line_of_byte(h.doc.text.sel.caret);
    let top = line as f32 * h.metrics.line_h;
    let view_h = SCREEN.y;
    assert!(
        top >= h.doc.text.scroll_y - h.metrics.line_h
            && top <= h.doc.text.scroll_y + view_h + h.metrics.line_h,
        "caret line {line} at y={top} is outside the viewport starting at {}",
        h.doc.text.scroll_y
    );
}

#[test]
fn horizontal_scroll_follows_a_caret_far_to_the_right() {
    let mut h = Harness::new(&format!("{}\n", "x".repeat(4000)));
    h.frame(vec![]);
    h.focus();

    // End of the long line. (Ctrl+End would go to the empty line after the
    // terminator, which is at column 0 and needs no horizontal scroll.)
    h.frame(vec![key(Key::End, Modifiers::NONE)]);
    assert_eq!(h.doc.text.sel.caret, 4000);
    assert!(
        h.doc.text.scroll_x > 0.0,
        "the viewport must scroll right to keep a distant caret visible, got {}",
        h.doc.text.scroll_x
    );
    assert!(h.shapes > 0);
}

#[test]
fn selection_paints_without_panicking_at_line_edges() {
    let mut h = Harness::new("ab\ncd\nef\n");
    h.frame(vec![]);

    // Selections that span whole lines, end exactly on a terminator, and cover the
    // entire document — the cases where the newline cell is drawn.
    for (a, b) in [
        (0usize, 3usize),
        (0, 2),
        (2, 3),
        (0, h.doc.len()),
        (1, 7),
        (5, 6),
    ] {
        h.doc.text.sel = Sel { anchor: a, caret: b };
        h.frame(vec![]);
        assert!(h.shapes > 0, "selection {a}..{b} failed to paint");
    }
}

#[test]
fn empty_document_paints_and_accepts_typing() {
    let mut h = Harness::new("");
    h.frame(vec![]);
    assert!(h.shapes > 0, "an empty document must still draw the gutter");

    h.focus();
    h.frame(vec![Event::Text("first".to_string())]);
    assert_eq!(h.text(), "first");
    assert_eq!(h.doc.line_count(), 1);
}

#[test]
fn document_with_only_newlines_has_correct_line_count() {
    let mut h = Harness::new("\n\n\n");
    h.frame(vec![]);
    assert_eq!(h.doc.line_count(), 4);
    assert!(h.shapes > 0);
}

#[test]
fn undo_of_typing_through_the_view_is_one_step() {
    let mut h = Harness::new("");
    h.frame(vec![]);
    h.focus();

    for ch in ["h", "e", "l", "l", "o"] {
        h.frame(vec![Event::Text(ch.to_string())]);
    }
    assert_eq!(h.text(), "hello");

    let ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    h.frame(vec![key(Key::Z, ctrl)]);
    assert_eq!(h.text(), "", "consecutive typing is a single undo step");
}

#[test]
fn editing_kind_is_recorded_for_grouping() {
    // The view must tag typing as typing (groupable) rather than as paste
    // (never grouped), which is what makes undo steps sensible.
    let mut h = Harness::new("");
    h.frame(vec![]);
    h.focus();
    h.frame(vec![Event::Text("ab".to_string())]);
    assert_eq!(h.doc.text.history.undo_depth(), 1);

    // A paste is its own step and does not merge with the typing before it.
    h.doc.insert_str("PASTED", EditKind::Paste).unwrap();
    assert_eq!(h.doc.text.history.undo_depth(), 2);
}
