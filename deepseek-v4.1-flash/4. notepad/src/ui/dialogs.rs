//! Modal confirmation dialogs.
//!
//! Unsaved-change decisions are the one place where losing data is possible, so
//! the dialog offers exactly the three outcomes the brief requires — save,
//! discard, cancel — and closing it any other way (Escape, backdrop click) is
//! treated as *cancel*, never as discard.

use egui::{Context, Id};

/// What is being confirmed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Confirm {
    /// Closing one tab, by index into the document list.
    CloseTab(usize),
    /// Exiting the application.
    Exit,
}

/// The user's decision.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    Save,
    Discard,
    Cancel,
}

/// Draw the confirmation modal, if one is pending.
pub fn confirm_modal(ctx: &Context, confirm: Confirm, name: &str) -> Option<Decision> {
    let mut decision = None;
    let title = match confirm {
        Confirm::CloseTab(_) => "Close document",
        Confirm::Exit => "Exit",
    };

    let resp = egui::Modal::new(Id::new("confirm-unsaved")).show(ctx, |ui| {
        ui.set_min_width(360.0);
        ui.heading(title);
        ui.add_space(6.0);
        ui.label(format!(
            "“{name}” has unsaved changes.\n\nSave them before closing?"
        ));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                decision = Some(Decision::Save);
            }
            if ui.button("Discard").clicked() {
                decision = Some(Decision::Discard);
            }
            if ui.button("Cancel").clicked() {
                decision = Some(Decision::Cancel);
            }
        });
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Escape or clicking outside cancels and keeps your changes.")
                .small()
                .weak(),
        );
    });

    // Escape, backdrop click, or any dismissal means "cancel".
    if decision.is_none() && resp.should_close() {
        decision = Some(Decision::Cancel);
    }
    if decision.is_none() && resp.backdrop_response.clicked() {
        decision = Some(Decision::Cancel);
    }
    decision
}

/// Draw a blocking error report for a failed file operation.
///
/// The document itself is never touched by a failure, so this only informs.
pub fn error_modal(ctx: &Context, message: &str) -> bool {
    let mut dismissed = false;
    egui::Modal::new(Id::new("error-modal")).show(ctx, |ui| {
        ui.set_min_width(420.0);
        ui.heading("File error");
        ui.add_space(6.0);
        ui.label(message);
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new("The open document has been left unchanged.")
                .small()
                .weak(),
        );
        ui.add_space(8.0);
        if ui.button("OK").clicked() {
            dismissed = true;
        }
    });
    dismissed
}
