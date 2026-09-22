//! Shared UI types: monospace metrics and the colour scheme.

pub mod dialogs;
pub mod jobs;
pub mod searchbar;
pub mod tabs;
pub mod textview;

use egui::{Color32, FontId};

/// Monospace font metrics, measured once per frame from the live font atlas.
///
/// Every glyph is assumed to advance by exactly [`Metrics::char_w`]. That is the
/// central simplifying decision of the renderer: it makes cursor and selection
/// positions *exact* (the requirement is consistent positioning, not
/// script-aware shaping), it removes the need for a text shaper on the hot path,
/// and it turns "pixel offset -> byte offset" into a byte scan instead of a
/// layout. Wide glyphs (CJK, emoji) may overhang their cell; their cell is still
/// one character wide, so columns never drift.
#[derive(Clone)]
pub struct Metrics {
    pub font: FontId,
    pub char_w: f32,
    pub line_h: f32,
    /// Tab stop width in pixels.
    pub tab_w: f32,
    /// Ascent, used to centre text in the line box.
    pub row_height: f32,
}

impl Metrics {
    pub fn new(ctx: &egui::Context, size: f32, tab_columns: f32) -> Self {
        let font = FontId::monospace(size);
        // Every text-layout query takes `&mut FontsView`, so this must go through
        // `fonts_mut`; `fonts` only exposes `&self` accessors.
        let (char_w, line_h, row_height) = ctx.fonts_mut(|f| {
            let cw = f.glyph_width(&font, 'M');
            let rh = f.row_height(&font);
            // Measure a tall glyph to get the true line box, not just the em size.
            let lh = f
                .layout_no_wrap("Mg".to_string(), font.clone(), Color32::WHITE)
                .size()
                .y;
            (cw, lh.max(rh), rh)
        });
        Metrics {
            font,
            char_w,
            line_h,
            tab_w: char_w * tab_columns,
            row_height,
        }
    }
}

/// Fixed colour scheme for the editor surface.
///
/// Chosen for contrast against the text so selection and match highlights stay
/// legible without any theming machinery.
#[derive(Clone)]
pub struct Palette {
    pub bg: Color32,
    pub text: Color32,
    pub gutter_bg: Color32,
    pub gutter_text: Color32,
    pub gutter_active: Color32,
    pub caret: Color32,
    pub selection: Color32,
    pub match_bg: Color32,
    pub match_current_bg: Color32,
    pub current_line: Color32,
    pub scrollbar: Color32,
    pub scrollbar_hover: Color32,
    pub error: Color32,
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            bg: Color32::from_rgb(0x1e, 0x1f, 0x22),
            text: Color32::from_rgb(0xd8, 0xdc, 0xe2),
            gutter_bg: Color32::from_rgb(0x18, 0x19, 0x1c),
            gutter_text: Color32::from_rgb(0x6b, 0x72, 0x7d),
            gutter_active: Color32::from_rgb(0xc8, 0xcd, 0xd6),
            caret: Color32::from_rgb(0xff, 0xd0, 0x66),
            selection: Color32::from_rgb(0x2e, 0x4a, 0x6e),
            match_bg: Color32::from_rgb(0x3d, 0x40, 0x2a),
            match_current_bg: Color32::from_rgb(0x7a, 0x5c, 0x14),
            current_line: Color32::from_rgb(0x24, 0x26, 0x2a),
            scrollbar: Color32::from_rgb(0x3a, 0x3d, 0x43),
            scrollbar_hover: Color32::from_rgb(0x55, 0x5a, 0x62),
            error: Color32::from_rgb(0xff, 0x8a, 0x80),
        }
    }
}
