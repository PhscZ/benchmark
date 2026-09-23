/** Small DOM/format helpers shared by the editor modules. */

export function clamp(value, lo, hi) {
  return value < lo ? lo : value > hi ? hi : value;
}

export function nextFrame() {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

/** OffscreenCanvas where available, detached <canvas> otherwise. */
export function createCanvas(width, height) {
  if (typeof OffscreenCanvas === 'function') return new OffscreenCanvas(width, height);
  const canvas = document.createElement('canvas');
  canvas.width = width;
  canvas.height = height;
  return canvas;
}

export function get2d(canvas) {
  const ctx = canvas.getContext('2d', { willReadFrequently: true });
  if (!ctx) throw new Error('2D canvas context unavailable');
  return ctx;
}

export function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

/** '#rrggbb' -> {r,g,b}; invalid input yields null. */
export function parseHexColor(hex) {
  const m = /^#?([0-9a-f]{6})$/i.exec(String(hex).trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return { r: (n >> 16) & 0xff, g: (n >> 8) & 0xff, b: n & 0xff };
}

export function rgbToHex(r, g, b) {
  const to2 = (v) => clamp(Math.round(v), 0, 255).toString(16).padStart(2, '0');
  return `#${to2(r)}${to2(g)}${to2(b)}`;
}

/** Strip path separators, dot runs and control characters so the name is safe to download. */
export function sanitizeFilename(name, fallback) {
  const cleaned = String(name || '')
    .replace(/[\\/:*?"<>|\u0000-\u001f]/g, '_')
    .replace(/\.{2,}/g, '_')
    .replace(/^[.\s]+|[.\s]+$/g, '')
    .trim();
  return /[a-z0-9]/i.test(cleaned) ? cleaned : fallback;
}

export function ensureExtension(name, extension) {
  const lower = name.toLowerCase();
  return lower.endsWith(`.${extension}`) ? name : `${name}.${extension}`;
}
