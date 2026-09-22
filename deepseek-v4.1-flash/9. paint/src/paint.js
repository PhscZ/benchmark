import { createCanvas, get2d } from './util.js';

/** Tools that produce a single geometric shape instead of a freehand stroke. */
export const SHAPE_TOOLS = new Set(['line', 'rect', 'ellipse']);
export const PAINT_TOOLS = new Set(['brush', 'eraser', 'line', 'rect', 'ellipse']);

const LABELS = {
  brush: 'Brush',
  eraser: 'Eraser',
  line: 'Line',
  rect: 'Rectangle',
  ellipse: 'Ellipse',
};

/**
 * Drawing engine.
 *
 * Strokes are rendered into a scratch layer at document resolution with full
 * alpha, then composited onto the document exactly once with the tool opacity.
 * That keeps a self-overlapping freehand stroke at a uniform opacity, and makes
 * the eraser a true `destination-out` removal to transparency.
 *
 * Brush sizes are expressed in image pixels, so a stroke covers the same image
 * area regardless of the viewport zoom.
 *
 * Live preview uses a second scratch canvas holding `document + layer` for the
 * affected region, computed with the same composite operation as the commit, so
 * what is shown during a drag is exactly what lands in the document.
 */
export class PaintEngine {
  constructor() {
    /** @type {import('./doc.js').RasterDocument|null} */
    this.doc = null;
    this.layer = null;
    this.layerCtx = null;
    this.preview = null;
    this.previewCtx = null;
    this.active = false;
    this.tool = null;
    this.options = null;
    this.start = null;
    this.last = null;
    this.dirty = null;
  }

  get isActive() { return this.active; }

  /** Canvas that should be displayed while a stroke is in progress. */
  get previewCanvas() { return this.active ? this.preview : null; }

  /** (Re)allocate scratch canvases when the document dimensions change. */
  syncSize(doc) {
    if (this.layer && (this.layer.width !== doc.width || this.layer.height !== doc.height)) this.release();
    if (this.layer) return;
    this.layer = createCanvas(doc.width, doc.height);
    this.layerCtx = get2d(this.layer);
    this.preview = createCanvas(doc.width, doc.height);
    this.previewCtx = get2d(this.preview);
  }

  /** Free scratch memory (document replaced, or the editor is torn down). */
  release() {
    this.layer = null;
    this.layerCtx = null;
    this.preview = null;
    this.previewCtx = null;
    this.active = false;
    this.dirty = null;
  }

  expandDirty(x, y, width, height) {
    const x0 = Math.floor(x);
    const y0 = Math.floor(y);
    const x1 = Math.ceil(x + width);
    const y1 = Math.ceil(y + height);
    if (!this.dirty) this.dirty = { x: x0, y: y0, x1, y1 };
    else {
      this.dirty.x = Math.min(this.dirty.x, x0);
      this.dirty.y = Math.min(this.dirty.y, y0);
      this.dirty.x1 = Math.max(this.dirty.x1, x1);
      this.dirty.y1 = Math.max(this.dirty.y1, y1);
    }
    this.dirty.x = Math.max(0, this.dirty.x);
    this.dirty.y = Math.max(0, this.dirty.y);
    this.dirty.x1 = Math.min(this.doc.width, this.dirty.x1);
    this.dirty.y1 = Math.min(this.doc.height, this.dirty.y1);
  }

  dirtyRect() {
    if (!this.dirty) return null;
    const { x, y, x1, y1 } = this.dirty;
    const width = x1 - x;
    const height = y1 - y;
    if (width <= 0 || height <= 0) return null;
    return { x, y, width, height };
  }

  /**
   * @param {{doc:import('./doc.js').RasterDocument, tool:string, point:{x:number,y:number},
   *          color:string, size:number, opacity:number, shapeMode:'outline'|'fill'|'both'}} params
   */
  begin(params) {
    const { doc, tool, point, color, size, opacity, shapeMode } = params;
    this.syncSize(doc);
    this.doc = doc;
    this.tool = tool;
    this.options = { color, size, opacity, shapeMode };
    this.start = { x: point.x, y: point.y };
    this.last = { x: point.x, y: point.y };
    this.dirty = null;
    this.active = true;

    const ctx = this.layerCtx;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = 'source-over';
    ctx.clearRect(0, 0, this.layer.width, this.layer.height);
    ctx.lineCap = 'round';
    ctx.lineJoin = 'round';

    // Seed the preview with the pristine document so every region is valid.
    const previewCtx = this.previewCtx;
    previewCtx.setTransform(1, 0, 0, 1, 0, 0);
    previewCtx.globalAlpha = 1;
    previewCtx.globalCompositeOperation = 'source-over';
    previewCtx.clearRect(0, 0, this.preview.width, this.preview.height);
    previewCtx.drawImage(doc.canvas, 0, 0);

    if (SHAPE_TOOLS.has(tool)) this.drawShape(point);
    else this.stampDot(point);
  }

  stampDot(point) {
    const ctx = this.layerCtx;
    const radius = this.options.size / 2;
    ctx.fillStyle = this.options.color;
    ctx.beginPath();
    ctx.arc(point.x, point.y, Math.max(radius, 0.5), 0, Math.PI * 2);
    ctx.fill();
    this.expandDirty(point.x - radius - 1, point.y - radius - 1, radius * 2 + 2, radius * 2 + 2);
    this.compositeRegion(this.lastSegmentRect(point, point));
  }

  lastSegmentRect(from, to) {
    const radius = this.options.size / 2 + 1;
    const x = Math.min(from.x, to.x) - radius;
    const y = Math.min(from.y, to.y) - radius;
    return { x, y, width: Math.abs(to.x - from.x) + radius * 2, height: Math.abs(to.y - from.y) + radius * 2 };
  }

  strokeSegment(from, to) {
    const ctx = this.layerCtx;
    ctx.strokeStyle = this.options.color;
    ctx.lineWidth = this.options.size;
    ctx.beginPath();
    ctx.moveTo(from.x, from.y);
    ctx.lineTo(to.x, to.y);
    ctx.stroke();
  }

  /** Constrain shape geometry to squares/circles or 45-degree lines. */
  constrain(point) {
    const start = this.start;
    const dx = point.x - start.x;
    const dy = point.y - start.y;
    if (this.tool === 'line') {
      const angle = Math.round(Math.atan2(dy, dx) / (Math.PI / 4)) * (Math.PI / 4);
      const length = Math.hypot(dx, dy);
      return { x: start.x + Math.cos(angle) * length, y: start.y + Math.sin(angle) * length };
    }
    const size = Math.max(Math.abs(dx), Math.abs(dy));
    return { x: start.x + Math.sign(dx || 1) * size, y: start.y + Math.sign(dy || 1) * size };
  }

  shapeGeometry(point) {
    const start = this.start;
    if (this.tool === 'line') return { from: start, to: point };
    const x = Math.min(start.x, point.x);
    const y = Math.min(start.y, point.y);
    return { x, y, width: Math.abs(point.x - start.x), height: Math.abs(point.y - start.y) };
  }

  drawShape(point) {
    const ctx = this.layerCtx;
    const { size, color, shapeMode } = this.options;
    ctx.clearRect(0, 0, this.layer.width, this.layer.height);
    ctx.lineWidth = size;
    ctx.strokeStyle = color;
    ctx.fillStyle = color;
    ctx.lineCap = 'round';
    ctx.lineJoin = 'round';

    const geometry = this.shapeGeometry(point);
    const pad = size / 2 + 1;
    ctx.beginPath();
    if (this.tool === 'line') {
      ctx.moveTo(geometry.from.x, geometry.from.y);
      ctx.lineTo(geometry.to.x, geometry.to.y);
      ctx.stroke();
      this.expandDirty(
        Math.min(geometry.from.x, geometry.to.x) - pad,
        Math.min(geometry.from.y, geometry.to.y) - pad,
        Math.abs(geometry.to.x - geometry.from.x) + pad * 2,
        Math.abs(geometry.to.y - geometry.from.y) + pad * 2,
      );
    } else if (this.tool === 'rect') {
      ctx.rect(geometry.x, geometry.y, geometry.width, geometry.height);
    } else {
      ctx.ellipse(
        geometry.x + geometry.width / 2,
        geometry.y + geometry.height / 2,
        Math.max(geometry.width / 2, 0.001),
        Math.max(geometry.height / 2, 0.001),
        0, 0, Math.PI * 2,
      );
    }
    if (this.tool === 'rect' || this.tool === 'ellipse') {
      if (shapeMode === 'fill' || shapeMode === 'both') ctx.fill();
      if (shapeMode === 'outline' || shapeMode === 'both') ctx.stroke();
      this.expandDirty(
        geometry.x - pad, geometry.y - pad,
        geometry.width + pad * 2, geometry.height + pad * 2,
      );
    }
  }

  /** Recompute `preview = document (op) layer` for one image-space rectangle. */
  compositeRegion(rect) {
    const cx = this.previewCtx;
    if (!cx || !rect) return;
    const x = Math.max(0, Math.floor(rect.x));
    const y = Math.max(0, Math.floor(rect.y));
    const width = Math.min(this.doc.width - x, Math.ceil(rect.width + (rect.x - x)));
    const height = Math.min(this.doc.height - y, Math.ceil(rect.height + (rect.y - y)));
    if (width <= 0 || height <= 0) return;
    cx.setTransform(1, 0, 0, 1, 0, 0);
    cx.globalCompositeOperation = 'source-over';
    cx.globalAlpha = 1;
    cx.clearRect(x, y, width, height);
    cx.drawImage(this.doc.canvas, x, y, width, height, x, y, width, height);
    cx.globalAlpha = this.options.opacity;
    cx.globalCompositeOperation = this.tool === 'eraser' ? 'destination-out' : 'source-over';
    cx.drawImage(this.layer, x, y, width, height, x, y, width, height);
    cx.globalAlpha = 1;
    cx.globalCompositeOperation = 'source-over';
  }

  /** @param {{x:number,y:number}} point @param {boolean} [constrain] */
  move(point, constrain = false) {
    if (!this.active) return;
    const target = constrain && SHAPE_TOOLS.has(this.tool) ? this.constrain(point) : point;
    if (SHAPE_TOOLS.has(this.tool)) {
      this.drawShape(target);
      this.compositeRegion(this.dirtyRect());
      this.last = target;
      return;
    }
    this.strokeSegment(this.last, target);
    this.compositeRegion(this.lastSegmentRect(this.last, target));
    this.expandDirty(
      Math.min(this.last.x, target.x) - this.options.size / 2 - 1,
      Math.min(this.last.y, target.y) - this.options.size / 2 - 1,
      Math.abs(target.x - this.last.x) + this.options.size + 2,
      Math.abs(target.y - this.last.y) + this.options.size + 2,
    );
    this.last = target;
  }

  /**
   * Finish the stroke and apply it to the document.
   * @returns {{changed:boolean,label:string,rect:object|null}}
   */
  end() {
    if (!this.active) return { changed: false, label: '', rect: null };
    const rect = this.dirtyRect();
    const tool = this.tool;
    this.active = false;
    if (!rect) return { changed: false, label: LABELS[tool] ?? tool, rect: null };

    const ctx = this.doc.ctx;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalAlpha = this.options.opacity;
    ctx.globalCompositeOperation = tool === 'eraser' ? 'destination-out' : 'source-over';
    ctx.drawImage(this.layer, rect.x, rect.y, rect.width, rect.height, rect.x, rect.y, rect.width, rect.height);
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = 'source-over';
    this.dirty = null;
    return { changed: true, label: LABELS[tool] ?? tool, rect };
  }

  /** Abandon the in-progress stroke without touching the document. */
  cancel() {
    if (!this.active) return;
    this.active = false;
    this.dirty = null;
    this.layerCtx.clearRect(0, 0, this.layer.width, this.layer.height);
  }

  /**
   * Tool cursor overlay in screen space: a ring matching the brush footprint.
   * @param {CanvasRenderingContext2D} ctx @param {import('./viewport.js').Viewport} viewport
   * @param {{x:number,y:number}} imagePoint @param {number} size brush size in image pixels
   */
  static drawCursor(ctx, viewport, imagePoint, size) {
    const radius = Math.max(2, (size / 2) * viewport.scale);
    const screen = viewport.imageToScreen(imagePoint.x, imagePoint.y);
    ctx.save();
    ctx.beginPath();
    ctx.arc(screen.x, screen.y, radius, 0, Math.PI * 2);
    ctx.lineWidth = 1;
    ctx.strokeStyle = 'rgba(0,0,0,0.75)';
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(screen.x, screen.y, Math.max(1, radius - 1), 0, Math.PI * 2);
    ctx.strokeStyle = 'rgba(255,255,255,0.9)';
    ctx.stroke();
    ctx.beginPath();
    ctx.moveTo(screen.x - 6, screen.y);
    ctx.lineTo(screen.x + 6, screen.y);
    ctx.moveTo(screen.x, screen.y - 6);
    ctx.lineTo(screen.x, screen.y + 6);
    ctx.strokeStyle = 'rgba(255,255,255,0.6)';
    ctx.stroke();
    ctx.restore();
  }
}
