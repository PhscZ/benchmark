//! Colour processing shared by the display path and the PNG export.
//!
//! The arithmetic here is an exact mirror of `shaders/present.frag`:
//! average the accumulated linear samples, apply exposure, Reinhard tone map
//! (`c / (1 + c)`), then convert to sRGB exactly once.
//!
//! Lighting is accumulated in linear RGB with 32-bit floats; the sRGB transfer
//! function is applied only at the very end.

pub const DEFAULT_EXPOSURE: f32 = 1.0;

/// sRGB transfer function (IEC 61966-2-1), identical to the shader version.
#[inline]
pub fn srgb_encode_channel(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[inline]
pub fn srgb_encode(c: [f32; 3]) -> [f32; 3] {
    [
        srgb_encode_channel(c[0]),
        srgb_encode_channel(c[1]),
        srgb_encode_channel(c[2]),
    ]
}

#[inline]
pub fn reinhard_channel(c: f32) -> f32 {
    c / (1.0 + c)
}

#[inline]
pub fn reinhard(c: [f32; 3]) -> [f32; 3] {
    [
        reinhard_channel(c[0]),
        reinhard_channel(c[1]),
        reinhard_channel(c[2]),
    ]
}

#[inline]
pub fn quantize_u8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Full display/export pipeline for one pixel: accumulated linear sum -> 8-bit sRGB.
///
/// `sample_count` is the number of samples actually accumulated; a partial
/// render therefore averages over exactly the samples that exist.
pub fn linear_sum_to_srgb8(sum: [f32; 3], sample_count: u32, exposure: f32) -> [u8; 3] {
    let count = sample_count.max(1) as f32;
    let mut c = [
        sum[0] / count,
        sum[1] / count,
        sum[2] / count,
    ];
    for value in c.iter_mut() {
        *value *= exposure;
    }
    let toned = reinhard(c);
    let encoded = srgb_encode(toned);
    [
        quantize_u8(encoded[0]),
        quantize_u8(encoded[1]),
        quantize_u8(encoded[2]),
    ]
}

/// SHA-256 over decoded RGBA8 pixels; used to prove reproducible output.
pub fn hash_pixels_rgba8(pixels: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update((pixels.len() as u64).to_le_bytes());
    hasher.update(pixels);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reinhard_is_monotonic_and_bounded() {
        assert_eq!(reinhard_channel(0.0), 0.0);
        assert!((reinhard_channel(1.0) - 0.5).abs() < 1e-6);
        assert!(reinhard_channel(1.0e6) < 1.0);
        assert!(reinhard_channel(0.3) < reinhard_channel(0.4));
    }

    #[test]
    fn srgb_endpoints() {
        assert!((srgb_encode_channel(0.0)).abs() < 1e-6);
        assert!((srgb_encode_channel(1.0) - 1.0).abs() < 1e-6);
        // 0.0031308 is the linear/gamma breakpoint.
        assert!((srgb_encode_channel(0.0031308) - 0.04045).abs() < 1e-4);
    }

    #[test]
    fn average_is_applied_before_exposure() {
        let sum = [4.0f32, 2.0, 0.0];
        // 4 samples -> 1.0 average, exposure 1.0 -> tone mapped 0.5 -> sRGB 188
        let out = linear_sum_to_srgb8(sum, 4, 1.0);
        assert_eq!(out[0], quantize_u8(srgb_encode_channel(0.5)));
        // exposure 0.5 -> 0.5 average -> tone mapped 1/3
        let dim = linear_sum_to_srgb8(sum, 4, 0.5);
        assert_eq!(dim[0], quantize_u8(srgb_encode_channel(1.0 / 3.0)));
        // 2x exposure on a sum that averages 1.0 saturates towards 1
        let bright = linear_sum_to_srgb8([4.0, 4.0, 4.0], 2, 2.0);
        assert_eq!(bright[0], quantize_u8(srgb_encode_channel(0.8)));
    }

    #[test]
    fn black_stays_black() {
        assert_eq!(linear_sum_to_srgb8([0.0; 3], 8, 1.0), [0, 0, 0]);
    }
}
