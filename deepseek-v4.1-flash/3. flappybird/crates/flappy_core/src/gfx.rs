//! Small software rasterizer + text drawing over a 32-bit framebuffer.
//!
//! Colours are `0x00RRGGBB`, which is exactly the layout `softbuffer` wants on
//! the native side, so the desktop blit is a straight `copy_from_slice`. The
//! wasm side converts to RGBA8 once per frame for `ImageData`.

use crate::font::{self, Align};

#[derive(Clone)]
pub struct Framebuffer {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
}

impl Framebuffer {
    pub fn new(w: i32, h: i32) -> Self {
        Framebuffer {
            w,
            h,
            px: vec![0; (w * h) as usize],
        }
    }

    #[inline]
    pub fn resize(&mut self, w: i32, h: i32) {
        if self.w != w || self.h != h {
            self.w = w;
            self.h = h;
            self.px.clear();
            self.px.resize((w * h) as usize, 0);
        }
    }

    #[inline]
    pub fn clear(&mut self, c: u32) {
        self.px.fill(c);
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, c: u32) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = c;
        }
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u32 {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize]
        } else {
            0
        }
    }

    /// Source-over blend of `c` at `alpha` in `0..=255` over the existing pixel.
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, c: u32, alpha: u32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let idx = (y * self.w + x) as usize;
        let dst = self.px[idx];
        self.px[idx] = lerp_rgb(dst, c, alpha);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: u32) {
        if w <= 0 || h <= 0 {
            return;
        }
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.w);
        let y1 = (y + h).min(self.h);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for row in y0..y1 {
            let base = (row * self.w) as usize;
            self.px[base + x0 as usize..base + x1 as usize].fill(c);
        }
    }

    pub fn fill_rect_blend(&mut self, x: i32, y: i32, w: i32, h: i32, c: u32, alpha: u32) {
        if alpha == 0 || w <= 0 || h <= 0 {
            return;
        }
        if alpha >= 255 {
            self.fill_rect(x, y, w, h, c);
            return;
        }
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.w);
        let y1 = (y + h).min(self.h);
        for row in y0..y1 {
            let base = (row * self.w) as usize;
            for col in x0..x1 {
                let idx = base + col as usize;
                self.px[idx] = lerp_rgb(self.px[idx], c, alpha);
            }
        }
    }

    pub fn rect_outline(&mut self, x: i32, y: i32, w: i32, h: i32, t: i32, c: u32) {
        if w <= 0 || h <= 0 || t <= 0 {
            return;
        }
        self.fill_rect(x, y, w, t, c);
        self.fill_rect(x, y + h - t, w, t, c);
        self.fill_rect(x, y + t, t, h - 2 * t, c);
        self.fill_rect(x + w - t, y + t, t, h - 2 * t, c);
    }

    /// Filled axis-aligned ellipse. Used for the bird body, eyes and clouds.
    pub fn fill_ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, c: u32) {
        if rx <= 0.0 || ry <= 0.0 {
            return;
        }
        let y0 = (cy - ry).floor() as i32;
        let y1 = (cy + ry).ceil() as i32;
        for py in y0..=y1 {
            let dy = (py as f32 + 0.5 - cy) / ry;
            if dy.abs() > 1.0 {
                continue;
            }
            let half = rx * (1.0 - dy * dy).max(0.0).sqrt();
            let x0 = (cx - half).round() as i32;
            let x1 = (cx + half).round() as i32;
            self.fill_rect(x0, py, x1 - x0, 1, c);
        }
    }

    /// Filled triangle (integer coords) via half-space tests over the bbox.
    pub fn fill_triangle(&mut self, p0: (i32, i32), p1: (i32, i32), p2: (i32, i32), c: u32) {
        let min_x = p0.0.min(p1.0).min(p2.0).max(0);
        let max_x = p0.0.max(p1.0).max(p2.0).min(self.w - 1);
        let min_y = p0.1.min(p1.1).min(p2.1).max(0);
        let max_y = p0.1.max(p1.1).max(p2.1).min(self.h - 1);
        if min_x > max_x || min_y > max_y {
            return;
        }
        let area = edge(p0, p1, p2);
        if area == 0 {
            return;
        }
        let sign = if area > 0 { 1 } else { -1 };
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let p = (px, py);
                let w0 = edge(p1, p2, p) * sign;
                let w1 = edge(p2, p0, p) * sign;
                let w2 = edge(p0, p1, p) * sign;
                if w0 >= 0 && w1 >= 0 && w2 >= 0 {
                    self.px[(py * self.w + px) as usize] = c;
                }
            }
        }
    }

    /// Vertical gradient over `[y0, y1)`.
    pub fn vgradient(&mut self, y0: i32, y1: i32, top: u32, bottom: u32) {
        let span = (y1 - y0).max(1);
        for y in y0.max(0)..y1.min(self.h) {
            let t = (((y - y0) as f32 / span as f32) * 255.0) as u32;
            let c = lerp_rgb(top, bottom, t.min(255));
            self.fill_rect(0, y, self.w, 1, c);
        }
    }

    /// Single-column vertical span, used by the parallax hill silhouettes.
    pub fn fill_column(&mut self, x: i32, y0: i32, y1: i32, c: u32) {
        if y1 <= y0 {
            return;
        }
        let y0 = y0.max(0);
        let y1 = y1.min(self.h);
        if x < 0 || x >= self.w {
            return;
        }
        for y in y0..y1 {
            self.px[(y * self.w + x) as usize] = c;
        }
    }

    pub fn text_width(s: &str, scale: i32) -> i32 {
        if s.is_empty() {
            return 0;
        }
        s.chars().count() as i32 * font::ADVANCE * scale - scale
    }

    pub fn draw_text(&mut self, x: i32, y: i32, s: &str, scale: i32, c: u32, align: Align) {
        let w = Self::text_width(s, scale);
        let mut pen = match align {
            Align::Left => x,
            Align::Center => x - w / 2,
            Align::Right => x - w,
        };
        for ch in s.chars() {
            let g = font::glyph(ch);
            for (row, bits) in g.iter().enumerate() {
                if *bits == 0 {
                    continue;
                }
                for col in 0..font::GLYPH_W {
                    if bits & (1 << (font::GLYPH_W - 1 - col)) != 0 {
                        self.fill_rect(
                            pen + col * scale,
                            y + row as i32 * scale,
                            scale,
                            scale,
                            c,
                        );
                    }
                }
            }
            pen += font::ADVANCE * scale;
        }
    }

    /// Text with a 1px (scaled) drop shadow — keeps HUD text readable over pipes.
    pub fn draw_text_shadow(
        &mut self,
        x: i32,
        y: i32,
        s: &str,
        scale: i32,
        c: u32,
        shadow: u32,
        align: Align,
    ) {
        let o = scale.max(1);
        self.draw_text(x + o, y + o, s, scale, shadow, align);
        self.draw_text(x, y, s, scale, c, align);
    }
}

#[inline]
fn edge(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> i32 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// `alpha` 0..=255: 0 keeps `dst`, 255 returns `src`.
#[inline]
pub fn lerp_rgb(dst: u32, src: u32, alpha: u32) -> u32 {
    if alpha >= 255 {
        return src;
    }
    if alpha == 0 {
        return dst;
    }
    let inv = 255 - alpha;
    let r = (((src >> 16) & 0xff) * alpha + ((dst >> 16) & 0xff) * inv) / 255;
    let g = (((src >> 8) & 0xff) * alpha + ((dst >> 8) & 0xff) * inv) / 255;
    let b = ((src & 0xff) * alpha + (dst & 0xff) * inv) / 255;
    (r << 16) | (g << 8) | b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_keeps_writes_inside() {
        let mut fb = Framebuffer::new(16, 8);
        fb.fill_rect(-5, -5, 100, 100, 0xff0000);
        assert!(fb.px.iter().all(|p| *p == 0xff0000));
        fb.fill_rect(100, 100, 10, 10, 0x00ff00);
        assert!(fb.px.iter().all(|p| *p == 0xff0000));
    }

    #[test]
    fn text_width_matches_advance() {
        assert_eq!(Framebuffer::text_width("A", 1), 5);
        assert_eq!(Framebuffer::text_width("AB", 1), 11);
        assert_eq!(Framebuffer::text_width("", 4), 0);
        assert_eq!(Framebuffer::text_width("AB", 2), 22);
    }

    #[test]
    fn text_is_drawn_somewhere() {
        let mut fb = Framebuffer::new(64, 16);
        fb.draw_text(0, 0, "RUST", 2, 0xffffff, Align::Left);
        assert!(fb.px.iter().any(|p| *p == 0xffffff));
    }

    #[test]
    fn blend_endpoints() {
        assert_eq!(lerp_rgb(0x000000, 0xffffff, 0), 0x000000);
        assert_eq!(lerp_rgb(0x000000, 0xffffff, 255), 0xffffff);
        assert_eq!(lerp_rgb(0x000000, 0xffffff, 128), 0x808080);
    }
}
