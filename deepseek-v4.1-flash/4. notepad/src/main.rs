//! Binary entry point.
//!
//! Usage:
//!
//! ```text
//!   notepad [FILE]
//! ```
//!
//! With no argument the editor opens a single empty document.

// Do not pop up a console window alongside the GUI on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use notepad::NotepadApp;
use std::path::PathBuf;

fn main() -> eframe::Result {
    let initial: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(match &initial {
                Some(p) => format!("{} — Notepad", p.display()),
                None => "Notepad".to_string(),
            })
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Notepad",
        native_options,
        Box::new(move |cc| Ok(Box::new(NotepadApp::new(cc, initial)))),
    )
}
