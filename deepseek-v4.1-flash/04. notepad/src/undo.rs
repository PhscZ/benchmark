//! Undo/redo history.
//!
//! # Representation
//!
//! An [`Edit`] records a *piece-level* delta, not a text copy:
//!
//! ```text
//!   removed  : the pieces that used to occupy [pos, pos + removed_len)
//!   inserted : the pieces that now occupy     [pos, pos + inserted_len)
//! ```
//!
//! Because pieces point into immutable sources, undoing an edit is a splice of
//! the piece list — no bytes are copied, and undoing a 100 MiB delete is as cheap
//! as undoing a one-character one.
//!
//! # Saved-state tracking
//!
//! Every edit carries a monotonically increasing serial. `applied` is the serial
//! of the topmost applied edit (0 = pristine), `saved` is the serial that was on
//! top at the last successful save. The document is dirty exactly when
//! `applied != saved`, which makes "undo back to the saved state clears the
//! indicator" fall out for free — including the case where the user saves, edits,
//! and then undoes, or saves, undoes past the save point, and redoes onto it.
//!
//! # Grouping
//!
//! Consecutive keystrokes coalesce into one undo step. A group is *open* only
//! while the user keeps doing the same thing at the same place within
//! [`GROUP_WINDOW`]; a cursor move, a save, a paste, an undo/redo, or a newline
//! closes it. Closing on save is what keeps "type, save, type" two undo steps
//! instead of one, so undoing after a save lands exactly on the saved state.

use crate::buffer::Buffer;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Consecutive same-kind edits within this window coalesce.
pub const GROUP_WINDOW: Duration = Duration::from_millis(700);

/// Default ceiling on text retained by history, in bytes.
///
/// Undo records keep deleted and inserted bytes alive. Without a cap, a long
/// session on a large file would grow without bound; with it, the oldest steps
/// are dropped first, which only costs the ability to undo that far back.
pub const DEFAULT_HISTORY_BUDGET: usize = 256 * 1024 * 1024;

/// What produced an edit; drives grouping.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    /// Ordinary character insertion.
    Typing,
    /// Backspace.
    Backspace,
    /// Forward delete.
    Delete,
    /// Tab insertion.
    Tab,
    /// Paste.
    Paste,
    /// Cut.
    Cut,
    /// Replace of the current match.
    Replace,
    /// Replace-all across the document.
    ReplaceAll,
    /// Programmatic whole-document swap.
    Reload,
}

/// Selection snapshot: anchor stays put, caret moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Sel {
    pub anchor: usize,
    pub caret: usize,
}

impl Sel {
    #[inline]
    pub fn caret(at: usize) -> Self {
        Sel {
            anchor: at,
            caret: at,
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.caret
    }

    /// `(start, end)` with `start <= end`.
    #[inline]
    pub fn range(&self) -> (usize, usize) {
        if self.anchor <= self.caret {
            (self.anchor, self.caret)
        } else {
            (self.caret, self.anchor)
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        let (a, b) = self.range();
        b - a
    }

    /// Clamp both ends to `[0, len]`.
    pub fn clamped(self, len: usize) -> Self {
        Sel {
            anchor: self.anchor.min(len),
            caret: self.caret.min(len),
        }
    }
}

/// One reversible document mutation.
pub struct Edit {
    /// Where the change happened.
    pub pos: usize,
    /// Pieces removed from `[pos, pos + removed_len)`.
    pub removed: Vec<crate::buffer::Piece>,
    pub removed_len: usize,
    /// Pieces inserted at `pos`, occupying `[pos, pos + inserted_len)`.
    pub inserted: Vec<crate::buffer::Piece>,
    pub inserted_len: usize,
    /// Selection before the edit; restored on undo.
    pub sel_before: Sel,
    /// Selection after the edit; restored on redo.
    pub sel_after: Sel,
    pub kind: EditKind,
    /// Monotonic serial; see the module docs.
    pub serial: u64,
}

impl Edit {
    /// Bytes this record keeps alive.
    fn retained(&self) -> usize {
        self.removed_len + self.inserted_len
    }

    /// Undo the edit against `buffer`.
    fn revert(&self, buffer: &mut Buffer) {
        if self.inserted_len > 0 {
            buffer.take_pieces(self.pos, self.pos + self.inserted_len);
        }
        buffer.insert_pieces(self.pos, &self.removed);
    }

    /// Redo the edit against `buffer`.
    fn reapply(&self, buffer: &mut Buffer) {
        if self.removed_len > 0 {
            buffer.take_pieces(self.pos, self.pos + self.removed_len);
        }
        buffer.insert_pieces(self.pos, &self.inserted);
    }

    /// Whether `next` can be folded into `self` as one undo step.
    fn absorbs(&self, next: &Edit) -> bool {
        match (self.kind, next.kind) {
            (EditKind::Typing, EditKind::Typing) => {
                // Only pure appends right after the previous insertion. A
                // newline is the natural group boundary, so an edit containing
                // one neither absorbs nor is absorbed.
                self.removed.is_empty()
                    && next.removed.is_empty()
                    && next.pos == self.pos + self.inserted_len
                    && !self.contains_newline()
                    && !next.contains_newline()
            }
            (EditKind::Backspace, EditKind::Backspace) => {
                // Backspace walks backwards, so the newer edit starts exactly
                // where the older one ended.
                self.inserted.is_empty()
                    && next.inserted.is_empty()
                    && next.pos + next.removed_len == self.pos
            }
            (EditKind::Delete, EditKind::Delete) => {
                // Forward delete keeps the same start; the tail shifts left.
                self.inserted.is_empty()
                    && next.inserted.is_empty()
                    && next.pos == self.pos
            }
            _ => false,
        }
    }

    fn contains_newline(&self) -> bool {
        self.inserted.iter().any(|p| p.nl > 0)
    }

    /// Fold `next` into `self`, keeping the earlier selection as `sel_before`.
    fn merge(&mut self, next: Edit) {
        match self.kind {
            EditKind::Typing => {
                self.inserted.extend(next.inserted);
                self.inserted_len += next.inserted_len;
            }
            EditKind::Backspace => {
                // `next` lies earlier in the document, so its pieces come first.
                let mut pieces = next.removed;
                pieces.append(&mut self.removed);
                self.removed = pieces;
                self.removed_len += next.removed_len;
                self.pos = next.pos;
            }
            EditKind::Delete => {
                self.removed.extend(next.removed);
                self.removed_len += next.removed_len;
            }
            _ => unreachable!("only grouping kinds are merged"),
        }
        self.sel_after = next.sel_after;
        self.serial = next.serial;
    }
}

/// Undo/redo stacks for one document.
pub struct History {
    undo: VecDeque<Edit>,
    redo: VecDeque<Edit>,
    next_serial: u64,
    /// Serial of the topmost applied edit; 0 means "pristine".
    applied: u64,
    /// Serial on top at the last save; 0 means "saved while pristine".
    saved: u64,
    /// Serial on top when the file was last written successfully.
    open_group: bool,
    last_edit: Instant,
    retained: usize,
    budget: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_BUDGET)
    }
}

impl History {
    pub fn new(budget: usize) -> Self {
        History {
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            next_serial: 1,
            applied: 0,
            saved: 0,
            open_group: false,
            last_edit: Instant::now(),
            retained: 0,
            budget,
        }
    }

    /// True when the document differs from its last saved state.
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.applied != self.saved
    }

    /// Mark the current state as saved.
    ///
    /// Closes the open group first, so post-save typing starts a fresh undo step
    /// and undoing it lands exactly on the saved state.
    pub fn mark_saved(&mut self) {
        self.open_group = false;
        self.saved = self.applied;
    }

    /// Record that the state at `serial` is what was written to disk.
    ///
    /// This is *not* the same as [`Self::mark_saved`] when the user kept typing
    /// during the write: the file holds the state at `serial`, so the saved marker
    /// moves there and the later edits correctly remain unsaved. Undoing back to
    /// `serial` clears the indicator, which is right — the file really does
    /// contain that text.
    pub fn mark_saved_at(&mut self, serial: u64) {
        self.open_group = false;
        self.saved = serial;
    }

    /// Serial of the topmost applied edit; 0 means "pristine".
    pub fn applied_serial(&self) -> u64 {
        self.applied
    }

    /// Force the document to be considered dirty (e.g. a failed save attempt
    /// after edits).
    pub fn mark_dirty(&mut self) {
        if self.saved == self.applied {
            // Move the saved marker off the current state.
            self.saved = u64::MAX;
        }
    }

    /// Treat the current contents as the pristine, saved state and drop all
    /// history. Used when a document is first loaded.
    pub fn reset(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.retained = 0;
        self.applied = 0;
        self.saved = 0;
        self.open_group = false;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// Bytes currently retained by history.
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }

    /// Record an edit that has already been applied to the buffer.
    ///
    /// The caller has already performed the mutation; this only maintains the
    /// history, so no `Buffer` is needed here.
    pub fn record(&mut self, mut edit: Edit) {
        debug_assert!(edit.removed_len > 0 || edit.inserted_len > 0);
        let now = Instant::now();
        let contiguous = now.duration_since(self.last_edit) <= GROUP_WINDOW;

        // Any new edit abandons the redo branch.
        self.drop_redo();

        edit.serial = self.next_serial;
        self.next_serial += 1;

        if self.open_group && contiguous {
            if let Some(top) = self.undo.back_mut() {
                // Never merge an edit away that carries the saved state. Merging
                // removes the older edit's serial from the stack, so if that serial
                // is the save point the saved state would become unreachable by
                // undo — and the unsaved indicator could never be cleared.
                let holds_save_point = top.serial == self.saved;
                if !holds_save_point && top.kind == edit.kind && top.absorbs(&edit) {
                    let before = top.retained();
                    top.merge(edit);
                    self.retained = self.retained + top.retained() - before;
                    self.applied = top.serial;
                    self.last_edit = now;
                    self.enforce_budget();
                    return;
                }
            }
        }

        self.retained += edit.retained();
        self.applied = edit.serial;
        self.undo.push_back(edit);
        self.open_group = true;
        self.last_edit = now;
        self.enforce_budget();
    }

    /// Close the current undo group so the next edit starts a new step.
    pub fn break_group(&mut self) {
        self.open_group = false;
    }

    /// Undo one step, returning the selection to restore.
    pub fn undo(&mut self, buffer: &mut Buffer) -> Option<Sel> {
        let edit = self.undo.pop_back()?;
        edit.revert(buffer);
        let sel = edit.sel_before;
        self.applied = self.undo.back().map(|e| e.serial).unwrap_or(0);
        self.redo.push_back(edit);
        self.open_group = false;
        Some(sel)
    }

    /// Redo one step, returning the selection to restore.
    pub fn redo(&mut self, buffer: &mut Buffer) -> Option<Sel> {
        let edit = self.redo.pop_back()?;
        edit.reapply(buffer);
        let sel = edit.sel_after;
        self.applied = edit.serial;
        self.undo.push_back(edit);
        self.open_group = false;
        Some(sel)
    }

    fn drop_redo(&mut self) {
        for e in self.redo.drain(..) {
            self.retained -= e.retained();
        }
    }

    /// Evict the oldest undo steps until history fits its budget.
    fn enforce_budget(&mut self) {
        while self.retained > self.budget && !self.undo.is_empty() {
            let e = self.undo.pop_front().expect("loop guard checks emptiness");
            self.retained -= e.retained();
            // If the evicted step was the save point, the saved marker can no
            // longer be reached; the document stays dirty until the next save.
        }
    }
}

/// Build an [`Edit`] from a completed mutation.
#[allow(clippy::too_many_arguments)]
pub fn make_edit(
    pos: usize,
    removed: Vec<crate::buffer::Piece>,
    removed_len: usize,
    inserted: Vec<crate::buffer::Piece>,
    inserted_len: usize,
    sel_before: Sel,
    sel_after: Sel,
    kind: EditKind,
) -> Edit {
    Edit {
        pos,
        removed,
        removed_len,
        inserted,
        inserted_len,
        sel_before,
        sel_after,
        kind,
        serial: 0,
    }
}
