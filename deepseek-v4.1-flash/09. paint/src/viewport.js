import { CHECKER_DARK, CHECKER_LIGHT, CHECKER_SIZE, ZOOM_MAX, ZOOM_MIN, ZOOM_STEP } from './config.js';
import { clamp, createCanvas, get2d } from './util.js';

/**
 * Viewport: maps image pixels <-> screen pixels and renders the document.
 *
 * The viewport never writes to the document canvas, so zooming, panning and
 * window resizing cannot modify the image or create undo steps.
 *
 * Transform model (CSS pixels, origin at the canvas top-left):
 *   screen = image * scale + offset
 *   image  = (screen - offset) / scale
 */
export class Viewport {
  /**
   * @param {HTMLCanvasElement} canvas image layer
   * @param {HTMLCanvasElement} overlayCanvas interaction layer drawn above it
   * @param {HTMLElement} container
   */
  constructor(canvas, overlayCanvas, container) {
    this.canvas = canvas;
    this.overlayCanvas = overlayCanvas;
    this.container = container;
    this.ctx = get2d(canvas);
    this.overlayCtx = get2d(overlayCanvas);
    this.scale = 1;
    this.offsetX = 0;
    this.offsetY = 0;
    this.viewWidth = 1;
    this.viewHeight = 1;
    this.dpr = 1;
    /** @type {import('./doc.js').RasterDocument|null} */
    this.doc = null;
    /** Extra image-space layers drawn above the document (pending stroke, preview). */
    this.layers = [];
    /** @type {((ctx:CanvasRenderingContext2D, viewport:Viewport)=>void)|null} */
    this.overlay = null;
    this.onTransform = null;
    this.checkerPattern = null;
    this.frameHandle = 0;
    this.overlayFrameHandle = 0;
    this.observer = new ResizeObserver(() => { this.resize(); });
    this.observer.observe(container);
    this.resize();
  }

  /** @param {import('./doc.js').RasterDocument} doc */
  setDocument(doc) {
    this.doc = doc;
    this.fit();
  }

  /** Measure the container, size the backing store for the device pixel ratio. */
  resize() {
    const rect = this.container.getBoundingClientRect();
    const width = Math.max(1, Math.round(rect.width));
    const height = Math.max(1, Math.round(rect.height));
    const dpr = Math.max(1, window.devicePixelRatio || 1);
    this.viewWidth = width;
    this.viewHeight = height;
    this.dpr = dpr;
    const backingWidth = Math.round(width * dpr);
    const backingHeight = Math.round(height * dpr);
    if (this.canvas.width !== backingWidth || this.canvas.height !== backingHeight) {
      this.canvas.width = backingWidth;
      this.canvas.height = backingHeight;
    }
    if (this.overlayCanvas.width !== backingWidth || this.overlayCanvas.height !== backingHeight) {
      this.overlayCanvas.width = backingWidth;
      this.overlayCanvas.height = backingHeight;
    }
    this.checkerPattern = null;
    this.clampPan();
    this.drawAll();
  }

  get imageWidth() { return this.doc ? this.doc.width : 0; }
  get imageHeight() { return this.doc ? this.doc.height : 0; }

  /** Screen-space rectangle occupied by the image, in CSS pixels. */
  get imageRect() {
    return {
      x: this.offsetX,
      y: this.offsetY,
      width: this.imageWidth * this.scale,
      height: this.imageHeight * this.scale,
    };
  }

  /** @param {number} clientX @param {number} clientY */
  screenToImage(clientX, clientY) {
    const rect = this.canvas.getBoundingClientRect();
    const sx = clientX - rect.left;
    const sy = clientY - rect.top;
    return {
      x: (sx - this.offsetX) / this.scale,
      y: (sy - this.offsetY) / this.scale,
      screenX: sx,
      screenY: sy,
    };
  }

  /** @param {number} x @param {number} y image coordinates */
  imageToScreen(x, y) {
    return { x: x * this.scale + this.offsetX, y: y * this.scale + this.offsetY };
  }

  /** Image coordinates -> viewport-relative CSS pixels (used by the e2e harness). */
  imageToCanvasPoint(x, y) {
    const rect = this.canvas.getBoundingClientRect();
    const p = this.imageToScreen(x, y);
    return { x: p.x + rect.left, y: p.y + rect.top };
  }

  setScale(nextScale, anchorScreenX, anchorScreenY) {
    const scale = clamp(nextScale, ZOOM_MIN, ZOOM_MAX);
    if (!this.doc) { this.scale = scale; this.drawAll(); return; }
    const ax = anchorScreenX ?? this.viewWidth / 2;
    const ay = anchorScreenY ?? this.viewHeight / 2;
    const imageX = (ax - this.offsetX) / this.scale;
    const imageY = (ay - this.offsetY) / this.scale;
    this.scale = scale;
    this.offsetX = ax - imageX * scale;
    this.offsetY = ay - imageY * scale;
    this.clampPan();
    this.drawAll();
    this.onTransform?.();
  }

  zoomBy(factor, anchorScreenX, anchorScreenY) {
    this.setScale(this.scale * factor, anchorScreenX, anchorScreenY);
  }

  /** Zoom in/out by one step, anchored at the viewport centre. */
  zoomStep(direction) {
    this.setScale(direction > 0 ? this.scale * ZOOM_STEP : this.scale / ZOOM_STEP);
  }

  fit(padding = 24) {
    if (!this.doc) { this.drawAll(); return; }
    const availWidth = Math.max(1, this.viewWidth - padding * 2);
    const availHeight = Math.max(1, this.viewHeight - padding * 2);
    const scale = clamp(Math.min(availWidth / this.imageWidth, availHeight / this.imageHeight), ZOOM_MIN, ZOOM_MAX);
    this.scale = scale;
    this.center();
    this.drawAll();
    this.onTransform?.();
  }

  actualSize() {
    if (!this.doc) { this.drawAll(); return; }
    const centreX = this.viewWidth / 2;
    const centreY = this.viewHeight / 2;
    const imageX = (centreX - this.offsetX) / this.scale;
    const imageY = (centreY - this.offsetY) / this.scale;
    this.scale = 1;
    this.offsetX = centreX - imageX;
    this.offsetY = centreY - imageY;
    this.clampPan();
    this.drawAll();
    this.onTransform?.();
  }

  center() {
    this.offsetX = (this.viewWidth - this.imageWidth * this.scale) / 2;
    this.offsetY = (this.viewHeight - this.imageHeight * this.scale) / 2;
  }

  /** Pan by a screen-space delta (CSS pixels). */
  panBy(dx, dy) {
    this.offsetX += dx;
    this.offsetY += dy;
    this.clampPan();
    this.drawAll();
    this.onTransform?.();
  }

  /** Keep the image reachable: centre an axis that fits, clamp one that overflows. */
  clampPan() {
    if (!this.doc) return;
    const scaledWidth = this.imageWidth * this.scale;
    const scaledHeight = this.imageHeight * this.scale;
    const margin = 40;
    if (scaledWidth <= this.viewWidth) {
      this.offsetX = (this.viewWidth - scaledWidth) / 2;
    } else {
      this.offsetX = clamp(this.offsetX, this.viewWidth - scaledWidth - margin, margin);
    }
    if (scaledHeight <= this.viewHeight) {
      this.offsetY = (this.viewHeight - scaledHeight) / 2;
    } else {
      this.offsetY = clamp(this.offsetY, this.viewHeight - scaledHeight - margin, margin);
    }
  }

  checkerboard() {
    if (this.checkerPattern) return this.checkerPattern;
    const tile = createCanvas(CHECKER_SIZE * 2, CHECKER_SIZE * 2);
    const ctx = get2d(tile);
    ctx.fillStyle = CHECKER_LIGHT;
    ctx.fillRect(0, 0, CHECKER_SIZE * 2, CHECKER_SIZE * 2);
    ctx.fillStyle = CHECKER_DARK;
    ctx.fillRect(0, 0, CHECKER_SIZE, CHECKER_SIZE);
    ctx.fillRect(CHECKER_SIZE, CHECKER_SIZE, CHECKER_SIZE, CHECKER_SIZE);
    this.checkerPattern = this.ctx.createPattern(tile, 'repeat');
    return this.checkerPattern;
  }

  /** Add a transient layer drawn above the document (pending stroke/preview). */
  setLayers(layers) {
    this.layers = layers;
  }

  render() {
    const ctx = this.ctx;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = 'source-over';
    ctx.clearRect(0, 0, this.viewWidth, this.viewHeight);
    ctx.fillStyle = '#22252a';
    ctx.fillRect(0, 0, this.viewWidth, this.viewHeight);
    if (!this.doc) return;

    const rect = this.imageRect;
    ctx.save();
    ctx.beginPath();
    ctx.rect(rect.x, rect.y, rect.width, rect.height);
    ctx.clip();

    const pattern = this.checkerboard();
    if (pattern) {
      ctx.save();
      ctx.translate(rect.x, rect.y);
      ctx.fillStyle = pattern;
      ctx.fillRect(0, 0, rect.width, rect.height);
      ctx.restore();
    }

    // Crisp pixels when magnified, smooth resampling when reduced.
    ctx.imageSmoothingEnabled = this.scale < 2;
    ctx.imageSmoothingQuality = 'high';
    ctx.drawImage(this.doc.canvas, rect.x, rect.y, rect.width, rect.height);
    for (const layer of this.layers) {
      if (!layer) continue;
      ctx.globalAlpha = layer.opacity ?? 1;
      ctx.drawImage(layer.canvas, rect.x, rect.y, rect.width, rect.height);
    }
    ctx.globalAlpha = 1;
    ctx.restore();
  }

  /** Interaction layer (crop handles, tool cursor) on top of the image. */
  renderOverlay() {
    const ctx = this.overlayCtx;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = 'source-over';
    ctx.clearRect(0, 0, this.viewWidth, this.viewHeight);
    if (!this.doc) return;
    this.overlay?.(ctx, this);
  }

  /** Redraw image and interaction layers together. */
  drawAll() {
    this.render();
    this.renderOverlay();
  }

  /** Redraw image and overlay on the next animation frame (coalesced). */
  requestRender() {
    if (this.frameHandle) return;
    this.frameHandle = requestAnimationFrame(() => {
      this.frameHandle = 0;
      this.drawAll();
    });
  }

  /** Redraw only the interaction layer (cheap: pointer tracking, crop drag). */
  requestOverlayRender() {
    if (this.overlayFrameHandle) return;
    this.overlayFrameHandle = requestAnimationFrame(() => {
      this.overlayFrameHandle = 0;
      this.renderOverlay();
    });
  }
}
