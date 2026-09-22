/**
 * Colour adjustments.
 *
 * Pure functions over straight-alpha RGBA byte buffers (the layout of ImageData),
 * with no DOM dependency so the formulas can be unit-tested in Node against the
 * reference values in test/fixtures/color_fixture.expected.json.
 *
 * Documented pipeline (see README "Adjustment formulas"), applied to the *base*
 * image each time so previews never compound:
 *
 *   brightness k in [-100,100]:  v' = clamp(round(v + k * 2.55))
 *   contrast   k in [-100,100]:  f = (100 + k) / 100 ; v' = clamp(round((v - 127.5) * f + 127.5))
 *   grayscale:                   Y = clamp(round(0.2126R + 0.7152G + 0.0722B)); R = G = B = Y
 *   invert:                      v' = 255 - v
 *
 * Rounding is half-up (Math.round), i.e. floor(v + 0.5) for the non-negative
 * values produced here. Alpha is never modified, so transparency is preserved.
 */

export const DEFAULT_ADJUSTMENTS = Object.freeze({
  brightness: 0,
  contrast: 0,
  grayscale: false,
  invert: false,
});

export function isNeutral(adjustments) {
  return !adjustments.brightness && !adjustments.contrast && !adjustments.grayscale && !adjustments.invert;
}

/**
 * Per-channel lookup table for the brightness/contrast stage. A single rounding
 * step happens at the end of the stage, matching the reference implementation.
 */
export function buildToneLut(brightness = 0, contrast = 0) {
  const lut = new Uint8ClampedArray(256);
  const factor = (100 + contrast) / 100;
  const offset = brightness * 2.55;
  for (let v = 0; v < 256; v++) {
    const tone = (v + offset - 127.5) * factor + 127.5;
    lut[v] = tone <= 0 ? 0 : tone >= 255 ? 255 : Math.floor(tone + 0.5);
  }
  return lut;
}

/**
 * Apply adjustments in place to rows [y0, y1) of an RGBA buffer.
 * Chunking by row keeps long operations interruptible for progress reporting.
 */
export function applyAdjustments(pixels, width, adjustments, y0 = 0, y1 = Infinity) {
  const { brightness = 0, contrast = 0, grayscale = false, invert = false } = adjustments;
  const height = Math.floor(pixels.length / (width * 4));
  const rowStart = Math.max(0, y0);
  const rowEnd = Math.min(height, y1);
  const lut = buildToneLut(brightness, contrast);
  const hasTone = brightness !== 0 || contrast !== 0;

  for (let y = rowStart; y < rowEnd; y++) {
    let i = y * width * 4;
    for (let x = 0; x < width; x++, i += 4) {
      let r = pixels[i];
      let g = pixels[i + 1];
      let b = pixels[i + 2];
      if (hasTone) { r = lut[r]; g = lut[g]; b = lut[b]; }
      if (grayscale) {
        const y709 = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        r = g = b = y709 <= 0 ? 0 : y709 >= 255 ? 255 : Math.floor(y709 + 0.5);
      }
      if (invert) { r = 255 - r; g = 255 - g; b = 255 - b; }
      pixels[i] = r;
      pixels[i + 1] = g;
      pixels[i + 2] = b;
      // alpha (i + 3) intentionally untouched
    }
  }
  return pixels;
}
