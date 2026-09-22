/**
 * Image container metadata: format sniffing, stored dimensions, EXIF orientation,
 * and removal of orientation metadata before decoding.
 *
 * Why strip: browsers disagree about whether a decoder honours EXIF orientation
 * (Chromium applies it to JPEG/WebP even when `createImageBitmap` is called with
 * `imageOrientation: 'none'`). If the decoder orients the bitmap *and* we apply
 * the orientation ourselves, the image is rotated twice. Removing the orientation
 * tag before handing bytes to the decoder makes the decode result unambiguous on
 * every browser, so the editor's own transform is the single source of truth.
 */

export const EXIF_ORIENTATION_TAG = 0x0112;

/** @typedef {{format:'jpeg'|'png'|'webp', orientation:number, width:number, height:number}} ImageMeta */

function ascii(bytes, offset, length) {
  let out = '';
  for (let i = 0; i < length; i++) out += String.fromCharCode(bytes[offset + i]);
  return out;
}

function u16be(bytes, i) { return (bytes[i] << 8) | bytes[i + 1]; }
function u32be(bytes, i) { return bytes[i] * 0x1000000 + (bytes[i + 1] << 16) + (bytes[i + 2] << 8) + bytes[i + 3]; }
function u16le(bytes, i) { return bytes[i] | (bytes[i + 1] << 8); }
function u24le(bytes, i) { return bytes[i] | (bytes[i + 1] << 8) | (bytes[i + 2] << 16); }

/** @returns {'jpeg'|'png'|'webp'|null} */
export function sniffFormat(bytes) {
  if (!bytes || bytes.length < 4) return null;
  if (bytes[0] === 0xff && bytes[1] === 0xd8) return 'jpeg';
  if (bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47) return 'png';
  if (bytes.length >= 12 && ascii(bytes, 0, 4) === 'RIFF' && ascii(bytes, 8, 4) === 'WEBP') return 'webp';
  return null;
}

/**
 * Parse a TIFF header (as embedded in JPEG APP1, PNG eXIf, WebP EXIF) and return
 * the EXIF orientation (1 when absent/invalid).
 */
export function parseTiffOrientation(bytes, start, end) {
  if (end - start < 8) return 1;
  const order = ascii(bytes, start, 2);
  let le;
  if (order === 'II') le = true;
  else if (order === 'MM') le = false;
  else return 1;
  const rd16 = (i) => (le ? u16le(bytes, i) : u16be(bytes, i));
  const rd32 = (i) => (le ? u16le(bytes, i) | (u16le(bytes, i + 2) << 16) : u32be(bytes, i));
  if (rd16(start + 2) !== 0x002a) return 1;
  const ifd = start + rd32(start + 4);
  if (ifd + 2 > end) return 1;
  const count = rd16(ifd);
  for (let i = 0; i < count; i++) {
    const entry = ifd + 2 + i * 12;
    if (entry + 12 > end) break;
    if (rd16(entry) === EXIF_ORIENTATION_TAG) {
      const value = rd16(entry + 8);
      return value >= 1 && value <= 8 ? value : 1;
    }
  }
  return 1;
}

const SOF_MARKERS = new Set([0xc0, 0xc1, 0xc2, 0xc3, 0xc5, 0xc6, 0xc7, 0xc9, 0xca, 0xcb, 0xcd, 0xce, 0xcf]);

function scanJpeg(bytes) {
  const meta = { orientation: 1, width: 0, height: 0, exifSegments: [] };
  let off = 2;
  while (off + 3 < bytes.length) {
    if (bytes[off] !== 0xff) break;
    const marker = bytes[off + 1];
    if (marker === 0xff) { off++; continue; }
    if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd8)) { off += 2; continue; }
    if (marker === 0xda) break;
    const length = u16be(bytes, off + 2);
    if (length < 2) break;
    const payload = off + 4;
    const end = off + 2 + length;
    if (end > bytes.length) break;
    if (marker === 0xe1 && ascii(bytes, payload, 4) === 'Exif' && bytes[payload + 4] === 0) {
      meta.exifSegments.push([off, end]);
      const orientation = parseTiffOrientation(bytes, payload + 6, end);
      if (orientation !== 1) meta.orientation = orientation;
    } else if (SOF_MARKERS.has(marker) && meta.width === 0) {
      meta.height = u16be(bytes, payload + 1);
      meta.width = u16be(bytes, payload + 3);
    }
    off = end;
  }
  return meta;
}

function scanPng(bytes) {
  const meta = { orientation: 1, width: 0, height: 0, exifChunks: [] };
  let off = 8;
  while (off + 8 <= bytes.length) {
    const length = u32be(bytes, off);
    const type = ascii(bytes, off + 4, 4);
    const end = off + 12 + length;
    if (end > bytes.length) break;
    if (type === 'IHDR') {
      meta.width = u32be(bytes, off + 8);
      meta.height = u32be(bytes, off + 12);
    } else if (type === 'eXIf') {
      meta.exifChunks.push([off, end]);
      const orientation = parseTiffOrientation(bytes, off + 8, off + 8 + length);
      if (orientation !== 1) meta.orientation = orientation;
    }
    if (type === 'IEND') break;
    off = end;
  }
  return meta;
}

function scanWebp(bytes) {
  const meta = { orientation: 1, width: 0, height: 0, exifChunks: [] };
  let off = 12;
  while (off + 8 <= bytes.length) {
    const fourcc = ascii(bytes, off, 4);
    const size = u16le(bytes, off + 4) | (u16le(bytes, off + 6) << 16);
    const payload = off + 8;
    const end = payload + size;
    if (end > bytes.length) break;
    if (fourcc === 'VP8X') {
      meta.width = u24le(bytes, payload + 4) + 1;
      meta.height = u24le(bytes, payload + 7) + 1;
    } else if (fourcc === 'VP8 ') {
      meta.width = u16le(bytes, payload + 6) & 0x3fff;
      meta.height = u16le(bytes, payload + 8) & 0x3fff;
    } else if (fourcc === 'VP8L') {
      const bits = bytes[payload + 1] | (bytes[payload + 2] << 8) | (bytes[payload + 3] << 16) | (bytes[payload + 4] << 24);
      meta.width = (bits & 0x3fff) + 1;
      meta.height = ((bits >>> 14) & 0x3fff) + 1;
    } else if (fourcc === 'EXIF') {
      meta.exifChunks.push([off, end + (size % 2)]);
      const orientation = parseTiffOrientation(bytes, payload, end);
      if (orientation !== 1) meta.orientation = orientation;
    }
    off = end + (size % 2);
  }
  return meta;
}

/**
 * Container metadata: stored (pre-orientation) pixel dimensions and EXIF orientation.
 * @returns {ImageMeta|null} null when the bytes are not a supported container
 */
export function readImageMeta(bytes) {
  const format = sniffFormat(bytes);
  if (!format) return null;
  if (format === 'jpeg') {
    const s = scanJpeg(bytes);
    return { format, orientation: s.orientation, width: s.width, height: s.height };
  }
  if (format === 'png') {
    const s = scanPng(bytes);
    return { format, orientation: s.orientation, width: s.width, height: s.height };
  }
  const s = scanWebp(bytes);
  return { format, orientation: s.orientation, width: s.width, height: s.height };
}

function concatBytes(parts, total) {
  const out = new Uint8Array(total);
  let at = 0;
  for (const p of parts) { out.set(p, at); at += p.length; }
  return out;
}

function dropRanges(bytes, ranges) {
  if (!ranges.length) return bytes;
  const kept = [];
  let at = 0;
  let total = 0;
  for (const [start, end] of ranges) {
    if (start > at) { const part = bytes.subarray(at, start); kept.push(part); total += part.length; }
    at = Math.max(at, end);
  }
  if (at < bytes.length) { const part = bytes.subarray(at); kept.push(part); total += part.length; }
  return concatBytes(kept, total);
}

/**
 * Return bytes with orientation metadata removed so that decoding cannot apply it.
 * Dimensions and all other metadata are left untouched.
 */
export function stripOrientation(bytes) {
  const format = sniffFormat(bytes);
  if (!format) return bytes;
  if (format === 'jpeg') {
    return dropRanges(bytes, scanJpeg(bytes).exifSegments);
  }
  if (format === 'png') {
    return dropRanges(bytes, scanPng(bytes).exifChunks);
  }
  const ranges = scanWebp(bytes).exifChunks;
  if (!ranges.length) return bytes;
  const stripped = dropRanges(bytes, ranges);
  // The RIFF header's VP8X flags advertise the presence of an EXIF chunk; clear
  // that bit so the container stays self-consistent.
  let off = 12;
  while (off + 8 <= stripped.length) {
    const fourcc = ascii(stripped, off, 4);
    const size = u16le(stripped, off + 4) | (u16le(stripped, off + 6) << 16);
    if (fourcc === 'VP8X') stripped[off + 8] &= ~0x08;
    off += 8 + size + (size % 2);
  }
  const riffSize = stripped.length - 8;
  stripped[4] = riffSize & 0xff;
  stripped[5] = (riffSize >>> 8) & 0xff;
  stripped[6] = (riffSize >>> 16) & 0xff;
  stripped[7] = (riffSize >>> 24) & 0xff;
  return stripped;
}
