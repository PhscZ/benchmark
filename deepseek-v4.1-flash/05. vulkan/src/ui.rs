//! Immediate-mode UI: a bitmap-font atlas plus a flat list of coloured,
//! textured quads.  The UI is drawn by the presentation pass; it is never part
//! of the ray-traced image, so saved PNGs contain no overlays.

use crate::error::{msg, Result};

pub const ATLAS_WIDTH: u32 = 128;
pub const ATLAS_HEIGHT: u32 = 64;
const CELL: u32 = 8;
const COLUMNS: u32 = ATLAS_WIDTH / CELL; // 16
/// Cell reserved for a solid white texel (used by untextured rectangles).
const WHITE_CELL: u32 = 127;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UiVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn contains(&self, point: [f32; 2]) -> bool {
        point[0] >= self.x
            && point[0] < self.x + self.width
            && point[1] >= self.y
            && point[1] < self.y + self.height
    }
}

/// Everything the UI can trigger.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    SetSamples(u32),
    ReflectionsDelta(i32),
    ExposureDelta(i32),
    Restart,
    ResetCamera,
    SavePng,
    TogglePause,
    RunBenchmark,
    Quit,
}

/// Renders the 8x8 bitmap font into an R8 atlas (white = coverage).
pub fn build_font_atlas() -> Vec<u8> {
    let mut data = vec![0u8; (ATLAS_WIDTH * ATLAS_HEIGHT) as usize];
    for ch in 0u32..128 {
        if ch == WHITE_CELL {
            continue;
        }
        let glyph = font8x8::legacy::BASIC_LEGACY[ch as usize];
        let cell_x = (ch % COLUMNS) * CELL;
        let cell_y = (ch / COLUMNS) * CELL;
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..CELL {
                // font8x8 stores bit 0 as the leftmost pixel of each row.
                if (bits >> col) & 1 != 0 {
                    let x = cell_x + col;
                    let y = cell_y + row as u32;
                    data[(y * ATLAS_WIDTH + x) as usize] = 255;
                }
            }
        }
    }
    // Solid white cell for untextured quads.
    let cell_x = (WHITE_CELL % COLUMNS) * CELL;
    let cell_y = (WHITE_CELL / COLUMNS) * CELL;
    for y in cell_y..cell_y + CELL {
        for x in cell_x..cell_x + CELL {
            data[(y * ATLAS_WIDTH + x) as usize] = 255;
        }
    }
    data
}

/// UV rectangle of a glyph cell, in atlas texture coordinates.
fn glyph_uv(ch: u8) -> [f32; 2] {
    let ch = u32::from(ch).min(126);
    let cell_x = (ch % COLUMNS) * CELL;
    let cell_y = (ch / COLUMNS) * CELL;
    [
        cell_x as f32 / ATLAS_WIDTH as f32,
        cell_y as f32 / ATLAS_HEIGHT as f32,
    ]
}

fn white_uv() -> [f32; 2] {
    let cell_x = (WHITE_CELL % COLUMNS) * CELL;
    let cell_y = (WHITE_CELL / COLUMNS) * CELL;
    [
        (cell_x as f32 + CELL as f32 * 0.5) / ATLAS_WIDTH as f32,
        (cell_y as f32 + CELL as f32 * 0.5) / ATLAS_HEIGHT as f32,
    ]
}

const GLYPH_U: f32 = CELL as f32 / ATLAS_WIDTH as f32;
const GLYPH_V: f32 = CELL as f32 / ATLAS_HEIGHT as f32;

pub struct UiBuilder {
    pub vertices: Vec<UiVertex>,
    pub buttons: Vec<(Action, Rect)>,
    width: f32,
    height: f32,
}

impl UiBuilder {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            vertices: Vec::with_capacity(4096),
            buttons: Vec::new(),
            width,
            height,
        }
    }

    pub fn reset(&mut self, width: f32, height: f32) {
        self.vertices.clear();
        self.buttons.clear();
        self.width = width;
        self.height = height;
    }

    pub fn viewport(&self) -> [f32; 2] {
        [self.width, self.height]
    }

    /// Text size in pixels for a given integer scale (the bitmap font is 8x8).
    pub fn text_size(text: &str, scale: f32) -> [f32; 2] {
        [text.chars().count() as f32 * 8.0 * scale, 8.0 * scale]
    }

    pub fn rect(&mut self, rect: Rect, color: [f32; 4]) {
        let uv = white_uv();
        let x0 = rect.x;
        let y0 = rect.y;
        let x1 = rect.x + rect.width;
        let y1 = rect.y + rect.height;
        let corners = [
            ([x0, y0], [uv[0], uv[1]]),
            ([x1, y0], [uv[0], uv[1]]),
            ([x1, y1], [uv[0], uv[1]]),
            ([x0, y0], [uv[0], uv[1]]),
            ([x1, y1], [uv[0], uv[1]]),
            ([x0, y1], [uv[0], uv[1]]),
        ];
        for (position, uv) in corners {
            self.vertices.push(UiVertex {
                position,
                uv,
                color,
            });
        }
    }

    /// Draws text with the top-left corner at `(x, y)`.
    pub fn text(&mut self, x: f32, y: f32, scale: f32, color: [f32; 4], text: &str) {
        let mut cursor = x;
        for ch in text.chars() {
            let code = ch as u32;
            let code = if (32..127).contains(&code) { code } else { b'?' as u32 };
            let uv = glyph_uv(code as u8);
            let w = 8.0 * scale;
            let h = 8.0 * scale;
            let x0 = cursor;
            let y0 = y;
            let x1 = cursor + w;
            let y1 = y + h;
            let u0 = uv[0];
            let v0 = uv[1];
            let u1 = uv[0] + GLYPH_U;
            let v1 = uv[1] + GLYPH_V;
            let corners = [
                ([x0, y0], [u0, v0]),
                ([x1, y0], [u1, v0]),
                ([x1, y1], [u1, v1]),
                ([x0, y0], [u0, v0]),
                ([x1, y1], [u1, v1]),
                ([x0, y1], [u0, v1]),
            ];
            for (position, uv) in corners {
                self.vertices.push(UiVertex {
                    position,
                    uv,
                    color,
                });
            }
            cursor += w;
        }
    }

    /// Draws a labelled button and registers it for hit testing.
    /// Returns the width consumed.
    pub fn button(
        &mut self,
        x: f32,
        y: f32,
        scale: f32,
        label: &str,
        action: Action,
        highlighted: bool,
    ) -> f32 {
        let padding = 6.0;
        let [text_w, text_h] = Self::text_size(label, scale);
        let rect = Rect {
            x,
            y,
            width: text_w + padding * 2.0,
            height: text_h + padding * 2.0,
        };
        let background = if highlighted {
            [0.24, 0.38, 0.62, 0.92]
        } else {
            [0.10, 0.12, 0.16, 0.82]
        };
        self.rect(rect, background);
        self.text(
            x + padding,
            y + padding,
            scale,
            [0.92, 0.94, 0.98, 1.0],
            label,
        );
        self.buttons.push((action, rect));
        rect.width
    }

    /// Draws a text panel with a translucent background.
    pub fn panel(&mut self, x: f32, y: f32, scale: f32, lines: &[String], line_gap: f32) {
        let max_chars = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f32;
        let padding = 8.0;
        let width = max_chars * 8.0 * scale + padding * 2.0;
        let height = lines.len() as f32 * line_gap + padding * 2.0;
        self.rect(
            Rect {
                x,
                y,
                width,
                height,
            },
            [0.05, 0.06, 0.08, 0.72],
        );
        for (i, line) in lines.iter().enumerate() {
            self.text(
                x + padding,
                y + padding + i as f32 * line_gap,
                scale,
                [0.88, 0.90, 0.94, 1.0],
                line,
            );
        }
    }

    /// Returns the action under `point`, if any (later buttons win).
    pub fn hit_test(&self, point: [f32; 2]) -> Option<Action> {
        self.buttons
            .iter()
            .rev()
            .find(|(_, rect)| rect.contains(point))
            .map(|(action, _)| *action)
    }
}

/// Converts the atlas into an 8-bit single-channel image descriptor.
pub fn atlas_extent() -> Result<(u32, u32)> {
    if ATLAS_WIDTH == 0 || ATLAS_HEIGHT == 0 {
        return msg("font atlas has zero extent");
    }
    Ok((ATLAS_WIDTH, ATLAS_HEIGHT))
}
