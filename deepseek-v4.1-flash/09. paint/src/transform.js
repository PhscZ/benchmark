import { createCanvas, get2d } from './util.js';
import { MAX_DIMENSION, RESIZE_MIN } from './config.js';

/**
 * Whole-document geometric transforms.
 *
 * Every transform renders the complete current raster (base image, drawings and
 * applied adjustments alike) into a new canvas of the correct size and swaps it
 * into the document, so pixel content and document dimensions always agree.
 */

/**
 * Rotate 90 degrees. `direction` is 1 for clockwise and -1 for counter-clockwise.
 * @param {import('./doc.js').RasterDocument} doc
 * @param {1|-1} direction
 */
export function rotate90(doc, direction) {
  const source = doc.canvas;
  const width = doc.width;
  const height = doc.height;
  const target = createCanvas(height, width);
  const ctx = get2d(target);
  ctx.imageSmoothingEnabled = false;
  if (direction === 1) ctx.transform(0, 1, -1, 0, height, 0);
  else ctx.transform(0, -1, 1, 0, 0, width);
  ctx.drawImage(source, 0, 0);
  doc.replaceWith(target);
}

/** Rotate 180 degrees. */
export function rotate180(doc) {
  const source = doc.canvas;
  const target = createCanvas(doc.width, doc.height);
  const ctx = get2d(target);
  ctx.imageSmoothingEnabled = false;
  ctx.transform(-1, 0, 0, -1, doc.width, doc.height);
  ctx.drawImage(source, 0, 0);
  doc.replaceWith(target);
}

/**
 * Mirror horizontally by swapping left and right.
 * @param {import('./doc.js').RasterDocument} doc
 */
export function mirrorHorizontal(doc) {
  const source = doc.canvas;
  const target = createCanvas(doc.width, doc.height);
  const ctx = get2d(target);
  ctx.imageSmoothingEnabled = false;
  ctx.transform(-1, 0, 0, 1, doc.width, 0);
  ctx.drawImage(source, 0, 0);
  doc.replaceWith(target);
}

/**
 * Mirror vertically by swapping top and bottom.
 * @param {import('./doc.js').RasterDocument} doc
 */
export function mirrorVertical(doc) {
  const source = doc.canvas;
  const target = createCanvas(doc.width, doc.height);
  const ctx = get2d(target);
  ctx.imageSmoothingEnabled = false;
  ctx.transform(1, 0, 0, -1, 0, doc.height);
  ctx.drawImage(source, 0, 0);
  doc.replaceWith(target);
}

/**
 * Resize to exact pixel dimensions.
 * @param {import('./doc.js').RasterDocument} doc
 * @param {number} width @param {number} height
 * @param {{smooth?:boolean}} [options]
 */
export function resizeDocument(doc, width, height, options = {}) {
  const error = validateDimensions(width, height);
  if (error) return { ok: false, error, width: doc.width, height: doc.height };
  if (width === doc.width && height === doc.height) {
    return { ok: false, error: 'The new size is identical to the current size.', width, height };
  }
  const source = doc.canvas;
  const target = createCanvas(width, height);
  const ctx = get2d(target);
  ctx.imageSmoothingEnabled = options.smooth !== false;
  ctx.imageSmoothingQuality = 'high';
  ctx.drawImage(source, 0, 0, doc.width, doc.height, 0, 0, width, height);
  doc.replaceWith(target);
  return { ok: true, error: null, width, height };
}

/** @returns {string|null} message when the dimensions are not usable */
export function validateDimensions(width, height) {
  if (!Number.isFinite(width) || !Number.isFinite(height) || !Number.isInteger(width) || !Number.isInteger(height)) {
    return 'Width and height must be whole numbers of pixels.';
  }
  if (width < RESIZE_MIN || height < RESIZE_MIN) {
    return `Width and height must be at least ${RESIZE_MIN} pixel.`;
  }
  if (width > MAX_DIMENSION || height > MAX_DIMENSION) {
    return `Width and height must not exceed ${MAX_DIMENSION} pixels.`;
  }
  return null;
}

/**
 * Aspect-ratio helper for the resize fields: derive the other axis from the
 * current document ratio so locked resizing cannot drift.
 * @param {number} currentWidth @param {number} currentHeight
 * @param {'width'|'height'} changed axis the user typed
 * @param {number} value new value for that axis
 */
export function lockAspect(currentWidth, currentHeight, changed, value) {
  const ratio = currentWidth / currentHeight;
  if (changed === 'width') {
    return { width: value, height: Math.max(RESIZE_MIN, Math.round(value / ratio)) };
  }
  return { width: Math.max(RESIZE_MIN, Math.round(value * ratio)), height: value };
}
