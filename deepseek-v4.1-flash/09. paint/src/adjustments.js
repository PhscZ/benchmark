import { applyAdjustments, DEFAULT_ADJUSTMENTS, isNeutral } from './adjust.js';
import { createCanvas, get2d, nextFrame } from './util.js';

/** Pixels processed per progress chunk; keeps long previews interruptible. */
const CHUNK_PIXELS = 250000;

/**
 * Adjustment preview.
 *
 * The preview is always recomputed from the *unmodified* document, so dragging a
 * slider never compounds the effect. The document itself is only touched when
 * `commit()` runs, which makes Cancel side-effect free.
 */
export class AdjustmentPreview {
  constructor() {
    this.canvas = null;
    this.ctx = null;
    this.active = false;
    this.adjustments = { ...DEFAULT_ADJUSTMENTS };
    this.revision = 0;
  }

  syncSize(doc) {
    if (this.canvas && (this.canvas.width !== doc.width || this.canvas.height !== doc.height)) this.release();
    if (!this.canvas) {
      this.canvas = createCanvas(doc.width, doc.height);
      this.ctx = get2d(this.canvas);
    }
  }

  release() {
    this.canvas = null;
    this.ctx = null;
    this.active = false;
  }

  get previewCanvas() { return this.active ? this.canvas : null; }

  /**
   * Recompute the preview from the document.
   * @param {import('./doc.js').RasterDocument} doc
   * @param {typeof DEFAULT_ADJUSTMENTS} adjustments
   * @param {(fraction:number)=>void} [onProgress]
   */
  async update(doc, adjustments, onProgress) {
    this.adjustments = { ...adjustments };
    if (isNeutral(this.adjustments)) {
      this.active = false;
      onProgress?.(1);
      return false;
    }
    this.syncSize(doc);
    const width = doc.width;
    const height = doc.height;
    const ctx = this.ctx;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = 'source-over';
    ctx.clearRect(0, 0, width, height);
    ctx.drawImage(doc.canvas, 0, 0);

    const image = ctx.getImageData(0, 0, width, height);
    const rowsPerChunk = Math.max(1, Math.floor(CHUNK_PIXELS / width));
    const revision = ++this.revision;
    for (let y = 0; y < height; y += rowsPerChunk) {
      const y1 = Math.min(height, y + rowsPerChunk);
      applyAdjustments(image.data, width, this.adjustments, y, y1);
      ctx.putImageData(image, 0, 0, 0, y, width, y1 - y);
      onProgress?.((y1 / height) * 0.98);
      await nextFrame();
      // A newer update superseded this one: stop early, its result wins.
      if (revision !== this.revision) return false;
    }
    this.active = true;
    onProgress?.(1);
    return true;
  }

  /** Apply the current preview pixels to the document. */
  commit(doc) {
    if (!this.active || !this.canvas) return false;
    doc.replaceWith(this.canvas);
    return true;
  }

  clear() {
    this.active = false;
    this.adjustments = { ...DEFAULT_ADJUSTMENTS };
    if (this.ctx) this.ctx.clearRect(0, 0, this.canvas.width, this.canvas.height);
  }
}
