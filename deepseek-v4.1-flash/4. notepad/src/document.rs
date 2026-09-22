//! Per-document state and the editing operations that act on it.
//!
//! A [`Document`] owns everything that must survive switching tabs: the buffer,
//! its undo history, cursor/selection, scroll offset, file identity, line-ending
//! style, search state, and any background operation in flight.

use crate::buffer::{Buffer, BufferError, Delta, Snapshot};
use crate::motion;
use crate::progress::Op;
use crate::search::SearchState;
use crate::undo::{make_edit, EditKind, History, Sel};
use std::path::PathBuf;
use std::time::Instant;

pub type DocId = u64;

/// Line-ending style of a document.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Eol {
    Lf,
    Crlf,
    /// The file mixes both; new lines follow the majority style.
    Mixed,
}

impl Eol {
    pub fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
            Eol::Mixed => "Mixed",
        }
    }

    /// Bytes inserted for a new line.
    pub fn newline(self) -> &'static str {
        match self {
            Eol::Crlf => "\r\n",
            _ => "\n",
        }
    }
}

/// Detect the dominant line-ending style of raw bytes.
pub fn detect_eol(bytes: &[u8]) -> Eol {
    let lf = memchr::memchr_iter(b'\n', bytes).count();
    if lf == 0 {
        return Eol::Lf;
    }
    let crlf = memchr::memchr_iter(b'\r', bytes)
        .filter(|&i| bytes.get(i + 1) == Some(&b'\n'))
        .count();
    if crlf == 0 {
        Eol::Lf
    } else if crlf == lf {
        Eol::Crlf
    } else {
        Eol::Mixed
    }
}

/// A transient message shown to the user.
pub struct Notice {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// Text, selection, history and viewport for one document.
pub struct TextState {
    pub buffer: Buffer,
    pub history: History,
    pub sel: Sel,
    /// Desired column for Up/Down; cleared by any horizontal motion.
    pub target_col: Option<usize>,
    pub scroll_x: f32,
    pub scroll_y: f32,
    /// Widest line measured so far, used to size the horizontal scrollbar.
    pub max_line_width: f32,
    /// Set when the caret moved and the viewport must reveal it.
    pub scroll_to_caret: bool,
}

impl TextState {
    pub fn new(buffer: Buffer) -> Self {
        TextState {
            buffer,
            history: History::default(),
            sel: Sel::caret(0),
            target_col: None,
            scroll_x: 0.0,
            scroll_y: 0.0,
            max_line_width: 0.0,
            scroll_to_caret: false,
        }
    }
}

pub struct Document {
    pub id: DocId,
    pub text: TextState,
    pub path: Option<PathBuf>,
    pub eol: Eol,
    /// The file began with a UTF-8 BOM; preserved on save.
    pub had_bom: bool,
    /// Background operation currently owning this document.
    pub op: Option<Op>,
    pub search: SearchState,
    /// Bumped on every edit so in-flight searches can be discarded.
    pub search_epoch: u64,
    pub notice: Option<Notice>,
    col_cache: Option<(u64, usize, (usize, usize))>,
}

impl Document {
    pub fn new(id: DocId, buffer: Buffer, path: Option<PathBuf>, eol: Eol, had_bom: bool) -> Self {
        Document {
            id,
            text: TextState::new(buffer),
            path,
            eol,
            had_bom,
            op: None,
            search: SearchState::default(),
            search_epoch: 0,
            notice: None,
            col_cache: None,
        }
    }

    // ---- identity and status -------------------------------------------

    pub fn is_dirty(&self) -> bool {
        self.text.history.is_dirty()
    }

    pub fn len(&self) -> usize {
        self.text.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.buffer.is_empty()
    }

    /// Name shown on the tab.
    pub fn display_name(&self) -> String {
        match &self.path {
            Some(p) => p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned()),
            None => "Untitled".to_string(),
        }
    }

    /// Tab label, with the unsaved-change indicator.
    pub fn tab_label(&self) -> String {
        let name = self.display_name();
        if self.is_dirty() {
            format!("{name} *")
        } else {
            name
        }
    }

    pub fn set_notice(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some(Notice {
            text: text.into(),
            error,
            at: Instant::now(),
        });
    }

    /// True while a background operation prevents editing.
    pub fn editing_locked(&self) -> bool {
        self.op.as_ref().is_some_and(|o| o.kind.locks_editing())
    }

    /// Whether the document may accept edits right now.
    pub fn accepts_edits(&self) -> bool {
        !self.editing_locked()
    }

    pub fn mark_saved(&mut self) {
        self.text.history.mark_saved();
    }

    /// Record the undo serial whose state was written to disk.
    pub fn mark_saved_at(&mut self, serial: u64) {
        self.text.history.mark_saved_at(serial);
    }

    /// Serial of the topmost applied edit, captured before a save starts.
    pub fn applied_serial(&self) -> u64 {
        self.text.history.applied_serial()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.text.buffer.snapshot()
    }

    /// Take a snapshot for saving, with the undo serial it represents.
    ///
    /// Breaks the open undo group first. That guarantees the returned serial names
    /// a real, separate undo step which can never afterwards be absorbed by a
    /// later edit — and it is precisely that property which makes "undo back to
    /// the saved state clears the unsaved indicator" reliable. Merging an edit
    /// away removes its serial from the stack, so a save point captured without
    /// this guarantee could become unreachable by undo.
    pub fn snapshot_for_save(&mut self) -> (Snapshot, u64) {
        self.text.history.break_group();
        let serial = self.text.history.applied_serial();
        (self.text.buffer.snapshot(), serial)
    }

    // ---- selection ------------------------------------------------------

    #[inline]
    pub fn sel_range(&self) -> (usize, usize) {
        self.text.sel.range()
    }

    pub fn has_selection(&self) -> bool {
        !self.text.sel.is_empty()
    }

    pub fn set_sel(&mut self, sel: Sel) {
        let len = self.text.buffer.len();
        self.text.sel = sel.clamped(len);
        self.text.target_col = None;
        self.text.scroll_to_caret = true;
    }

    /// Move the caret, collapsing any selection.
    pub fn set_caret(&mut self, pos: usize) {
        let p = pos.min(self.text.buffer.len());
        self.text.sel = Sel::caret(p);
        // A horizontal move abandons the remembered column.
        self.text.target_col = None;
        self.text.scroll_to_caret = true;
        // A caret move ends the current undo group.
        self.text.history.break_group();
    }

    /// Move the caret vertically, keeping the remembered column.
    ///
    /// Up/Down and PageUp/PageDown must not clear [`TextState::target_col`],
    /// otherwise walking through a short line would permanently clamp the caret to
    /// that shorter column instead of returning to the original one.
    pub fn set_caret_vertical(&mut self, pos: usize) {
        let p = pos.min(self.text.buffer.len());
        self.text.sel = Sel::caret(p);
        self.text.scroll_to_caret = true;
        self.text.history.break_group();
    }

    /// Extend the selection to `pos`, keeping the anchor.
    pub fn extend_to(&mut self, pos: usize) {
        let len = self.text.buffer.len();
        self.text.sel.caret = pos.min(len);
        self.text.scroll_to_caret = true;
    }

    pub fn select_all(&mut self) {
        let len = self.text.buffer.len();
        self.text.sel = Sel {
            anchor: 0,
            caret: len,
        };
        self.text.scroll_to_caret = false;
    }

    // ---- mutations ------------------------------------------------------

    /// Record a completed mutation and invalidate anything that depended on the
    /// previous text.
    fn commit(
        &mut self,
        delta: Delta,
        pos: usize,
        sel_before: Sel,
        sel_after: Sel,
        kind: EditKind,
    ) {
        let edit = make_edit(
            pos,
            delta.removed,
            delta.removed_len,
            delta.inserted,
            delta.inserted_len,
            sel_before,
            sel_after,
            kind,
        );
        self.text.history.record(edit);
        self.text.sel = sel_after;
        self.text.scroll_to_caret = true;
        self.note_change();
    }

    /// Invalidate search results and cancel an in-flight scan, because the text
    /// they describe no longer exists.
    fn note_change(&mut self) {
        self.search_epoch += 1;
        self.col_cache = None;
        self.search.invalidate();
        if let Some(op) = &self.op {
            if op.kind == crate::progress::OpKind::Search {
                op.progress.cancel();
            }
        }
    }

    /// Replace the current selection with `text`.
    ///
    /// Returns `Err` only when the result would exceed the document size limit;
    /// in that case nothing is modified.
    pub fn insert_str(&mut self, text: &str, kind: EditKind) -> Result<(), BufferError> {
        if text.is_empty() {
            return Ok(());
        }
        let before = self.text.sel.clamped(self.text.buffer.len());
        let (from, to) = before.range();
        let delta = self.text.buffer.replace_capture(from, to, text.as_bytes())?;
        let after = Sel::caret(from + text.len());
        self.commit(delta, from, before, after, kind);
        Ok(())
    }

    pub fn insert_newline(&mut self) -> Result<(), BufferError> {
        let nl = self.eol.newline();
        self.insert_str(nl, EditKind::Typing)
    }

    pub fn insert_tab(&mut self) -> Result<(), BufferError> {
        self.insert_str("\t", EditKind::Tab)
    }

    /// Backspace: delete the selection, or one character before the caret.
    pub fn backspace(&mut self) -> bool {
        let before = self.text.sel.clamped(self.text.buffer.len());
        let (a, b) = before.range();
        let (from, to) = if a != b {
            (a, b)
        } else {
            let p = self.text.buffer.prev_char(a);
            if p == a {
                return false;
            }
            (p, a)
        };
        let delta = self.text.buffer.remove(from, to);
        let after = Sel::caret(from);
        self.commit(delta, from, before, after, EditKind::Backspace);
        true
    }

    /// Delete: remove the selection, or one character after the caret.
    pub fn delete_forward(&mut self) -> bool {
        let before = self.text.sel.clamped(self.text.buffer.len());
        let (a, b) = before.range();
        let (from, to) = if a != b {
            (a, b)
        } else {
            let p = self.text.buffer.next_char(a);
            if p == a {
                return false;
            }
            (a, p)
        };
        let delta = self.text.buffer.remove(from, to);
        let after = Sel::caret(from);
        self.commit(delta, from, before, after, EditKind::Delete);
        true
    }

    /// Text of the current selection, if any.
    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.sel_range();
        if a == b {
            return None;
        }
        let bytes = self.text.buffer.read_vec(a, b);
        String::from_utf8(bytes).ok()
    }

    /// Cut the selection: returns the removed text so the caller can place it on
    /// the clipboard.
    pub fn cut(&mut self) -> Option<String> {
        let text = self.selected_text()?;
        let before = self.text.sel.clamped(self.text.buffer.len());
        let (from, to) = before.range();
        let delta = self.text.buffer.remove(from, to);
        let after = Sel::caret(from);
        self.commit(delta, from, before, after, EditKind::Cut);
        Some(text)
    }

    /// Undo one step. Returns true when something was undone.
    pub fn undo(&mut self) -> bool {
        match self.text.history.undo(&mut self.text.buffer) {
            Some(sel) => {
                self.text.sel = sel.clamped(self.text.buffer.len());
                self.text.target_col = None;
                self.text.scroll_to_caret = true;
                self.note_change();
                true
            }
            None => false,
        }
    }

    /// Redo one step. Returns true when something was redone.
    pub fn redo(&mut self) -> bool {
        match self.text.history.redo(&mut self.text.buffer) {
            Some(sel) => {
                self.text.sel = sel.clamped(self.text.buffer.len());
                self.text.target_col = None;
                self.text.scroll_to_caret = true;
                self.note_change();
                true
            }
            None => false,
        }
    }

    /// Apply a replace-all result produced by a background worker.
    ///
    /// `segments` is the worker's plan; the bytes it wants inserted are
    /// materialised into this document's existing chunk store, so every piece
    /// that undo will need stays valid. Recorded as a single undo step.
    pub fn apply_replace_all(&mut self, segments: Vec<crate::search::Segment>, count: usize) {
        use crate::search::Segment;

        let before = self.text.sel.clamped(self.text.buffer.len());
        let old_len = self.text.buffer.len();
        let old_pieces = self.text.buffer.flatten();

        // Materialise the plan against the live store.
        let mut new_pieces = Vec::with_capacity(segments.len());
        for seg in segments {
            match seg {
                Segment::Keep(p) => new_pieces.push(p),
                Segment::Insert(bytes) => {
                    new_pieces.extend(self.text.buffer.stage_bytes(&bytes));
                }
            }
        }
        let new_len: usize = new_pieces.iter().map(|p| p.len as usize).sum();

        self.text.buffer.install(new_pieces.clone());
        let after = Sel::caret(before.caret.min(new_len));
        let delta = Delta {
            removed: old_pieces,
            removed_len: old_len,
            inserted: new_pieces,
            inserted_len: new_len,
        };
        self.commit(delta, 0, before, after, EditKind::ReplaceAll);
        self.set_notice(format!("Replaced {count} occurrence(s)"), false);
    }

    // ---- status ---------------------------------------------------------

    /// 1-based `(line, column)` of the caret.
    pub fn cursor_line_col(&mut self) -> (usize, usize) {
        let caret = self.text.sel.caret.min(self.text.buffer.len());
        let key = (self.text.buffer.generation(), caret);
        if let Some((g, c, out)) = self.col_cache {
            if (g, c) == key {
                return out;
            }
        }
        let line = self.text.buffer.line_of_byte(caret);
        let col = motion::column_of(&self.text.buffer, caret);
        let out = (line + 1, col + 1);
        self.col_cache = Some((key.0, key.1, out));
        out
    }

    pub fn line_count(&self) -> usize {
        self.text.buffer.line_count()
    }
}
