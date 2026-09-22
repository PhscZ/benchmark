import { createCanvas, get2d } from './util.js';
import { MAX_DIMENSION } from './config.js';

/**
 * The single raster document: one canvas holding the full-resolution RGBA image.
 * Every mutating operation reads and writes this canvas; the viewport only ever
 * *draws* it, so view transforms can never modify pixel content.
 */
export class RasterDocument {
  /** @param {number} width @param {number} height */
  constructor(width, height) {
    RasterDocument.assertDimensions(width, height);
    this.canvas = createCanvas(width, height);
    this.ctx = get2d(this.canvas);
  }

  static assertDimensions(width, height) {
    const ok = Number.isInteger(width) && Number.isInteger(height)
      && width >= 1 && height >= 1 && width <= MAX_DIMENSION && height <= MAX_DIMENSION;
    if (!ok) {
      throw new RangeError(`Unsupported document size ${width}x${height}: width and height must be whole numbers between 1 and ${MAX_DIMENSION}.`);
    }
  }

  /** Wrap an existing canvas (already at document resolution) without copying. */
  static fromCanvas(canvas) {
    const doc = Object.create(RasterDocument.prototype);
    doc.canvas = canvas;
    doc.ctx = get2d(canvas);
    return doc;
  }

  /** Copy another document or canvas into a fresh document. */
  static copyOf(source) {
    const src = source.canvas ?? source;
    const doc = new RasterDocument(src.width, src.height);
    doc.ctx.drawImage(src, 0, 0);
    return doc;
  }

  get width() { return this.canvas.width; }
  get height() { return this.canvas.height; }
  get pixelCount() { return this.canvas.width * this.canvas.height; }

  /** @returns {{width:number,height:number,data:ImageData}} exact RGBA snapshot */
  snapshot() {
    return {
      width: this.width,
      height: this.height,
      data: this.ctx.getImageData(0, 0, this.width, this.height),
    };
  }

  /** Restore a snapshot, resizing the canvas when the snapshot has other dimensions. */
  applySnapshot(snapshot) {
    RasterDocument.assertDimensions(snapshot.width, snapshot.height);
    if (this.canvas.width !== snapshot.width || this.canvas.height !== snapshot.height) {
      this.canvas.width = snapshot.width;
      this.canvas.height = snapshot.height;
    }
    this.resetState();
    this.ctx.putImageData(snapshot.data, 0, 0);
  }

  /** Restore 2D context defaults (canvas resize and reuse both need this). */
  resetState() {
    this.ctx.setTransform(1, 0, 0, 1, 0, 0);
    this.ctx.globalAlpha = 1;
    this.ctx.globalCompositeOperation = 'source-over';
    this.ctx.filter = 'none';
    this.ctx.imageSmoothingEnabled = true;
  }

  /** Replace all pixels with a canvas of the given dimensions. */
  replaceWith(source) {
    const src = source.canvas ?? source;
    RasterDocument.assertDimensions(src.width, src.height);
    this.canvas.width = src.width;
    this.canvas.height = src.height;
    this.resetState();
    this.ctx.drawImage(src, 0, 0);
  }

  clear() {
    this.resetState();
    this.ctx.clearRect(0, 0, this.width, this.height);
  }

  /** Read a single pixel as straight-alpha RGBA (0,0,0,0 outside the canvas). */
  sample(x, y) {
    const px = Math.floor(x);
    const py = Math.floor(y);
    if (px < 0 || py < 0 || px >= this.width || py >= this.height) return null;
    const d = this.ctx.getImageData(px, py, 1, 1).data;
    return { r: d[0], g: d[1], b: d[2], a: d[3] };
  }
}
