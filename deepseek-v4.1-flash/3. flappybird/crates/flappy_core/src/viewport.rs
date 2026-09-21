//! Letterboxed scaling between the fixed logical play area and a physical
//! surface (window pixels or a canvas backing store).
//!
//! Both backends use this one definition, which is what guarantees that
//! resizing a window or rotating a phone never changes gameplay: the logical
//! area is always `360x640`, and the remainder of the surface is letterboxed.
//!
//! The scaling factor is continuous (not integer-only) so the play area uses as
//! much of the screen as possible; sampling is nearest-neighbour, which suits
//! the flat, pixel-art-style rendering and keeps the blit cheap.

use crate::game::{LOGICAL_H, LOGICAL_W};
use crate::gfx::Framebuffer;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Viewport {
    pub phys_w: i32,
    pub phys_h: i32,
    /// Physical pixels per logical pixel.
    pub scale: f32,
    /// Destination rectangle of the play area inside the surface.
    pub dst_x: i32,
    pub dst_y: i32,
    pub dst_w: i32,
    pub dst_h: i32,
}

impl Viewport {
    pub fn new(phys_w: u32, phys_h: u32) -> Self {
        // A zero-sized surface happens transiently while minimising on Windows.
        let phys_w = phys_w.max(1) as i32;
        let phys_h = phys_h.max(1) as i32;
        let scale = (phys_w as f32 / LOGICAL_W as f32).min(phys_h as f32 / LOGICAL_H as f32);
        // `round` rather than `floor`: truncating loses a pixel whenever the
        // product lands just under an integer (500/360*360 == 499.99997), which
        // would leave a 1px sliver of letterbox on an otherwise exact fit.
        let dst_w = ((LOGICAL_W as f32 * scale).round() as i32).clamp(1, phys_w);
        let dst_h = ((LOGICAL_H as f32 * scale).round() as i32).clamp(1, phys_h);
        Viewport {
            phys_w,
            phys_h,
            scale,
            dst_x: (phys_w - dst_w) / 2,
            dst_y: (phys_h - dst_h) / 2,
            dst_w,
            dst_h,
        }
    }

    /// Physical surface point -> logical play-area point.
    ///
    /// Points inside a letterbox bar map outside `0..LOGICAL_W`/`0..LOGICAL_H`,
    /// so hit-testing against logical rectangles rejects them naturally.
    #[inline]
    pub fn to_logical(&self, px: f32, py: f32) -> (f32, f32) {
        (
            (px - self.dst_x as f32) / self.scale,
            (py - self.dst_y as f32) / self.scale,
        )
    }

    /// Fills the whole surface with the letterbox colour and draws the logical
    /// frame scaled into the middle of it.
    ///
    /// `dst` is a `0x00RRGGBB` buffer of `phys_h * stride` pixels, matching
    /// `softbuffer`'s layout on Windows.
    pub fn blit_rgb(&self, fb: &Framebuffer, dst: &mut [u32], stride: usize, letterbox: u32) {
        if stride == 0 || dst.len() < self.phys_h as usize * stride {
            return;
        }
        let used = self.phys_h as usize * stride;
        dst[..used].fill(letterbox);

        if fb.w <= 0 || fb.h <= 0 {
            return;
        }
        // Integer stepping keeps the sampling stable frame to frame.
        for dy in 0..self.dst_h {
            let sy = (dy as i64 * fb.h as i64 / self.dst_h as i64).clamp(0, fb.h as i64 - 1) as i32;
            let row = (self.dst_y + dy) as usize * stride + self.dst_x as usize;
            let src_row = (sy * fb.w) as usize;
            for dx in 0..self.dst_w {
                let sx = (dx as i64 * fb.w as i64 / self.dst_w as i64).clamp(0, fb.w as i64 - 1)
                    as i32;
                dst[row + dx as usize] = fb.px[src_row + sx as usize];
            }
        }
    }
}

/// Colour used for the letterbox bars: a dark slate that reads as a deliberate
/// frame around the play area.
pub const LETTERBOX: u32 = 0x101f27;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_aspect_fills_the_surface_without_bars() {
        let v = Viewport::new(720, 1280);
        assert_eq!((v.dst_x, v.dst_y), (0, 0));
        assert_eq!((v.dst_w, v.dst_h), (720, 1280));
        assert!((v.scale - 2.0).abs() < 1e-6);
    }

    #[test]
    fn wide_surface_letterboxes_left_and_right_only() {
        let v = Viewport::new(1920, 1080);
        assert_eq!(v.dst_y, 0);
        assert_eq!(v.dst_h, 1080);
        assert!(v.dst_w < 1920);
        assert!(v.dst_x > 0);
        // Bars are centred to within the unavoidable single-pixel rounding.
        let right = 1920 - v.dst_w - v.dst_x;
        assert!((v.dst_x - right).abs() <= 1, "bars {left} vs {right}", left = v.dst_x);
        // The play area keeps its aspect ratio to within a pixel.
        let aspect = v.dst_w as f32 / v.dst_h as f32;
        let want = LOGICAL_W as f32 / LOGICAL_H as f32;
        assert!((aspect - want).abs() < 0.01, "{aspect} vs {want}");
    }

    #[test]
    fn tall_surface_letterboxes_top_and_bottom_only() {
        let v = Viewport::new(500, 1600);
        assert_eq!(v.dst_x, 0);
        assert_eq!(v.dst_w, 500);
        assert!(v.dst_h < 1600);
        let bottom = 1600 - v.dst_h - v.dst_y;
        assert!((v.dst_y - bottom).abs() <= 1, "bars {top} vs {bottom}", top = v.dst_y);
    }

    #[test]
    fn play_area_never_exceeds_the_surface() {
        for (w, h) in [
            (1, 1),
            (320, 240),
            (640, 480),
            (800, 600),
            (1080, 1920),
            (2560, 1440),
            (3840, 2160),
            (200, 900),
        ] {
            let v = Viewport::new(w, h);
            assert!(v.dst_x >= 0 && v.dst_y >= 0);
            assert!(v.dst_x + v.dst_w <= w as i32, "{w}x{h}");
            assert!(v.dst_y + v.dst_h <= h as i32, "{w}x{h}");
            assert!(v.scale > 0.0);
        }
    }

    #[test]
    fn logical_mapping_round_trips_through_the_play_area() {
        let v = Viewport::new(1920, 1080);
        let (x, y) = v.to_logical(v.dst_x as f32 + 10.0, v.dst_y as f32 + 20.0);
        assert!((x - 10.0 / v.scale).abs() < 0.01);
        assert!((y - 20.0 / v.scale).abs() < 0.01);
        // Centre of the play area maps to the centre of the logical area.
        let (cx, cy) = v.to_logical(
            (v.dst_x + v.dst_w / 2) as f32,
            (v.dst_y + v.dst_h / 2) as f32,
        );
        assert!((cx - LOGICAL_W as f32 / 2.0).abs() < 1.0);
        assert!((cy - LOGICAL_H as f32 / 2.0).abs() < 1.0);
        // A point in the left bar maps outside the logical area.
        let (bx, _) = v.to_logical(2.0, 500.0);
        assert!(bx < 0.0);
    }

    #[test]
    fn blit_paints_bars_and_the_scaled_frame() {
        let mut fb = Framebuffer::new(LOGICAL_W, LOGICAL_H);
        fb.clear(0x00ff00);
        let v = Viewport::new(1920, 1080);
        let mut dst = vec![0u32; 1920 * 1080];
        v.blit_rgb(&fb, &mut dst, 1920, LETTERBOX);
        assert_eq!(dst[0], LETTERBOX, "top-left corner is a bar");
        assert_eq!(
            dst[540 * 1920 + 960],
            0x00ff00,
            "centre of the play area is game content"
        );
        // The whole play rectangle is content, the rest is bar.
        let content = dst.iter().filter(|p| **p == 0x00ff00).count();
        assert_eq!(content, (v.dst_w * v.dst_h) as usize);
    }
}
