//! A desktop text editor with a hand-written document buffer.
//!
//! The crate is split so that everything except [`ui`] and [`app`] is free of GUI
//! dependencies and directly testable:
//!
//! * [`buffer`] — the piece table: chunk store, implicit treap, line index,
//!   snapshots.
//! * [`undo`] — piece-level undo records and saved-state tracking.
//! * [`document`] — per-tab state and the editing operations.
//! * [`motion`] — cursor motions and word boundaries.
//! * [`search`] — literal search and replace-all planning.
//! * [`fileio`] — validated loading and atomic saving.
//! * [`progress`] — cross-thread progress and cancellation.
//! * [`ui`] — the custom text view, tab strip, find bar and dialogs.
//! * [`app`] — frame loop and document lifecycle.

pub mod app;
pub mod buffer;
pub mod document;
pub mod fileio;
pub mod motion;
pub mod progress;
pub mod search;
pub mod ui;
pub mod undo;

pub use app::NotepadApp;
