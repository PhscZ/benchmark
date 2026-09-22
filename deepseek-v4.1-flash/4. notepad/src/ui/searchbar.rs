//! The find / replace bar.
//!
//! The two text fields here are the only `TextEdit` widgets in the program, and
//! they are deliberately *not* the document area: a single-line search box is a
//! UI control, not a document, and the brief's prohibition on ready-made editing
//! widgets is scoped to the document. The document itself is drawn and driven
//! entirely by [`crate::ui::textview`].

use crate::document::Document;
use crate::progress::OpKind;
use crate::search::SearchState;

/// What the user asked the search bar to do.
#[derive(Default)]
pub enum SearchAction {
    #[default]
    None,
    /// Re-run the search against the current text.
    Run,
    Next,
    Previous,
    ReplaceCurrent,
    ReplaceAll,
    CancelScan,
    Close,
}

/// Draw the bar.
///
/// Returns the action plus whether one of the bar's fields currently owns the
/// keyboard. The app uses that to stop the document view from also handling the
/// keystrokes: the bar being *open* is not the same as the bar having focus, and
/// conflating the two would make the editor dead while the bar is visible.
pub fn show(ui: &mut egui::Ui, doc: &mut Document, focus_query: bool) -> (SearchAction, bool) {
    let mut action = SearchAction::None;
    let mut field_focused = false;
    let scanning = doc
        .op
        .as_ref()
        .is_some_and(|o| o.kind == OpKind::Search || o.kind == OpKind::ReplaceAll);

    ui.horizontal(|ui| {
        ui.label("Find:");
        let q = ui.add(
            egui::TextEdit::singleline(&mut doc.search.query)
                .desired_width(220.0)
                .hint_text("literal text"),
        );
        if focus_query {
            q.request_focus();
        }
        if q.changed() {
            action = SearchAction::Run;
        }
        if q.has_focus() {
            field_focused = true;
        }
        if q.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            action = SearchAction::Next;
        }

        if ui
            .checkbox(&mut doc.search.case_sensitive, "Aa")
            .on_hover_text("Case sensitive")
            .changed()
        {
            action = SearchAction::Run;
        }

        if ui.button("◀").on_hover_text("Previous match (Shift+Enter)").clicked() {
            action = SearchAction::Previous;
        }
        if ui.button("▶").on_hover_text("Next match (Enter)").clicked() {
            action = SearchAction::Next;
        }

        ui.separator();
        ui.label(doc.search.count_label(scanning));

        if scanning {
            ui.spinner();
            if ui.button("Cancel").clicked() {
                action = SearchAction::CancelScan;
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("✕").on_hover_text("Close (Esc)").clicked() {
                action = SearchAction::Close;
            }
            if doc.search.replace_open {
                if ui
                    .button("All")
                    .on_hover_text("Replace every match (single undo step)")
                    .clicked()
                {
                    action = SearchAction::ReplaceAll;
                }
                if ui.button("Replace").clicked() {
                    action = SearchAction::ReplaceCurrent;
                }
                let r = ui.add(
                    egui::TextEdit::singleline(&mut doc.search.replacement)
                        .desired_width(220.0)
                        .hint_text("replace with"),
                );
                if r.has_focus() {
                    field_focused = true;
                }
                ui.label("With:");
            }
        });
    });

    // Escape closes the bar from anywhere while it is open.
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = SearchAction::Close;
    }

    (action, field_focused)
}

/// Text shown when a search cannot run, so the empty-query rule is visible rather
/// than mysterious.
pub fn empty_query_hint(state: &SearchState) -> &'static str {
    if state.query.is_empty() {
        "Type something to search for"
    } else {
        ""
    }
}
