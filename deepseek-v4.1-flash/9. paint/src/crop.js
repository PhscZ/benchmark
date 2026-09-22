import { createCanvas, get2d } from './util.js';

/**
 * Crop tool: a draggable, resizable rectangle in image coordinates with optional
 * aspect-ratio locking and exact numeric entry.
 *
 * Nothing here touches the document until `apply()` is called, so a preview can
 * always be cancelled without side effects.
 */

export const ASPECT_PRESETS = [
  { id: 'free', label: 'Freeform', ratio: null },
  { id: '1:1', label: '1:1', ratio: 1 },
  { id: '4:3', label: '4:3', ratio: 4 / 3 },
  { id: '3:2', label: '3:2', ratio: 3 / 2 },
  { id: '16:9', label: '16:9', ratio: 16 / 9 },
  { id: '3:4', label: '3:4', ratio: 3 / 4 },
  { id: '2:3', label: '2:3', ratio: 2 / 3 },
  { id: '9:16', label: '9:16', ratio: 9 / 16 },
];

export const HANDLES = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];
export const HANDLE_CURSORS = {
  nw: 'nwse-resize', se: 'nwse-resize', ne: 'nesw-resize', sw: 'nesw-resize',
  n: 'ns-resize', s: 'ns-resize', e: 'ew-resize', w: 'ew-resize',
  move: 'move', new: 'crosshair',
};

const MIN_SIZE = 1;
const HANDLE_HIT_PX = 9;

export class CropSession {
  constructor() {
    /** @type {{x:number,y:number,width:number,height:number}|null} image coordinates */
    this.rect = null;
    /** @type {number|null} width / height ratio, null for freeform */
    this.aspect = null;
    this.dragMode = null;
    this.dragAnchor = null;
    this.dragOrigin = null;
  }

  get active() { return this.rect !== null; }

  /** Select the whole document. */
  reset(doc) {
    this.rect = { x: 0, y: 0, width: doc.width, height: doc.height };
  }

  clear() {
    this.rect = null;
    this.dragMode = null;
  }

  /** @param {string} presetId */
  setAspectPreset(presetId, doc) {
    const preset = ASPECT_PRESETS.find((p) => p.id === presetId) ?? ASPECT_PRESETS[0];
    this.aspect = preset.ratio;
    if (this.rect && this.aspect) this.fitAspect(doc);
  }

  /** Resize the current rect to the locked ratio, anchored at its centre. */
  fitAspect(doc) {
    const centreX = this.rect.x + this.rect.width / 2;
    const centreY = this.rect.y + this.rect.height / 2;
    let width = this.rect.width;
    let height = width / this.aspect;
    if (height > this.rect.height) {
      height = this.rect.height;
      width = height * this.aspect;
    }
    this.rect = { x: centreX - width / 2, y: centreY - height / 2, width, height };
    this.clamp(doc);
  }

  /** Keep the rectangle inside the document and at least MIN_SIZE in both axes. */
  clamp(doc) {
    if (!this.rect) return;
    let { x, y, width, height } = this.rect;
    width = Math.min(Math.max(width, MIN_SIZE), doc.width);
    height = Math.min(Math.max(height, MIN_SIZE), doc.height);
    x = Math.min(Math.max(x, 0), doc.width - width);
    y = Math.min(Math.max(y, 0), doc.height - height);
    this.rect = { x, y, width, height };
  }

  /**
   * @param {{x:number,y:number}} point image coordinates
   * @param {number} tolerance image-space hit radius for handles
   * @param {{width:number,height:number}} [doc] when the selection covers the whole
   *   document a drag inside it can only mean "start a new selection", because
   *   moving a full-size selection is impossible
   * @returns {'move'|'new'|'nw'|'n'|'ne'|'e'|'se'|'s'|'sw'|'w'}
   */
  hitTest(point, tolerance, doc) {
    if (!this.rect) return 'new';
    const { x, y, width, height } = this.rect;
    const near = (px, py) => Math.abs(point.x - px) <= tolerance && Math.abs(point.y - py) <= tolerance;
    if (near(x, y)) return 'nw';
    if (near(x + width, y)) return 'ne';
    if (near(x, y + height)) return 'sw';
    if (near(x + width, y + height)) return 'se';
    if (Math.abs(point.y - y) <= tolerance && point.x > x && point.x < x + width) return 'n';
    if (Math.abs(point.y - (y + height)) <= tolerance && point.x > x && point.x < x + width) return 's';
    if (Math.abs(point.x - x) <= tolerance && point.y > y && point.y < y + height) return 'w';
    if (Math.abs(point.x - (x + width)) <= tolerance && point.y > y && point.y < y + height) return 'e';
    if (point.x > x && point.x < x + width && point.y > y && point.y < y + height) {
      const coversDocument = doc && width >= doc.width && height >= doc.height;
      return coversDocument ? 'new' : 'move';
    }
    return 'new';
  }

  /** The point that stays fixed while `mode` is dragged. */
  fixedPoint(mode) {
    const { x, y, width, height } = this.rect;
    switch (mode) {
      case 'nw': return { x: x + width, y: y + height };
      case 'ne': return { x, y: y + height };
      case 'sw': return { x: x + width, y };
      case 'se': return { x, y };
      case 'n': return { x: x + width / 2, y: y + height };
      case 's': return { x: x + width / 2, y };
      case 'w': return { x: x + width, y: y + height / 2 };
      case 'e': return { x, y: y + height / 2 };
      default: return { x, y };
    }
  }

  /** @param {string} mode @param {{x:number,y:number}} point */
  beginDrag(mode, point, doc) {
    this.dragMode = mode;
    this.dragOrigin = { x: point.x, y: point.y };
    if (mode === 'new') {
      this.rect = { x: point.x, y: point.y, width: MIN_SIZE, height: MIN_SIZE };
      this.dragAnchor = { x: point.x, y: point.y };
    } else if (mode === 'move') {
      this.dragAnchor = { x: this.rect.x, y: this.rect.y };
    } else {
      this.dragAnchor = this.fixedPoint(mode);
    }
    this.clamp(doc);
  }

  /** @param {{x:number,y:number}} point @param {boolean} keepAspect */
  drag(point, doc, keepAspect = true) {
    if (!this.dragMode) return;
    const mode = this.dragMode;
    const anchor = this.dragAnchor;

    if (mode === 'move') {
      const dx = point.x - this.dragOrigin.x;
      const dy = point.y - this.dragOrigin.y;
      this.rect = {
        x: this.dragAnchor.x + dx,
        y: this.dragAnchor.y + dy,
        width: this.rect.width,
        height: this.rect.height,
      };
      this.clamp(doc);
      return;
    }

    const useAspect = keepAspect && this.aspect;
    const signX = mode.includes('e') ? 1 : mode.includes('w') ? -1 : 0;
    const signY = mode.includes('s') ? 1 : mode.includes('n') ? -1 : 0;
    const dx = (point.x - anchor.x) * (signX || 1);
    const dy = (point.y - anchor.y) * (signY || 1);

    let width;
    let height;
    if (mode === 'new') {
      width = Math.abs(point.x - anchor.x);
      height = Math.abs(point.y - anchor.y);
      if (useAspect) {
        if (width >= height * this.aspect) height = width / this.aspect;
        else width = height * this.aspect;
      }
    } else if (signX && signY) {
      width = dx;
      height = useAspect ? width / this.aspect : dy;
    } else if (signX) {
      width = dx;
      height = useAspect ? width / this.aspect : this.rect.height;
    } else {
      height = dy;
      width = useAspect ? height * this.aspect : this.rect.width;
    }

    width = Math.max(width, MIN_SIZE);
    height = Math.max(height, MIN_SIZE);

    // Scale down proportionally if the rectangle would not fit in the document.
    const fit = Math.min(1, doc.width / width, doc.height / height);
    if (fit < 1) {
      width *= fit;
      height *= fit;
    }

    let x;
    let y;
    if (mode === 'new') {
      x = point.x >= anchor.x ? anchor.x : anchor.x - width;
      y = point.y >= anchor.y ? anchor.y : anchor.y - height;
    } else if (signX && signY) {
      x = signX > 0 ? anchor.x : anchor.x - width;
      y = signY > 0 ? anchor.y : anchor.y - height;
    } else if (signX) {
      x = signX > 0 ? anchor.x : anchor.x - width;
      y = anchor.y - height / 2;
    } else {
      x = anchor.x - width / 2;
      y = signY > 0 ? anchor.y : anchor.y - height;
    }

    this.rect = { x, y, width, height };
    this.clamp(doc);
  }

  endDrag() {
    this.dragMode = null;
    this.dragAnchor = null;
    this.dragOrigin = null;
  }

  /** Integer pixel rectangle used for applying the crop. */
  integerRect() {
    if (!this.rect) return null;
    return {
      x: Math.round(this.rect.x),
      y: Math.round(this.rect.y),
      width: Math.round(this.rect.width),
      height: Math.round(this.rect.height),
    };
  }

  /**
   * Validate the current rectangle against the document.
   * @returns {{ok:boolean,error:string|null,rect:{x:number,y:number,width:number,height:number}|null}}
   */
  validate(doc) {
    if (!this.rect) return { ok: false, error: 'Drag a crop rectangle first.', rect: null };
    const rect = this.integerRect();
    if (rect.width < 1 || rect.height < 1) {
      return { ok: false, error: 'Crop width and height must be at least 1 pixel.', rect };
    }
    if (rect.x < 0 || rect.y < 0) {
      return { ok: false, error: 'Crop x and y must be 0 or greater.', rect };
    }
    if (rect.x + rect.width > doc.width) {
      return { ok: false, error: `Crop x (${rect.x}) + width (${rect.width}) exceeds the image width (${doc.width}).`, rect };
    }
    if (rect.y + rect.height > doc.height) {
      return { ok: false, error: `Crop y (${rect.y}) + height (${rect.height}) exceeds the image height (${doc.height}).`, rect };
    }
    return { ok: true, error: null, rect };
  }

  /**
   * Set the rectangle from exact numeric values, reporting validation errors.
   * @returns {{ok:boolean,error:string|null}}
   */
  setNumeric(doc, values) {
    for (const field of ['x', 'y', 'width', 'height']) {
      const value = values[field];
      if (!Number.isFinite(value) || !Number.isInteger(value)) {
        return { ok: false, error: `Crop ${field} must be a whole number of pixels.` };
      }
    }
    if (values.width < 1 || values.height < 1) {
      return { ok: false, error: 'Crop width and height must be at least 1 pixel.' };
    }
    if (values.x < 0 || values.y < 0) {
      return { ok: false, error: 'Crop x and y must be 0 or greater.' };
    }
    if (values.x + values.width > doc.width) {
      return { ok: false, error: `Crop x (${values.x}) + width (${values.width}) exceeds the image width (${doc.width}).` };
    }
    if (values.y + values.height > doc.height) {
      return { ok: false, error: `Crop y (${values.y}) + height (${values.height}) exceeds the image height (${doc.height}).` };
    }
    if (this.aspect && Math.abs(values.width / values.height - this.aspect) > 0.01) {
      return { ok: false, error: `Crop size ${values.width}x${values.height} does not match the locked aspect ratio.` };
    }
    this.rect = { x: values.x, y: values.y, width: values.width, height: values.height };
    return { ok: true, error: null };
  }

  /**
   * Apply the crop by resizing the document in place.
   * @returns {{ok:boolean,error:string|null,width:number,height:number}}
   */
  apply(doc) {
    const check = this.validate(doc);
    if (!check.ok) return { ok: false, error: check.error, width: doc.width, height: doc.height };
    const { x, y, width, height } = check.rect;
    const target = createCanvas(width, height);
    const ctx = get2d(target);
    ctx.imageSmoothingEnabled = false;
    ctx.drawImage(doc.canvas, x, y, width, height, 0, 0, width, height);
    doc.replaceWith(target);
    this.rect = null;
    return { ok: true, error: null, width, height };
  }

  /**
   * Screen-space overlay: dimmed surround, outline, rule-of-thirds grid, handles.
   * @param {CanvasRenderingContext2D} ctx @param {import('./viewport.js').Viewport} viewport
   */
  drawOverlay(ctx, viewport, showHandles = true) {
    if (!this.rect) return;
    const rect = this.rect;
    const topLeft = viewport.imageToScreen(rect.x, rect.y);
    const bottomRight = viewport.imageToScreen(rect.x + rect.width, rect.y + rect.height);
    const width = bottomRight.x - topLeft.x;
    const height = bottomRight.y - topLeft.y;
    const image = viewport.imageRect;

    ctx.save();
    ctx.beginPath();
    ctx.rect(image.x, image.y, image.width, image.height);
    ctx.rect(topLeft.x, topLeft.y, width, height);
    ctx.fillStyle = 'rgba(0,0,0,0.55)';
    ctx.fill('evenodd');

    ctx.lineWidth = 1;
    ctx.strokeStyle = 'rgba(0,0,0,0.8)';
    ctx.strokeRect(topLeft.x - 0.5, topLeft.y - 0.5, width + 1, height + 1);
    ctx.strokeStyle = '#ffffff';
    ctx.strokeRect(topLeft.x + 0.5, topLeft.y + 0.5, width - 1, height - 1);

    ctx.beginPath();
    for (let i = 1; i <= 2; i++) {
      const gx = topLeft.x + (width * i) / 3;
      const gy = topLeft.y + (height * i) / 3;
      ctx.moveTo(gx, topLeft.y);
      ctx.lineTo(gx, topLeft.y + height);
      ctx.moveTo(topLeft.x, gy);
      ctx.lineTo(topLeft.x + width, gy);
    }
    ctx.strokeStyle = 'rgba(255,255,255,0.45)';
    ctx.stroke();

    if (showHandles) {
      const size = 8;
      const positions = {
        nw: [topLeft.x, topLeft.y], n: [topLeft.x + width / 2, topLeft.y], ne: [topLeft.x + width, topLeft.y],
        e: [topLeft.x + width, topLeft.y + height / 2], se: [topLeft.x + width, topLeft.y + height],
        s: [topLeft.x + width / 2, topLeft.y + height], sw: [topLeft.x, topLeft.y + height],
        w: [topLeft.x, topLeft.y + height / 2],
      };
      ctx.fillStyle = '#ffffff';
      ctx.strokeStyle = 'rgba(0,0,0,0.75)';
      for (const key of HANDLES) {
        const [hx, hy] = positions[key];
        ctx.beginPath();
        ctx.rect(hx - size / 2, hy - size / 2, size, size);
        ctx.fill();
        ctx.stroke();
      }
      ctx.font = '12px system-ui, sans-serif';
      const label = `${Math.round(rect.width)} x ${Math.round(rect.height)} px`;
      const labelX = topLeft.x + 4;
      const labelY = Math.max(image.y + 14, topLeft.y - 8);
      ctx.fillStyle = 'rgba(0,0,0,0.75)';
      ctx.fillText(label, labelX + 1, labelY + 1);
      ctx.fillStyle = '#ffffff';
      ctx.fillText(label, labelX, labelY);
    }
    ctx.restore();
  }

  /** Screen-space handle hit radius expressed in image pixels. */
  static hitTolerance(viewport) {
    return HANDLE_HIT_PX / viewport.scale;
  }
}
