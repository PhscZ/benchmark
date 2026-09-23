//! Rendering. Everything is drawn into the fixed logical framebuffer with
//! procedural primitives only — no sprites, no font files, no external assets.
//!
//! Collision geometry lives in `game.rs` and the shapes drawn here are derived
//! from the same constants, so what you see is what you hit.

use crate::font::Align;
use crate::game::*;
use crate::gfx::Framebuffer;

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

const SKY_TOP: u32 = 0x4fbfe8;
const SKY_BOTTOM: u32 = 0xbfe9f5;
const CLOUD: u32 = 0xffffff;
const CLOUD_SHADE: u32 = 0xdceef6;
const HILL_FAR: u32 = 0x8ec9df;
const HILL_NEAR: u32 = 0x6aa9c6;

const PIPE_MAIN: u32 = 0x5cb531;
const PIPE_LIGHT: u32 = 0x93de63;
const PIPE_SHADE: u32 = 0x3d7d1c;
const PIPE_OUTLINE: u32 = 0x2a5410;

const GROUND_GRASS: u32 = 0x7cc24a;
const GROUND_GRASS_TOP: u32 = 0x9ade6a;
const GROUND_GRASS_EDGE: u32 = 0x3f6b25;
const GROUND_DIRT: u32 = 0xe3d9a2;
const GROUND_DIRT_LINE: u32 = 0xc9bc7c;
const GROUND_DIRT_DOT: u32 = 0xd0c489;

const BIRD_BODY: u32 = 0xf7d64a;
const BIRD_BELLY: u32 = 0xfdf2b0;
const BIRD_OUTLINE: u32 = 0x3a2a10;
const BIRD_WING: u32 = 0xfdfdfd;
const BIRD_BEAK: u32 = 0xf2802a;
const BIRD_EYE: u32 = 0xffffff;
const BIRD_PUPIL: u32 = 0x2a1e0c;

const TEXT: u32 = 0xffffff;
const TEXT_DARK: u32 = 0x1d3b47;
const TEXT_ACCENT: u32 = 0xffe066;

const PANEL_BG: u32 = 0x14313d;
const PANEL_EDGE: u32 = 0xffffff;
const BUTTON_BG: u32 = 0x10242d;
const BUTTON_EDGE: u32 = 0xe8f6fb;
const MUTED_EDGE: u32 = 0xff8a80;

const TAU: f32 = std::f32::consts::TAU;

/// Panels the HUD draws text into. Named so `tests::hud_text_fits_its_panels`
/// can assert every string fits without hard-coding the numbers twice.
const PAUSE_PANEL: (i32, i32, i32, i32) = (16, 236, LOGICAL_W - 32, 132);
const OVER_PANEL: (i32, i32, i32, i32) = (10, 180, LOGICAL_W - 20, 320);
/// Panel border thickness, subtracted from the interior available to text.
const PANEL_BORDER: i32 = 3;

impl Game {
    /// Draws one complete frame into the logical framebuffer.
    pub fn draw(&self, fb: &mut Framebuffer) {
        self.draw_background(fb);
        self.draw_pipes(fb);
        self.draw_ground(fb);
        self.draw_bird_sprite(fb);
        self.draw_hud(fb);
        self.draw_buttons(fb);
        if self.flash > 0.0 {
            fb.fill_rect_blend(
                0,
                0,
                LOGICAL_W,
                PLAY_H,
                0xffffff,
                (self.flash * 170.0) as u32,
            );
        }
    }

    // -- scenery -----------------------------------------------------------

    fn draw_background(&self, fb: &mut Framebuffer) {
        fb.vgradient(0, PLAY_H, SKY_TOP, SKY_BOTTOM);
        self.draw_clouds(fb);
        draw_hills(fb, self.hill_far, PLAY_H, 52.0, 190.0, HILL_FAR);
        draw_hills(fb, self.hill_near, PLAY_H, 34.0, 120.0, HILL_NEAR);
    }

    fn draw_clouds(&self, fb: &mut Framebuffer) {
        // Fixed, hand-placed cloud belt; only its offset changes.
        const CLOUDS: [(f32, f32, f32); 6] = [
            (20.0, 74.0, 1.00),
            (168.0, 132.0, 0.68),
            (286.0, 62.0, 1.18),
            (404.0, 118.0, 0.82),
            (498.0, 86.0, 0.60),
            (612.0, 140.0, 0.95),
        ];
        const PERIOD: f32 = 700.0;
        for (bx, by, s) in CLOUDS {
            let mut x = (bx - self.cloud_scroll) % PERIOD;
            if x < 0.0 {
                x += PERIOD;
            }
            let x = x - 90.0;
            if x > LOGICAL_W as f32 || x < -160.0 {
                continue;
            }
            draw_cloud(fb, x, by, s);
        }
    }

    fn draw_pipes(&self, fb: &mut Framebuffer) {
        for p in &self.pipes {
            let x = p.x.round() as i32;
            if x + PIPE_W + PIPE_CAP_OVERHANG < 0 || x - PIPE_CAP_OVERHANG > LOGICAL_W {
                continue;
            }
            let gt = p.gap_top();
            let gb = p.gap_bottom();
            // Upper pipe (body then cap, cap drawn wider on top).
            draw_pipe_segment(fb, x, 0, PIPE_W, gt, false);
            draw_pipe_segment(fb, x, gt - PIPE_CAP_H, PIPE_W, PIPE_CAP_H, true);
            // Lower pipe.
            draw_pipe_segment(fb, x, gb, PIPE_W, PLAY_H - gb, false);
            draw_pipe_segment(fb, x, gb, PIPE_W, PIPE_CAP_H, true);
        }
    }

    fn draw_ground(&self, fb: &mut Framebuffer) {
        let scroll = (self.ground_scroll % 34.0 + 34.0) % 34.0;
        // Grass strip.
        fb.fill_rect(0, PLAY_H, LOGICAL_W, 16, GROUND_GRASS);
        fb.fill_rect(0, PLAY_H, LOGICAL_W, 4, GROUND_GRASS_TOP);
        fb.fill_rect(0, PLAY_H, LOGICAL_W, 3, GROUND_GRASS_EDGE);
        fb.fill_rect(0, PLAY_H + 13, LOGICAL_W, 3, GROUND_GRASS_EDGE);
        // Dirt.
        fb.fill_rect(0, PLAY_H + 16, LOGICAL_W, GROUND_H - 16, GROUND_DIRT);
        fb.fill_rect(0, PLAY_H + 16, LOGICAL_W, 2, GROUND_DIRT_LINE);
        let mut i = 0;
        while i * 34 < LOGICAL_W + 34 {
            let x = (i as f32 * 34.0 - scroll) as i32;
            fb.fill_rect(x, PLAY_H + 38, 13, 9, GROUND_DIRT_DOT);
            fb.fill_rect(x + 17, PLAY_H + 64, 13, 9, GROUND_DIRT_DOT);
            i += 1;
        }
    }

    // -- bird --------------------------------------------------------------

    fn wing_frame(&self) -> u8 {
        if self.phase == Phase::Ready {
            // Idle flutter on the title screen.
            ((self.wing_t * 5.0) as u8) % 4
        } else if self.flap_anim < 0.24 {
            (self.flap_anim / 0.06) as u8
        } else {
            3
        }
    }

    fn draw_bird_sprite(&self, fb: &mut Framebuffer) {
        draw_bird(
            fb,
            BIRD_X,
            self.bird_draw_y(),
            self.bird_rot,
            self.wing_frame(),
        );
    }

    // -- HUD ---------------------------------------------------------------

    fn draw_hud(&self, fb: &mut Framebuffer) {
        match self.phase {
            Phase::Ready => self.draw_ready(fb),
            Phase::Playing => {
                let s = self.score.to_string();
                fb.draw_text_shadow(
                    LOGICAL_W / 2,
                    30,
                    &s,
                    5,
                    TEXT,
                    TEXT_DARK,
                    Align::Center,
                );
            }
            Phase::Paused => {
                self.draw_score_line(fb);
                self.draw_pause_overlay(fb);
            }
            Phase::GameOver => {
                if !self.panel_visible() {
                    self.draw_score_line(fb);
                } else {
                    self.draw_game_over(fb);
                }
            }
        }
    }

    fn draw_score_line(&self, fb: &mut Framebuffer) {
        let s = self.score.to_string();
        fb.draw_text_shadow(LOGICAL_W / 2, 30, &s, 5, TEXT, TEXT_DARK, Align::Center);
    }

    fn draw_ready(&self, fb: &mut Framebuffer) {
        fb.draw_text_shadow(
            LOGICAL_W / 2,
            104,
            "FLAPPY RUST",
            4,
            TEXT,
            TEXT_DARK,
            Align::Center,
        );
        fb.draw_text_shadow(
            LOGICAL_W / 2,
            150,
            "ONE CORE, NATIVE + WASM",
            2,
            TEXT,
            TEXT_DARK,
            Align::Center,
        );

        let best = format!("BEST {}", self.best());
        fb.draw_text_shadow(
            LOGICAL_W / 2,
            196,
            &best,
            2,
            TEXT_ACCENT,
            TEXT_DARK,
            Align::Center,
        );

        // Prompt panel keeps the text legible over the moving scenery.
        fb.fill_rect_blend(28, 336, LOGICAL_W - 56, 92, PANEL_BG, 190);
        fb.rect_outline(28, 336, LOGICAL_W - 56, 92, 2, PANEL_EDGE);
        fb.draw_text_shadow(
            LOGICAL_W / 2,
            352,
            "TAP, CLICK OR SPACE",
            2,
            TEXT,
            TEXT_DARK,
            Align::Center,
        );
        fb.draw_text_shadow(
            LOGICAL_W / 2,
            372,
            "TO FLAP",
            2,
            TEXT,
            TEXT_DARK,
            Align::Center,
        );
        fb.draw_text_shadow(
            LOGICAL_W / 2,
            396,
            "P OR ESC PAUSES",
            2,
            TEXT_ACCENT,
            TEXT_DARK,
            Align::Center,
        );

        fb.draw_text_shadow(
            LOGICAL_W / 2,
            452,
            "M TOGGLES SOUND",
            2,
            TEXT,
            TEXT_DARK,
            Align::Center,
        );
    }

    fn draw_pause_overlay(&self, fb: &mut Framebuffer) {
        fb.fill_rect_blend(0, 0, LOGICAL_W, PLAY_H, 0x081c24, 150);
        let (px, py, pw, ph) = PAUSE_PANEL;
        fb.fill_rect_blend(px, py, pw, ph, PANEL_BG, 225);
        fb.rect_outline(px, py, pw, ph, PANEL_BORDER, PANEL_EDGE);
        fb.draw_text_shadow(
            LOGICAL_W / 2,
            256,
            "PAUSED",
            4,
            TEXT,
            TEXT_DARK,
            Align::Center,
        );
        fb.draw_text_shadow(
            LOGICAL_W / 2,
            312,
            "TAP OR PRESS P TO RESUME",
            2,
            TEXT_ACCENT,
            TEXT_DARK,
            Align::Center,
        );
        if self.paused_by_focus {
            fb.draw_text_shadow(
                LOGICAL_W / 2,
                336,
                "FOCUS WAS LOST",
                2,
                TEXT,
                TEXT_DARK,
                Align::Center,
            );
        }
    }

    fn draw_game_over(&self, fb: &mut Framebuffer) {
        let a = (self.panel_t * 255.0) as u32;
        fb.fill_rect_blend(0, 0, LOGICAL_W, PLAY_H, 0x081c24, a.min(140));
        let (px, py, pw, ph) = OVER_PANEL;
        fb.fill_rect_blend(px, py, pw, ph, PANEL_BG, (a * 230 / 255).min(230));
        fb.rect_outline(px, py, pw, ph, PANEL_BORDER, PANEL_EDGE);

        fb.draw_text_shadow(
            LOGICAL_W / 2,
            204,
            "GAME OVER",
            3,
            TEXT,
            TEXT_DARK,
            Align::Center,
        );

        fb.draw_text(
            LOGICAL_W / 2,
            258,
            "SCORE",
            2,
            0xa9d6e4,
            Align::Center,
        );
        let score = self.score.to_string();
        fb.draw_text_shadow(LOGICAL_W / 2, 276, &score, 5, TEXT, TEXT_DARK, Align::Center);

        fb.draw_text(LOGICAL_W / 2, 330, "BEST", 2, 0xa9d6e4, Align::Center);
        let best = self.best().to_string();
        fb.draw_text_shadow(LOGICAL_W / 2, 346, &best, 3, TEXT_ACCENT, TEXT_DARK, Align::Center);

        if self.new_best {
            fb.draw_text_shadow(
                LOGICAL_W / 2,
                374,
                "NEW BEST!",
                2,
                TEXT_ACCENT,
                TEXT_DARK,
                Align::Center,
            );
        }

        // Restart button (rect matches `Game::button_rect(Button::Restart)`).
        let r = self.button_rect(Button::Restart);
        draw_button_frame(fb, r, false);
        fb.draw_text_shadow(
            r.x + r.w / 2,
            r.y + (r.h - 14) / 2,
            "RESTART",
            2,
            TEXT,
            TEXT_DARK,
            Align::Center,
        );

        if self.can_restart() {
            fb.draw_text_shadow(
                LOGICAL_W / 2,
                466,
                "TAP OR SPACE TO PLAY AGAIN",
                2,
                TEXT,
                TEXT_DARK,
                Align::Center,
            );
        }
    }

    fn draw_buttons(&self, fb: &mut Framebuffer) {
        if self.pause_button_visible() {
            let r = self.button_rect(Button::Pause);
            draw_button_frame(fb, r, false);
            if self.phase == Phase::Paused {
                // Play glyph.
                fb.fill_triangle(
                    (r.x + 17, r.y + 13),
                    (r.x + 17, r.y + 31),
                    (r.x + 31, r.y + 22),
                    TEXT,
                );
            } else {
                // Pause glyph.
                fb.fill_rect(r.x + 15, r.y + 13, 6, 18, TEXT);
                fb.fill_rect(r.x + 25, r.y + 13, 6, 18, TEXT);
            }
        }

        let m = self.button_rect(Button::Mute);
        draw_button_frame(fb, m, self.muted());
        let (cx, cy) = (m.x + m.w / 2, m.y + m.h / 2);
        // Speaker body + cone.
        fb.fill_rect(cx - 12, cy - 4, 6, 8, TEXT);
        fb.fill_triangle(
            (cx - 6, cy - 4),
            (cx - 6, cy + 4),
            (cx + 2, cy + 10),
            TEXT,
        );
        fb.fill_triangle(
            (cx - 6, cy - 4),
            (cx + 2, cy - 10),
            (cx + 2, cy + 10),
            TEXT,
        );
        if self.muted() {
            // Crossed out.
            let c = MUTED_EDGE;
            for i in 0..18 {
                fb.fill_rect(cx + 2 + i, cy - 9 + i, 2, 2, c);
                fb.fill_rect(cx + 19 - i, cy - 9 + i, 2, 2, c);
            }
        } else {
            // Two sound arcs approximated by short bars.
            fb.fill_rect(cx + 7, cy - 6, 2, 12, TEXT);
            fb.fill_rect(cx + 11, cy - 9, 2, 18, TEXT);
        }
    }
}

// ---------------------------------------------------------------------------
// Free drawing helpers
// ---------------------------------------------------------------------------

fn draw_button_frame(fb: &mut Framebuffer, r: Rect, alert: bool) {
    fb.fill_rect_blend(r.x, r.y, r.w, r.h, BUTTON_BG, 205);
    fb.rect_outline(r.x, r.y, r.w, r.h, 2, if alert { MUTED_EDGE } else { BUTTON_EDGE });
}

fn draw_cloud(fb: &mut Framebuffer, x: f32, y: f32, s: f32) {
    let (r1, r2, r3) = (20.0 * s, 15.0 * s, 12.0 * s);
    fb.fill_ellipse(x + 22.0 * s, y + 8.0 * s, r1 * 1.5, r2, CLOUD_SHADE);
    fb.fill_ellipse(x + 18.0 * s, y, r1, r2, CLOUD);
    fb.fill_ellipse(x + 40.0 * s, y + 4.0 * s, r3 * 1.2, r2 * 0.9, CLOUD);
    fb.fill_ellipse(x + 2.0 * s, y + 5.0 * s, r3, r2 * 0.8, CLOUD);
}

/// Rolling silhouette built from two summed sine harmonics.
fn draw_hills(fb: &mut Framebuffer, scroll: f32, base_y: i32, amp: f32, wavelength: f32, c: u32) {
    for x in 0..LOGICAL_W {
        let t = (x as f32 + scroll) / wavelength * TAU;
        let mut h = amp * (0.55 + 0.45 * t.sin()) + amp * 0.16 * (t * 2.7).sin();
        if h < 0.0 {
            h = 0.0;
        }
        fb.fill_column(x, base_y - h as i32, base_y, c);
    }
}

fn draw_pipe_segment(fb: &mut Framebuffer, x: i32, y: i32, w: i32, h: i32, cap: bool) {
    if h <= 0 {
        return;
    }
    let (x, w) = if cap {
        (x - PIPE_CAP_OVERHANG, w + 2 * PIPE_CAP_OVERHANG)
    } else {
        (x, w)
    };
    fb.fill_rect(x, y, w, h, PIPE_OUTLINE);
    fb.fill_rect(x + 2, y + 2, w - 4, h - 4, PIPE_MAIN);
    fb.fill_rect(x + 5, y + 2, 6, h - 4, PIPE_LIGHT);
    fb.fill_rect(x + w - 10, y + 2, 8, h - 4, PIPE_SHADE);
}

/// Wing offsets for the 4 animation frames: down, mid, up, mid. The value is
/// added to the wing's rest offset so the wing visibly beats.
const WING_DY: [f32; 4] = [2.4, 0.6, -2.8, 0.6];

/// Bird colour at a point in bird-local space (origin = centre, +x = nose,
/// +y = down). Returns 0 for "transparent".
///
/// Draw order matters: beak, then body outline, then wing, then eye, then belly.
/// The beak and eye are placed where the wing cannot reach so no shape is
/// clipped by another.
fn bird_pixel(lx: f32, ly: f32, wing: u8) -> u32 {
    // Beak: a tapering wedge that pokes out past the body on the right.
    if (7.0..=16.5).contains(&lx) && (-3.2..=4.4).contains(&ly) {
        let tip = ((lx - 7.0) / 9.5).clamp(0.0, 1.0);
        let half = 2.9 - 2.3 * tip;
        let mid = 0.7 - 0.2 * tip;
        if ly >= mid - half && ly <= mid + half {
            return BIRD_BEAK;
        }
        if ly >= mid - half - 1.0 && ly <= mid + half + 1.0 {
            return BIRD_OUTLINE;
        }
    }

    if !in_ellipse(lx, ly, 12.0, 9.0) {
        return 0;
    }

    let dy = WING_DY[(wing as usize).min(3)];
    let (wx, wy) = (lx + 4.0, ly - 2.0 - dy);
    if in_ellipse(wx, wy, 5.2, 3.2) {
        return if in_ellipse(wx, wy, 3.9, 2.1) {
            BIRD_WING
        } else {
            BIRD_OUTLINE
        };
    }

    if !in_ellipse(lx, ly, 10.6, 7.6) {
        return BIRD_OUTLINE;
    }

    // Eye: white sclera with a dark pupil, upper-right of the body.
    let (ex, ey) = (lx - 5.0, ly + 3.6);
    let ed = (ex * ex + ey * ey).sqrt();
    if ed < 3.9 {
        return if ed < 1.9 { BIRD_PUPIL } else { BIRD_EYE };
    }

    if in_ellipse(lx, ly - 3.4, 9.2, 5.0) {
        return BIRD_BELLY;
    }
    BIRD_BODY
}

#[inline]
fn in_ellipse(x: f32, y: f32, rx: f32, ry: f32) -> bool {
    let nx = x / rx;
    let ny = y / ry;
    nx * nx + ny * ny <= 1.0
}

/// Draws the bird rotated about its centre. The rotated bounding box is only
/// ~40x40 logical pixels, so the per-pixel inverse transform is cheap.
fn draw_bird(fb: &mut Framebuffer, cx: f32, cy: f32, angle: f32, wing: u8) {
    // Must cover the rotated silhouette including the beak: the furthest local
    // point is sqrt(16.5^2 + 9^2) ~= 18.8 px from the centre.
    let r = 20.0;
    let (s, c) = angle.sin_cos();
    let x0 = (cx - r).floor() as i32;
    let x1 = (cx + r).ceil() as i32;
    let y0 = (cy - r).floor() as i32;
    let y1 = (cy + r).ceil() as i32;
    for py in y0..=y1 {
        if py < 0 || py >= LOGICAL_H {
            continue;
        }
        for px in x0..=x1 {
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            let lx = dx * c + dy * s;
            let ly = -dx * s + dy * c;
            let col = bird_pixel(lx, ly, wing);
            if col != 0 {
                fb.set(px, py, col);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::Align;
    use crate::game::Game;

    /// Regression guard: text that overflows its panel or the play area is a
    /// real bug (it happened once), and it is invisible to the type system.
    #[test]
    fn hud_text_fits_its_panels() {
        let inner = |p: (i32, i32, i32, i32)| p.2 - 2 * PANEL_BORDER;
        let pause_inner = inner(PAUSE_PANEL);
        let over_inner = inner(OVER_PANEL);

        for (text, scale, limit, what) in [
            ("TAP OR PRESS P TO RESUME", 2, pause_inner, "pause panel"),
            ("FOCUS WAS LOST", 2, pause_inner, "pause panel"),
            ("TAP OR SPACE TO PLAY AGAIN", 2, over_inner, "game-over panel"),
            ("GAME OVER", 3, over_inner, "game-over panel"),
            ("NEW BEST!", 2, over_inner, "game-over panel"),
            ("FLAPPY RUST", 4, LOGICAL_W - 24, "play area"),
            ("ONE CORE, NATIVE + WASM", 2, LOGICAL_W - 24, "play area"),
            ("TAP, CLICK OR SPACE", 2, LOGICAL_W - 24, "play area"),
            ("M TOGGLES SOUND", 2, LOGICAL_W - 24, "play area"),
            ("P OR ESC PAUSES", 2, LOGICAL_W - 24, "play area"),
            ("BEST 1234567", 2, LOGICAL_W - 24, "play area"),
            ("1234567", 5, LOGICAL_W - 24, "play area"),
            ("PAUSED", 4, LOGICAL_W - 24, "play area"),
            ("RESTART", 2, 184 - 2 * 2, "restart button"),
        ] {
            let w = Framebuffer::text_width(text, scale);
            assert!(
                w <= limit,
                "{what}: {text:?} is {w} px wide but only {limit} px are available"
            );
        }
    }

    /// The bird must stay inside the play area, and the beak must be part of the
    /// silhouette at every wing frame (it is the farthest-forward feature).
    #[test]
    fn bird_silhouette_is_stable_across_wing_frames() {
        for wing in 0..4u8 {
            let mut min_x = f32::MAX;
            let mut max_x = f32::MIN;
            let mut min_y = f32::MAX;
            let mut max_y = f32::MIN;
            for y in -20..20 {
                for x in -20..20 {
                    if bird_pixel(x as f32 + 0.5, y as f32 + 0.5, wing) != 0 {
                        min_x = min_x.min(x as f32);
                        max_x = max_x.max(x as f32);
                        min_y = min_y.min(y as f32);
                        max_y = max_y.max(y as f32);
                    }
                }
            }
            assert!(max_x >= 15.0, "wing {wing}: beak is missing (max_x {max_x})");
            assert!(min_x <= -9.0, "wing {wing}: body is too narrow (min_x {min_x})");
            assert!(max_y - min_y >= 14.0, "wing {wing}: body is too short");
        }
    }

    /// The drawn bird must sit within the collision box's neighbourhood: a
    /// silhouette far larger than the hitbox would read as unfair.
    #[test]
    fn drawn_bird_matches_its_collision_box() {
        let mut min_x = f32::MAX;
        let mut max_x = f32::MIN;
        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        for y in -20..20 {
            for x in -20..20 {
                if bird_pixel(x as f32 + 0.5, y as f32 + 0.5, 1) != 0 {
                    min_x = min_x.min(x as f32 + 0.5);
                    max_x = max_x.max(x as f32 + 0.5);
                    min_y = min_y.min(y as f32 + 0.5);
                    max_y = max_y.max(y as f32 + 0.5);
                }
            }
        }
        // Body half-extents (ignoring the beak, which only points forward).
        assert!((max_y - min_y) / 2.0 <= BIRD_HIT_HALF_H + 2.0, "too tall for the hitbox");
        assert!(min_x >= -(BIRD_HIT_HALF_W + 2.0), "too wide behind the hitbox");
        // The bird is drawn at the x position the hitbox uses.
        let g = Game::new(1);
        assert_eq!(g.bird_hitbox().0 + BIRD_HIT_HALF_W, BIRD_X);
        let _ = Align::Center;
    }
}
