import { readImageMeta, stripOrientation, sniffFormat } from './exif.js';
import { RasterDocument } from './doc.js';
import { createCanvas, get2d, formatBytes } from './util.js';
import { MAX_DIMENSION, MAX_FILE_BYTES } from './config.js';

const MIME_BY_FORMAT = {
  jpeg: 'image/jpeg',
  png: 'image/png',
  webp: 'image/webp',
};

/** Error type for problems that must leave the currently open image untouched. */
export class ImageLoadError extends Error {
  constructor(message) {
    super(message);
    this.name = 'ImageLoadError';
  }
}

/**
 * Decode bytes into a bitmap, preferring createImageBitmap and falling back to
 * an <img> element (object URL revoked immediately after decode).
 * @param {Uint8Array} bytes
 * @param {string} mime
 */
async function decodeToBitmap(bytes, mime) {
  const blob = new Blob([bytes], { type: mime });
  try {
    return await createImageBitmap(blob);
  } catch (bitmapError) {
    const url = URL.createObjectURL(blob);
    try {
      const img = new Image();
      img.decoding = 'sync';
      img.src = url;
      await img.decode();
      return img;
    } catch (imgError) {
      throw new ImageLoadError('Corrupted or unreadable image data.');
    } finally {
      URL.revokeObjectURL(url);
    }
  }
}

/**
 * Orientation transform for EXIF values 1..8, mapping the *stored* raster (which
 * is what we decode after stripping the orientation tag) onto the upright image.
 * @param {CanvasRenderingContext2D} ctx
 * @param {number} orientation
 * @param {number} storedWidth
 * @param {number} storedHeight
 */
function applyOrientation(ctx, orientation, storedWidth, storedHeight) {
  const w = storedWidth;
  const h = storedHeight;
  switch (orientation) {
    case 2: ctx.transform(-1, 0, 0, 1, w, 0); break;
    case 3: ctx.transform(-1, 0, 0, -1, w, h); break;
    case 4: ctx.transform(1, 0, 0, -1, 0, h); break;
    case 5: ctx.transform(0, 1, 1, 0, 0, 0); break;
    case 6: ctx.transform(0, 1, -1, 0, h, 0); break;
    case 7: ctx.transform(0, -1, -1, 0, h, w); break;
    case 8: ctx.transform(0, -1, 1, 0, 0, w); break;
    default: break;
  }
}

function orientedDimensions(orientation, width, height) {
  return orientation >= 5 && orientation <= 8 ? [height, width] : [width, height];
}

/**
 * Load a user-selected file into a fresh RasterDocument.
 *
 * Throws ImageLoadError (leaving any existing document untouched) for
 * unsupported containers, oversized images and corrupted data.
 *
 * @param {File|Blob} file
 * @returns {Promise<{doc: RasterDocument, info: {name:string, format:string, orientation:number, storedWidth:number, storedHeight:number, width:number, height:number, bytes:number, warnings:string[]}}>}
 */
export async function loadImageFile(file) {
  const name = file.name || 'image';
  if (file.size === 0) throw new ImageLoadError(`"${name}" is empty.`);
  if (file.size > MAX_FILE_BYTES) {
    throw new ImageLoadError(`"${name}" is ${formatBytes(file.size)}; the limit is ${formatBytes(MAX_FILE_BYTES)}.`);
  }

  const bytes = new Uint8Array(await file.arrayBuffer());
  const format = sniffFormat(bytes);
  if (!format) {
    throw new ImageLoadError(`"${name}" is not a JPEG, PNG or WebP file.`);
  }

  const meta = readImageMeta(bytes);
  if (!meta || !meta.width || !meta.height) {
    throw new ImageLoadError(`"${name}" has an unreadable ${format.toUpperCase()} header.`);
  }

  const orientation = meta.orientation;
  const [outWidth, outHeight] = orientedDimensions(orientation, meta.width, meta.height);
  if (outWidth > MAX_DIMENSION || outHeight > MAX_DIMENSION) {
    throw new ImageLoadError(
      `"${name}" is ${outWidth}x${outHeight} pixels; the supported maximum is ${MAX_DIMENSION}x${MAX_DIMENSION}.`,
    );
  }

  // Remove orientation metadata so the decoder cannot rotate the pixels as well;
  // the editor applies the EXIF orientation exactly once, below.
  const stripped = stripOrientation(bytes);
  const bitmap = await decodeToBitmap(stripped, MIME_BY_FORMAT[format]);

  const warnings = [];
  const decodedWidth = bitmap.width;
  const decodedHeight = bitmap.height;
  const matchesStored = decodedWidth === meta.width && decodedHeight === meta.height;
  const matchesOriented = decodedWidth === outWidth && decodedHeight === outHeight;
  let effectiveOrientation = orientation;
  if (!matchesStored && !matchesOriented) {
    warnings.push(`Decoded size ${decodedWidth}x${decodedHeight} disagrees with the header (${meta.width}x${meta.height}); EXIF orientation was ignored.`);
    effectiveOrientation = 1;
  }

  const [canvasWidth, canvasHeight] = orientedDimensions(
    effectiveOrientation, decodedWidth, decodedHeight,
  );
  const canvas = createCanvas(canvasWidth, canvasHeight);
  const ctx = get2d(canvas);
  ctx.imageSmoothingEnabled = false;
  applyOrientation(ctx, effectiveOrientation, decodedWidth, decodedHeight);
  ctx.drawImage(bitmap, 0, 0);
  bitmap.close?.();

  const doc = RasterDocument.fromCanvas(canvas);
  return {
    doc,
    info: {
      name,
      format,
      orientation: effectiveOrientation,
      storedWidth: decodedWidth,
      storedHeight: decodedHeight,
      width: doc.width,
      height: doc.height,
      bytes: file.size,
      warnings,
    },
  };
}
