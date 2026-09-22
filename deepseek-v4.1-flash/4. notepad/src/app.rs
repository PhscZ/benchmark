//! Application state and the frame loop.
//!
//! # Frame order
//!
//! ```text
//!   1. veto a close request that has unsaved documents behind it
//!   2. drain background job results
//!   3. global shortcuts (consumed, so the editor never sees them)
//!   4. menu bar / toolbar / tab strip / find bar / status bar
//!   5. document view   <- the only place document text is drawn
//!   6. modals
//! ```
//!
//! # Threading contract
//!
//! The UI thread never performs file I/O and never scans a document. Everything
//! that could outlast a frame — load, save, search, replace-all planning — goes to
//! a worker holding a [`crate::buffer::Snapshot`]. A snapshot costs `O(#pieces)`
//! pointer copies, which is what makes it affordable to take one on a 100 MiB
//! document for every Ctrl+S.
//!
//! Results arrive over a channel and are applied at the top of the next frame, so
//! no document is ever mutated from a worker thread.

use crate::buffer::Buffer;
use crate::document::{detect_eol, DocId, Document, Eol, TextState};
use crate::fileio::{self, FileError};
use crate::progress::{Op, OpKind};
use crate::search::{self, Query, SearchState};
use crate::ui::dialogs::{self, Confirm, Decision};
use crate::ui::jobs::{JobMsg, Jobs, MAX_MATCHES};
use crate::ui::searchbar::{self, SearchAction};
use crate::ui::tabs::{self, TabAction};
use crate::ui::textview::{self, ViewState};
use crate::ui::{Metrics, Palette};
use crate::undo::{EditKind, Sel};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Editor font size in points at 100% zoom.
const BASE_FONT_SIZE: f32 = 14.0;
/// Tab stop width, in character cells.
const TAB_COLUMNS: f32 = 4.0;
/// How long a transient notice stays on screen.
const NOTICE_TTL: Duration = Duration::from_secs(5);
/// Bytes sampled from the head of a file to decide its line-ending style.
const EOL_SAMPLE: usize = 1 << 20;

/// What to do once a save that was triggered by a close/exit prompt completes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AfterSave {
    None,
    CloseTab(usize),
    Exit,
}

pub struct NotepadApp {
    docs: Vec<Document>,
    active: usize,
    next_id: DocId,
    views: HashMap<DocId, ViewState>,
    jobs: Jobs,
    metrics: Option<Metrics>,
    palette: Palette,
    zoom: f32,
    /// A blocking file-error report.
    error: Option<String>,
    /// A pending unsaved-changes decision.
    confirm: Option<Confirm>,
    /// Follow-up action for a save started by the confirmation dialog.
    after_save: AfterSave,
    /// Document that should take keyboard focus on the next frame.
    focus_request: Option<DocId>,
    /// The find bar's query field should take focus on the next frame.
    focus_search: bool,
    /// True while the find bar owns the keyboard.
    search_focused: bool,
    /// Whether the shortcut cheat-sheet is open.
    help_open: bool,
    /// A close was approved and must be replayed to the viewport.
    exit_requested: bool,
    clipboard: Option<arboard::Clipboard>,
}

impl NotepadApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        cc.egui_ctx.all_styles_mut(|s| {
            s.visuals.panel_fill = egui::Color32::from_rgb(0x25, 0x26, 0x2a);
            s.visuals.window_fill = egui::Color32::from_rgb(0x25, 0x26, 0x2a);
            s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        });

        let mut app = NotepadApp {
            docs: Vec::new(),
            active: 0,
            next_id: 1,
            views: HashMap::new(),
            jobs: Jobs::new(),
            metrics: None,
            palette: Palette::default(),
            zoom: 1.0,
            error: None,
            confirm: None,
            after_save: AfterSave::None,
            focus_request: None,
            focus_search: false,
            search_focused: false,
            help_open: false,
            exit_requested: false,
            clipboard: arboard::Clipboard::new().ok(),
        };

        match initial {
            Some(path) => {
                app.open_path(path);
            }
            None => {
                app.new_document();
            }
        }
        app
    }

    // ---- document lifecycle ---------------------------------------------

    fn new_document(&mut self) -> DocId {
        let id = self.next_id;
        self.next_id += 1;
        self.docs
            .push(Document::new(id, Buffer::empty(), None, Eol::Lf, false));
        self.active = self.docs.len() - 1;
        self.views.insert(id, ViewState::default());
        self.focus_request = Some(id);
        id
    }

    /// Start loading `path` on a worker thread.
    fn open_path(&mut self, path: PathBuf) {
        // Focus an existing tab instead of opening the same file twice.
        let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if let Some(i) = self.docs.iter().position(|d| {
            d.path
                .as_ref()
                .is_some_and(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == canonical)
        }) {
            self.active = i;
            self.focus_request = Some(self.docs[i].id);
            return;
        }

        let id = self.next_id;
        self.next_id += 1;

        // The tab appears immediately in a locked "loading" state, so the user can
        // see what is happening and cannot type into a document that is not there
        // yet.
        let mut doc = Document::new(id, Buffer::empty(), Some(path.clone()), Eol::Lf, false);
        let op = Op::new(OpKind::Load);
        op.progress.set_phase("opening");
        doc.op = Some(op);
        self.docs.push(doc);
        self.active = self.docs.len() - 1;
        self.views.insert(id, ViewState::default());

        let progress = self.docs[self.active]
            .op
            .as_ref()
            .expect("op was just set")
            .progress
            .clone();
        self.jobs.spawn_load(id, path, progress);
    }

    fn active_index(&self) -> Option<usize> {
        if self.docs.is_empty() {
            None
        } else {
            Some(self.active.min(self.docs.len() - 1))
        }
    }

    fn index_of(&self, id: DocId) -> Option<usize> {
        self.docs.iter().position(|d| d.id == id)
    }

    /// Close the tab at `index` without further prompting.
    fn close_tab(&mut self, index: usize) {
        if index >= self.docs.len() {
            return;
        }
        let id = self.docs[index].id;
        self.views.remove(&id);
        self.docs.remove(index);
        if self.docs.is_empty() {
            self.active = 0;
        } else if self.active >= self.docs.len() {
            self.active = self.docs.len() - 1;
        } else if self.active > index {
            self.active -= 1;
        }
        if let Some(d) = self.docs.get(self.active) {
            self.focus_request = Some(d.id);
        }
    }

    /// Close a tab, prompting first when it has unsaved changes.
    fn request_close(&mut self, index: usize) {
        match self.docs.get(index) {
            None => {}
            Some(d) if d.is_dirty() => self.confirm = Some(Confirm::CloseTab(index)),
            Some(_) => self.close_tab(index),
        }
    }

    // ---- saving ---------------------------------------------------------

    /// Save the active document, falling back to Save As when it has no path.
    fn save_active(&mut self) {
        let Some(i) = self.active_index() else { return };
        if self.docs[i].editing_locked() {
            self.docs[i].set_notice("A background operation is already running here", true);
            return;
        }
        match self.docs[i].path.clone() {
            Some(path) => self.begin_save(i, path),
            None => self.save_as_active(),
        }
    }

    fn save_as_active(&mut self) {
        let Some(i) = self.active_index() else { return };
        if self.docs[i].editing_locked() {
            self.docs[i].set_notice("A background operation is already running here", true);
            return;
        }
        let suggested = self.docs[i].display_name();
        let picked = rfd::FileDialog::new()
            .set_title("Save As")
            .set_file_name(suggested)
            .add_filter("Text files", &["txt", "md", "log", "csv", "json"])
            .add_filter("All files", &["*"])
            .save_file();
        let Some(path) = picked else {
            // The user backed out; do not leave a close/exit hanging on it.
            self.after_save = AfterSave::None;
            return;
        };
        self.docs[i].path = Some(path.clone());
        self.begin_save(i, path);
    }

    /// Snapshot the document and hand the write to a worker.
    ///
    /// The snapshot is what makes "editing stays enabled during a save" safe: the
    /// worker writes a consistent document, and anything typed after this point
    /// leaves the tab dirty.
    fn begin_save(&mut self, index: usize, path: PathBuf) {
        let doc = &mut self.docs[index];
        // `snapshot_for_save` breaks the undo group, so the captured serial is a
        // real undo step that later typing cannot merge away. That is what lets
        // "undo back to the saved state" clear the unsaved indicator even when the
        // user keeps typing during the write.
        let (snapshot, serial) = doc.snapshot_for_save();
        let had_bom = doc.had_bom;

        let op = Op::new(OpKind::Save);
        op.progress.set_phase("preparing");
        doc.op = Some(op);
        let progress = doc.op.as_ref().expect("just set").progress.clone();
        let id = doc.id;

        self.jobs
            .spawn_save(id, path, snapshot, had_bom, serial, progress);
    }

    // ---- search ---------------------------------------------------------

    /// Start a search of the active document against a fresh snapshot.
    fn start_search(&mut self) {
        let Some(i) = self.active_index() else { return };
        let doc = &mut self.docs[i];

        // Any in-flight scan is superseded; other operation kinds are not.
        if let Some(op) = doc.op.take() {
            if op.kind == OpKind::Search {
                op.progress.cancel();
            } else {
                doc.op = Some(op);
                return;
            }
        }
        doc.search.invalidate();

        let Some(query) = Query::new(&doc.search.query, doc.search.case_sensitive) else {
            // Empty query: no scan, no matches, and explicitly not an error.
            doc.search.complete = true;
            return;
        };

        let snapshot = doc.snapshot();
        let epoch = doc.search_epoch;
        let op = Op::new(OpKind::Search);
        op.progress.set_phase("scanning");
        doc.op = Some(op);
        let progress = doc.op.as_ref().expect("just set").progress.clone();
        let id = doc.id;

        self.jobs.spawn_search(id, snapshot, query, epoch, progress);
    }

    /// Move to the next or previous match, wrapping around.
    fn step_match(&mut self, forward: bool) {
        let Some(i) = self.active_index() else { return };
        let doc = &mut self.docs[i];
        if doc.search.matches.is_empty() {
            return;
        }
        let caret = doc.text.sel.caret;
        let idx = match doc.search.current {
            Some(cur) if forward => {
                if cur + 1 < doc.search.matches.len() {
                    cur + 1
                } else {
                    0 // wraparound
                }
            }
            Some(cur) => {
                if cur > 0 {
                    cur - 1
                } else {
                    doc.search.matches.len() - 1 // wraparound
                }
            }
            None => search::current_for(&doc.search.matches, caret).unwrap_or(0),
        };
        doc.search.current = Some(idx);
        let m = doc.search.matches[idx];
        doc.text.sel = Sel {
            anchor: m.start,
            caret: m.end,
        };
        doc.text.scroll_to_caret = true;
        doc.text.history.break_group();
    }

    /// Replace the current match with the replacement text, as one undo step.
    fn replace_current(&mut self) {
        let Some(i) = self.active_index() else { return };
        if !self.docs[i].accepts_edits() {
            self.docs[i].set_notice("Editing is paused while a background operation finishes", true);
            return;
        }
        let doc = &mut self.docs[i];
        let Some(cur) = doc.search.current else { return };
        let Some(m) = doc.search.matches.get(cur).copied() else {
            return;
        };
        let replacement = doc.search.replacement.clone();

        // Select the match, then type over it: one undoable action.
        doc.text.sel = Sel {
            anchor: m.start,
            caret: m.end,
        };
        if doc.insert_str(&replacement, EditKind::Replace).is_err() {
            doc.set_notice("Document size limit reached; no replacement made", true);
            return;
        }
        // The text changed, so the cached matches are stale until the next scan.
        doc.search.invalidate();
        self.start_search();
    }

    fn replace_all(&mut self) {
        let Some(i) = self.active_index() else { return };
        if !self.docs[i].accepts_edits() {
            self.docs[i].set_notice("Editing is paused while a background operation finishes", true);
            return;
        }
        let doc = &mut self.docs[i];
        // Refuse rather than delete the document: an empty pattern matches
        // everywhere, so replacing it would insert the replacement between every
        // character. This is the safe behaviour the brief asks for.
        if doc.search.query.is_empty() {
            doc.set_notice("Nothing to replace: the search box is empty", true);
            return;
        }
        if doc.search.matches.is_empty() {
            doc.set_notice("No matches to replace", false);
            return;
        }

        let snapshot = doc.snapshot();
        let matches = doc.search.matches.clone();
        let replacement = doc.search.replacement.clone();
        let epoch = doc.search_epoch;

        let op = Op::new(OpKind::ReplaceAll);
        op.progress.set_phase("planning");
        doc.op = Some(op);
        let progress = doc.op.as_ref().expect("just set").progress.clone();
        let id = doc.id;

        self.jobs
            .spawn_replace_plan(id, snapshot, matches, replacement, epoch, progress);
    }

    // ---- job results ----------------------------------------------------

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        for msg in self.jobs.poll() {
            match msg {
                JobMsg::Loaded { doc, path, result } => self.on_loaded(doc, path, result),
                JobMsg::Saved {
                    doc,
                    path,
                    result,
                    serial,
                } => self.on_saved(doc, path, result, serial),
                JobMsg::Searched {
                    doc,
                    epoch,
                    matches,
                    truncated,
                    completed,
                } => self.on_searched(doc, epoch, matches, truncated, completed),
                JobMsg::ReplacePlanned {
                    doc,
                    epoch,
                    plan,
                    count,
                } => self.on_replace_planned(doc, epoch, plan, count),
            }
        }

        // Keep animating while any document has work in flight.
        if self.docs.iter().any(|d| d.op.is_some()) {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }

    fn on_loaded(&mut self, doc: DocId, path: PathBuf, result: Result<fileio::Loaded, FileError>) {
        let Some(i) = self.index_of(doc) else { return };
        let d = &mut self.docs[i];
        d.op = None;
        match result {
            Ok(loaded) => {
                let head = loaded.buffer.read_vec(0, loaded.buffer.len().min(EOL_SAMPLE));
                let eol = detect_eol(&head);
                d.text = TextState::new(loaded.buffer);
                d.eol = eol;
                d.had_bom = loaded.had_bom;
                d.path = Some(path.clone());
                d.text.history.reset();
                d.search = SearchState::default();
                d.search_epoch += 1;
                d.set_notice(
                    format!(
                        "Loaded {} ({:.1} MiB)",
                        file_name_of(&path),
                        loaded.size as f64 / (1024.0 * 1024.0)
                    ),
                    false,
                );
                // Only claim the keyboard if this is the tab on screen; otherwise
                // the request would keep bouncing to a background document and the
                // visible one would never receive focus.
                if self.active == i {
                    self.focus_request = Some(doc);
                }
            }
            Err(FileError::Cancelled) => {
                d.set_notice("Load cancelled", false);
            }
            Err(e) => {
                // The tab stays open and empty so the user can retry or close it.
                self.error = Some(e.to_string());
                d.set_notice(e.to_string(), true);
            }
        }
    }

    fn on_saved(
        &mut self,
        doc: DocId,
        path: PathBuf,
        result: Result<u64, FileError>,
        serial: u64,
    ) {
        let Some(i) = self.index_of(doc) else { return };
        let d = &mut self.docs[i];
        d.op = None;
        match result {
            Ok(bytes) => {
                d.path = Some(path.clone());
                // The file holds the state at `serial`; edits made during the
                // write stay unsaved, and undoing back to `serial` clears the
                // indicator because the file really does hold that text.
                d.mark_saved_at(serial);
                d.set_notice(
                    format!(
                        "Saved {} ({:.1} MiB)",
                        file_name_of(&path),
                        bytes as f64 / (1024.0 * 1024.0)
                    ),
                    false,
                );
            }
            Err(FileError::Cancelled) => {
                d.set_notice("Save cancelled — the original file is unchanged", true);
                self.after_save = AfterSave::None;
                return;
            }
            Err(e) => {
                // Report without touching the document.
                self.error = Some(e.to_string());
                d.set_notice(format!("Save failed: {e}"), true);
                self.after_save = AfterSave::None;
                return;
            }
        }

        // A save that was part of a close/exit prompt now continues.
        match std::mem::replace(&mut self.after_save, AfterSave::None) {
            AfterSave::None => {}
            AfterSave::CloseTab(idx) => self.close_tab(idx),
            AfterSave::Exit => {
                // The active document is clean now; ask about the next dirty one,
                // if any.
                match self.docs.iter().position(|d| d.is_dirty()) {
                    Some(next) => {
                        self.active = next;
                        self.confirm = Some(Confirm::Exit);
                    }
                    None => self.exit_requested = true,
                }
            }
        }
    }

    fn on_searched(
        &mut self,
        doc: DocId,
        epoch: u64,
        matches: Vec<search::Match>,
        truncated: bool,
        completed: bool,
    ) {
        let Some(i) = self.index_of(doc) else { return };
        let d = &mut self.docs[i];
        // Discard results computed from text that no longer exists.
        if epoch != d.search_epoch {
            return;
        }
        if let Some(op) = d.op.take() {
            if op.kind != OpKind::Search {
                d.op = Some(op);
            }
        }
        d.search.matches = matches;
        d.search.complete = completed;
        d.search.stale = !completed;
        d.search.current = search::current_for(&d.search.matches, d.text.sel.caret);
        if truncated {
            d.set_notice(
                format!("Search stopped after {MAX_MATCHES} matches; refine the query"),
                false,
            );
        }
    }

    fn on_replace_planned(
        &mut self,
        doc: DocId,
        epoch: u64,
        plan: Option<Vec<search::Segment>>,
        count: usize,
    ) {
        let Some(i) = self.index_of(doc) else { return };
        let d = &mut self.docs[i];
        if epoch != d.search_epoch {
            if let Some(op) = d.op.take() {
                if op.kind != OpKind::ReplaceAll {
                    d.op = Some(op);
                }
            }
            return;
        }
        d.op = None;
        match plan {
            // A plan with no segments would empty the document. That can only
            // come from a bug, so refuse it rather than destroy the user's text.
            Some(segments) if segments.is_empty() && !d.is_empty() => {
                d.set_notice("Replace-all produced an empty result; no changes made", true);
            }
            Some(segments) => {
                d.apply_replace_all(segments, count);
                // The cached matches describe the pre-replacement text.
                self.start_search();
            }
            None => d.set_notice("Replace cancelled — no changes made", false),
        }
    }

    // ---- shortcuts ------------------------------------------------------

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::Key;

        // Ctrl+Tab / Ctrl+Shift+Tab cycle tabs.
        if take_key(ctx, Key::Tab, true, false) && !self.docs.is_empty() {
            self.active = (self.active + 1) % self.docs.len();
            self.focus_request = Some(self.docs[self.active].id);
        }
        if take_key(ctx, Key::Tab, true, true) && !self.docs.is_empty() {
            self.active = (self.active + self.docs.len() - 1) % self.docs.len();
            self.focus_request = Some(self.docs[self.active].id);
        }

        if take_key(ctx, Key::N, true, false) {
            self.new_document();
        }
        if take_key(ctx, Key::O, true, false) {
            if let Some(p) = rfd::FileDialog::new()
                .set_title("Open file")
                .add_filter("Text files", &["txt", "md", "log", "csv", "json"])
                .add_filter("All files", &["*"])
                .pick_file()
            {
                self.open_path(p);
            }
        }
        if take_key(ctx, Key::S, true, true) {
            self.save_as_active();
        } else if take_key(ctx, Key::S, true, false) {
            self.save_active();
        }
        if take_key(ctx, Key::W, true, false) {
            if let Some(i) = self.active_index() {
                self.request_close(i);
            }
        }
        if take_key(ctx, Key::F, true, false) {
            self.open_find(false);
        }
        if take_key(ctx, Key::H, true, false) {
            self.open_find(true);
        }
        if take_key(ctx, Key::F3, false, false) {
            self.step_match(true);
        }
        if take_key(ctx, Key::F3, false, true) {
            self.step_match(false);
        }
    }

    fn open_find(&mut self, replace: bool) {
        if let Some(i) = self.active_index() {
            self.docs[i].search.open = true;
            self.docs[i].search.replace_open = replace;
            self.focus_search = true;
        }
    }

    // ---- exit handling --------------------------------------------------

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        let requested = ctx.input(|i| i.viewport().close_requested());
        if !requested || self.confirm.is_some() {
            return;
        }
        if let Some(i) = self.docs.iter().position(|d| d.is_dirty()) {
            // Veto the close, ask, and replay it only if the user agrees.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.active = i;
            self.confirm = Some(Confirm::Exit);
        }
    }

    // ---- clipboard ------------------------------------------------------

    fn copy_to_clipboard(&mut self, ctx: &egui::Context, text: String) {
        ctx.copy_text(text);
    }

    fn paste_from_clipboard(&mut self, index: usize) {
        let Some(text) = self
            .clipboard
            .as_mut()
            .and_then(|c| c.get_text().ok())
            .filter(|t| !t.is_empty())
        else {
            self.docs[index].set_notice("Clipboard is empty or unavailable", true);
            return;
        };
        if !self.docs[index].accepts_edits() {
            self.docs[index]
                .set_notice("Editing is paused while a background operation finishes", true);
            return;
        }
        if self.docs[index]
            .insert_str(&text, EditKind::Paste)
            .is_err()
        {
            self.docs[index].set_notice("Document size limit reached; paste not applied", true);
        }
    }

    // ---- bars -----------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New            Ctrl+N").clicked() {
                    self.new_document();
                    ui.close();
                }
                if ui.button("Open…          Ctrl+O").clicked() {
                    ui.close();
                    if let Some(p) = rfd::FileDialog::new().set_title("Open file").pick_file() {
                        self.open_path(p);
                    }
                }
                ui.separator();
                let has_doc = self.active_index().is_some();
                if ui
                    .add_enabled(has_doc, egui::Button::new("Save           Ctrl+S"))
                    .clicked()
                {
                    self.save_active();
                    ui.close();
                }
                if ui
                    .add_enabled(has_doc, egui::Button::new("Save As…       Ctrl+Shift+S"))
                    .clicked()
                {
                    self.save_as_active();
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(has_doc, egui::Button::new("Close tab      Ctrl+W"))
                    .clicked()
                {
                    if let Some(i) = self.active_index() {
                        self.request_close(i);
                    }
                    ui.close();
                }
                if ui.button("Exit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            ui.menu_button("Edit", |ui| {
                let (can_undo, can_redo) = match self.active_index() {
                    Some(i) => (
                        self.docs[i].text.history.can_undo(),
                        self.docs[i].text.history.can_redo(),
                    ),
                    None => (false, false),
                };
                if ui
                    .add_enabled(can_undo, egui::Button::new("Undo           Ctrl+Z"))
                    .clicked()
                {
                    if let Some(i) = self.active_index() {
                        self.docs[i].undo();
                    }
                    ui.close();
                }
                if ui
                    .add_enabled(can_redo, egui::Button::new("Redo           Ctrl+Y"))
                    .clicked()
                {
                    if let Some(i) = self.active_index() {
                        self.docs[i].redo();
                    }
                    ui.close();
                }
                ui.separator();
                if ui.button("Select All     Ctrl+A").clicked() {
                    if let Some(i) = self.active_index() {
                        self.docs[i].select_all();
                    }
                    ui.close();
                }
                if ui.button("Copy           Ctrl+C").clicked() {
                    let text = self
                        .active_index()
                        .and_then(|i| self.docs[i].selected_text());
                    if let Some(t) = text {
                        ui.ctx().copy_text(t);
                    }
                    ui.close();
                }
                if ui.button("Cut            Ctrl+X").clicked() {
                    let text = self.active_index().and_then(|i| self.docs[i].cut());
                    if let Some(t) = text {
                        ui.ctx().copy_text(t);
                    }
                    ui.close();
                }
                if ui.button("Paste          Ctrl+V").clicked() {
                    if let Some(i) = self.active_index() {
                        self.paste_from_clipboard(i);
                    }
                    ui.close();
                }
            });

            ui.menu_button("Search", |ui| {
                let has_doc = self.active_index().is_some();
                if ui
                    .add_enabled(has_doc, egui::Button::new("Find…          Ctrl+F"))
                    .clicked()
                {
                    self.open_find(false);
                    ui.close();
                }
                if ui
                    .add_enabled(has_doc, egui::Button::new("Replace…       Ctrl+H"))
                    .clicked()
                {
                    self.open_find(true);
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(has_doc, egui::Button::new("Find next      F3"))
                    .clicked()
                {
                    self.step_match(true);
                    ui.close();
                }
                if ui
                    .add_enabled(has_doc, egui::Button::new("Find previous  Shift+F3"))
                    .clicked()
                {
                    self.step_match(false);
                    ui.close();
                }
            });

            ui.menu_button("View", |ui| {
                if ui.button("Zoom in").clicked() {
                    self.set_zoom(self.zoom * 1.15);
                }
                if ui.button("Zoom out").clicked() {
                    self.set_zoom(self.zoom / 1.15);
                }
                if ui.button("Reset zoom (100%)").clicked() {
                    self.set_zoom(1.0);
                }
            });

            ui.menu_button("Help", |ui| {
                if ui.button("Keyboard shortcuts").clicked() {
                    self.help_open = true;
                    ui.close();
                }
            });
        });
    }

    fn set_zoom(&mut self, z: f32) {
        self.zoom = z.clamp(0.5, 4.0);
        self.metrics = None; // rebuilt at the new size next frame
    }

    /// A row of plain buttons so no main action is menu-only.
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("New").on_hover_text("Ctrl+N").clicked() {
                self.new_document();
            }
            if ui.button("Open").on_hover_text("Ctrl+O").clicked() {
                if let Some(p) = rfd::FileDialog::new().set_title("Open file").pick_file() {
                    self.open_path(p);
                }
            }
            let has_doc = self.active_index().is_some();
            if ui
                .add_enabled(has_doc, egui::Button::new("Save"))
                .on_hover_text("Ctrl+S")
                .clicked()
            {
                self.save_active();
            }
            if ui
                .add_enabled(has_doc, egui::Button::new("Save As"))
                .on_hover_text("Ctrl+Shift+S")
                .clicked()
            {
                self.save_as_active();
            }
            ui.separator();
            let (can_undo, can_redo) = match self.active_index() {
                Some(i) => (
                    self.docs[i].text.history.can_undo(),
                    self.docs[i].text.history.can_redo(),
                ),
                None => (false, false),
            };
            if ui
                .add_enabled(can_undo, egui::Button::new("Undo"))
                .on_hover_text("Ctrl+Z")
                .clicked()
            {
                if let Some(i) = self.active_index() {
                    self.docs[i].undo();
                }
            }
            if ui
                .add_enabled(can_redo, egui::Button::new("Redo"))
                .on_hover_text("Ctrl+Y")
                .clicked()
            {
                if let Some(i) = self.active_index() {
                    self.docs[i].redo();
                }
            }
            ui.separator();
            if ui
                .add_enabled(has_doc, egui::Button::new("Find"))
                .on_hover_text("Ctrl+F")
                .clicked()
            {
                self.open_find(false);
            }
            if ui
                .add_enabled(has_doc, egui::Button::new("Replace"))
                .on_hover_text("Ctrl+H")
                .clicked()
            {
                self.open_find(true);
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let Some(i) = self.active_index() else {
                ui.label("No document");
                return;
            };
            let (line, col) = self.docs[i].cursor_line_col();
            let sel = self.docs[i].text.sel.len();
            let total = self.docs[i].line_count();
            let dirty = self.docs[i].is_dirty();
            let eol = self.docs[i].eol;
            let bom = self.docs[i].had_bom;
            let len = self.docs[i].len();
            let pieces = self.docs[i].text.buffer.piece_count();

            ui.label(format!("Ln {line}, Col {col}"));
            ui.separator();
            ui.label(format!("{total} lines"));
            ui.separator();
            ui.label("UTF-8");
            if bom {
                ui.label("BOM")
                    .on_hover_text("File begins with a UTF-8 byte order mark, preserved on save");
            }
            ui.separator();
            ui.label(eol.label())
                .on_hover_text("Line-ending style used for new lines");
            ui.separator();
            if sel > 0 {
                ui.label(format!("{sel} B selected"));
                ui.separator();
            }
            if dirty {
                ui.colored_label(ui.visuals().warn_fg_color, "Unsaved changes *");
            } else {
                ui.label("Saved");
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(n) = &self.docs[i].notice {
                    if n.at.elapsed() < NOTICE_TTL {
                        if n.error {
                            ui.colored_label(ui.visuals().error_fg_color, &n.text);
                        } else {
                            ui.weak(&n.text);
                        }
                        ui.separator();
                    }
                }

                ui.label(format!("{pieces} pieces"))
                    .on_hover_text("Piece-table fragments backing this document");
                ui.separator();
                ui.label(human_bytes(len));

                if let Some(op) = &self.docs[i].op {
                    ui.separator();
                    ui.label(op.status_text());
                    match op.progress.fraction() {
                        Some(f) => {
                            ui.add(
                                egui::ProgressBar::new(f)
                                    .desired_width(140.0)
                                    .desired_height(10.0),
                            );
                        }
                        None => {
                            ui.spinner();
                        }
                    }
                    if ui.small_button("Cancel").clicked() {
                        op.progress.cancel();
                    }
                }
            });
        });
    }

    // ---- central area ---------------------------------------------------

    fn editor_area(&mut self, ui: &mut egui::Ui, metrics: &Metrics) {
        let Some(i) = self.active_index() else {
            ui.centered_and_justified(|ui| {
                ui.label("No document open.\n\nCtrl+N for a new file, Ctrl+O to open one.");
            });
            return;
        };

        let doc_id = self.docs[i].id;
        let mut view = self.views.remove(&doc_id).unwrap_or_default();
        let search_focused = self.search_focused;
        let palette = self.palette.clone();

        let resp = {
            let doc = &mut self.docs[i];
            textview::show(ui, doc, &mut view, metrics, &palette, search_focused)
        };
        self.views.insert(doc_id, view);

        if let Some(text) = resp.copied {
            self.copy_to_clipboard(ui.ctx(), text);
        }
        if resp.paste_requested {
            self.paste_from_clipboard(i);
        }
        if resp.edit_blocked {
            self.docs[i].set_notice(
                "Editing is paused while a background operation finishes; input was not applied",
                true,
            );
        }

        if let Some(id) = self.focus_request.take() {
            if id == doc_id {
                ui.ctx()
                    .memory_mut(|m| m.request_focus(textview::focus_id(doc_id)));
            } else {
                self.focus_request = Some(id);
            }
        }
    }

    /// Paste delivered as an `Event::Paste`, which is how the winit integration
    /// hands over Ctrl+V. Applied only when the document view owns the keyboard,
    /// so pasting into the search box does not also hit the document.
    fn handle_paste_event(&mut self, ctx: &egui::Context) {
        let Some(i) = self.active_index() else { return };
        let doc_id = self.docs[i].id;
        let owns_keyboard = ctx.memory(|m| m.focused()) == Some(textview::focus_id(doc_id));
        if !owns_keyboard || self.search_focused {
            return;
        }
        let pasted = ctx.input_mut(|inp| {
            let mut found = None;
            inp.events.retain(|e| match e {
                egui::Event::Paste(t) => {
                    found = Some(t.clone());
                    false
                }
                _ => true,
            });
            found
        });
        let Some(text) = pasted else { return };
        if !self.docs[i].accepts_edits() {
            self.docs[i].set_notice(
                "Editing is paused while a background operation finishes; paste was not applied",
                true,
            );
            return;
        }
        if self.docs[i].insert_str(&text, EditKind::Paste).is_err() {
            self.docs[i].set_notice("Document size limit reached; paste not applied", true);
        }
    }

    // ---- modals ---------------------------------------------------------

    fn show_modals(&mut self, ctx: &egui::Context) {
        if self.help_open {
            egui::Modal::new(egui::Id::new("help-modal")).show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.heading("Keyboard shortcuts");
                ui.add_space(8.0);
                egui::Grid::new("shortcut-grid")
                    .num_columns(2)
                    .spacing([24.0, 4.0])
                    .show(ui, |ui| {
                        for (k, v) in SHORTCUTS {
                            ui.label(egui::RichText::new(*k).monospace());
                            ui.label(*v);
                            ui.end_row();
                        }
                    });
                ui.add_space(10.0);
                if ui.button("Close").clicked() {
                    self.help_open = false;
                }
            });
            return;
        }

        if let Some(msg) = self.error.clone() {
            if dialogs::error_modal(ctx, &msg) {
                self.error = None;
            }
            return;
        }

        let Some(confirm) = self.confirm else { return };
        let name = match confirm {
            Confirm::CloseTab(i) => self
                .docs
                .get(i)
                .map(|d| d.display_name())
                .unwrap_or_else(|| "document".to_string()),
            Confirm::Exit => self
                .docs
                .iter()
                .find(|d| d.is_dirty())
                .map(|d| d.display_name())
                .unwrap_or_else(|| "document".to_string()),
        };

        match dialogs::confirm_modal(ctx, confirm, &name) {
            Some(Decision::Save) => {
                self.confirm = None;
                match confirm {
                    Confirm::CloseTab(i) => {
                        self.active = i;
                        self.after_save = AfterSave::CloseTab(i);
                        if self.docs[i].path.is_some() {
                            let path = self.docs[i].path.clone().expect("checked");
                            self.begin_save(i, path);
                        } else {
                            self.save_as_active();
                        }
                    }
                    Confirm::Exit => {
                        self.after_save = AfterSave::Exit;
                        if let Some(i) = self.docs.iter().position(|d| d.is_dirty()) {
                            self.active = i;
                            if self.docs[i].path.is_some() {
                                let path = self.docs[i].path.clone().expect("checked");
                                self.begin_save(i, path);
                            } else {
                                self.save_as_active();
                            }
                        }
                    }
                }
            }
            Some(Decision::Discard) => {
                self.confirm = None;
                match confirm {
                    Confirm::CloseTab(i) => self.close_tab(i),
                    Confirm::Exit => {
                        self.after_save = AfterSave::None;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            Some(Decision::Cancel) => {
                self.confirm = None;
                self.after_save = AfterSave::None;
            }
            None => {}
        }
    }
}

impl eframe::App for NotepadApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if self.metrics.is_none() {
            self.metrics = Some(Metrics::new(&ctx, BASE_FONT_SIZE * self.zoom, TAB_COLUMNS));
        }
        let metrics = self.metrics.clone().expect("just initialised");

        self.handle_close_request(&ctx);
        self.poll_jobs(&ctx);
        if std::mem::take(&mut self.exit_requested) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.handle_shortcuts(&ctx);
        self.handle_paste_event(&ctx);

        egui::Panel::top("menu-bar")
            .exact_size(26.0)
            .show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("toolbar")
            .exact_size(32.0)
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::top("tab-strip")
            .exact_size(28.0)
            .show(ui, |ui| match tabs::show(ui, &self.docs, self.active) {
                TabAction::None => {}
                TabAction::Activate(i) => {
                    self.active = i;
                    self.focus_request = Some(self.docs[i].id);
                }
                TabAction::Close(i) => self.request_close(i),
            });

        // The find bar belongs to a document, so it only shows for one that has
        // it open.
        let show_search = self
            .active_index()
            .map(|i| self.docs[i].search.open)
            .unwrap_or(false);
        if show_search {
            let focus = std::mem::take(&mut self.focus_search);
            let mut action = SearchAction::None;
            let mut field_focused = false;
            egui::Panel::top("find-bar").show(ui, |ui| {
                if let Some(i) = self.active_index() {
                    let (a, f) = searchbar::show(ui, &mut self.docs[i], focus);
                    action = a;
                    field_focused = f;
                }
            });
            // Only the fields owning the keyboard suppress document input.
            self.search_focused = field_focused;
            self.apply_search_action(action);
        } else {
            self.search_focused = false;
        }

        egui::Panel::bottom("status-bar")
            .exact_size(26.0)
            .show(ui, |ui| self.status_bar(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.editor_area(ui, &metrics));

        self.show_modals(&ctx);
    }
}

impl NotepadApp {
    fn apply_search_action(&mut self, action: SearchAction) {
        match action {
            SearchAction::None => {}
            SearchAction::Run => self.start_search(),
            SearchAction::Next => {
                if self.active_index().is_none() {
                    return;
                }
                if self.docs[self.active].search.matches.is_empty() {
                    self.start_search();
                }
                self.step_match(true);
            }
            SearchAction::Previous => {
                if self.active_index().is_none() {
                    return;
                }
                if self.docs[self.active].search.matches.is_empty() {
                    self.start_search();
                }
                self.step_match(false);
            }
            SearchAction::ReplaceCurrent => self.replace_current(),
            SearchAction::ReplaceAll => self.replace_all(),
            SearchAction::CancelScan => {
                if let Some(i) = self.active_index() {
                    if let Some(op) = self.docs[i].op.take() {
                        op.progress.cancel();
                        match op.kind {
                            OpKind::Search | OpKind::ReplaceAll => self.docs[i]
                                .set_notice("Operation cancelled — document unchanged", false),
                            _ => self.docs[i].op = Some(op),
                        }
                    }
                }
            }
            SearchAction::Close => {
                if let Some(i) = self.active_index() {
                    self.docs[i].search.open = false;
                    self.docs[i].search.invalidate();
                    self.focus_request = Some(self.docs[i].id);
                }
            }
        }
    }
}

/// Consume a key chord from the input queue so the editor never sees it.
///
/// Matching is explicit rather than via `consume_key`, because the editor cares
/// about the exact Ctrl/Shift combination (`Ctrl+S` vs `Ctrl+Shift+S`) and about
/// the difference between `Ctrl+Tab` and a plain `Tab` that must reach the text.
fn take_key(ctx: &egui::Context, key: egui::Key, ctrl: bool, shift: bool) -> bool {
    let mut hit = false;
    ctx.input_mut(|i| {
        i.events.retain(|e| match e {
            egui::Event::Key {
                key: k,
                pressed: true,
                modifiers,
                ..
            } if *k == key
                && (modifiers.ctrl || modifiers.command) == ctrl
                && modifiers.shift == shift =>
            {
                hit = true;
                false
            }
            _ => true,
        });
    });
    hit
}

/// File name for a path, or the whole path if it has none.
fn file_name_of(p: &std::path::Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Human-readable byte count.
fn human_bytes(n: usize) -> String {
    const KIB: f64 = 1024.0;
    let f = n as f64;
    if f < KIB {
        format!("{n} B")
    } else if f < KIB * KIB {
        format!("{:.1} KiB", f / KIB)
    } else if f < KIB * KIB * KIB {
        format!("{:.1} MiB", f / (KIB * KIB))
    } else {
        format!("{:.2} GiB", f / (KIB * KIB * KIB))
    }
}

/// Shortcut reference shown by Help.
const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+N", "New document"),
    ("Ctrl+O", "Open file"),
    ("Ctrl+S", "Save"),
    ("Ctrl+Shift+S", "Save as"),
    ("Ctrl+W", "Close tab"),
    ("Ctrl+Tab", "Next tab"),
    ("Ctrl+Shift+Tab", "Previous tab"),
    ("Ctrl+F", "Find"),
    ("Ctrl+H", "Find and replace"),
    ("F3 / Shift+F3", "Next / previous match"),
    ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
    ("Ctrl+A", "Select all"),
    ("Ctrl+C / X / V", "Copy / cut / paste"),
    ("Ctrl+← / →", "Word left / right"),
    ("Ctrl+Shift+← / →", "Select word left / right"),
    ("Home / End", "Line start / end"),
    ("Ctrl+Home / End", "Document start / end"),
    ("PgUp / PgDn", "Page up / down"),
    ("Shift+click", "Extend selection"),
    ("Double-click", "Select word"),
];
