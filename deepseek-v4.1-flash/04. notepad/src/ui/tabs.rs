//! The tab strip.
//!
//! Each tab carries its own unsaved marker. The strip scrolls horizontally rather
//! than shrinking tabs, so the label stays readable no matter how many documents
//! are open.

use crate::document::Document;

/// What the user did to the tab strip this frame.
pub enum TabAction {
    None,
    Activate(usize),
    Close(usize),
}

pub fn show(ui: &mut egui::Ui, docs: &[Document], active: usize) -> TabAction {
    let mut action = TabAction::None;

    egui::ScrollArea::horizontal()
        .id_salt("tab-strip")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, doc) in docs.iter().enumerate() {
                    let selected = i == active;
                    let label = doc.tab_label();

                    // The label carries the unsaved marker; the tooltip gives the
                    // full path so two files with the same name stay tellable apart.
                    let text = egui::RichText::new(label).monospace();
                    let text = if doc.is_dirty() {
                        text.color(ui.visuals().warn_fg_color)
                    } else {
                        text
                    };

                    let resp = ui.selectable_label(selected, text);
                    let resp = if let Some(p) = &doc.path {
                        resp.on_hover_text(p.display().to_string())
                    } else {
                        resp.on_hover_text("Unsaved new document")
                    };

                    if resp.clicked() {
                        action = TabAction::Activate(i);
                    }
                    if resp.middle_clicked() {
                        action = TabAction::Close(i);
                    }

                    // Per-tab close button.
                    if ui
                        .small_button("×")
                        .on_hover_text("Close tab (Ctrl+W)")
                        .clicked()
                    {
                        action = TabAction::Close(i);
                    }
                    ui.add_space(2.0);
                }
            });
        });

    action
}
