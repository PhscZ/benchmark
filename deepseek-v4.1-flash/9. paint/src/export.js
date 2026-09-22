import { createCanvas, get2d, ensureExtension, sanitizeFilename, parseHexColor } from './util.js';
import { EXPORT_BACKGROUND_DEFAULT, JPEG_QUALITY_DEFAULT } from './config.js';

export const EXPORT_FORMATS = {
  png: { id: 'png', label: 'PNG', mime: 'image/png', extension: 'png', lossy: false },
  jpeg: { id: 'jpeg', label: 'JPEG', mime: 'image/jpeg', extension: 'jpg', lossy: true },
};

/** Encode a canvas through whichever blob API it supports. */
async function encodeCanvas(canvas, mime, quality) {
  if (typeof canvas.convertToBlob === 'function') {
    return canvas.convertToBlob({ type: mime, quality });
  }
  return new Promise((resolve, reject) => {
    canvas.toBlob(
      (blob) => (blob ? resolve(blob) : reject(new Error(`Encoding to ${mime} failed.`))),
      mime,
      quality,
    );
  });
}

/**
 * Encode the full document at its own resolution.
 *
 * The document canvas holds only image pixels: selection handles, the crop
 * overlay, the checkerboard and the brush cursor live in the viewport canvases
 * and can therefore never appear in an export. Encoding is a straight read of
 * the document, so exported dimensions always equal the document dimensions.
 *
 * PNG keeps the alpha channel. JPEG cannot store alpha, so transparency is
 * flattened onto the selected background colour (white by default).
 *
 * @param {import('./doc.js').RasterDocument} doc
 * @param {{format:'png'|'jpeg', quality?:number, background?:string, filename?:string}} options
 * @returns {Promise<{blob:Blob,filename:string,width:number,height:number,format:string,type:string}>}
 */
export async function exportDocument(doc, options) {
  const format = EXPORT_FORMATS[options.format];
  if (!format) throw new RangeError(`Unsupported export format "${options.format}".`);

  const width = doc.width;
  const height = doc.height;
  const baseName = sanitizeFilename(options.filename, 'edited-image');
  const filename = ensureExtension(baseName, format.extension);

  let source = doc.canvas;
  if (format.lossy) {
    const background = parseHexColor(options.background ?? EXPORT_BACKGROUND_DEFAULT);
    if (!background) throw new RangeError(`Invalid background colour "${options.background}".`);
    const flattened = createCanvas(width, height);
    const ctx = get2d(flattened);
    ctx.fillStyle = `rgb(${background.r}, ${background.g}, ${background.b})`;
    ctx.fillRect(0, 0, width, height);
    ctx.drawImage(doc.canvas, 0, 0);
    source = flattened;
  }

  const blob = await encodeCanvas(source, format.mime, options.quality ?? JPEG_QUALITY_DEFAULT);
  if (!blob || blob.size === 0) throw new Error(`Encoding to ${format.label} produced no data.`);

  return { blob, filename, width, height, format: format.id, type: format.mime };
}

/**
 * Hand a blob to the browser as a download. The object URL is revoked as soon as
 * the download has been triggered so nothing leaks between exports.
 * @param {Blob} blob @param {string} filename
 */
export function downloadBlob(blob, filename) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = filename;
  link.rel = 'noopener';
  document.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
  return url;
}

/** Verify the encoder actually produced an image with the expected dimensions. */
export async function verifyEncodedDimensions(blob, expectedWidth, expectedHeight) {
  let bitmap;
  try {
    bitmap = await createImageBitmap(blob);
  } catch {
    return { ok: false, error: 'The exported file could not be decoded again.' };
  }
  const { width, height } = bitmap;
  bitmap.close?.();
  if (width !== expectedWidth || height !== expectedHeight) {
    return { ok: false, error: `Exported dimensions are ${width}x${height}, expected ${expectedWidth}x${expectedHeight}.` };
  }
  return { ok: true, error: null };
}
